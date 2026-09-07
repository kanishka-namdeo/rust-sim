//! TCP HIL link (SPEC §3.1, §3.4, §3.10): the simulator is the TCP **server**
//! on 4560+i; PX4's `simulator_mavlink` module connects as a client.
//!
//! Task layout (SPEC §2.4 — the I/O plane owns all sockets):
//! - `serve_hil_link`: accepts exactly one client (later connections are
//!   refused with a log line while the first is live), then spawns the
//!   reader and writer halves.
//! - **Reader**: parses MAVLink v2 frames, updates the shared
//!   latest-actuator slot, forwards COMMAND_LONGs, answers them per §3.4
//!   (511 -> COMMAND_ACK result 0 + rate adaptation; other commands ->
//!   COMMAND_ACK result 3 UNSUPPORTED so PX4 does not retry), and marks
//!   `loop_closed` on the first HIL_ACTUATOR_CONTROLS.
//! - **Writer**: drains the frame channel (encoded frames from the sim
//!   thread and acks from the reader) and writes them to the socket.
//!
//! The sim thread never touches a socket: it receives actuator updates and
//! commands through channels and hands frames back through a channel.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;

use sitsim_mavlink::{CommandAck, CommandLong, Frame, FrameParser, Message, SYS_ID, COMP_ID, msg_id};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch};
use tracing::{info, warn};

/// Shared link statistics and state (all lock-free atomics).
#[derive(Debug, Default)]
pub struct LinkStats {
    /// A client is currently connected.
    pub connected: AtomicBool,
    /// A client has connected at least once.
    pub ever_connected: AtomicBool,
    /// The control loop closed: at least one HIL_ACTUATOR_CONTROLS arrived
    /// (SPEC §3.9: "the system's loop-closed signal").
    pub loop_closed: AtomicBool,
    pub sent_hil_sensor: AtomicU64,
    pub sent_hil_state_quaternion: AtomicU64,
    pub sent_hil_gps: AtomicU64,
    pub sent_command_ack: AtomicU64,
    pub sent_bytes: AtomicU64,
    pub recv_hil_actuator_controls: AtomicU64,
    pub recv_command_long: AtomicU64,
    pub recv_unknown_msgid: AtomicU64,
    pub recv_bad_crc: AtomicU64,
    pub recv_v1: AtomicU64,
    pub recv_dropped_garbage_bytes: AtomicU64,
    /// Negotiated HIL_STATE_QUATERNION interval, us (starts at the 200 Hz
    /// default; §3.4 bounds 2500..=20000 = 400..50 Hz).
    pub quat_interval_us: AtomicU32,
    /// Connections refused while the first client was live.
    pub refused_connections: AtomicU64,
    /// Peer port (0 = none).
    pub peer_port: AtomicU32,
}

impl LinkStats {
    /// Current phase-relevant booleans as a compact snapshot for status JSON.
    pub fn snapshot(&self) -> LinkSnapshot {
        LinkSnapshot {
            connected: self.connected.load(Ordering::Relaxed),
            loop_closed: self.loop_closed.load(Ordering::Relaxed),
            ever_connected: self.ever_connected.load(Ordering::Relaxed),
            peer_port: self.peer_port.load(Ordering::Relaxed),
            quat_interval_us: self.quat_interval_us.load(Ordering::Relaxed),
            sent_hil_sensor: self.sent_hil_sensor.load(Ordering::Relaxed),
            sent_hil_state_quaternion: self.sent_hil_state_quaternion.load(Ordering::Relaxed),
            sent_hil_gps: self.sent_hil_gps.load(Ordering::Relaxed),
            sent_command_ack: self.sent_command_ack.load(Ordering::Relaxed),
            sent_bytes: self.sent_bytes.load(Ordering::Relaxed),
            recv_hil_actuator_controls: self.recv_hil_actuator_controls.load(Ordering::Relaxed),
            recv_command_long: self.recv_command_long.load(Ordering::Relaxed),
            recv_unknown_msgid: self.recv_unknown_msgid.load(Ordering::Relaxed),
            recv_bad_crc: self.recv_bad_crc.load(Ordering::Relaxed),
            recv_v1: self.recv_v1.load(Ordering::Relaxed),
            recv_dropped_garbage_bytes: self.recv_dropped_garbage_bytes.load(Ordering::Relaxed),
            refused_connections: self.refused_connections.load(Ordering::Relaxed),
        }
    }
}

