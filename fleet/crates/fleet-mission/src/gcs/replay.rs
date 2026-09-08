//! Analyze View backend (GCS_SPEC.md §5.5 — Log / ULog Analysis).
//!
//! Two artifact types are browsable from the Analyze View:
//!
//! - `.replay` files — RustSim's own deterministic binary record
//!   (`sim/docs/SPEC.md` §8.2). 64-byte header + 96-byte fixed tick records,
//!   each carrying 17 state floats (pos, vel, q, omega, rotors) + 16 motor
//!   inputs + a fault-active bitmap + CRC-16/X.25. The catalog parses these
//!   server-side and exposes per-topic data ranges for the strip charts.
//!
//! - `.ulg` files — PX4's standard ULog binary format. Parsing is done by
//!   `pyulog` per ADR-0021 (Q-3); the catalog delegates to a Python
//!   subprocess. When `pyulog` is unavailable, the endpoints degrade
//!   gracefully: `/api/ulogs` still lists files (filesystem-only), but
//!   `/api/ulogs/{file}/topics` returns 503 so the frontend can show a
//!   "ULog parser unavailable" banner. This module owns the filesystem
//!   listing + the replay parser; the ULog subprocess shim lives in the
//!   `gcs::server` handlers.
//!
//! The replay parser is vendored here (vs. depending on `sitsim-sdk`) so
//! `fleet-mission` does not pull in the full sim workspace — only the
//! `:8300` catalog needs the binary read path, and that path is small
//! enough to keep self-contained.

#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Constants (mirror sitsim-sdk/src/replay.rs)
// ---------------------------------------------------------------------------

/// Replay file magic — `b"RSITSIM1"`.
pub const MAGIC: &[u8; 8] = b"RSITSIM1";
/// Replay format version (1 — only version 1 exists in v0.1).
pub const VERSION: u16 = 1;
/// Header length, bytes.
pub const HEADER_LEN: usize = 64;
/// Per-record length, bytes.
pub const RECORD_LEN: usize = 96;

// ---------------------------------------------------------------------------
// CRC-16/X.25 (mirror sitsim-mavlink/src/lib.rs::x25_crc)
// ---------------------------------------------------------------------------

/// CRC-16/X.25 (a.k.a. CRC-16-IBM-SDLC, CRC-16-CCITT-reflected).
///
/// Polynomial 0x8408 (reflected 0x1021), init 0xFFFF, no final XOR. Reference
/// vector: `x25_crc(b"123456789") == 0x6F91`.
fn x25_crc(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in data {
        crc ^= b as u16;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0x8408;
            } else {
                crc >>= 1;
            }
        }
    }
    crc
}

// ---------------------------------------------------------------------------
// Parsed header + record
// ---------------------------------------------------------------------------

/// Parsed replay header.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplayHeader {
    pub version: u16,
    /// Tick rate, Hz.
    pub rate_hz: f64,
    pub seed: u64,
    pub start_t_us: u64,
    pub scenario_hash: [u8; 32],
}

/// One decoded tick record. State floats are arranged per
/// `sim/docs/SPEC.md` §8.2: `[pos(3), vel(3), q(4), omega(3), rotors(4)]`.
#[derive(Debug, Clone, PartialEq)]
pub struct TickRecord {
    pub t_us: u64,
    /// Raw commanded motor inputs, u in [0, 1] (dequantized from u8/255).
    pub motors: [f64; 16],
    /// pos(3), vel(3), q(4), omega(3), rotors(4) — 17 floats total.
    pub state: [f32; 17],
    /// Fault-active bitmap (bit i = spec i of the first 16).
    pub fault_flags: u16,
}

/// Decode + validate the 64-byte header.
pub fn decode_header(buf: &[u8]) -> Result<ReplayHeader, String> {
    if buf.len() != HEADER_LEN {
        return Err(format!("header length {} != {HEADER_LEN}", buf.len()));
    }
    if &buf[0..8] != MAGIC {
        return Err("bad magic (not a rustsitsim replay)".into());
    }
    let version = u16::from_le_bytes(buf[8..10].try_into().unwrap());
    if version != VERSION {
        return Err(format!("unsupported replay version {version}"));
    }
    let millihz = u32::from_le_bytes(buf[12..16].try_into().unwrap());
    let seed = u64::from_le_bytes(buf[16..24].try_into().unwrap());
    let start_t_us = u64::from_le_bytes(buf[24..32].try_into().unwrap());
    let mut scenario_hash = [0u8; 32];
    scenario_hash.copy_from_slice(&buf[32..64]);
    Ok(ReplayHeader {
        version,
        rate_hz: millihz as f64 / 1000.0,
        seed,
        start_t_us,
        scenario_hash,
    })
}

