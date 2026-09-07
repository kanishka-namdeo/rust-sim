//! Per-vehicle MAVLink UDP link task.
//!
//! Topology per spec §3.1: the link **binds** 127.0.0.1:14540+i (PX4's rcS
//! streams telemetry to whatever socket holds that port — the pattern proven
//! by `scripts/mavlink_check.py`) and **sends** commands/setpoints to PX4's
//! onboard listen port 127.0.0.1:14580+i from the same socket. Manager
//! addressing is sysid 255 / compid 190 on every link, one channel per
//! vehicle with independent sequence numbers.
//!
//! The task owns three cadences (spec §2.3): decode as-arrives, a 20 Hz
//! setpoint pump, a 1 Hz heartbeat, plus the command client (500 ms retry,
//! three attempts, spec §3.2). Commands are serialised: one COMMAND_LONG in
//! flight at a time; later requests queue behind it.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot};
use tokio::time::interval;

pub use crate::messages::TelemetryEvent;
use crate::messages::{cmds, enums, ids, CommandLong, Heartbeat, Ping, SetPositionTargetLocalNed};
use crate::frame::{crc_extra, Decoder, Frame};
use fleet_modes::{POSITION_ONLY, VELOCITY_YAWRATE};

#[derive(Debug, Clone)]
pub struct LinkConfig {
    /// Vehicle index i (0-based).
    pub instance: u8,
    /// Local bind "127.0.0.1:14540+i".
    pub bind_addr: String,
    /// PX4 onboard listen endpoint "127.0.0.1:14580+i".
    pub remote_addr: String,
    pub manager_sysid: u8,
    pub manager_compid: u8,
    pub heartbeat_ms: u64,
    pub setpoint_ms: u64,
    pub cmd_timeout_ms: u64,
    pub cmd_attempts: u32,
}

impl LinkConfig {
    /// Canonical per-instance config from spec §3.1 / §17.
    pub fn for_instance(instance: u8) -> Self {
        LinkConfig {
            instance,
            bind_addr: format!("127.0.0.1:{}", 14540 + instance as u16),
            remote_addr: format!("127.0.0.1:{}", 14580 + instance as u16),
            manager_sysid: 255,
            manager_compid: 190,
            heartbeat_ms: 1000,
            setpoint_ms: 50,
            cmd_timeout_ms: 500,
            cmd_attempts: 3,
        }
    }
}

/// Live link counters, written by the link task, read by the supervisor.
#[derive(Debug, Clone, Default)]
pub struct LinkStats {
    pub sent: u64,
    pub recv: u64,
    pub dropped_link_loss: u64,
    pub crc_or_unknown_dropped: u64,
    pub retries: u64,
    pub cmd_failures: u64,
    pub commands_sent: u64,
    pub setpoints_sent: u64,
    pub heartbeats_sent: u64,
    /// SET_MESSAGE_INTERVAL subscription requests sent (V-10 evidence).
    pub subscribe_requests_sent: u64,
    /// Message ids observed **before** the subscription requests went out
    /// (the rcS default stream set — V-10's empirical answer).
    pub pre_request_msg_ids: std::collections::BTreeMap<u32, u64>,
    /// Inbound message counts by message id.
    pub msg_counts: std::collections::BTreeMap<u32, u64>,
    /// Average inbound message rate since link start (Hz).
    pub recv_rate_hz: f32,
}

/// The 20 Hz setpoint slot: the mission runner writes the goal, the link's
/// pump steps toward it with a velocity-capped profile and transmits
/// (spec §3.3 / §7.1).
#[derive(Debug, Clone)]
pub struct SetpointSlot {
    pub active: bool,
    pub goal: Option<SetpointGoal>,
    pub current: [f32; 3],
    pub yaw: f32,
    /// Velocity cap for the profile step (m/s, default cruise 4.0).
    pub cruise_ms: f32,
    /// Selects POSITION_ONLY or VELOCITY_YAWRATE semantics.
    pub velocity_mode: bool,
    pub velocity: [f32; 3],
    pub yaw_rate: f32,
}

