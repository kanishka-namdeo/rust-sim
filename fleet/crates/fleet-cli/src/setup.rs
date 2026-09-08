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
/// Params serialize as an array (id, typed value, raw value, type,
/// index) — stable order, the console groups/prefixes client-side. The
/// `value` field is the TYPED value (INT32 params as integers, the
/// numbers QGC shows); `raw` keeps the wire f32 for diagnostics.
pub fn build_params_json(store: &ParamStore) -> serde_json::Value {
    let params: Vec<serde_json::Value> = store
        .params
        .iter()
        .map(|(id, e)| {
            let typed = e.typed_value();
            let kind = match typed {
                ParamVal::Int32(_) => "int32",
                ParamVal::Real32(_) => "real32",
                ParamVal::Other(_) => "other",
            };
            serde_json::json!({
                "id": id,
                "value": val_json(typed),
                "raw": e.value,
                "type": e.param_type,
                "kind": kind,
                "index": e.param_index,
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
        let j = build_params_json(&st);
        let cells = j["params"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == "BAT_N_CELLS")
            .unwrap();
        assert_eq!(cells["value"], 4);
        assert_eq!(cells["kind"], "int32");
        // ParamVal round-trip: to_wire + wire_type reproduce the wire form
        let pv = ParamVal::Int32(1070);
        assert_eq!(pv.to_wire(), f32::from_bits(1070));
        assert_eq!(pv.wire_type(), 6);
        assert_eq!(ParamVal::Real32(3.1).wire_type(), 9);
        assert_eq!(ParamVal::Real32(3.1).to_wire(), 3.1);
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
