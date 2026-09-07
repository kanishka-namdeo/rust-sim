//! CLI end-to-end test against a **fake PX4** (a plain TCP + MAVLink v2
//! client speaking the §3 handshake): exercises the same protocol surface
//! the real PX4 uses, without needing the PX4 binary. The real-PX4 case is
//! `tests/run_i1.sh` (I-1).
//!
//! Covers SPEC §10.3 Case I-1's sim-side requirements: HIL_SENSOR v2 stream
//! with id=0 and exact 5000 us cadence, COMMAND_LONG 511 -> COMMAND_ACK(0),
//! loop-closed on HIL_ACTUATOR_CONTROLS, control-plane REST + WS, exit code
//! 3 on disconnect.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use sitsim_mavlink::{Frame, FrameParser, Message, SYS_ID, COMP_ID};

/// Path to the built binary (overridable for CI).
fn cli_bin() -> PathBuf {
    std::env::var("SITSIM_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/debug/sitsim-cli")
        })
}

/// Port pairs picked away from the real PX4 conventions to avoid collisions
/// (one pair per test: cargo runs test binaries in parallel).
const HIL_PORT_A: u16 = 4590;
const API_PORT_A: u16 = 8290;
const HIL_PORT_B: u16 = 4591;
const API_PORT_B: u16 = 8291;

fn write_scenario(dir: &std::path::Path, name: &str, extra: &str, hil_port: u16, api_port: u16) -> PathBuf {
    let toml = format!(
        "[io]\ntcp_port = {hil_port}\napi_port = {api_port}\n\n[sim]\nrate_hz = 200\nduration_s = 8\nseed = 42\nspeed = 1.0\n\n[vehicle]\norigin = {{ lat_deg = 47.397770, lon_deg = 8.545580, alt_m = 500.0 }}\n\n[sensors.imu]\ngyro_noise_density = 0.0\ngyro_bias_walk = 0.0\ngyro_turnon_sigma = 0.0\ngyro_scale_sigma = 0.0\ngyro_misalign_deg = 0.0\naccel_noise_density = 0.0\naccel_bias_walk = 0.0\naccel_turnon_sigma = 0.0\naccel_scale_sigma = 0.0\naccel_misalign_deg = 0.0\n\n[sensors.mag]\nnoise_gauss = 0.0\nhard_iron_gauss = 0.0\n\n[sensors.baro]\nnoise_m = 0.0\nwalk_m_per_min = 0.0\n\n[env]\nturbulence = \"off\"\n{extra}"
    );
    let p = dir.join(name);
    std::fs::write(&p, toml).unwrap();
    p
}

struct Cli {
    child: Child,
    stdout_file: PathBuf,
    hil_port: u16,
    api_port: u16,
}

impl Cli {
    fn start(scenario: &PathBuf, dir: &PathBuf, args: &[&str], hil_port: u16, api_port: u16) -> Cli {
        let stdout_file = dir.join(format!("cli-stdout-{api_port}.txt"));
        let f = std::fs::File::create(&stdout_file).unwrap();
        let child = Command::new(cli_bin())
            .arg("scenario-run")
            .arg(scenario)
            .arg("--replay-out")
            .arg(dir.join(format!("run-{api_port}.replay")))
            .args(args)
            .stdout(Stdio::from(f))
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sitsim-cli");
        wait_for_api(api_port);
        Cli { child, stdout_file, hil_port, api_port }
    }

    fn hil_port(&self) -> u16 {
        self.hil_port
    }

    fn wait(&mut self) -> i32 {
        self.child.wait().unwrap().code().unwrap_or(-1)
    }

    fn stdout(&self) -> String {
        std::fs::read_to_string(&self.stdout_file).unwrap_or_default()
    }
}

