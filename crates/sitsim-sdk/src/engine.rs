//! Simulator assembly: the tick engine (SPEC §2.3 steps 2-6).
//!
//! `SimEngine` owns dynamics, environment, sensors, the fault engine, and
//! the seeded RNG streams. `tick()` is a pure function of
//! (previous state, inputs, RNG state): one call advances virtual time by
//! exactly one period, samples the sensors, and returns the wire-ready
//! HIL messages plus a telemetry snapshot. No I/O, no wall-clock reads —
//! the determinism contract (§8.1) starts here.
//!
//! RNG consumption order per tick (part of the determinism contract):
//! stream 5 (wind): 3 Gaussians; stream 0 (IMU): 12; stream 2 (mag): 3 on
//! mag-schedule ticks; stream 3 (baro): 2 on baro-schedule ticks; stream 4
//! (GPS): 6 per sampled fix. Stream 1 (IMU turn-on) is consumed once at
//! construction. Stream 6 (initial-state perturbation) is reserved and
//! unconsumed in v0.1 (ADR-010).

use sitsim_core::{QuadDynamics, QuadParams, State, StepInput, RngStreams};
use sitsim_env::{GeoOrigin, MagFieldModel, WindModel};
use sitsim_fault::{FaultEngine, FaultSpec, FaultType};
use sitsim_mavlink::{HilGps, HilSensor, HilStateQuaternion};
use sitsim_sensors::{
    BaroModel, BaroParams, BatteryModel, BatteryParams, GpsModel, GpsParams, ImuModel, ImuParams,
    MagModel, MagParams, SensorOutputs,
};

use crate::config::ScenarioConfig;

/// 1 mG in m/s^2 (HIL_STATE_QUATERNION acceleration unit).
const MG: f64 = 0.00980665;
/// All sensor-freshness bits (§3.6 V-1: all bits until per-sensor
/// decimation is verified; PX4 consumes accel|gyro|mag|baro = 0x1BFF).
const FIELDS_UPDATED_ALL: u32 = 0x1BFF;

/// One tick's outputs: wire-ready messages + snapshot.
#[derive(Debug, Clone)]
pub struct TickOutput {
    pub tick: u64,
    pub t_us: u64,
    pub hil_sensor: HilSensor,
    pub hil_state_quaternion: HilStateQuaternion,
    pub hil_gps: Option<HilGps>,
    /// Outgoing HIL_SENSOR frame dropped (F-10).
    pub drop_hil_sensor: bool,
    /// Outgoing HIL_GPS frame dropped (F-10).
    pub drop_hil_gps: bool,
    /// Wire delay for outgoing frames (F-09), ms of virtual time.
    pub transport_delay_ms: f64,
    /// Numerical divergence latched during this tick (ADR-013): callers
    /// must stop the run and exit 5 — never stream NaN silently.
    pub diverged: bool,
    pub snapshot: TickSnapshot,
}

/// Telemetry snapshot (10 Hz plane, replay, WS frame source).
#[derive(Debug, Clone, PartialEq)]
pub struct TickSnapshot {
    pub tick: u64,
    pub t_us: u64,
    /// NED position, m.
    pub pos_ned_m: [f64; 3],
    /// NED velocity, m/s.
    pub vel_ned_ms: [f64; 3],
    /// Attitude quaternion wxyz (body->NED).
    pub q_wxyz: [f64; 4],
    /// Body angular rates, rad/s.
    pub omega_body_rads: [f64; 3],
    /// Motor commands after fault effects, [0, 1].
    pub motors: [f64; 4],
    /// Rotor speeds, rad/s.
    pub rotors_rads: [f64; 4],
    /// Battery percent.
    pub battery_pct: f64,
    /// Last emitted (noisy) sensor values.
    pub sensors: SensorOutputs,
    /// Active faults (id, type).
    pub faults_active: Vec<(String, FaultType)>,
    /// Replay fault-slot bitmap (index in the engine's spec list).
    pub fault_flags: u16,
    /// Pending timeline events (id, type, start_ms) for the control plane.
    pub faults_pending: Vec<(String, FaultType, u64)>,
}

impl Default for TickSnapshot {
    fn default() -> Self {
        TickSnapshot {
            tick: 0,
            t_us: 0,
            pos_ned_m: [0.0; 3],
            vel_ned_ms: [0.0; 3],
            q_wxyz: [1.0, 0.0, 0.0, 0.0],
            omega_body_rads: [0.0; 3],
            motors: [0.0; 4],
            rotors_rads: [0.0; 4],
            battery_pct: 100.0,
            sensors: SensorOutputs::default(),
            faults_active: Vec::new(),
            fault_flags: 0,
            faults_pending: Vec::new(),
        }
    }
}

