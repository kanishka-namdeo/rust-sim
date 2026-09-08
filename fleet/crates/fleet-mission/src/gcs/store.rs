//! ADR-0020: Mission + preset persistence — filesystem with atomic writes.
//!
//! One TOML file per mission version, in a ULID-keyed directory tree.
//! Atomic writes via temp-file + fsync + rename (the SQLite journal
//! pattern). Version history as sibling files. Soft-deletes as
//! `.tombstone` files.

#![forbid(unsafe_code)]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::gcs::mission_file::MissionFile;
use crate::gcs::preset::PresetFile;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum StoreError {
    Io(std::io::Error),
    TomlSerialize(toml::ser::Error),
    TomlDeserialize(toml::de::Error),
    NotFound(String),
    AlreadyExists(String),
    InvalidId(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Io(e) => write!(f, "io: {e}"),
            StoreError::TomlSerialize(e) => write!(f, "toml serialize: {e}"),
            StoreError::TomlDeserialize(e) => write!(f, "toml deserialize: {e}"),
            StoreError::NotFound(id) => write!(f, "mission {id} not found"),
            StoreError::AlreadyExists(id) => write!(f, "mission {id} already exists"),
            StoreError::InvalidId(id) => write!(f, "invalid mission id '{id}'"),
        }
    }
}
impl std::error::Error for StoreError {}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        StoreError::Io(e)
    }
}
impl From<toml::ser::Error> for StoreError {
    fn from(e: toml::ser::Error) -> Self {
        StoreError::TomlSerialize(e)
    }
}
impl From<toml::de::Error> for StoreError {
    fn from(e: toml::de::Error) -> Self {
        StoreError::TomlDeserialize(e)
    }
}

// ---------------------------------------------------------------------------
// Meta file (catalog metadata, JSON on disk)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct MetaFile {
    name: String,
    created_at: String,
    updated_at: String,
    current_version: u32,
    deleted: bool,
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

/// The mission catalog store. Owns a root directory on disk.
pub struct Store {
    root: PathBuf,
}

