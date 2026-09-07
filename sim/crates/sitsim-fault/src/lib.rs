//! Fault injection engine (SPEC §7).
//!
//! The engine is a pure function of (virtual time, event parameters, state):
//! `evaluate(t_us)` walks the event list (timeline + runtime-injected),
//! computes each active event's effect from its elapsed time, and aggregates
//! them into an [`Effects`] snapshot applied by the tick engine before the
//! dynamics step and sensor sampling (all effects act on the model layer,
//! upstream of sensor and HIL emission, SPEC §7.1).
//!
//! Timeline semantics (SPEC §7.2): `start_ms` and (`duration_ms` |
//! `persistent`) define membership; parameters are frozen at event creation;
//! events are idempotent and may overlap. Runtime injection (SPEC §7.3) uses
//! the same schema with `start_ms` interpreted as "now"; persistent runtime
//! faults end at `clear(id)`.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// Fault catalog (SPEC §7.1). IDs F-01..F-10.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FaultType {
    /// F-01: multiplies u_i (motor, factor 0-1).
    MotorEfficiency,
    /// F-02: sets u_i to 0 with rotor wind-down (motor).
    MotorCut,
    /// F-03: ramping bias on gyro or accel output (sensor, axis, rate/s).
    ImuBiasRamp,
    /// F-04: clamps the sensor output (sensor, axis, limit).
    ImuSaturation,
    /// F-05: suppresses GPS fixes, ramps eph (duration).
    GpsDenial,
    /// F-06: constant offset to reported GPS position (offset_m, duration).
    GpsGlitch,
    /// F-07: ramps pressure-altitude bias (rate m/s).
    BaroDrift,
    /// F-08: gust added to steady wind (vector, rise time).
    WindEvent,
    /// F-09: delays HIL frames on the wire (ms).
    TransportDelay,
    /// F-10: drops outgoing sensor frames (percent).
    PacketDrop,
}

impl FaultType {
    /// Catalog id (SPEC §7.1).
    pub fn catalog_id(&self) -> &'static str {
        match self {
            FaultType::MotorEfficiency => "F-01",
            FaultType::MotorCut => "F-02",
            FaultType::ImuBiasRamp => "F-03",
            FaultType::ImuSaturation => "F-04",
            FaultType::GpsDenial => "F-05",
            FaultType::GpsGlitch => "F-06",
            FaultType::BaroDrift => "F-07",
            FaultType::WindEvent => "F-08",
            FaultType::TransportDelay => "F-09",
            FaultType::PacketDrop => "F-10",
        }
    }
}

/// Which IMU a bias/saturation fault targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImuSensor {
    Gyro,
    Accel,
}

/// Fault parameters; only the fields relevant to the type are set (all
/// others `None`). Mirrors the `[[fault]]` TOML table of SPEC §7.2/§9.1 and
/// the POST /api/faults JSON body (SPEC §7.3).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FaultParams {
    /// Motor index 0-3 (F-01, F-02).
    pub motor: Option<u8>,
    /// Efficiency factor in [0, 1] (F-01).
    pub factor: Option<f32>,
    /// IMU axis 0-2 (F-03, F-04).
    pub axis: Option<u8>,
    /// Gyro or accel (F-03, F-04).
    pub sensor: Option<ImuSensor>,
    /// Bias ramp rate, unit/s (F-03).
    pub rate_per_s: Option<f32>,
    /// Saturation limit, unit (F-04).
    pub limit: Option<f32>,
    /// GPS glitch offset, NED meters (F-06).
    pub offset_ned_m: Option<[f32; 3]>,
    /// Gust vector, NED m/s (F-08).
    pub wind_gust_ms: Option<[f32; 3]>,
    /// Gust rise time, s (F-08).
    pub rise_s: Option<f32>,
    /// Wire delay, ms (F-09).
    pub delay_ms: Option<f32>,
    /// Drop probability, percent (F-10).
    pub drop_pct: Option<f32>,
}

