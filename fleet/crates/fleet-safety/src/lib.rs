//! fleet-safety — geofence (the polygon + altitude box model that the
//! GCS uses for inclusion checks, breach-depth reporting, and setpoint
//! clamping). The ordered 8-policy safety ladder (`policy.rs`) was
//! removed in Task 7b — autonomy safety escalation doesn't belong in a
//! GCS; the operator is in the loop.

pub mod geofence;

pub use geofence::{Geofence, GeofenceError};
