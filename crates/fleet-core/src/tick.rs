//! Supervisor tick support: the fleet frame (REST/WS view) and the shared
//! action vocabulary. The tick loop itself is assembled in `fleet-cli`
//! (the composition root that owns links, mission runners and the
//! allocator — ADR-0006); everything mechanical (flags, FSM arcs, frames,
//! event log) lives here.

#![forbid(unsafe_code)]

use serde::Serialize;

use crate::events::Event;
use crate::fsm::FsmState;
use crate::health::HealthFlag;
use crate::state::VehicleState;

/// The 5 Hz WS frame / GET /api/fleet payload core (spec §3.4). `tasks` is
/// filled by the mission engine (fleet-mission types stay out of
/// fleet-core; the wire contract is the JSON schema in `schemas/`).
#[derive(Debug, Clone, Serialize)]
pub struct FleetFrame {
    pub t_ms: u64,
    pub phase: String,
    pub tick_count: u64,
    pub vehicles: Vec<VehicleView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tasks: Option<serde_json::Value>,
    pub events_tail: Vec<Event>,
}

#[derive(Debug, Clone, Serialize)]
pub struct VehicleView {
    pub index: u8,
    pub sysid: u8,
    pub compid: u16,
    pub fsm: String,
    pub mode: String,
    pub mode_word: u32,
    pub armed: bool,
    pub position_ned_m: [f32; 3],
    pub velocity_ned_ms: [f32; 3],
    pub attitude_q_wxyz: [f32; 4],
    pub battery_pct: i8,
    pub voltage_v: f32,
    pub home_ned_m: [f32; 3],
    pub home_set: bool,
    pub health: Vec<String>,
    pub stale: bool,
    pub last_heartbeat_ms: u64,
    pub last_msg_ms: u64,
    pub link: crate::state::LinkCounters,
    pub recv_rate_hz: f32,
    pub msg_counts: std::collections::BTreeMap<String, u64>,
    pub task_queue: Vec<String>,
    pub current_task: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_statustext: Option<String>,
}

impl VehicleView {
    pub fn from_state(
        fsm: FsmState,
        state: &VehicleState,
        flags: &[HealthFlag],
        task_queue: Vec<String>,
        current_task: Option<String>,
    ) -> Self {
        let mut msg_counts = std::collections::BTreeMap::new();
        for (id, n) in &state.msg_counts {
            msg_counts.insert(message_name(*id), *n);
        }
        VehicleView {
            index: state.index,
            sysid: state.sysid,
            compid: state.compid,
            fsm: fsm.name().to_string(),
            mode: fleet_modes::mode_name(state.mode_word),
            mode_word: state.mode_word,
            armed: state.armed,
            position_ned_m: state.position_ned_m,
            velocity_ned_ms: state.velocity_ned_ms,
            attitude_q_wxyz: state.attitude_q_wxyz,
            battery_pct: state.battery_pct,
            voltage_v: if state.voltage_v.is_nan() {
                -1.0
            } else {
                state.voltage_v
            },
            home_ned_m: state.home_ned_m,
            home_set: state.home_set,
            health: flags.iter().map(|f| f.name().to_string()).collect(),
            stale: flags.contains(&HealthFlag::LinkStale),
            last_heartbeat_ms: state.last_heartbeat_ms,
            last_msg_ms: state.last_msg_ms,
            link: state.link.clone(),
            recv_rate_hz: state.link.recv_rate_hz,
            msg_counts,
            task_queue,
            current_task,
            last_statustext: state.last_statustext.clone(),
        }
    }
}

pub fn message_name(msgid: u32) -> String {
    match msgid {
        0 => "HEARTBEAT".into(),
        1 => "SYS_STATUS".into(),
        4 => "PING".into(),
        30 => "ATTITUDE".into(),
        32 => "LOCAL_POSITION_NED".into(),
        33 => "GLOBAL_POSITION_INT".into(),
        76 => "COMMAND_LONG".into(),
        77 => "COMMAND_ACK".into(),
        84 => "SET_POSITION_TARGET_LOCAL_NED".into(),
        147 => "BATTERY_STATUS".into(),
        242 => "HOME_POSITION".into(),
        253 => "STATUSTEXT".into(),
        other => format!("MSGID_{other}"),
    }
}

/// Supervisor actions emitted by the safety policy engine (spec §8.1) and
/// consumed by the tick loop, which maps them to FSM arcs + MAVLink
/// commands + event log entries.
#[derive(Debug, Clone, PartialEq)]
pub enum SafetyAction {
    /// Policy 1: land everything now, abort the run.
    Estop,
    /// Policies 2/3/5/8: RTL now (hard) or after the current task (soft).
    Rtl {
        cause: crate::fsm::FsmCause,
        after_current_task: bool,
    },
    /// Policies 2 (deep breach) / 3 (10 s escalation) / 4: LAND.
    Land {
        cause: crate::fsm::FsmCause,
    },
    /// Policy 7: altitude-divergence setpoint override.
    AltitudeDiverge { dz_m: f32 },
    /// Policy 6: flag-only.
    Flag(crate::health::HealthFlag),
    None,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vehicle_view_serializes_spec_fields() {
        let mut s = VehicleState::new(0);
        s.sysid = 1;
        s.compid = 1;
        s.mode_word = fleet_modes::MODE_WORD_AUTO_RTL;
        s.position_ned_m = [1.0, 2.0, -3.0];
        s.home_set = true;
        s.msg_counts.insert(0, 10);
        s.msg_counts.insert(30, 200);
        let v = VehicleView::from_state(FsmState::Active, &s, &[HealthFlag::LinkStale], vec!["t1".into()], Some("t1".into()));
        let json = serde_json::to_value(&v).unwrap();
        assert_eq!(json["fsm"], "ACTIVE");
        assert_eq!(json["mode"], "AUTO.RTL");
        assert_eq!(json["sysid"], 1);
        assert_eq!(json["health"][0], "LINK_STALE");
        assert_eq!(json["msg_counts"]["HEARTBEAT"], 10);
        assert_eq!(json["msg_counts"]["ATTITUDE"], 200);
        assert_eq!(json["battery_pct"], -1);
    }

    #[test]
    fn fleet_frame_roundtrip() {
        let f = FleetFrame {
            t_ms: 5,
            phase: "RUNNING".into(),
            tick_count: 50,
            vehicles: vec![],
            tasks: Some(serde_json::json!([{"id": "wp_n"}])),
            events_tail: vec![],
        };
        let j = serde_json::to_string(&f).unwrap();
        assert!(j.contains("\"phase\":\"RUNNING\""));
        assert!(j.contains("wp_n"));
    }
}