/// A fault event: parameters + membership window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FaultSpec {
    pub id: String,
    #[serde(rename = "type")]
    pub ftype: FaultType,
    /// Virtual-time start, ms. Interpreted as "now" for runtime injection,
    /// where the POST body omits it (§7.3).
    #[serde(default)]
    pub start_ms: u64,
    /// Window duration, ms; `None` + !persistent = instantaneous? No: with
    /// neither duration nor persistent the event is active for a single
    /// tick window is undefined — the spec's timeline requires one of
    /// duration_ms/persistent; validation enforces it for timeline faults.
    pub duration_ms: Option<u64>,
    /// Active until cleared (runtime DELETE).
    #[serde(default)]
    pub persistent: bool,
    #[serde(default, flatten)]
    pub params: FaultParams,
}

impl FaultSpec {
    /// Membership at virtual time t (ms).
    pub fn active_at(&self, t_ms: u64) -> bool {
        if t_ms < self.start_ms {
            return false;
        }
        match (self.persistent, self.duration_ms) {
            (true, _) => true,
            (false, Some(d)) => t_ms < self.start_ms.saturating_add(d),
            (false, None) => t_ms == self.start_ms, // single-tick pulse
        }
    }
}

/// Aggregated effects snapshot (the engine's output for one tick).
#[derive(Debug, Clone, PartialEq)]
pub struct Effects {
    /// Per-motor efficiency multipliers (F-01; overlapping faults multiply).
    pub motor_eff: [f64; 4],
    /// Per-motor cut flags (F-02).
    pub motor_cut: [bool; 4],
    /// Added gyro bias, rad/s (F-03).
    pub gyro_bias_add: [f64; 3],
    /// Added accel bias, m/s^2 (F-03).
    pub accel_bias_add: [f64; 3],
    /// Per-axis saturation limits (±), gyro; `None` per axis (F-04).
    pub gyro_sat: [Option<f64>; 3],
    pub accel_sat: [Option<f64>; 3],
    /// GPS denial active (F-05).
    pub gps_denied: bool,
    /// Denial progress in [0, 1] for the eph ramp (F-05).
    pub gps_denial_progress: f64,
    /// GPS position offset, NED m (F-06).
    pub gps_glitch_ned_m: [f64; 3],
    /// Baro altitude bias, m (F-07).
    pub baro_bias_m: f64,
    /// Gust to add to the wind, NED m/s (F-08).
    pub wind_gust_ned: [f64; 3],
    /// Outgoing frame wire delay, ms (F-09).
    pub transport_delay_ms: f64,
    /// Outgoing sensor-frame drop probability, percent (F-10).
    pub drop_pct: f64,
    /// Active faults (id, type) for the control plane / WS stream.
    pub active: Vec<(String, FaultType)>,
}

impl Default for Effects {
    fn default() -> Self {
        Effects {
            motor_eff: [1.0; 4],
            motor_cut: [false; 4],
            gyro_bias_add: [0.0; 3],
            accel_bias_add: [0.0; 3],
            gyro_sat: [None; 3],
            accel_sat: [None; 3],
            gps_denied: false,
            gps_denial_progress: 0.0,
            gps_glitch_ned_m: [0.0; 3],
            baro_bias_m: 0.0,
            wind_gust_ned: [0.0; 3],
            transport_delay_ms: 0.0,
            drop_pct: 0.0,
            active: Vec::new(),
        }
    }
}

/// The fault engine: timeline specs + runtime-injected specs.
#[derive(Debug, Clone, Default)]
pub struct FaultEngine {
    specs: Vec<FaultSpec>,
    cleared: HashSet<String>,
}

impl FaultEngine {
    pub fn new(specs: Vec<FaultSpec>) -> Self {
        FaultEngine { specs, cleared: HashSet::new() }
    }