impl Store {
    /// Create a new store rooted at `root`. Creates the directory tree
    /// if it does not exist.
    pub fn new(root: impl AsRef<Path>) -> Result<Self, StoreError> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root)?;
        fs::create_dir_all(root.join("missions"))?;
        fs::create_dir_all(root.join("presets"))?;
        fs::create_dir_all(root.join("replays"))?;
        Ok(Store { root })
    }

    fn missions_dir(&self) -> PathBuf {
        self.root.join("missions")
    }

    fn mission_dir(&self, id: &str) -> PathBuf {
        self.missions_dir().join(id)
    }

    fn version_path(&self, id: &str, version: u32) -> PathBuf {
        self.mission_dir(id).join(format!("v{version}.toml"))
    }

    fn meta_path(&self, id: &str) -> PathBuf {
        self.mission_dir(id).join("meta.json")
    }

    fn tombstone_path(&self, id: &str) -> PathBuf {
        self.mission_dir(id).join(".tombstone")
    }

    // -----------------------------------------------------------------
    // Atomic write (ADR-0020 §Atomic write protocol)
    // -----------------------------------------------------------------

    /// Write `content` to `final_path` atomically: write to a temp file
    /// in the same directory, fsync, rename, fsync the directory.
    fn write_atomic(final_path: &Path, content: &str) -> Result<(), StoreError> {
        let dir = final_path
            .parent()
            .ok_or_else(|| StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "path has no parent",
            )))?;
        fs::create_dir_all(dir)?;

        let pid = std::process::id();
        let rand: u64 = {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            now as u64
        };
        let tmp_path = dir.join(format!(
            "{}.tmp.{pid}.{rand}",
            final_path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("file")
        ));

        // Step 1: write content to temp file
        let mut f = fs::File::create(&tmp_path)?;
        f.write_all(content.as_bytes())?;
        f.sync_all()?; // Step 2: fsync the temp file

        // Step 3: rename (atomic on POSIX)
        fs::rename(&tmp_path, final_path)?;

        // Step 4: fsync the parent directory
        Self::fsync_dir(dir)?;

        Ok(())
    }

    #[cfg(unix)]
    fn fsync_dir(dir: &Path) -> Result<(), StoreError> {
        let f = fs::File::open(dir)?;
        f.sync_all()?;
        Ok(())
    }

    #[cfg(not(unix))]
    fn fsync_dir(_dir: &Path) -> Result<(), StoreError> {
        // On non-Unix (Windows), directory fsync is not standard.
        // The rename is still atomic via MoveFileEx.
        Ok(())
    }

    // -----------------------------------------------------------------
    // CRUD
    // -----------------------------------------------------------------

    /// Create a new mission. Generates a ULID, writes v1, writes meta.json.
    /// Returns the created mission with its assigned id.
    pub fn create(&self, mut mission: MissionFile) -> Result<MissionFile, StoreError> {
        let id = ulid_string();
        let now = now_rfc3339();
        mission.mission.id = id.clone();
        mission.mission.version = 1;
        mission.mission.created_at = now.clone();
        mission.mission.updated_at = now;

        let dir = self.mission_dir(&id);
        fs::create_dir_all(&dir)?;

        let toml_str = mission.to_toml_pretty()?;
        Self::write_atomic(&self.version_path(&id, 1), &toml_str)?;
        self.write_meta(&id, &mission)?;

        Ok(mission)
    }

    /// List all non-deleted missions (id, name, version, updated_at,
    /// waypoint_count, fence_count, rally_count).
    pub fn list(&self) -> Result<Vec<MissionSummary>, StoreError> {
        let mut summaries = Vec::new();
        if !self.missions_dir().exists() {
            return Ok(summaries);
        }
        for entry in fs::read_dir(self.missions_dir())? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let id = match entry.file_name().to_str() {
                Some(s) => s.to_string(),
                None => continue,
            };
            if self.is_tombstoned(&id) {
                continue;
            }
            if let Ok(meta) = self.read_meta(&id) {
                let mission = self.load_latest(&id).ok();
                let (waypoint_count, fence_count, rally_count) = mission
                    .as_ref()
                    .map(|m| (m.waypoints.len(), m.geofence.inclusion.len(), m.rally.len()))
                    .unwrap_or((0, 0, 0));
                summaries.push(MissionSummary {
                    id: id.clone(),
                    name: meta.name,
                    version: meta.current_version,
                    updated_at: meta.updated_at,
                    waypoint_count,
                    fence_count,
                    rally_count,
                    deleted: false,
                });
            }
        }
        summaries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at)); // most recent first
        Ok(summaries)
    }

    /// List all missions including deleted ones (with `deleted: true`).
    pub fn list_all(&self) -> Result<Vec<MissionSummary>, StoreError> {
        let mut summaries = self.list()?;
        for entry in fs::read_dir(self.missions_dir())? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let id = match entry.file_name().to_str() {
                Some(s) => s.to_string(),
                None => continue,
            };
            if self.is_tombstoned(&id) {
                if let Ok(meta) = self.read_meta(&id) {
                    summaries.push(MissionSummary {
                        id,
                        name: meta.name,
                        version: meta.current_version,
                        updated_at: meta.updated_at,
                        waypoint_count: 0,
                        fence_count: 0,
                        rally_count: 0,
                        deleted: true,
                    });
                }
            }
        }
        Ok(summaries)
    }

    /// Fetch the latest version of a mission by id.
    pub fn get(&self, id: &str) -> Result<MissionFile, StoreError> {
        let meta = self.read_meta(id)?;
        let path = self.version_path(id, meta.current_version);
        let toml_str = fs::read_to_string(&path)
            .map_err(|_| StoreError::NotFound(id.to_string()))?;
        MissionFile::from_toml_str(&toml_str).map_err(Into::into)
    }

    /// Fetch a specific version of a mission.
    pub fn get_version(&self, id: &str, version: u32) -> Result<MissionFile, StoreError> {
        let path = self.version_path(id, version);
        if !path.exists() {
            return Err(StoreError::NotFound(format!("{id} v{version}")));
        }
        let toml_str = fs::read_to_string(&path)
            .map_err(|_| StoreError::NotFound(format!("{id} v{version}")))?;
        MissionFile::from_toml_str(&toml_str).map_err(Into::into)
    }

    /// Update a mission. Creates a new version (v<N+1>), updates meta.
    pub fn update(&self, id: &str, mut mission: MissionFile) -> Result<MissionFile, StoreError> {
        let mut meta = self.read_meta(id)?;
        if self.is_tombstoned(id) {
            return Err(StoreError::NotFound(id.to_string()));
        }
        let new_version = meta.current_version + 1;
        mission.mission.id = id.to_string();
        mission.mission.version = new_version;
        mission.mission.created_at = meta.created_at.clone();
        mission.mission.updated_at = now_rfc3339();

        let toml_str = mission.to_toml_pretty()?;
        Self::write_atomic(&self.version_path(id, new_version), &toml_str)?;

        meta.current_version = new_version;
        meta.updated_at = mission.mission.updated_at.clone();
        self.write_meta(id, &mission)?;

        Ok(mission)
    }

    /// Soft-delete a mission (writes a .tombstone file).
    pub fn delete(&self, id: &str) -> Result<(), StoreError> {
        if !self.mission_dir(id).exists() {
            return Err(StoreError::NotFound(id.to_string()));
        }
        let tombstone_content = now_rfc3339();
        Self::write_atomic(&self.tombstone_path(id), &tombstone_content)?;
        Ok(())
    }

    /// Roll back to a previous version (creates a new version whose
    /// content is a copy of the target version).
    pub fn rollback(&self, id: &str, to_version: u32) -> Result<MissionFile, StoreError> {
        let old = self.get_version(id, to_version)?;
        self.update(id, old)
    }

    // -----------------------------------------------------------------
    // Meta.json helpers
    // -----------------------------------------------------------------

    fn write_meta(&self, id: &str, mission: &MissionFile) -> Result<(), StoreError> {
        let meta = MetaFile {
            name: mission.mission.name.clone(),
            created_at: mission.mission.created_at.clone(),
            updated_at: mission.mission.updated_at.clone(),
            current_version: mission.mission.version,
            deleted: false,
        };
        let json = serde_json::to_string_pretty(&meta)
            .map_err(|e| StoreError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())))?;
        Self::write_atomic(&self.meta_path(id), &json)?;
        Ok(())
    }

    fn read_meta(&self, id: &str) -> Result<MetaFile, StoreError> {
        let path = self.meta_path(id);
        if !path.exists() {
            return Err(StoreError::NotFound(id.to_string()));
        }
        let json = fs::read_to_string(&path)
            .map_err(|_| StoreError::NotFound(id.to_string()))?;
        serde_json::from_str(&json)
            .map_err(|e| StoreError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())))
    }

    fn is_tombstoned(&self, id: &str) -> bool {
        self.tombstone_path(id).exists()
    }

    fn load_latest(&self, id: &str) -> Result<MissionFile, StoreError> {
        let meta = self.read_meta(id)?;
        let path = self.version_path(id, meta.current_version);
        let toml_str = fs::read_to_string(&path)
            .map_err(|_| StoreError::NotFound(id.to_string()))?;
        MissionFile::from_toml_str(&toml_str).map_err(Into::into)
    }

    // -----------------------------------------------------------------
    // Param presets (ADR-0025, GCS_SPEC.md §5.3 — Vehicle Setup)
    // -----------------------------------------------------------------
    //
    // One TOML file per preset, at
    // `<root>/presets/vehicle_<i>/<name>.toml`. Vehicle is the fleet
    // index (0..count-1) — presets are per-vehicle because each
    // vehicle's param store is its own. The `name` is the operator's
    // chosen preset identifier ("aggressive-corners", "high-wind").
    //
    // Names are constrained to `[A-Za-z0-9_-]+` (no path separators,
    // no shell metachars, no leading dots) — the name becomes a path
    // component, so this is the path-injection guard. Matches QGC's
    // own preset-name validation regex.

    /// Path to the directory holding vehicle `i`'s presets.
    pub fn presets_dir(&self, vehicle_id: u8) -> PathBuf {
        self.root.join("presets").join(format!("vehicle_{vehicle_id}"))
    }

    /// Path to a specific preset file. Caller is responsible for having
    /// already validated the name (see [`validate_preset_name`]).
    fn preset_path(&self, vehicle_id: u8, name: &str) -> PathBuf {
        self.presets_dir(vehicle_id).join(format!("{name}.toml"))
    }

    /// List all param presets saved for vehicle `i`. Returns the
    /// summary form (name, created_at, param_count) — QGC's preset
    /// picker shows the summary; the full preset body is fetched
    /// separately via [`Self::load_preset`]. Stable name-sorted order.
    pub fn list_presets(
        &self,
        vehicle_id: u8,
    ) -> Result<Vec<PresetSummary>, StoreError> {
        let dir = self.presets_dir(vehicle_id);
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut summaries = Vec::new();
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                continue;
            }
            let stem = match path.file_stem().and_then(|s| s.to_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };
            let toml_str = match fs::read_to_string(&path) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let preset: PresetFile = match PresetFile::from_toml_str(&toml_str) {
                Ok(p) => p,
                Err(_) => continue,
            };
            summaries.push(PresetSummary {
                name: preset.name,
                created_at: preset.created_at,
                param_count: preset.params.len(),
            });
            let _ = stem; // stem is unused; we trust the TOML's `name` field
        }
        // Stable, name-sorted order (QGC sorts its preset picker the same).
        summaries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(summaries)
    }

    /// Save a param preset for vehicle `i`. Overwrites an existing
    /// preset of the same name (QGC's "Save As" replaces silently when
    /// the operator confirms the overwrite dialog). Atomic write
    /// (temp-file + fsync + rename) — same protocol as mission writes.
    /// Returns the saved [`PresetFile`] with `created_at` filled in.
    pub fn save_preset(
        &self,
        vehicle_id: u8,
        name: &str,
        params: Vec<crate::gcs::preset::PresetParam>,
    ) -> Result<PresetFile, StoreError> {
        validate_preset_name(name)?;
        let preset = PresetFile {
            name: name.to_string(),
            created_at: now_rfc3339(),
            vehicle_id,
            params,
        };
        let dir = self.presets_dir(vehicle_id);
        fs::create_dir_all(&dir)?;
        let path = self.preset_path(vehicle_id, name);
        let toml_str = preset.to_toml_pretty()?;
        Self::write_atomic(&path, &toml_str)?;
        Ok(preset)
    }

    /// Load a param preset by name. Returns the full preset body so
    /// the catalog's `/load` endpoint can hand the param list back to
    /// the operator (or push it to the vehicle via PARAM_SET).
    pub fn load_preset(&self, vehicle_id: u8, name: &str) -> Result<PresetFile, StoreError> {
        validate_preset_name(name)?;
        let path = self.preset_path(vehicle_id, name);
        if !path.exists() {
            return Err(StoreError::NotFound(format!("preset '{name}'")));
        }
        let toml_str = fs::read_to_string(&path)
            .map_err(|_| StoreError::NotFound(format!("preset '{name}'")))?;
        PresetFile::from_toml_str(&toml_str).map_err(Into::into)
    }

    /// Delete a param preset by name. Errors if the preset does not
    /// exist (QGC's "Delete" button only appears on existing presets,
    /// so a 404 here surfaces a real race the operator should see).
    pub fn delete_preset(&self, vehicle_id: u8, name: &str) -> Result<(), StoreError> {
        validate_preset_name(name)?;
        let path = self.preset_path(vehicle_id, name);
        if !path.exists() {
            return Err(StoreError::NotFound(format!("preset '{name}'")));
        }
        fs::remove_file(&path)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Summary (for list endpoints)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize)]