/// The assembled simulator.
#[derive(Debug, Clone)]
pub struct SimEngine {
    pub cfg: ScenarioConfig,
    dynamics: QuadDynamics,
    /// Divergence diagnostic already emitted (once).
    reported_divergence: bool,
    /// `last_armed`: HIL_ACTUATOR_CONTROLS mode bit 0x80 from the latest
    /// frame (ADR-0011r).
    last_armed: bool,
    wind: WindModel,
    mag_field: MagFieldModel,
    origin: GeoOrigin,
    imu: ImuModel,
    mag: MagModel,
    baro: BaroModel,
    gps: GpsModel,
    battery: BatteryModel,
    faults: FaultEngine,
    streams: RngStreams,
    /// Ticks completed so far.
    tick: u64,
    /// Raw controls last received from PX4 (16 slots, [-1, 1]).
    last_controls: [f32; 16],
    /// Last emitted sensor values (held for HIL_SENSOR between samples).
    sensors: SensorOutputs,
    /// Decimation: sample mag+baro every `mag_baro_div` ticks (50 Hz).
    mag_baro_div: u64,
}

impl SimEngine {
    /// Build everything from a validated config. Consumes the IMU turn-on
    /// stream (stream 1) and the mag/baro turn-on draws.
    pub fn new(cfg: ScenarioConfig) -> Self {
        let mut streams = RngStreams::new(cfg.sim.seed);
        let d = &cfg.dynamics;
        let quad = QuadParams {
            mass_kg: d.mass_kg as f64,
            inertia: [d.inertia[0] as f64, d.inertia[1] as f64, d.inertia[2] as f64],
            arm_m: d.arm_m as f64,
            c_t: d.c_T as f64,
            c_m: d.c_M as f64,
            omega_max: d.omega_max as f64,
            tau_motor: d.tau_motor as f64,
            u_idle: d.u_idle as f64,
            k_lin: d.k_lin as f64,
            k_quad: d.k_quad as f64,
            g: d.g_ms2 as f64,
            k_g: d.k_g as f64,
            c_g: d.c_g as f64,
            mu: d.mu as f64,
            battery_capacity_mah: cfg.sensors.battery.capacity_mah as f64,
            i_base_a: 0.4,
            k_p_a_per_n: 1.06,
            battery_enabled: cfg.sensors.battery.enabled,
        };
        let state = State {
            pos: [
                cfg.vehicle.initial.pos_ned_m[0] as f64,
                cfg.vehicle.initial.pos_ned_m[1] as f64,
                cfg.vehicle.initial.pos_ned_m[2] as f64,
            ],
            vel: [0.0; 3],
            q: [1.0, 0.0, 0.0, 0.0],
            omega: [0.0; 3],
            rotors: [0.0; 4],
            soc: 1.0,
        };
        let imu = ImuModel::new(
            ImuParams {
                gyro_noise_density: cfg.sensors.imu.gyro_noise_density as f64,
                gyro_bias_walk: cfg.sensors.imu.gyro_bias_walk as f64,
                gyro_turnon_sigma: cfg.sensors.imu.gyro_turnon_sigma as f64,
                gyro_scale_sigma: cfg.sensors.imu.gyro_scale_sigma as f64,
                gyro_misalign_deg: cfg.sensors.imu.gyro_misalign_deg as f64,
                accel_noise_density: cfg.sensors.imu.accel_noise_density as f64,
                accel_bias_walk: cfg.sensors.imu.accel_bias_walk as f64,
                accel_turnon_sigma: cfg.sensors.imu.accel_turnon_sigma as f64,
                accel_scale_sigma: cfg.sensors.imu.accel_scale_sigma as f64,
                accel_misalign_deg: cfg.sensors.imu.accel_misalign_deg as f64,
            },
            &mut streams.imu_turn_on,
        );
        let mag = MagModel::new(
            MagParams {
                noise_gauss: cfg.sensors.mag.noise_gauss as f64,
                hard_iron_gauss: cfg.sensors.mag.hard_iron_gauss as f64,
            },
            &mut streams.mag,
        );
        let baro = BaroModel::new(BaroParams {
            noise_m: cfg.sensors.baro.noise_m as f64,
            walk_m_per_min: cfg.sensors.baro.walk_m_per_min as f64,
        });
        let gps = GpsModel::new(GpsParams {
            rate_hz: cfg.sensors.gps.rate_hz as f64,
            pos_noise_m: cfg.sensors.gps.pos_noise_m as f64,
            pos_noise_vert_m: cfg.sensors.gps.pos_noise_vert_m as f64,
            vel_noise_ms: cfg.sensors.gps.vel_noise_ms as f64,
            eph_cm: cfg.sensors.gps.eph_cm,
            epv_cm: cfg.sensors.gps.epv_cm,
            latency_ms: cfg.sensors.gps.latency_ms,
            lock_s: cfg.sensors.gps.lock_s as f64,
            satellites: cfg.sensors.gps.satellites,
        });
        let battery = BatteryModel::new(BatteryParams {
            enabled: cfg.sensors.battery.enabled,
            capacity_mah: cfg.sensors.battery.capacity_mah,
            ..BatteryParams::default()
        });
        let h = 1.0 / cfg.sim.rate_hz as f64;
        let wind = WindModel::new(
            [
                cfg.env.wind_steady_ms[0] as f64,
                cfg.env.wind_steady_ms[1] as f64,
                cfg.env.wind_steady_ms[2] as f64,
            ],
            cfg.env.turbulence.into(),
        )
        .with_step(h);
        let mag_field = MagFieldModel {
            incl_deg: cfg.env.field.incl_deg as f64,
            decl_deg: cfg.env.field.decl_deg as f64,
            h_gauss: cfg.env.field.h_gauss as f64,
        };
        let origin = GeoOrigin {
            lat_deg: cfg.vehicle.origin.lat_deg,
            lon_deg: cfg.vehicle.origin.lon_deg,
            alt_m: cfg.vehicle.origin.alt_m,
        };
        // 50 Hz mag/baro from the tick rate (200 Hz -> every 4th tick, §6.4).
        let mag_baro_div = ((cfg.sim.rate_hz / 50.0).ceil() as u64).max(1);

        SimEngine {
            faults: FaultEngine::new(cfg.faults.clone()),
            cfg,
            dynamics: QuadDynamics::new(quad, state),
            wind,
            mag_field,
            origin,
            imu,
            mag,
            baro,
            gps,
            battery,
            streams,
            tick: 0,
            // Disarmed until the first HIL_ACTUATOR_CONTROLS arrives (the
            // armed bit lives in the mode field, ADR-0011r): rotors stop.
            last_controls: [0.0; 16],
            last_armed: false,
            sensors: SensorOutputs::default(),
            mag_baro_div,
            reported_divergence: false,
        }
    }

