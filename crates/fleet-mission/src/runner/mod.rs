//! The per-vehicle mission runner (spec §7): a pure phase machine that walks
//! a vehicle's task queue — engage offboard (§3.3), fly to the target at the
//! vehicle's transit altitude layer, descend to hover altitude, hold
//! `hover_s`, next — and emits setpoint goals + lifecycle events for the
//! composition root (fleet-cli), which applies them to the link.
//!
//! All positions are NED metres, home-relative. Setpoints are clamped into
//! the geofence shrunk by 2 m (§7.2) so the manager's own commands can never
//! violate the fence.
//!
//! Completion is telemetry-confirmed when available (estimate within 1 m of
//! the task point for `hover_s`, spec F-2) and falls back to the open-loop
//! profile deadline when the estimate never arrives. The interim simulator
//! has no dynamics, so the profile completes while the hover is *not*
//! observed — that honesty is preserved in the task report
//! (`hover_observed: false`).

#![forbid(unsafe_code)]

use std::collections::VecDeque;

use fleet_safety::geofence::Geofence;

/// Setpoint clamp margin (spec §7.2).
pub const CLAMP_MARGIN_M: f32 = 2.0;
/// Hover completion radius (spec F-2: position within 1 m of the task).
pub const HOVER_RADIUS_M: f32 = 1.0;
/// Offboard engage timeout before retry (seconds).
pub const ENGAGE_TIMEOUT_S: f32 = 5.0;
/// Grace factor beyond the open-loop profile before forced completion.
pub const PROFILE_GRACE: f32 = 1.3;
/// Cruise speed for the open-loop profile (spec §6.2 / §7.1).
pub const CRUISE_MS: f32 = 4.0;
/// Horizontal distance under which the runner commands the final hover
/// altitude instead of the transit layer (final approach, spec §6.1).
pub const FINAL_APPROACH_M: f32 = 25.0;

