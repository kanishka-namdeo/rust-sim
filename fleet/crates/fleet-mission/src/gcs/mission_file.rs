//! ADR-0019: Mission file format — TOML on disk, JSON on the wire.
//!
//! Single serde `MissionFile` struct, two serializations. The catalog
//! store writes TOML; every REST endpoint accepts/returns JSON. Both
//! go through serde so a schema change is one Rust edit.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// The mission struct tree
// ---------------------------------------------------------------------------

/// Top-level mission file (ADR-0019 §Decision). One per saved mission.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissionFile {
    pub mission: MissionMeta,
    #[serde(default)]
    pub waypoints: Vec<Waypoint>,
    #[serde(default)]
    pub geofence: Geofence,
    #[serde(default)]
    pub rally: Vec<RallyPoint>,
}

/// `[mission]` metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissionMeta {
    /// ULID (26 chars, sortable). Set by the catalog on create.
    pub id: String,
    /// Operator-chosen display name.
    pub name: String,
    /// Monotonic version; starts at 1, bumps on every PUT.
    pub version: u32,
    /// RFC 3339 creation timestamp.
    pub created_at: String,
    /// RFC 3339 last-update timestamp.
    pub updated_at: String,
    /// "quad" | "fixed" | "vtol" | "rover" — drives validation V-5.
    #[serde(default = "default_vehicle_type")]
    pub vehicle_type: String,
    /// The pinned PX4 version this mission was authored against.
    /// Informational in v1 (ADR-0029 checks the vehicle's version, not
    /// the mission's); may become enforced in v1.1.
    #[serde(default = "default_px4_version")]
    pub px4_version: String,
}

fn default_vehicle_type() -> String {
    "quad".into()
}
fn default_px4_version() -> String {
    "v1.16.2".into()
}

/// A single flight-plan waypoint. Maps 1:1 to a MAVLink `MISSION_ITEM_INT`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Waypoint {
    /// 0-indexed sequence number.
    pub seq: u16,
    /// MAV_FRAME_* (V-13). 3 = GLOBAL_RELATIVE_ALT (default).
    pub frame: u8,
    /// MAV_CMD_* (V-12). 16 = NAV_WAYPOINT (default).
    pub command: u16,
    /// Latitude (frame 0/3) or local X (frame 1/8).
    pub x: f64,
    /// Longitude (frame 0/3) or local Y (frame 1/8).
    pub y: f64,
    /// Altitude in metres (relative to home for frame 3).
    pub z: f32,
    /// param1: hold time in seconds for MAV_CMD_NAV_WAYPOINT.
    #[serde(default)]
    pub param1: f32,
    /// param2: accept radius in metres.
    #[serde(default)]
    pub param2: f32,
    /// param3: pass radius in metres.
    #[serde(default)]
    pub param3: f32,
    /// param4: yaw in degrees (NaN = face direction of travel).
    #[serde(default)]
    pub param4: f32,
}

/// `[geofence]` section.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Geofence {
    #[serde(default)]
    pub ceiling_m: f32,
    #[serde(default)]
    pub floor_m: f32,
    /// Inclusion polygon: Vec of [lat, lon] pairs. Empty = no fence.
    #[serde(default)]
    pub inclusion: Vec<[f64; 2]>,
    /// Exclusion polygons: list of polygons, each a Vec of [lat, lon].
    #[serde(default)]
    pub exclusion: Vec<Vec<[f64; 2]>>,
}

/// A rally / safe-landing point.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RallyPoint {
    pub seq: u16,
    pub lat: f64,
    pub lon: f64,
    pub alt_m: f32,
}

// ---------------------------------------------------------------------------
// Serialization helpers
// ---------------------------------------------------------------------------

