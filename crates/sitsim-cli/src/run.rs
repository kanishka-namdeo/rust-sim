//! Scenario orchestration (SPEC §2.5): configure -> listen -> wait for PX4
//! -> handshake (sim thread starts streaming on accept) -> run -> drain.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use sitsim_sdk::ScenarioConfig;
use sitsim_transport::{bind_hil_listener, serve_hil_link, LinkEvent, LinkStats};
use tokio::sync::{mpsc, watch};

use crate::api::{self, AppState, Phase};
use crate::simthread::{sim_loop, SimEnd, SimPlane};

/// Commands into the sim thread (from the HIL reader and the REST plane).
pub enum SimCommand {
    /// Latest HIL_ACTUATOR_CONTROLS: 16 control values (v1.16 PWMSim [0,1]
    /// motor scale, ADR-0011r) + the mode-field armed bit (0x80).
    Actuator { controls: [f32; 16], armed: bool },
    InjectFault(sitsim_fault::FaultSpec),
    ClearFault(String),
    /// POST /api/estop (§4.1): stop at the end of the current tick.
    EStop,
    /// The HIL link dropped (TCP EOF) — wind down (§2.5).
    Stop,
}

/// Why the run ended (maps to exit codes; §4.3).
pub enum RunEnd {
    DurationReached,
    EStop,
    Px4Disconnected,
    /// `--wait-timeout-s` expired while waiting for PX4 (ADR-014).
    WaitTimeout,
    /// Numerical divergence in the dynamics (ADR-013).
    Diverged,
}

impl RunEnd {
    pub fn exit_code(&self) -> u8 {
        match self {
            RunEnd::DurationReached | RunEnd::EStop => 0,
            RunEnd::Px4Disconnected => 3,
            RunEnd::WaitTimeout => 4,
            RunEnd::Diverged => 5,
        }
    }
}

pub fn run(opts: crate::RunOpts) -> Result<u8, String> {
    // ---- Configure (load + validate; exit 4 on error).
    let mut cfg = sitsim_sdk::load_scenario_file(&opts.scenario_path)?;
    if let Some(speed) = opts.speed {
        cfg.sim.speed = speed;
    }
    if let Some(d) = opts.duration_s {
        cfg.sim.duration_s = d;
    }
    sitsim_sdk::config::validate(&cfg).map_err(|e| e.to_string())?;
    if cfg.sim.speed < 0.0 {
        return Err("speed must be >= 0".into());
    }

    let replay_path = opts.replay_out.clone().unwrap_or_else(|| default_replay_path(&opts.scenario_path));
    let scenario_hash = sitsim_sdk::scenario_hash(&cfg);

    // Structured JSON logs on stderr (§4.3), tick metrics via the 10 s info
    // lines from the sim thread.
    init_tracing();

    let code = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("runtime: {e}"))?
        .block_on(run_async(cfg, opts, replay_path, scenario_hash))?;

    Ok(code)
}

pub(crate) fn default_replay_path(scenario: &PathBuf) -> PathBuf {
    let stem = scenario
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "scenario".into());
    PathBuf::from(format!("{stem}.replay"))
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .json()
        .with_target(false)
        .init();
}