/// Decode + CRC-verify a 96-byte record.
pub fn decode_record(buf: &[u8]) -> Result<TickRecord, String> {
    if buf.len() != RECORD_LEN {
        return Err(format!("record length {} != {RECORD_LEN}", buf.len()));
    }
    let crc = u16::from_le_bytes([buf[94], buf[95]]);
    let expect = x25_crc(&buf[..94]);
    if crc != expect {
        return Err(format!(
            "CRC mismatch: stored {crc:#06x}, computed {expect:#06x}"
        ));
    }
    let t_us = u64::from_le_bytes(buf[0..8].try_into().unwrap());
    let mut motors = [0.0f64; 16];
    for (i, m) in motors.iter_mut().enumerate() {
        *m = buf[8 + i] as f64 / 255.0;
    }
    let mut state = [0f32; 17];
    for (i, v) in state.iter_mut().enumerate() {
        *v = f32::from_le_bytes(buf[24 + 4 * i..28 + 4 * i].try_into().unwrap());
    }
    let fault_flags = u16::from_le_bytes([buf[92], buf[93]]);
    Ok(TickRecord {
        t_us,
        motors,
        state,
        fault_flags,
    })
}

// ---------------------------------------------------------------------------
// Topic enumeration (state float layout per sim/docs/SPEC.md §8.2)
// ---------------------------------------------------------------------------

/// A named slice of the state vector. The Analyze View's topic picker
/// enumerates these so the operator can plot `pos_ned_m.z` or `q_wxyz.w`
/// without needing to know the raw index layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TopicSpec {
    pub name: &'static str,
    pub label: &'static str,
    pub unit: &'static str,
    /// Inclusive start index into `TickRecord::state`.
    pub start: usize,
    /// Exclusive end index into `TickRecord::state`.
    pub end: usize,
    /// Component names (length = end - start).
    pub components: &'static [&'static str],
}

/// Static topic catalogue — matches the layout documented in
/// `sim/docs/SPEC.md` §8.2: `pos(3), vel(3), q(4), omega(3), rotors(4)`.
pub static TOPICS: &[TopicSpec] = &[
    TopicSpec {
        name: "pos_ned_m",
        label: "Position (NED)",
        unit: "m",
        start: 0,
        end: 3,
        components: &["x", "y", "z"],
    },
    TopicSpec {
        name: "vel_ned_ms",
        label: "Velocity (NED)",
        unit: "m/s",
        start: 3,
        end: 6,
        components: &["x", "y", "z"],
    },
    TopicSpec {
        name: "q_wxyz",
        label: "Attitude quaternion (wxyz)",
        unit: "",
        start: 6,
        end: 10,
        components: &["w", "x", "y", "z"],
    },
    TopicSpec {
        name: "omega_rads",
        label: "Body rates",
        unit: "rad/s",
        start: 10,
        end: 13,
        components: &["x", "y", "z"],
    },
    TopicSpec {
        name: "rotors",
        label: "Rotor state",
        unit: "",
        start: 13,
        end: 17,
        components: &["r0", "r1", "r2", "r3"],
    },
];

/// Look up a topic by name. Returns `None` for unknown names — the HTTP
/// layer translates that into a 404 `TOPIC_NOT_FOUND`.
pub fn topic_by_name(name: &str) -> Option<&'static TopicSpec> {
    TOPICS.iter().find(|t| t.name == name)
}

// ---------------------------------------------------------------------------
// High-level read API (used by the HTTP handlers)
// ---------------------------------------------------------------------------

/// A summary of one `.replay` file in the catalog — the shape returned by
/// `GET /api/replays` (GCS_SPEC.md §5.5).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ReplaySummary {
    pub filename: String,
    /// File size, bytes.
    pub size_bytes: u64,
    /// mtime, seconds since UNIX epoch.
    pub mtime: u64,
}

