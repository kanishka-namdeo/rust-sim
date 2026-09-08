//! Vehicle-setup control plane (ADR-0016): the QGroundControl /
//! Mission-Planner-style vehicle configuration view, served from the
//! manager's live per-vehicle state + each link's parameter store.
//!
//! Sections (the QGC setup rail, mapped to this stack):
//! - *Summary* — airframe, flight mode, battery, parameter-download state
//! - *Airframe* — SYS_AUTOSTART resolution against the ROMFS-derived catalog
//! - *Sensors/Calibration* — CAL_*_ID params (nonzero = calibrated)
//! - *Power* — BAT_* params
//! - *Safety* — NAV_RCL_ACT / NAV_DLL_ACT / COM_LOW_BAT_ACT / GF_* params
//! - *Flight Modes* — current mode + the switchable set
//!
//! Everything param-derived is read from the link's PARAM_VALUE cache
//! (populated by PARAM_REQUEST_LIST / write echoes), so the view is always
//! the live vehicle's own configuration, never a client-side copy.

#![forbid(unsafe_code)]

use crate::airframes::{AirframeCategory, AIRFRAMES};
use crate::state::AppState;
use fleet_mavlink::{ParamStore, ParamVal};

/// Resolve the airframe a vehicle is actually running: SYS_AUTOSTART from
/// the param store against the catalog. SYS_AUTOSTART is an INT32 param —
/// the typed store decodes the wire bit-cast (a naive f32 cast reads the
/// integer 10015 as the denormal 1.4e-41, which resolves to nothing).
/// Falls back to unknown with the raw id when the catalog has no match
/// (e.g. a custom ROMFS).
pub fn resolve_airframe(store: Option<&ParamStore>) -> serde_json::Value {
    let id = store.and_then(|st| st.get_i32("SYS_AUTOSTART"));
    let entry = id.and_then(|i| AIRFRAMES.iter().find(|a| a.id as i64 == i as i64));
    serde_json::json!({
        "sys_autostart": id,
        "name": entry.map(|a| a.name).unwrap_or("Unknown airframe"),
        "frame_type": entry.map(|a| a.frame_type).unwrap_or(""),
        "category": entry.map(|a| a.category.label()).unwrap_or("Other"),
        // This stack's HIL dynamics is a 6-DOF quadrotor (sim/): multirotor
        // airframes are physics-compatible; everything else configures fine
        // at the parameter level but will not fly correctly against the
        // quad dynamics. Honest flag, per the repo's LIVE/SIMULATED rule.
        "dynamics_compatible": entry.map(|a| a.category == AirframeCategory::Copter
            || a.category == AirframeCategory::Simulator).unwrap_or(false),
    })
}

/// Calibration status from the CAL_*_ID params (QGC's rule: a nonzero
/// device id means that sensor is calibrated on that instance). The
/// CAL ids are INT32 params — read typed.
fn cal_flag(store: &ParamStore, id: &str) -> Option<bool> {
    store.get_i32(id).map(|v| v != 0)
}

/// ParamVal -> JSON number (INT32 params surface as integers — the
/// values QGC's editors show).
pub(crate) fn val_json(v: ParamVal) -> serde_json::Value {
    match v {
        ParamVal::Int32(i) => serde_json::json!(i),
        ParamVal::Real32(f) | ParamVal::Other(f) => serde_json::json!(f),
    }
}

/// A setup-relevant param as its typed value (INT32 params surface as
/// integers — the values QGC's editors show).
fn opt_val(store: &ParamStore, id: &str) -> Option<serde_json::Value> {
    store.typed(id).map(val_json)
}

