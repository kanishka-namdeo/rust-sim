//! Health flag model (spec §5.2) with rising-edge discipline.

#![forbid(unsafe_code)]

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum HealthFlag {
    /// No telemetry for 1.5 s.
    LinkStale,
    /// No heartbeat for 3 s.
    HeartbeatLost,
    /// Battery under 30 percent.
    BatteryLow,
    /// Battery under 20 percent.
    BatteryCrit,
    /// Within 10 m of a geofence boundary (computed by the safety engine).
    GeofenceWarn,
    /// Mode left OFFBOARD unexpectedly.
    OffboardDropout,
    /// Sim status reports tick p95 over budget.
    SimDegraded,
}

impl HealthFlag {
    pub fn name(&self) -> &'static str {
        match self {
            HealthFlag::LinkStale => "LINK_STALE",
            HealthFlag::HeartbeatLost => "HEARTBEAT_LOST",
            HealthFlag::BatteryLow => "BATTERY_LOW",
            HealthFlag::BatteryCrit => "BATTERY_CRIT",
            HealthFlag::GeofenceWarn => "GEOFENCE_WARN",
            HealthFlag::OffboardDropout => "OFFBOARD_DROPOUT",
            HealthFlag::SimDegraded => "SIM_DEGRADED",
        }
    }
}

/// Staleness / battery thresholds (spec §4, §5.2, §8.1).
pub const STALE_MS: u64 = 1500;
pub const HEARTBEAT_LOST_MS: u64 = 3000;
pub const BATTERY_LOW_PCT: i8 = 30;
pub const BATTERY_CRIT_PCT: i8 = 20;

/// Compute the flag set for one vehicle from its state snapshot.
/// `geofence_warn` is supplied by the safety engine (it owns the fence);
/// `sim_degraded` likewise.
pub fn compute_flags(
    state: &crate::state::VehicleState,
    now_ms: u64,
    geofence_warn: bool,
    sim_degraded: bool,
) -> Vec<HealthFlag> {
    let mut flags = Vec::new();
    let msg_age = now_ms.saturating_sub(state.last_msg_ms);
    let hb_age = now_ms.saturating_sub(state.last_heartbeat_ms);
    if state.last_msg_ms > 0 && msg_age > STALE_MS {
        flags.push(HealthFlag::LinkStale);
    }
    if state.last_heartbeat_ms > 0 && hb_age > HEARTBEAT_LOST_MS {
        flags.push(HealthFlag::HeartbeatLost);
    }
    // Battery flags only when the estimate is known (the interim sim has no
    // battery model — the ladder is disabled with a logged warning, §8.1).
    if state.battery_pct >= 0 {
        if state.battery_pct < BATTERY_LOW_PCT {
            flags.push(HealthFlag::BatteryLow);
        }
        if state.battery_pct < BATTERY_CRIT_PCT {
            flags.push(HealthFlag::BatteryCrit);
        }
    }
    if geofence_warn {
        flags.push(HealthFlag::GeofenceWarn);
    }
    if sim_degraded {
        flags.push(HealthFlag::SimDegraded);
    }
    flags
}

/// Rising-edge tracker: flags raise supervisor events once, never
/// repeatedly (spec §5.2).
#[derive(Debug, Default)]
pub struct FlagEdge {
    prev: Vec<HealthFlag>,
}

impl FlagEdge {
    pub fn update(&mut self, current: &[HealthFlag]) -> Vec<HealthFlag> {
        let rising: Vec<HealthFlag> = current
            .iter()
            .copied()
            .filter(|f| !self.prev.contains(f))
            .collect();
        self.prev = current.to_vec();
        rising
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::VehicleState;

    fn state_with(now: u64, last_msg: u64, last_hb: u64, battery: i8) -> VehicleState {
        let mut s = VehicleState::new(0);
        s.last_msg_ms = last_msg;
        s.last_heartbeat_ms = last_hb;
        s.battery_pct = battery;
        let _ = now;
        s
    }

    #[test]
    fn staleness_thresholds() {
        // fresh
        assert!(compute_flags(&state_with(1000, 900, 900, 100), 1000, false, false).is_empty());
        // 1.5s stale
        let f = compute_flags(&state_with(3000, 1000, 1000, 100), 3000, false, false);
        assert!(f.contains(&HealthFlag::LinkStale));
        assert!(!f.contains(&HealthFlag::HeartbeatLost));
        // 3s heartbeat loss
        let f = compute_flags(&state_with(4100, 1000, 1000, 100), 4100, false, false);
        assert!(f.contains(&HealthFlag::HeartbeatLost));
        assert!(f.contains(&HealthFlag::LinkStale));
    }

    #[test]
    fn battery_thresholds_and_unknown() {
        assert!(compute_flags(&state_with(0, 1, 1, 35), 0, false, false).is_empty());
        let f = compute_flags(&state_with(0, 1, 1, 25), 0, false, false);
        assert!(f.contains(&HealthFlag::BatteryLow));
        assert!(!f.contains(&HealthFlag::BatteryCrit));
        let f = compute_flags(&state_with(0, 1, 1, 15), 0, false, false);
        assert!(f.contains(&HealthFlag::BatteryCrit));
        // unknown battery (-1): no flags
        assert!(compute_flags(&state_with(0, 1, 1, -1), 0, false, false).is_empty());
    }

    #[test]
    fn rising_edge_only_once() {
        let mut e = FlagEdge::default();
        let flags = [HealthFlag::LinkStale];
        assert_eq!(e.update(&flags).len(), 1);
        assert!(e.update(&flags).is_empty());
        // clears and re-raises
        assert!(e.update(&[]).is_empty());
        assert_eq!(e.update(&flags).len(), 1);
    }

    #[test]
    fn no_staleness_before_first_message() {
        // BOOTING: no telemetry yet — must not flag (age unknown, §4).
        let s = state_with(10_000, 0, 0, 100);
        assert!(compute_flags(&s, 10_000, false, false).is_empty());
    }
}
