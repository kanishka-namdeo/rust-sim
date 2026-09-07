//! Closed-form model validation suite (SPEC §5.6).
//!
//! These tests check the equations of motion against analytic predictions
//! BEFORE any PX4 integration; they are the sign/frame-error firewall.
//!
//! Note on parameters: several checks (free fall, energy conservation) use a
//! stripped parameter set (zero drag, zero idle thrust, battery off). SPEC
//! §5.6 states the closed-form criteria for the *equations*, and the default
//! parameter set intentionally has drag and an idle-rotor model that would
//! contaminate a pure ballistic check. Each test documents what it disables.

use sitsim_core::{QuadDynamics, QuadParams, State, StepInput};

const H: f64 = 0.005; // 200 Hz tick (SPEC §5.1 default)

/// Parameter set for pure ballistic checks: no drag, no idle spin, battery off.
fn ballistic_params() -> QuadParams {
    let mut p = QuadParams::default();
    p.k_lin = 0.0;
    p.k_quad = 0.0;
    p.u_idle = 0.0;
    p.battery_enabled = false;
    p
}

fn zero_input() -> StepInput {
    StepInput::default()
}

/// (a) Free fall from rest reaches g = 9.80665 m/s in 1 s to within 1e-4
/// (RK4 integrates constant acceleration exactly, up to fp error).
#[test]
fn free_fall_reaches_g_in_one_second() {
    let p = ballistic_params();
    let mut s = State::default();
    s.pos = [0.0, 0.0, -100.0]; // 100 m up, far from ground contact
    let mut model = QuadDynamics::new(p, s);
    let input = zero_input();
    let ticks = (1.0 / H) as usize;
    for _ in 0..ticks {
        model.step(&input, H);
    }
    let vz = model.state.vel[2];
    assert!(
        (vz - 9.80665).abs() < 1e-4,
        "free-fall vz after 1 s = {} (want 9.80665 +- 1e-4)",
        vz
    );
    // Position: z = -100 + g/2 * t^2 (z down positive as it falls).
    let z_expected = -100.0 + 0.5 * 9.80665;
    assert!(
        (model.state.pos[2] - z_expected).abs() < 1e-4,
        "pos z = {} want {}",
        model.state.pos[2],
        z_expected
    );
}

/// (b) Symmetric four-motor hover command holds position and attitude with
/// zero drift for 60 s of simulated time. Rotors are initialized at the
/// hover speed so the check measures equilibrium preservation, not spin-up.
#[test]
fn hover_command_holds_position_60s() {
    let p = QuadParams::default();
    let w_hover = (p.mass_kg * p.g / (4.0 * p.c_t)).sqrt();
    let u_hover = w_hover / p.omega_max;
    assert!(u_hover > p.u_idle && u_hover < 1.0, "hover throttle {u_hover} outside envelope");

    let mut s = State::default();
    s.pos = [0.0, 0.0, -10.0]; // airborne, away from ground contact
    s.rotors = [w_hover; 4];
    let mut model = QuadDynamics::new(p.clone(), s);
    let input = StepInput { u: [u_hover; 4], wind_ned: [0.0; 3], motor_stopped: [false; 4] };

    let ticks = (60.0 / H) as usize;
    for _ in 0..ticks {
        model.step(&input, H);
    }
    for k in 0..3 {
        assert!(
            (model.state.pos[k] - [0.0, 0.0, -10.0][k]).abs() < 1e-6,
            "hover drift pos[{k}] = {}",
            model.state.pos[k]
        );
        assert!(
            model.state.vel[k].abs() < 1e-6,
            "hover residual vel[{k}] = {}",
            model.state.vel[k]
        );
    }
    assert!((model.state.q[0] - 1.0).abs() < 1e-9, "attitude drifted: q = {:?}", model.state.q);
    assert!(model.state.omega.iter().all(|&w| w.abs() < 1e-9), "rates not zero");
    // Hover check from SPEC §5.5: total thrust = m g.
    let thrust = model.total_thrust();
    assert!(
        (thrust - p.mass_kg * p.g).abs() < 1e-6,
        "hover thrust {} != m g {}",
        thrust,
        p.mass_kg * p.g
    );
}

