//! Mission compiler (spec §6.1, §9): scenario -> allocation-ready inputs
//! with compile-time rejection of tasks no vehicle can fly, and per-vehicle
//! transit altitude layers for planner-level deconfliction.

#![forbid(unsafe_code)]

use fleet_alloc::Task;
use fleet_safety::geofence::Geofence;

use crate::scenario::Scenario;

/// Transit altitude for vehicle k (spec §6.1): 10 + 5k m AGL.
pub const fn transit_altitude_m(vehicle_index: usize) -> f32 {
    10.0 + 5.0 * vehicle_index as f32
}

/// How far a vehicle may be asked to fly, as a crude reachability bound
/// (compile-time): a task must be within this radius of home (geofence
/// diagonal + margin) — the geofence check below is the real gate; this
/// catches absurd scenarios early.
pub fn max_reach_m(fence: &Geofence) -> f32 {
    let mut r = 0.0f32;
    for p in &fence.points {
        r = r.max(p[0].hypot(p[1]));
    }
    r + 50.0
}

#[derive(Debug, Clone)]
pub struct MissionPlan {
    /// Tasks accepted for allocation (fleet-alloc model).
    pub tasks: Vec<Task>,
    /// Tasks rejected at compile time, with reasons (logged, spec §6.1).
    pub rejections: Vec<(String, String)>,
    pub fence: Geofence,
    /// Per-vehicle transit altitude (AGL, positive up).
    pub transit_altitude_m: Vec<f32>,
}

impl MissionPlan {
    pub fn task_index(&self, id: &str) -> Option<usize> {
        self.tasks.iter().position(|t| t.id == id)
    }

    /// Compile the scenario. Tasks outside the fence (or outside the
    /// altitude box) are rejected with a reason, not discovered mid-flight.
    pub fn compile(scenario: &Scenario) -> MissionPlan {
        let fence = scenario.fence().expect("validated scenario has a fence");
        let reach = max_reach_m(&fence);
        let mut tasks = Vec::new();
        let mut rejections = Vec::new();
        for t in &scenario.tasks {
            let xy_ok = fence.contains_xy([t.pos_ned_m[0], t.pos_ned_m[1]]);
            let alt_ok = fence.altitude_ok(t.pos_ned_m[2]);
            if !xy_ok {
                rejections.push((
                    t.id.clone(),
                    format!(
                        "task outside geofence polygon (pos {:?}, breach {:.1} m)",
                        t.pos_ned_m,
                        fence.horizontal_breach([t.pos_ned_m[0], t.pos_ned_m[1]])
                    ),
                ));
                continue;
            }
            if !alt_ok {
                rejections.push((
                    t.id.clone(),
                    format!(
                        "task outside altitude box (z {:.1} m NED; box -{:.0}..-{:.0})",
                        t.pos_ned_m[2], fence.ceiling_m, fence.floor_m
                    ),
                ));
                continue;
            }
            let d = t.pos_ned_m[0].hypot(t.pos_ned_m[1]);
            if d > reach {
                rejections.push((
                    t.id.clone(),
                    format!("task unreachable: {d:.0} m from home, budget {reach:.0} m"),
                ));
                continue;
            }
            tasks.push(Task {
                id: t.id.clone(),
                pos_ned_m: [t.pos_ned_m[0], t.pos_ned_m[1]],
                hover_s: t.hover_s,
                reward: t.reward,
                deadline_s: t.deadline_s.unwrap_or(f32::INFINITY),
            });
        }
        let count = scenario.count() as usize;
        MissionPlan {
            tasks,
            rejections,
            fence,
            transit_altitude_m: (0..count).map(transit_altitude_m).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::Scenario;

    #[test]
    fn altitude_layers_per_6_1() {
        assert_eq!(transit_altitude_m(0), 10.0);
        assert_eq!(transit_altitude_m(1), 15.0);
        assert_eq!(transit_altitude_m(3), 25.0);
    }

    #[test]
    fn tasks_outside_fence_rejected_at_compile() {
        let s = Scenario::parse_toml(
            "[[tasks]]\nid = \"in\"\npos_ned_m = [0.0, 0.0, -12.0]\n\
             [[tasks]]\nid = \"out\"\npos_ned_m = [150.0, 0.0, -12.0]\n\
             [[tasks]]\nid = \"high\"\npos_ned_m = [0.0, 0.0, -90.0]\n",
        )
        .unwrap();
        let plan = MissionPlan::compile(&s);
        assert_eq!(plan.tasks.len(), 1);
        assert_eq!(plan.tasks[0].id, "in");
        let rejected: Vec<&str> = plan.rejections.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(rejected, vec!["out", "high"]);
        assert!(plan.rejections[1].1.contains("altitude"));
    }

    #[test]
    fn canonical_scenario_compiles_clean() {
        let s = Scenario::parse_toml(include_str!("../../../docs/examples/fleet-basic.toml")).unwrap();
        let plan = MissionPlan::compile(&s);
        assert!(plan.rejections.is_empty());
        assert_eq!(plan.tasks.len(), 4);
        assert_eq!(plan.transit_altitude_m, vec![10.0, 15.0]);
    }
}
