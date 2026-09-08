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
}