/// Plain-value snapshot of [`LinkStats`].
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LinkSnapshot {
    pub connected: bool,
    pub loop_closed: bool,
    pub ever_connected: bool,
    pub peer_port: u32,
    pub quat_interval_us: u32,
    pub sent_hil_sensor: u64,
    pub sent_hil_state_quaternion: u64,
    pub sent_hil_gps: u64,
    pub sent_command_ack: u64,
    pub sent_bytes: u64,
    pub recv_hil_actuator_controls: u64,
    pub recv_command_long: u64,
    pub recv_unknown_msgid: u64,
    pub recv_bad_crc: u64,
    pub recv_v1: u64,
    pub recv_dropped_garbage_bytes: u64,
    pub refused_connections: u64,
}

/// Events the reader forwards to the sim thread / control plane.
#[derive(Debug)]
pub enum LinkEvent {
    Connected(SocketAddr),
    Disconnected,
    /// Latest HIL_ACTUATOR_CONTROLS (only the newest is interesting).
    Actuator(sitsim_mavlink::HilActuatorControls),
}

/// Default HIL_STATE_QUATERNION interval (200 Hz).
pub const DEFAULT_QUAT_INTERVAL_US: u32 = 5_000;
/// Bounds honored for 511-adapted intervals (SPEC §3.4: within 50-400 Hz).
pub const QUAT_INTERVAL_MIN_US: u32 = 2_500;
pub const QUAT_INTERVAL_MAX_US: u32 = 20_000;

/// Bind the HIL TCP listener with retry/backoff (SPEC §2.5: instance
/// directories from a crashed PX4 run may hold sockets briefly).
pub async fn bind_hil_listener(addr: SocketAddr, retries: u8) -> std::io::Result<TcpListener> {
    let mut delay_ms = 50u64;
    let mut last_err = None;
    for attempt in 0..=retries {
        match TcpListener::bind(addr).await {
            Ok(l) => return Ok(l),
            Err(e) => {
                warn!(%addr, attempt, error = %e, "HIL TCP bind failed; retrying with backoff");
                last_err = Some(e);
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                delay_ms *= 2;
            }
        }
    }
    Err(last_err.expect("at least one attempt"))
}

/// Serve the HIL link: accept exactly one client, then run reader/writer
/// until EOF, writer error, or the `shutdown` signal fires.
///
/// `frame_rx` is the sim thread's outgoing frame channel (the reader also
/// pushes its COMMAND_ACKs into it via a cloned sender). `event_tx`
/// delivers connection lifecycle + actuator updates to the sim thread.
#[allow(clippy::too_many_arguments)]
pub async fn serve_hil_link(
    listener: TcpListener,
    stats: Arc<LinkStats>,
    event_tx: mpsc::UnboundedSender<LinkEvent>,
    frame_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    mut shutdown: watch::Receiver<bool>,
) -> std::io::Result<()> {
    // Wait for the first client (cancellable by shutdown).
    let (stream, peer) = tokio::select! {
        r = listener.accept() => r?,
        _ = shutdown.changed() => return Ok(()),
    };
    // Lockstep latency: disable Nagle so HIL frames go out immediately.
    let _ = stream.set_nodelay(true);
    stats.connected.store(true, Ordering::Relaxed);
    stats.ever_connected.store(true, Ordering::Relaxed);
    stats.peer_port.store(peer.port() as u32, Ordering::Relaxed);
    stats.quat_interval_us.store(DEFAULT_QUAT_INTERVAL_US, Ordering::Relaxed);
    let _ = event_tx.send(LinkEvent::Connected(peer));
    info!(%peer, "PX4 connected to the HIL link");

    // Refuse further connections while the first is live (SPEC §3.1).
    let refuser = tokio::spawn(refuse_extra_clients(listener, Arc::clone(&stats), shutdown.clone()));

    let (frame_tx_for_reader, frame_rx_shared) = mpsc::unbounded_channel::<Vec<u8>>();
    // Merge the sim-thread frames with reader acks: forward sim frames.
    let frame_tx_shared = frame_tx_for_reader.clone();
    let mut merger = tokio::spawn(async move {
        let mut frame_rx = frame_rx;
        while let Some(f) = frame_rx.recv().await {
            if frame_tx_shared.send(f).is_err() {
                break;
            }
        }
    });

    let (rd, wr) = stream.into_split();
    let reader = tokio::spawn(reader_half(
        rd,
        Arc::clone(&stats),
        event_tx.clone(),
        frame_tx_for_reader,
    ));
    let mut writer = tokio::spawn(writer_half(wr, frame_rx_shared));

    // Finish when the reader ends (EOF / error), the shutdown fires, the
    // writer ends, or the sim thread closes its frame channel (graceful
    // drain: the sim is done sending and wants the link wound down).
    let result: std::io::Result<()> = tokio::select! {
        r = reader => {
            let _ = r;
            Ok(())
        }
        r = &mut writer => {
            let _ = r;
            Ok(())
        }
        _ = shutdown.changed() => Ok(()),
        _ = &mut merger => {
            // Sim side closed its frame channel. Give the writer a bounded
            // window to flush frames already queued (including acks the
            // reader pushed), then wind the link down.
            let _ = tokio::time::timeout(std::time::Duration::from_millis(500), &mut writer).await;
            Ok(())
        }
    };

    stats.connected.store(false, Ordering::Relaxed);
    let _ = event_tx.send(LinkEvent::Disconnected);
    merger.abort();
    refuser.abort();
    writer.abort();
    result
}

