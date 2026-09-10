//! Minimal fleet config (TOML) parser.
//!
//! Replaces the (removed) `fleet-mission::scenario::Scenario` parser for
//! the lean `mavfleet run` path (Task 7b). The scenario DSL — task list,
//! timeline events, success criteria — is gone; what's left is the
//! bring-up surface: fleet size, the geofence, the geo origin, the
//! per-vehicle sim command template, and a sim duration. Unknown keys
//! are accepted (forward-compat for existing TOML files in `tests/`
//! that still carry `[[tasks]]` / `[[event]]` / `[success]` blocks the
//! lean manager no longer reads).

#![forbid(unsafe_code)]

use std::path::Path;

use serde::Deserialize;

use fleet_core::geo::GeoOrigin;
use fleet_safety::geofence::Geofence;

/// `[fleet]` — the only section `mavfleet run` actually reads.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct FleetSection {
    pub count: Option<u8>,
    /// Setup-bench mode: the fleet boots to READY and holds — vehicles
    /// stay disarmed for the QGC-style configuration workflow. The run
    /// ends at max_time_s / SIGINT. (Task 7b: the lean manager keeps the
    /// READY-hold semantics for the operator; it doesn't auto-start any
    /// mission.)
    #[serde(default)]
    pub hold_for_setup: bool,
}

/// `[env] geofence` — the inclusion polygon + altitude box (spec §8.2).
#[derive(Debug, Clone, Deserialize)]
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

/// `[env] origin` — the geodetic anchor of the local NED frame.
#[derive(Debug, Clone, Copy, Deserialize)]
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
        OriginSpec {
            lat_deg: 47.397_770,
            lon_deg: 8.545_580,
            alt_m: 500.0,
        }
    }
}

impl From<OriginSpec> for GeoOrigin {
    fn from(o: OriginSpec) -> Self {
        GeoOrigin {
            lat_deg: o.lat_deg,
            lon_deg: o.lon_deg,
            alt_m: o.alt_m,
        }
    }
}

/// `[env]` — geofence + wind + turbulence (the latter two are passed
/// through to the sim wrapper as `RSIM_WIND_MS` / `RSIM_TURBULENCE`).
#[derive(Debug, Clone, Deserialize, Default)]
pub struct EnvSection {
    pub geofence: Option<GeofenceSpec>,
    #[serde(default = "default_wind")]
    pub wind_steady_ms: [f32; 3],
    #[serde(default = "default_turbulence")]
    pub turbulence: String,
    #[serde(default)]
    pub origin: Option<OriginSpec>,
}

fn default_wind() -> [f32; 3] {
    [0.0, 0.0, 0.0]
}
fn default_turbulence() -> String {
    "moderate".into()
}

/// `[sim]` — the per-vehicle sim command template + stream duration.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct SimSection {
    /// Sim command template (substituted with `{instance}`, `{hil_port}`,
    /// `{duration_s}`, …). When omitted, `SimCtlConfig::from_env` falls
    /// back to `FLEET_SIM_COMMAND` or the interim `python3 sim_stream.py`
    /// template.
    pub command: Option<String>,
    /// Seconds the interim sim should stream (must outlast the run).
    #[serde(default = "default_sim_duration_s")]
    pub duration_s: f64,
}

fn default_sim_duration_s() -> f64 {
    300.0
}

/// The lean fleet config (Task 7b) — a small subset of the old
/// scenario DSL. Sections the lean manager ignores (`[[tasks]]`,
/// `[[event]]`, `[success]`, `[[vehicle_override]]`) are accepted
/// silently so existing `tests/*.toml` scenario files still load.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct FleetConfig {
    #[serde(default)]
    pub fleet: FleetSection,
    #[serde(default)]
    pub env: EnvSection,
    #[serde(default)]
    pub sim: SimSection,
}

impl FleetConfig {
    /// Parse a fleet TOML file from disk. Unknown top-level tables are
    /// accepted (the lean manager ignores `[[tasks]]` / `[success]` /
    /// `[[event]]` / `[[vehicle_override]]`); unknown keys inside the
    /// three sections we *do* read are also accepted (forward-compat for
    /// `battery_sim` / `restart_on_fault` / `tick_hz` we no longer use).
    pub fn parse_file(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read fleet config {}: {e}", path.display()))?;
        Self::parse_toml(&text)
    }

    /// Parse a fleet TOML from text (used by tests + the parse-error
    /// path that re-parses the file the user just edited).
    pub fn parse_toml(text: &str) -> Result<Self, String> {
        toml::from_str::<Self>(text).map_err(|e| format!("invalid fleet config: {e}"))
    }

