//! The sim thread (SPEC §2.4): owns the engine, virtual time, the replay
//! writer, and the telemetry hash. Plain synchronous loop, no locks, no
//! wall-clock reads inside the model — a monotonic clock is consulted only
//! for pacing and performance metrics, never for computed values.

use std::time::{Duration, Instant};

use sitsim_mavlink::{Frame, Message, SYS_ID, COMP_ID};
use sitsim_sdk::{Fnv1a64, ReplayWriter, ScenarioConfig, SimEngine, TickRecord};
use tokio::sync::{mpsc, watch};

use crate::run::SimCommand;

/// What the sim thread reports when it finishes (maps to exit codes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimEnd {
    /// Scenario duration reached.
    DurationReached,
    /// REST estop (§4.1): stop at the end of the current tick.
    EStop,
    /// PX4 closed the HIL link (TCP EOF) — exit code 3 (§2.5).
    Px4Disconnected,
    /// Numerical divergence (ADR-013) — exit code 5. The diagnostic line
    /// is emitted by the engine the moment it latches; the sim never
    /// streams NaN frames.
    Diverged,
}

/// Everything the I/O plane needs at 10 Hz: the snapshot plus sim-side
/// counters and tick-time percentiles (§4.1/§4.2/§8.3).
#[derive(Debug, Clone)]
pub struct SimPlane {
    pub tick: u64,
    pub t_us: u64,
    pub snapshot: sitsim_sdk::TickSnapshot,
    pub sent_hil_sensor: u64,
    pub sent_hil_state_quaternion: u64,
    pub sent_hil_gps: u64,
    pub tick_p50_us: u64,
    pub tick_p95_us: u64,
    pub tick_p99_us: u64,
    pub tick_p999_us: u64,
    pub tick_max_us: u64,
    /// Set on the final plane after the loop ends.
    pub telemetry_hash: Option<u64>,
}

/// Percentile over the 1024-sample duration ring (§8.3).
fn percentile(mut v: Vec<u64>, p: f64) -> u64 {
    if v.is_empty() {
        return 0;
    }
    v.sort_unstable();
    let idx = ((p / 100.0) * (v.len() - 1) as f64).round() as usize;
    v[idx.min(v.len() - 1)]
}

