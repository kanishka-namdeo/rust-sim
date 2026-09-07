//! Vehicle registry: the supervisor's view of the fleet — per-vehicle state
//! slot + FSM — with transition application and event logging (spec §4).

#![forbid(unsafe_code)]

use std::sync::{Arc, Mutex};

use crate::events::EventKind;
use crate::events::EventLog;
use crate::fsm::{transition, FsmCause, FsmState};
use crate::state::{Slot, VehicleState};

pub struct RegistryEntry {
    pub index: u8,
    pub state: Arc<Slot<VehicleState>>,
    fsm: Mutex<FsmState>,
}

impl RegistryEntry {
    pub fn fsm(&self) -> FsmState {
        *self.fsm.lock().unwrap()
    }
}

#[derive(Default)]
pub struct Registry {
    entries: Vec<RegistryEntry>,
}

impl Registry {
    pub fn new(count: u8) -> Arc<Registry> {
        let entries = (0..count)
            .map(|i| RegistryEntry {
                index: i,
                state: Arc::new(Slot::new(VehicleState::new(i))),
                fsm: Mutex::new(FsmState::Init),
            })
            .collect();
        Arc::new(Registry { entries })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entry(&self, index: u8) -> Option<&RegistryEntry> {
        self.entries.get(index as usize)
    }

    pub fn indices(&self) -> Vec<u8> {
        self.entries.iter().map(|e| e.index).collect()
    }

    pub fn snapshot(&self, index: u8) -> Option<VehicleState> {
        self.entry(index).map(|e| e.state.read())
    }

    pub fn fsm(&self, index: u8) -> Option<FsmState> {
        self.entry(index).map(|e| e.fsm())
    }

    /// Apply a transition if legal; log either way. Returns the new state,
    /// or the unchanged old state when the arc is illegal (with a
    /// `fsm_rejected` event, spec §11.1).
    pub fn apply_transition(
        &self,
        index: u8,
        cause: FsmCause,
        log: &EventLog,
    ) -> Option<FsmState> {
        let entry = self.entry(index)?;
        let from = entry.fsm();
        match transition(from, cause) {
            Some(to) => {
                *entry.fsm.lock().unwrap() = to;
                log.log(
                    EventKind::FsmTransition,
                    Some(index),
                    format!("{}->{} cause={}", from.name(), to.name(), cause.name()),
                );
                Some(to)
            }
            None => {
                log.log(
                    EventKind::FsmRejected,
                    Some(index),
                    format!("rejected {}+{}", from.name(), cause.name()),
                );
                Some(from)
            }
        }
    }

    /// Direct state override for the bring-up driver (INIT -> SPAWNING ->
    /// BOOTING are driven by process events, not policy).
    pub fn force_transition(&self, index: u8, cause: FsmCause, log: &EventLog) -> Option<FsmState> {
        self.apply_transition(index, cause, log)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (Arc<Registry>, Arc<EventLog>) {
        (Registry::new(2), EventLog::in_memory())
    }

    #[test]
    fn full_lifecycle_walk() {
        let (r, log) = setup();
        use FsmCause as C;
        assert_eq!(r.apply_transition(0, C::Spawn, &log), Some(FsmState::Spawning));
        assert_eq!(r.apply_transition(0, C::Px4Launched, &log), Some(FsmState::Booting));
        assert_eq!(r.apply_transition(0, C::TelemetryValid, &log), Some(FsmState::Ready));
        assert_eq!(r.apply_transition(0, C::TaskAccepted, &log), Some(FsmState::Active));
        assert_eq!(r.apply_transition(0, C::HeartbeatLoss, &log), Some(FsmState::Rtl));
        assert_eq!(r.apply_transition(0, C::DisarmObserved, &log), Some(FsmState::Landed));
        assert_eq!(r.apply_transition(0, C::Rearm, &log), Some(FsmState::Ready));
    }

    #[test]
    fn illegal_arc_keeps_state_and_logs() {
        let (r, log) = setup();
        r.apply_transition(0, FsmCause::Spawn, &log);
        let before = r.fsm(0).unwrap();
        let after = r.apply_transition(0, FsmCause::TaskAccepted, &log);
        assert_eq!(after, Some(before));
        let kinds: Vec<_> = log.all().iter().map(|e| e.kind).collect();
        assert!(kinds.contains(&EventKind::FsmRejected));
    }

    #[test]
    fn bring_up_booting_gate_requires_telemetry_valid() {
        let (r, log) = setup();
        r.apply_transition(1, FsmCause::Spawn, &log);
        r.apply_transition(1, FsmCause::Px4Launched, &log);
        assert_eq!(r.fsm(1).unwrap(), FsmState::Booting);
        // READY only from telemetry_valid
        assert!(r.apply_transition(1, FsmCause::TaskAccepted, &log).is_some());
        assert_eq!(r.fsm(1).unwrap(), FsmState::Booting);
        assert_eq!(r.apply_transition(1, FsmCause::TelemetryValid, &log), Some(FsmState::Ready));
    }
}
