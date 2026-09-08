//! The fleet scenario DSL (spec §9): one TOML file describing the fleet
//! parameters, environment, task set, timeline and success criteria.
//!
//! Key set is frozen per spec §9.1 / R-16; the `[sim]` section is the one
//! documented addition (ADR-0008: per-vehicle simulator command template for
//! the interim-sim strategy of ADR-0001). Unknown keys are **rejected**, not
//! ignored — a scenario file that quietly drops a typo'd key is a test
//! hazard.

#![forbid(unsafe_code)]

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use fleet_safety::geofence::Geofence;

// ---------------------------------------------------------------------------
// Sections
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FleetSection {
    pub count: Option<u8>,
    #[serde(default = "default_true")]
    pub battery_sim: bool,
    #[serde(default)]
    pub restart_on_fault: bool,
    /// Supervisor rate; reserved at 10 in v0.1 (spec §9.1).
    #[serde(default = "default_tick_hz")]
    pub tick_hz: u8,
    /// Setup-bench mode (ADR-0016): the fleet boots to READY and holds —
    /// the mission never starts, vehicles stay disarmed for the
    /// QGC-style configuration workflow (airframe / calibration / params
    /// all require disarm). The run ends at max_time_s (or SIGINT).
    #[serde(default)]
    pub hold_for_setup: bool,
}

impl Default for FleetSection {
    fn default() -> Self {
        FleetSection { count: None, battery_sim: true, restart_on_fault: false, tick_hz: 10, hold_for_setup: false }
    }
}

