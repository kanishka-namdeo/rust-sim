//! ADR-0025: Parameter preset file format — TOML on disk, JSON on the wire.
//!
//! The on-disk form mirrors ADR-0019's mission-file convention (one
//! serde struct, two serializations): the catalog store writes TOML at
//! `$CATALOG_DIR/presets/vehicle_<i>/<name>.toml`; every REST endpoint
//! accepts and returns JSON. Both go through serde so a schema change
//! is one Rust edit.
//!
//! Schema (the on-disk TOML the operator can also hand-edit):
//!
//! ```toml
//! name = "aggressive-corners"
//! created_at = "2026-09-09T10:00:00Z"
//! vehicle_id = 0
//!
//! [[params]]
//! id = "MPC_XY_VEL_MAX"
//! value = 8.0
//! type = 9  # MAV_PARAM_TYPE_REAL32
//!
//! [[params]]
//! id = "MC_ROLLRATE_P"
//! value = 7.5
//! type = 9
//! ```
//!
//! `type` is the MAVLink param_type byte (6 = INT32, 9 = REAL32) — the
//! same byte PARAM_SET carries on the wire, so a preset can faithfully
//! reproduce a typed param write (NAV_DLL_ACT-style INT32s included).

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// The preset struct tree
// ---------------------------------------------------------------------------

/// Top-level param preset file (ADR-0025 §Decision). One per saved
/// preset, named by the operator ("aggressive-corners", "high-wind").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresetFile {
    /// Operator-chosen display name (matches the on-disk filename stem).
    pub name: String,
    /// RFC 3339 creation timestamp (the same format missions use).
    pub created_at: String,
    /// The vehicle index this preset was saved against (0..count-1).
    /// Informational — presets can be loaded onto any vehicle, but the
    /// UI surfaces this so the operator notices a vehicle-id mismatch.
    pub vehicle_id: u8,
    /// The saved params, in the order they were captured. Default empty
    /// so a freshly-created "empty" preset (operator clicked Save before
    /// checking any params) round-trips through TOML — the test
    /// `empty_preset_parses` pins this behaviour.
    #[serde(default)]
    pub params: Vec<PresetParam>,
}

/// One saved parameter entry. `value` is the typed value as a float
/// (INT32 params round-trip cleanly through f64 within ±2^53), `type`
/// is the MAVLink param_type byte (6 = INT32, 9 = REAL32) — the same
/// byte PARAM_SET carries on the wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresetParam {
    /// The 1..=16-char param id (PX4's own constraint).
    pub id: String,
    /// Typed value as f64 (INT32 params fit cleanly in f64's 53-bit mantissa).
    pub value: f64,
    /// MAVLink param_type byte (6 = INT32, 9 = REAL32). Renamed to `type`
    /// on the wire/TOML so the schema matches ADR-0025's spec.
    #[serde(rename = "type")]
    pub param_type: u8,
}

// ---------------------------------------------------------------------------
// Serialization helpers
// ---------------------------------------------------------------------------

impl PresetFile {
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

    fn sample_preset() -> PresetFile {
        PresetFile {
            name: "aggressive-corners".into(),
            created_at: "2026-09-09T10:00:00Z".into(),
            vehicle_id: 0,
            params: vec![
                PresetParam {
                    id: "MPC_XY_VEL_MAX".into(),
                    value: 8.0,
                    param_type: 9,
                },
                PresetParam {
                    id: "MC_ROLLRATE_P".into(),
                    value: 7.5,
                    param_type: 9,
                },
                PresetParam {
                    id: "BAT_N_CELLS".into(),
                    value: 6.0,
                    param_type: 6,
                },
            ],
        }
    }

    #[test]
    fn toml_roundtrip_preserves_fields() {
        let p = sample_preset();
        let toml_str = p.to_toml_pretty().unwrap();
        let p2 = PresetFile::from_toml_str(&toml_str).unwrap();
        assert_eq!(p2.name, p.name);
        assert_eq!(p2.created_at, p.created_at);
        assert_eq!(p2.vehicle_id, p.vehicle_id);
        assert_eq!(p2.params.len(), 3);
        assert_eq!(p2.params[0].id, "MPC_XY_VEL_MAX");
        assert_eq!(p2.params[0].value, 8.0);
        assert_eq!(p2.params[0].param_type, 9);
        assert_eq!(p2.params[2].id, "BAT_N_CELLS");
        assert_eq!(p2.params[2].param_type, 6); // INT32 preserved
    }

    #[test]
    fn toml_uses_type_key_not_param_type() {
        // ADR-0025 spec: the TOML key is `type`, not `param_type`.
        let p = sample_preset();
        let toml_str = p.to_toml_pretty().unwrap();
        assert!(
            toml_str.contains("type ="),
            "TOML must serialize the field as `type =`, got:\n{toml_str}"
        );
        assert!(
            !toml_str.contains("param_type"),
            "TOML must NOT contain `param_type`, got:\n{toml_str}"
        );
    }

    #[test]
    fn json_roundtrip_preserves_fields() {
        let p = sample_preset();
        let json_str = serde_json::to_string(&p).unwrap();
        let p2 = PresetFile::from_json_str(&json_str).unwrap();
        assert_eq!(p2.name, p.name);
        assert_eq!(p2.params.len(), 3);
        assert_eq!(p2.params[0].param_type, 9);
    }

    #[test]
    fn hand_editable_toml_parses() {
        // An operator hand-edits a preset TOML (the spec's exact format).
        let toml_str = r#"
name = "aggressive-corners"
created_at = "2026-09-09T10:00:00Z"
vehicle_id = 0

[[params]]
id = "MPC_XY_VEL_MAX"
value = 8.0
type = 9

[[params]]
id = "MC_ROLLRATE_P"
value = 7.5
type = 9
"#;
        let p = PresetFile::from_toml_str(toml_str).unwrap();
        assert_eq!(p.name, "aggressive-corners");
        assert_eq!(p.vehicle_id, 0);
        assert_eq!(p.params.len(), 2);
        assert_eq!(p.params[0].id, "MPC_XY_VEL_MAX");
        assert_eq!(p.params[0].value, 8.0);
        assert_eq!(p.params[0].param_type, 9);
        assert_eq!(p.params[1].id, "MC_ROLLRATE_P");
        assert_eq!(p.params[1].value, 7.5);
    }

    #[test]
    fn empty_preset_parses() {
        let toml_str = r#"
name = "empty"
created_at = "2026-09-09T10:00:00Z"
vehicle_id = 1
"#;
        let p = PresetFile::from_toml_str(toml_str).unwrap();
        assert_eq!(p.name, "empty");
        assert_eq!(p.vehicle_id, 1);
        assert!(p.params.is_empty());
    }
}
