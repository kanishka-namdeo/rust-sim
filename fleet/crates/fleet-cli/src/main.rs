//! mavfleet — the fleet-cli composition root binary (spec §2.2):
//! "Binary: fleet bring-up, REST/WS plane, exit codes".
//!
//! ADR-0006 splits the world: every library crate owns pure decision logic
//! (FSM, health, geofence, MAVLink codec); this binary owns the async
//! composition — links, processes, the 10 Hz supervisor tick, the
//! control plane, and the run teardown.
//!
//! Task 7b removed the scenario-DSL half: the `mavfleet check` subcommand
//! (which validated the scenario DSL we deleted) and the autonomy the
//! supervisor used to drive (auction allocator, mission runner, safety
//! policy engine, fault-event timeline). The lean `mavfleet run` spawns
//! N PX4 SITL + sim pairs, binds N MAVLink links, aggregates state into
//! FleetFrame at 10 Hz, serves the REST/WS plane, and lets the operator
//! drive the rest.

#![forbid(unsafe_code)]

mod airframes;
mod api;
mod config;
mod manager;
mod pump;
mod setup;
mod state;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "mavfleet",
    version,
    about = "mavfleet: multi-vehicle PX4 SITL fleet manager (spec v0.1)",
    disable_help_subcommand = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run a fleet end-to-end: spawn N PX4 SITL + sim pairs, bind N MAVLink
    /// links, serve the REST + WS control plane on :8400, aggregate the
    /// fleet frame at 10 Hz, and let the operator drive every flight
    /// action via the API (arm/land/rtl/hold/goto, mission upload/start,
    /// fleet start, e-stop). The run ends at SIGINT or the hard wall-clock
    /// cap (default 5 min).
    Run {
        /// Fleet TOML file (the lean config: `[fleet]`, `[env]`, `[sim]`).
        #[arg(long, value_name = "FILE")]
        fleet: PathBuf,
        /// REST/WS control-plane port (spec §3.4; binds 127.0.0.1 only).
        #[arg(long, default_value_t = 8400)]
        api_port: u16,
        /// Run directory (default: fleet-runs/fleet-run-<unix_s>).
        #[arg(long, value_name = "DIR")]
        run_dir: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let code = match cli.command {
        Command::Run {
            fleet,
            api_port,
            run_dir,
        } => {
            manager::run_scenario(manager::RunArgs {
                fleet,
                api_port,
                run_dir,
            })
            .await
        }
    };
    std::process::exit(code);
}
