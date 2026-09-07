//! 6-DOF quadrotor-X rigid-body dynamics (SPEC §5).
//!
//! Frames: world NED, body FRD, thrust along body −z. The quaternion q
//! (w-first) rotates body->NED. Integration is fixed-step RK4 over the
//! 18-dim state [p(3), v(3), q(4), omega(3), rotor(4), soc(1)], with
//! post-step quaternion renormalization.
//!
//! Motor geometry and spin signs mirror PX4's iris (SYS_AUTOSTART 10015)
//! control-allocation table (`ROMFS/.../10015_gazebo-classic_iris`):
//!
//! ```text
//! motor 0: front-right (+a, +a, 0), yaw-moment ratio positive
//! motor 1: back-left   (-a, -a, 0), positive
//! motor 2: front-left  (+a, -a, 0), negative
//! motor 3: back-right  (-a, +a, 0), negative
//! ```
//!
//! A rotor spinning with positive yaw-moment sign contributes
//! `+c_M * w^2` about body +z (matching PX4's effectiveness matrix
//! `moment = ct*pos×axis - ct*km*axis` with axis = (0,0,-1)).

use crate::quat::{quat_mul, quat_normalize, quat_to_rot, rot_mul, rot_t_mul};

/// Rigid-body + actuator + ground + battery parameters (SPEC §5.5).
#[derive(Debug, Clone, PartialEq)]
pub struct QuadParams {
    pub mass_kg: f64,
    /// Diagonal inertia (Jx, Jy, Jz).
    pub inertia: [f64; 3],
    /// Arm length (arm ends at (±arm/√2, ±arm/√2, 0)).
    pub arm_m: f64,
    /// Thrust coefficient, N per (rad/s)^2.
    pub c_t: f64,
    /// Yaw moment coefficient, N·m per (rad/s)^2 (signed per motor table).
    pub c_m: f64,
    pub omega_max: f64,
    pub tau_motor: f64,
    /// Idle throttle threshold.
    pub u_idle: f64,
    pub k_lin: f64,
    pub k_quad: f64,
    pub g: f64,
    pub k_g: f64,
    pub c_g: f64,
    pub mu: f64,
    /// Battery capacity, mAh.
    pub battery_capacity_mah: f64,
    /// Battery current model (SPEC §6.5): I = i_base + k_p * total_thrust.
    /// Defaults give ~16 A at hover on the default airframe.
    pub i_base_a: f64,
    pub k_p_a_per_n: f64,
    /// Discharge model toggle (SPEC §6.5: disabled by default — infinite
    /// battery). When false the SoC derivative is forced to zero.
    pub battery_enabled: bool,
}

impl Default for QuadParams {
    fn default() -> Self {
        QuadParams {
            mass_kg: 1.5,
            inertia: [0.02, 0.02, 0.04],
            arm_m: 0.225,
            c_t: 1.10e-5,
            c_m: 1.25e-7,
            omega_max: 950.0,
            tau_motor: 0.03,
            u_idle: 0.05,
            k_lin: 0.10,
            k_quad: 0.85,
            g: 9.80665,
            k_g: 4000.0,
            c_g: 240.0,
            mu: 0.8,
            battery_capacity_mah: 5200.0,
            i_base_a: 0.4,
            k_p_a_per_n: 1.06,
            battery_enabled: false,
        }
    }
}

/// Yaw-moment sign per motor (PX4 iris order: FR, BL, FL, BR).
pub const MOTOR_SPIN: [f64; 4] = [1.0, 1.0, -1.0, -1.0];

impl QuadParams {
    /// Arm-end contact/rotor positions in body FRD (meters).
    pub fn arm_positions(&self) -> [[f64; 3]; 4] {
        let a = self.arm_m * libm::sqrt(0.5);
        [[a, a, 0.0], [-a, -a, 0.0], [a, -a, 0.0], [-a, a, 0.0]]
    }

    /// Rotor speed commanded by throttle u (idle model, SPEC §5.3).
    /// `stopped` forces the target to zero (motor cut / disarm-stop).
    pub fn rotor_target(&self, u: f64, stopped: bool) -> f64 {
        if stopped {
            return 0.0;
        }
        let u = u.clamp(0.0, 1.0);
        if u < self.u_idle {
            self.u_idle * self.omega_max
        } else {
            u * self.omega_max
        }
    }
}