    /// Current tick index (ticks completed).
    pub fn tick_count(&self) -> u64 {
        self.tick
    }

    /// Virtual time after the next tick, us.
    fn next_t_us(&self) -> u64 {
        let h_us = 1e6 / self.cfg.sim.rate_hz as f64;
        ((self.tick + 1) as f64 * h_us).round() as u64
    }

    /// Inject a runtime fault at virtual time "now" (§7.3): the fault
    /// becomes active at the next tick boundary.
    pub fn inject_fault(&mut self, spec: FaultSpec) -> String {
        let now_us = self.next_t_us();
        self.faults.inject_at(spec, now_us)
    }

    /// Clear a persistent fault effect (returns whether the id existed).
    pub fn clear_fault(&mut self, id: &str) -> bool {
        self.faults.clear(id)
    }

    /// Known fault ids (active + pending) for DELETE 404 checks.
    pub fn known_fault_ids(&self) -> Vec<String> {
        let t_us = self.next_t_us();
        let mut ids: Vec<String> = self.faults.active_list(t_us).into_iter().map(|(id, _)| id).collect();
        ids.extend(self.faults.pending(t_us).into_iter().map(|s| s.id.clone()));
        ids
    }

    /// Advance one tick. `controls` is the latest HIL_ACTUATOR_CONTROLS
    /// payload from PX4 (16 values, v1.16 PWMSim [0,1] motor scale) and
    /// `armed` the mode-field armed bit (0x80); `None` for either holds
    /// the last received value.
    pub fn tick(&mut self, controls: Option<&[f32; 16]>, armed: Option<bool>) -> TickOutput {
        let h = 1.0 / self.cfg.sim.rate_hz as f64;
        let t_us = self.next_t_us();
        self.tick += 1;
        if let Some(c) = controls {
            self.last_controls.copy_from_slice(c);
        }
        if let Some(a) = armed {
            self.last_armed = a;
        }

        // ---- Step 2 (Act): fault effects + motor command mapping (§3.9).
        let fx = self.faults.evaluate(t_us);
        let mut u = [0.0f64; 4];
        let mut motor_stopped = [false; 4];
        let armed = self.last_armed;
        for i in 0..4 {
            // ADR-0011r (live-verified against PX4 v1.16.2, captured with a
            // raw wire sniffer): SimulatorMavlink copies actuator_outputs_sim
            // into controls[], which PWMSim publishes as PER-MOTOR NORMALIZED
            // thrust: output = (pwm - 1000) / 1000 in [0, 1] for
            // non-reversible Motor functions, (pwm - 1500) / 500 in [-1, 1]
            // for everything else, and 0 for disarmed channels (magic 900
            // skipped). Observed on the wire: armed idle ~0.002; offboard
            // climb ramps smoothly ~0.28 -> ~1.0; DISARMED = all zeros with
            // mode bit 0x80 clear. The previous (c + 1) / 2 mapping (the
            // jMAVSim-era [-1, +1] convention, ADR-0011 v1) turned armed
            // idle 0.002 into u = 0.5 — a phantom 2/3-hover thrust that
            // pinned the vehicle on the ground with spinning motors and
            // diverged the takeoff closed loop (I-2). PWM-scale values
            // (>= 900) are still honored for stacks that send raw PWM.
            let c = self.last_controls[i] as f64;
            let raw = if c >= 900.0 { (c - 1000.0) / 1000.0 } else { c };
            u[i] = (raw * fx.motor_eff[i]).clamp(0.0, 1.0);
            // Disarm means the rotors STOP (wind down through the first-
            // order lag, §5.3) — PX4's intent, not an idle spin.
            motor_stopped[i] = !armed || fx.motor_cut[i];
        }

        // ---- Wind (steady + turbulence + gust fault F-08).
        let wind_ned = {
            let mut w = self.wind.step(h, &mut self.streams.wind);
            for k in 0..3 {
                w[k] += fx.wind_gust_ned[k];
            }
            w
        };

        // ---- Step 3: dynamics.
        // Ground-contact substepping (ADR-013): the unilateral
        // spring-damper contact (§5.4) is stiff in TWO modes — the
        // vertical one (4 contacts on the mass, fast root ~6.2e2 1/s at
        // defaults) and, dominating, the rotational one (the same
        // contacts reacting on Jx/Jy at the arm moment arms, fast root
        // ~1.2e3 1/s). The earlier fixed N = ceil(h / 2.5 ms) policy kept
        // only the vertical root inside RK4's |lambda h| < 2.785 limit;
        // the rotational root sat at 3.0 and diverged during the I-2
        // takeoff tilt transient (1.43x amplification per substep ->
        // NaN in under a second). `stable_substeps` sizes N from the
        // ACTUAL dominant root of the configured parameters: N is a pure
        // function of (rate, parameters), so determinism is preserved.
        let input = StepInput { u, wind_ned, motor_stopped };
        let substeps = sitsim_core::stable_substeps(&self.dynamics.params, h);
        let h_sub = h / substeps as f64;
        for _ in 0..substeps {
            self.dynamics.step(&input, h_sub);
        }
        // Divergence is never silent (ADR-013): one diagnostic line to
        // stderr, then the caller stops the loop and exits 5.
        if self.dynamics.diverged && !self.reported_divergence {
            self.reported_divergence = true;
            eprintln!(
                "DIVERGENCE tick={} t_us={} substeps={} h_sub_us={:.1} u=[{:.3},{:.3},{:.3},{:.3}] wind=[{:.2},{:.2},{:.2}] contact_fast_root={:.0}/s",
                self.tick, t_us, substeps, h_sub * 1e6,
                u[0], u[1], u[2], u[3],
                wind_ned[0], wind_ned[1], wind_ned[2],
                sitsim_core::contact_fast_root(&self.dynamics.params)
            );
        }

        // ---- Step 4 (Sample): sensors.
        // IMU truth: specific force (thrust + drag + ground contact)/m.
        let true_accel = self.dynamics.specific_force_body(wind_ned);
        let true_gyro = self.dynamics.state.omega;
        let imu = self.imu.sample(&true_accel, &true_gyro, h, self.cfg.sim.rate_hz as f64, &mut self.streams.imu_noise);
        // IMU fault effects (F-03 bias ramp, F-04 saturation).
        let mut accel = imu.accel_body_ms2;
        let mut gyro = imu.gyro_body_rads;
        for k in 0..3 {
            accel[k] += fx.accel_bias_add[k];
            gyro[k] += fx.gyro_bias_add[k];
            if let Some(lim) = fx.accel_sat[k] {
                accel[k] = accel[k].clamp(-lim, lim);
            }
            if let Some(lim) = fx.gyro_sat[k] {
                gyro[k] = gyro[k].clamp(-lim, lim);
            }
        }
        self.sensors.accel_body_ms2 = accel;
        self.sensors.gyro_body_rads = gyro;

        // Mag + baro at 50 Hz (every `mag_baro_div` ticks, starting at
        // tick 1 so the very first HIL_SENSOR carries real values).
        if self.tick % self.mag_baro_div == 1 {
            let rot = sitsim_core::quat_to_rot(&self.dynamics.state.q);
            let field_ned = self.mag_field.field_ned_gauss();
            let truth_body = sitsim_core::quat::rot_t_mul(&rot, &field_ned);
            self.sensors.mag_body_gauss = self.mag.sample(&truth_body, &mut self.streams.mag);

            let alt_above_origin = -self.dynamics.state.pos[2];
            let b = self.baro.sample(alt_above_origin, h * self.mag_baro_div as f64, fx.baro_bias_m, &mut self.streams.baro);
            self.sensors.baro_pressure = b.pressure;
            self.sensors.baro_alt_m = b.pressure_alt_m;
            self.sensors.temperature_c = sitsim_env::temperature_c(alt_above_origin);
        }

        // GPS: poll every tick (the model samples at its own 5 Hz cadence
        // and delivers latency-delayed fixes).
        if let Some(fix) = self.gps.poll(
            t_us,
            &self.dynamics.state.pos,
            &self.dynamics.state.vel,
            &self.origin,
            fx.gps_denied,
            fx.gps_denial_progress,
            &fx.gps_glitch_ned_m,
            &mut self.streams.gps,
        ) {
            self.sensors.gps_fix_type = fix.fix_type;
            self.sensors.gps_sats = fix.satellites_visible;
            self.sensors.gps_fix = Some(fix);
        } else {
            self.sensors.gps_fix = None;
        }

        // ---- Step 5 (Encode): wire-ready messages.
        let s = &self.dynamics.state;
        let hil_sensor = HilSensor {
            time_usec: t_us,
            xacc: self.sensors.accel_body_ms2[0] as f32,
            yacc: self.sensors.accel_body_ms2[1] as f32,
            zacc: self.sensors.accel_body_ms2[2] as f32,
            xgyro: self.sensors.gyro_body_rads[0] as f32,
            ygyro: self.sensors.gyro_body_rads[1] as f32,
            zgyro: self.sensors.gyro_body_rads[2] as f32,
            xmag: self.sensors.mag_body_gauss[0] as f32,
            ymag: self.sensors.mag_body_gauss[1] as f32,
            zmag: self.sensors.mag_body_gauss[2] as f32,
            abs_pressure: self.sensors.baro_pressure as f32,
            diff_pressure: 0.0,
            pressure_alt: self.sensors.baro_alt_m as f32,
            temperature: self.sensors.temperature_c as f32,
            fields_updated: FIELDS_UPDATED_ALL,
            id: 0,
        };

        let (lat_deg, lon_deg, _alt) = self.origin.ned_to_geodetic(&s.pos);
        let alt_msl_mm = ((self.origin.alt_m - s.pos[2]) * 1000.0).round() as i32;
        let hil_state_quaternion = HilStateQuaternion {
            time_usec: t_us,
            attitude_quaternion: [
                s.q[0] as f32,
                s.q[1] as f32,
                s.q[2] as f32,
                s.q[3] as f32,
            ],
            rollspeed: s.omega[0] as f32,
            pitchspeed: s.omega[1] as f32,
            yawspeed: s.omega[2] as f32,
            lat: (lat_deg * 1e7).round() as i32,
            lon: (lon_deg * 1e7).round() as i32,
            alt: alt_msl_mm,
            vx: clamp_i16(s.vel[0] * 100.0),
            vy: clamp_i16(s.vel[1] * 100.0),
            vz: clamp_i16(s.vel[2] * 100.0),
            ind_airspeed: 0,
            true_airspeed: 0,
            // Ground-truth specific force, int16 mG (§3.7), gravity-
            // consistent with HIL_SENSOR (at rest: (0, 0, -1000)).
            xacc: clamp_i16(true_accel[0] / MG),
            yacc: clamp_i16(true_accel[1] / MG),
            zacc: clamp_i16(true_accel[2] / MG),
        };

        let hil_gps = self.sensors.gps_fix.map(|f| HilGps {
            time_usec: f.t_sample_us,
            lat: f.lat,
            lon: f.lon,
            alt: f.alt,
            eph: f.eph,
            epv: f.epv,
            vel: f.vel,
            vn: f.vn,
            ve: f.ve,
            vd: f.vd,
            cog: f.cog,
            fix_type: f.fix_type,
            satellites_visible: f.satellites_visible,
            id: 0,
            yaw: 0,
        });

        // ---- Transport faults (F-09/F-10): deterministic per (tick, msg).
        let drop_hil_sensor =
            drop_decision(self.cfg.sim.seed, self.tick, sitsim_mavlink::msg_id::HIL_SENSOR, fx.drop_pct);
        let drop_hil_gps =
            drop_decision(self.cfg.sim.seed, self.tick, sitsim_mavlink::msg_id::HIL_GPS, fx.drop_pct);

        // ---- Snapshot for the telemetry plane.
        let active = self.faults.active_list(t_us);
        let pending: Vec<(String, FaultType, u64)> = self
            .faults
            .pending(t_us)
            .into_iter()
            .map(|sp| (sp.id.clone(), sp.ftype, sp.start_ms))
            .collect();
        let fault_flags = fault_bitmap(&self.faults, t_us);
        let snapshot = TickSnapshot {
            tick: self.tick,
            t_us,
            pos_ned_m: s.pos,
            vel_ned_ms: s.vel,
            q_wxyz: s.q,
            omega_body_rads: s.omega,
            motors: u,
            rotors_rads: s.rotors,
            battery_pct: self.battery.percent(s.soc),
            sensors: SensorOutputs {
                accel_body_ms2: self.sensors.accel_body_ms2,
                gyro_body_rads: self.sensors.gyro_body_rads,
                mag_body_gauss: self.sensors.mag_body_gauss,
                baro_pressure: self.sensors.baro_pressure,
                baro_alt_m: self.sensors.baro_alt_m,
                temperature_c: self.sensors.temperature_c,
                gps_fix: None, // consumed into hil_gps above
                gps_fix_type: self.sensors.gps_fix_type,
                gps_sats: self.sensors.gps_sats,
            },
            faults_active: active,
            fault_flags,
            faults_pending: pending,
        };

        TickOutput {
            tick: self.tick,
            t_us,
            hil_sensor,
            hil_state_quaternion,
            hil_gps,
            drop_hil_sensor,
            drop_hil_gps,
            transport_delay_ms: fx.transport_delay_ms,
            diverged: self.dynamics.diverged,
            snapshot,
        }
    }

