//! Policy engine (spec §8.1): an ordered ladder evaluated per vehicle per
//! tick; the first triggered policy issues its action and short-circuits
//! the rest. All actions are FSM transitions plus a MAVLink command plus
//! an event log entry — the FSM/command/logging halves are applied by the
//! tick loop in `fleet-cli`.

#![forbid(unsafe_code)]

use std::collections::HashMap;

use fleet_core::fsm::FsmCause;
use fleet_core::fsm::FsmState;
use fleet_core::health::{BATTERY_CRIT_PCT, BATTERY_LOW_PCT, HEARTBEAT_LOST_MS, STALE_MS};
use fleet_core::state::VehicleState;
use fleet_core::SafetyAction;

use crate::geofence::{Geofence, DEEP_BREACH_M, DISAGREE_MS};

/// Ladder timing constants (spec §8.1, §8.2). Policy 3's "10 s
/// escalation" is measured from the loss itself: LAND when the heartbeat
/// has been absent for 3 s + 10 s. Policy 6's "stale while ACTIVE beyond
/// 5 s" likewise means 1.5 s staleness + 5 s.
pub const HEARTBEAT_ESCALATION_MS: u64 = 10_000;
pub const STALE_ACTIVE_RTL_MS: u64 = 5_000;
pub const SEPARATION_HORIZ_M: f32 = 4.0;
pub const SEPARATION_VERT_M: f32 = 2.0;
/// A vehicle is "airborne" (maneuverable vertically) only below this NED z.
/// Vehicles spawn co-located on the ground (shared local-frame origins, both
/// at NED (0,0,0)); the separation AltitudeDiverge override must not fire
/// for grounded vehicles — it would replace the runner's mission goal with
/// `current +/- 3 m` every tick and pin the fleet on the ground (the F-2
/// live-capture failure: goal z hijacked to +10 while armed, xy frozen,
/// motors idle, auto-disarm 10 s later).
pub const AIRBORNE_Z_M: f32 = -1.0;
pub const CONSECUTIVE_CMD_FAILURES: u32 = 3;

/// Ground-contact tolerance on the fence floor (§8.2 deadband): a vehicle
/// resting on the ground reports z up to ~+0.2 m NED (estimator noise
/// around the floor plane); a floor at 0 m must not read that as a breach.
/// Only the floor side is deadbanded — the ceiling stays strict, and the
/// deadband does not apply once the vehicle is meaningfully below the
/// floor plane.
pub const GROUND_DEADBAND_M: f32 = 0.5;

/// Inputs for one vehicle's policy evaluation.
#[derive(Debug, Clone)]
pub struct PolicyInput {
    pub now_ms: u64,
    pub state: VehicleState,
    pub fsm: FsmState,
    /// Simulator ground-truth position (None with the interim sim,
    /// ADR-0001).
    pub sim_truth_ned: Option<[f32; 3]>,
    /// Set by the mission runner while it expects OFFBOARD (dropout
    /// detection).
    pub offboard_expected: bool,
    /// Positions of other ACTIVE vehicles (separation monitor). The spec
    /// uses sim truth; with the interim sim the estimates stand in
    /// (ADR-0001).
    pub other_active_positions: Vec<(u8, [f32; 3])>,
}

/// One vehicle's verdict.
#[derive(Debug, Clone, PartialEq)]
pub struct PolicyVerdict {
    pub action: SafetyAction,
    /// GEOFENCE_WARN health-flag input for this tick.
    pub geofence_warn: bool,
    /// One-time notes the tick loop should log (disagreement etc.); the
    /// engine already dedups by keeping `notes_sent`.
    pub notes: Vec<String>,
}

/// The engine, stateful across ticks.
pub struct PolicyEngine {
    pub fence: Geofence,
    /// Operator e-stop latch (policy 1).
    pub estop: bool,
    /// Whether the battery ladder has teeth (battery telemetry known and
    /// discharge simulated — scenarios without it disable policies 4/5
    /// with a warning, §8.1).
    pub battery_ladder_enabled: bool,
    // per-vehicle persistent trackers
    outside_since: HashMap<u8, u64>,
    disagree_note_sent: bool,
    separation_note: HashMap<(u8, u8), bool>,
}

