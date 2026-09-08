//! mavfleet — the fleet-cli composition root binary (spec §2.2, last table
//! row: "Binary: scenario load, fleet bring-up, REST/WS plane, exit codes").
//!
//! ADR-0006 splits the world: every library crate owns pure decision logic
//! (FSM, health, policy, allocation, profiles); this binary owns the async
//! composition — links, processes, the 10 Hz supervisor tick, the mission
//! engine, the run report and the control plane.

#![forbid(unsafe_code)]

mod airframes;
mod api;
mod manager;
mod pump;
mod report;
mod setup;
mod state;

use std::path::{Path, PathBuf};

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
    /// Validate a fleet scenario (parse + schema + compile). Exit 0 iff valid.
    Check {
        /// Scenario TOML file (spec §9.1 schema).
        scenario: PathBuf,
    },
    /// Run a fleet scenario end-to-end: spawn, supervise, fly, report, teardown.
    Run {
        /// Scenario TOML file (spec §9.1 schema).
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
        Command::Check { scenario } => cmd_check(&scenario),
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

/// `mavfleet check <toml>`: exit code reflects validity (0 valid, 1 invalid).
fn cmd_check(path: &Path) -> i32 {
    match fleet_mission::Scenario::parse_file(path) {
        Ok((scenario, source)) => {
            let plan = fleet_mission::MissionPlan::compile(&scenario);
            println!("scenario: {source}");
            println!("  fleet:      {} vehicles, tick {} Hz, battery_sim {}, restart_on_fault {}",
                scenario.count(), scenario.fleet.tick_hz, scenario.fleet.battery_sim, scenario.fleet.restart_on_fault);
            let fence = scenario.fence().expect("validated");
            println!("  geofence:   {} vertices, ceiling {} m, floor {} m",
                fence.points.len(), fence.ceiling_m, fence.floor_m);
            println!("  sim:        duration {} s{}",
                scenario.sim_duration_s(),
                scenario.sim_command().map(|c| format!(", command `{c}`")).unwrap_or_default());
            println!("  tasks:      {} accepted, {} rejected at compile",
                plan.tasks.len(), plan.rejections.len());
            for (id, reason) in &plan.rejections {
                println!("    REJECT {id}: {reason}");
            }
            for ev in &scenario.events {
                println!("  event:      {} vehicle={:?} start={:?}s duration={:?}s",
                    ev.kind, ev.vehicle, ev.start_s, ev.duration_s);
            }
            println!("  success:    all_tasks_done={:?} all_landed={:?} max_time_s={:?} no_geofence_breach={:?} fsm_trace={}",
                scenario.success.all_tasks_done, scenario.success.all_landed,
                scenario.success.max_time_s, scenario.success.no_geofence_breach,
                scenario.success.fsm_trace.len());
            println!("OK: scenario is valid");
            0
        }
        Err(e) => {
            eprintln!("INVALID: {e}");
            1
        }
    }
}
