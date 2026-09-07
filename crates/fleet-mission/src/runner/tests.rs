//! Mission runner unit tests (spec §7 discipline).

use super::*;
use fleet_safety::geofence::Geofence;

fn tasks() -> Vec<RunnerTask> {
    vec![
        RunnerTask { id: "t0".into(), pos_ned_m: [50.0, 0.0, -12.0], hover_s: 2.0 },
        RunnerTask { id: "t1".into(), pos_ned_m: [-50.0, 40.0, -12.0], hover_s: 2.0 },
    ]
}

fn fence() -> Geofence {
    Geofence::default_square()
}

fn input(now_ms: u64, pos: Option<[f32; 3]>, in_offboard: bool) -> RunnerInput {
    RunnerInput {
        now_ms,
        est_pos: pos,
        est_yaw: pos.map(|_| 0.0),
        in_offboard,
        armed: true,
    }
}

impl MissionRunner {
    fn fence(&self) -> &Geofence {
        &self.fence
    }
}

#[test]
fn engage_sequence_follows_3_3() {
    let mut r = MissionRunner::new(0, vec![0, 1], tasks(), fence());
    // First step: task starts, hold-at-current + DO_SET_MODE offboard.
    let (cmds, events) = r.step(&input(1000, Some([0.0, 0.0, 0.0]), false));
    assert!(events.contains(&RunnerEvent::TaskStart { task: 0 }));
    assert!(cmds.contains(&RunnerCmd::HoldAt { pos: [0.0, 0.0, 0.0], yaw: 0.0 }));
    assert!(cmds.contains(&RunnerCmd::EnterOffboard));
    assert!(matches!(r.phase(), RunnerPhase::Engaging { .. }));
    // Not yet in offboard: stream kept alive.
    let (cmds, _) = r.step(&input(2000, Some([0.0, 0.0, 0.0]), false));
    assert!(cmds.iter().any(|c| matches!(c, RunnerCmd::HoldAt { .. })));
    // Mode echo observed: ENGAGED + first goal.
    let (cmds, events) = r.step(&input(2100, Some([0.0, 0.0, 0.0]), true));
    assert!(events.contains(&RunnerEvent::Engaged));
    let goal = cmds.iter().find_map(|c| match c {
        RunnerCmd::Goal { pos, yaw } => Some((*pos, *yaw)),
        _ => None,
    });
    let (pos, _) = goal.expect("goal commanded");
    // Clamped into the shrunk fence; transit layer z (vehicle 0: -10).
    assert!(pos[0].abs() <= 98.1 && pos[1].abs() <= 98.1);
    assert!((pos[2] - (-10.0)).abs() < 0.01, "transit layer: {pos:?}");
}

#[test]
fn engage_retries_then_rtl() {
    let mut r = MissionRunner::new(1, vec![0], tasks(), fence());
    r.step(&input(0, Some([0.0, 0.0, 0.0]), false));
    // attempts 1..3 across ENGAGE_TIMEOUT windows
    let mut now = 0u64;
    for _ in 0..2 {
        now += 6000;
        let (cmds, _) = r.step(&input(now, Some([0.0, 0.0, 0.0]), false));
        assert!(cmds.contains(&RunnerCmd::EnterOffboard), "retry at {now}");
    }
    now += 6000;
    let (cmds, events) = r.step(&input(now, Some([0.0, 0.0, 0.0]), false));
    assert!(events.contains(&RunnerEvent::EngageFailed { attempts: 3 }));
    assert!(cmds.contains(&RunnerCmd::Rtl));
    assert_eq!(*r.phase(), RunnerPhase::StoodDown);
}