/// Build `GET /api/vehicles/{i}/setup`.
pub fn build_setup_summary(s: &AppState, index: u8) -> Option<serde_json::Value> {
    if index >= s.count {
        return None;
    }
    let snap = s.registry.snapshot(index)?;
    let link = s.link(index);
    let store = link.as_ref().map(|h| h.param_store());
    let store = store.as_ref();

    let params = store.map(|st| {
        serde_json::json!({
            "total": st.total,
            "received": st.params.len(),
            "state": format!("{:?}", st.state),
        })
    });

    let airframe = resolve_airframe(store);

    let calibration = store.map(|st| {
        serde_json::json!({
            "accel": cal_flag(st, "CAL_ACC0_ID"),
            "gyro": cal_flag(st, "CAL_GYRO0_ID"),
            "mag0": cal_flag(st, "CAL_MAG0_ID"),
            "mag1": cal_flag(st, "CAL_MAG1_ID"),
            "mag2": cal_flag(st, "CAL_MAG2_ID"),
            "level_horizon": cal_flag(st, "CAL_LEVEL"),
        })
    });

    // PX4 v1.16 battery params carry the BAT1_ instance prefix (lib/battery
    // module.yaml); the warning thresholds are unprefixed (commander).
    let power = store.map(|st| {
        serde_json::json!({
            "BAT1_N_CELLS": opt_val(st, "BAT1_N_CELLS"),
            "BAT1_V_EMPTY": opt_val(st, "BAT1_V_EMPTY"),
            "BAT1_V_CHARGED": opt_val(st, "BAT1_V_CHARGED"),
            "BAT_LOW_THR": opt_val(st, "BAT_LOW_THR"),
            "BAT_CRIT_THR": opt_val(st, "BAT_CRIT_THR"),
            "BAT_EMERGEN_THR": opt_val(st, "BAT_EMERGEN_THR"),
            "BAT1_R_INTERNAL": opt_val(st, "BAT1_R_INTERNAL"),
        })
    });

    let safety = store.map(|st| {
        serde_json::json!({
            "NAV_RCL_ACT": opt_val(st, "NAV_RCL_ACT"),
            "NAV_DLL_ACT": opt_val(st, "NAV_DLL_ACT"),
            "COM_LOW_BAT_ACT": opt_val(st, "COM_LOW_BAT_ACT"),
            "COM_OBL_RC_ACT": opt_val(st, "COM_OBL_RC_ACT"),
            "GF_ACTION": opt_val(st, "GF_ACTION"),
            "GF_MAX_HOR_DIST": opt_val(st, "GF_MAX_HOR_DIST"),
            "GF_MAX_VER_DIST": opt_val(st, "GF_MAX_VER_DIST"),
            "RTL_RETURN_ALT": opt_val(st, "RTL_RETURN_ALT"),
        })
    });

    Some(serde_json::json!({
        "index": index,
        "sysid": snap.sysid,
        "compid": snap.compid,
        "fsm": format!("{:?}", s.registry.fsm(index).unwrap_or(fleet_core::fsm::FsmState::Init)),
        "mode": fleet_modes::mode_name(snap.mode_word),
        "mode_word": snap.mode_word,
        "armed": snap.armed,
        "battery_pct": snap.battery_pct,
        "voltage_v": snap.voltage_v,
        // an airframe-apply restart is queued but not yet actioned
        "restart_pending": s.restart_pending(index),
        "autopilot": {
            "type": "PX4",
            // The manager spawns this pinned build (fleet-simctl's
            // px4_binary path); reported as manager-known metadata.
            "version": "v1.16.2 SITL",
        },
        "airframe": airframe,
        "params": params,
        "calibration": calibration,
        "power": power,
        "safety": safety,
    }))
}

