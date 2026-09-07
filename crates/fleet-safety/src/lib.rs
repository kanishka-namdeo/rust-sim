//! fleet-safety — geofence and the ordered safety policy ladder
//! (spec §8).

pub mod geofence;
pub mod policy;

pub use geofence::{Geofence, GeofenceError};
pub use policy::{PolicyEngine, PolicyInput, PolicyVerdict};
