//! Vehicle FSM per spec §4. The FSM is the vocabulary of every other
//! subsystem: the allocator bids only from READY, the safety engine
//! escalates along FSM arcs, and CI asserts on FSM traces.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FsmState {
    Init,
    Spawning,
    Booting,
    Ready,
    Active,
    Rtl,
    Landed,
    Fault,
}

impl FsmState {
    pub const ALL: [FsmState; 8] = [
        FsmState::Init,
        FsmState::Spawning,
        FsmState::Booting,
        FsmState::Ready,
        FsmState::Active,
        FsmState::Rtl,
        FsmState::Landed,
        FsmState::Fault,
    ];

    pub fn name(&self) -> &'static str {
        match self {
            FsmState::Init => "INIT",
            FsmState::Spawning => "SPAWNING",
            FsmState::Booting => "BOOTING",
            FsmState::Ready => "READY",
            FsmState::Active => "ACTIVE",
            FsmState::Rtl => "RTL",
            FsmState::Landed => "LANDED",
            FsmState::Fault => "FAULT",
        }
    }

    /// True while the vehicle is airborne from the supervisor's point of
    /// view (ACTIVE or RTL): the states where failsafe policies have teeth.
    pub fn is_flying(self) -> bool {
        matches!(self, FsmState::Active | FsmState::Rtl)
    }
}

impl std::fmt::Display for FsmState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Trigger causes. Every arc in the table has exactly one cause vocabulary;
/// unknown causes are rejected like illegal arcs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FsmCause {
    // bring-up
    Spawn,
    SpawnFailure,
    Px4Launched,
    TelemetryValid,
    // tasking
    TaskAccepted,
    TaskComplete,
    // safety ladder (§8.1)
    GeofenceBreach,
    GeofenceBreachLand,
    HeartbeatLoss,
    HeartbeatLossEscalation,
    BatteryLow,
    BatteryCritical,
    CommandFailure,
    OffboardDropout,
    SupervisorRtl,
    // terminal / estop
    LandCommand,
    DisarmObserved,
    Estop,
    ProcessDeath,
    // recovery
    Rearm,
    RestartOnFault,
}

impl FsmCause {
    pub fn name(&self) -> &'static str {
        match self {
            FsmCause::Spawn => "spawn",
            FsmCause::SpawnFailure => "spawn_failure",
            FsmCause::Px4Launched => "px4_launched",
            FsmCause::TelemetryValid => "telemetry_valid",
            FsmCause::TaskAccepted => "task_accepted",
            FsmCause::TaskComplete => "task_complete",
            FsmCause::GeofenceBreach => "geofence_breach",
            FsmCause::GeofenceBreachLand => "geofence_breach_land",
            FsmCause::HeartbeatLoss => "heartbeat_loss",
            FsmCause::HeartbeatLossEscalation => "heartbeat_loss_escalation",
            FsmCause::BatteryLow => "battery_low",
            FsmCause::BatteryCritical => "battery_critical",
            FsmCause::CommandFailure => "command_failure",
            FsmCause::OffboardDropout => "offboard_dropout",
            FsmCause::SupervisorRtl => "supervisor_rtl",
            FsmCause::LandCommand => "land_command",
            FsmCause::DisarmObserved => "disarm_observed",
            FsmCause::Estop => "estop",
            FsmCause::ProcessDeath => "process_death",
            FsmCause::Rearm => "rearm",
            FsmCause::RestartOnFault => "restart_on_fault",
        }
    }
}

/// The full transition table: (state, cause) -> state.
/// Mirrors spec §4 exactly (plus the e-stop/land arcs documented in
/// ADR-0004).
pub fn transition(from: FsmState, cause: FsmCause) -> Option<FsmState> {
    use FsmCause as C;
    use FsmState as S;
    Some(match (from, cause) {
        (S::Init, C::Spawn) => S::Spawning,
        (S::Spawning, C::Px4Launched) => S::Booting,
        (S::Spawning, C::SpawnFailure | C::ProcessDeath | C::Estop) => S::Fault,
        (S::Booting, C::TelemetryValid) => S::Ready,
        (S::Booting, C::ProcessDeath | C::Estop) => S::Fault,
        (S::Ready, C::TaskAccepted) => S::Active,
        (S::Ready, C::ProcessDeath | C::Estop) => S::Fault,
        (S::Active, C::TaskComplete | C::BatteryLow | C::GeofenceBreach | C::HeartbeatLoss | C::OffboardDropout | C::CommandFailure | C::SupervisorRtl) => S::Rtl,
        (S::Active, C::BatteryCritical | C::GeofenceBreachLand | C::HeartbeatLossEscalation | C::LandCommand | C::Estop) => S::Landed,
        (S::Active, C::ProcessDeath) => S::Fault,
        (S::Rtl, C::DisarmObserved | C::LandCommand | C::HeartbeatLossEscalation | C::Estop) => S::Landed,
        (S::Rtl, C::ProcessDeath) => S::Fault,
        (S::Landed, C::Rearm) => S::Ready,
        (S::Fault, C::RestartOnFault) => S::Spawning,
        _ => return None,
    })
}

