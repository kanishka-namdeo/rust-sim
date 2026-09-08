//! ADR-0021 / GCS_SPEC.md §5.5: ULog (`.ulg`) serving via server-side
//! `pyulog` (already installed on `/usr/bin/python3.13` per
//! `SANDBOX_SETUP.md` §5).
//!
//! PX4's `.ulg` files are the standard ULog binary format. Per
//! ADR-0021, v1 parses them server-side by shelling out to
//! `/usr/bin/python3.13` with the `pyulog` package; no browser-side
//! ULog parser is written in v1, and no Rust ULog parser is pulled
//! into the fleet crate (layering: sim-side crates stay on the sim
//! side; the catalog server shells out instead).
//!
//! The directory holding `.ulg` files is configurable via the
//! `RSIM_ULOG_DIR` env var (defaults to the catalog's own `ulogs/`
//! subdir, which `Store::new` creates). The catalog server never
//! writes ULogs — it only reads them (PX4 SITL writes them via its
//! logger; the operator or harness copies / symlinks them into the
//! catalog's `ulogs/` dir for the Analyze View to find).
//!
//! Endpoints (all on `:8300`):
//!
//! - `GET /api/ulogs`                                — list `.ulg` files
//! - `GET /api/ulogs/{file}/topics`                  — list topic names
//! - `GET /api/ulogs/{file}/topics/{topic}/data`     — fetch topic data

#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Path to the python3.13 interpreter that has `pyulog` installed
/// (per SANDBOX_SETUP.md §5). Hard-coded to match the sandbox default;
/// the env var `RSIM_PYTHON` overrides this for non-sandbox installs.
fn python_bin() -> PathBuf {
    std::env::var("RSIM_PYTHON")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/usr/bin/python3.13"))
}

/// Cap on data points returned per
/// `/api/ulogs/{file}/topics/{topic}/data` request. AC-5.5.2 calls
/// for plotting a ≤1 MB `.ulg` topic in <1 s; 10k points × ~50 B
/// JSON ≈ 500 KB per response, well under the AC.
pub const MAX_POINTS_PER_REQUEST: usize = 10_000;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum ULogError {
    Io(std::io::Error),
    NotFound(String),
    Pyulog(String),
    Json(serde_json::Error),
}

impl std::fmt::Display for ULogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ULogError::Io(e) => write!(f, "io: {e}"),
            ULogError::NotFound(name) => write!(f, "ulog file '{name}' not found"),
            ULogError::Pyulog(msg) => write!(f, "pyulog: {msg}"),
            ULogError::Json(e) => write!(f, "json: {e}"),
        }
    }
}
impl std::error::Error for ULogError {}

impl From<std::io::Error> for ULogError {
    fn from(e: std::io::Error) -> Self { ULogError::Io(e) }
}
impl From<serde_json::Error> for ULogError {
    fn from(e: serde_json::Error) -> Self { ULogError::Json(e) }
}

// ---------------------------------------------------------------------------
// Summary (response shape for /api/ulogs)
// ---------------------------------------------------------------------------

/// One entry in the `/api/ulogs` listing. Filesystem metadata only —
/// does NOT open the file (so a directory with 100 `.ulg` files lists
/// instantly). The browser-side Analyze View fetches the topic list
/// separately via `/api/ulogs/{file}/topics` once the operator clicks
/// a file.
///
/// Mirrors `gcs::replay::ReplaySummary` field-for-field so the
/// frontend's "Recent flights" panel can render `.replay` and `.ulg`
/// rows with the same component (only the file extension differs).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ULogSummary {
    pub filename: String,
    pub size_bytes: u64,
    /// mtime, seconds since UNIX epoch (stable, JSON-friendly, matches
    /// `gcs::replay::ReplaySummary::mtime`).
    pub mtime: u64,
}

// ---------------------------------------------------------------------------
// Filesystem listing
// ---------------------------------------------------------------------------

/// Scan `dir` for `.ulg` files. Sorted by filename (stable, ASCII) —
/// same ordering `gcs::replay::list_replay_files` uses. Non-`.ulg`
/// files and sub-directories are skipped.
pub fn list_ulogs(dir: &Path) -> Result<Vec<ULogSummary>, ULogError> {
    let mut out = Vec::new();
    if !dir.exists() {
        return Ok(out);
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("ulg") {
            continue;
        }
        if !entry.file_type()?.is_file() && !entry.file_type()?.is_symlink() {
            continue;
        }
        let filename = match path.file_name().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        let meta = match fs::metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        out.push(ULogSummary {
            filename,
            size_bytes: meta.len(),
            mtime,
        });
    }
    out.sort_by(|a, b| a.filename.cmp(&b.filename));
    Ok(out)
}

