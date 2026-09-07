//! sitsim-sdk: scenario configuration, simulator assembly, replay, and the
//! determinism harness (SPEC §2.2, §8, §9).
//!
//! This crate is the embeddable core of the simulator: given a validated
//! [`config::ScenarioConfig`], [`engine::SimEngine`] runs pure-function
//! ticks (no I/O, no wall-clock reads) that produce wire-ready HIL messages
//! plus telemetry snapshots. The CLI crate adds sockets and the control
//! plane; other tools (mavfleet, CI harnesses) can drive the engine
//! directly.

pub mod config;
pub mod engine;
pub mod hash;
pub mod replay;

pub use config::{load_scenario_file, parse_scenario, scenario_hash, ScenarioConfig};
pub use engine::{run_headless, SimEngine, TickOutput, TickSnapshot};
pub use hash::{Fnv1a64, sha256};
pub use replay::{
    decode_header, encode_header, ReplayHeader, ReplayReader, ReplayWriter, TickRecord,
    HEADER_LEN, MAGIC, RECORD_LEN, VERSION,
};