/// A 3-D task target for the runner (the alloc model is 2-D by design).
#[derive(Debug, Clone, PartialEq)]
pub struct RunnerTask {
    pub id: String,
    pub pos_ned_m: [f32; 3],
    pub hover_s: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RunnerPhase {
    /// Waiting for a task.
    Idle,
    /// Streaming hold setpoints + DO_SET_MODE(OFFBOARD) (spec §3.3).
    Engaging { since_ms: u64, attempts: u32 },
    /// Flying the current task's profile.
    Flying,
    /// Queue empty, all tasks complete.
    Done,
    /// Supervisor or dropout policy stood the vehicle down.
    StoodDown,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RunnerEvent {
    TaskStart { task: usize },
    TaskComplete { task: usize, hover_observed: bool },
    Engaged,
    EngageFailed { attempts: u32 },
    OffboardDropout { consecutive: u32 },
    ReEngaged,
    StoodDown,
}

/// Commands the composition root applies to the link.
#[derive(Debug, Clone, PartialEq)]
pub enum RunnerCmd {
    /// Position-hold at the current anchored position (engage / re-anchor).
    HoldAt { pos: [f32; 3], yaw: f32 },
    /// Position goal (velocity-capped stepping lives in the link pump).
    Goal { pos: [f32; 3], yaw: f32 },
    /// Stop the setpoint stream.
    Stop,
    /// Send DO_SET_MODE OFFBOARD (the stream is already flowing).
    EnterOffboard,
    /// Send DO_SET_MODE AUTO.RTL.
    Rtl,
}

/// Telemetry input for one step.
#[derive(Debug, Clone, Default)]
pub struct RunnerInput {
    pub now_ms: u64,
    /// Position estimate; None until LOCAL_POSITION_NED arrives.
    pub est_pos: Option<[f32; 3]>,
    pub est_yaw: Option<f32>,
    pub in_offboard: bool,
    pub armed: bool,
}

#[derive(Debug, Clone, Default)]
struct HoverTracker {
    in_radius_since_ms: Option<u64>,
    observed: bool,
}

pub struct MissionRunner {
    vehicle_index: usize,
    queue: VecDeque<usize>,
    current: Option<usize>,
    phase: RunnerPhase,
    /// Anchored position for hold/re-anchor (estimate at engage time).
    anchor: Option<[f32; 3]>,
    anchor_yaw: f32,
    transit_z: f32,
    /// Open-loop profile deadline for the current task.
    task_deadline_ms: Option<u64>,
    hover: HoverTracker,
    consecutive_dropouts: u32,
    fence: Geofence,
    tasks: Vec<RunnerTask>,
}

impl MissionRunner {
    pub fn new(
        vehicle_index: usize,
        task_indices: Vec<usize>,
        tasks: Vec<RunnerTask>,
        fence: Geofence,
    ) -> Self {
        MissionRunner {
            vehicle_index,
            queue: task_indices.into(),
            current: None,
            phase: RunnerPhase::Idle,
            anchor: None,
            anchor_yaw: 0.0,
            transit_z: -crate::compile::transit_altitude_m(vehicle_index),
            task_deadline_ms: None,
            hover: HoverTracker::default(),
            consecutive_dropouts: 0,
            fence,
            tasks,
        }
    }

    pub fn vehicle_index(&self) -> usize {
        self.vehicle_index
    }

    pub fn phase(&self) -> &RunnerPhase {
        &self.phase
    }

    pub fn current_task(&self) -> Option<usize> {
        self.current
    }

    pub fn queue_snapshot(&self) -> Vec<usize> {
        self.queue.iter().copied().collect()
    }

    /// Pool the remaining (un-started) tasks (spec §6.5): everything still
    /// queued; the active task is never pooled.
    pub fn pool_remaining(&mut self) -> Vec<usize> {
        std::mem::take(&mut self.queue).into_iter().collect()
    }

    /// Append tasks to the queue (runtime reallocation, spec §6.5): the
    /// composition root awards pooled tasks to a vehicle that is already
    /// flying its own queue. Appending is safe from Idle/Flying/Done; the
    /// active task is untouched.
    pub fn push_tasks(&mut self, task_indices: Vec<usize>) {
        self.queue.extend(task_indices);
    }

    /// Supervisor stand-down: stop flying, no more work from this runner.
    pub fn stand_down(&mut self) -> RunnerEvent {
        self.phase = RunnerPhase::StoodDown;
        RunnerEvent::StoodDown
    }

    /// One step: returns (commands, events). Pure with respect to the input
    /// snapshot; the phase advances on the fleet clock in `now_ms`.
    pub fn step(&mut self, input: &RunnerInput) -> (Vec<RunnerCmd>, Vec<RunnerEvent>) {
        let mut cmds = Vec::new();
        let mut events = Vec::new();

        if input.est_pos.is_some() && self.anchor.is_none() {
            self.anchor = input.est_pos;
            self.anchor_yaw = input.est_yaw.unwrap_or(0.0);
        }

        match self.phase {
            RunnerPhase::Idle => {
                if let Some(ti) = self.queue.pop_front() {
                    self.current = Some(ti);
                    let anchor = self.anchor.or(input.est_pos).unwrap_or([0.0, 0.0, 0.0]);
                    self.anchor = Some(anchor);
                    self.phase = RunnerPhase::Engaging {
                        since_ms: input.now_ms,
                        attempts: 1,
                    };
                    cmds.push(RunnerCmd::HoldAt { pos: anchor, yaw: self.anchor_yaw });
                    cmds.push(RunnerCmd::EnterOffboard);
                    events.push(RunnerEvent::TaskStart { task: ti });
                }
            }
            RunnerPhase::Engaging { since_ms, attempts } => {
                if input.in_offboard {
                    self.phase = RunnerPhase::Flying;
                    events.push(RunnerEvent::Engaged);
                    self.begin_task_profile(input.now_ms);
                    cmds.extend(self.task_cmds(input));
                } else if input.now_ms.saturating_sub(since_ms)
                    > (ENGAGE_TIMEOUT_S * 1000.0) as u64
                {
                    if attempts >= 3 {
                        events.push(RunnerEvent::EngageFailed { attempts });
                        cmds.push(RunnerCmd::Rtl);
                        self.phase = RunnerPhase::StoodDown;
                    } else {
                        // stream keeps flowing (§7.2); retry the mode command
                        let anchor = self.anchor.or(input.est_pos).unwrap_or([0.0, 0.0, 0.0]);
                        self.anchor = Some(anchor);
                        cmds.push(RunnerCmd::HoldAt { pos: anchor, yaw: self.anchor_yaw });
                        cmds.push(RunnerCmd::EnterOffboard);
                        self.phase = RunnerPhase::Engaging {
                            since_ms: input.now_ms,
                            attempts: attempts + 1,
                        };
                    }
                } else if let Some(a) = self.anchor {
                    // keep the stream alive while waiting for the mode echo
                    cmds.push(RunnerCmd::HoldAt { pos: a, yaw: self.anchor_yaw });
                }
            }
            RunnerPhase::Flying => {
                // Offboard dropout detection (§7.2): pause, re-anchor,
                // re-engage; second consecutive dropout -> RTL.
                if !input.in_offboard {
                    self.consecutive_dropouts += 1;
                    if self.consecutive_dropouts >= 2 {
                        events.push(RunnerEvent::OffboardDropout {
                            consecutive: self.consecutive_dropouts,
                        });
                        cmds.push(RunnerCmd::Rtl);
                        self.phase = RunnerPhase::StoodDown;
                        return (cmds, events);
                    }
                    let anchor = input.est_pos.or(self.anchor).unwrap_or([0.0, 0.0, 0.0]);
                    self.anchor = Some(anchor);
                    self.phase = RunnerPhase::Engaging {
                        since_ms: input.now_ms,
                        attempts: 1,
                    };
                    cmds.push(RunnerCmd::HoldAt { pos: anchor, yaw: self.anchor_yaw });
                    cmds.push(RunnerCmd::EnterOffboard);
                    events.push(RunnerEvent::ReEngaged);
                    return (cmds, events);
                }
                self.consecutive_dropouts = 0;

                // Hover observation bookkeeping (telemetry-confirmed).
                if let (Some(ti), Some(pos)) = (self.current, input.est_pos) {
                    let t = &self.tasks[ti];
                    let d = (pos[0] - t.pos_ned_m[0]).hypot(pos[1] - t.pos_ned_m[1]);
                    let dz = (pos[2] - t.pos_ned_m[2]).abs();
                    if d <= HOVER_RADIUS_M && dz <= HOVER_RADIUS_M {
                        match self.hover.in_radius_since_ms {
                            None => self.hover.in_radius_since_ms = Some(input.now_ms),
                            Some(s) => {
                                let held = (input.now_ms.saturating_sub(s)) as f32 / 1000.0;
                                if held >= t.hover_s {
                                    self.hover.observed = true;
                                }
                            }
                        }
                    } else {
                        self.hover.in_radius_since_ms = None;
                    }
                }

                // Completion: observed hover, or open-loop deadline.
                let done = match self.current {
                    Some(ti) => {
                        self.hover.observed
                            || self
                                .task_deadline_ms
                                .map(|d| input.now_ms >= d)
                                .unwrap_or(false)
                    }
                    None => true,
                };
                if done {
                    if let Some(ti) = self.current.take() {
                        events.push(RunnerEvent::TaskComplete {
                            task: ti,
                            hover_observed: self.hover.observed,
                        });
                    }
                    self.task_deadline_ms = None;
                    self.hover = HoverTracker::default();
                    // Dropouts count per task (spec §7.2): reset on completion,
                    // not on re-engage.
                    self.consecutive_dropouts = 0;
                    if self.queue.is_empty() {
                        self.phase = RunnerPhase::Done;
                        cmds.push(RunnerCmd::Stop);
                    } else {
                        // begin the next task without re-engaging
                        self.current = self.queue.pop_front();
                        self.begin_task_profile(input.now_ms);
                        if let Some(ti) = self.current {
                            events.push(RunnerEvent::TaskStart { task: ti });
                        }
                        cmds.extend(self.task_cmds(input));
                    }
                } else {
                    cmds.extend(self.task_cmds(input));
                }
            }
            RunnerPhase::Done | RunnerPhase::StoodDown => {
                // terminal; the supervisor owns RTL/LAND from here.
            }
        }
        (cmds, events)
    }

    /// Open-loop profile deadline for the current task from `now`.
    fn begin_task_profile(&mut self, now_ms: u64) {
        let ti = match self.current {
            Some(t) => t,
            None => return,
        };
        let t = &self.tasks[ti];
        let start = self.anchor.unwrap_or([0.0, 0.0, 0.0]);
        let dist = (t.pos_ned_m[0] - start[0])
            .hypot(t.pos_ned_m[1] - start[1])
            .max((self.transit_z - start[2]).abs());
        let dur_s = dist / CRUISE_MS * PROFILE_GRACE + t.hover_s + 5.0;
        self.task_deadline_ms = Some(now_ms + (dur_s * 1000.0) as u64);
        self.hover = HoverTracker::default();
    }

    /// Setpoint command for the current task: position target at the task
    /// xy clamped into the shrunk fence (§7.2); the transit layer while far
    /// out, the task altitude on final approach (§6.1); yaw faces the
    /// direction of travel (§7.1). The velocity-capped stepping toward this
    /// goal lives in the link's 20 Hz pump.
    fn task_cmds(&self, input: &RunnerInput) -> Vec<RunnerCmd> {
        let ti = match self.current {
            Some(t) => t,
            None => return Vec::new(),
        };
        let t = &self.tasks[ti];
        let xy = self.fence.clamp_setpoint([t.pos_ned_m[0], t.pos_ned_m[1]], CLAMP_MARGIN_M);
        let target_z = self.fence.clamp_altitude(t.pos_ned_m[2], CLAMP_MARGIN_M);
        let start = self.anchor.unwrap_or([0.0, 0.0, 0.0]);
        let dist = (t.pos_ned_m[0] - start[0]).hypot(t.pos_ned_m[1] - start[1]);
        let z = if dist > FINAL_APPROACH_M {
            self.fence.clamp_altitude(self.transit_z, CLAMP_MARGIN_M)
        } else {
            target_z
        };
        let yaw = input
            .est_yaw
            .or_else(|| {
                let dx = t.pos_ned_m[0] - start[0];
                let dy = t.pos_ned_m[1] - start[1];
                if dx.abs() + dy.abs() > 0.5 {
                    Some(dy.atan2(dx))
                } else {
                    None
                }
            })
            .unwrap_or(self.anchor_yaw);
        vec![RunnerCmd::Goal {
            pos: [xy[0], xy[1], z],
            yaw,
        }]
    }
}

#[cfg(test)]
mod tests;