impl Drop for Cli {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn wait_for_api(api_port: u16) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Ok(mut s) = TcpStream::connect(("127.0.0.1", api_port)) {
            let req = format!("GET /api/status HTTP/1.1\r\nHost: 127.0.0.1:{api_port}\r\nConnection: close\r\n\r\n");
            if s.write_all(req.as_bytes()).is_ok() {
                let mut buf = String::new();
                s.set_read_timeout(Some(Duration::from_millis(500))).ok();
                if s.read_to_string(&mut buf).is_ok() && buf.contains("WAIT") {
                    return;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("control plane did not come up on {api_port}");
}

/// Minimal HTTP/1.1 client (no deps): returns (status_line, body).
fn http(method: &str, path: &str, body: Option<&str>, api_port: u16) -> (String, String) {
    let mut s = TcpStream::connect(("127.0.0.1", api_port)).expect("connect api");
    s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let body = body.unwrap_or("");
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{api_port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).unwrap();
    let mut buf = String::new();
    s.read_to_string(&mut buf).unwrap_or_default();
    let mut parts = buf.splitn(2, "\r\n\r\n");
    let status = parts.next().unwrap_or("").lines().next().unwrap_or("").to_string();
    let rest = parts.next().unwrap_or("").to_string();
    (status, rest)
}

/// The full protocol dance with a fake PX4.
#[test]
fn fake_px4_boot_handshake_loop_and_exit_code() {
    let dir = std::env::temp_dir().join("sitsim-it-fakepx4");
    std::fs::create_dir_all(&dir).unwrap();
    let scenario = write_scenario(&dir, "boot.toml", "", HIL_PORT_A, API_PORT_A);
    let mut cli = Cli::start(&scenario, &dir, &["--telemetry-hash"], HIL_PORT_A, API_PORT_A);

    // ---- Fake PX4: connect; the sim must stream immediately (§3.3).
    let mut px4 = TcpStream::connect(("127.0.0.1", HIL_PORT_A)).expect("connect HIL");
    px4.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    px4.set_nodelay(true).unwrap();

    // ---- Rate negotiation (§3.4): COMMAND_LONG 511 for 115 @ 5000 us.
    let cl = Message::CommandLong(sitsim_mavlink::CommandLong {
        param1: 115.0,
        param2: 5000.0,
        param3: 0.0,
        param4: 0.0,
        param5: 0.0,
        param6: 0.0,
        param7: 0.0,
        command: 511,
        target_system: SYS_ID,
        target_component: COMP_ID,
        confirmation: 0,
    });
    px4.write_all(&Frame::from_message(&cl, 0, 1, 1).encode()).unwrap();

    // ---- Close the loop (§3.9): one HIL_ACTUATOR_CONTROLS.
    let hac = Message::HilActuatorControls(sitsim_mavlink::HilActuatorControls {
        time_usec: 5_000,
        controls: {
            let mut c = [0f32; 16];
            c[0] = -0.2;
            c[1] = -0.2;
            c[2] = -0.2;
            c[3] = -0.2;
            c
        },
        mode: 1,
        flags: 0,
    });
    px4.write_all(&Frame::from_message(&hac, 1, 1, 1).encode()).unwrap();

    // ---- Read the stream: expect COMMAND_ACK + HIL_SENSOR frames with the
    // §3.10 time contract and id == 0 (boot gate).
    let mut parser = FrameParser::new();
    let mut buf = [0u8; 8192];
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut saw_ack = false;
    let mut sensor_times: Vec<u64> = Vec::new();
    let mut saw_gps = false;
    while Instant::now() < deadline
        && (!saw_ack || sensor_times.len() < 40 || !saw_gps)
    {
        match px4.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                parser.feed(&buf[..n]);
                for f in parser.take() {
                    match f.decode().unwrap() {
                        Message::CommandAck(a) => {
                            assert_eq!(a.command, 511);
                            assert_eq!(a.result, 0);
                            saw_ack = true;
                        }
                        Message::HilSensor(s) => {
                            assert_eq!(s.id, 0, "boot gate: HIL_SENSOR.id must be 0");
                            // v2 framing is mandatory (§3.2): the parser only
                            // accepts 0xFD frames — reaching here proves it.
                            sensor_times.push(s.time_usec);
                        }
                        Message::HilGps(_) => saw_gps = true,
                        _ => {}
                    }
                }
            }
            Err(_) => break,
        }
    }
    assert!(saw_ack, "COMMAND_ACK(511, 0) not observed");
    assert!(saw_gps, "HIL_GPS not observed");
    assert!(sensor_times.len() >= 40, "HIL_SENSOR stream too thin: {}", sensor_times.len());
    for w in sensor_times.windows(2) {
        assert_eq!(w[1] - w[0], 5_000, "lockstep time contract violated");
    }
    // At rest: the prototype's verified accelerometer convention.
    assert!(
        sensor_times.len() > 1,
        "need at least two HIL_SENSOR frames"
    );