/// Full vehicle state (SPEC §5.1). Internal precision is f64; the wire and
/// replay formats quantize at their own boundaries.
#[derive(Debug, Clone, PartialEq)]
pub struct State {
    /// NED position (m), z down, ground plane at z = 0.
    pub pos: [f64; 3],
    /// NED velocity (m/s).
    pub vel: [f64; 3],
    /// Attitude quaternion wxyz, body->NED, normalized.
    pub q: [f64; 4],
    /// Body angular rates (rad/s).
    pub omega: [f64; 3],
    /// Rotor speeds (rad/s).
    pub rotors: [f64; 4],
    /// Battery state of charge in [0, 1] (1 when discharge disabled).
    pub soc: f64,
}

impl Default for State {
    fn default() -> Self {
        State {
            pos: [0.0; 3],
            vel: [0.0; 3],
            q: [1.0, 0.0, 0.0, 0.0],
            omega: [0.0; 3],
            rotors: [0.0; 4],
            soc: 1.0,
        }
    }
}

/// External per-step inputs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StepInput {
    /// Motor commands AFTER fault scaling, in [0, 1] (SPEC §3.9 mapping).
    pub u: [f64; 4],
    /// NED wind at the vehicle (steady + turbulence + gust).
    pub wind_ned: [f64; 3],
    /// Per-motor "propellers stopped" flag (motor-cut fault, SPEC §5.3/§7.1
    /// F-02): the rotor target is 0 and the rotor winds down through the
    /// first-order lag instead of idling.
    pub motor_stopped: [bool; 4],
}

impl Default for StepInput {
    fn default() -> Self {
        StepInput { u: [0.0; 4], wind_ned: [0.0; 3], motor_stopped: [false; 4] }
    }
}

/// Coulomb-friction viscous gain for the ground-contact horizontal force
/// (N per m/s of contact slip, clamped to mu * F_n).
const K_FRICTION: f64 = 2000.0;

const NSTATE: usize = 18;

fn pack(s: &State) -> [f64; NSTATE] {
    let mut x = [0.0; NSTATE];
    x[0..3].copy_from_slice(&s.pos);
    x[3..6].copy_from_slice(&s.vel);
    x[6..10].copy_from_slice(&s.q);
    x[10..13].copy_from_slice(&s.omega);
    x[13..17].copy_from_slice(&s.rotors);
    x[17] = s.soc;
    x
}

fn unpack(x: &[f64; NSTATE]) -> State {
    let mut s = State::default();
    s.pos.copy_from_slice(&x[0..3]);
    s.vel.copy_from_slice(&x[3..6]);
    s.q.copy_from_slice(&x[6..10]);
    s.omega.copy_from_slice(&x[10..13]);
    s.rotors.copy_from_slice(&x[13..17]);
    s.soc = x[17];
    s
}

/// The quadrotor dynamics integrator.
#[derive(Debug, Clone)]
pub struct QuadDynamics {
    pub params: QuadParams,
    pub state: State,
    /// Latched numerical-divergence flag (ADR-013): set the first time any
    /// state component goes non-finite. The integrator keeps running (the
    /// quaternion guard below keeps the wire output parseable) but callers
    /// must stop and report — silently streaming NaN is what let the
    /// I-2 takeoff divergence hide as a frozen EKF.
    pub diverged: bool,
}

impl QuadDynamics {
    pub fn new(params: QuadParams, state: State) -> Self {
        QuadDynamics { params, state, diverged: false }
    }

    /// True when no state component is NaN/inf.
    pub fn state_is_finite(&self) -> bool {
        let s = &self.state;
        s.pos.iter().chain(s.vel.iter()).chain(s.q.iter()).chain(s.omega.iter()).chain(s.rotors.iter())
            .all(|v| v.is_finite())
            && s.soc.is_finite()
    }

    /// Total rotor thrust (N), body −z.
    pub fn total_thrust(&self) -> f64 {
        self.state
            .rotors
            .iter()
            .map(|&w| self.params.c_t * w * w)
            .sum()
    }

