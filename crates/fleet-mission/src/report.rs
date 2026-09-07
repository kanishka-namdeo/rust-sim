//! Run report (spec §9.2): written at run end regardless of outcome; the
//! success criteria are pure predicates over the report's own data, so CI
//! asserts exactly what the operator sees.

#![forbid(unsafe_code)]

use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use fleet_core::fsm::{FsmCause, FsmState};

use crate::scenario::Scenario;

/// Exit codes the CI driver can classify (spec §12.3, F-7).
pub const EXIT_OK: i32 = 0;
pub const EXIT_CRITERIA_FAILED: i32 = 1;
pub const EXIT_ABORTED: i32 = 2;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct FsmArc {
    pub from: String,
    pub to: String,
    pub cause: String,
    pub t_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct VehicleReport {
    pub index: u8,
    pub sysid: u8,
    pub final_fsm: String,
    pub final_mode: String,
    pub armed: bool,
    pub battery_pct: i8,
    /// Ordered FSM trace (the CI assertion vocabulary).
    pub fsm_trace: Vec<FsmArc>,
    /// Task indices completed (telemetry-confirmed hover true/false).
    pub tasks_completed: Vec<TaskDone>,
    /// Tasks still assigned when the run ended.
    pub tasks_remaining: Vec<String>,
    pub geofence_breaches: u32,
    pub supervisor_actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskDone {
    pub id: String,
    pub hover_observed: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TaskStatus {
    pub id: String,
    pub pos_ned_m: [f32; 3],
    /// None = unassigned; Some(v) = vehicle index.
    pub assigned: Option<u8>,
    pub state: String, // pending | queued | active | complete | rejected
    pub hover_observed: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Criterion {
    pub name: String,
    pub required: bool,
    pub passed: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunReport {
    pub scenario: String,
    pub started_unix_s: u64,
    pub duration_ms: u64,
    pub phase: String,
    pub aborted: bool,
    pub vehicle_count: u8,
    pub vehicles: Vec<VehicleReport>,
    pub tasks: Vec<TaskStatus>,
    pub criteria: Vec<Criterion>,
    pub success: bool,
    pub exit_code: i32,
}

/// Evaluate the scenario's success criteria against the report data.
///
/// `all_landed` semantics (documented because the spec table is terse):
/// every vehicle that flew (ever left READY) ended in LANDED, and no
/// vehicle is ACTIVE or RTL at run end.
pub fn evaluate_criteria(report: &mut RunReport, scenario: &Scenario) {
    let mut criteria = Vec::new();

    if let Some(required) = scenario.success.all_tasks_done {
        let incomplete: Vec<&str> = report
            .tasks
            .iter()
            .filter(|t| t.state != "complete")
            .map(|t| t.id.as_str())
            .collect();
        let passed = incomplete.is_empty();
        criteria.push(Criterion {
            name: "all_tasks_done".into(),
            required,
            passed,
            detail: if passed {
                format!("{} tasks complete", report.tasks.len())
            } else {
                format!("incomplete: {incomplete:?}")
            },
        });
    }

    if let Some(required) = scenario.success.all_landed {
        let flying: Vec<u8> = report
            .vehicles
            .iter()
            .filter(|v| v.final_fsm == "ACTIVE" || v.final_fsm == "RTL")
            .map(|v| v.index)
            .collect();
        let flew_but_not_landed: Vec<u8> = report
            .vehicles
            .iter()
            .filter(|v| {
                let ever_flew = v.fsm_trace.iter().any(|a| a.to == "ACTIVE");
                ever_flew && v.final_fsm != "LANDED"
            })
            .map(|v| v.index)
            .collect();
        let passed = flying.is_empty() && flew_but_not_landed.is_empty();
        criteria.push(Criterion {
            name: "all_landed".into(),
            required,
            passed,
            detail: if passed {
                "no vehicle flying at end; all that flew landed".into()
            } else {
                format!("still flying: {flying:?}; flew-but-not-landed: {flew_but_not_landed:?}")
            },
        });
    }

    if let Some(max_s) = scenario.success.max_time_s {
        let passed = (report.duration_ms as f64 / 1000.0) <= max_s;
        criteria.push(Criterion {
            name: "max_time_s".into(),
            required: true,
            passed,
            detail: format!(
                "run {:.1}s vs budget {:.0}s",
                report.duration_ms as f64 / 1000.0,
                max_s
            ),
        });
    }

    if let Some(required) = scenario.success.no_geofence_breach {
        let breaches: u32 = report.vehicles.iter().map(|v| v.geofence_breaches).sum();
        criteria.push(Criterion {
            name: "no_geofence_breach".into(),
            required,
            passed: breaches == 0,
            detail: format!("{breaches} breach events"),
        });
    }

    for tr in &scenario.success.fsm_trace {
        let veh = report
            .vehicles
            .iter()
            .find(|v| v.index == tr.vehicle)
            .cloned();
        let passed = veh
            .map(|v| {
                v.fsm_trace
                    .iter()
                    .any(|a| a.from == tr.from && a.to == tr.to && a.cause == tr.cause)
            })
            .unwrap_or(false);
        criteria.push(Criterion {
            name: format!(
                "fsm_trace[{} {}->{} {}]",
                tr.vehicle, tr.from, tr.to, tr.cause
            ),
            required: true,
            passed,
            detail: if passed {
                "arc observed".into()
            } else {
                "arc not observed".into()
            },
        });
    }

    let success = criteria.iter().all(|c| c.passed);
    let exit = if report.aborted {
        EXIT_ABORTED
    } else if success {
        EXIT_OK
    } else {
        EXIT_CRITERIA_FAILED
    };
    report.criteria = criteria;
    report.success = success;
    report.exit_code = exit;
}

/// Current unix seconds for the report header.
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Parse an FSM state name (for trace assertions against string specs).
pub fn parse_state(name: &str) -> Option<FsmState> {
    FsmState::ALL.iter().copied().find(|s| s.name() == name)
}

/// Parse an FSM cause name.
pub fn parse_cause(name: &str) -> Option<FsmCause> {
    fleet_core::fsm::CAUSES.iter().copied().find(|c| c.name() == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn veh(index: u8, final_fsm: &str, trace: Vec<(&str, &str, &str)>) -> VehicleReport {
        VehicleReport {
            index,
            sysid: index + 1,
            final_fsm: final_fsm.into(),
            final_mode: "POSCTL".into(),
            armed: false,
            battery_pct: -1,
            fsm_trace: trace
                .into_iter()
                .map(|(from, to, cause)| FsmArc {
                    from: from.into(),
                    to: to.into(),
                    cause: cause.into(),
                    t_ms: 0,
                })
                .collect(),
            tasks_completed: Vec::new(),
            tasks_remaining: Vec::new(),
            geofence_breaches: 0,
            supervisor_actions: Vec::new(),
        }
    }

    fn report(vehicles: Vec<VehicleReport>, tasks: Vec<TaskStatus>) -> RunReport {
        RunReport {
            scenario: "test.toml".into(),
            started_unix_s: 0,
            duration_ms: 10_000,
            phase: "COMPLETE".into(),
            aborted: false,
            vehicle_count: vehicles.len() as u8,
            vehicles,
            tasks,
            criteria: Vec::new(),
            success: false,
            exit_code: 0,
        }
    }

    #[test]
    fn all_tasks_done_criterion() {
        let mut r = report(
            vec![veh(0, "LANDED", vec![])],
            vec![
                TaskStatus { id: "a".into(), pos_ned_m: [0.0, 0.0, -10.0], assigned: Some(0), state: "complete".into(), hover_observed: Some(false) },
                TaskStatus { id: "b".into(), pos_ned_m: [0.0, 0.0, -10.0], assigned: Some(0), state: "active".into(), hover_observed: None },
            ],
        );
        let scenario = Scenario::parse_toml("[success]\nall_tasks_done = true\n").unwrap();
        evaluate_criteria(&mut r, &scenario);
        assert!(!r.success);
        assert_eq!(r.exit_code, EXIT_CRITERIA_FAILED);
        r.tasks[1].state = "complete".into();
        let mut r2 = report(r.vehicles.clone(), r.tasks.clone());
        evaluate_criteria(&mut r2, &scenario);
        assert!(r2.success);
        assert_eq!(r2.exit_code, EXIT_OK);
    }

    #[test]
    fn all_landed_semantics() {
        let scenario = Scenario::parse_toml("[success]\nall_landed = true\n").unwrap();
        // never flew: READY at end is fine
        let mut r = report(
            vec![veh(0, "READY", vec![("INIT", "SPAWNING", "spawn")])],
            vec![],
        );
        evaluate_criteria(&mut r, &scenario);
        assert!(r.success, "never-flew vehicle passes all_landed");
        // flew and landed: pass
        let mut r = report(
            vec![veh(0, "LANDED", vec![
                ("READY", "ACTIVE", "task_accepted"),
                ("ACTIVE", "RTL", "task_complete"),
                ("RTL", "LANDED", "disarm_observed"),
            ])],
            vec![],
        );
        evaluate_criteria(&mut r, &scenario);
        assert!(r.success);
        // flew, still ACTIVE: fail
        let mut r = report(
            vec![veh(0, "ACTIVE", vec![("READY", "ACTIVE", "task_accepted")])],
            vec![],
        );
        evaluate_criteria(&mut r, &scenario);
        assert!(!r.success);
        // flew, ended RTL (not landed): fail
        let mut r = report(
            vec![veh(0, "RTL", vec![("READY", "ACTIVE", "task_accepted"), ("ACTIVE", "RTL", "battery_low")])],
            vec![],
        );
        evaluate_criteria(&mut r, &scenario);
        assert!(!r.success);
    }

    #[test]
    fn fsm_trace_criterion_matches_arcs() {
        let scenario = Scenario::parse_toml(
            "[success]\nfsm_trace = [{vehicle = 1, from = \"ACTIVE\", to = \"RTL\", cause = \"heartbeat_loss\"}]\n",
        )
        .unwrap();
        let mut r = report(
            vec![
                veh(0, "LANDED", vec![]),
                veh(1, "LANDED", vec![("ACTIVE", "RTL", "heartbeat_loss")]),
            ],
            vec![],
        );
        evaluate_criteria(&mut r, &scenario);
        assert!(r.success);
        // wrong cause
        let mut r = report(
            vec![veh(1, "LANDED", vec![("ACTIVE", "RTL", "battery_low")])],
            vec![],
        );
        evaluate_criteria(&mut r, &scenario);
        assert!(!r.success);
    }

    #[test]
    fn aborted_beats_failed_criteria_for_exit_code() {
        let mut r = report(vec![veh(0, "ACTIVE", vec![])], vec![]);
        r.aborted = true;
        let scenario = Scenario::parse_toml("[success]\nall_tasks_done = true\n").unwrap();
        evaluate_criteria(&mut r, &scenario);
        assert_eq!(r.exit_code, EXIT_ABORTED);
    }

    #[test]
    fn max_time_criterion() {
        let mut r = report(vec![veh(0, "READY", vec![])], vec![]);
        r.duration_ms = 200_000;
        let scenario = Scenario::parse_toml("[success]\nmax_time_s = 100\n").unwrap();
        evaluate_criteria(&mut r, &scenario);
        assert!(!r.success);
        r.duration_ms = 50_000;
        let mut r2 = report(vec![veh(0, "READY", vec![])], vec![]);
        r2.duration_ms = 50_000;
        evaluate_criteria(&mut r2, &scenario);
        assert!(r2.success);
    }

    #[test]
    fn state_and_cause_parsers() {
        assert_eq!(parse_state("READY"), Some(FsmState::Ready));
        assert_eq!(parse_state("READYX"), None);
        assert_eq!(parse_cause("heartbeat_loss"), Some(FsmCause::HeartbeatLoss));
        assert_eq!(parse_cause("nope"), None);
    }
}