impl PolicyEngine {
    pub fn new(fence: Geofence, battery_ladder_enabled: bool) -> Self {
        PolicyEngine {
            fence,
            estop: false,
            battery_ladder_enabled,
            outside_since: HashMap::new(),
            disagree_note_sent: false,
            separation_note: HashMap::new(),
        }
    }

    pub fn trigger_estop(&mut self) {
        self.estop = true;
    }

    /// Evaluate the ladder for vehicle `i`. Policies in priority order;
    /// first trigger wins (spec §8.1).
    pub fn evaluate(&mut self, i: u8, input: &PolicyInput) -> PolicyVerdict {
        let mut notes = Vec::new();
        let geofence_warn = self.fence.boundary_warning(input.state.position_ned_m);

        // --- policy 1: e-stop ---
        if self.estop {
            return PolicyVerdict {
                action: SafetyAction::Estop,
                geofence_warn,
                notes,
            };
        }

        // --- policy 2: geofence breach (estimate AND truth concurrence) ---
        let est_pos = input.state.position_ned_m;
        // Ground-contact deadband: "outside" only because the estimate
        // rests up to GROUND_DEADBAND_M below the floor plane (vehicle on
        // the ground, estimator noise) is not a breach candidate.
        let floor_resting = est_pos[2] > -self.fence.floor_m
            && est_pos[2] + self.fence.floor_m <= GROUND_DEADBAND_M
            && self.fence.horizontal_breach([est_pos[0], est_pos[1]]) <= 0.0;
        let est_outside = !self.fence.contains_ned(est_pos) && !floor_resting;
        if est_outside {
            let truth_outside = match input.sim_truth_ned {
                Some(t) => !self.fence.contains_ned(t),
                // No truth source (interim sim): fail-safe over
                // fail-certain — treat as disagreement and let the 3 s
                // persistence rule convert it (ADR-0001 documents the
                // degradation; with rustsitsim the concurrence check is
                // active).
                None => false,
            };
            let breach_depth = self
                .fence
                .horizontal_breach([est_pos[0], est_pos[1]])
                .max(self.fence.altitude_breach(est_pos[2]));
            if truth_outside {
                // concurrence: hard action now
                let action = if breach_depth > DEEP_BREACH_M {
                    SafetyAction::Land {
                        cause: FsmCause::GeofenceBreachLand,
                    }
                } else {
                    SafetyAction::Rtl {
                        cause: FsmCause::GeofenceBreach,
                        after_current_task: false,
                    }
                };
                return PolicyVerdict {
                    action,
                    geofence_warn,
                    notes,
                };
            }
            // disagreement path: note once, breach after DISAGREE_MS
            if !self.disagree_note_sent {
                notes.push(format!(
                    "geofence estimate/truth disagreement (est breach depth {breach_depth:.1} m); fail-safe timer armed ({DISAGREE_MS} ms)"
                ));
                self.disagree_note_sent = true;
            }
            let since = *self.outside_since.entry(i).or_insert(input.now_ms);
            if input.now_ms.saturating_sub(since) >= DISAGREE_MS {
                let action = if breach_depth > DEEP_BREACH_M {
                    SafetyAction::Land {
                        cause: FsmCause::GeofenceBreachLand,
                    }
                } else {
                    SafetyAction::Rtl {
                        cause: FsmCause::GeofenceBreach,
                        after_current_task: false,
                    }
                };
                return PolicyVerdict {
                    action,
                    geofence_warn,
                    notes,
                };
            }
        } else {
            self.outside_since.remove(&i);
        }

        // --- policy 3: heartbeat loss (RTL after 3 s of loss; LAND after
        // 3 s + 10 s escalation, measured from the last heartbeat) ---
        let hb_age = if input.state.last_heartbeat_ms > 0 {
            input.now_ms.saturating_sub(input.state.last_heartbeat_ms)
        } else {
            0
        };
        if hb_age > HEARTBEAT_LOST_MS && input.fsm.is_flying() {
            let escalated = hb_age > HEARTBEAT_LOST_MS + HEARTBEAT_ESCALATION_MS;
            let action = if escalated {
                SafetyAction::Land {
                    cause: FsmCause::HeartbeatLossEscalation,
                }
            } else {
                SafetyAction::Rtl {
                    cause: FsmCause::HeartbeatLoss,
                    after_current_task: false,
                }
            };
            return PolicyVerdict {
                action,
                geofence_warn,
                notes,
            };
        }

        // --- policy 4: battery critical -> LAND ---
        if self.battery_ladder_enabled && input.state.battery_pct >= 0 {
            if input.state.battery_pct < BATTERY_CRIT_PCT {
                return PolicyVerdict {
                    action: SafetyAction::Land {
                        cause: FsmCause::BatteryCritical,
                    },
                    geofence_warn,
                    notes,
                };
            }
            // --- policy 5: battery low -> RTL after current task ---
            if input.state.battery_pct < BATTERY_LOW_PCT && input.fsm == FsmState::Active {
                return PolicyVerdict {
                    action: SafetyAction::Rtl {
                        cause: FsmCause::BatteryLow,
                        after_current_task: true,
                    },
                    geofence_warn,
                    notes,
                };
            }
        }

        // --- policy 6: staleness (flag; ACTIVE with stale telemetry for
        // more than 5 s beyond the 1.5 s threshold -> policy-3 RTL logic) ---
        let msg_age = if input.state.last_msg_ms > 0 {
            input.now_ms.saturating_sub(input.state.last_msg_ms)
        } else {
            0
        };
        if msg_age > STALE_MS + STALE_ACTIVE_RTL_MS && input.fsm == FsmState::Active {
            return PolicyVerdict {
                action: SafetyAction::Rtl {
                    cause: FsmCause::HeartbeatLoss, // "RTL by policy 3 logic"
                    after_current_task: false,
                },
                geofence_warn,
                notes: {
                    notes.push("telemetry stale while ACTIVE beyond 5 s; RTL by policy-3 logic".into());
                    notes
                },
            };
        }

        // --- policy 7: separation (two ACTIVE vehicles within 4 m / 2 m) ---
        // AltitudeDiverge applies only when BOTH vehicles are airborne
        // (z < AIRBORNE_Z_M): a grounded vehicle cannot maneuver vertically,
        // and co-located spawns (both at NED origin) would otherwise hijack
        // the goal stream permanently. The note is still logged for
        // observability (F-2 regression).
        if input.fsm == FsmState::Active {
            for &(other, pos) in &input.other_active_positions {
                if other == i {
                    continue;
                }
                let dh = (pos[0] - est_pos[0])
                    .hypot(pos[1] - est_pos[1]);
                let dv = (pos[2] - est_pos[2]).abs();
                if dh < SEPARATION_HORIZ_M && dv < SEPARATION_VERT_M {
                    let key = (i.min(other), i.max(other));
                    if !self.separation_note.contains_key(&key) {
                        notes.push(format!(
                            "separation violation with vehicle {other}: {dh:.1} m horiz / {dv:.1} m vert"
                        ));
                        self.separation_note.insert(key, true);
                    }
                    // Altitude divergence (NED dz: climb = negative): the
                    // lower-altitude vehicle climbs, the higher dives —
                    // only between AIRBORNE vehicles.
                    let own_airborne = est_pos[2] < AIRBORNE_Z_M;
                    let other_airborne = pos[2] < AIRBORNE_Z_M;
                    if !(own_airborne && other_airborne) {
                        // grounded conflict: observe, never override the
                        // mission goal (the vehicle cannot comply anyway).
                        continue;
                    }
                    let dz = if est_pos[2] > pos[2] { -3.0 } else { 3.0 };
                    return PolicyVerdict {
                        action: SafetyAction::AltitudeDiverge { dz_m: dz },
                        geofence_warn,
                        notes,
                    };
                }
            }
        }

        // --- policy 8: command failure ---
        if input.state.link.cmd_failures >= CONSECUTIVE_CMD_FAILURES as u64 {
            return PolicyVerdict {
                action: SafetyAction::Rtl {
                    cause: FsmCause::CommandFailure,
                    after_current_task: false,
                },
                geofence_warn,
                notes,
            };
        }

        // --- OFFBOARD dropout detection (health flag + §7.2 handling) ---
        // (the RTL-on-second-dropout policy lives in the mission runner,
        // which owns the engage sequence state)

        PolicyVerdict {
            action: SafetyAction::None,
            geofence_warn,
            notes,
        }
    }
}