    /// Specific force in body frame (what the accelerometer measures,
    /// before sensor errors): total non-gravitational force / mass, with the
    /// wind input used for the drag term.
    pub fn specific_force_body(&self, wind_ned: [f64; 3]) -> [f64; 3] {
        let (f, _) = forces(&self.params, &self.state, wind_ned);
        [f[0] / self.params.mass_kg, f[1] / self.params.mass_kg, f[2] / self.params.mass_kg]
    }

    /// Advance one fixed step with RK4; renormalizes the quaternion
    /// afterwards (SPEC §5.1). `dt` is the tick period.
    pub fn step(&mut self, input: &StepInput, dt: f64) {
        let p = &self.params;
        let x0 = pack(&self.state);

        let f = |x: &[f64; NSTATE]| -> [f64; NSTATE] {
            let s = unpack(x);
            deriv(p, &s, input)
        };

        let k1 = f(&x0);
        let mut x = x0;
        for i in 0..NSTATE {
            x[i] = x0[i] + 0.5 * dt * k1[i];
        }
        let k2 = f(&x);
        let mut x = x0;
        for i in 0..NSTATE {
            x[i] = x0[i] + 0.5 * dt * k2[i];
        }
        let k3 = f(&x);
        let mut x = x0;
        for i in 0..NSTATE {
            x[i] = x0[i] + dt * k3[i];
        }
        let k4 = f(&x);
        let mut xn = [0.0; NSTATE];
        for i in 0..NSTATE {
            xn[i] = x0[i] + dt / 6.0 * (k1[i] + 2.0 * k2[i] + 2.0 * k3[i] + k4[i]);
        }
        self.state = unpack(&xn);
        self.state.q = quat_normalize(&self.state.q);
        // Guard: quaternion must never go through zero.
        if !self.state.q[0].is_finite() {
            self.state.q = [1.0, 0.0, 0.0, 0.0];
        }
        if !self.diverged && !self.state_is_finite() {
            self.diverged = true;
        }
    }
}

/// Fastest (dominant) eigenvalue magnitude of the unilateral
/// spring-damper ground contact, for explicit-integration stability
/// sizing (ADR-013).
///
/// Two contact modes matter:
/// - vertical: the full mass rides 4 parallel (k_g, c_g) contacts;
/// - rotational (roll/pitch): inertia Jx/Jy reacts to the same contacts
///   at the arm moment arms — stiffer per unit inertia, so it dominates
///   whenever the arm geometry is real (lambda_rot ~ 1.2e3 1/s at SPEC
///   defaults vs ~6.2e2 1/s vertical).
///
/// For an overdamped mode the fast root is wn*(zeta + sqrt(zeta^2 - 1));
/// for an underdamped mode the pair magnitude is wn. Both are what an
/// explicit RK4 step of size h must satisfy |lambda*h| < 2.785 against.
pub fn contact_fast_root(p: &QuadParams) -> f64 {
    let arms = p.arm_positions();
    let sum_ry2: f64 = arms.iter().map(|r| r[1] * r[1]).sum();
    let sum_rx2: f64 = arms.iter().map(|r| r[0] * r[0]).sum();
    let vertical = overdamped_fast_root(4.0 * p.k_g, 4.0 * p.c_g, p.mass_kg);
    let roll = overdamped_fast_root(p.k_g * sum_ry2, p.c_g * sum_ry2, p.inertia[0]);
    let pitch = overdamped_fast_root(p.k_g * sum_rx2, p.c_g * sum_rx2, p.inertia[1]);
    vertical.max(roll).max(pitch)
}

/// Fast-root magnitude of (k, c) acting on mass m (see `contact_fast_root`).
fn overdamped_fast_root(k: f64, c: f64, m: f64) -> f64 {
    if k <= 0.0 || m <= 0.0 {
        return 0.0;
    }
    let wn = libm::sqrt(k / m);
    if c <= 0.0 {
        return wn;
    }
    let zeta = c / (2.0 * libm::sqrt(k * m));
    if zeta >= 1.0 {
        wn * (zeta + libm::sqrt(zeta * zeta - 1.0))
    } else {
        wn
    }
}