    pub fn specs(&self) -> &[FaultSpec] {
        &self.specs
    }

    /// Inject a runtime fault whose window starts at `now_us` (SPEC §7.3).
    /// Returns the (possibly de-duplicated) id.
    pub fn inject_at(&mut self, mut spec: FaultSpec, now_us: u64) -> String {
        spec.start_ms = now_us / 1000;
        let mut id = spec.id.clone();
        if id.is_empty() {
            id = format!("runtime-{}", self.specs.len() + 1);
        }
        // De-duplicate ids: append a counter.
        while self.specs.iter().any(|s| s.id == id) {
            id.push('\'');
        }
        spec.id = id.clone();
        self.specs.push(spec);
        id
    }

    /// Clear a persistent fault effect (DELETE /api/faults/{id}).
    pub fn clear(&mut self, id: &str) -> bool {
        if self.specs.iter().any(|s| s.id == id) {
            self.cleared.insert(id.to_string())
        } else {
            false
        }
    }

    /// Pending (future) timeline events for the control plane.
    pub fn pending(&self, t_us: u64) -> Vec<&FaultSpec> {
        let t_ms = t_us / 1000;
        self.specs
            .iter()
            .filter(|s| !self.cleared.contains(&s.id) && s.start_ms > t_ms)
            .collect()
    }

    /// Active (present) fault list for the control plane.
    pub fn active_list(&self, t_us: u64) -> Vec<(String, FaultType)> {
        let t_ms = t_us / 1000;
        self.specs
            .iter()
            .filter(|s| !self.cleared.contains(&s.id) && s.active_at(t_ms))
            .map(|s| (s.id.clone(), s.ftype))
            .collect()
    }

