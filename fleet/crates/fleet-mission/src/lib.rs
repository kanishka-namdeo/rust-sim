//! fleet-mission: the GCS mission catalog (ADR-0019 through ADR-0029).
//!
//! The `gcs` module owns the mission file format, filesystem store,
//! validation rules, PX4 version check, ULog browse, and the `:8300`
//! REST server. The `fleet-catalog` binary (`src/main.rs`) is the
//! composition root.
//!
//! Removed in Task 7b: the scenario DSL (`scenario.rs`), the mission
//! compiler (`compile.rs`), the per-vehicle task runner (`runner/`),
//! and the run-report / success-criteria module (`report.rs`). Those
//! were the autonomy half (auction-driven mission execution + CI
//! criteria); the GCS is operator-driven, so they don't belong here.
//! `TaskStatus` and `unix_now` moved to `fleet-core`.

#![forbid(unsafe_code)]

pub mod gcs;