/// Path-injection guard for the `{file}` path parameter. Resolves
/// `name` against `dir`, refusing path separators, `..`, empty, and
/// over-long (192-char) names. Returns the canonical path and the
/// sanitized filename.
pub fn resolve_ulog_path(dir: &Path, name: &str) -> Result<(PathBuf, String), ULogError> {
    if name.is_empty()
        || name.len() > 192
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0')
        || name == "."
        || name == ".."
        || name.contains("..")
    {
        return Err(ULogError::NotFound(name.to_string()));
    }
    let with_ext = if name.ends_with(".ulg") {
        name.to_string()
    } else {
        format!("{name}.ulg")
    };
    let path = dir.join(&with_ext);
    if !path.exists() {
        return Err(ULogError::NotFound(name.to_string()));
    }
    // Canonicalize to refuse symlinks that escape `dir`.
    let canon = path
        .canonicalize()
        .map_err(|_| ULogError::NotFound(name.to_string()))?;
    let dir_canon = dir.canonicalize().map_err(ULogError::Io)?;
    if !canon.starts_with(&dir_canon) {
        return Err(ULogError::NotFound(name.to_string()));
    }
    Ok((canon, with_ext))
}

/// Validate a topic name. Topic names like `vehicle_local_position`
/// or `sensor_combined.gyro_rad` use `[A-Za-z0-9_.]+` (no path
/// separators, no shell metachars — the topic is passed to pyulog as
/// a Python identifier, not a path).
pub fn validate_topic_name(name: &str) -> Result<(), ULogError> {
    if name.is_empty() || name.len() > 192 {
        return Err(ULogError::NotFound(name.to_string()));
    }
    let valid = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.');
    if !valid {
        return Err(ULogError::NotFound(name.to_string()));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// pyulog shell-out
// ---------------------------------------------------------------------------

/// `GET /api/ulogs/{file}/topics` — list topic names in the ULog.
///
/// Shells out to:
///   `python3.13 -c "import sys, json, pyulog;
///    ulog = pyulog.ULog(sys.argv[1]);
///    print(json.dumps([d.name for d in ulog.data_list]))" <path>`
///
/// The ULog path is passed via `argv[1]` (not interpolated into the
/// `-c` script) so paths with quotes / shell metachars survive
/// round-trip cleanly and the subprocess stays shell-injection-safe.
/// Returns the topic list as a `Vec<String>`.
pub fn list_topics(path: &Path) -> Result<Vec<String>, ULogError> {
    let script = "import sys, json, pyulog\n\
                  ulog = pyulog.ULog(sys.argv[1])\n\
                  print(json.dumps([d.name for d in ulog.data_list]))";
    let out = Command::new(python_bin())
        .arg("-c")
        .arg(script)
        .arg(path)
        .output()?;
    if !out.status.success() {
        return Err(ULogError::Pyulog(
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())?;
    let arr = parsed
        .as_array()
        .ok_or_else(|| ULogError::Pyulog(format!("pyulog returned non-array: {stdout}")))?;
    Ok(arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
}

/// Response shape for `/api/ulogs/{file}/topics/{topic}/data`.
/// `t` is the per-sample time in seconds since the start of the log
/// (px4 timestamps are µs since boot; we subtract `start_timestamp`
/// and divide by 1e6). `fields` maps each non-timestamp field name to
/// its per-sample value list. `total_points` is the total samples in
/// the topic (before stride-sampling); `stride` is the downsample
/// factor (1 = no downsampling).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ULogTopicData {
    pub topic: String,
    pub from_s: Option<f64>,
    pub to_s: Option<f64>,
    pub total_points: usize,
    pub stride: usize,
    pub t: Vec<f64>,
    /// `BTreeMap` for stable JSON key ordering (so two requests for
    /// the same topic produce byte-identical responses — useful for
    /// cache keys and tests).
    pub fields: std::collections::BTreeMap<String, Vec<serde_json::Value>>,
}

/// `GET /api/ulogs/{file}/topics/{topic}/data` — fetch a time range
/// of one topic.
///
/// Shells out to a python script that:
///   1. Reads a JSON request `{path, topic, from_s, to_s, max_points}`
///      from stdin (so paths with quotes / special chars survive
///      round-trip cleanly — passing them as `argv` would force shell
///      escaping and open a shell-injection surface).
///   2. Opens the ULog with `message_name_filter_list=[topic]` (only
///      loads the requested topic — much faster than full parse).
///   3. Converts absolute timestamps to relative seconds.
///   4. Filters by `[from_s, to_s]` if either is set.
///   5. Stride-samples if the resulting range exceeds
///      `MAX_POINTS_PER_REQUEST`.
///   6. Returns `{topic, t: [...], fields: {name: [values...]}}` as JSON.
///
/// All values are returned as JSON numbers (or `null` for NaN/Inf,
/// which JSON cannot represent). The `t` array is the shared x-axis.
pub fn read_topic_data(
    path: &Path,
    topic: &str,
    from_s: Option<f64>,
    to_s: Option<f64>,
) -> Result<ULogTopicData, ULogError> {
    validate_topic_name(topic)?;

    // Pass through env to allow operators to bump the cap (useful for
    // headless analysis scripts that download the full topic).
    let max_points = std::env::var("RSIM_ULOG_MAX_POINTS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(MAX_POINTS_PER_REQUEST);

    // Request payload sent to python via stdin (no shell escaping).
    let request = serde_json::json!({
        "path": path.display().to_string(),
        "topic": topic,
        "from_s": from_s,
        "to_s": to_s,
        "max_points": max_points,
    });

    // The python script. Reads JSON from stdin, writes JSON to stdout.
    let script = r#"
import json, sys
import numpy as np
import pyulog

req = json.loads(sys.stdin.read())
path = req["path"]
topic = req["topic"]
from_s = req["from_s"]
to_s = req["to_s"]
max_points = int(req["max_points"])

ulog = pyulog.ULog(path, message_name_filter_list=[topic])
if not ulog.data_list:
    print(json.dumps({"error": "topic not found", "topic": topic}))
    sys.exit(0)
d = ulog.data_list[0]
if "timestamp" not in d.data:
    print(json.dumps({"error": "topic has no timestamp field", "topic": topic}))
    sys.exit(0)
ts = d.data["timestamp"]
if len(ts) == 0:
    print(json.dumps({"topic": topic, "t": [], "fields": {}, "total_points": 0, "stride": 1}))
    sys.exit(0)
start = ulog.start_timestamp if ulog.start_timestamp else ts[0]
rel_t = (ts - start) / 1.0e6

mask = np.ones(len(rel_t), dtype=bool)
if from_s is not None:
    mask &= (rel_t >= from_s)
if to_s is not None:
    mask &= (rel_t <= to_s)

n = int(mask.sum())
if n > max_points:
    stride = (n + max_points - 1) // max_points
else:
    stride = 1

t_sel = rel_t[mask][::stride]
out_fields = {}
for k, v in d.data.items():
    if k == "timestamp":
        continue
    arr = np.asarray(v)[mask][::stride]
    cleaned = []
    for x in arr:
        if isinstance(x, (np.floating, float)):
            f = float(x)
            if f != f or f in (float("inf"), float("-inf")):
                cleaned.append(None)
            else:
                cleaned.append(f)
        elif isinstance(x, (np.integer, int)):
            cleaned.append(int(x))
        elif isinstance(x, (np.bool_, bool)):
            cleaned.append(bool(x))
        else:
            cleaned.append(str(x))
    out_fields[k] = cleaned
print(json.dumps({
    "topic": topic,
    "t": t_sel.tolist(),
    "fields": out_fields,
    "total_points": int(len(rel_t)),
    "selected_points": int(len(t_sel)),
    "stride": int(stride),
}))
"#;

    let mut child = Command::new(python_bin())
        .arg("-c")
        .arg(script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write as _;
        let _ = stdin.write_all(request.to_string().as_bytes());
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        return Err(ULogError::Pyulog(
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())?;

    if let Some(err) = parsed.get("error").and_then(|v| v.as_str()) {
        return Err(ULogError::Pyulog(err.to_string()));
    }

    let t: Vec<f64> = parsed
        .get("t")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_f64()).collect())
        .unwrap_or_default();

    let mut fields: std::collections::BTreeMap<String, Vec<serde_json::Value>> =
        std::collections::BTreeMap::new();
    if let Some(obj) = parsed.get("fields").and_then(|v| v.as_object()) {
        for (k, v) in obj {
            if let Some(arr) = v.as_array() {
                fields.insert(k.clone(), arr.iter().cloned().collect());
            }
        }
    }

    let total_points = parsed
        .get("total_points")
        .and_then(|v| v.as_u64())
        .unwrap_or(t.len() as u64) as usize;
    let stride = parsed
        .get("stride")
        .and_then(|v| v.as_u64())
        .unwrap_or(1) as usize;

    Ok(ULogTopicData {
        topic: topic.to_string(),
        from_s,
        to_s,
        total_points,
        stride,
        t,
        fields,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicU64, Ordering};
    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn test_dir() -> PathBuf {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("rustsim_ulog_test_{}_{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Touch an empty `.ulg` file. The list endpoint only reads
    /// filesystem metadata, so empty files are sufficient.
    fn touch_ulog(path: &Path) {
        fs::write(path, b"").unwrap();
    }

    /// Find a real `.ulg` file in the repo (the f1/f2 test artifacts
    /// produce them). Returns `None` if none are present (CI without
    /// those artifacts will skip the pyulog-dependent test).
    fn find_real_ulog() -> Option<PathBuf> {
        let candidates = [
            "/home/z/my-project/rust-sim/fleet/tests/f1_artifacts/run/vehicle_0/log/2026-09-08/15_08_14.ulg",
            "/home/z/my-project/rust-sim/fleet/tests/f1_artifacts/run/vehicle_1/log/2026-09-08/15_08_17.ulg",
            "/home/z/my-project/rust-sim/fleet/tests/f2_artifacts/run/vehicle_0/log/2026-09-08/15_08_38.ulg",
            "/home/z/my-project/rust-sim/fleet/tests/f2_artifacts/run/vehicle_1/log/2026-09-08/15_08_41.ulg",
        ];
        for c in candidates {
            if Path::new(c).exists() {
                return Some(PathBuf::from(c));
            }
        }
        // Last-resort: scan the fleet/tests dir for any .ulg.
        let walk_dir = Path::new("/home/z/my-project/rust-sim/fleet/tests");
        if walk_dir.exists() {
            for entry in walk_dir.read_dir().ok()?.flatten() {
                if entry.path().extension().and_then(|s| s.to_str()) == Some("ulg") {
                    return Some(entry.path());
                }
            }
        }
        None
    }

    #[test]
    fn ulog_list_returns_files() {
        let dir = test_dir();
        touch_ulog(&dir.join("alpha.ulg"));
        touch_ulog(&dir.join("beta.ulg"));
        // Non-.ulg files must be skipped.
        fs::write(dir.join("not_a_ulog.txt"), "hello").unwrap();
        // A sub-directory named like "old_ulogs" must be skipped.
        fs::create_dir_all(dir.join("subdir.ulg")).unwrap();

        let list = list_ulogs(&dir).unwrap();
        let names: Vec<_> = list.iter().map(|s| s.filename.as_str()).collect();
        assert!(names.contains(&"alpha.ulg"));
        assert!(names.contains(&"beta.ulg"));
        assert_eq!(list.len(), 2, "list must skip non-.ulg files and directories");
        assert_eq!(list[0].size_bytes, 0); // empty files
    }

    #[test]
    fn ulog_list_empty_dir_returns_empty() {
        let dir = test_dir();
        let list = list_ulogs(&dir).unwrap();
        assert!(list.is_empty());
    }

    #[test]
    fn ulog_list_nonexistent_dir_returns_empty() {
        // A directory that doesn't exist → empty list, not an error
        // (the operator just sees "no ULogs available yet").
        let dir = test_dir().join("never_created");
        let list = list_ulogs(&dir).unwrap();
        assert!(list.is_empty());
    }

    #[test]
    fn resolve_ulog_path_rejects_traversal() {
        let dir = test_dir();
        touch_ulog(&dir.join("ok.ulg"));

        // Path traversal → NotFound
        let err = resolve_ulog_path(&dir, "../escape.ulg").unwrap_err();
        assert!(matches!(err, ULogError::NotFound(_)));

        // `..` alone → NotFound
        let err = resolve_ulog_path(&dir, "..").unwrap_err();
        assert!(matches!(err, ULogError::NotFound(_)));

        // Empty → NotFound
        let err = resolve_ulog_path(&dir, "").unwrap_err();
        assert!(matches!(err, ULogError::NotFound(_)));

        // Valid file resolves.
        let (p, name) = resolve_ulog_path(&dir, "ok.ulg").unwrap();
        assert!(p.exists());
        assert_eq!(name, "ok.ulg");

        // Bare stem resolves too.
        let (p, _name) = resolve_ulog_path(&dir, "ok").unwrap();
        assert!(p.exists());
    }

    #[test]
    fn validate_topic_name_accepts_known_px4_topics() {
        // Real PX4 topic names use [A-Za-z0-9_]+.
        validate_topic_name("vehicle_local_position").unwrap();
        validate_topic_name("sensor_combined").unwrap();
        validate_topic_name("actuator_armed").unwrap();
        // Subfield-style topic names use a dot (pyulog accepts these
        // in its message_name_filter_list too).
        validate_topic_name("sensor_combined.gyro_rad").unwrap();
        validate_topic_name("vehicle_local_position.x").unwrap();
    }

    #[test]
    fn validate_topic_name_rejects_path_traversal() {
        // Path separators / shell metachars → rejected (the topic name
        // becomes an argument to pyulog, so this is the shell-injection
        // guard).
        assert!(validate_topic_name("../escape").is_err());
        assert!(validate_topic_name("has space").is_err());
        assert!(validate_topic_name("has;rm -rf").is_err());
        assert!(validate_topic_name("").is_err());
        // Long name → rejected (192-char cap).
        let long = "a".repeat(200);
        assert!(validate_topic_name(&long).is_err());
    }

    /// GCS_SPEC.md §5.5 / task spec: "if a real .ulg is available,
    /// test topics; otherwise skip with a note." This test runs only
    /// when the f1/f2 test artifacts (which include real .ulg files
    /// from a PX4 SITL run) are present in the repo.
    #[test]
    fn ulog_topics_calls_pyulog_when_real_ulog_available() {
        let Some(path) = find_real_ulog() else {
            eprintln!("[note] no real .ulg file available; skipping pyulog integration test");
            return;
        };
        let topics = list_topics(&path).expect(
            "pyulog must be installed on /usr/bin/python3.13 (SANDBOX_SETUP.md §5); \
             install via: /usr/bin/python3.13 -m pip install --user --break-system-packages pyulog"
        );
        assert!(!topics.is_empty(), "a real .ulg must have at least one topic");
        // PX4 always logs `vehicle_local_position` and `sensor_combined`
        // by default — assert at least one well-known topic is present.
        let known = [
            "vehicle_local_position",
            "sensor_combined",
            "actuator_armed",
            "vehicle_attitude",
        ];
        let any_known = topics.iter().any(|t| known.contains(&t.as_str()));
        assert!(any_known,
            "expected at least one well-known PX4 topic in {:?}, got: {:?}",
            path, topics);
    }

    /// Bonus: end-to-end test of read_topic_data against a real .ulg
    /// file. Skipped when no real .ulg is present.
    #[test]
    fn ulog_data_returns_topic_range_when_real_ulog_available() {
        let Some(path) = find_real_ulog() else {
            eprintln!("[note] no real .ulg file available; skipping pyulog integration test");
            return;
        };
        let topics = list_topics(&path).unwrap();
        let topic = topics.iter().find(|t| !t.is_empty()).unwrap();
        let data = read_topic_data(&path, topic, None, None).expect(
            "read_topic_data must succeed on a real .ulg with a known topic"
        );
        assert!(!data.t.is_empty(), "topic '{topic}' must have at least one sample");
        assert!(data.total_points > 0);
        assert!(data.t.len() <= MAX_POINTS_PER_REQUEST + 1); // +1 for stride rounding
        assert!(!data.fields.is_empty(),
            "topic '{topic}' must expose at least one field (besides timestamp)");
    }

    #[test]
    fn ulog_topics_returns_error_on_missing_file() {
        let dir = test_dir();
        let err = list_topics(&dir.join("never_existed.ulg")).unwrap_err();
        // pyulog will fail with a FileNotFoundError → surfaces as Pyulog.
        match err {
            ULogError::Pyulog(_) => {}, // expected
            other => panic!("expected Pyulog error for missing file, got {other:?}"),
        }
    }

    #[test]
    fn read_topic_data_rejects_invalid_topic_name() {
        let dir = test_dir();
        touch_ulog(&dir.join("ok.ulg"));
        let (path, _) = resolve_ulog_path(&dir, "ok.ulg").unwrap();
        // Path-separator → rejected by validate_topic_name.
        let err = read_topic_data(&path, "../escape", None, None).unwrap_err();
        assert!(matches!(err, ULogError::NotFound(_)));
        // Shell metachar → rejected.
        let err = read_topic_data(&path, "has;rm -rf", None, None).unwrap_err();
        assert!(matches!(err, ULogError::NotFound(_)));
        // Empty → rejected.
        let err = read_topic_data(&path, "", None, None).unwrap_err();
        assert!(matches!(err, ULogError::NotFound(_)));
    }
}