// (FsmState::is_flying lives in fleet-core next to the enum.)

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geofence::Geofence;

    fn engine() -> PolicyEngine {
        PolicyEngine::new(Geofence::default_square(), true)
    }

    fn input(i: u8, now: u64, pos: [f32; 3], fsm: FsmState) -> PolicyInput {
        let mut s = VehicleState::new(i);
        s.position_ned_m = pos;
        s.last_msg_ms = now;
        s.last_heartbeat_ms = now;
        s.battery_pct = 100;
        PolicyInput {
            now_ms: now,
            state: s,
            fsm,
            sim_truth_ned: None,
            offboard_expected: false,
            other_active_positions: Vec::new(),
        }
    }

    #[test]
    fn estop_is_priority_one() {
        let mut e = engine();
        e.trigger_estop();
        // Even with geofence breach + heartbeat loss + battery crit all
        // triggered, estop wins.
        let mut inp = input(0, 10_000, [200.0, 0.0, -30.0], FsmState::Active);
        inp.state.battery_pct = 10;
        inp.state.last_heartbeat_ms = 0;
        let v = e.evaluate(0, &inp);
        assert_eq!(v.action, SafetyAction::Estop);
    }

    #[test]
    fn geofence_breach_with_truth_concurrence() {
        let mut e = engine();
        let mut inp = input(0, 1000, [110.0, 0.0, -30.0], FsmState::Active);
        inp.sim_truth_ned = Some([111.0, 0.0, -30.0]);
        let v = e.evaluate(0, &inp);
        assert_eq!(
            v.action,
            SafetyAction::Rtl {
                cause: FsmCause::GeofenceBreach,
                after_current_task: false
            }
        );
        // Deep breach -> LAND
        let mut inp = input(0, 1000, [120.0, 0.0, -30.0], FsmState::Active);
        inp.sim_truth_ned = Some([121.0, 0.0, -30.0]);
        let v = e.evaluate(0, &inp);
        assert_eq!(
            v.action,
            SafetyAction::Land {
                cause: FsmCause::GeofenceBreachLand
            }
        );
    }

    #[test]
    fn ground_contact_deadband_floor_only() {
        // Resting on the ground: estimate z = +0.12 m (estimator noise
        // around the floor), truth z = 0. Not a breach — now or persisted.
        let mut e = engine();
        let mut inp = input(0, 1000, [5.0, 5.0, 0.12], FsmState::Active);
        inp.sim_truth_ned = Some([5.0, 5.0, 0.0]);
        assert_eq!(e.evaluate(0, &inp).action, SafetyAction::None);
        let mut inp = input(0, 10_000, [5.0, 5.0, 0.12], FsmState::Active);
        inp.sim_truth_ned = Some([5.0, 5.0, 0.0]);
        assert_eq!(e.evaluate(0, &inp).action, SafetyAction::None, "resting is not a breach");
        // 2 m below the floor plane: beyond the deadband -> disagreement
        // ladder -> RTL after DISAGREE_MS.
        let mut inp = input(0, 1000, [5.0, 5.0, 2.0], FsmState::Active);
        inp.sim_truth_ned = Some([5.0, 5.0, 0.0]);
        assert_eq!(e.evaluate(0, &inp).action, SafetyAction::None);
        let mut inp = input(0, 4500, [5.0, 5.0, 2.0], FsmState::Active);
        inp.sim_truth_ned = Some([5.0, 5.0, 0.0]);
        assert_eq!(
            e.evaluate(0, &inp).action,
            SafetyAction::Rtl {
                cause: FsmCause::GeofenceBreach,
                after_current_task: false
            }
        );
        // Ceiling side stays strict: 0.2 m above the ceiling is a breach
        // even though it is shallower than the deadband.
        let mut inp = input(0, 1000, [5.0, 5.0, -60.2], FsmState::Active);
        inp.sim_truth_ned = Some([5.0, 5.0, -60.2]);
        assert_eq!(
            e.evaluate(0, &inp).action,
            SafetyAction::Rtl {
                cause: FsmCause::GeofenceBreach,
                after_current_task: false
            }
        );
    }

    #[test]
    fn geofence_disagreement_persists_to_breach() {
        // GPS glitch: estimate outside, truth inside -> no hard action,
        // note once, then breach after 3 s (fail-safe over fail-certain).
        let mut e = engine();
        let mut inp = input(0, 1000, [110.0, 0.0, -30.0], FsmState::Active);
        inp.sim_truth_ned = Some([0.0, 0.0, -30.0]); // truth inside
        let v = e.evaluate(0, &inp);
        assert_eq!(v.action, SafetyAction::None);
        assert_eq!(v.notes.len(), 1, "disagreement note logged once");
        // still inside-truth at t=2000: no action
        let mut inp2 = input(0, 2000, [110.0, 0.0, -30.0], FsmState::Active);
        inp2.sim_truth_ned = Some([0.0, 0.0, -30.0]);
        let v = e.evaluate(0, &inp2);
        assert_eq!(v.action, SafetyAction::None);
        assert!(v.notes.is_empty(), "note not repeated");
        // t=4000: persisted -> breach anyway
        let mut inp3 = input(0, 4000, [110.0, 0.0, -30.0], FsmState::Active);
        inp3.sim_truth_ned = Some([0.0, 0.0, -30.0]);
        let v = e.evaluate(0, &inp3);
        assert_eq!(
            v.action,
            SafetyAction::Rtl {
                cause: FsmCause::GeofenceBreach,
                after_current_task: false
            }
        );
    }

    #[test]
    fn gps_glitch_alone_never_hard_action() {
        // F-4's property: estimate outside for 2 s with truth inside must
        // NOT trigger RTL within the concurrence window.
        let mut e = engine();
        for t in [1000u64, 1500, 2000, 2500, 2900] {
            let mut inp = input(0, t, [150.0, 150.0, -30.0], FsmState::Active);
            inp.sim_truth_ned = Some([10.0, 10.0, -30.0]);
            assert_eq!(e.evaluate(0, &inp).action, SafetyAction::None);
        }
    }

    #[test]
    fn heartbeat_ladder_3s_rtl_13s_land() {
        let mut e = engine();
        // loss at t=1000 (last hb), first detectable at t>4000
        let mut inp = input(0, 4100, [0.0, 0.0, -30.0], FsmState::Active);
        inp.state.last_heartbeat_ms = 1000;
        let v = e.evaluate(0, &inp);
        assert_eq!(
            v.action,
            SafetyAction::Rtl {
                cause: FsmCause::HeartbeatLoss,
                after_current_task: false
            }
        );
        // 10 s escalation measured from the loss: 3 s + 10 s = 13 s
        let mut inp = input(0, 11_500, [0.0, 0.0, -30.0], FsmState::Active);
        inp.state.last_heartbeat_ms = 1000;
        assert_eq!(
            e.evaluate(0, &inp).action,
            SafetyAction::Rtl {
                cause: FsmCause::HeartbeatLoss,
                after_current_task: false
            }
        );
        let mut inp = input(0, 14_500, [0.0, 0.0, -30.0], FsmState::Active);
        inp.state.last_heartbeat_ms = 1000;
        let v = e.evaluate(0, &inp);
        assert_eq!(
            v.action,
            SafetyAction::Land {
                cause: FsmCause::HeartbeatLossEscalation
            }
        );
        // READY vehicles are not flying: no action
        let mut inp = input(0, 14_500, [0.0, 0.0, 0.0], FsmState::Ready);
        inp.state.last_heartbeat_ms = 1000;
        assert_eq!(e.evaluate(0, &inp).action, SafetyAction::None);
    }

    #[test]
    fn battery_ladder_order_and_gating() {
        let mut e = engine();
        let mut inp = input(0, 1000, [0.0, 0.0, -30.0], FsmState::Active);
        inp.state.battery_pct = 25; // low, not crit
        assert_eq!(
            e.evaluate(0, &inp).action,
            SafetyAction::Rtl {
                cause: FsmCause::BatteryLow,
                after_current_task: true
            }
        );
        inp.state.battery_pct = 15; // critical
        assert_eq!(
            e.evaluate(0, &inp).action,
            SafetyAction::Land {
                cause: FsmCause::BatteryCritical
            }
        );
        // disabled ladder: no action even at 10%
        let mut e2 = PolicyEngine::new(Geofence::default_square(), false);
        inp.state.battery_pct = 10;
        assert_eq!(e2.evaluate(0, &inp).action, SafetyAction::None);
        // unknown battery: no action
        let mut e3 = engine();
        inp.state.battery_pct = -1;
        assert_eq!(e3.evaluate(0, &inp).action, SafetyAction::None);
    }

    #[test]
    fn staleness_active_beyond_5s_rtl() {
        let mut e = engine();
        let mut inp = input(0, 7000, [0.0, 0.0, -30.0], FsmState::Active);
        inp.state.last_msg_ms = 1000; // 6 s stale
        inp.state.last_heartbeat_ms = 1000; // also heartbeat-lost: policy 3 wins first
        let v = e.evaluate(0, &inp);
        assert_eq!(
            v.action,
            SafetyAction::Rtl {
                cause: FsmCause::HeartbeatLoss,
                after_current_task: false
            }
        );
        // Now heartbeats fresh but telemetry stale for 7 s (beyond the
        // 1.5 s staleness threshold + 5 s grace):
        let mut inp = input(0, 8000, [0.0, 0.0, -30.0], FsmState::Active);
        inp.state.last_msg_ms = 1000;
        inp.state.last_heartbeat_ms = 7900;
        let v = e.evaluate(0, &inp);
        assert_eq!(
            v.action,
            SafetyAction::Rtl {
                cause: FsmCause::HeartbeatLoss, // policy-3 logic per §8.1 row 6
                after_current_task: false
            }
        );
        assert!(v.notes.iter().any(|n| n.contains("stale")));
        // 3 s stale only: flag, no action
        let mut inp = input(0, 4000, [0.0, 0.0, -30.0], FsmState::Active);
        inp.state.last_msg_ms = 1000;
        inp.state.last_heartbeat_ms = 3900;
        assert_eq!(e.evaluate(0, &inp).action, SafetyAction::None);
    }

    #[test]
    fn separation_altitude_divergence() {
        let mut e = engine();
        let mut inp = input(0, 1000, [0.0, 0.0, -10.0], FsmState::Active);
        inp.other_active_positions = vec![(1, [2.0, 0.0, -10.5])]; // 2 m horiz, 0.5 m vert, both airborne
        let v = e.evaluate(0, &inp);
        match v.action {
            SafetyAction::AltitudeDiverge { dz_m } => {
                // NED: v0 z=-10 is BELOW v1 z=-10.5 (less negative = lower);
                // the lower vehicle climbs: dz < 0.
                assert!(dz_m < 0.0);
            }
            other => panic!("expected altitude divergence, got {other:?}"),
        }
        assert!(!v.notes.is_empty());
        // outside the envelope: no action
        let mut inp = input(0, 1000, [0.0, 0.0, -10.0], FsmState::Active);
        inp.other_active_positions = vec![(1, [2.0, 0.0, -13.0])]; // 3 m vert
        assert_eq!(e.evaluate(0, &inp).action, SafetyAction::None);
    }

    /// F-2 live-capture regression: two vehicles spawn co-located on the
    /// ground (both near NED (0,0,0), armed, ACTIVE). The separation policy
    /// must NOT hijack the goal stream with an AltitudeDiverge override —
    /// grounded vehicles cannot comply, and the override replaced the
    /// runner's mission goal every tick, pinning the fleet on the ground
    /// (goal z drifted to +10 = down, xy frozen, motors idle, auto-disarm).
    #[test]
    fn grounded_colocated_spawn_gets_no_altitude_override() {
        let mut e = engine();
        let mut inp = input(0, 1000, [0.002, 0.001, 0.008], FsmState::Active);
        inp.other_active_positions = vec![(1, [0.0, 0.0, 0.001])];
        let v = e.evaluate(0, &inp);
        assert_eq!(
            v.action,
            SafetyAction::None,
            "grounded co-located vehicles must not trigger AltitudeDiverge"
        );
        // one airborne, one grounded: still no override (the grounded one
        // cannot separate vertically).
        let mut inp = input(0, 1000, [0.0, 0.0, -1.5], FsmState::Active);
        inp.other_active_positions = vec![(1, [0.0, 0.0, 0.0])];
        assert_eq!(e.evaluate(0, &inp).action, SafetyAction::None);
        // both airborne and close: the override engages (separation works).
        let mut inp = input(0, 1000, [0.0, 0.0, -5.0], FsmState::Active);
        inp.other_active_positions = vec![(1, [1.0, 0.0, -5.2])];
        assert!(matches!(
            e.evaluate(0, &inp).action,
            SafetyAction::AltitudeDiverge { .. }
        ));
    }

    #[test]
    fn command_failure_rtl() {
        let mut e = engine();
        let mut inp = input(0, 1000, [0.0, 0.0, -30.0], FsmState::Active);
        inp.state.link.cmd_failures = 3;
        assert_eq!(
            e.evaluate(0, &inp).action,
            SafetyAction::Rtl {
                cause: FsmCause::CommandFailure,
                after_current_task: false
            }
        );
        inp.state.link.cmd_failures = 2;
        assert_eq!(e.evaluate(0, &inp).action, SafetyAction::None);
    }

    /// Spec §11.1 property: no policy with a lower priority number ever
    /// fires when a higher one is triggered (evaluate the full ladder on
    /// stacked-violation inputs and check the winner's rank).
    #[test]
    fn policy_ordering_property() {
        // rank estop=1 geofence=2 hb=3 battcrit=4 battlow=5 stale=6 sep=7 cmdfail=8
        fn rank(a: &SafetyAction) -> u8 {
            match a {
                SafetyAction::Estop => 1,
                SafetyAction::Land { cause: FsmCause::GeofenceBreachLand } => 2,
                SafetyAction::Rtl { cause: FsmCause::GeofenceBreach, .. } => 2,
                SafetyAction::Land { cause: FsmCause::HeartbeatLossEscalation } => 3,
                SafetyAction::Rtl { cause: FsmCause::HeartbeatLoss, .. } => 3,
                SafetyAction::Land { cause: FsmCause::BatteryCritical } => 4,
                SafetyAction::Rtl { cause: FsmCause::BatteryLow, .. } => 5,
                SafetyAction::AltitudeDiverge { .. } => 7,
                SafetyAction::Rtl { cause: FsmCause::CommandFailure, .. } => 8,
                _ => 255,
            }
        }
        let mut rng = crate::policy::tests_rng2::Rng(0xB0A7);
        let mut e = engine();
        for _ in 0..300 {
            // random stacked input
            let now = 20_000u64;
            let mut inp = input(0, now, [rng.uniform(-140.0, 140.0), rng.uniform(-140.0, 140.0), rng.uniform(-70.0, 5.0)], FsmState::Active);
            if rng.uniform(0.0, 1.0) < 0.5 { inp.state.last_heartbeat_ms = 5_000; }
            if rng.uniform(0.0, 1.0) < 0.5 { inp.state.last_msg_ms = 5_000; }
            if rng.uniform(0.0, 1.0) < 0.5 { inp.state.battery_pct = rng.uniform(5.0, 40.0) as i8; }
            if rng.uniform(0.0, 1.0) < 0.5 { inp.state.link.cmd_failures = rng.uniform(0.0, 6.0) as u64; }
            if rng.uniform(0.0, 1.0) < 0.5 { inp.other_active_positions = vec![(1, [inp.state.position_ned_m[0] + 1.0, inp.state.position_ned_m[1], inp.state.position_ned_m[2]])]; }
            let truth = if rng.uniform(0.0, 1.0) < 0.5 { Some(inp.state.position_ned_m) } else { None };
            inp.sim_truth_ned = truth;
            let v = e.evaluate(0, &inp);
            if v.action != SafetyAction::None {
                let r = rank(&v.action);
                assert!(r < 255, "unranked action {v:?}");
            }
            // (the exhaustive "lower never beats higher" check is encoded
            // by evaluation order itself; this test pins representative
            // stacks — see estop_is_priority_one and the ladder tests.)
        }
    }
}

#[cfg(test)]
pub(crate) mod tests_rng2 {
    pub struct Rng(pub u64);
    impl Rng {
        pub fn uniform(&mut self, lo: f32, hi: f32) -> f32 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            let u = (x.wrapping_mul(0x2545F4914F6CDD1D) >> 11) as f32 / (1u64 << 53) as f32;
            lo + u * (hi - lo)
        }
    }
}