    /// Evaluate all effects at virtual time `t_us`.
    pub fn evaluate(&mut self, t_us: u64) -> Effects {
        let t_ms = t_us / 1000;
        let mut fx = Effects::default();
        for spec in &self.specs {
            if self.cleared.contains(&spec.id) || !spec.active_at(t_ms) {
                continue;
            }
            let elapsed_s = (t_ms - spec.start_ms) as f64 / 1000.0;
            fx.active.push((spec.id.clone(), spec.ftype));
            match spec.ftype {
                FaultType::MotorEfficiency => {
                    let m = spec.params.motor.unwrap_or(0).min(3) as usize;
                    let f = spec.params.factor.unwrap_or(1.0).clamp(0.0, 1.0) as f64;
                    fx.motor_eff[m] *= f;
                }
                FaultType::MotorCut => {
                    let m = spec.params.motor.unwrap_or(0).min(3) as usize;
                    fx.motor_cut[m] = true;
                }
                FaultType::ImuBiasRamp => {
                    let axis = spec.params.axis.unwrap_or(0).min(2) as usize;
                    let rate = spec.params.rate_per_s.unwrap_or(0.0) as f64;
                    let bias = rate * elapsed_s;
                    match spec.params.sensor.unwrap_or(ImuSensor::Gyro) {
                        ImuSensor::Gyro => fx.gyro_bias_add[axis] += bias,
                        ImuSensor::Accel => fx.accel_bias_add[axis] += bias,
                    }
                }
                FaultType::ImuSaturation => {
                    let axis = spec.params.axis.unwrap_or(0).min(2) as usize;
                    let limit = spec.params.limit.unwrap_or(0.0).abs() as f64;
                    let slot = match spec.params.sensor.unwrap_or(ImuSensor::Gyro) {
                        ImuSensor::Gyro => &mut fx.gyro_sat,
                        ImuSensor::Accel => &mut fx.accel_sat,
                    };
                    // Tightest limit wins when several overlap.
                    slot[axis] = Some(slot[axis].map_or(limit, |prev| prev.min(limit)));
                }
                FaultType::GpsDenial => {
                    fx.gps_denied = true;
                    // Ramp progress over the window (persistent/undefined
                    // windows: full ramp after 20 s).
                    let dur_ms = spec.duration_ms.unwrap_or(20_000) as f64;
                    let p = if dur_ms <= 0.0 { 1.0 } else { (elapsed_s * 1000.0 / dur_ms).clamp(0.0, 1.0) };
                    fx.gps_denial_progress = fx.gps_denial_progress.max(p);
                }
                FaultType::GpsGlitch => {
                    let off = spec.params.offset_ned_m.unwrap_or([0.0; 3]);
                    for k in 0..3 {
                        fx.gps_glitch_ned_m[k] += off[k] as f64;
                    }
                }
                FaultType::BaroDrift => {
                    let rate = spec.params.rate_per_s.unwrap_or(0.0) as f64;
                    // SPEC §7.1 F-07 parameter is "rate m/s": the ramp rate of
                    // the altitude bias. `rate_per_s` carries it.
                    fx.baro_bias_m += rate * elapsed_s;
                }
                FaultType::WindEvent => {
                    let gust = spec.params.wind_gust_ms.unwrap_or([0.0; 3]);
                    let rise = spec.params.rise_s.unwrap_or(1.0).max(1e-3) as f64;
                    let shape = 1.0 - (-elapsed_s / rise).exp(); // first-order rise
                    for k in 0..3 {
                        fx.wind_gust_ned[k] += gust[k] as f64 * shape;
                    }
                }
                FaultType::TransportDelay => {
                    let d = spec.params.delay_ms.unwrap_or(0.0).max(0.0) as f64;
                    fx.transport_delay_ms = fx.transport_delay_ms.max(d);
                }
                FaultType::PacketDrop => {
                    let p = spec.params.drop_pct.unwrap_or(0.0).clamp(0.0, 100.0) as f64;
                    fx.drop_pct = fx.drop_pct.max(p);
                }
            }
        }
        fx
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(id: &str, ftype: FaultType, start_ms: u64, duration_ms: Option<u64>, persistent: bool) -> FaultSpec {
        FaultSpec {
            id: id.into(),
            ftype,
            start_ms,
            duration_ms,
            persistent,
            params: FaultParams::default(),
        }
    }

    /// SPEC §10.2: timeline membership boundary tests.
    #[test]
    fn membership_boundaries() {
        let s = spec("a", FaultType::GpsDenial, 1_000, Some(2_000), false);
        assert!(!s.active_at(999), "before start");
        assert!(s.active_at(1_000), "at start");
        assert!(s.active_at(2_999), "just before end");
        assert!(!s.active_at(3_000), "at end (exclusive)");
    }

    #[test]
    fn persistent_membership() {
        let s = spec("p", FaultType::MotorCut, 5_000, None, true);
        assert!(!s.active_at(4_999));
        assert!(s.active_at(5_000));
        assert!(s.active_at(u64::MAX / 2));
    }

    #[test]
    fn overlapping_events_both_active() {
        let mut e = FaultEngine::new(vec![
            spec("a", FaultType::GpsDenial, 0, Some(10_000), false),
            spec("b", FaultType::GpsDenial, 5_000, Some(10_000), false),
        ]);
        let fx = e.evaluate(6_000_000);
        assert_eq!(fx.active.len(), 2);
        let fx = e.evaluate(14_000_000);
        assert_eq!(fx.active.len(), 1);
        assert_eq!(fx.active[0].0, "b");
    }

    #[test]
    fn motor_efficiency_multiplies_overlaps() {
        let mut a = spec("a", FaultType::MotorEfficiency, 0, Some(10_000), false);
        a.params.motor = Some(2);
        a.params.factor = Some(0.5);
        let mut b = spec("b", FaultType::MotorEfficiency, 0, Some(10_000), false);
        b.params.motor = Some(2);
        b.params.factor = Some(0.6);
        let mut e = FaultEngine::new(vec![a, b]);
        let fx = e.evaluate(1_000_000);
        assert!((fx.motor_eff[2] - 0.30).abs() < 1e-6, "eff {}", fx.motor_eff[2]);
        assert!((fx.motor_eff[0] - 1.0).abs() < 1e-12);
    }

    #[test]
    fn bias_ramp_grows_with_elapsed_time() {
        let mut s = spec("r", FaultType::ImuBiasRamp, 1_000, Some(10_000), false);
        s.params.sensor = Some(ImuSensor::Accel);
        s.params.axis = Some(1);
        s.params.rate_per_s = Some(0.02);
        let mut e = FaultEngine::new(vec![s]);
        let fx = e.evaluate(6_000_000); // 5 s elapsed
        assert!((fx.accel_bias_add[1] - 0.10).abs() < 1e-6, "bias {}", fx.accel_bias_add[1]);
        assert_eq!(fx.gyro_bias_add, [0.0; 3]);
    }

    #[test]
    fn runtime_injection_starts_now_and_clears() {
        let mut e = FaultEngine::new(vec![]);
        let mut s = spec("", FaultType::MotorCut, 0, None, true);
        s.params.motor = Some(3);
        let id = e.inject_at(s, 12_345_000);
        assert!(!id.is_empty());
        assert!(e.evaluate(12_000_000).active.is_empty(), "starts at now, not before");
        let fx = e.evaluate(12_345_000);
        assert!(fx.motor_cut[3]);
        assert!(e.clear(&id));
        assert!(!e.evaluate(12_500_000).motor_cut[3]);
        assert!(!e.clear(&id), "second clear is a no-op");
    }

    #[test]
    fn denial_progress_ramps_over_window() {
        let mut s = spec("d", FaultType::GpsDenial, 10_000, Some(10_000), false);
        let mut e = FaultEngine::new(vec![s.clone()]);
        let fx = e.evaluate(15_000_000);
        assert!((fx.gps_denial_progress - 0.5).abs() < 1e-9);
        let fx = e.evaluate(25_000_000);
        assert!(!fx.gps_denied, "denial ended");
        // Progress of an ended event is not applied.
        assert_eq!(fx.gps_denial_progress, 0.0);
    }

    #[test]
    fn wind_gust_first_order_rise() {
        let mut s = spec("w", FaultType::WindEvent, 0, Some(60_000), false);
        s.params.wind_gust_ms = Some([3.0, 0.0, 0.0]);
        s.params.rise_s = Some(2.0);
        let mut e = FaultEngine::new(vec![s]);
        let fx = e.evaluate(2_000_000); // 2 s = one rise constant
        let expect = 3.0 * (1.0 - (-1.0f64).exp());
        assert!((fx.wind_gust_ned[0] - expect).abs() < 1e-9);
    }

    #[test]
    fn transport_effects_take_max() {
        let mut a = spec("a", FaultType::TransportDelay, 0, Some(10_000), false);
        a.params.delay_ms = Some(20.0);
        let mut b = spec("b", FaultType::TransportDelay, 0, Some(10_000), false);
        b.params.delay_ms = Some(50.0);
        let mut c = spec("c", FaultType::PacketDrop, 0, Some(10_000), false);
        c.params.drop_pct = Some(10.0);
        let mut e = FaultEngine::new(vec![a, b, c]);
        let fx = e.evaluate(1_000_000);
        assert_eq!(fx.transport_delay_ms, 50.0);
        assert_eq!(fx.drop_pct, 10.0);
    }

    #[test]
    fn pending_and_active_lists() {
        let mut e = FaultEngine::new(vec![
            spec("future", FaultType::GpsGlitch, 60_000, Some(5_000), false),
            spec("now", FaultType::BaroDrift, 0, Some(1_000_000), false),
        ]);
        let pending = e.pending(1_000_000);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, "future");
        let active = e.active_list(1_000_000);
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].0, "now");
    }
}
