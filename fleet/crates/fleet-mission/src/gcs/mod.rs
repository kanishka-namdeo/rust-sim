//! GCS catalog modules (ADR-0019 through ADR-0029).
//!
//! The `fleet-catalog` binary (`src/main.rs`) composes these into the
//! `:8300` REST server. Library users (tests, other crates) can import
//! the modules directly.

pub mod mission_file;
pub mod server;
pub mod store;
pub mod validation;
pub mod version_check;
