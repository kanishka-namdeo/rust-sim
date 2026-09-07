//! Run report assembly (spec §9.2): the report is built at run end regardless
//! of outcome, from the registry + the event log (the log is the spine —
//! traces are parsed out of it, not from a parallel bookkeeping structure).

#![forbid(unsafe_code)]

use std::path::Path;
use std::sync::Arc;

use fleet_core::events::{Event, EventKind, EventLog};
use fleet_core::registry::Registry;
use fleet_mission::report::{
    evaluate_criteria, Criterion, FsmArc, RunReport, VehicleReport,
};
use fleet_mission::scenario::Scenario;

use crate::state::AppState;

/// Parse one FSM-transition event detail ("FROM->TO cause=CAUSE") into an arc.
fn parse_fsm_detail(detail: &str) -> Option<(String, String, String)> {
    let (states, cause) = detail.split_once(" cause=")?;
    let (from, to) = states.split_once("->")?;
    Some((from.to_string(), to.to_string(), cause.to_string()))
}

/// Ordered FSM trace for one vehicle, reconstructed from the event log.
pub fn fsm_trace(log: &EventLog, vehicle: u8) -> Vec<FsmArc> {
    log.all()
        .into_iter()
        .filter(|e| e.vehicle == Some(vehicle) && e.kind == EventKind::FsmTransition)
        .filter_map(|e: Event| {
            let (from, to, cause) = parse_fsm_detail(&e.detail)?;
            Some(FsmArc { from, to, cause, t_ms: e.t_ms })
        })
        .collect()
}

/// Supervisor actions (event-log detail strings) for one vehicle (or the
/// fleet-wide `None` ones when vehicle is None).
fn supervisor_actions(log: &EventLog, vehicle: Option<u8>) -> Vec<String> {
    log.all()
        .into_iter()
        .filter(|e| e.kind == EventKind::SupervisorAction && e.vehicle == vehicle)
        .map(|e| format!("[{}ms] {}", e.t_ms, e.detail))
        .collect()
}

fn geofence_breaches(trace: &[FsmArc]) -> u32 {
    trace
        .iter()
        .filter(|a| a.cause == "geofence_breach" || a.cause == "geofence_breach_land")
        .count() as u32
}

/// Build the run report from live state + the event log.
pub fn build_report(
    state: &AppState,
    registry: &Arc<Registry>,
    log: &Arc<EventLog>,
    scenario_path: &str,
    phase: &str,
    aborted: bool,
    duration_ms: u64,
) -> RunReport {
    let tasks = state.tasks_snapshot();
    let mut vehicles = Vec::new();
    for i in 0..state.count {
        let snap = registry.snapshot(i).unwrap_or_default();
        let fsm = registry
            .fsm(i)
            .map(|f| f.name().to_string())
            .unwrap_or_else(|| "INIT".into());
        let trace = fsm_trace(log, i);
        let breaches = geofence_breaches(&trace);
        let tasks_completed: Vec<_> = tasks
            .iter()
            .filter(|t| t.assigned == Some(i) && t.state == "complete")
            .map(|t| fleet_mission::report::TaskDone {
                id: t.id.clone(),
                hover_observed: t.hover_observed.unwrap_or(false),
            })
            .collect();
        let tasks_remaining: Vec<_> = tasks
            .iter()
            .filter(|t| t.assigned == Some(i) && t.state != "complete")
            .map(|t| t.id.clone())
            .collect();
        vehicles.push(VehicleReport {
            index: i,
            sysid: snap.sysid,
            final_fsm: fsm,
            final_mode: fleet_modes::mode_name(snap.mode_word),
            armed: snap.armed,
            battery_pct: snap.battery_pct,
            fsm_trace: trace,
            tasks_completed,
            tasks_remaining,
            geofence_breaches: breaches,
            supervisor_actions: supervisor_actions(log, Some(i)),
        });
    }
    RunReport {
        scenario: scenario_path.to_string(),
        started_unix_s: state.started_unix,
        duration_ms,
        phase: phase.to_string(),
        aborted,
        vehicle_count: state.count,
        vehicles,
        tasks,
        criteria: Vec::new(),
        success: false,
        exit_code: 0,
    }
}

/// Evaluate the scenario's success criteria against the report (spec §9.2:
/// the same code path CI and the operator see), returning the criteria.
pub fn evaluate(report: &mut RunReport, scenario: &Scenario) -> Vec<Criterion> {
    evaluate_criteria(report, scenario);
    report.criteria.clone()
}

/// Write the report JSON to `<run_dir>/run-report.json`.
pub fn write_report(report: &RunReport, run_dir: &Path) -> std::io::Result<()> {
    let path = run_dir.join("run-report.json");
    let json = serde_json::to_string_pretty(report)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(&path, json)?;
    Ok(())
}

/// Parse the FSM transition detail — exposed for unit tests of the parser
/// against the registry's exact log format.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fsm_detail_parser_matches_registry_format() {
        // registry.rs logs: format!("{}->{} cause={}", from.name(), to.name(), cause.name())
        let (from, to, cause) = parse_fsm_detail("ACTIVE->RTL cause=heartbeat_loss").unwrap();
        assert_eq!((from.as_str(), to.as_str(), cause.as_str()), ("ACTIVE", "RTL", "heartbeat_loss"));
        assert!(parse_fsm_detail("ACTIVE->RTL").is_none());
        assert!(parse_fsm_detail("garbage").is_none());
    }

    #[test]
    fn trace_parser_and_breach_count() {
        let log = fleet_core::events::EventLog::in_memory();
        log.log(EventKind::FsmTransition, Some(1), "INIT->SPAWNING cause=spawn");
        log.log(EventKind::FsmTransition, Some(1), "ACTIVE->RTL cause=heartbeat_loss");
        log.log(EventKind::FsmTransition, Some(1), "RTL->LANDED cause=disarm_observed");
        log.log(EventKind::FsmTransition, Some(0), "ACTIVE->RTL cause=geofence_breach");
        log.log(EventKind::SupervisorAction, Some(1), "rtl commanded");
        log.log(EventKind::FsmTransition, None, "INIT->SPAWNING cause=spawn");
        let trace = fsm_trace(&log, 1);
        assert_eq!(trace.len(), 3);
        assert_eq!(trace[1].cause, "heartbeat_loss");
        assert_eq!(geofence_breaches(&trace), 0);
        let t0 = fsm_trace(&log, 0);
        assert_eq!(geofence_breaches(&t0), 1);
        assert_eq!(supervisor_actions(&log, Some(1)).len(), 1);
    }
}
