//! sitsim-cli: the rustsitsim binary.
//!
//! Subcommands:
//! - `scenario-run <file.toml>`: load a scenario, serve the HIL link
//!   (TCP 4560+i) and the control plane (REST+WS on the api_port), run
//!   the tick loop until the scenario ends / PX4 disconnects / REST stop.
//! - `replay-info <file>`: print a replay header + record summary.
//!
//! Exit codes (SPEC §4.3): 0 clean end (scenario duration or REST stop),
//! 2 scenario failure, 3 PX4 disconnect, 4 configuration error.

mod api;
mod run;
mod simthread;

use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Debug, Clone)]
pub struct RunOpts {
    pub scenario_path: PathBuf,
    /// Override [sim] speed (wall-clock pacing factor; 0 = unbounded).
    pub speed: Option<f32>,
    /// Override duration_s.
    pub duration_s: Option<f32>,
    /// Replay output path (default: <scenario stem>.replay in the CWD).
    pub replay_out: Option<PathBuf>,
    /// Print the final telemetry hash (determinism harness, §8.1) to stdout.
    pub telemetry_hash: bool,
    /// Exit if PX4 has not connected within this many seconds (0 = wait
    /// forever).
    pub wait_timeout_s: u64,
}

const USAGE: &str = "\
rustsitsim (sitsim-cli) — lockstep HIL flight simulator for PX4 SITL

USAGE:
  sitsim-cli scenario-run <scenario.toml> [OPTIONS]
  sitsim-cli replay-info <file.replay>
  sitsim-cli --help

scenario-run OPTIONS:
  --speed <F>            Wall-clock pacing factor: 1.0 = real time (default
                         from [sim] speed), 0 = unbounded virtual time
  --duration-s <F>       Override [sim] duration_s
  --replay-out <PATH>    Replay output path (default <scenario>.replay)
  --telemetry-hash       Print the final 64-bit telemetry hash to stdout
                         (determinism harness, SPEC §8.1)
  --wait-timeout-s <N>   Fail (exit 4) if PX4 does not connect within N s
                         (default: wait forever)

The control plane (REST + WebSocket, SPEC §4) serves on [io] api_port
(default 8200): GET /api/status, GET/PUT /api/scenario, GET/POST
/api/faults, DELETE /api/faults/{id}, POST /api/estop, GET /api/replay,
WebSocket at /ws/telemetry (and at / for gateway-forwarded sockets).

Exit codes: 0 clean, 2 scenario failure, 3 PX4 disconnect, 4 config error.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else {
        print!("{USAGE}");
        return ExitCode::from(0);
    };

    match cmd.as_str() {
        "--help" | "-h" | "help" => {
            print!("{USAGE}");
            ExitCode::from(0)
        }
        "scenario-run" => {
            let opts = parse_run_opts(&args[1..]).unwrap_or_else(|e| {
                eprintln!("error: {e}\n\n{USAGE}");
                std::process::exit(4);
            });
            match run::run(opts) {
                Ok(code) => ExitCode::from(code),
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::from(4)
                }
            }
        }
        "replay-info" => {
            let Some(path) = args.get(1) else {
                eprintln!("error: replay-info needs a file argument\n\n{USAGE}");
                return ExitCode::from(4);
            };
            match replay_info(&PathBuf::from(path)) {
                Ok(()) => ExitCode::from(0),
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::from(4)
                }
            }
        }
        other => {
            eprintln!("error: unknown subcommand `{other}`\n\n{USAGE}");
            ExitCode::from(4)
        }
    }
}

fn parse_run_opts(args: &[String]) -> Result<RunOpts, String> {
    let mut scenario_path: Option<PathBuf> = None;
    let mut speed = None;
    let mut duration_s = None;
    let mut replay_out = None;
    let mut telemetry_hash = false;
    let mut wait_timeout_s = 0u64;

    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        let mut next = || {
            i += 1;
            args.get(i).cloned().ok_or_else(|| format!("missing value after {a}"))
        };
        match a.as_str() {
            "--speed" => speed = Some(next()?.parse::<f32>().map_err(|e| format!("--speed: {e}"))?),
            "--duration-s" => {
                duration_s = Some(next()?.parse::<f32>().map_err(|e| format!("--duration-s: {e}"))?)
            }
            "--replay-out" => replay_out = Some(PathBuf::from(next()?)),
            "--telemetry-hash" => telemetry_hash = true,
            "--wait-timeout-s" => {
                wait_timeout_s = next()?.parse::<u64>().map_err(|e| format!("--wait-timeout-s: {e}"))?
            }
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            s => {
                if scenario_path.is_some() {
                    return Err(format!("unexpected positional argument {s}"));
                }
                scenario_path = Some(PathBuf::from(s));
            }
        }
        i += 1;
    }

    Ok(RunOpts {
        scenario_path: scenario_path.ok_or("scenario-run needs a scenario TOML path")?,
        speed,
        duration_s,
        replay_out,
        telemetry_hash,
        wait_timeout_s,
    })
}

fn replay_info(path: &PathBuf) -> Result<(), String> {
    let mut r = sitsim_sdk::ReplayReader::open(path)?;
    let h = r.header().clone();
    let mut last_t = h.start_t_us;
    let mut records = 0u64;
    let mut last_state = [0f32; 17];
    while let Some(rec) = r.next_record()? {
        last_t = rec.t_us;
        last_state = rec.state;
        records += 1;
    }
    let hash_hex: String = h.scenario_hash.iter().map(|b| format!("{b:02x}")).collect();
    println!("file:                 {}", path.display());
    println!("version:              {}", h.version);
    println!("tick_rate_hz:         {}", h.rate_hz);
    println!("seed:                 {}", h.seed);
    println!("scenario_sha256:      {hash_hex}");
    println!("records:              {records}");
    if records > 0 {
        let dur_s = (last_t - h.start_t_us) as f64 / 1e6;
        println!("virtual_duration_s:   {dur_s:.3}");
        println!("final_pos_ned_m:      [{:.3}, {:.3}, {:.3}]", last_state[0], last_state[1], last_state[2]);
        println!("final_q_wxyz:         [{:.5}, {:.5}, {:.5}, {:.5}]",
            last_state[6], last_state[7], last_state[8], last_state[9]);
    }
    Ok(())
}
