//! fleet-mission: the scenario DSL, mission compiler, per-vehicle task
//! runner, and run report / success criteria (spec §9, §6, §7).
//!
//! Composition (which crate owns which half) per ADR-0006: the pure
//! decision logic lives here and in fleet-core/fleet-safety/fleet-alloc;
//! the async composition root — links, processes, the tick loop — lives in
//! fleet-cli.
//!
//! GCS catalog (ADR-0019 through ADR-0029): the `gcs` module owns the
//! mission file format, filesystem store, validation rules, PX4 version
//! check, and the `:8300` REST server. The `fleet-catalog` binary
//! (`src/main.rs`) is the composition root.

#![forbid(unsafe_code)]

pub mod compile;
pub mod gcs;
pub mod report;
pub mod runner;
pub mod scenario;

pub use compile::{transit_altitude_m, MissionPlan};
pub use report::{
    evaluate_criteria, parse_cause, parse_state, unix_now, Criterion, FsmArc, RunReport, TaskDone,
    TaskStatus, VehicleReport, EXIT_ABORTED, EXIT_CRITERIA_FAILED, EXIT_OK,
};
pub use runner::{MissionRunner, RunnerCmd, RunnerEvent, RunnerInput, RunnerPhase, RunnerTask};
pub use scenario::{EventSpec, Scenario, ScenarioError, SuccessSection, TaskSpec};