async fn refuse_extra_clients(listener: TcpListener, stats: Arc<LinkStats>, mut shutdown: watch::Receiver<bool>) {
    loop {
        tokio::select! {
            r = listener.accept() => {
                if let Ok((_s, peer)) = r {
                    stats.refused_connections.fetch_add(1, Ordering::Relaxed);
                    warn!(%peer, "refusing extra HIL connection while a client is live");
                    drop(_s);
                }
            }
            _ = shutdown.changed() => return,
        }
    }
}

async fn reader_half(
    mut rd: tokio::net::tcp::OwnedReadHalf,
    stats: Arc<LinkStats>,
    event_tx: mpsc::UnboundedSender<LinkEvent>,
    frame_tx: mpsc::UnboundedSender<Vec<u8>>,
) {
    let mut parser = FrameParser::new();
    let mut buf = [0u8; 4096];
    let seq = Arc::new(AtomicU8::new(0));
    loop {
        let n = match rd.read(&mut buf).await {
            Ok(0) => {
                info!("HIL link: PX4 closed the connection (EOF)");
                break;
            }
            Ok(n) => n,
            Err(e) => {
                warn!(error = %e, "HIL link read error");
                break;
            }
        };
        parser.feed(&buf[..n]);
        // Export parser counters.
        stats.recv_unknown_msgid.store(parser.unknown_msgid_frames, Ordering::Relaxed);
        stats.recv_bad_crc.store(parser.bad_crc_frames, Ordering::Relaxed);
        stats.recv_v1.store(parser.v1_frames, Ordering::Relaxed);
        stats.recv_dropped_garbage_bytes.store(parser.dropped_bytes, Ordering::Relaxed);

        for frame in parser.take() {
            match frame.decode() {
                Ok(Message::HilActuatorControls(a)) => {
                    if !stats.loop_closed.load(Ordering::Relaxed) {
                        stats.loop_closed.store(true, Ordering::Relaxed);
                        info!("HIL_ACTUATOR_CONTROLS received -> control loop closed");
                    }
                    stats.recv_hil_actuator_controls.fetch_add(1, Ordering::Relaxed);
                    let _ = event_tx.send(LinkEvent::Actuator(a));
                }
                Ok(Message::CommandLong(c)) => {
                    stats.recv_command_long.fetch_add(1, Ordering::Relaxed);
                    handle_command_long(&c, &stats, &frame_tx, &seq);
                }
                Ok(_) => {
                    stats.recv_unknown_msgid.fetch_add(1, Ordering::Relaxed);
                }
                Err(e) => {
                    warn!(%e, "undecodable frame on the HIL link");
                }
            }
        }
    }
}