impl Default for SetpointSlot {
    fn default() -> Self {
        SetpointSlot {
            active: false,
            goal: None,
            current: [0.0, 0.0, 0.0],
            yaw: 0.0,
            cruise_ms: 4.0,
            velocity_mode: false,
            velocity: [0.0, 0.0, 0.0],
            yaw_rate: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SetpointGoal {
    pub position: [f32; 3],
    pub yaw: f32,
}

#[derive(Debug)]
pub enum LinkCommand {
    /// COMMAND_LONG with ACK correlation; queued, one in flight at a time.
    SendCommand {
        command: u16,
        params: [f32; 7],
        ack: oneshot::Sender<CmdAck>,
    },
    /// PARAM_SET write, confirmed by the PARAM_VALUE echo (v0.1 uses this
    /// for exactly one parameter, so a single in-flight slot suffices; a
    /// concurrent write replaces the pending one and its caller gets None).
    SetParam {
        param_id: String,
        value: f32,
        reply: oneshot::Sender<Option<f32>>,
    },
}

/// Result of a COMMAND_LONG exchange (spec §3.2 contract).
#[derive(Debug, Clone, PartialEq)]
pub enum CmdAck {
    Accepted,
    /// MAV_RESULT_TEMPORARILY_REJECTED / DENIED / UNSUPPORTED / FAILED.
    Rejected { result: u8 },
    /// No COMMAND_ACK after the full retry ladder — supervisor escalation
    /// territory (policy 8).
    Timeout { attempts: u32 },
}

struct Pending {
    command: u16,
    params: [f32; 7],
    ack: Option<oneshot::Sender<CmdAck>>,
    attempts: u32,
    deadline: Instant,
}

struct Queued {
    command: u16,
    params: [f32; 7],
    ack: oneshot::Sender<CmdAck>,
}

struct PendingParam {
    param_id: [u8; 16],
    value: f32,
    reply: oneshot::Sender<Option<f32>>,
    attempts: u32,
    deadline: Instant,
}

/// Shared state between the link task and the rest of the manager.
pub struct LinkShared {
    pub stats: Mutex<LinkStats>,
    pub slot: Mutex<SetpointSlot>,
    /// When set, inbound datagrams are dropped entirely (scenario
    /// `link_loss` event) while outbound continues.
    pub link_loss_until: Mutex<Option<Instant>>,
    /// Arrival timestamps (staleness inputs aggregated here so the
    /// supervisor does not need to watch the event channel).
    pub last_recv: Mutex<Option<Instant>>,
    pub last_heartbeat_in: Mutex<Option<Instant>>,
}

impl LinkShared {
    pub fn new() -> Arc<Self> {
        Arc::new(LinkShared {
            stats: Mutex::new(LinkStats::default()),
            slot: Mutex::new(SetpointSlot::default()),
            link_loss_until: Mutex::new(None),
            last_recv: Mutex::new(None),
            last_heartbeat_in: Mutex::new(None),
        })
    }

    pub fn stats(&self) -> LinkStats {
        self.stats.lock().unwrap().clone()
    }

    pub fn set_link_loss(&self, until: Option<Instant>) {
        *self.link_loss_until.lock().unwrap() = until;
    }

    /// Install a position-hold goal at the given NED point (offboard engage
    /// re-anchor, spec §3.3 / §7.2).
    pub fn hold_at(&self, pos: [f32; 3], yaw: f32) {
        let mut s = self.slot.lock().unwrap();
        s.active = true;
        s.velocity_mode = false;
        s.current = pos;
        s.yaw = yaw;
        s.goal = Some(SetpointGoal { position: pos, yaw });
    }

    pub fn set_goal(&self, goal: SetpointGoal) {
        let mut s = self.slot.lock().unwrap();
        s.velocity_mode = false;
        s.goal = Some(goal);
    }

    pub fn set_velocity(&self, v: [f32; 3], yaw_rate: f32) {
        let mut s = self.slot.lock().unwrap();
        s.velocity_mode = true;
        s.velocity = v;
        s.yaw_rate = yaw_rate;
    }

    pub fn stop_stream(&self) {
        self.slot.lock().unwrap().active = false;
    }

    pub fn slot_snapshot(&self) -> SetpointSlot {
        self.slot.lock().unwrap().clone()
    }
}

/// Handle used by the manager to drive one vehicle's link.
#[derive(Clone)]
pub struct LinkHandle {
    pub shared: Arc<LinkShared>,
    cmd_tx: mpsc::Sender<LinkCommand>,
}

impl LinkHandle {
    pub async fn send_command(&self, command: u16, params: [f32; 7]) -> CmdAck {
        let (tx, rx) = oneshot::channel();
        if self
            .cmd_tx
            .send(LinkCommand::SendCommand { command, params, ack: tx })
            .await
            .is_err()
        {
            return CmdAck::Timeout { attempts: 0 };
        }
        // 3 attempts x 500 ms plus slack for queueing behind other commands.
        match tokio::time::timeout(Duration::from_millis(2200), rx).await {
            Ok(Ok(ack)) => ack,
            _ => CmdAck::Timeout { attempts: 3 },
        }
    }

    /// Write a parameter (PARAM_SET, REAL32); returns the PARAM_VALUE echo
    /// on confirmation, None after three unconfirmed attempts (600 ms apart).
    pub async fn set_param(&self, param_id: &str, value: f32) -> Option<f32> {
        let (tx, rx) = oneshot::channel();
        if self
            .cmd_tx
            .send(LinkCommand::SetParam {
                param_id: param_id.to_string(),
                value,
                reply: tx,
            })
            .await
            .is_err()
        {
            return None;
        }
        match tokio::time::timeout(Duration::from_millis(2600), rx).await {
            Ok(Ok(v)) => v,
            _ => None,
        }
    }

    /// Convenience wrapper for DO_SET_MODE (spec §3.2).
    ///
    /// PX4's commander decodes VEHICLE_CMD_DO_SET_MODE as SEPARATE params —
    /// `custom_main_mode = (uint8_t)cmd.param2; custom_sub_mode =
    /// (uint8_t)cmd.param3` (Commander.cpp:788-790) — NOT the packed 32-bit
    /// custom-mode word. Sending the packed word truncates to 0 in the uint8
    /// cast and silently no-ops (still ACKed). The mode_word passed here
    /// (and compared against heartbeat echoes, which DO use the packed form)
    /// is decomposed into main/sub params.
    pub async fn set_mode(&self, mode_word: u32) -> CmdAck {
        let main_mode = ((mode_word >> 16) & 0xFF) as u8;
        let sub_mode = ((mode_word >> 24) & 0xFF) as u8;
        self.send_command(
            cmds::DO_SET_MODE,
            [
                enums::MAV_MODE_FLAG_CUSTOM_ENABLED as f32,
                main_mode as f32,
                sub_mode as f32,
                0.0,
                0.0,
                0.0,
                0.0,
            ],
        )
        .await
    }
}

/// Spawn the link task; returns the manager-side handle and the telemetry
/// event receiver.
pub fn spawn_link(cfg: LinkConfig) -> std::io::Result<(LinkHandle, mpsc::UnboundedReceiver<TelemetryEvent>)> {
    let shared = LinkShared::new();
    let (cmd_tx, cmd_rx) = mpsc::channel(64);
    let (ev_tx, ev_rx) = mpsc::unbounded_channel();
    let handle = LinkHandle {
        shared: Arc::clone(&shared),
        cmd_tx,
    };
    let cfg2 = cfg.clone();
    let instance = cfg.instance;
    tokio::spawn(async move {
        if let Err(e) = run(cfg2, shared, cmd_rx, ev_tx).await {
            eprintln!("[fleet-mavlink] link i={instance} terminated: {e}");
        }
    });
    Ok((handle, ev_rx))
}

/// The link task body.
pub async fn run(
    cfg: LinkConfig,
    shared: Arc<LinkShared>,
    mut cmd_rx: mpsc::Receiver<LinkCommand>,
    events: mpsc::UnboundedSender<TelemetryEvent>,
) -> std::io::Result<()> {
    let sock = UdpSocket::bind(&cfg.bind_addr).await?;
    let remote: SocketAddr = cfg.remote_addr.parse().expect("valid remote addr");
    let mut seq: u8 = 0;
    let mut decoder = Decoder::new();
    let mut buf = vec![0u8; 2048];

    let started = Instant::now();
    let mut hb = interval(Duration::from_millis(cfg.heartbeat_ms));
    let mut sp = interval(Duration::from_millis(cfg.setpoint_ms));
    let mut watchdog = interval(Duration::from_millis(1000));
    // Drives the pending-command retry ladder at sub-second cadence so the
    // 500 ms retry contract is not quantised to the 1 s watchdog.
    let mut cmd_timer = interval(Duration::from_millis(200));

    let mut pending: Option<Pending> = None;
    let mut queue: VecDeque<Queued> = VecDeque::new();
    let mut pending_param: Option<PendingParam> = None;
    // V-10: telemetry subscription (spec 3.1) sent once, right after the
    // first inbound frame proves PX4's mavlink instance is alive.
    let mut subscribed = false;

    let send_frame = |sock: &UdpSocket, frame: &Frame, seq: u8| -> std::io::Result<()> {
        let bytes = frame.encode_v2(seq, crc_extra(frame.msgid).unwrap_or(0));
        sock.try_send_to(&bytes, remote)?;
        Ok(())
    };

    loop {
        // Pending-command retry ladder (500 ms, three attempts, spec §3.2).
        if let Some(p) = pending.as_mut() {
            if Instant::now() >= p.deadline {
                if p.attempts < cfg.cmd_attempts {
                    p.attempts += 1;
                    p.deadline = Instant::now() + Duration::from_millis(cfg.cmd_timeout_ms);
                    let frame = command_frame(&cfg, p.command, &p.params);
                    send_frame(&sock, &frame, seq)?;
                    seq = seq.wrapping_add(1);
                    let mut st = shared.stats.lock().unwrap();
                    st.retries += 1;
                    st.commands_sent += 1;
                    st.sent += 1;
                } else {
                    let p = pending.take().unwrap();
                    let mut st = shared.stats.lock().unwrap();
                    st.cmd_failures += 1;
                    if let Some(ack) = p.ack {
                        let _ = ack.send(CmdAck::Timeout { attempts: p.attempts });
                    }
                }
            }
        }
        // Pending param write: 600 ms retry, three attempts; confirmed by
        // the PARAM_VALUE echo, not COMMAND_ACK.
        if let Some(pp) = pending_param.as_mut() {
            if Instant::now() >= pp.deadline {
                if pp.attempts < cfg.cmd_attempts {
                    pp.attempts += 1;
                    pp.deadline = Instant::now() + Duration::from_millis(600);
                    let frame = param_set_frame(&cfg, &pp.param_id, pp.value);
                    send_frame(&sock, &frame, seq)?;
                    seq = seq.wrapping_add(1);
                    shared.stats.lock().unwrap().sent += 1;
                } else {
                    let pp = pending_param.take().unwrap();
                    let _ = pp.reply.send(None);
                }
            }
        }
        // Start the next queued command if the pipe is free.
        if pending.is_none() {
            if let Some(q) = queue.pop_front() {
                let frame = command_frame(&cfg, q.command, &q.params);
                send_frame(&sock, &frame, seq)?;
                seq = seq.wrapping_add(1);
                {
                    let mut st = shared.stats.lock().unwrap();
                    st.commands_sent += 1;
                    st.sent += 1;
                }
                pending = Some(Pending {
                    command: q.command,
                    params: q.params,
                    ack: Some(q.ack),
                    attempts: 1,
                    deadline: Instant::now() + Duration::from_millis(cfg.cmd_timeout_ms),
                });
            }
        }

        tokio::select! {
            r = sock.recv_from(&mut buf) => {
                let (n, _src) = r?;
                // link_loss injection: drop the datagram entirely.
                let loss = *shared.link_loss_until.lock().unwrap();
                if let Some(until) = loss {
                    if Instant::now() < until {
                        shared.stats.lock().unwrap().dropped_link_loss += 1;
                        continue;
                    } else {
                        *shared.link_loss_until.lock().unwrap() = None;
                    }
                }
                decoder.push(&buf[..n]);
                while let Some(frame) = decoder.next_frame() {
                    if !frame.crc_ok {
                        shared.stats.lock().unwrap().crc_or_unknown_dropped += 1;
                    }
                    {
                        let mut st = shared.stats.lock().unwrap();
                        st.recv += 1;
                        *st.msg_counts.entry(frame.msgid).or_insert(0) += 1;
                        *shared.last_recv.lock().unwrap() = Some(Instant::now());
                        if frame.msgid == ids::HEARTBEAT {
                            *shared.last_heartbeat_in.lock().unwrap() = Some(Instant::now());
                        }
                    }
                    // V-10: on first contact from the vehicle, subscribe to
                    // the telemetry set (spec 3.1): ATTITUDE and
                    // LOCAL_POSITION_NED at 20 Hz, the rest at their default
                    // cadence. The pre-request stream set is snapshotted for
                    // the V-10 evidence event.
                    if !subscribed && frame.sysid == cfg.instance + 1 {
                        subscribed = true;
                        {
                            let mut st = shared.stats.lock().unwrap();
                            st.pre_request_msg_ids = st.msg_counts.clone();
                        }
                        for (msgid, interval_us) in SUBSCRIBE_SET {
                            queue.push_back(Queued {
                                command: cmds::SET_MESSAGE_INTERVAL,
                                params: [
                                    msgid as f32,
                                    interval_us,
                                    0.0,
                                    0.0,
                                    0.0,
                                    0.0,
                                    0.0,
                                ],
                                ack: {
                                    // fire-and-forget: dropped receiver.
                                    let (tx, _rx) = oneshot::channel();
                                    tx
                                },
                            });
                            shared.stats.lock().unwrap().subscribe_requests_sent += 1;
                        }
                    }
                    // Command-ACK correlation (before publishing so the
                    // exchange completes even if the aggregator is slow).
                    if frame.msgid == ids::COMMAND_ACK {
                        if let TelemetryEvent::CommandAck { command, result, target_system, target_component, .. } =
                            crate::messages::decode_event(&frame).unwrap()
                        {
                            let _ = target_component;
                            let for_us = target_system == 0 || target_system == cfg.manager_sysid;
                            if for_us {
                                if let Some(p) = pending.take() {
                                    if p.command == command {
                                        let ack = match result {
                                            enums::MAV_RESULT_ACCEPTED => CmdAck::Accepted,
                                            r => CmdAck::Rejected { result: r },
                                        };
                                        if let Some(tx) = p.ack {
                                            let _ = tx.send(ack);
                                        }
                                    } else {
                                        // ACK for a different command (stale
                                        // retry): keep waiting for ours.
                                        pending = Some(p);
                                    }
                                }
                            }
                        }
                    }
                    // Param write confirmation: resolve the pending write
                    // when the echo's id matches (unsolicited echoes restore).
                    if frame.msgid == ids::PARAM_VALUE {
                        if let TelemetryEvent::ParamValue { param_id, value, .. } =
                            crate::messages::decode_event(&frame).unwrap()
                        {
                            let ours = pending_param.as_ref().map_or(false, |pp| {
                                crate::messages::param_id_to_string(&pp.param_id) == param_id
                            });
                            if ours {
                                let pp = pending_param.take().unwrap();
                                let _ = pp.reply.send(Some(value));
                            }
                        }
                    }
                    // Respond to pings (link liveness evidence).
                    if frame.msgid == ids::PING {
                        if let TelemetryEvent::Ping { time_usec, seq: pseq } =
                            crate::messages::decode_event(&frame).unwrap()
                        {
                            let reply = Frame {
                                seq: 0,
                                sysid: cfg.manager_sysid,
                                compid: cfg.manager_compid,
                                msgid: ids::PING,
                                payload: Ping {
                                    time_usec,
                                    seq: pseq,
                                    target_system: frame.sysid,
                                    target_component: frame.compid,
                                }
                                .pack(),
                                crc_ok: true,
                            };
                            send_frame(&sock, &reply, seq)?;
                            seq = seq.wrapping_add(1);
                            shared.stats.lock().unwrap().sent += 1;
                        }
                    }
                    let ev = crate::messages::decode_event(&frame)
                        .unwrap_or(TelemetryEvent::Unknown { msgid: frame.msgid, len: frame.payload.len() });
                    let _ = events.send(ev);
                }
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    None => return Ok(()), // manager dropped the link
                    Some(LinkCommand::SendCommand { command, params, ack }) => {
                        queue.push_back(Queued { command, params, ack });
                    }
                    Some(LinkCommand::SetParam { param_id, value, reply }) => {
                        let mut id = [0u8; 16];
                        for (i, b) in param_id.as_bytes().iter().take(15).enumerate() {
                            id[i] = *b;
                        }
                        let frame = param_set_frame(&cfg, &id, value);
                        send_frame(&sock, &frame, seq)?;
                        seq = seq.wrapping_add(1);
                        shared.stats.lock().unwrap().sent += 1;
                        pending_param = Some(PendingParam {
                            param_id: id,
                            value,
                            reply,
                            attempts: 1,
                            deadline: Instant::now() + Duration::from_millis(600),
                        });
                    }
                }
            }
            _ = hb.tick() => {
                let frame = Frame {
                    seq: 0,
                    sysid: cfg.manager_sysid,
                    compid: cfg.manager_compid,
                    msgid: ids::HEARTBEAT,
                    payload: Heartbeat {
                        custom_mode: 0,
                        mavtype: enums::MAV_TYPE_GCS,
                        autopilot: enums::MAV_AUTOPILOT_INVALID,
                        base_mode: enums::MAV_MODE_FLAG_CUSTOM_ENABLED,
                        system_status: enums::MAV_STATE_ACTIVE,
                        mavlink_version: 3,
                    }
                    .pack(),
                    crc_ok: true,
                };
                send_frame(&sock, &frame, seq)?;
                seq = seq.wrapping_add(1);
                let mut st = shared.stats.lock().unwrap();
                st.sent += 1;
                st.heartbeats_sent += 1;
            }
            _ = sp.tick() => {
                let slot = {
                    let mut s = shared.slot.lock().unwrap();
                    if !s.active {
                        continue;
                    }
                    if !s.velocity_mode {
                        if let Some(goal) = s.goal {
                            // Velocity-capped profile step (spec §7.1):
                            // saturate displacement per 20 Hz step.
                            let max_step = s.cruise_ms * (cfg.setpoint_ms as f32 / 1000.0);
                            for i in 0..3 {
                                let d = goal.position[i] - s.current[i];
                                let step = d.clamp(-max_step, max_step);
                                s.current[i] += step;
                            }
                            s.yaw = goal.yaw;
                        }
                    }
                    s.clone()
                };
                let msg = if slot.velocity_mode {
                    SetPositionTargetLocalNed {
                        time_boot_ms: started.elapsed().as_millis() as u32,
                        target_system: cfg.instance + 1,
                        target_component: 1,
                        coordinate_frame: enums::MAV_FRAME_LOCAL_NED,
                        type_mask: VELOCITY_YAWRATE,
                        vx: slot.velocity[0],
                        vy: slot.velocity[1],
                        vz: slot.velocity[2],
                        yaw_rate: slot.yaw_rate,
                        ..Default::default()
                    }
                } else {
                    SetPositionTargetLocalNed {
                        time_boot_ms: started.elapsed().as_millis() as u32,
                        target_system: cfg.instance + 1,
                        target_component: 1,
                        coordinate_frame: enums::MAV_FRAME_LOCAL_NED,
                        type_mask: POSITION_ONLY,
                        x: slot.current[0],
                        y: slot.current[1],
                        z: slot.current[2],
                        yaw: slot.yaw,
                        ..Default::default()
                    }
                };
                let frame = Frame {
                    seq: 0,
                    sysid: cfg.manager_sysid,
                    compid: cfg.manager_compid,
                    msgid: ids::SET_POSITION_TARGET_LOCAL_NED,
                    payload: msg.pack(),
                    crc_ok: true,
                };
                send_frame(&sock, &frame, seq)?;
                seq = seq.wrapping_add(1);
                let mut st = shared.stats.lock().unwrap();
                st.sent += 1;
                st.setpoints_sent += 1;
            }
            _ = cmd_timer.tick() => {
                // wake the loop: retry/timeout logic at the top runs now.
            }
            _ = watchdog.tick() => {
                let mut st = shared.stats.lock().unwrap();
                let el = started.elapsed().as_secs_f32().max(0.001);
                st.recv_rate_hz = st.recv as f32 / el;
            }
        }
    }
}

fn command_frame(cfg: &LinkConfig, command: u16, params: &[f32; 7]) -> Frame {
    Frame {
        seq: 0,
        sysid: cfg.manager_sysid,
        compid: cfg.manager_compid,
        msgid: ids::COMMAND_LONG,
        payload: CommandLong {
            target_system: cfg.instance + 1,
            target_component: 1,
            command,
            confirmation: 0,
            params: *params,
        }
        .pack(),
        crc_ok: true,
    }
}

fn param_set_frame(cfg: &LinkConfig, param_id: &[u8; 16], value: f32) -> Frame {
    let mut ps = crate::messages::ParamSet::from_str("", value, cfg.instance + 1, 1);
    ps.param_id = *param_id;
    Frame {
        seq: 0,
        sysid: cfg.manager_sysid,
        compid: cfg.manager_compid,
        msgid: ids::PARAM_SET,
        payload: ps.pack(),
        crc_ok: true,
    }
}

/// The telemetry subscription set (spec 3.1): (message id, interval us).
/// 20 Hz for the estimator-grade streams, `0` = request the default rate
/// for the rest. STATUSTEXT is event-driven but subscription is conformant.
pub const SUBSCRIBE_SET: [(u32, f32); 7] = [
    (ids::ATTITUDE, 50_000.0),
    (ids::LOCAL_POSITION_NED, 50_000.0),
    (ids::SYS_STATUS, 0.0),
    (ids::GLOBAL_POSITION_INT, 0.0),
    (ids::BATTERY_STATUS, 0.0),
    (ids::HOME_POSITION, 0.0),
    (ids::STATUSTEXT, 0.0),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_config_ports_per_spec() {
        let c = LinkConfig::for_instance(0);
        assert_eq!(c.bind_addr, "127.0.0.1:14540");
        assert_eq!(c.remote_addr, "127.0.0.1:14580");
        assert_eq!(c.manager_sysid, 255);
        assert_eq!(c.manager_compid, 190);
        let c1 = LinkConfig::for_instance(1);
        assert_eq!(c1.bind_addr, "127.0.0.1:14541");
        assert_eq!(c1.remote_addr, "127.0.0.1:14581");
    }

    #[test]
    fn setpoint_slot_velocity_cap() {
        // 20 Hz, cruise 4 m/s -> max step 0.2 m
        let mut slot = SetpointSlot {
            active: true,
            cruise_ms: 4.0,
            ..Default::default()
        };
        slot.goal = Some(SetpointGoal { position: [100.0, 0.0, -10.0], yaw: 0.0 });
        let max_step = slot.cruise_ms * (50.0 / 1000.0);
        for i in 0..3 {
            let d = slot.goal.unwrap().position[i] - slot.current[i];
            let step = d.clamp(-max_step, max_step);
            slot.current[i] += step;
        }
        assert_eq!(slot.current, [0.2, 0.0, -0.2]);
    }

    #[tokio::test]
    async fn hold_at_anchors_slot() {
        let shared = LinkShared::new();
        shared.hold_at([1.0, 2.0, -3.0], 0.5);
        let s = shared.slot_snapshot();
        assert!(s.active);
        assert_eq!(s.current, [1.0, 2.0, -3.0]);
        assert_eq!(s.goal.unwrap().position, [1.0, 2.0, -3.0]);
        shared.stop_stream();
        assert!(!shared.slot_snapshot().active);
    }
}