/// Build `GET /api/vehicles/{i}/params`: the full cache with progress.
/// Params serialize as an array (id, typed value, raw value, type, kind,
/// index, group, default, is_changed) — stable order, the console
/// groups/prefixes client-side. The `value` field is the TYPED value
/// (INT32 params as integers, the numbers QGC shows); `raw` keeps the
/// wire f32 for diagnostics. The `group`/`default`/`is_changed` triple
/// drives the diff-against-defaults view (GCS_SPEC.md §5.3, AC-5.3.4):
///   - `group` — the param id's prefix before the first `_` (PX4's own
///     grouping convention: `MPC_XY_VEL_MAX` → `MPC`). Params with no
///     underscore get group `"Other"`.
///   - `default` — the compiled-in PX4 default (`null` for params not
///     in the [`known_default`] table).
///   - `is_changed` — true iff a known default exists AND the current
///     typed value differs from it (INT32: exact; REAL32: |Δ| ≥ 0.001).
///     Unknown-default params surface `is_changed=false` (we don't claim
///     a diff we can't compute).
///
/// `filter` narrows the `params` array (GCS_SPEC.md §5.3 `?search=` and
/// `?group=`). The summary fields (`total`/`received`/`state`/…
/// `requested_ms`/`last_value_ms`) always reflect the FULL store —
/// QGC's own search does the same: the progress bar tracks the whole
/// download even when the list is filtered.
pub fn build_params_json(store: &ParamStore, filter: &ParamsFilter) -> serde_json::Value {
    let params: Vec<serde_json::Value> = store
        .params
        .iter()
        .filter(|(id, _)| filter.matches(id))
        .map(|(id, e)| {
            let typed = e.typed_value();
            let kind = match typed {
                ParamVal::Int32(_) => "int32",
                ParamVal::Real32(_) => "real32",
                ParamVal::Other(_) => "other",
            };
            let default = known_default(id);
            let changed = is_changed(typed, default);
            serde_json::json!({
                "id": id,
                "value": val_json(typed),
                "raw": e.value,
                "type": e.param_type,
                "kind": kind,
                "index": e.param_index,
                "group": param_group(id),
                "default": default,
                "is_changed": changed,
            })
        })
        .collect();
    serde_json::json!({
        "total": store.total,
        "received": store.params.len(),
        "state": format!("{:?}", store.state),
        "requested_ms": store.requested_ms,
        "last_value_ms": store.last_value_ms,
        "params": params,
    })
}

/// A param's group: the id's prefix before the first `_` (PX4's own
/// grouping convention — `MPC_XY_VEL_MAX` → `MPC`, `MC_ROLLRATE_P` →
/// `MC`). Params with no underscore (rare: `SYS_AUTOSTART`'s prefix is
/// `SYS`, but a stray `MAV_PARAM_INDEX`-style id with no separator)
/// fall back to `"Other"` so the group filter always has a bucket.
pub fn param_group(id: &str) -> String {
    match id.find('_') {
        Some(0) | None => "Other".to_string(),
        Some(i) => id[..i].to_string(),
    }
}

/// Compiled-in default value for the ~20 most common PX4 params on the
/// Iris quad SITL airframe (PX4 v1.16.2). Powers the diff-against-defaults
/// view (GCS_SPEC.md §5.3 AC-5.3.4): every param the console lists is
/// cross-referenced against this table to compute `is_changed`. Params
/// not in the table return `None` → `default: null, is_changed: false`
/// (we don't claim a diff we can't compute).
pub fn known_default(id: &str) -> Option<f64> {
    match id {
        "MPC_XY_VEL_MAX" => Some(12.0),
        "MPC_Z_VEL_MAX_UP" => Some(3.0),
        "MPC_Z_VEL_MAX_DN" => Some(1.0),
        "MPC_XY_CRUISE" => Some(5.0),
        "MPC_CRUISE_90" => Some(5.0),
        "MPC_TKO_SPEED" => Some(1.0),
        "MC_ROLLRATE_P" => Some(6.5),
        "MC_ROLLRATE_I" => Some(0.0),
        "MC_ROLLRATE_D" => Some(0.003),
        "MC_PITCHRATE_P" => Some(6.5),
        "MC_PITCHRATE_I" => Some(0.0),
        "MC_PITCHRATE_D" => Some(0.003),
        "MC_YAWRATE_P" => Some(200.0),
        "MC_YAWRATE_I" => Some(0.0),
        "MC_YAWRATE_D" => Some(0.0),
        "FW_AIRSPD_TRIM" => Some(15.0),
        "BAT_N_CELLS" => Some(4.0),
        "BAT_V_EMPTY" => Some(3.4),
        "BAT_V_CHARGED" => Some(4.1),
        "NAV_DLL_ACT" => Some(0.0),
        _ => None,
    }
}