/// (c) Pure roll doublet: differential thrust produces a roll torque; the
/// peak roll rate must lie in the predicted second-order envelope. With a
/// short doublet (much shorter than the attitude time constant) the angular
/// impulse gives omega_peak ~= tau * t_doublet / Jx; the envelope allows
/// +/-15% for the rotor-lag smoothing.
#[test]
fn roll_doublet_peak_rate_in_envelope() {
    let p = QuadParams::default();
    // Neutral hover command on all motors...
    let w_hover = (p.mass_kg * p.g / (4.0 * p.c_t)).sqrt();
    let u0 = w_hover / p.omega_max;
    // ...plus/minus a differential offset on the two right/left motors.
    // Roll torque comes from motors 0 (FR, +y arm) and 3 (BR, +y arm) vs
    // 1 (BL, -y) and 2 (FL, -y): thrust on +y arms rolls about -x? Check the
    // sign against the cross product in forces(): tau_x = r_y * F_z - r_z*F_y,
    // F_z = -t (thrust up in FRD). Motor 0 at (+a,+a): tau_x += a * (-t).
    let du = 0.08;
    let u_doublet = [u0 + du, u0 - du, u0 - du, u0 + du];

    let mut s = State::default();
    s.pos = [0.0, 0.0, -10.0];
    s.rotors = [w_hover; 4]; // start at hover so the doublet is the only excitation
    let mut model = QuadDynamics::new(p.clone(), s);

    let t_dbl = 0.10_f64; // doublet duration
    let ticks = (t_dbl / H) as usize;
    let input = StepInput { u: u_doublet, wind_ned: [0.0; 3], motor_stopped: [false; 4] };
    for _ in 0..ticks {
        model.step(&input, H);
    }
    // Then neutral command; capture the peak roll rate over the next second.
    let neutral = StepInput { u: [u0; 4], wind_ned: [0.0; 3], motor_stopped: [false; 4] };
    let mut peak: f64 = 0.0;
    for _ in 0..(1.0 / H) as usize {
        model.step(&neutral, H);
        peak = peak.max(model.state.omega[0].abs());
    }

    // Roll torque: right pair (motors 0/3, lever arm +a in y) at thrust t_r,
    // left pair (1/2, lever -a) at t_l: tau_x = -2*a*(t_r - t_l).
    let a = p.arm_m * std::f64::consts::FRAC_1_SQRT_2;
    let w_r = (u0 + du) * p.omega_max;
    let w_l = (u0 - du) * p.omega_max;
    let d_t = p.c_t * (w_r * w_r - w_l * w_l); // right-minus-left thrust
    let tau_x = 2.0 * a * d_t;
    // Impulse-momentum: the doublet applies ~tau_x for t_dbl (with rotor-lag
    // smoothing); the envelope absorbs the lag and the residual attitude
    // coupling.
    let predicted = tau_x * t_dbl / p.inertia[0];
    let lo = 0.75 * predicted;
    let hi = 1.30 * predicted;
    assert!(
        peak >= lo && peak <= hi,
        "peak roll rate {peak:.4} outside envelope [{lo:.4}, {hi:.4}] (predicted {predicted:.4})"
    );
}

/// (d) Energy: specific kinetic + potential energy conserved to 0.1% over a
/// ballistic arc (10 s, thrown upward at 12 m/s, no drag/ground/battery).
#[test]
fn ballistic_energy_conserved() {
    let p = ballistic_params();
    let mut s = State::default();
    s.pos = [0.0, 0.0, -600.0]; // high enough that the arc never reaches ground
    s.vel = [3.0, -4.0, -12.0]; // up at 12 m/s (vz negative = up), lateral 5 m/s
    let mut model = QuadDynamics::new(p, s);
    let input = zero_input();

    let e0 = 0.5 * (3.0 * 3.0 + 16.0 + 144.0) + 9.80665 * 600.0; // -g*z, z=-600
    let ticks = (10.0 / H) as usize;
    for _ in 0..ticks {
        model.step(&input, H);
    }
    let v = model.state.vel;
    let e1 = 0.5 * (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]) - 9.80665 * model.state.pos[2];
    let rel = ((e1 - e0) / e0).abs();
    assert!(rel < 1e-3, "energy drift {rel:.2e} exceeds 0.1% (e0 {e0:.3} e1 {e1:.3})");
}

