//! Environment models (SPEC §6.1-6.2): ISA atmosphere, WGS84 tangent-plane
//! geodesy, dipole-style magnetic field, and Gauss-Markov (Dryden-class)
//! wind turbulence.
//!
//! All models are pure functions of (state, RNG) — no wall-clock reads, no
//! I/O — preserving the SPEC §8.1 determinism contract.

pub mod atmosphere;
pub mod geodesy;
pub mod mag;
pub mod wind;

pub use atmosphere::{pressure_alt_m, pressure_field_units, temperature_c};
pub use geodesy::GeoOrigin;
pub use mag::MagFieldModel;
pub use wind::{Turbulence, WindModel};