/// Whether the live value differs from the compiled-in default. INT32
/// params compare exactly on their integer values (after the wire
/// bit-cast); REAL32 params compare with a 0.001 tolerance — PX4's float
/// params are authored to 3 decimal places, so the tolerance matches
/// the editor's own rounding. `default = None` (param not in the table)
/// → `false` (no diff to claim).
pub fn is_changed(typed: ParamVal, default: Option<f64>) -> bool {
    let d = match default {
        Some(d) => d,
        None => return false,
    };
    match typed {
        ParamVal::Int32(v) => (v as f64) != d,
        ParamVal::Real32(v) | ParamVal::Other(v) => ((v as f64) - d).abs() >= 0.001,
    }
}

/// Query filter for `GET /api/vehicles/{i}/params?search=&group=`
/// (GCS_SPEC.md §5.3). Both fields optional — `None` means no filter.
#[derive(Debug, Clone, Default)]
pub struct ParamsFilter {
    /// Case-insensitive substring match on the param id (QGC's search box).
    pub search: Option<String>,
    /// Exact match on the group prefix (e.g. `MPC`).
    pub group: Option<String>,
}

impl ParamsFilter {
    /// Returns true if the param passes both the search and group filters.
    /// Empty / `None` filters pass everything.
    fn matches(&self, id: &str) -> bool {
        if let Some(g) = &self.group {
            if param_group(id) != *g {
                return false;
            }
        }
        if let Some(q) = &self.search {
            if !id.to_ascii_lowercase().contains(&q.to_ascii_lowercase()) {
                return false;
            }
        }
        true
    }
}

/// The QGC-style mode list this stack can switch to (fleet-modes' verified
/// set): name -> mode word, for the command endpoint and the console UI.
pub fn switchable_modes() -> Vec<(String, u32)> {
    use fleet_modes::{mode_word, AutoSubMode, MainMode};
    vec![
        ("MANUAL".into(), mode_word(MainMode::Manual, None)),
        ("ALTCTL".into(), mode_word(MainMode::Altctl, None)),
        ("POSCTL".into(), mode_word(MainMode::Posctl, None)),
        ("STABILIZED".into(), mode_word(MainMode::Stabilized, None)),
        ("ACRO".into(), mode_word(MainMode::Acro, None)),
        ("AUTO.LOITER".into(), mode_word(MainMode::Auto, Some(AutoSubMode::Loiter))),
        ("AUTO.RTL".into(), mode_word(MainMode::Auto, Some(AutoSubMode::Rtl))),
        ("AUTO.LAND".into(), mode_word(MainMode::Auto, Some(AutoSubMode::Land))),
        ("AUTO.MISSION".into(), mode_word(MainMode::Auto, Some(AutoSubMode::Mission))),
        ("OFFBOARD".into(), mode_word(MainMode::Offboard, None)),
    ]
}

/// Resolve a mode name to its mode word (None for unknown names).
pub fn mode_word_by_name(name: &str) -> Option<u32> {
    switchable_modes().into_iter().find(|(n, _)| n == name).map(|(_, w)| w)
}