fn default_true() -> bool {
    true
}
fn default_tick_hz() -> u8 {
    10
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeofenceSpec {
    pub points_ned_m: Vec<[f32; 2]>,
    #[serde(default = "default_ceiling")]
    pub ceiling_m: f32,
    #[serde(default = "default_floor")]
    pub floor_m: f32,
}

fn default_ceiling() -> f32 {
    60.0
}
fn default_floor() -> f32 {
    0.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvSection {
    pub geofence: Option<GeofenceSpec>,
    #[serde(default = "default_wind")]
    pub wind_steady_ms: [f32; 3],
    #[serde(default = "default_turbulence")]
    pub turbulence: String,
    /// Geo origin every vehicle's sim HIL_GPS anchors to (ADR-0017): the
    /// `[env] origin` the fleet converts lat/lon <-> NED against AND the
    /// value SimCtl exports to the sim wrapper (`RSIM_ORIGIN_*`). Defaults
    /// to the PX4 test field — same constant as rustsitsim's
    /// `GeoOrigin::DEFAULT`, so the two sides cannot disagree silently.
    #[serde(default)]
    pub origin: Option<OriginSpec>,
}

impl Default for EnvSection {
    fn default() -> Self {
        EnvSection {
            geofence: None,
            wind_steady_ms: [0.0, 0.0, 0.0],
            turbulence: "moderate".into(),
            origin: None,
        }
    }
}

fn default_wind() -> [f32; 3] {
    [0.0, 0.0, 0.0]
}
fn default_turbulence() -> String {
    "moderate".into()
}

/// `[env] origin` spec (ADR-0017): the geodetic anchor of the local NED
/// frame. Same defaults as rustsitsim's `GeoOrigin::DEFAULT`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OriginSpec {
    pub lat_deg: f64,
    pub lon_deg: f64,
    #[serde(default = "default_origin_alt")]
    pub alt_m: f64,
}

fn default_origin_alt() -> f64 {
    500.0
}

impl Default for OriginSpec {
    fn default() -> Self {
        OriginSpec { lat_deg: 47.397770, lon_deg: 8.545580, alt_m: 500.0 }
    }
}

impl From<OriginSpec> for fleet_core::geo::GeoOrigin {
    fn from(o: OriginSpec) -> Self {
        fleet_core::geo::GeoOrigin { lat_deg: o.lat_deg, lon_deg: o.lon_deg, alt_m: o.alt_m }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSpec {
    pub id: String,
    /// NED metres, home-relative (z negative up, per Appendix B).
    pub pos_ned_m: [f32; 3],
    #[serde(default = "default_hover")]
    pub hover_s: f32,
    #[serde(default = "default_reward")]
    pub reward: f32,
    /// Deadline in fleet seconds; omitted = none.
    #[serde(default)]
    pub deadline_s: Option<f32>,
}

fn default_hover() -> f32 {
    0.0
}
fn default_reward() -> f32 {
    1.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VehicleOverride {
    pub index: u8,
    /// Partial rustsitsim config patch (battery discharge scale etc.).
    /// Passed through to the sim config generator; opaque to v0.1 (ADR-0001).
    pub battery: Option<toml::Table>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FaultSpec {
    #[serde(rename = "type")]
    pub kind: String,
    pub start_ms: Option<u64>,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventSpec {
    /// "link_loss" | "fault" | "estop".
    pub kind: String,
    /// Vehicle index the event targets.
    #[serde(default)]
    pub vehicle: Option<u8>,
    /// Fire time in fleet seconds.
    #[serde(default)]
    pub start_s: Option<f64>,
    #[serde(default)]
    pub duration_s: Option<f64>,
    /// `fault` payload (Appendix B shape).
    pub fault: Option<FaultSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FsmTraceSpec {
    pub vehicle: u8,
    pub from: String,
    pub to: String,
    pub cause: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct SuccessSection {
    #[serde(default)]
    pub all_tasks_done: Option<bool>,
    #[serde(default)]
    pub all_landed: Option<bool>,
    #[serde(default)]
    pub max_time_s: Option<f64>,
    #[serde(default)]
    pub no_geofence_breach: Option<bool>,
    #[serde(default)]
    pub fsm_trace: Vec<FsmTraceSpec>,
}

/// Per-vehicle simulator command template (ADR-0008).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimSection {
    /// argv template; placeholders {hil_port} {instance} {sysid}
    /// {duration_s} {sitsim} {sim_script} (fleet-simctl renders).
    pub command: Option<String>,
    /// Seconds the simulator should run (must outlast the scenario).
    #[serde(default = "default_sim_duration")]
    pub duration_s: Option<f64>,
}

impl Default for SimSection {
    fn default() -> Self {
        SimSection { command: None, duration_s: Some(300.0) }
    }
}

fn default_sim_duration() -> Option<f64> {
    Some(300.0)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    #[serde(default)]
    pub fleet: FleetSection,
    #[serde(default)]
    pub env: EnvSection,
    #[serde(default)]
    pub tasks: Vec<TaskSpec>,
    #[serde(default, rename = "vehicle_override")]
    pub vehicle_overrides: Vec<VehicleOverride>,
    #[serde(default, rename = "event")]
    pub events: Vec<EventSpec>,
    #[serde(default)]
    pub success: SuccessSection,
    #[serde(default)]
    pub sim: SimSection,
}

impl Default for Scenario {
    fn default() -> Self {
        serde::Deserialize::deserialize(toml::Value::Table(Default::default())).unwrap()
    }
}

// ---------------------------------------------------------------------------
// Parsing + validation
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct ScenarioError {
    pub message: String,
}

impl std::fmt::Display for ScenarioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}
impl std::error::Error for ScenarioError {}

impl Scenario {
    /// Parse from TOML text with schema validation.
    pub fn parse_toml(text: &str) -> Result<Scenario, ScenarioError> {
        let value: toml::Value = toml::from_str(text)
            .map_err(|e| ScenarioError { message: format!("TOML syntax: {e}") })?;
        let scenario: Scenario = value
            .try_into()
            .map_err(|e: toml::de::Error| ScenarioError { message: format!("schema: {e}") })?;
        scenario.validate()?;
        Ok(scenario)
    }

    /// Parse from a file path.
    pub fn parse_file(path: &std::path::Path) -> Result<(Scenario, String), ScenarioError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| ScenarioError { message: format!("read {}: {e}", path.display()) })?;
        let s = Self::parse_toml(&text)?;
        Ok((s, path.display().to_string()))
    }

    /// Semantic validation beyond the shape (spec §9.1 ranges + §6.1
    /// compile-time task checks).
    pub fn validate(&self) -> Result<(), ScenarioError> {
        let count = self.count();
        if !(1..=8).contains(&count) {
            return Err(ScenarioError {
                message: format!("[fleet] count must be 1-8, got {count}"),
            });
        }
        if self.fleet.tick_hz != 10 {
            return Err(ScenarioError {
                message: format!(
                    "[fleet] tick_hz is fixed at 10 in v0.1 (spec §9.1), got {}",
                    self.fleet.tick_hz
                ),
            });
        }
        // unique task ids
        let mut ids = HashSet::new();
        for t in &self.tasks {
            if !ids.insert(t.id.as_str()) {
                return Err(ScenarioError {
                    message: format!("duplicate task id '{}'", t.id),
                });
            }
            if t.hover_s < 0.0 {
                return Err(ScenarioError {
                    message: format!("task '{}' hover_s must be >= 0", t.id),
                });
            }
        }
        // geofence parses
        self.fence()
            .map_err(|e| ScenarioError { message: format!("[env] geofence: {e}") })?;
        // geo origin range check (ADR-0017)
        if let Some(o) = &self.env.origin {
            if !(-90.0..=90.0).contains(&o.lat_deg) || !(-180.0..=180.0).contains(&o.lon_deg) {
                return Err(ScenarioError {
                    message: format!(
                        "[env] origin out of range (lat {}, lon {})",
                        o.lat_deg, o.lon_deg
                    ),
                });
            }
        }
        // events: kinds + vehicles in range
        for ev in &self.events {
            match ev.kind.as_str() {
                "link_loss" | "fault" | "estop" => {}
                other => {
                    return Err(ScenarioError {
                        message: format!("unknown [[event]] kind '{other}'"),
                    })
                }
            }
            if let Some(v) = ev.vehicle {
                if v >= count {
                    return Err(ScenarioError {
                        message: format!("[[event]] vehicle {v} out of range (count {count})"),
                    });
                }
            }
            if ev.kind == "estop" && ev.vehicle.is_some() {
                return Err(ScenarioError {
                    message: "[[event]] estop is fleet-wide; 'vehicle' must be omitted".into(),
                });
            }
            if ev.kind == "link_loss" && (ev.vehicle.is_none() || ev.duration_s.is_none()) {
                return Err(ScenarioError {
                    message: "[[event]] link_loss requires vehicle and duration_s".into(),
                });
            }
        }
        for ov in &self.vehicle_overrides {
            if ov.index >= count {
                return Err(ScenarioError {
                    message: format!(
                        "[[vehicle_override]] index {} out of range (count {count})",
                        ov.index
                    ),
                });
            }
        }
        if let Some(mt) = self.success.max_time_s {
            if mt <= 0.0 {
                return Err(ScenarioError {
                    message: "[success] max_time_s must be positive".into(),
                });
            }
        }
        for tr in &self.success.fsm_trace {
            let from_ok = fleet_core::fsm::FsmState::ALL.iter().any(|s| s.name() == tr.from);
            let to_ok = fleet_core::fsm::FsmState::ALL.iter().any(|s| s.name() == tr.to);
            if !from_ok || !to_ok {
                return Err(ScenarioError {
                    message: format!(
                        "[success] fsm_trace from/to must be FSM states, got {}->{}",
                        tr.from, tr.to
                    ),
                });
            }
            if fleet_core::fsm::CAUSES.iter().all(|c| c.name() != tr.cause) {
                return Err(ScenarioError {
                    message: format!("[success] fsm_trace unknown cause '{}'", tr.cause),
                });
            }
            if tr.vehicle >= count {
                return Err(ScenarioError {
                    message: format!("[success] fsm_trace vehicle {} out of range", tr.vehicle),
                });
            }
        }
        Ok(())
    }

    pub fn count(&self) -> u8 {
        self.fleet.count.unwrap_or(2)
    }

    /// Effective geofence (spec §9.1 default: 100 m square, 60 m ceiling).
    pub fn fence(&self) -> Result<Geofence, fleet_safety::geofence::GeofenceError> {
        match &self.env.geofence {
            Some(g) => Geofence::parse(g.points_ned_m.clone(), g.ceiling_m, g.floor_m),
            None => Ok(Geofence::default_square()),
        }
    }

    /// Effective geo origin (ADR-0017): `[env] origin` or the PX4 test
    /// field default — the single anchor for fleet geo conversion and the
    /// sims' HIL_GPS (exported via `RSIM_ORIGIN_*`).
    pub fn geo_origin(&self) -> fleet_core::geo::GeoOrigin {
        self.env.origin.map(Into::into).unwrap_or(fleet_core::geo::GeoOrigin::DEFAULT)
    }

    pub fn max_time_s(&self) -> Option<f64> {
        self.success.max_time_s
    }

    pub fn sim_command(&self) -> Option<String> {
        self.sim.command.clone()
    }

    pub fn sim_duration_s(&self) -> f64 {
        self.sim.duration_s.unwrap_or(300.0).max(30.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CANONICAL: &str = include_str!("../../../docs/examples/fleet-basic.toml");

    #[test]
    fn canonical_appendix_b_scenario_parses() {
        let s = Scenario::parse_toml(CANONICAL).expect("canonical scenario is valid");
        assert_eq!(s.count(), 2);
        assert_eq!(s.tasks.len(), 4);
        assert_eq!(s.tasks[0].id, "wp_n");
        assert_eq!(s.tasks[0].pos_ned_m, [-60.0, 0.0, -12.0]);
        assert_eq!(s.tasks[0].hover_s, 5.0);
        assert_eq!(s.events.len(), 2);
        assert_eq!(s.events[0].kind, "link_loss");
        assert_eq!(s.events[0].vehicle, Some(1));
        assert_eq!(s.events[0].start_s, Some(40.0));
        assert_eq!(s.events[0].duration_s, Some(15.0));
        assert_eq!(s.events[1].kind, "fault");
        assert_eq!(s.events[1].fault.as_ref().unwrap().kind, "gps_denial");
        assert_eq!(s.success.all_tasks_done, Some(true));
        assert_eq!(s.success.max_time_s, Some(300.0));
        assert_eq!(s.success.fsm_trace.len(), 1);
        assert_eq!(s.success.fsm_trace[0].vehicle, 1);
        assert_eq!(s.success.fsm_trace[0].cause, "heartbeat_loss");
        assert_eq!(s.vehicle_overrides.len(), 1);
        assert_eq!(s.vehicle_overrides[0].index, 1);
        assert!(!s.fleet.restart_on_fault);
        assert!(s.fleet.battery_sim);
    }

    #[test]
    fn geo_origin_parses_and_defaults() {
        // default: the PX4 test field
        let s = Scenario::parse_toml("[fleet]\ncount = 1\n").unwrap();
        assert_eq!(s.geo_origin(), fleet_core::geo::GeoOrigin::DEFAULT);

        // explicit origin (ADR-0017)
        let s = Scenario::parse_toml(
            "[fleet]\ncount = 1\n[env]\norigin = { lat_deg = 47.123, lon_deg = 8.456 }\n",
        )
        .unwrap();
        let o = s.geo_origin();
        assert!((o.lat_deg - 47.123).abs() < 1e-9);
        assert!((o.lon_deg - 8.456).abs() < 1e-9);
        assert!((o.alt_m - 500.0).abs() < 1e-9); // alt defaults

        // out of range is rejected, not silently clamped
        let e = Scenario::parse_toml(
            "[fleet]\ncount = 1\n[env]\norigin = { lat_deg = 91.0, lon_deg = 0.0 }\n",
        );
        assert!(e.is_err());
    }

    #[test]
    fn hold_for_setup_key_parses() {
        let s = Scenario::parse_toml(
            "[fleet]\ncount = 1\nbattery_sim = false\nhold_for_setup = true\n",
        )
        .unwrap();
        assert!(s.fleet.hold_for_setup);
        assert!(!s.fleet.battery_sim);
        assert_eq!(s.count(), 1);
        // unknown keys are still rejected (deny_unknown_fields)
        let e = Scenario::parse_toml("[fleet]\ncount = 1\nhold_for_setups = true\n");
        assert!(e.is_err(), "typo'd hold_for_setup must be rejected, not dropped");
    }

    #[test]
    fn defaults_match_spec_9_1() {
        let s = Scenario::parse_toml("[fleet]\ncount = 3\n").unwrap();
        assert_eq!(s.count(), 3);
        assert!(s.fleet.battery_sim);
        assert!(!s.fleet.restart_on_fault);
        assert!(!s.fleet.hold_for_setup, "setup-bench is opt-in (ADR-0016)");
        assert_eq!(s.fleet.tick_hz, 10);
        assert!(s.tasks.is_empty());
        let fence = s.fence().unwrap();
        assert!(fence.contains_xy([99.0, 99.0]));
        assert!(!fence.contains_xy([101.0, 0.0]));
        assert_eq!(s.max_time_s(), None);
        assert_eq!(s.sim_duration_s(), 300.0);
    }

    #[test]
    fn unknown_keys_rejected() {
        let e = Scenario::parse_toml("[fleet]\ncount = 2\nspam = true\n").unwrap_err();
        assert!(e.message.contains("spam"), "{}", e.message);
        let e = Scenario::parse_toml("[sucess]\nx = 1\n").unwrap_err();
        assert!(!e.message.is_empty());
    }

    #[test]
    fn count_out_of_range_rejected() {
        let e = Scenario::parse_toml("[fleet]\ncount = 9\n").unwrap_err();
        assert!(e.message.contains("1-8"));
        let e = Scenario::parse_toml("[fleet]\ncount = 0\n").unwrap_err();
        assert!(e.message.contains("1-8"));
    }

    #[test]
    fn duplicate_task_ids_rejected() {
        let e = Scenario::parse_toml(
            "[[tasks]]\nid = \"a\"\npos_ned_m = [0.0, 0.0, -10.0]\n[[tasks]]\nid = \"a\"\npos_ned_m = [10.0, 0.0, -10.0]\n",
        )
        .unwrap_err();
        assert!(e.message.contains("duplicate"));
    }

    #[test]
    fn bad_geofence_rejected() {
        let e = Scenario::parse_toml(
            "[env]\ngeofence = { points_ned_m = [[0,0],[1,1],[2,2]], ceiling_m = 60, floor_m = 0 }\n",
        )
        .unwrap_err();
        assert!(e.message.contains("geofence"));
    }

    #[test]
    fn event_validation() {
        // link_loss needs vehicle + duration
        let e = Scenario::parse_toml("[[event]]\nkind = \"link_loss\"\nstart_s = 10.0\n").unwrap_err();
        assert!(e.message.contains("link_loss"));
        // unknown kind
        let e = Scenario::parse_toml("[[event]]\nkind = \"meteor\"\n").unwrap_err();
        assert!(e.message.contains("meteor"));
        // vehicle out of range
        let e = Scenario::parse_toml(
            "[fleet]\ncount = 2\n[[event]]\nkind = \"link_loss\"\nvehicle = 5\nduration_s = 5.0\n",
        )
        .unwrap_err();
        assert!(e.message.contains("out of range"));
    }

    #[test]
    fn fsm_trace_spec_names_validated() {
        let e = Scenario::parse_toml(
            "[success]\nfsm_trace = [{vehicle = 0, from = \"ACTVE\", to = \"RTL\", cause = \"heartbeat_loss\"}]\n",
        )
        .unwrap_err();
        assert!(e.message.contains("ACTVE"));
        let e = Scenario::parse_toml(
            "[success]\nfsm_trace = [{vehicle = 0, from = \"ACTIVE\", to = \"RTL\", cause = \"sunspot\"}]\n",
        )
        .unwrap_err();
        assert!(e.message.contains("sunspot"));
    }

    #[test]
    fn sim_section_parsed() {
        let s = Scenario::parse_toml(
            "[sim]\ncommand = \"python3 /tmp/s.py {hil_port} {duration_s}\"\nduration_s = 120\n",
        )
        .unwrap();
        assert_eq!(s.sim_command().unwrap(), "python3 /tmp/s.py {hil_port} {duration_s}");
        assert_eq!(s.sim_duration_s(), 120.0);
    }
}