/// Parsed replay metadata — the shape returned by
/// `GET /api/replays/{file}/meta` (GCS_SPEC.md §5.5). `records` is
/// derived from `(file_size - 64) / 96`; `virtual_duration_s` is
/// `records / rate_hz`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ReplayMeta {
    pub filename: String,
    pub version: u16,
    pub tick_rate_hz: f64,
    pub seed: u64,
    pub start_t_us: u64,
    pub scenario_sha256: String,
    pub records: u64,
    pub virtual_duration_s: f64,
    /// Size of the file on disk, bytes.
    pub size_bytes: u64,
}

/// One tick's contribution to a topic data fetch — `t_s` is the
/// virtual time in seconds; `tick` is the 0-based index.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TopicDataPoint {
    pub tick: u64,
    pub t_s: f64,
    /// Component values for this tick — length matches the topic's
    /// component count (e.g. 3 for `pos_ned_m`, 4 for `q_wxyz`).
    pub values: Vec<f32>,
}

/// Topic data response — the shape returned by
/// `GET /api/replays/{file}/data?from_tick&to_tick&topic=…`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ReplayTopicData {
    pub topic: String,
    pub label: String,
    pub unit: String,
    pub components: Vec<String>,
    pub from_tick: u64,
    pub to_tick: u64,
    pub tick_rate_hz: f64,
    pub points: Vec<TopicDataPoint>,
}

/// Errors returned by the high-level API. Mapped to HTTP status + error
/// codes by the `gcs::server` handlers.
#[derive(Debug)]
pub enum ReplayError {
    NotFound(String),
    BadHeader(String),
    BadRecord(String),
    TopicNotFound(String),
    RangeOutOfBounds { from: u64, to: u64, records: u64 },
    Io(std::io::Error),
}

impl std::fmt::Display for ReplayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReplayError::NotFound(n) => write!(f, "replay '{n}' not found"),
            ReplayError::BadHeader(m) => write!(f, "bad replay header: {m}"),
            ReplayError::BadRecord(m) => write!(f, "bad replay record: {m}"),
            ReplayError::TopicNotFound(t) => write!(f, "topic '{t}' not found"),
            ReplayError::RangeOutOfBounds { from, to, records } => write!(
                f,
                "tick range [{from}, {to}] out of bounds (file has {records} records)"
            ),
            ReplayError::Io(e) => write!(f, "io: {e}"),
        }
    }
}
impl std::error::Error for ReplayError {}
impl From<std::io::Error> for ReplayError {
    fn from(e: std::io::Error) -> Self {
        ReplayError::Io(e)
    }
}

/// Sanitize a user-supplied filename into a safe path component. Rejects
/// path separators, leading dots, and anything that escapes the replays
/// dir. Returns the sanitized filename on success.
fn sanitize_filename(name: &str) -> Result<String, ReplayError> {
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || name.starts_with('.')
    {
        return Err(ReplayError::NotFound(name.to_string()));
    }
    Ok(name.to_string())
}

/// List all `.replay` files in `dir`, sorted by filename (stable, ASCII).
/// Skips sub-directories (even if they happen to have a `.replay`
/// extension) — only regular files and symlinks-to-files are listed.
pub fn list_replay_files(dir: &Path) -> Result<Vec<ReplaySummary>, ReplayError> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("replay") {
            continue;
        }
        // Skip sub-directories named like `subdir.replay` (a directory
        // matches the extension check, but is not a readable replay
        // file). Symlinks-to-files are accepted (the catalog supports
        // symlinked .replay files per ADR-0027).
        let ftype = entry.file_type()?;
        if !ftype.is_file() && !ftype.is_symlink() {
            continue;
        }
        let filename = match path.file_name().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        let md = entry.metadata()?;
        let mtime = md
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        out.push(ReplaySummary {
            filename,
            size_bytes: md.len(),
            mtime,
        });
    }
    out.sort_by(|a, b| a.filename.cmp(&b.filename));
    Ok(out)
}