/// Run the sim loop on the current (blocking) thread.
///
/// `cmd_rx` receives actuator updates and control commands (from the HIL
/// reader and the REST plane); `frame_tx` carries encoded MAVLink bytes to
/// the link writer; `plane_tx` publishes 10 Hz snapshots; `done_tx`
/// notifies the async side when the loop ends.
#[allow(clippy::too_many_arguments)]
pub fn sim_loop(
    cfg: ScenarioConfig,
    mut cmd_rx: mpsc::UnboundedReceiver<SimCommand>,
    frame_tx: mpsc::UnboundedSender<Vec<u8>>,
    plane_tx: watch::Sender<SimPlane>,
    done_tx: mpsc::UnboundedSender<SimEnd>,
    replay_path: std::path::PathBuf,
    print_hash: bool,
) {
    let rate = cfg.sim.rate_hz as f64;
    let h = 1.0 / rate;
    let speed = cfg.sim.speed as f64;
    let duration_us = (cfg.sim.duration_s.max(0.0) * 1e6) as u64;
    let scenario_hash = sitsim_sdk::scenario_hash(&cfg);
    let tick_us_exact = (h * 1e6).round() as u64;

    let mut engine = SimEngine::new(cfg.clone());
    let mut replay = match ReplayWriter::create(&replay_path, rate, cfg.sim.seed, &scenario_hash) {
        Ok(w) => w,
        Err(e) => {
            tracing::error!(path = %replay_path.display(), error = %e, "cannot create replay file");
            let _ = done_tx.send(SimEnd::DurationReached);
            return;
        }
    };

    let mut hasher = Fnv1a64::new();
    let mut seq: u8 = 0;
    let mut tick_durations: Vec<u64> = Vec::with_capacity(1024);
    let mut sent_hil_sensor = 0u64;
    let mut sent_hil_state_quaternion = 0u64;
    let mut sent_hil_gps = 0u64;
    // F-09 virtual-time delay queue: (release_t_us, bytes).
    let mut delay_queue: Vec<(u64, Vec<u8>)> = Vec::new();
    let snapshot_period = ((rate / 10.0).ceil() as u64).max(1); // 10 Hz
    // Latest actuator frame from PX4 (held across ticks, §2.3): (controls,
    // armed bit from the mode field, ADR-0011r).
    let mut pending_controls: Option<([f32; 16], bool)> = None;
    let wall_start = Instant::now();
    let mut end: Option<SimEnd> = None;

    while end.is_none() {
        // ---- Step 1 (Receive): drain commands (non-blocking).
        let mut estop = false;
        let mut disconnected = false;
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                SimCommand::Actuator { controls, armed } => pending_controls = Some((controls, armed)),
                SimCommand::InjectFault(spec) => {
                    let id = engine.inject_fault(spec);
                    tracing::info!(id, "runtime fault injected");
                }
                SimCommand::ClearFault(id) => {
                    if engine.clear_fault(&id) {
                        tracing::info!(id, "fault cleared");
                    }
                }
                SimCommand::EStop => estop = true,
                SimCommand::Stop => disconnected = true,
            }
        }
        if estop {
            end = Some(SimEnd::EStop);
            break;
        }
        if disconnected {
            end = Some(SimEnd::Px4Disconnected);
            break;
        }

        let controls = pending_controls.take();
        let raw_controls = controls; // RAW commanded values (pre-fault), for the replay record

        let t_start = Instant::now();
        let out = match &controls {
            Some((c, armed)) => engine.tick(Some(c), Some(*armed)),
            None => engine.tick(None, None),
        };
        let tick_us = t_start.elapsed().as_micros() as u64;

        // ---- Telemetry hash (§8.1): every 100th tick.
        if out.tick % 100 == 0 {
            for v in engine.state_vector() {
                hasher.update_f64(v);
            }
        }

        // ---- Replay record (§8.2): RAW commanded values in the [0,1]
        // wire convention (ADR-0011r; byte = round(u*255)).
        let mut motors = [0f64; 16];
        if let Some((raw, _armed)) = raw_controls {
            for (i, m) in motors.iter_mut().enumerate() {
                *m = (raw[i] as f64).clamp(0.0, 1.0);
            }
        }
        let rec = TickRecord {
            t_us: out.t_us,
            motors,
            state: {
                let mut s = [0f32; 17];
                for (k, v) in out.snapshot.pos_ned_m.iter().enumerate() {
                    s[k] = *v as f32;
                }
                for (k, v) in out.snapshot.vel_ned_ms.iter().enumerate() {
                    s[3 + k] = *v as f32;
                }
                for (k, v) in out.snapshot.q_wxyz.iter().enumerate() {
                    s[6 + k] = *v as f32;
                }
                for (k, v) in out.snapshot.omega_body_rads.iter().enumerate() {
                    s[10 + k] = *v as f32;
                }
                for (k, v) in out.snapshot.rotors_rads.iter().enumerate() {
                    s[13 + k] = *v as f32;
                }
                s
            },
            fault_flags: out.snapshot.fault_flags,
        };
        if let Err(e) = replay.push(&rec) {
            tracing::warn!(error = %e, "replay write failed; continuing");
        }

        // Divergence (ADR-013): record the (already NaN) tick as replay
        // evidence, then stop — NaN frames must never reach PX4.
        if out.diverged {
            tracing::error!(tick = out.tick, t_us = out.t_us, "numerical divergence: stopping run (exit 5)");
            end = Some(SimEnd::Diverged);
            break;
        }

        // ---- Step 5 (Encode and send), with F-09 delay / F-10 drop.
        let mut push = |msg: Message, seq: &mut u8| {
            let f = Frame::from_message(&msg, *seq, SYS_ID, COMP_ID);
            *seq = seq.wrapping_add(1);
            let bytes = f.encode();
            if out.transport_delay_ms > 0.0 {
                delay_queue.push((out.t_us + (out.transport_delay_ms * 1000.0) as u64, bytes));
            } else {
                let _ = frame_tx.send(bytes);
            }
        };
        if !out.drop_hil_sensor {
            push(Message::HilSensor(out.hil_sensor), &mut seq);
            sent_hil_sensor += 1;
        }
        push(Message::HilStateQuaternion(out.hil_state_quaternion), &mut seq);
        sent_hil_state_quaternion += 1;
        if let Some(gps) = out.hil_gps {
            if !out.drop_hil_gps {
                push(Message::HilGps(gps), &mut seq);
                sent_hil_gps += 1;
            }
        }
        release_due(&mut delay_queue, out.t_us, &frame_tx);

        // ---- Stats + 10 Hz snapshot (§2.4).
        if tick_durations.len() < 1024 {
            tick_durations.push(tick_us);
        } else {
            tick_durations[(out.tick % 1024) as usize] = tick_us;
        }
        if out.tick % snapshot_period == 0 {
            let durs = tick_durations.clone();
            let plane = SimPlane {
                tick: out.tick,
                t_us: out.t_us,
                snapshot: out.snapshot.clone(),
                sent_hil_sensor,
                sent_hil_state_quaternion,
                sent_hil_gps,
                tick_p50_us: percentile(durs.clone(), 50.0),
                tick_p95_us: percentile(durs.clone(), 95.0),
                tick_p99_us: percentile(durs.clone(), 99.0),
                tick_p999_us: percentile(durs.clone(), 99.9),
                tick_max_us: durs.iter().copied().max().unwrap_or(0),
                telemetry_hash: None,
            };
            let _ = plane_tx.send(plane);
        }

        // ---- Duration end condition.
        if duration_us > 0 && out.t_us >= duration_us {
            end = Some(SimEnd::DurationReached);
        }

        // ---- Pacing (ADR-007): keep tick wall time near virtual time /
        // speed. Chunked sleeps keep estop responsive at slow speeds; the
        // clock never enters the model.
        if speed > 0.0 && end.is_none() {
            let target = (out.t_us as f64 / 1e6) / speed;
            let mut elapsed = wall_start.elapsed().as_secs_f64();
            while target > elapsed && cmd_rx.len() == 0 {
                let chunk = (target - elapsed).min(0.01);
                std::thread::sleep(Duration::from_secs_f64(chunk));
                elapsed = wall_start.elapsed().as_secs_f64();
            }
        }
    }

    // ---- Drain: release remaining delayed frames, flush replay.
    for (_, bytes) in delay_queue.drain(..) {
        let _ = frame_tx.send(bytes);
    }
    let records = replay.finish().unwrap_or(0);
    let hash = hasher.finalize();
    let ticks = engine.tick_count();
    let _ = plane_tx.send(SimPlane {
        tick: ticks,
        t_us: ticks * tick_us_exact,
        snapshot: Default::default(),
        sent_hil_sensor,
        sent_hil_state_quaternion,
        sent_hil_gps,
        tick_p50_us: 0,
        tick_p95_us: 0,
        tick_p99_us: 0,
        tick_p999_us: 0,
        tick_max_us: 0,
        telemetry_hash: Some(hash),
    });

    if print_hash {
        // Determinism harness output (stdout; logs go to stderr).
        println!("telemetry_hash: {:016x}", hash);
    }
    tracing::info!(
        end = ?end,
        ticks,
        virtual_ms = ticks * tick_us_exact / 1000,
        replay_records = records,
        telemetry_hash = format!("{hash:016x}"),
        "sim loop finished"
    );
    let _ = done_tx.send(end.unwrap_or(SimEnd::DurationReached));
}

/// Release delay-queued frames whose virtual deadline has passed (F-09).
fn release_due(queue: &mut Vec<(u64, Vec<u8>)>, t_us: u64, tx: &mpsc::UnboundedSender<Vec<u8>>) {
    let mut i = 0;
    while i < queue.len() {
        if queue[i].0 <= t_us {
            let (_, bytes) = queue.remove(i);
            let _ = tx.send(bytes);
        } else {
            i += 1;
        }
    }
}