    // ---- Control plane: loop closed + counters (§4.1).
    let (status, body) = http("GET", "/api/status", None, API_PORT_A);
    assert!(status.contains("200"), "{status}");
    assert!(body.contains("\"loop_closed\":true"), "{body}");
    assert!(body.contains("\"px4_connected\":true"), "{body}");
    assert!(body.contains("\"phase\":\"RUN\""), "{body}");
    assert!(body.contains("\"hil_actuator_controls\":1"), "{body}");

    // ---- Fault injection over REST (§7.3): POST, then poll GET until the
    // 10 Hz snapshot carries the fault (the engine applies it at the next
    // tick boundary).
    let (status, body) = http(
        "POST",
        "/api/faults",
        Some(r#"{"type":"motor_efficiency","motor":1,"factor":0.5,"persistent":true}"#),
        API_PORT_A,
    );
    assert!(status.contains("200"), "{status} {body}");
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut seen = false;
    while Instant::now() < deadline {
        let (status, body) = http("GET", "/api/faults", None, API_PORT_A);
        assert!(status.contains("200"), "{status}");
        if body.contains("motor_efficiency") && body.contains("runtime-1") {
            seen = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(seen, "injected fault never became active");

    // ---- DELETE clears it.
    let (status, body) = http("DELETE", "/api/faults/runtime-1", None, API_PORT_A);
    assert!(status.contains("200"), "{status} {body}");

    // ---- WebSocket at BOTH /ws/telemetry and / (root, gateway-style).
    for path in ["/ws/telemetry", "/?XTransformPort=8290"] {
        let (head, mut sock, mut leftover) = ws_handshake(path, API_PORT_A);
        assert!(head.contains("101 Switching Protocols"), "WS upgrade at {path}: {head}");
        let frame = ws_read_text(&mut sock, &mut leftover);
        assert!(frame.contains("\"t_us\":"), "WS frame at {path}: {frame}");
        assert!(frame.contains("\"pos_ned_m\":"), "{frame}");
        assert!(frame.contains("\"battery_pct\":"), "{frame}");
        assert!(frame.contains("\"faults_active\":["), "{frame}");
    }

    // ---- Disconnect -> exit code 3 (§2.5/§4.3).
    drop(px4);
    let code = cli.wait();
    assert_eq!(code, 3, "PX4 disconnect must exit 3");
    let out = cli.stdout();
    assert!(out.contains("telemetry_hash:"), "hash line printed: {out}");
}

/// Raw WebSocket handshake (client side, no deps): returns the socket and
/// any bytes past the handshake header (the server pushes the first frame
/// immediately after upgrading).
fn ws_handshake(path: &str, api_port: u16) -> (String, TcpStream, Vec<u8>) {
    let mut s = TcpStream::connect(("127.0.0.1", api_port)).expect("connect api");
    s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{api_port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
    );
    s.write_all(req.as_bytes()).unwrap();
    let mut raw = Vec::new();
    let mut buf = [0u8; 2048];
    while !raw.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = match s.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        raw.extend_from_slice(&buf[..n]);
        if raw.len() > 65536 {
            break;
        }
    }
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4);
    let (head, rest) = match split {
        Some(p) => (String::from_utf8_lossy(&raw[..p]).to_string(), raw[p..].to_vec()),
        None => (String::from_utf8_lossy(&raw).to_string(), Vec::new()),
    };
    (head, s, rest)
}

/// Read one WebSocket text frame (server frames are unmasked, RFC 6455).
fn ws_read_text(s: &mut TcpStream, leftover: &mut Vec<u8>) -> String {
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut data: Vec<u8> = leftover.drain(..).collect();
    loop {
        if data.len() >= 2 {
            let len7 = (data[1] & 0x7F) as usize;
            let hdr_plen = if len7 < 126 {
                Some((2usize, len7))
            } else if len7 == 126 && data.len() >= 4 {
                Some((4usize, u16::from_be_bytes([data[2], data[3]]) as usize))
            } else {
                None
            };
            if let Some((hdr, plen)) = hdr_plen {
                if data.len() >= hdr + plen {
                    assert_eq!(data[0] & 0x0F, 1, "expected a text frame, got opcode {}", data[0] & 0x0F);
                    let text = String::from_utf8_lossy(&data[hdr..hdr + plen]).to_string();
                    *leftover = data[hdr + plen..].to_vec();
                    return text;
                }
            }
        }
        assert!(Instant::now() < deadline, "no WS text frame within deadline ({} bytes buffered)", data.len());
        let mut buf = [0u8; 4096];
        match s.read(&mut buf) {
            Ok(0) => panic!("WS closed before a frame"),
            Ok(n) => data.extend_from_slice(&buf[..n]),
            Err(_) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
}

/// Determinism harness at the CLI level (§8.1 / G-2): two runs with the
/// same seed print the same telemetry hash; a different seed differs.
#[test]
fn cli_telemetry_hash_deterministic() {
    let dir = std::env::temp_dir().join("sitsim-it-determinism");
    std::fs::create_dir_all(&dir).unwrap();

    let run = |seed: u64| -> (i32, String) {
        let scenario = write_scenario(
            &dir,
            &format!("det-{seed}.toml"),
            "[[sim_over]]\n", // placeholder, replaced below
            HIL_PORT_B,
            API_PORT_B,
        );
        // Rewrite with the seed and short duration (speed 8x to keep the
        // test quick: virtual 4 s in ~0.5 s wall).
        let toml = std::fs::read_to_string(&scenario).unwrap();
        let toml = toml
            .replace("duration_s = 8", "duration_s = 4")
            .replace("speed = 1.0", "speed = 8.0")
            .replace("seed = 42", &format!("seed = {seed}"))
            // Turbulence ON: the wind stream makes the trajectory seed-
            // sensitive (the zero-noise ideal-sensor base is not).
            .replace("turbulence = \"off\"", "turbulence = \"moderate\"")
            .replace("[[sim_over]]\n", "");
        std::fs::write(&scenario, toml).unwrap();
        let mut cli = Cli::start(&scenario, &dir, &["--telemetry-hash"], HIL_PORT_B, API_PORT_B);
        // A fake PX4 that just holds the connection open.
        let px4 = TcpStream::connect(("127.0.0.1", HIL_PORT_B)).expect("connect HIL");
        let px2 = std::sync::Arc::new(px4);
        let px3 = std::sync::Arc::clone(&px2);
        std::thread::spawn(move || {
            // Drain so the sim is never blocked (TCP writer is async
            // anyway, but keep buffers small).
            let mut s = (*px3).try_clone().unwrap();
            let mut buf = [0u8; 65536];
            loop {
                match s.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
        });
        let code = cli.wait();
        (code, cli.stdout())
    };

    let (code_a, out_a) = run(42);
    let (code_b, out_b) = run(42);
    let (code_c, out_c) = run(43);

    assert_eq!(code_a, 0, "clean duration end exits 0");
    assert_eq!(code_b, 0);
    assert_eq!(code_c, 0);
    let hash = |o: &str| o.trim().strip_prefix("telemetry_hash: ").unwrap().to_string();
    assert_eq!(hash(&out_a), hash(&out_b), "same seed -> identical telemetry hash");
    assert_ne!(hash(&out_a), hash(&out_c), "different seed -> different hash");
}