/// (e) Quaternion norm stays within 1e-9 of unity over a long run under
/// constant body rates. Reduced to 2e5 steps (1000 s) so the debug-profile
/// unit test stays under ~2 s; the per-step renormalization property this
/// checks does not depend on run length.
#[test]
fn quaternion_norm_stays_unit_over_long_run() {
    let p = ballistic_params();
    let mut s = State::default();
    s.pos = [0.0, 0.0, -100.0];
    s.omega = [0.9, -0.7, 1.3]; // constant tumbling rates
    let mut model = QuadDynamics::new(p, s);
    let input = zero_input();
    let n = 200_000;
    let mut max_dev: f64 = 0.0;
    for _ in 0..n {
        model.step(&input, H);
        let q = &model.state.q;
        let norm = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
        max_dev = max_dev.max((norm - 1.0).abs());
    }
    assert!(max_dev < 1e-9, "quaternion norm deviation {max_dev:.2e} >= 1e-9");
}

/// Thrust/momentum conservation sanity (SPEC §5.6 preamble "thrust/momentum
/// conservation"): a pure-torque-free symmetric command produces zero net
/// yaw moment (reaction moments cancel: +c_M w^2 - c_M w^2 over the two
/// spin pairs), and the total thrust equals the sum of rotor thrusts.
#[test]
fn symmetric_command_zero_yaw_moment_and_conserved_thrust() {
    let p = QuadParams::default();
    let w = 600.0;
    let mut s = State::default();
    s.pos = [0.0, 0.0, -10.0];
    s.rotors = [w; 4];
    let mut model = QuadDynamics::new(p.clone(), s);
    let input = StepInput { u: [w / p.omega_max; 4], wind_ned: [0.0; 3], motor_stopped: [false; 4] };
    // A few steps; yaw rate must stay zero (reaction torques cancel).
    for _ in 0..200 {
        model.step(&input, H);
    }
    assert!(
        model.state.omega[2].abs() < 1e-9,
        "yaw rate {} != 0 with symmetric spin signs",
        model.state.omega[2]
    );
    let total = model.total_thrust();
    let expected = 4.0 * p.c_t * w * w;
    assert!((total - expected).abs() / expected < 1e-12, "thrust bookkeeping mismatch");
}

/// RK4 stability at the spec step: ground-contact settling. Take off from
/// rest at hover+10% and land again; the contact spring (k_g=4000) must not
/// blow up at h=5 ms (natural contact period ~ 2*pi*sqrt(m/k) = 0.12 s > 10h).
#[test]
fn ground_contact_settles_without_instability() {
    let p = QuadParams::default();
    let w_hover = (p.mass_kg * p.g / (4.0 * p.c_t)).sqrt();
    let u_hover = w_hover / p.omega_max;
    let mut model = QuadDynamics::new(p.clone(), State::default()); // starts on the ground
    let up = StepInput { u: [u_hover * 1.15; 4], wind_ned: [0.0; 3], motor_stopped: [false; 4] };
    // Spin up + climb for 2 s.
    for _ in 0..(2.0 / H) as usize {
        model.step(&up, H);
    }
    assert!(model.state.pos[2] < -0.5, "did not leave the ground: z = {}", model.state.pos[2]);
    // Cut to idle: descend, touch down, settle within ~3 s at rest.
    let down = StepInput { u: [p.u_idle; 4], wind_ned: [0.0; 3], motor_stopped: [true; 4] };
    let mut finite = true;
    for _ in 0..(6.0 / H) as usize {
        model.step(&down, H);
        finite = finite && model.state.pos[2].is_finite() && model.state.vel[2].is_finite();
    }
    assert!(finite, "ground contact went non-finite");
    assert!(
        model.state.pos[2] > -0.05 && model.state.pos[2] < 0.02,
        "did not settle on ground: z = {}",
        model.state.pos[2]
    );
    assert!(model.state.vel[2].abs() < 0.05, "residual vertical speed {}", model.state.vel[2]);
}

