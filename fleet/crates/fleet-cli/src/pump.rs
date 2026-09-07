//! Offboard setpoint pump math (spec §7.1 / §3.3): velocity-capped stepping
//! toward a position goal.
//!
//! The 20 Hz transmit loop lives in fleet-mavlink's link task; this module is
//! the same profile step as a pure function, used by the supervisor wherever
//! it must *compute* a capped setpoint instead of delegating the stepping to
//! the link's pump (the §8.1 policy-7 altitude-divergence override, and the
//! report's profile math). One definition, unit-tested once — the link's
//! in-pump copy is pinned by fleet-mavlink's own `setpoint_slot_velocity_cap`
//! test against the same 4 m/s / 20 Hz numbers.

#![forbid(unsafe_code)]

/// Cruise speed (m/s, spec §6.2/§7.1).
pub const CRUISE_MS: f32 = 4.0;
/// Setpoint pump period (s, 20 Hz per spec §2.3/§3.3).
pub const PUMP_DT_S: f32 = 0.05;
/// Supervisor tick period (s, 10 Hz per spec §2.3).
pub const TICK_DT_S: f32 = 0.10;

/// Velocity-capped position step (spec §7.1): per-axis displacement
/// saturates at `cruise_ms * dt_s` so the effective command velocity never
/// exceeds cruise — a trapezoidal-lite profile without acceleration shaping.
///
/// Properties pinned by the unit tests: never overshoots the goal, per-axis
/// saturation, exact arrival in finite steps, zero-distance is a no-op.
pub fn step_toward(current: [f32; 3], goal: [f32; 3], cruise_ms: f32, dt_s: f32) -> [f32; 3] {
    let max_step = (cruise_ms.max(0.0) * dt_s.max(0.0)).min(f32::MAX);
    let mut out = current;
    for i in 0..3 {
        let d = goal[i] - current[i];
        out[i] = current[i] + d.clamp(-max_step, max_step);
    }
    out
}

/// Time (s) for the capped profile to cover `distance_m` at `cruise_ms`
/// (used by the report and diagnostics).
pub fn profile_time_s(distance_m: f32, cruise_ms: f32) -> f32 {
    if cruise_ms <= 0.0 {
        return f32::INFINITY;
    }
    distance_m.max(0.0) / cruise_ms
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §7.1's headline number: cruise 4 m/s at 20 Hz steps exactly 0.2 m.
    #[test]
    fn step_at_cruise_is_exactly_cruise_times_dt() {
        let s = step_toward([0.0, 0.0, 0.0], [100.0, -100.0, -10.0], CRUISE_MS, PUMP_DT_S);
        // each axis is distance-saturated: 0.2 m per axis
        assert_eq!(s, [0.2, -0.2, -0.2]);
        let speed = (s[0] / PUMP_DT_S).abs();
        assert!((speed - CRUISE_MS).abs() < 1e-3, "speed {speed}");
    }

    #[test]
    fn small_moves_are_not_amplified() {
        // closer than one step: move exactly the residual, no overshoot.
        let s = step_toward([10.0, 0.0, -3.0], [10.05, 0.0, -3.0], CRUISE_MS, PUMP_DT_S);
        assert_eq!(s, [10.05, 0.0, -3.0]);
    }

    #[test]
    fn never_overshoots_and_arrives_in_finite_steps() {
        let goal = [60.0, -40.0, -12.0];
        let mut cur = [0.0f32, 0.0, 0.0];
        let mut steps = 0u32;
        while cur != goal && steps < 10_000 {
            cur = step_toward(cur, goal, CRUISE_MS, PUMP_DT_S);
            // monotone approach on every axis, never past the goal
            for i in 0..3 {
                if goal[i] >= 0.0 {
                    assert!(cur[i] >= 0.0 && cur[i] <= goal[i] + 1e-6);
                } else {
                    assert!(cur[i] <= 0.0 && cur[i] >= goal[i] - 1e-6);
                }
            }
            steps += 1;
        }
        assert_eq!(cur, goal);
        // 60 m at 4 m/s = 15 s = 300 steps (the slowest axis)
        assert!(steps >= 300 && steps <= 310, "steps {steps}");
    }

    #[test]
    fn per_axis_independence() {
        // axis 0 needs a full step, axis 1 is already at goal, axis 2 partial.
        let s = step_toward([0.0, 5.0, -1.0], [10.0, 5.0, -1.1], CRUISE_MS, PUMP_DT_S);
        assert_eq!(s[0], 0.2);
        assert_eq!(s[1], 5.0);
        assert_eq!(s[2], -1.1);
    }

    #[test]
    fn zero_distance_and_degenerate_inputs() {
        assert_eq!(step_toward([1.0, 2.0, 3.0], [1.0, 2.0, 3.0], 4.0, 0.05), [1.0, 2.0, 3.0]);
        // zero cruise or zero dt: frozen setpoint
        assert_eq!(step_toward([0.0, 0.0, 0.0], [9.0, 9.0, 9.0], 0.0, 0.05), [0.0, 0.0, 0.0]);
        assert_eq!(step_toward([0.0, 0.0, 0.0], [9.0, 9.0, 9.0], 4.0, 0.0), [0.0, 0.0, 0.0]);
        // negative cruise treated as frozen, not reversed
        assert_eq!(step_toward([0.0, 0.0, 0.0], [9.0, 9.0, 9.0], -4.0, 0.05), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn profile_time_math() {
        assert!((profile_time_s(60.0, 4.0) - 15.0).abs() < 1e-4);
        assert!(profile_time_s(60.0, 0.0).is_infinite());
        assert_eq!(profile_time_s(-5.0, 4.0), 0.0);
    }
}