#[test]
fn telemetry_confirmed_completion() {
    let mut r = MissionRunner::new(0, vec![0], tasks(), fence());
    r.step(&input(0, Some([0.0, 0.0, 0.0]), false));
    r.step(&input(100, Some([0.0, 0.0, 0.0]), true));
    // Vehicle "arrives" within 1 m of the task (interim sim: would be
    // stationary; here we simulate the estimator report).
    let (cmds, events) = r.step(&input(20_000, Some([50.0, 0.0, -12.0]), true));
    assert!(cmds.iter().any(|c| matches!(c, RunnerCmd::Goal { .. })));
    assert!(events.is_empty(), "not yet: {:?}", events);
    // hover_s = 2.0: hold for 2.5 s
    let (_, events) = r.step(&input(21_000, Some([50.0, 0.0, -12.0]), true));
    assert!(events.is_empty(), "not yet: {:?}", events);
    let (cmds, events) = r.step(&input(23_000, Some([50.0, 0.0, -12.0]), true));
    assert!(events.contains(&RunnerEvent::TaskComplete { task: 0, hover_observed: true }));
    assert!(cmds.contains(&RunnerCmd::Stop));
    assert_eq!(*r.phase(), RunnerPhase::Done);
}

#[test]
fn open_loop_deadline_completes_without_motion() {
    // Interim-sim reality: the estimate never moves. The profile deadline
    // fires, the task completes with hover_observed = false.
    let mut r = MissionRunner::new(0, vec![0, 1], tasks(), fence());
    r.step(&input(0, Some([0.0, 0.0, 0.0]), false));
    r.step(&input(100, Some([0.0, 0.0, 0.0]), true));
    // dist = 50 m / 4 m/s * 1.3 + 2 + 5 ~ 23.25 s
    let (_, events) = r.step(&input(30_000, Some([0.0, 0.0, 0.0]), true));
    assert!(
        events.contains(&RunnerEvent::TaskComplete { task: 0, hover_observed: false }),
        "deadline completion: {events:?}"
    );
    // second task starts automatically
    assert!(events.contains(&RunnerEvent::TaskStart { task: 1 }));
    assert_eq!(*r.phase(), RunnerPhase::Flying);
}

#[test]
fn dropout_reanchor_then_rtl_on_second() {
    let mut r = MissionRunner::new(0, vec![0], tasks(), fence());
    r.step(&input(0, Some([0.0, 0.0, 0.0]), false));
    r.step(&input(100, Some([0.0, 0.0, 0.0]), true));
    // First spontaneous dropout: pause + re-anchor + re-engage.
    let (cmds, events) = r.step(&input(5000, Some([10.0, 0.0, -8.0]), false));
    assert!(events.contains(&RunnerEvent::ReEngaged));
    assert!(cmds.contains(&RunnerCmd::HoldAt { pos: [10.0, 0.0, -8.0], yaw: 0.0 }));
    assert!(cmds.contains(&RunnerCmd::EnterOffboard));
    // Re-engage succeeds, then drops out again: RTL with event.
    r.step(&input(5100, Some([10.0, 0.0, -8.0]), true));
    let (cmds, events) = r.step(&input(9000, Some([10.0, 0.0, -8.0]), false));
    assert!(
        events.contains(&RunnerEvent::OffboardDropout { consecutive: 2 }),
        "events: {events:?}"
    );
    assert!(cmds.contains(&RunnerCmd::Rtl));
    assert_eq!(*r.phase(), RunnerPhase::StoodDown);
}

#[test]
fn setpoints_clamped_into_shrunk_fence() {
    // Task at the very corner: the runner's goal must be inside the fence
    // and ~2 m off the boundary.
    let ts = vec![RunnerTask { id: "edge".into(), pos_ned_m: [100.0, 100.0, -30.0], hover_s: 0.0 }];
    let mut r = MissionRunner::new(2, vec![0], ts, fence());
    r.step(&input(0, Some([0.0, 0.0, 0.0]), false));
    let (cmds, _) = r.step(&input(100, Some([0.0, 0.0, 0.0]), true));
    let pos = cmds
        .iter()
        .find_map(|c| match c {
            RunnerCmd::Goal { pos, .. } => Some(*pos),
            _ => None,
        })
        .unwrap();
    assert!(fence().contains_xy([pos[0], pos[1]]));
    assert!(
        fence().distance_to_polygon([pos[0], pos[1]]) >= CLAMP_MARGIN_M * 0.7,
        "goal too close to boundary: {pos:?}"
    );
    assert!(fence().contains_ned(pos), "goal must be inside the fence");
}