    /// Vehicle count (defaults to 1 if the file omits `[fleet] count`).
    pub fn count(&self) -> u8 {
        self.fleet.count.unwrap_or(1)
    }

    /// The geofence (parsed + validated), or the spec default
    /// (`Geofence::default_square()` — 100 m square, 60 m ceiling).
    pub fn fence(&self) -> Geofence {
        if let Some(spec) = &self.env.geofence {
            Geofence::parse(spec.points_ned_m.clone(), spec.ceiling_m, spec.floor_m)
                .unwrap_or_else(|_| Geofence::default_square())
        } else {
            Geofence::default_square()
        }
    }

    /// Geo origin (defaults to the PX4 test field).
    pub fn geo_origin(&self) -> GeoOrigin {
        self.env.origin.map(GeoOrigin::from).unwrap_or(GeoOrigin::DEFAULT)
    }

    /// `[sim] command` template (or `None` to let `SimCtlConfig` pick
    /// the env-var or interim-sim default).
    pub fn sim_command(&self) -> Option<String> {
        self.sim.command.clone()
    }

    /// `[sim] duration_s` (or the default).
    pub fn sim_duration_s(&self) -> f64 {
        self.sim.duration_s
    }

    /// `[fleet] hold_for_setup`.
    pub fn hold_for_setup(&self) -> bool {
        self.fleet.hold_for_setup
    }

    /// `[env] wind_steady_ms` — passed to the sim wrapper as `RSIM_WIND_MS`.
    pub fn wind_steady_ms(&self) -> [f32; 3] {
        self.env.wind_steady_ms
    }

    /// `[env] turbulence` — passed to the sim wrapper as `RSIM_TURBULENCE`.
    pub fn turbulence(&self) -> &str {
        &self.env.turbulence
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_fleet_config() {
        let cfg = FleetConfig::parse_toml(
            "[fleet]\ncount = 2\nhold_for_setup = true\n[sim]\nduration_s = 600\n",
        )
        .expect("parse");
        assert_eq!(cfg.count(), 2);
        assert!(cfg.hold_for_setup());
        assert!((cfg.sim_duration_s() - 600.0).abs() < 1e-6);
        // defaults: PX4 test field, 100 m square fence
        assert_eq!(cfg.geo_origin(), GeoOrigin::DEFAULT);
        let f = cfg.fence();
        assert_eq!(f.ceiling_m, 60.0);
        assert_eq!(f.points.len(), 4);
    }

    #[test]
    fn parses_env_origin_and_geofence() {
        let cfg = FleetConfig::parse_toml(
            "[fleet]\ncount = 1\n[env]\norigin = { lat_deg = 47.0, lon_deg = 8.0 }\n\
             geofence = { points_ned_m = [[-50, -50], [50, -50], [50, 50], [-50, 50]], ceiling_m = 50, floor_m = 0 }\n\
             wind_steady_ms = [1.0, 0.0, 0.0]\nturbulence = \"light\"\n",
        )
        .expect("parse");
        let o = cfg.geo_origin();
        assert!((o.lat_deg - 47.0).abs() < 1e-9);
        assert!((o.lon_deg - 8.0).abs() < 1e-9);
        assert_eq!(o.alt_m, 500.0); // default
        let f = cfg.fence();
        assert_eq!(f.ceiling_m, 50.0);
        assert_eq!(f.points.len(), 4);
        assert_eq!(cfg.wind_steady_ms(), [1.0, 0.0, 0.0]);
        assert_eq!(cfg.turbulence(), "light");
    }

    /// Existing scenario TOMLs (with `[[tasks]]`, `[success]`, `[[event]]`)
    /// still load — the lean manager ignores those tables.
    #[test]
    fn ignores_removed_scenario_tables() {
        let text = "[fleet]\ncount = 2\n[sim]\nduration_s = 420\n\
                    [[tasks]]\nid = \"wp_n\"\npos_ned_m = [-50.0, 0.0, -12.0]\n\
                    [[event]]\nkind = \"link_loss\"\n\
                    [success]\nall_tasks_done = true\nmax_time_s = 360\n";
        let cfg = FleetConfig::parse_toml(text).expect("parse");
        assert_eq!(cfg.count(), 2);
        assert!((cfg.sim_duration_s() - 420.0).abs() < 1e-6);
    }

    #[test]
    fn bad_toml_is_an_error() {
        let e = FleetConfig::parse_toml("[fleet\ncount = 2\n").unwrap_err();
        assert!(e.contains("invalid fleet config"));
    }
}
