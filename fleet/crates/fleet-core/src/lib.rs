//! fleet-core — vehicle registry, FSM, health model, event log and the
//! fleet frame (spec §4, §5).

pub mod events;
pub mod fsm;
pub mod geo;
pub mod health;
pub mod registry;
pub mod state;
pub mod tick;

pub use events::{Event, EventKind, EventLog};
pub use fsm::{legal_arcs, transition, FsmCause, FsmState};
pub use geo::GeoOrigin;
pub use health::{compute_flags, FlagEdge, HealthFlag};
pub use registry::{Registry, RegistryEntry};
pub use state::{LinkCounters, SimStatus, Slot, VehicleState};
pub use tick::{message_name, FleetFrame, SafetyAction, VehicleView};