pub struct MissionSummary {
    pub id: String,
    pub name: String,
    pub version: u32,
    pub updated_at: String,
    pub waypoint_count: usize,
    pub fence_count: usize,
    pub rally_count: usize,
    pub deleted: bool,
}

// ---------------------------------------------------------------------------
// Preset summary (for the list-presets endpoint)
// ---------------------------------------------------------------------------

/// Summary form returned by `GET /api/vehicles/{i}/param-presets`
/// (GCS_SPEC.md §5.3): QGC's preset picker shows name + count, not the
/// full body — the full body is fetched separately via `/load`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PresetSummary {
    pub name: String,
    pub created_at: String,
    pub param_count: usize,
}

/// Validate a preset name: non-empty, 1..=64 chars, filesystem-safe
/// (`[A-Za-z0-9_-]+` — no path separators, no shell metachars, no
/// leading dots). The name becomes a path component under
/// `<catalog>/presets/vehicle_<i>/`, so this is the path-injection
/// guard. Matches QGC's own preset-name validation regex.
fn validate_preset_name(name: &str) -> Result<(), StoreError> {
    if name.is_empty() || name.len() > 64 {
        return Err(StoreError::InvalidId(format!(
            "preset name '{name}' must be 1..=64 chars"
        )));
    }
    let valid = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if !valid {
        return Err(StoreError::InvalidId(format!(
            "preset name '{name}' must match [A-Za-z0-9_-]+"
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Generate a ULID-like string: 26 chars of Crockford Base32, time-sorted.
/// Simplified implementation (time-ms + random) — 26 chars, Crockford
/// Base32, sortable. Good enough for non-colliding IDs in single-operator v1.
fn ulid_string() -> String {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();

    // 48 bits of time (6 bytes) + 80 bits of randomness (10 bytes) = 128 bits = 26 Crockford chars
    let mut bytes = [0u8; 16];
    let time_bytes = now_ms.to_be_bytes(); // u128 → 16 bytes, we take the last 6
    bytes[0..6].copy_from_slice(&time_bytes[10..16]);

    // pseudo-random fill (good enough for non-colliding IDs in single-operator v1)
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let pid = std::process::id() as u64;
    let mix = nanos as u64 ^ (pid << 32) ^ (now_ms as u64);
    let mix_bytes = mix.to_be_bytes(); // 8 bytes
    bytes[6..14].copy_from_slice(&mix_bytes);
    let extra = (nanos as u16).to_be_bytes(); // 2 bytes
    bytes[14..16].copy_from_slice(&extra);

    encode_crockford(&bytes)
}

fn encode_crockford(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    // Convert 16 bytes (128 bits) to 26 Crockford Base32 chars.
    // Process 5 bits at a time from MSB to LSB. 128 bits = 25.6 chars,
    // so we pad to 130 bits (26 chars) with 2 leading zero bits.
    let mut bits: u128 = 0;
    for &b in bytes {
        bits = (bits << 8) | b as u128;
    }
    // Now bits holds 128 bits. We need 26 chars × 5 bits = 130 bits.
    // Left-shift by 2 to get 130 bits, then extract 5-bit groups from the top.
    bits <<= 2; // now 130 bits
    let mut chars = Vec::with_capacity(26);
    for i in (0..26).rev() {
        let shift = i * 5;
        let idx = ((bits >> shift) & 0x1F) as usize;
        chars.push(ALPHABET[idx]);
    }
    // chars[0] is the most significant (time-based, sortable)
    String::from_utf8(chars).unwrap_or_else(|_| "00000000000000000000000000".into())
}

fn now_rfc3339() -> String {
    // Minimal RFC 3339 UTC timestamp without pulling chrono.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let (year, month, day, hour, min, sec) = unix_to_utc(secs);
    format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}Z"
    )
}

/// Convert Unix seconds to UTC (Y, M, D, h, m, s). Valid for 1970-2099.
fn unix_to_utc(secs: u64) -> (u32, u32, u32, u32, u32, u32) {
    let days = (secs / 86400) as u32;
    let rem = (secs % 86400) as u32;
    let hour = rem / 3600;
    let min = (rem % 3600) / 60;
    let sec = rem % 60;

    // Days since 1970-01-01 → calendar date (proleptic Gregorian)
    let mut year = 1970u32;
    let mut day_of_year = days;
    loop {
        let is_leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
        let days_in_year = if is_leap { 366 } else { 365 };
        if day_of_year < days_in_year {
            break;
        }
        day_of_year -= days_in_year;
        year += 1;
    }
    let is_leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
    let month_days: [u32; 12] = [31, if is_leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month = 1u32;
    let mut day = day_of_year;
    for &md in &month_days {
        if day < md {
            break;
        }
        day -= md;
        month += 1;
    }
    (year, month, day + 1, hour, min, sec)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gcs::mission_file::{Geofence, MissionFile, MissionMeta, Waypoint};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn test_dir() -> PathBuf {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("rustsim_store_test_{}_{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn sample_mission(name: &str) -> MissionFile {
        MissionFile {
            mission: MissionMeta {
                id: "".into(),
                name: name.into(),
                version: 0,
                created_at: "".into(),
                updated_at: "".into(),
                vehicle_type: "quad".into(),
                px4_version: "v1.16.2".into(),
            },
            waypoints: vec![Waypoint {
                seq: 0, frame: 3, command: 16,
                x: 47.397770, y: 8.545580, z: 12.0,
                param1: 0.0, param2: 2.0, param3: 0.0, param4: 0.0,
            }],
            geofence: Geofence {
                ceiling_m: 60.0, floor_m: 0.0,
                inclusion: vec![
                    [47.3970, 8.5450], [47.3980, 8.5450],
                    [47.3980, 8.5460], [47.3970, 8.5460],
                ],
                exclusion: vec![],
            },
            rally: vec![],
        }
    }

    #[test]
    fn create_get_roundtrip() {
        let dir = test_dir();
        let store = Store::new(&dir).unwrap();
        let m = sample_mission("test-create");
        let created = store.create(m).unwrap();
        assert!(!created.mission.id.is_empty());
        assert_eq!(created.mission.version, 1);

        let fetched = store.get(&created.mission.id).unwrap();
        assert_eq!(fetched.mission.name, "test-create");
        assert_eq!(fetched.waypoints.len(), 1);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn list_excludes_deleted() {
        let dir = test_dir();
        let store = Store::new(&dir).unwrap();
        let m1 = store.create(sample_mission("m1")).unwrap();
        let _m2 = store.create(sample_mission("m2")).unwrap();
        assert_eq!(store.list().unwrap().len(), 2);

        store.delete(&m1.mission.id).unwrap();
        let live = store.list().unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].name, "m2");

        let all = store.list_all().unwrap();
        assert_eq!(all.len(), 2);
        let deleted = all.iter().find(|s| s.name == "m1").unwrap();
        assert!(deleted.deleted);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn update_creates_new_version() {
        let dir = test_dir();
        let store = Store::new(&dir).unwrap();
        let created = store.create(sample_mission("v1")).unwrap();
        let id = created.mission.id.clone();

        let mut m2 = store.get(&id).unwrap();
        m2.mission.name = "v2".into();
        m2.waypoints.push(Waypoint {
            seq: 1, frame: 3, command: 16,
            x: 47.3978, y: 8.5456, z: 15.0,
            param1: 0.0, param2: 2.0, param3: 0.0, param4: 0.0,
        });
        let updated = store.update(&id, m2).unwrap();
        assert_eq!(updated.mission.version, 2);
        assert_eq!(updated.waypoints.len(), 2);

        // v1 still exists
        let v1 = store.get_version(&id, 1).unwrap();
        assert_eq!(v1.waypoints.len(), 1);
        assert_eq!(v1.mission.name, "v1");

        // latest is v2
        let latest = store.get(&id).unwrap();
        assert_eq!(latest.mission.version, 2);
        assert_eq!(latest.waypoints.len(), 2);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rollback_creates_new_version_with_old_content() {
        let dir = test_dir();
        let store = Store::new(&dir).unwrap();
        let created = store.create(sample_mission("v1")).unwrap();
        let id = created.mission.id.clone();

        // Update to v2
        let mut m2 = store.get(&id).unwrap();
        m2.mission.name = "v2".into();
        store.update(&id, m2).unwrap();

        // Roll back to v1 → creates v3 with v1's content
        let rolled = store.rollback(&id, 1).unwrap();
        assert_eq!(rolled.mission.version, 3);
        assert_eq!(rolled.mission.name, "v1");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn get_nonexistent_returns_not_found() {
        let dir = test_dir();
        let store = Store::new(&dir).unwrap();
        let err = store.get("nonexistent-id").unwrap_err();
        assert!(matches!(err, StoreError::NotFound(_)));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn atomic_write_survives_directory_inspection() {
        // The on-disk layout should match ADR-0020 exactly:
        // missions/<id>/v1.toml, missions/<id>/meta.json
        let dir = test_dir();
        let store = Store::new(&dir).unwrap();
        let created = store.create(sample_mission("layout")).unwrap();
        let id = &created.mission.id;

        let mission_dir = dir.join("missions").join(id);
        assert!(mission_dir.exists());
        assert!(mission_dir.join("v1.toml").exists());
        assert!(mission_dir.join("meta.json").exists());
        assert!(!mission_dir.join(".tombstone").exists());

        // No temp files left behind
        let entries: Vec<_> = fs::read_dir(&mission_dir).unwrap().collect();
        for e in entries {
            let name = e.unwrap().file_name();
            let name = name.to_str().unwrap();
            assert!(!name.contains(".tmp."), "temp file left behind: {name}");
        }

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ulid_is_26_chars_and_sortable() {
        let id1 = ulid_string();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let id2 = ulid_string();
        assert_eq!(id1.len(), 26, "ULID must be 26 chars, got {}: {id1}", id1.len());
        assert_eq!(id2.len(), 26);
        // id2 should sort after id1 (time-ordered)
        assert!(id2 > id1, "ULID not sortable: {id1} vs {id2}");
    }

    #[test]
    fn now_rfc3339_is_valid_format() {
        let ts = now_rfc3339();
        // 2026-09-09T10:00:00Z format
        assert_eq!(ts.len(), 20);
        assert_eq!(ts.as_bytes()[4], b'-');
        assert_eq!(ts.as_bytes()[10], b'T');
        assert_eq!(ts.as_bytes()[19], b'Z');
    }

    // -----------------------------------------------------------------
    // Param preset tests (M4 — GCS_SPEC.md §5.3)
    // -----------------------------------------------------------------

    use crate::gcs::preset::PresetParam;

    fn sample_preset_params() -> Vec<PresetParam> {
        vec![
            PresetParam { id: "MPC_XY_VEL_MAX".into(), value: 8.0, param_type: 9 },
            PresetParam { id: "MC_ROLLRATE_P".into(), value: 7.5, param_type: 9 },
            PresetParam { id: "BAT_N_CELLS".into(), value: 6.0, param_type: 6 },
        ]
    }

    #[test]
    fn preset_save_load_roundtrip() {
        let dir = test_dir();
        let store = Store::new(&dir).unwrap();

        // Save
        let saved = store
            .save_preset(0, "aggressive-corners", sample_preset_params())
            .unwrap();
        assert_eq!(saved.name, "aggressive-corners");
        assert_eq!(saved.vehicle_id, 0);
        assert_eq!(saved.params.len(), 3);
        assert!(!saved.created_at.is_empty());

        // List — one preset, with param_count=3
        let list = store.list_presets(0).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "aggressive-corners");
        assert_eq!(list[0].param_count, 3);
        assert_eq!(list[0].created_at, saved.created_at);

        // Load — params match what we saved
        let loaded = store.load_preset(0, "aggressive-corners").unwrap();
        assert_eq!(loaded.name, "aggressive-corners");
        assert_eq!(loaded.vehicle_id, 0);
        assert_eq!(loaded.params.len(), 3);
        assert_eq!(loaded.params[0].id, "MPC_XY_VEL_MAX");
        assert_eq!(loaded.params[0].value, 8.0);
        assert_eq!(loaded.params[0].param_type, 9);
        assert_eq!(loaded.params[1].id, "MC_ROLLRATE_P");
        assert_eq!(loaded.params[1].value, 7.5);
        assert_eq!(loaded.params[2].id, "BAT_N_CELLS");
        assert_eq!(loaded.params[2].value, 6.0);
        assert_eq!(loaded.params[2].param_type, 6); // INT32 preserved

        // The on-disk file is TOML at the spec-defined path.
        let path = dir.join("presets").join("vehicle_0").join("aggressive-corners.toml");
        assert!(path.exists(), "preset file must exist at {path:?}");
        let on_disk = fs::read_to_string(&path).unwrap();
        assert!(on_disk.contains("name = \"aggressive-corners\""));
        assert!(on_disk.contains("vehicle_id = 0"));
        assert!(on_disk.contains("[[params]]"));
        assert!(on_disk.contains("type = 9"));
        assert!(on_disk.contains("type = 6"));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn preset_delete_removes_file() {
        let dir = test_dir();
        let store = Store::new(&dir).unwrap();

        // Save then list — 1 preset
        store.save_preset(0, "to-delete", sample_preset_params()).unwrap();
        assert_eq!(store.list_presets(0).unwrap().len(), 1);

        // Delete then list — 0 presets
        store.delete_preset(0, "to-delete").unwrap();
        assert_eq!(store.list_presets(0).unwrap().len(), 0);

        // The on-disk file is gone
        let path = dir.join("presets").join("vehicle_0").join("to-delete.toml");
        assert!(!path.exists());

        // Deleting again returns NotFound (QGC's "Delete" only appears on
        // existing presets; a 404 here surfaces a real race).
        let err = store.delete_preset(0, "to-delete").unwrap_err();
        assert!(matches!(err, StoreError::NotFound(_)));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn preset_list_isolated_per_vehicle() {
        let dir = test_dir();
        let store = Store::new(&dir).unwrap();
        store.save_preset(0, "v0-preset", sample_preset_params()).unwrap();
        store.save_preset(1, "v1-preset", sample_preset_params()).unwrap();
        // Each vehicle sees only its own presets
        assert_eq!(store.list_presets(0).unwrap().len(), 1);
        assert_eq!(store.list_presets(1).unwrap().len(), 1);
        assert_eq!(store.list_presets(0).unwrap()[0].name, "v0-preset");
        assert_eq!(store.list_presets(1).unwrap()[0].name, "v1-preset");
        // Vehicle 2 has no presets → empty list (not an error)
        assert_eq!(store.list_presets(2).unwrap().len(), 0);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn preset_save_overwrites_existing() {
        let dir = test_dir();
        let store = Store::new(&dir).unwrap();
        // Save v1
        store.save_preset(0, "evolving", vec![
            PresetParam { id: "MPC_XY_VEL_MAX".into(), value: 8.0, param_type: 9 },
        ]).unwrap();
        assert_eq!(store.list_presets(0).unwrap()[0].param_count, 1);
        // Save v2 with the same name — overwrites
        store.save_preset(0, "evolving", vec![
            PresetParam { id: "MPC_XY_VEL_MAX".into(), value: 10.0, param_type: 9 },
            PresetParam { id: "MC_ROLLRATE_P".into(), value: 7.5, param_type: 9 },
        ]).unwrap();
        let list = store.list_presets(0).unwrap();
        assert_eq!(list.len(), 1); // not 2 — overwrite, not duplicate
        assert_eq!(list[0].param_count, 2);
        let loaded = store.load_preset(0, "evolving").unwrap();
        assert_eq!(loaded.params[0].value, 10.0); // latest value, not the original 8.0
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn preset_name_validation_rejects_path_traversal() {
        let dir = test_dir();
        let store = Store::new(&dir).unwrap();
        // Path separators → rejected
        let err = store.save_preset(0, "../escape", sample_preset_params()).unwrap_err();
        assert!(matches!(err, StoreError::InvalidId(_)));
        // Leading dot → rejected
        let err = store.save_preset(0, ".hidden", sample_preset_params()).unwrap_err();
        assert!(matches!(err, StoreError::InvalidId(_)));
        // Spaces → rejected
        let err = store.save_preset(0, "has space", sample_preset_params()).unwrap_err();
        assert!(matches!(err, StoreError::InvalidId(_)));
        // Empty name → rejected
        let err = store.save_preset(0, "", sample_preset_params()).unwrap_err();
        assert!(matches!(err, StoreError::InvalidId(_)));
        // Valid names accepted
        store.save_preset(0, "aggressive-corners", sample_preset_params()).unwrap();
        store.save_preset(0, "high_wind_2026", sample_preset_params()).unwrap();
        store.save_preset(0, "Preset1", sample_preset_params()).unwrap();
        assert_eq!(store.list_presets(0).unwrap().len(), 3);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn preset_load_nonexistent_returns_not_found() {
        let dir = test_dir();
        let store = Store::new(&dir).unwrap();
        let err = store.load_preset(0, "never-saved").unwrap_err();
        assert!(matches!(err, StoreError::NotFound(_)));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn presets_dir_returns_spec_path() {
        let dir = test_dir();
        let store = Store::new(&dir).unwrap();
        // GCS_SPEC.md §5.3 + ADR-0025: <root>/presets/vehicle_<i>/
        assert_eq!(
            store.presets_dir(0),
            dir.join("presets").join("vehicle_0")
        );
        assert_eq!(
            store.presets_dir(7),
            dir.join("presets").join("vehicle_7")
        );
        fs::remove_dir_all(&dir).unwrap();
    }
}