async fn run_async(
    cfg: ScenarioConfig,
    opts: crate::RunOpts,
    replay_path: PathBuf,
    scenario_hash: [u8; 32],
) -> Result<u8, String> {
    let bind_ip: std::net::IpAddr = cfg
        .io
        .bind
        .parse()
        .map_err(|e| format!("io.bind '{}': {e}", cfg.io.bind))?;
    let hil_addr = SocketAddr::new(bind_ip, cfg.io.tcp_port);
    let api_addr = SocketAddr::new(bind_ip, cfg.io.api_port);

    // ---- Listen (with retry/backoff for stale instance sockets, §2.5).
    let hil_listener = bind_hil_listener(hil_addr, 5)
        .await
        .map_err(|e| format!("cannot bind HIL TCP {hil_addr}: {e}"))?;
    let api_listener = tokio::net::TcpListener::bind(api_addr)
        .await
        .map_err(|e| format!("cannot bind control plane {api_addr}: {e}"))?;

    tracing::info!(
        hil_addr = %hil_addr,
        api_addr = %api_addr,
        rate_hz = cfg.sim.rate_hz,
        duration_s = cfg.sim.duration_s,
        speed = cfg.sim.speed,
        seed = cfg.sim.seed,
        scenario_sha256 = scenario_hash.iter().map(|b| format!("{b:02x}")).collect::<String>(),
        replay = %replay_path.display(),
        "rustsitsim listening; waiting for PX4 to connect"
    );

    // ---- Shared state + channels.
    let stats = Arc::new(LinkStats::default());
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<LinkEvent>();
    let (frame_tx, frame_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<SimCommand>();
    // Moved into the sim thread exactly once, on the (single) accept.
    let mut sim_boot: Option<(
        mpsc::UnboundedReceiver<SimCommand>,
        mpsc::UnboundedSender<Vec<u8>>,
    )> = Some((cmd_rx, frame_tx));
    let (plane_tx, plane_rx) = watch::channel(SimPlane {
        tick: 0,
        t_us: 0,
        snapshot: Default::default(),
        sent_hil_sensor: 0,
        sent_hil_state_quaternion: 0,
        sent_hil_gps: 0,
        tick_p50_us: 0,
        tick_p95_us: 0,
        tick_p99_us: 0,
        tick_p999_us: 0,
        tick_max_us: 0,
        telemetry_hash: None,
    });
    let (done_tx, mut done_rx) = mpsc::unbounded_channel::<SimEnd>();
    let (shut_tx, shut_rx) = watch::channel(false);

    let state = Arc::new(AppState {
        cfg: RwLock::new(cfg.clone()),
        stats: Arc::clone(&stats),
        phase: AtomicU8::new(Phase::Wait as u8),
        plane: plane_rx.clone(),
        cmd_tx: cmd_tx.clone(),
        replay_path: replay_path.clone(),
        scenario_hash,
        inject_counter: AtomicU64::new(0),
        started: Instant::now(),
        shut_tx: shut_tx.clone(),
    });

    // ---- HIL link + control plane.
    let link_task = tokio::spawn(serve_hil_link(
        hil_listener,
        Arc::clone(&stats),
        event_tx,
        frame_rx,
        shut_rx.clone(),
    ));
    let api_task = tokio::spawn(api::serve(api_listener, Arc::clone(&state)));

    // ---- Event loop: route link events, spawn the sim thread on accept,
    // finish on sim end.
    let wait_deadline = (opts.wait_timeout_s > 0)
        .then(|| Instant::now() + Duration::from_secs(opts.wait_timeout_s));
    let mut sim_running = false;
    let mut end: Option<RunEnd> = None;

    loop {
        tokio::select! {
            ev = event_rx.recv() => {
                match ev {
                    Some(LinkEvent::Connected(peer)) => {
                        tracing::info!(%peer, "PX4 connected; starting the tick loop");
                        state.phase.store(Phase::Run as u8, Ordering::SeqCst);
                        // Build the engine from the (possibly replaced)
                        // scenario under the lock.
                        let cfg_now = state.cfg.read().expect("cfg lock").clone();
                        let done_tx = done_tx.clone();
                        let (cmd_rx, frame_tx) = sim_boot
                            .take()
                            .expect("exactly one connection per run (§3.1)");
                        let plane_tx = plane_tx.clone();
                        let replay_path = replay_path.clone();
                        let print_hash = opts.telemetry_hash;
                        // The sim thread is plain synchronous code (§2.4).
                        let handle = std::thread::Builder::new()
                            .name("sitsim-sim".into())
                            .spawn(move || {
                                sim_loop(cfg_now, cmd_rx, frame_tx, plane_tx, done_tx, replay_path, print_hash);
                            })
                            .expect("spawn sim thread");
                        let _ = handle; // joined implicitly at process end
                        sim_running = true;
                    }
                    Some(LinkEvent::Actuator(a)) => {
                        // ADR-0011r: the armed bit is mode & 0x80.
                        let _ = cmd_tx.send(SimCommand::Actuator {
                            controls: a.controls,
                            armed: a.mode & 0x80 != 0,
                        });
                    }
                    Some(LinkEvent::Disconnected) => {
                        if sim_running {
                            let _ = cmd_tx.send(SimCommand::Stop);
                        }
                    }
                    None => {
                        // serve_hil_link task ended without a Disconnected
                        // event (shutdown path).
                        if sim_running {
                            let _ = cmd_tx.send(SimCommand::Stop);
                        } else if end.is_none() {
                            end = Some(RunEnd::WaitTimeout);
                        }
                    }
                }
            }
            done = done_rx.recv() => {
                if let Some(sim_end) = done {
                    end = Some(match sim_end {
                        SimEnd::DurationReached => RunEnd::DurationReached,
                        SimEnd::EStop => RunEnd::EStop,
                        SimEnd::Px4Disconnected => RunEnd::Px4Disconnected,
                        SimEnd::Diverged => RunEnd::Diverged,
                    });
                }
            }
            _ = maybe_deadline(wait_deadline), if !sim_running && end.is_none() => {
                end = Some(RunEnd::WaitTimeout);
            }
        }
        if end.is_some() {
            break;
        }
    }

    // ---- Drain (§2.5): sim thread has finished (or never started); wind
    // the link down, stop the control plane, exit with the mapped code.
    state.phase.store(Phase::Draining as u8, Ordering::SeqCst);
    let _ = shut_tx.send(true);
    let _ = tokio::time::timeout(Duration::from_secs(3), link_task).await;
    state.phase.store(Phase::Done as u8, Ordering::SeqCst);
    api_task.abort();

    let code = end.map(|e| e.exit_code()).unwrap_or(0);
    tracing::info!(exit_code = code, "run complete");
    Ok(code)
}

/// `select!`-compatible deadline that never fires when `None`.
async fn maybe_deadline(d: Option<Instant>) {
    match d {
        Some(t) => tokio::time::sleep_until(tokio::time::Instant::from_std(t)).await,
        None => std::future::pending::<()>().await,
    }
}