/// Rate negotiation per SPEC §3.4: COMMAND_LONG 511 with param1 = message id
/// and param2 = interval_us is honored for HIL_STATE_QUATERNION (115)
/// within 50-400 Hz bounds and acknowledged ACCEPTED; every other command
/// is acknowledged UNSUPPORTED (3) so PX4 does not retry.
fn handle_command_long(c: &CommandLong, stats: &LinkStats, frame_tx: &mpsc::UnboundedSender<Vec<u8>>, seq: &Arc<AtomicU8>) {
    const MAV_CMD_SET_MESSAGE_INTERVAL: u16 = 511;
    let result: u8 = if c.command == MAV_CMD_SET_MESSAGE_INTERVAL {
        let msgid = c.param1 as u32;
        let interval_us = c.param2.max(0.0) as u32;
        if msgid == msg_id::HIL_STATE_QUATERNION {
            let bounded = interval_us.clamp(QUAT_INTERVAL_MIN_US, QUAT_INTERVAL_MAX_US);
            let prev = stats.quat_interval_us.swap(bounded, Ordering::Relaxed);
            if prev != bounded {
                info!(msgid, interval_us = bounded, "adapting HIL_STATE_QUATERNION cadence (COMMAND_LONG 511)");
            }
        } else {
            info!(msgid, "COMMAND_LONG 511 for a non-115 message; acknowledged, no cadence change");
        }
        0 // ACCEPTED
    } else {
        3 // UNSUPPORTED
    };
    let ack = Message::CommandAck(CommandAck::new(c.command, result));
    let s = seq.fetch_add(1, Ordering::Relaxed);
    let bytes = Frame::from_message(&ack, s, SYS_ID, COMP_ID).encode();
    stats.sent_command_ack.fetch_add(1, Ordering::Relaxed);
    stats.sent_bytes.fetch_add(bytes.len() as u64, Ordering::Relaxed);
    let _ = frame_tx.send(bytes);
}