/// List all `.ulg` files in `dir`, sorted by filename. Filesystem-only —
/// does not invoke `pyulog`. Used by `GET /api/ulogs` (GCS_SPEC.md §5.5).
/// Skips sub-directories named like `subdir.ulg` (same path-injection
/// guard as `list_replay_files`).
pub fn list_ulog_files(dir: &Path) -> Result<Vec<ReplaySummary>, ReplayError> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("ulg") {
            continue;
        }
        let ftype = entry.file_type()?;
        if !ftype.is_file() && !ftype.is_symlink() {
            continue;
        }
        let filename = match path.file_name().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        let md = entry.metadata()?;
        let mtime = md
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        out.push(ReplaySummary {
            filename,
            size_bytes: md.len(),
            mtime,
        });
    }
    out.sort_by(|a, b| a.filename.cmp(&b.filename));
    Ok(out)
}

/// Path to a specific replay file under `dir`, after sanitizing the name.
pub fn replay_path(dir: &Path, name: &str) -> Result<PathBuf, ReplayError> {
    let safe = sanitize_filename(name)?;
    let p = dir.join(&safe);
    if !p.exists() {
        return Err(ReplayError::NotFound(name.to_string()));
    }
    Ok(p)
}

/// Parse a replay file's header + record count without decoding records.
pub fn read_replay_meta(dir: &Path, name: &str) -> Result<ReplayMeta, ReplayError> {
    let path = replay_path(dir, name)?;
    let bytes = fs::read(&path)?;
    if bytes.len() < HEADER_LEN {
        return Err(ReplayError::BadHeader(format!(
            "file too small: {} bytes (need >= {HEADER_LEN})",
            bytes.len()
        )));
    }
    let header = decode_header(&bytes[..HEADER_LEN])
        .map_err(ReplayError::BadHeader)?;
    let records = ((bytes.len() - HEADER_LEN) as u64) / (RECORD_LEN as u64);
    let virtual_duration_s = if header.rate_hz > 0.0 {
        records as f64 / header.rate_hz
    } else {
        0.0
    };
    Ok(ReplayMeta {
        filename: name.to_string(),
        version: header.version,
        tick_rate_hz: header.rate_hz,
        seed: header.seed,
        start_t_us: header.start_t_us,
        scenario_sha256: hex_lower(&header.scenario_hash),
        records,
        virtual_duration_s,
        size_bytes: bytes.len() as u64,
    })
}