impl MissionFile {
    /// Serialize to a pretty TOML string (for on-disk storage).
    pub fn to_toml_pretty(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    /// Deserialize from a TOML string (from disk or REST body).
    pub fn from_toml_str(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    /// Serialize to a JSON Value (for REST API responses).
    pub fn to_json_value(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }

    /// Deserialize from a JSON string (from REST body).
    pub fn from_json_str(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_mission() -> MissionFile {
        MissionFile {
            mission: MissionMeta {
                id: "01J8K2EXAMPLE00000000000001".into(),
                name: "demo-square".into(),
                version: 1,
                created_at: "2026-09-09T10:00:00Z".into(),
                updated_at: "2026-09-09T10:00:00Z".into(),
                vehicle_type: "quad".into(),
                px4_version: "v1.16.2".into(),
            },
            waypoints: vec![
                Waypoint {
                    seq: 0,
                    frame: 3,
                    command: 16,
                    x: 37.4135,
                    y: -122.1015,
                    z: 12.0,
                    param1: 0.5,
                    param2: 2.0,
                    param3: 0.0,
                    param4: 0.0,
                },
                Waypoint {
                    seq: 1,
                    frame: 3,
                    command: 16,
                    x: 37.4140,
                    y: -122.1015,
                    z: 12.0,
                    param1: 0.0,
                    param2: 2.0,
                    param3: 0.0,
                    param4: 0.0,
                },
            ],
            geofence: Geofence {
                ceiling_m: 60.0,
                floor_m: 0.0,
                inclusion: vec![
                    [37.4130, -122.1020],
                    [37.4140, -122.1020],
                    [37.4140, -122.1010],
                    [37.4130, -122.1010],
                ],
                exclusion: vec![],
            },
            rally: vec![RallyPoint {
                seq: 0,
                lat: 37.4133,
                lon: -122.1014,
                alt_m: 0.0,
            }],
        }
    }

    #[test]
    fn toml_roundtrip_preserves_fields() {
        let m = sample_mission();
        let toml_str = m.to_toml_pretty().unwrap();
        let m2 = MissionFile::from_toml_str(&toml_str).unwrap();
        assert_eq!(m2.mission.id, m.mission.id);
        assert_eq!(m2.mission.name, m.mission.name);
        assert_eq!(m2.waypoints.len(), 2);
        assert_eq!(m2.waypoints[0].x, 37.4135);
        assert_eq!(m2.geofence.ceiling_m, 60.0);
        assert_eq!(m2.geofence.inclusion.len(), 4);
        assert_eq!(m2.rally.len(), 1);
    }

    #[test]
    fn json_roundtrip_preserves_fields() {
        let m = sample_mission();
        let json_str = serde_json::to_string(&m).unwrap();
        let m2 = MissionFile::from_json_str(&json_str).unwrap();
        assert_eq!(m2.mission.id, m.mission.id);
        assert_eq!(m2.waypoints.len(), 2);
        assert_eq!(m2.geofence.inclusion.len(), 4);
    }

    #[test]
    fn toml_to_json_to_toml_preserves_fields() {
        let m = sample_mission();
        let toml_str = m.to_toml_pretty().unwrap();
        let json_val = m.to_json_value();
        let json_str = serde_json::to_string(&json_val).unwrap();
        let m_from_json = MissionFile::from_json_str(&json_str).unwrap();
        let toml_str2 = m_from_json.to_toml_pretty().unwrap();
        let m2 = MissionFile::from_toml_str(&toml_str2).unwrap();
        assert_eq!(m2.mission.id, m.mission.id);
        assert_eq!(m2.waypoints.len(), m.waypoints.len());
        assert_eq!(m2.geofence.inclusion.len(), m.geofence.inclusion.len());
        // The two TOML strings should be equal (same struct, same pretty printer)
        assert_eq!(toml_str, toml_str2);
    }

    #[test]
    fn empty_geofence_defaults() {
        let toml_str = r#"
[mission]
id = "01J8K2EXAMPLE00000000000002"
name = "no-fence"
version = 1
created_at = "2026-09-09T10:00:00Z"
updated_at = "2026-09-09T10:00:00Z"

[[waypoints]]
seq = 0
frame = 3
command = 16
x = 47.397770
y = 8.545580
z = 12.0
"#;
        let m = MissionFile::from_toml_str(toml_str).unwrap();
        assert_eq!(m.waypoints.len(), 1);
        assert_eq!(m.geofence.ceiling_m, 0.0); // default
        assert_eq!(m.geofence.inclusion.len(), 0); // empty = no fence
        assert_eq!(m.rally.len(), 0);
        assert_eq!(m.mission.vehicle_type, "quad"); // default
    }

    #[test]
    fn hand_editable_toml_parses() {
        // An operator hand-edits a TOML file and uploads it.
        let toml_str = r#"
[mission]
id = "01J8K2HAND"
name = "hand-edited"
version = 1
created_at = "2026-09-09T10:00:00Z"
updated_at = "2026-09-09T10:05:00Z"
vehicle_type = "fixed"
px4_version = "v1.16.2"

# This is a comment — TOML preserves it on disk.
[[waypoints]]
seq = 0
frame = 3
command = 16
x = 47.397770
y = 8.545580
z = 50.0  # high altitude for fixed-wing cruise

[[waypoints]]
seq = 1
frame = 3
command = 21  # MAV_CMD_NAV_LAND
x = 47.397780
y = 8.545590
z = 0.0

[geofence]
ceiling_m = 150
floor_m = 0
inclusion = [[47.3970, 8.5450], [47.3980, 8.5450], [47.3980, 8.5460], [47.3970, 8.5460]]
"#;
        let m = MissionFile::from_toml_str(toml_str).unwrap();
        assert_eq!(m.mission.vehicle_type, "fixed");
        assert_eq!(m.waypoints.len(), 2);
        assert_eq!(m.waypoints[1].command, 21); // land
        assert_eq!(m.geofence.ceiling_m, 150.0);
    }
}