#[test]
fn pool_remaining_never_pools_active_task() {
    let mut r = MissionRunner::new(0, vec![0, 1], tasks(), fence());
    r.step(&input(0, Some([0.0, 0.0, 0.0]), false));
    assert_eq!(r.current_task(), Some(0));
    let pooled = r.pool_remaining();
    assert_eq!(pooled, vec![1]);
    assert_eq!(r.queue_snapshot(), Vec::<usize>::new());
    // active task intact
    assert_eq!(r.current_task(), Some(0));
}

#[test]
fn push_tasks_appends_without_disturbing_active_task() {
    // Reallocation path (spec §6.5): a flying vehicle receives pooled
    // tasks behind its current work.
    let mut r = MissionRunner::new(0, vec![0], tasks(), fence());
    r.step(&input(0, Some([0.0, 0.0, 0.0]), false));
    r.step(&input(100, Some([0.0, 0.0, 0.0]), true));
    assert_eq!(r.current_task(), Some(0));
    r.push_tasks(vec![1]);
    assert_eq!(r.queue_snapshot(), vec![1]);
    assert_eq!(r.current_task(), Some(0));
    // Completing task 0 rolls straight into the pushed task 1.
    let (cmds, events) = r.step(&input(10_000_000, Some([0.0, 0.0, 0.0]), true));
    assert!(events.contains(&RunnerEvent::TaskComplete { task: 0, hover_observed: false }));
    assert!(events.contains(&RunnerEvent::TaskStart { task: 1 }));
    assert!(cmds.iter().any(|c| matches!(c, RunnerCmd::Goal { .. })));
    // Stood-down runners keep their queue semantics if pushed anyway.
    let mut r2 = MissionRunner::new(1, vec![], tasks(), fence());
    r2.stand_down();
    r2.push_tasks(vec![0]);
    assert_eq!(r2.queue_snapshot(), vec![0]);
    let (cmds, events) = r2.step(&input(0, None, false));
    assert!(cmds.is_empty() && events.is_empty(), "stood-down runner ignores pushed work");
}

#[test]
fn stand_down_is_terminal_for_the_runner() {
    let mut r = MissionRunner::new(0, vec![0], tasks(), fence());
    r.step(&input(0, Some([0.0, 0.0, 0.0]), false));
    let ev = r.stand_down();
    assert_eq!(ev, RunnerEvent::StoodDown);
    let (cmds, events) = r.step(&input(1000, Some([0.0, 0.0, 0.0]), true));
    assert!(cmds.is_empty());
    assert!(events.is_empty());
    assert_eq!(*r.phase(), RunnerPhase::StoodDown);
}

#[test]
fn transit_layers_follow_vehicle_index() {
    let mut r0 = MissionRunner::new(0, vec![0], tasks(), fence());
    r0.step(&input(0, Some([0.0, 0.0, 0.0]), false));
    r0.step(&input(100, Some([0.0, 0.0, 0.0]), true));
    // dist 50 > 25: transit z = -10 for vehicle 0
    let cmds = r0.step(&input(200, Some([0.0, 0.0, 0.0]), true)).0;
    let z = cmds
        .iter()
        .find_map(|c| match c {
            RunnerCmd::Goal { pos, .. } => Some(pos[2]),
            _ => None,
        })
        .unwrap();
    assert!((z + 10.0).abs() < 0.01);

    let mut r3 = MissionRunner::new(3, vec![0], tasks(), fence());
    r3.step(&input(0, Some([0.0, 0.0, 0.0]), false));
    let cmds = r3.step(&input(100, Some([0.0, 0.0, 0.0]), true)).0;
    let z = cmds
        .iter()
        .find_map(|c| match c {
            RunnerCmd::Goal { pos, .. } => Some(pos[2]),
            _ => None,
        })
        .unwrap();
    assert!((z + 25.0).abs() < 0.01, "vehicle 3 layer -25: {z}");
}