/// Read a topic slice from a replay file across `[from_tick, to_tick]`
/// (inclusive). Returns one data point per tick in range; out-of-range
/// ticks are an error (the frontend clamps the scrubber, not the backend).
pub fn read_replay_topic(
    dir: &Path,
    name: &str,
    topic: &str,
    from_tick: u64,
    to_tick: u64,
) -> Result<ReplayTopicData, ReplayError> {
    let spec = topic_by_name(topic).ok_or_else(|| ReplayError::TopicNotFound(topic.to_string()))?;
    if from_tick > to_tick {
        return Err(ReplayError::RangeOutOfBounds {
            from: from_tick,
            to: to_tick,
            records: 0,
        });
    }
    let path = replay_path(dir, name)?;
    let bytes = fs::read(&path)?;
    if bytes.len() < HEADER_LEN {
        return Err(ReplayError::BadHeader(format!(
            "file too small: {} bytes",
            bytes.len()
        )));
    }
    let header = decode_header(&bytes[..HEADER_LEN])
        .map_err(ReplayError::BadHeader)?;
    let records = ((bytes.len() - HEADER_LEN) as u64) / (RECORD_LEN as u64);
    if to_tick >= records {
        return Err(ReplayError::RangeOutOfBounds {
            from: from_tick,
            to: to_tick,
            records,
        });
    }
    let mut points = Vec::with_capacity((to_tick - from_tick + 1) as usize);
    for tick in from_tick..=to_tick {
        let off = HEADER_LEN + (tick as usize) * RECORD_LEN;
        let rec = decode_record(&bytes[off..off + RECORD_LEN])
            .map_err(ReplayError::BadRecord)?;
        let t_s = if header.rate_hz > 0.0 {
            rec.t_us as f64 / 1_000_000.0
        } else {
            tick as f64 / header.rate_hz
        };
        let values: Vec<f32> = rec.state[spec.start..spec.end].to_vec();
        points.push(TopicDataPoint { tick, t_s, values });
    }
    Ok(ReplayTopicData {
        topic: spec.name.to_string(),
        label: spec.label.to_string(),
        unit: spec.unit.to_string(),
        components: spec.components.iter().map(|s| s.to_string()).collect(),
        from_tick,
        to_tick,
        tick_rate_hz: header.rate_hz,
        points,
    })
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn write_fake_replay(path: &Path, rate_hz: f64, seed: u64, n_records: usize) {
        // Header
        let mut buf = Vec::with_capacity(HEADER_LEN + n_records * RECORD_LEN);
        buf.extend_from_slice(MAGIC);
        buf.extend_from_slice(&VERSION.to_le_bytes());
        buf.extend_from_slice(&(HEADER_LEN as u16).to_le_bytes());
        let millihz = (rate_hz * 1000.0).round() as u32;
        buf.extend_from_slice(&millihz.to_le_bytes());
        buf.extend_from_slice(&seed.to_le_bytes());
        buf.extend_from_slice(&0u64.to_le_bytes()); // start_t_us
        let mut hash = [0u8; 32];
        for (i, b) in hash.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(7);
        }
        buf.extend_from_slice(&hash);
        assert_eq!(buf.len(), HEADER_LEN);

        // Records
        for tick in 0..n_records as u64 {
            let mut rec = [0u8; RECORD_LEN];
            let t_us = (tick * 5_000) as u64; // 200 Hz = 5ms/tick
            rec[0..8].copy_from_slice(&t_us.to_le_bytes());
            for i in 0..16 {
                rec[8 + i] = (tick as u8).wrapping_add(i as u8);
            }
            let mut state = [0f32; 17];
            for i in 0..3 {
                state[i] = (tick as f32) * 0.1 + i as f32; // pos_ned_m
            }
            for i in 3..6 {
                state[i] = (tick as f32) * 0.05 + (i - 3) as f32; // vel_ned_ms
            }
            state[6] = 1.0; // q_wxyz.w
            for i in 7..10 {
                state[i] = 0.0;
            }
            for i in 10..17 {
                state[i] = (tick as f32) * 0.01 + (i - 10) as f32;
            }
            for (i, &v) in state.iter().enumerate() {
                rec[24 + 4 * i..28 + 4 * i].copy_from_slice(&v.to_le_bytes());
            }
            rec[92..94].copy_from_slice(&0u16.to_le_bytes());
            let crc = x25_crc(&rec[..94]);
            rec[94..96].copy_from_slice(&crc.to_le_bytes());
            buf.extend_from_slice(&rec);
        }
        fs::write(path, &buf).unwrap();
    }

    #[test]
    fn x25_crc_reference_vectors() {
        assert_eq!(x25_crc(b""), 0xFFFF);
        assert_eq!(x25_crc(b"123456789"), 0x6F91);
        assert_eq!(x25_crc(b"\x00"), 0x0F87);
    }

    #[test]
    fn list_replay_files_skips_non_replay() {
        let dir = std::env::temp_dir().join(format!(
            "rustsim_replay_list_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        write_fake_replay(&dir.join("a.replay"), 200.0, 1, 5);
        write_fake_replay(&dir.join("b.replay"), 200.0, 2, 3);
        fs::write(dir.join("c.txt"), b"not a replay").unwrap();
        fs::write(dir.join("d.json"), b"{}").unwrap();
        let list = list_replay_files(&dir).unwrap();
        let names: Vec<_> = list.iter().map(|s| s.filename.clone()).collect();
        assert_eq!(names, vec!["a.replay", "b.replay"]);
        // size sanity
        assert_eq!(list[0].size_bytes, (HEADER_LEN + 5 * RECORD_LEN) as u64);
        assert_eq!(list[1].size_bytes, (HEADER_LEN + 3 * RECORD_LEN) as u64);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_meta_returns_correct_counts() {
        let dir = std::env::temp_dir().join(format!(
            "rustsim_replay_meta_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        write_fake_replay(&dir.join("test.replay"), 200.0, 42, 100);
        let meta = read_replay_meta(&dir, "test.replay").unwrap();
        assert_eq!(meta.records, 100);
        assert!((meta.tick_rate_hz - 200.0).abs() < 0.001);
        assert!((meta.virtual_duration_s - 0.5).abs() < 0.001, "got {}", meta.virtual_duration_s);
        assert_eq!(meta.seed, 42);
        assert_eq!(meta.scenario_sha256.len(), 64);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_meta_rejects_bad_magic() {
        let dir = std::env::temp_dir().join(format!(
            "rustsim_replay_bad_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let mut bad = vec![0u8; HEADER_LEN];
        bad[0..8].copy_from_slice(b"BADMAGIC");
        fs::write(dir.join("bad.replay"), &bad).unwrap();
        let err = read_replay_meta(&dir, "bad.replay").unwrap_err();
        match err {
            ReplayError::BadHeader(m) => assert!(m.contains("magic")),
            other => panic!("expected BadHeader, got {other:?}"),
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_topic_returns_expected_count() {
        let dir = std::env::temp_dir().join(format!(
            "rustsim_replay_topic_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        write_fake_replay(&dir.join("test.replay"), 200.0, 42, 100);
        let data = read_replay_topic(&dir, "test.replay", "pos_ned_m", 0, 50).unwrap();
        assert_eq!(data.topic, "pos_ned_m");
        assert_eq!(data.points.len(), 51);
        assert_eq!(data.points[0].tick, 0);
        assert_eq!(data.points[50].tick, 50);
        assert_eq!(data.points[0].values.len(), 3);
        // pos_ned_m x at tick 0 = 0.0, at tick 1 = 0.1, at tick 50 = 5.0
        assert!((data.points[0].values[0] - 0.0).abs() < 0.001);
        assert!((data.points[1].values[0] - 0.1).abs() < 0.001);
        assert!((data.points[50].values[0] - 5.0).abs() < 0.001);
        // times should be tick * (1/200)
        assert!((data.points[0].t_s - 0.0).abs() < 0.001);
        assert!((data.points[1].t_s - 0.005).abs() < 0.001);
        assert!((data.points[50].t_s - 0.25).abs() < 0.001);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_topic_unknown_returns_topic_not_found() {
        let dir = std::env::temp_dir().join(format!(
            "rustsim_replay_unknown_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        write_fake_replay(&dir.join("test.replay"), 200.0, 42, 10);
        let err = read_replay_topic(&dir, "test.replay", "not_a_topic", 0, 5).unwrap_err();
        match err {
            ReplayError::TopicNotFound(t) => assert_eq!(t, "not_a_topic"),
            other => panic!("expected TopicNotFound, got {other:?}"),
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_topic_out_of_bounds_returns_range_error() {
        let dir = std::env::temp_dir().join(format!(
            "rustsim_replay_oob_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        write_fake_replay(&dir.join("test.replay"), 200.0, 42, 10);
        let err = read_replay_topic(&dir, "test.replay", "pos_ned_m", 0, 10).unwrap_err();
        match err {
            ReplayError::RangeOutOfBounds { to, records, .. } => {
                assert_eq!(to, 10);
                assert_eq!(records, 10);
            }
            other => panic!("expected RangeOutOfBounds, got {other:?}"),
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sanitize_filename_rejects_path_traversal() {
        assert!(sanitize_filename("../etc/passwd").is_err());
        assert!(sanitize_filename(".hidden").is_err());
        assert!(sanitize_filename("a/b").is_err());
        assert!(sanitize_filename("a\\b").is_err());
        assert!(sanitize_filename("").is_err());
        assert!(sanitize_filename("test.replay").is_ok());
    }

    #[test]
    fn topic_catalog_covers_state_vector() {
        // 5 topics: pos, vel, q, omega, rotors — totals 3+3+4+3+4 = 17 floats.
        let total: usize = TOPICS.iter().map(|t| t.end - t.start).sum();
        assert_eq!(total, 17);
        assert!(topic_by_name("pos_ned_m").is_some());
        assert!(topic_by_name("vel_ned_ms").is_some());
        assert!(topic_by_name("q_wxyz").is_some());
        assert!(topic_by_name("omega_rads").is_some());
        assert!(topic_by_name("rotors").is_some());
        assert!(topic_by_name("battery_v").is_none());
    }
}