/// RK4 substeps per tick that keeps every ground-contact root inside the
/// stability region with margin: N = ceil(lambda_max * h / 1.5), clamped to
/// [1, 64]. The 1.5 target leaves ~46% margin to RK4's 2.785 real-axis
/// limit, covering the nonlinear unilateral-contact regime (mode mixing
/// during penetration chatter). N is a pure function of (params, h), so the
/// determinism contract (SPEC §8.1) is preserved.
///
/// At SPEC defaults (k_g = 4000, c_g = 240, J = 0.02) and h = 5 ms this
/// gives N = 4 (h_sub = 1.25 ms) — versus the ADR-013-rejected fixed
/// 2-substep policy whose rotational root sat at |lambda*h| = 3.0, the
/// measured cause of the I-2 takeoff divergence.
pub fn stable_substeps(p: &QuadParams, h: f64) -> usize {
    if h <= 0.0 {
        return 1;
    }
    let lam = contact_fast_root(p);
    let n = libm::ceil(lam * h / 1.5) as usize;
    n.clamp(1, 64)
}

/// Forces and moments (body frame) at state `s` given wind. Returns
/// (force_body, torque_body) including thrust, drag, ground contact and
/// aerodynamic yaw moments.
fn forces(p: &QuadParams, s: &State, wind_ned: [f64; 3]) -> ([f64; 3], [f64; 3]) {
    let rot = quat_to_rot(&s.q);
    let arms = p.arm_positions();

    // Rotor thrusts and yaw moments.
    let mut f_body = [0.0f64; 3];
    let mut tau_body = [0.0f64; 3];
    for i in 0..4 {
        let t = p.c_t * s.rotors[i] * s.rotors[i];
        let f_i = [0.0, 0.0, -t]; // body -z is up
        f_body[0] += f_i[0];
        f_body[1] += f_i[1];
        f_body[2] += f_i[2];
        // r × F
        let r = arms[i];
        tau_body[0] += r[1] * f_i[2] - r[2] * f_i[1];
        tau_body[1] += r[2] * f_i[0] - r[0] * f_i[2];
        tau_body[2] += r[0] * f_i[1] - r[1] * f_i[0];
        // Rotor reaction yaw moment.
        tau_body[2] += MOTOR_SPIN[i] * p.c_m * s.rotors[i] * s.rotors[i];
    }

    // Aerodynamic drag on the body airspeed (linear + quadratic).
    let v_body = rot_t_mul(&rot, &s.vel);
    let wind_body = rot_t_mul(&rot, &wind_ned);
    let mut v_air = [0.0f64; 3];
    for i in 0..3 {
        v_air[i] = v_body[i] - wind_body[i];
    }
    let spd = libm::sqrt(v_air[0] * v_air[0] + v_air[1] * v_air[1] + v_air[2] * v_air[2]);
    let drag_k = p.k_lin + p.k_quad * spd;
    for i in 0..3 {
        f_body[i] -= drag_k * v_air[i];
    }

    // Ground contact: four point contacts at the arm ends, spring-damper
    // normal + clamped-viscous Coulomb friction (SPEC §5.4).
    // NED, z down, ground plane at z = 0: a contact point with c_z > 0 is
    // BELOW the surface (penetrating). Depth d = c_z; the normal force acts
    // along -z (up); damping opposes downward (penetrating) motion.
    let mut ground_tau_world = [0.0f64; 3];
    let mut ground_f_world = [0.0f64; 3];
    for i in 0..4 {
        // World position of the contact point.
        let r_b = arms[i];
        let r_w = rot_mul(&rot, &r_b);
        let c = [s.pos[0] + r_w[0], s.pos[1] + r_w[1], s.pos[2] + r_w[2]];
        if c[2] <= 0.0 {
            continue; // at or above the ground plane: no contact
        }
        // Contact-point velocity: v + omega × r (both world/NED).
        let omega_w = rot_mul(&rot, &s.omega);
        let cross = [
            omega_w[1] * r_w[2] - omega_w[2] * r_w[1],
            omega_w[2] * r_w[0] - omega_w[0] * r_w[2],
            omega_w[0] * r_w[1] - omega_w[1] * r_w[0],
        ];
        let v_c = [s.vel[0] + cross[0], s.vel[1] + cross[1], s.vel[2] + cross[2]];
        // Penetration depth (positive below the surface).
        let d = c[2];
        let f_n = (p.k_g * d + p.c_g * v_c[2]).max(0.0);
        // NED: ground normal is -z (up); normal force pushes -z.
        let mut f_w = [0.0f64, 0.0f64, 0.0f64];
        f_w[2] -= f_n;
        // Friction (clamped viscous): opposes horizontal contact slip.
        let slip = libm::sqrt(v_c[0] * v_c[0] + v_c[1] * v_c[1]);
        if slip > 1e-9 {
            let cap = p.mu * f_n;
            let mag = (K_FRICTION * slip).min(cap);
            f_w[0] -= mag * v_c[0] / slip;
            f_w[1] -= mag * v_c[1] / slip;
        }
        for k in 0..3 {
            ground_f_world[k] += f_w[k];
        }
        // Torque about the CoM: r_w × f_w.
        let tq = [
            r_w[1] * f_w[2] - r_w[2] * f_w[1],
            r_w[2] * f_w[0] - r_w[0] * f_w[2],
            r_w[0] * f_w[1] - r_w[1] * f_w[0],
        ];
        for k in 0..3 {
            ground_tau_world[k] += tq[k];
        }
    }
    let f_ground_body = rot_t_mul(&rot, &ground_f_world);
    let tau_ground_body = rot_t_mul(&rot, &ground_tau_world);
    for k in 0..3 {
        f_body[k] += f_ground_body[k];
        tau_body[k] += tau_ground_body[k];
    }

    (f_body, tau_body)
}