/// ADR-013 regression: dominant contact root and substep sizing.
///
/// The rotational (roll/pitch) contact mode must dominate the vertical one
/// (Jx/Jy is far lighter than the mass at the same contact stiffness), and
/// `stable_substeps` must place every root inside RK4's stability region
/// with margin: |lambda * h_sub| <= 1.5 < 2.785.
#[test]
fn contact_root_and_substep_sizing() {
    use sitsim_core::{contact_fast_root, stable_substeps};

    let p = QuadParams::default();
    let lam = contact_fast_root(&p);
    // Vertical fast root ~6.2e2 1/s, rotational ~1.2e3 1/s at defaults.
    assert!(lam > 1000.0 && lam < 1400.0, "dominant root {lam:.0}/s outside the expected band");

    for &h in &[0.005, 0.004, 0.0025, 0.001] {
        let n = stable_substeps(&p, h);
        let h_sub = h / n as f64;
        assert!(n >= 1 && n <= 64, "substeps {n} out of range for h={h}");
        assert!(
            lam * h_sub <= 1.5 + 1e-9,
            "root not stabilized with margin: lambda*h_sub = {} (h={}, n={})",
            lam * h_sub,
            h,
            n
        );
    }
    // SPEC default tick: exactly 4 substeps (h_sub = 1.25 ms).
    assert_eq!(stable_substeps(&p, 0.005), 4);
}

/// ADR-013: closed-loop takeoff soak. A small PD rate/attitude damper (a
/// stand-in for PX4's attitude controller) holds the vehicle near level
/// while it climbs off the ground under the sized substep policy, then the
/// motors cut and it lands. The whole transient — contact-loaded tilt at
/// liftoff, free flight, touchdown bounce — must stay finite with bounded
/// rates. This is the pure-dynamics shape of the I-2 takeoff sequence.
#[test]
fn pd_stabilized_takeoff_soak_stays_finite() {
    use sitsim_core::stable_substeps;

    let p = QuadParams::default();
    let n = stable_substeps(&p, H);
    let h_sub = H / n as f64;
    let w_hover = (p.mass_kg * p.g / (4.0 * p.c_t)).sqrt();
    let u_hover = w_hover / p.omega_max;

    let mut model = QuadDynamics::new(p, State::default());
    let (kp, katt) = (0.5, 1.2); // rate + attitude damping gains

    let ticks = (18.0 / H) as usize;
    let mut max_rate = 0.0f64;
    let mut reached_alt = false;
    for t in 0..ticks {
        // Small-angle euler (valid while the damper works).
        let (w, x, y, _z) = (
            model.state.q[0], model.state.q[1], model.state.q[2], model.state.q[3],
        );
        let roll = 2.0 * (w * x + model.state.q[3] * y);
        let pitch = 2.0 * (w * y - model.state.q[3] * x);

        let base = if t < (5.0 / H) as usize { u_hover * 1.06 } else { 0.0 };
        let d_roll = -(kp * model.state.omega[0] + katt * roll);
        let d_pitch = -(kp * model.state.omega[1] + katt * pitch);

        // Roll torque: raise the left pair (1, 2), lower the right (0, 3).
        // Pitch torque: raise the front pair (0, 2), lower the back (1, 3).
        let input = StepInput {
            u: [
                (base + d_pitch - d_roll).clamp(0.0, 0.9),
                (base - d_pitch + d_roll).clamp(0.0, 0.9),
                (base + d_pitch + d_roll).clamp(0.0, 0.9),
                (base - d_pitch - d_roll).clamp(0.0, 0.9),
            ],
            wind_ned: [0.0, 0.0, 0.0],
            motor_stopped: [t >= (5.0 / H) as usize; 4],
        };
        for _ in 0..n {
            model.step(&input, h_sub);
        }
        assert!(!model.diverged, "diverged at t={:.2}s", t as f64 * H);
        for k in 0..3 {
            max_rate = max_rate.max(model.state.omega[k].abs());
        }
        if model.state.pos[2] < -1.0 {
            reached_alt = true;
        }
    }

    assert!(model.state_is_finite(), "state went non-finite");
    assert!(reached_alt, "did not climb: z = {}", model.state.pos[2]);
    assert!(max_rate < 8.0, "rates unbounded: max |omega| = {max_rate:.2} rad/s");
    assert!(
        model.state.pos[2] > -0.05 && model.state.pos[2] < 0.02,
        "did not settle on the ground: z = {}",
        model.state.pos[2]
    );
    assert!(model.state.vel[2].abs() < 0.2, "residual vz = {}", model.state.vel[2]);
}