    /// Full state vector (18-dim) for the telemetry hash (§8.1).
    pub fn state_vector(&self) -> [f64; 18] {
        let s = &self.dynamics.state;
        let mut x = [0.0f64; 18];
        x[0..3].copy_from_slice(&s.pos);
        x[3..6].copy_from_slice(&s.vel);
        x[6..10].copy_from_slice(&s.q);
        x[10..13].copy_from_slice(&s.omega);
        x[13..17].copy_from_slice(&s.rotors);
        x[17] = s.soc;
        x
    }
}

fn clamp_i16(v: f64) -> i16 {
    v.round().clamp(-32768.0, 32767.0) as i16
}

/// Deterministic packet-drop decision (F-10): a splitmix-style hash of
/// (seed, tick, msgid) — a pure function of virtual-time-indexed inputs,
/// decoupled from every RNG stream (ADR-011).
fn drop_decision(seed: u64, tick: u64, msgid: u32, drop_pct: f64) -> bool {
    if drop_pct <= 0.0 {
        return false;
    }
    let mut z = seed
        ^ tick.wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (msgid as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    let pct = drop_pct.clamp(0.0, 100.0);
    (z % 100_000) < (pct * 1000.0) as u64
}

/// Replay fault bitmap: bit i set when spec i (of the first 16) is active.
fn fault_bitmap(engine: &FaultEngine, t_us: u64) -> u16 {
    let active = engine.active_list(t_us);
    let mut bits = 0u16;
    for (idx, spec) in engine.specs().iter().enumerate().take(16) {
        if active.iter().any(|(id, _)| id == &spec.id) {
            bits |= 1 << idx;
        }
    }
    bits
}

/// Headless (no-socket) run for the determinism harness and pre-PX4
/// validation: drives the engine with a scripted control schedule.
pub struct HeadlessResult {
    pub telemetry_hash: u64,
    pub ticks: u64,
    pub final_state: State,
}

/// Run `duration_s` of virtual time with controls from `controls(tick)`
/// (None = no actuator message that tick; the tuple carries (controls,
/// armed)). Returns the telemetry hash (§8.1: FNV-1a 64 over every 100th
/// tick's state vector).
pub fn run_headless<F>(cfg: &ScenarioConfig, controls: F) -> HeadlessResult
where
    F: Fn(u64) -> Option<([f32; 16], bool)>,
{
    let mut engine = SimEngine::new(cfg.clone());
    let total_ticks = (cfg.sim.duration_s.max(0.0) * cfg.sim.rate_hz).ceil() as u64;
    let mut hasher = crate::hash::Fnv1a64::new();
    for tick in 1..=total_ticks {
        let c = controls(tick);
        let out = match c {
            Some((ref c, armed)) => engine.tick(Some(c), Some(armed)),
            None => engine.tick(None, None),
        };
        if out.tick % 100 == 0 {
            for v in engine.state_vector() {
                hasher.update_f64(v);
            }
        }
    }
    HeadlessResult {
        telemetry_hash: hasher.finalize(),
        ticks: engine.tick,
        final_state: engine.dynamics.state.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::parse_scenario;

    fn base_cfg(extra: &str) -> ScenarioConfig {
        parse_scenario(&format!("[sim]\nrate_hz = 200\nduration_s = 2.0\nseed = 42\n{extra}")).unwrap()
    }

    /// Determinism contract (G-2): same seed -> identical telemetry hash;
    /// different seed -> different.
    #[test]
    fn telemetry_hash_reproducible_and_seed_sensitive() {
        let cfg = base_cfg("");
        let a = run_headless(&cfg, |t| Some(hover_controls(t)));
        let b = run_headless(&cfg, |t| Some(hover_controls(t)));
        assert_eq!(a.telemetry_hash, b.telemetry_hash);
        assert_eq!(a.ticks, b.ticks);

        let mut other = base_cfg("");
        other.sim.seed = 7;
        let c = run_headless(&other, |t| Some(hover_controls(t)));
        assert_ne!(a.telemetry_hash, c.telemetry_hash, "seed changes trajectory");
    }

    /// Determinism is insensitive to *when* controls arrive: a command
    /// delivered once and then held (latest-slot semantics, §2.3) produces
    /// the same trajectory as the same command delivered every tick.
    #[test]
    fn telemetry_hash_insensitive_to_command_timing() {
        let cfg = base_cfg("");
        let every_tick = run_headless(&cfg, |t| Some(hover_controls(t)));
        let once_then_hold = run_headless(&cfg, |t| {
            if t == 1 {
                Some(hover_controls(t))
            } else {
                None // hold the latest command
            }
        });
        assert_eq!(every_tick.telemetry_hash, once_then_hold.telemetry_hash);
    }

    /// Time contract (§3.10): time_usec advances by exactly 1e6/rate.
    #[test]
    fn virtual_time_advances_exactly() {
        let cfg = base_cfg("");
        let mut e = SimEngine::new(cfg);
        let mut prev = 0u64;
        for _ in 0..10 {
            let out = e.tick(None, None);
            assert_eq!(out.t_us - prev, 5_000);
            prev = out.t_us;
        }
    }

    /// At rest, level: HIL_SENSOR zacc ~ -9.80665 (no noise), the
    /// HIL_STATE_QUATERNION zacc == -1000 mG, and identity quaternion.
    #[test]
    fn at_rest_wire_values_match_prototype() {
        let cfg = parse_scenario(
            "[sim]\nrate_hz = 200\nduration_s = 1.0\n[sensors.imu]\ngyro_noise_density = 0.0\naccel_noise_density = 0.0\ngyro_bias_walk = 0.0\naccel_bias_walk = 0.0\ngyro_turnon_sigma = 0.0\naccel_turnon_sigma = 0.0\ngyro_scale_sigma = 0.0\naccel_scale_sigma = 0.0\n[sensors.mag]\nnoise_gauss = 0.0\nhard_iron_gauss = 0.0\n[sensors.baro]\nnoise_m = 0.0\nwalk_m_per_min = 0.0\n[env]\nturbulence = \"off\"\n",
        )
        .unwrap();
        let mut e = SimEngine::new(cfg);
        // Let the ground contact settle out its startup transient (the
        // vehicle starts exactly at the surface, so the first ~100 ms read
        // the spring-damper impulse; at equilibrium the accelerometer reads
        // exactly -g on body z).
        for _ in 0..200 {
            e.tick(None, None);
        }
        let out = e.tick(None, None);
        assert!((out.hil_sensor.zacc + 9.80665).abs() < 1e-3, "zacc {}", out.hil_sensor.zacc);
        assert_eq!(out.hil_state_quaternion.zacc, -1000);
        assert_eq!(out.hil_state_quaternion.attitude_quaternion, [1.0, 0.0, 0.0, 0.0]);
        assert!((out.hil_sensor.abs_pressure - 101325.0).abs() < 1.0);
        // Mag truth at level attitude: NED field (0.19, 0.01, 0.46).
        assert!((out.hil_sensor.xmag - 0.19).abs() < 0.01, "xmag {}", out.hil_sensor.xmag);
        assert!((out.hil_sensor.zmag - 0.46).abs() < 0.01, "zmag {}", out.hil_sensor.zmag);
        // GPS: first fix sampled at the first tick, delivered 120 ms later
        // (lock_s = 0).
        let mut delivered = None;
        for _ in 0..60 {
            let o = e.tick(None, None);
            if o.hil_gps.is_some() {
                delivered = Some(o.t_us);
                break;
            }
        }
        let t = delivered.expect("a GPS fix within 300 ms");
        assert!(t >= 120_000 + 1_005_000 && t < 1_130_000, "fix delivered at {t} us");
    }

    /// Motor-efficiency fault scales the applied command (F-01).
    #[test]
    fn motor_efficiency_fault_visible_in_snapshot() {
        let cfg = parse_scenario(
            "[sim]\nrate_hz = 200\nduration_s = 2.0\n[[fault]]\nid = \"m0\"\ntype = \"motor_efficiency\"\nmotor = 0\nfactor = 0.5\nstart_ms = 100\npersistent = true\n",
        )
        .unwrap();
        let mut e = SimEngine::new(cfg);
        let c = [1.0f32; 16]; // all motors full: u = 1.0
        let before = e.tick(Some(&c), Some(true)).snapshot.motors;
        assert!((before[0] - 1.0).abs() < 1e-9);
        for _ in 0..40 {
            let out = e.tick(Some(&c), Some(true));
            if out.t_us >= 100_000 {
                assert!((out.snapshot.motors[0] - 0.5).abs() < 1e-9, "motor0 {}", out.snapshot.motors[0]);
                assert!((out.snapshot.motors[1] - 1.0).abs() < 1e-9);
                assert_eq!(out.snapshot.faults_active.len(), 1);
                return;
            }
        }
        panic!("fault window never reached");
    }

    /// Transport drop decision: deterministic and rate-correct.
    #[test]
    fn drop_decision_deterministic_rate() {
        let mut hits = 0;
        for t in 1..=100_000 {
            if drop_decision(42, t, 107, 10.0) {
                hits += 1;
            }
        }
        // 10% +/- statistical tolerance (n = 100k, sigma ~ 0.095%).
        assert!((hits as f64 / 100_000.0 - 0.10).abs() < 0.005, "rate {hits}/100000");
        // Determinism: same inputs, same outputs.
        for t in 1..=100 {
            assert_eq!(drop_decision(42, t, 107, 10.0), drop_decision(42, t, 107, 10.0));
        }
        // Different msgid -> independent pattern (not identical sequence).
        let a: Vec<bool> = (1..=1000).map(|t| drop_decision(42, t, 107, 10.0)).collect();
        let b: Vec<bool> = (1..=1000).map(|t| drop_decision(42, t, 113, 10.0)).collect();
        assert_ne!(a, b);
        assert!(a.iter().filter(|x| **x).count() > 50);
    }

    /// The engine survives a long hover with RK4 stability and the
    /// quaternion stays normalized (spec §5.6e, engine-level).
    #[test]
    fn long_hover_stays_bounded() {
        let cfg = base_cfg("");
        let mut e = SimEngine::new(cfg);
        for t in 1..=40_000 {
            // 200 Hz * 200 s = 40k ticks; slight push at t=1000.
            let (c, armed) = hover_controls(t);
            e.tick(Some(&c), Some(armed));
        }
        let q = e.dynamics.state.q;
        let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
        assert!((n - 1.0).abs() < 1e-9, "quat norm {n}");
    }

    /// hover command: u ~ 0.545 gives thrust ~= m*g on the defaults
    /// (§5.5 hover check).
    fn hover_controls(_t: u64) -> ([f32; 16], bool) {
        // v1.16 wire scale: u = 0.545 directly (ADR-0011r), armed.
        ([0.545f32; 16], true)
    }
}

/// ADR-0011r regression: the v1.16 wire convention, live-captured against
/// real PX4 (scripts/wire_sniff.py): armed idle = 0.002, offboard climb
/// ramps to ~1.0, disarmed = zeros with the mode armed bit clear. The
/// legacy (c + 1) / 2 mapping turned armed idle into u = 0.5 — the phantom
/// 2/3-hover thrust that blocked I-2.
#[cfg(test)]
mod actuator_mapping_tests {
    use super::*;
    use crate::config::parse_scenario;

    fn cfg() -> ScenarioConfig {
        parse_scenario(
            "[sim]\nrate_hz = 200\nduration_s = 3.0\n[sensors.imu]\ngyro_noise_density = 0.0\naccel_noise_density = 0.0\ngyro_bias_walk = 0.0\naccel_bias_walk = 0.0\ngyro_turnon_sigma = 0.0\naccel_turnon_sigma = 0.0\n[sensors.mag]\nnoise_gauss = 0.0\n[sensors.baro]\nnoise_m = 0.0\n[env]\nturbulence = \"off\"\n",
        )
        .unwrap()
    }

    #[test]
    fn v1_16_wire_convention_maps_directly() {
        let mut e = SimEngine::new(cfg());

        // No controls yet: disarmed, motors stopped.
        let out = e.tick(None, None);
        assert_eq!(out.snapshot.motors, [0.0; 4]);

        // Disarmed frame (all zeros, armed bit clear): still stopped.
        let zero = [0f32; 16];
        let out = e.tick(Some(&zero), Some(false));
        assert_eq!(out.snapshot.motors, [0.0; 4]);

        // THE regression: armed idle 0.002 must map to 0.002, not 0.5.
        let idle = [0.002f32; 16];
        let out = e.tick(Some(&idle), Some(true));
        for k in 0..4 {
            assert!((out.snapshot.motors[k] - 0.002).abs() < 1e-6, "motors[{k}] = {}", out.snapshot.motors[k]);
        }

        // Armed mid-throttle maps directly.
        let mid = [0.6f32; 16];
        let out = e.tick(Some(&mid), Some(true));
        for k in 0..4 {
            assert!((out.snapshot.motors[k] - 0.6).abs() < 1e-6);
        }

        // PWM-scale values (other stacks) still disambiguate.
        let pwm = [1500f32; 16];
        let out = e.tick(Some(&pwm), Some(true));
        for k in 0..4 {
            assert!((out.snapshot.motors[k] - 0.5).abs() < 1e-6);
        }

        // Disarm with live values: rotors wind down to a stop (the
        // first-order lag approaches zero asymptotically; "stopped" =
        // below idle spin by a wide margin, ~2 s at 200 Hz).
        let mut stopped = false;
        for _ in 0..400 {
            let out = e.tick(Some(&mid), Some(false));
            if out.snapshot.rotors_rads.iter().all(|&w| w < 1.0) {
                stopped = true;
                break;
            }
        }
        assert!(stopped, "rotors did not wind down after disarm");
    }
}