/// State derivative (SPEC §5.2).
fn deriv(p: &QuadParams, s: &State, input: &StepInput) -> [f64; NSTATE] {
    let mut dx = [0.0; NSTATE];
    let (f_body, tau_body) = forces(p, s, input.wind_ned);
    let rot = quat_to_rot(&s.q);

    // p_dot = v
    dx[0..3].copy_from_slice(&s.vel);

    // v_dot = R * f_body / m + g_NED
    let a_ned = rot_mul(&rot, &f_body);
    for i in 0..3 {
        dx[3 + i] = a_ned[i] / p.mass_kg;
    }
    dx[5] += p.g;

    // q_dot = 0.5 * q ⊗ (0, omega)
    let qd = quat_mul(&s.q, &[0.0, s.omega[0], s.omega[1], s.omega[2]]);
    for i in 0..4 {
        dx[6 + i] = 0.5 * qd[i];
    }

    // omega_dot = J^-1 (tau - omega × (J omega))
    let jw = [p.inertia[0] * s.omega[0], p.inertia[1] * s.omega[1], p.inertia[2] * s.omega[2]];
    let gyro = [
        s.omega[1] * jw[2] - s.omega[2] * jw[1],
        s.omega[2] * jw[0] - s.omega[0] * jw[2],
        s.omega[0] * jw[1] - s.omega[1] * jw[0],
    ];
    for i in 0..3 {
        dx[10 + i] = (tau_body[i] - gyro[i]) / p.inertia[i];
    }

    // Rotor first-order lag.
    for i in 0..4 {
        let target = p.rotor_target(input.u[i], input.motor_stopped[i]);
        dx[13 + i] = (target - s.rotors[i]) / p.tau_motor;
    }

    // Battery (SPEC §6.5): d(soc)/dt = -I / (Q_mAh * 3.6); capacity in mAh
    // -> charge in A·s = Q * 3.6. Disabled by default (infinite battery).
    if p.battery_enabled {
        let thrust_sum: f64 = (0..4).map(|i| p.c_t * s.rotors[i] * s.rotors[i]).sum();
        let current = p.i_base_a + p.k_p_a_per_n * thrust_sum;
        dx[17] = -current / (p.battery_capacity_mah * 3.6);
    } else {
        dx[17] = 0.0;
    }

    dx
}