async fn writer_half(
    mut wr: tokio::net::tcp::OwnedWriteHalf,
    mut frame_rx: mpsc::UnboundedReceiver<Vec<u8>>,
) -> std::io::Result<()> {
    while let Some(mut bytes) = frame_rx.recv().await {
        // Coalesce anything else already queued to reduce syscalls.
        while let Ok(mut more) = frame_rx.try_recv() {
            bytes.append(&mut more);
        }
        wr.write_all(&bytes).await?;
        wr.flush().await?;
    }
    let _ = wr.shutdown().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sitsim_mavlink::{HilActuatorControls, SYS_ID, COMP_ID};

    /// End-to-end loopback: a fake PX4 connects, sends a COMMAND_LONG 511
    /// (115 @ 5000 us) and a HIL_ACTUATOR_CONTROLS; expects a COMMAND_ACK
    /// and forwarded events; the sim side sends one HIL_SENSOR frame and
    /// the fake client must receive the bytes.
    #[tokio::test]
    async fn link_loopback_handshake_and_data() {
        let stats = Arc::new(LinkStats::default());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
        let (frame_tx, frame_rx) = mpsc::unbounded_channel();
        let (_shut_tx, shut_rx) = watch::channel(false);

        let server = tokio::spawn(serve_hil_link(listener, Arc::clone(&stats), event_tx, frame_rx, shut_rx));

        // Fake PX4.
        let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();

        // Send COMMAND_LONG 511 (115, 5000 us) — same shape PX4 sends.
        let cl = Message::CommandLong(CommandLong {
            param1: 115.0,
            param2: 5000.0,
            param3: 0.0,
            param4: 0.0,
            param5: 0.0,
            param6: 0.0,
            param7: 0.0,
            command: 511,
            target_system: 2,
            target_component: 1,
            confirmation: 0,
        });
        client.write_all(&Frame::from_message(&cl, 0, 1, 1).encode()).await.unwrap();

        // Send a HIL_ACTUATOR_CONTROLS frame (PX4 layout).
        let mut controls = [0f32; 16];
        controls[0] = 0.25;
        let hac = Message::HilActuatorControls(HilActuatorControls {
            time_usec: 1,
            controls,
            mode: 1,
            flags: 0,
        });
        client.write_all(&Frame::from_message(&hac, 1, 1, 1).encode()).await.unwrap();

        // Sim side sends a HIL_SENSOR.
        let hs = Message::HilSensor(sitsim_mavlink::HilSensor {
            time_usec: 5000,
            xacc: 0.0,
            yacc: 0.0,
            zacc: -9.80665,
            xgyro: 0.0,
            ygyro: 0.0,
            zgyro: 0.0,
            xmag: 0.19,
            ymag: 0.01,
            zmag: 0.46,
            abs_pressure: 101325.0,
            diff_pressure: 0.0,
            pressure_alt: 0.0,
            temperature: 20.0,
            fields_updated: 0x1BFF,
            id: 0,
        });
        frame_tx.send(Frame::from_message(&hs, 0, SYS_ID, COMP_ID).encode()).unwrap();

        // Read the client's side: expect COMMAND_ACK + HIL_SENSOR bytes.
        let mut got = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while got.len() < 40 && std::time::Instant::now() < deadline {
            let mut tmp = [0u8; 256];
            let n = tokio::time::timeout(std::time::Duration::from_millis(200), client.read(&mut tmp))
                .await
                .map(|r| r.unwrap_or(0))
                .unwrap_or(0);
            got.extend_from_slice(&tmp[..n]);
        }
        let mut parser = FrameParser::new();
        parser.feed(&got);
        let frames = parser.take();
        let mut saw_ack = false;
        let mut saw_sensor = false;
        for f in frames {
            match f.decode().unwrap() {
                Message::CommandAck(a) => {
                    assert_eq!(a.command, 511);
                    assert_eq!(a.result, 0);
                    saw_ack = true;
                }
                Message::HilSensor(s) => {
                    assert_eq!(s.id, 0);
                    assert_eq!(s.time_usec, 5000);
                    saw_sensor = true;
                }
                _ => {}
            }
        }
        assert!(saw_ack, "no COMMAND_ACK observed: {} bytes", got.len());
        assert!(saw_sensor, "no HIL_SENSOR observed: {} bytes", got.len());

        // Events: Connected + Actuator (the reader task may still be
        // processing; poll with a deadline).
        let mut connected = false;
        let mut actuator = false;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline && !(connected && actuator) {
            match event_rx.try_recv() {
                Ok(LinkEvent::Connected(_)) => connected = true,
                Ok(LinkEvent::Actuator(a)) => {
                    actuator = true;
                    assert!((a.controls[0] - 0.25).abs() < 1e-6);
                }
                Ok(LinkEvent::Disconnected) => {}
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(10)).await,
            }
        }
        assert!(connected);
        assert!(actuator);
        assert!(stats.loop_closed.load(Ordering::Relaxed));
        assert_eq!(stats.sent_command_ack.load(Ordering::Relaxed), 1);
        assert_eq!(stats.recv_command_long.load(Ordering::Relaxed), 1);
        assert_eq!(stats.quat_interval_us.load(Ordering::Relaxed), 5000);

        // Graceful shutdown: sim closes its frame channel -> writer drains.
        drop(frame_tx);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), server).await;
        assert!(!stats.connected.load(Ordering::Relaxed));
    }

    /// Non-511 commands get result 3 (UNSUPPORTED).
    #[tokio::test]
    async fn unsupported_command_acked_with_3() {
        let stats = Arc::new(LinkStats::default());
        let (frame_tx, mut frame_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let seq = Arc::new(AtomicU8::new(0));
        let cl = CommandLong {
            param1: 0.0,
            param2: 0.0,
            param3: 0.0,
            param4: 0.0,
            param5: 0.0,
            param6: 0.0,
            param7: 0.0,
            command: 400, // MAV_CMD_COMPONENT_ARM_DISCONNECT
            target_system: 2,
            target_component: 1,
            confirmation: 0,
        };
        handle_command_long(&cl, &stats, &frame_tx, &seq);
        let bytes = frame_rx.recv().await.unwrap();
        let mut p = FrameParser::new();
        p.feed(&bytes);
        match p.take().pop().unwrap().decode().unwrap() {
            Message::CommandAck(a) => {
                assert_eq!(a.command, 400);
                assert_eq!(a.result, 3);
            }
            _ => panic!("expected ack"),
        }
    }
}