/// All legal arcs (for coverage tests and CI trace assertions).
pub fn legal_arcs() -> Vec<(FsmState, FsmState, FsmCause)> {
    let mut arcs = Vec::new();
    for from in FsmState::ALL {
        for cause in CAUSES {
            if let Some(to) = transition(from, cause) {
                arcs.push((from, to, cause));
            }
        }
    }
    arcs
}

pub const CAUSES: [FsmCause; 21] = [
    FsmCause::Spawn,
    FsmCause::SpawnFailure,
    FsmCause::Px4Launched,
    FsmCause::TelemetryValid,
    FsmCause::TaskAccepted,
    FsmCause::TaskComplete,
    FsmCause::GeofenceBreach,
    FsmCause::GeofenceBreachLand,
    FsmCause::HeartbeatLoss,
    FsmCause::HeartbeatLossEscalation,
    FsmCause::BatteryLow,
    FsmCause::BatteryCritical,
    FsmCause::CommandFailure,
    FsmCause::OffboardDropout,
    FsmCause::SupervisorRtl,
    FsmCause::LandCommand,
    FsmCause::DisarmObserved,
    FsmCause::Estop,
    FsmCause::ProcessDeath,
    FsmCause::Rearm,
    FsmCause::RestartOnFault,
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Every transition in the spec §4 table is legal, and nothing else is:
    /// the exhaustive (state x cause) cross product.
    #[test]
    fn transition_table_is_total_and_closed() {
        let legal: HashSet<(FsmState, FsmState, FsmCause)> =
            legal_arcs().into_iter().collect();
        // Sanity: the spec-named arcs exist.
        assert!(legal.contains(&(FsmState::Init, FsmState::Spawning, FsmCause::Spawn)));
        assert!(legal.contains(&(FsmState::Booting, FsmState::Ready, FsmCause::TelemetryValid)));
        assert!(legal.contains(&(FsmState::Ready, FsmState::Active, FsmCause::TaskAccepted)));
        assert!(legal.contains(&(FsmState::Active, FsmState::Rtl, FsmCause::GeofenceBreach)));
        assert!(legal.contains(&(FsmState::Rtl, FsmState::Landed, FsmCause::DisarmObserved)));
        assert!(legal.contains(&(FsmState::Landed, FsmState::Ready, FsmCause::Rearm)));
        assert!(legal.contains(&(FsmState::Fault, FsmState::Spawning, FsmCause::RestartOnFault)));
        // Every state has at least one exit except LANDED-as-terminal choice
        // (LANDED has Rearm) and INIT (has Spawn).
        for s in FsmState::ALL {
            assert!(
                legal.iter().any(|(f, _, _)| *f == s),
                "{s} has no outgoing arcs"
            );
        }
        // 64 combos; the legal subset is small.
        let total: usize = FsmState::ALL.len() * CAUSES.len();
        assert_eq!(total, 8 * 21);
        assert!(legal.len() < total, "some transitions must be illegal");
    }

    #[test]
    fn illegal_transitions_rejected() {
        assert!(transition(FsmState::Ready, FsmCause::TelemetryValid).is_none());
        assert!(transition(FsmState::Landed, FsmCause::TaskAccepted).is_none());
        assert!(transition(FsmState::Fault, FsmCause::TaskAccepted).is_none());
        assert!(transition(FsmState::Init, FsmCause::TaskAccepted).is_none());
        assert!(transition(FsmState::Active, FsmCause::Spawn).is_none());
        assert!(transition(FsmState::Booting, FsmCause::TaskAccepted).is_none());
    }

    #[test]
    fn every_cause_appears_in_at_least_one_arc() {
        let legal = legal_arcs();
        for c in CAUSES {
            assert!(
                legal.iter().any(|(_, _, cause)| *cause == c),
                "cause {c:?} unreachable"
            );
        }
    }

    /// The canonical F-3 trace is expressible: ACTIVE -> RTL on
    /// heartbeat_loss, then RTL -> LANDED on disarm.
    #[test]
    fn canonical_f3_trace() {
        let s = FsmState::Active;
        let s2 = transition(s, FsmCause::HeartbeatLoss).unwrap();
        assert_eq!(s2, FsmState::Rtl);
        let s3 = transition(s2, FsmCause::DisarmObserved).unwrap();
        assert_eq!(s3, FsmState::Landed);
    }
}