/// The airframe catalog grouped for the console's picker.
pub fn airframes_json() -> serde_json::Value {
    let mut groups: Vec<serde_json::Value> = Vec::new();
    for cat in [
        AirframeCategory::Copter,
        AirframeCategory::Plane,
        AirframeCategory::Vtol,
        AirframeCategory::Rover,
        AirframeCategory::Boat,
        AirframeCategory::Underwater,
        AirframeCategory::Other,
        AirframeCategory::Simulator,
    ] {
        let entries: Vec<serde_json::Value> = AIRFRAMES
            .iter()
            .filter(|a| a.category == cat)
            .map(|a| {
                serde_json::json!({
                    "id": a.id,
                    "name": a.name,
                    "frame_type": a.frame_type,
                    "sim_model": a.sim_model,
                })
            })
            .collect();
        if !entries.is_empty() {
            groups.push(serde_json::json!({
                "category": cat.label(),
                "airframes": entries,
            }));
        }
    }
    serde_json::json!({ "groups": groups, "count": AIRFRAMES.len() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_mavlink::ParamEntry;

    /// A store with SYS_AUTOSTART as PX4 really sends it: INT32 with the
    /// value bit-cast into the f32 field (wire bits of 10015 → the
    /// denormal 1.4034e-41).
    fn px4_store(sys_autostart: i32) -> ParamStore {
        let mut st = ParamStore::default();
        st.ingest(
            "SYS_AUTOSTART",
            f32::from_bits(sys_autostart as u32),
            6,
            1,
            0,
            1,
        );
        st
    }

    #[test]
    fn resolve_airframe_known_and_unknown() {
        // PX4-faithful: INT32 bit-cast (10015 reads 1.4e-41 as raw f32)
        let st = px4_store(10015);
        let a = resolve_airframe(Some(&st));
        assert_eq!(a["sys_autostart"], 10015);
        assert_eq!(a["name"], "3DR Iris Quadrotor SITL");
        assert_eq!(a["category"], "Simulator (SITL models)");
        assert_eq!(a["dynamics_compatible"], true);
        let boat = resolve_airframe(Some(&px4_store(1070)));
        assert_eq!(boat["name"], "Boat");
        assert_eq!(boat["category"], "Boat (USV)");
        assert_eq!(boat["dynamics_compatible"], false);
        let unknown = resolve_airframe(Some(&px4_store(99999)));
        assert_eq!(unknown["name"], "Unknown airframe");
        let none = resolve_airframe(None);
        assert_eq!(none["sys_autostart"], serde_json::Value::Null);
    }

    #[test]
    fn typed_int_params_decode_from_wire_bits() {
        // BAT_N_CELLS = 4 as PX4 wires it (INT32, bit pattern)
        let mut st = px4_store(4001);
        st.ingest("BAT_N_CELLS", f32::from_bits(4u32), 6, 2, 1, 2);
        st.ingest("BAT_V_EMPTY", 3.1, 9, 3, 2, 3);
        assert_eq!(st.get_i32("BAT_N_CELLS"), Some(4));
        assert_eq!(st.get("BAT_V_EMPTY"), Some(3.1));
        // typed JSON: int params surface as integers
        let j = build_params_json(&st, &ParamsFilter::default());
        let cells = j["params"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == "BAT_N_CELLS")
            .unwrap();
        assert_eq!(cells["value"], 4);
        assert_eq!(cells["kind"], "int32");
        // M4 fields: group derived from id prefix, default from the table,
        // is_changed false because value 4 == default 4.0.
        assert_eq!(cells["group"], "BAT");
        assert_eq!(cells["default"], 4.0);
        assert_eq!(cells["is_changed"], false);
        // BAT_V_EMPTY (3.1) vs default (3.4) → is_changed=true
        let v_empty = j["params"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == "BAT_V_EMPTY")
            .unwrap();
        assert_eq!(v_empty["default"], 3.4);
        assert_eq!(v_empty["is_changed"], true);
        // ParamVal round-trip: to_wire + wire_type reproduce the wire form
        let pv = ParamVal::Int32(1070);
        assert_eq!(pv.to_wire(), f32::from_bits(1070));
        assert_eq!(pv.wire_type(), 6);
        assert_eq!(ParamVal::Real32(3.1).wire_type(), 9);
        assert_eq!(ParamVal::Real32(3.1).to_wire(), 3.1);
    }

    #[test]
    fn known_default_returns_known_params() {
        // All 20 entries in the table return Some; everything else None.
        assert_eq!(known_default("MPC_XY_VEL_MAX"), Some(12.0));
        assert_eq!(known_default("MPC_Z_VEL_MAX_UP"), Some(3.0));
        assert_eq!(known_default("MPC_Z_VEL_MAX_DN"), Some(1.0));
        assert_eq!(known_default("MPC_XY_CRUISE"), Some(5.0));
        assert_eq!(known_default("MPC_CRUISE_90"), Some(5.0));
        assert_eq!(known_default("MPC_TKO_SPEED"), Some(1.0));
        assert_eq!(known_default("MC_ROLLRATE_P"), Some(6.5));
        assert_eq!(known_default("MC_ROLLRATE_I"), Some(0.0));
        assert_eq!(known_default("MC_ROLLRATE_D"), Some(0.003));
        assert_eq!(known_default("MC_PITCHRATE_P"), Some(6.5));
        assert_eq!(known_default("MC_PITCHRATE_I"), Some(0.0));
        assert_eq!(known_default("MC_PITCHRATE_D"), Some(0.003));
        assert_eq!(known_default("MC_YAWRATE_P"), Some(200.0));
        assert_eq!(known_default("MC_YAWRATE_I"), Some(0.0));
        assert_eq!(known_default("MC_YAWRATE_D"), Some(0.0));
        assert_eq!(known_default("FW_AIRSPD_TRIM"), Some(15.0));
        assert_eq!(known_default("BAT_N_CELLS"), Some(4.0));
        assert_eq!(known_default("BAT_V_EMPTY"), Some(3.4));
        assert_eq!(known_default("BAT_V_CHARGED"), Some(4.1));
        assert_eq!(known_default("NAV_DLL_ACT"), Some(0.0));
        // Unknown params: None
        assert_eq!(known_default("UNKNOWN_PARAM"), None);
        assert_eq!(known_default("MPC_XY_VEL_MAX_X"), None);
        assert_eq!(known_default(""), None);
    }

    #[test]
    fn param_group_extracted_from_prefix() {
        assert_eq!(param_group("MPC_XY_VEL_MAX"), "MPC");
        assert_eq!(param_group("MC_ROLLRATE_P"), "MC");
        assert_eq!(param_group("BAT_N_CELLS"), "BAT");
        assert_eq!(param_group("FW_AIRSPD_TRIM"), "FW");
        assert_eq!(param_group("NAV_DLL_ACT"), "NAV");
        // No underscore → "Other"
        assert_eq!(param_group("NOGROUP"), "Other");
        assert_eq!(param_group(""), "Other");
        // Leading underscore (edge case) → "Other" (no prefix before _)
        assert_eq!(param_group("_LEADING"), "Other");
    }

    #[test]
    fn is_changed_when_value_differs_from_default() {
        // REAL32: value 8.0 vs default 12.0 → changed
        assert!(is_changed(ParamVal::Real32(8.0), known_default("MPC_XY_VEL_MAX")));
        // INT32: value 6 vs default 4 → changed
        assert!(is_changed(ParamVal::Int32(6), known_default("BAT_N_CELLS")));
        // No known default → false (we don't claim a diff we can't compute)
        assert!(!is_changed(ParamVal::Real32(99.0), known_default("UNKNOWN_PARAM")));
    }

    #[test]
    fn is_changed_false_when_value_equals_default() {
        // REAL32: value 12.0 vs default 12.0 → not changed
        assert!(!is_changed(ParamVal::Real32(12.0), known_default("MPC_XY_VEL_MAX")));
        // INT32: value 4 vs default 4 → not changed
        assert!(!is_changed(ParamVal::Int32(4), known_default("BAT_N_CELLS")));
        // REAL32 within tolerance (|Δ| = 0.0005 < 0.001) → not changed
        assert!(!is_changed(ParamVal::Real32(12.0005), known_default("MPC_XY_VEL_MAX")));
        // REAL32 exactly at tolerance boundary (|Δ| = 0.001) → changed
        assert!(is_changed(ParamVal::Real32(12.001), known_default("MPC_XY_VEL_MAX")));
        // No known default → not changed
        assert!(!is_changed(ParamVal::Real32(99.0), known_default("UNKNOWN_PARAM")));
    }

    /// A store with the ROLLRATE + MPC + BAT params populated for filter
    /// tests. The param ids are chosen so the search/group filters below
    /// produce unambiguous counts (3 ROLLRATE, 2 MPC, 1 BAT).
    fn filter_test_store() -> ParamStore {
        let mut st = ParamStore::default();
        st.ingest("MPC_XY_VEL_MAX", 12.0, 9, 6, 0, 1);
        st.ingest("MPC_Z_VEL_MAX_UP", 3.0, 9, 6, 1, 2);
        st.ingest("MC_ROLLRATE_P", 6.5, 9, 6, 2, 3);
        st.ingest("MC_ROLLRATE_I", 0.0, 9, 6, 3, 4);
        st.ingest("MC_ROLLRATE_D", 0.003, 9, 6, 4, 5);
        st.ingest("BAT_N_CELLS", f32::from_bits(4u32), 6, 6, 5, 6);
        st
    }

    #[test]
    fn search_filters_by_substring() {
        let st = filter_test_store();
        let filter = ParamsFilter {
            search: Some("ROLLRATE".into()),
            group: None,
        };
        let j = build_params_json(&st, &filter);
        let ids: Vec<String> = j["params"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["id"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(ids.len(), 3);
        assert!(ids.contains(&"MC_ROLLRATE_P".to_string()));
        assert!(ids.contains(&"MC_ROLLRATE_I".to_string()));
        assert!(ids.contains(&"MC_ROLLRATE_D".to_string()));
    }

    #[test]
    fn search_is_case_insensitive() {
        let st = filter_test_store();
        let filter = ParamsFilter {
            search: Some("rollrate".into()),
            group: None,
        };
        let j = build_params_json(&st, &filter);
        let arr = j["params"].as_array().unwrap();
        assert_eq!(arr.len(), 3, "lowercase query must match uppercase ids");
    }

    #[test]
    fn group_filter_returns_only_matching() {
        let st = filter_test_store();
        // group=MPC → 2 params (MPC_XY_VEL_MAX, MPC_Z_VEL_MAX_UP)
        let filter = ParamsFilter {
            search: None,
            group: Some("MPC".into()),
        };
        let j = build_params_json(&st, &filter);
        let arr = j["params"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        for p in arr {
            assert_eq!(p["group"], "MPC");
            assert_eq!(param_group(p["id"].as_str().unwrap()), "MPC");
        }
        // group=MC → 3 params (ROLLRATE_P/I/D)
        let filter_mc = ParamsFilter {
            search: None,
            group: Some("MC".into()),
        };
        let j2 = build_params_json(&st, &filter_mc);
        assert_eq!(j2["params"].as_array().unwrap().len(), 3);
        // group=BAT → 1 param (BAT_N_CELLS)
        let filter_bat = ParamsFilter {
            search: None,
            group: Some("BAT".into()),
        };
        let j3 = build_params_json(&st, &filter_bat);
        assert_eq!(j3["params"].as_array().unwrap().len(), 1);
        // nonexistent group → 0
        let filter_none = ParamsFilter {
            search: None,
            group: Some("NOPE".into()),
        };
        let j4 = build_params_json(&st, &filter_none);
        assert_eq!(j4["params"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn search_and_group_can_combine() {
        let st = filter_test_store();
        // group=MC + search=VEL → 0 (MC group has no VEL params)
        let filter = ParamsFilter {
            search: Some("VEL".into()),
            group: Some("MC".into()),
        };
        let j = build_params_json(&st, &filter);
        assert_eq!(j["params"].as_array().unwrap().len(), 0);
        // group=MPC + search=VEL → 2 (MPC_XY_VEL_MAX, MPC_Z_VEL_MAX_UP)
        let filter = ParamsFilter {
            search: Some("VEL".into()),
            group: Some("MPC".into()),
        };
        let j = build_params_json(&st, &filter);
        assert_eq!(j["params"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn mode_words_match_fleet_modes_constants() {
        use fleet_modes::{MODE_WORD_AUTO_LAND, MODE_WORD_AUTO_RTL, MODE_WORD_OFFBOARD, MODE_WORD_POSCTL};
        assert_eq!(mode_word_by_name("POSCTL").unwrap(), MODE_WORD_POSCTL);
        assert_eq!(mode_word_by_name("AUTO.RTL").unwrap(), MODE_WORD_AUTO_RTL);
        assert_eq!(mode_word_by_name("AUTO.LAND").unwrap(), MODE_WORD_AUTO_LAND);
        assert_eq!(mode_word_by_name("OFFBOARD").unwrap(), MODE_WORD_OFFBOARD);
        assert!(mode_word_by_name("NOT_A_MODE").is_none());
    }

    #[test]
    fn airframes_json_has_boat_and_quad() {
        let j = airframes_json();
        let flat = j["groups"].as_array().unwrap();
        assert!(flat.len() >= 6);
        let names: Vec<String> = flat
            .iter()
            .map(|g| g["category"].as_str().unwrap().to_string())
            .collect();
        assert!(names.contains(&"Boat (USV)".to_string()));
        // the catalog is sorted with multirotor first (QGC browser order)
        assert_eq!(names[0], "Multirotor (UAV)");
    }
}
