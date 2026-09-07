//! Scenario configuration (SPEC §9): TOML schema, defaults, and load-time
//! validation (§9.2).
//!
//! Unknown keys are rejected (`deny_unknown_fields`) because a typo silently
//! reverting to default is a debugging trap. Validation failures name the
//! offending key and value (§9.2) and exit code 4 at the CLI boundary.
//!
//! The scenario hash is SHA-256 of the canonical JSON serialization of the
//! parsed config (serde struct order is fixed, so the serialization is a
//! deterministic function of the parsed value); it is logged at startup and
//! embedded in the replay header (§8.2) so a replay file always identifies
//! the config that produced it.

use std::fmt;

use serde::{Deserialize, Serialize};

use sitsim_fault::FaultSpec;

/// Full parsed scenario. `fault` is `[[fault]]` in TOML.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioConfig {
    #[serde(default)]
    pub io: IoConfig,
    #[serde(default)]
    pub sim: SimConfig,
    #[serde(default)]
    pub vehicle: VehicleConfig,
    #[serde(default)]
    pub dynamics: DynamicsConfig,
    #[serde(default)]
    pub sensors: SensorsConfig,
    #[serde(default)]
    pub env: EnvConfig,
    #[serde(default, rename = "fault")]
    pub faults: Vec<FaultSpec>,
}

/// I/O endpoints (§9.1 `[io]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IoConfig {
    /// HIL listen port (4560 + i convention).
    #[serde(default = "d_tcp_port")]
    pub tcp_port: u16,
    /// Control-plane port (8200 + i convention).
    #[serde(default = "d_api_port")]
    pub api_port: u16,
    /// Bind address (loopback by default, SPEC §4.1).
    #[serde(default = "d_bind")]
    pub bind: String,
}

fn d_tcp_port() -> u16 {
    4560
}
fn d_api_port() -> u16 {
    8200
}
fn d_bind() -> String {
    "127.0.0.1".into()
}

impl Default for IoConfig {
    fn default() -> Self {
        IoConfig {
            tcp_port: d_tcp_port(),
            api_port: d_api_port(),
            bind: d_bind(),
        }
    }
}

/// Simulation core (§9.1 `[sim]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimConfig {
    /// Tick and HIL_SENSOR rate, 50-400 Hz.
    #[serde(default = "d_rate_hz")]
    pub rate_hz: f32,
    /// Scenario duration; 0 = until REST stop.
    #[serde(default = "d_duration_s")]
    pub duration_s: f32,
    /// Master RNG seed (streams per §8.1).
    #[serde(default = "d_seed")]
    pub seed: u64,
    /// Wall-clock pacing factor (ADR-007): 1.0 = real time, 0 = unbounded
    /// (lockstep tolerates both; SPEC §1.1/§3.10). Not in the §9.1 key
    /// table — see docs/adr/0007-sim-speed-key.md.
    #[serde(default = "d_speed")]
    pub speed: f32,
}

fn d_rate_hz() -> f32 {
    200.0
}
fn d_duration_s() -> f32 {
    120.0
}
fn d_seed() -> u64 {
    42
}
fn d_speed() -> f32 {
    1.0
}

impl Default for SimConfig {
    fn default() -> Self {
        SimConfig {
            rate_hz: d_rate_hz(),
            duration_s: d_duration_s(),
            seed: d_seed(),
            speed: d_speed(),
        }
    }
}

/// Vehicle placement (§9.1 `[vehicle]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VehicleConfig {
    #[serde(default)]
    pub origin: OriginConfig,
    #[serde(default)]
    pub initial: InitialConfig,
}

impl Default for VehicleConfig {
    fn default() -> Self {
        VehicleConfig {
            origin: OriginConfig::default(),
            initial: InitialConfig::default(),
        }
    }
}

/// Local NED origin (§6.2 default: the PX4 test field, 500 m MSL).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OriginConfig {
    #[serde(default = "d_lat")]
    pub lat_deg: f64,
    #[serde(default = "d_lon")]
    pub lon_deg: f64,
    #[serde(default = "d_alt")]
    pub alt_m: f64,
}

fn d_lat() -> f64 {
    47.397_770
}
fn d_lon() -> f64 {
    8.545_580
}
fn d_alt() -> f64 {
    500.0
}

impl Default for OriginConfig {
    fn default() -> Self {
        OriginConfig { lat_deg: d_lat(), lon_deg: d_lon(), alt_m: d_alt() }
    }
}

/// Initial state (position only in v0.1; attitude is level, at rest).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitialConfig {
    /// NED start position, m (z down: 0 = on the ground plane).
    #[serde(default = "d_pos")]
    pub pos_ned_m: [f32; 3],
}

fn d_pos() -> [f32; 3] {
    [0.0, 0.0, 0.0]
}

impl Default for InitialConfig {
    fn default() -> Self {
        InitialConfig { pos_ned_m: d_pos() }
    }
}

/// Dynamics parameters (§5.5). Key names follow Appendix B (`c_T`, `c_M`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(non_snake_case)] // c_T / c_M key names follow SPEC Appendix B
pub struct DynamicsConfig {
    #[serde(default = "d_mass")]
    pub mass_kg: f32,
    /// Diagonal inertia (Jx, Jy, Jz), kg m^2.
    #[serde(default = "d_inertia")]
    pub inertia: [f32; 3],
    #[serde(default = "d_arm")]
    pub arm_m: f32,
    /// Thrust coefficient, N per (rad/s)^2.
    #[serde(default = "d_ct")]
    pub c_T: f32,
    /// Yaw moment coefficient, N m per (rad/s)^2.
    #[serde(default = "d_cm")]
    pub c_M: f32,
    #[serde(default = "d_omax")]
    pub omega_max: f32,
    #[serde(default = "d_tau")]
    pub tau_motor: f32,
    /// Idle throttle threshold.
    #[serde(default = "d_uidle")]
    pub u_idle: f32,
    #[serde(default = "d_klin")]
    pub k_lin: f32,
    #[serde(default = "d_kquad")]
    pub k_quad: f32,
    #[serde(default = "d_g")]
    pub g_ms2: f32,
    #[serde(default = "d_kg")]
    pub k_g: f32,
    #[serde(default = "d_cg")]
    pub c_g: f32,
    #[serde(default = "d_mu")]
    pub mu: f32,
    /// Battery capacity, mAh.
    #[serde(default = "d_cap")]
    pub battery_capacity_mah: u32,
}

fn d_mass() -> f32 {
    1.5
}
fn d_inertia() -> [f32; 3] {
    [0.02, 0.02, 0.04]
}
fn d_arm() -> f32 {
    0.225
}
fn d_ct() -> f32 {
    1.10e-5
}
fn d_cm() -> f32 {
    1.25e-7
}
fn d_omax() -> f32 {
    950.0
}
fn d_tau() -> f32 {
    0.03
}
fn d_uidle() -> f32 {
    0.05
}
fn d_klin() -> f32 {
    0.10
}
fn d_kquad() -> f32 {
    0.85
}
fn d_g() -> f32 {
    9.80665
}
fn d_kg() -> f32 {
    4000.0
}
fn d_cg() -> f32 {
    240.0
}
fn d_mu() -> f32 {
    0.8
}
fn d_cap() -> u32 {
    5200
}

impl Default for DynamicsConfig {
    fn default() -> Self {
        DynamicsConfig {
            mass_kg: d_mass(),
            inertia: d_inertia(),
            arm_m: d_arm(),
            c_T: d_ct(),
            c_M: d_cm(),
            omega_max: d_omax(),
            tau_motor: d_tau(),
            u_idle: d_uidle(),
            k_lin: d_klin(),
            k_quad: d_kquad(),
            g_ms2: d_g(),
            k_g: d_kg(),
            c_g: d_cg(),
            mu: d_mu(),
            battery_capacity_mah: d_cap(),
        }
    }
}

/// Sensor models (§9.1 `[sensors.*]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SensorsConfig {
    #[serde(default)]
    pub imu: ImuConfig,
    #[serde(default)]
    pub gps: GpsConfig,
    #[serde(default)]
    pub baro: BaroConfig,
    #[serde(default)]
    pub mag: MagConfig,
    #[serde(default)]
    pub battery: BatteryConfig,
}

impl Default for SensorsConfig {
    fn default() -> Self {
        SensorsConfig {
            imu: ImuConfig::default(),
            gps: GpsConfig::default(),
            baro: BaroConfig::default(),
            mag: MagConfig::default(),
            battery: BatteryConfig::default(),
        }
    }
}

/// IMU error model (§6.3). §9.1 says "6 keys" but the §6.3 table defines a
/// 10-parameter model (noise/walk/turn-on/scale/misalignment per gyro and
/// accel); all ten are exposed (ADR-008).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImuConfig {
    #[serde(default = "d_gyro_noise")]
    pub gyro_noise_density: f32,
    #[serde(default = "d_gyro_walk")]
    pub gyro_bias_walk: f32,
    #[serde(default = "d_gyro_turnon")]
    pub gyro_turnon_sigma: f32,
    #[serde(default = "d_scale")]
    pub gyro_scale_sigma: f32,
    #[serde(default = "d_misalign")]
    pub gyro_misalign_deg: f32,
    #[serde(default = "d_accel_noise")]
    pub accel_noise_density: f32,
    #[serde(default = "d_accel_walk")]
    pub accel_bias_walk: f32,
    #[serde(default = "d_accel_turnon")]
    pub accel_turnon_sigma: f32,
    #[serde(default = "d_scale")]
    pub accel_scale_sigma: f32,
    #[serde(default = "d_misalign")]
    pub accel_misalign_deg: f32,
}

fn d_gyro_noise() -> f32 {
    0.00035
}
fn d_gyro_walk() -> f32 {
    0.0002
}
fn d_gyro_turnon() -> f32 {
    0.003
}
fn d_scale() -> f32 {
    0.002
}
fn d_misalign() -> f32 {
    0.1
}
fn d_accel_noise() -> f32 {
    0.0025
}
fn d_accel_walk() -> f32 {
    0.0004
}
fn d_accel_turnon() -> f32 {
    0.05
}

impl Default for ImuConfig {
    fn default() -> Self {
        ImuConfig {
            gyro_noise_density: d_gyro_noise(),
            gyro_bias_walk: d_gyro_walk(),
            gyro_turnon_sigma: d_gyro_turnon(),
            gyro_scale_sigma: d_scale(),
            gyro_misalign_deg: d_misalign(),
            accel_noise_density: d_accel_noise(),
            accel_bias_walk: d_accel_walk(),
            accel_turnon_sigma: d_accel_turnon(),
            accel_scale_sigma: d_scale(),
            accel_misalign_deg: d_misalign(),
        }
    }
}

/// GPS (§6.4, §9.1). `pos_noise_vert_m` and `satellites` are beyond the
/// §9.1 seven-key list (the §6.4 model needs them) — see ADR-008.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GpsConfig {
    #[serde(default = "d_gps_rate")]
    pub rate_hz: f32,
    #[serde(default = "d_eph")]
    pub eph_cm: u16,
    #[serde(default = "d_epv")]
    pub epv_cm: u16,
    #[serde(default = "d_latency")]
    pub latency_ms: u32,
    #[serde(default = "d_lock")]
    pub lock_s: f32,
    #[serde(default = "d_pos_noise")]
    pub pos_noise_m: f32,
    #[serde(default = "d_pos_noise_v")]
    pub pos_noise_vert_m: f32,
    #[serde(default = "d_vel_noise")]
    pub vel_noise_ms: f32,
    #[serde(default = "d_sats")]
    pub satellites: u8,
}

fn d_gps_rate() -> f32 {
    5.0
}
fn d_eph() -> u16 {
    100
}
fn d_epv() -> u16 {
    150
}
fn d_latency() -> u32 {
    120
}
fn d_lock() -> f32 {
    0.0
}
fn d_pos_noise() -> f32 {
    0.8
}
fn d_pos_noise_v() -> f32 {
    1.5
}
fn d_vel_noise() -> f32 {
    0.2
}
fn d_sats() -> u8 {
    10
}

impl Default for GpsConfig {
    fn default() -> Self {
        GpsConfig {
            rate_hz: d_gps_rate(),
            eph_cm: d_eph(),
            epv_cm: d_epv(),
            latency_ms: d_latency(),
            lock_s: d_lock(),
            pos_noise_m: d_pos_noise(),
            pos_noise_vert_m: d_pos_noise_v(),
            vel_noise_ms: d_vel_noise(),
            satellites: d_sats(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BaroConfig {
    #[serde(default = "d_baro_noise")]
    pub noise_m: f32,
    #[serde(default = "d_baro_walk")]
    pub walk_m_per_min: f32,
}

fn d_baro_noise() -> f32 {
    0.12
}
fn d_baro_walk() -> f32 {
    0.01
}

impl Default for BaroConfig {
    fn default() -> Self {
        BaroConfig { noise_m: d_baro_noise(), walk_m_per_min: d_baro_walk() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MagConfig {
    #[serde(default = "d_mag_noise")]
    pub noise_gauss: f32,
    #[serde(default = "d_hard_iron")]
    pub hard_iron_gauss: f32,
}

fn d_mag_noise() -> f32 {
    0.002
}
fn d_hard_iron() -> f32 {
    0.01
}

impl Default for MagConfig {
    fn default() -> Self {
        MagConfig { noise_gauss: d_mag_noise(), hard_iron_gauss: d_hard_iron() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatteryConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "d_batt_cap")]
    pub capacity_mah: u32,
}

fn d_batt_cap() -> u32 {
    5200
}

impl Default for BatteryConfig {
    fn default() -> Self {
        BatteryConfig { enabled: false, capacity_mah: d_batt_cap() }
    }
}

/// Environment (§9.1 `[env]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvConfig {
    /// Steady NED wind, m/s (|v| <= 15).
    #[serde(default = "d_wind")]
    pub wind_steady_ms: [f32; 3],
    #[serde(default = "d_turb")]
    pub turbulence: TurbulencePreset,
    #[serde(default)]
    pub field: FieldConfig,
}

fn d_wind() -> [f32; 3] {
    [0.0, 0.0, 0.0]
}
fn d_turb() -> TurbulencePreset {
    TurbulencePreset::Moderate
}

impl Default for EnvConfig {
    fn default() -> Self {
        EnvConfig {
            wind_steady_ms: d_wind(),
            turbulence: d_turb(),
            field: FieldConfig::default(),
        }
    }
}

/// Turbulence preset (§6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurbulencePreset {
    Off,
    Light,
    Moderate,
    Severe,
}

impl From<TurbulencePreset> for sitsim_env::Turbulence {
    fn from(p: TurbulencePreset) -> Self {
        match p {
            TurbulencePreset::Off => sitsim_env::Turbulence::Off,
            TurbulencePreset::Light => sitsim_env::Turbulence::Light,
            TurbulencePreset::Moderate => sitsim_env::Turbulence::Moderate,
            TurbulencePreset::Severe => sitsim_env::Turbulence::Severe,
        }
    }
}

/// Local magnetic field (§6.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldConfig {
    #[serde(default = "d_incl")]
    pub incl_deg: f32,
    #[serde(default = "d_decl")]
    pub decl_deg: f32,
    #[serde(default = "d_h")]
    pub h_gauss: f32,
}

fn d_incl() -> f32 {
    67.0
}
fn d_decl() -> f32 {
    2.0
}
fn d_h() -> f32 {
    0.5
}

impl Default for FieldConfig {
    fn default() -> Self {
        FieldConfig { incl_deg: d_incl(), decl_deg: d_decl(), h_gauss: d_h() }
    }
}

/// A validation failure naming the offending key and value (§9.2).
#[derive(Debug, Clone)]
pub struct ValidationError {
    pub key: String,
    pub value: String,
    pub reason: String,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid value for `{}`: {} ({})", self.key, self.value, self.reason)
    }
}

impl std::error::Error for ValidationError {}

/// Load a scenario from TOML text (parse + validate).
pub fn parse_scenario(toml_text: &str) -> Result<ScenarioConfig, String> {
    let cfg: ScenarioConfig = toml::from_str(toml_text).map_err(|e| format!("TOML parse error: {e}"))?;
    validate(&cfg).map_err(|e| e.to_string())?;
    Ok(cfg)
}

/// Load from a file (read + parse + validate).
pub fn load_scenario_file(path: &std::path::Path) -> Result<ScenarioConfig, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read scenario {}: {e}", path.display()))?;
    parse_scenario(&text)
}

/// Validate ranges and references (§9.2). All failures name key + value.
pub fn validate(cfg: &ScenarioConfig) -> Result<(), ValidationError> {
    let err = |key: &str, value: String, reason: &str| {
        Err(ValidationError { key: key.into(), value, reason: reason.into() })
    };

    if !(1024..=65535).contains(&cfg.io.tcp_port) {
        return err("io.tcp_port", cfg.io.tcp_port.to_string(), "must be 1024-65535");
    }
    if !(1024..=65535).contains(&cfg.io.api_port) {
        return err("io.api_port", cfg.io.api_port.to_string(), "must be 1024-65535");
    }
    if cfg.io.bind.parse::<std::net::IpAddr>().is_err() {
        return err("io.bind", cfg.io.bind.clone(), "must be an IP address");
    }
    if !(50.0..=400.0).contains(&cfg.sim.rate_hz) {
        return err("sim.rate_hz", cfg.sim.rate_hz.to_string(), "must be 50-400");
    }
    if cfg.sim.duration_s < 0.0 {
        return err("sim.duration_s", cfg.sim.duration_s.to_string(), "must be >= 0");
    }
    if cfg.sim.speed < 0.0 {
        return err("sim.speed", cfg.sim.speed.to_string(), "must be >= 0");
    }

    // Geodetic sanity (§9.1 "geodetic bounds").
    if !(-90.0..=90.0).contains(&cfg.vehicle.origin.lat_deg) {
        return err("vehicle.origin.lat_deg", cfg.vehicle.origin.lat_deg.to_string(), "must be -90..90");
    }
    if !(-180.0..=180.0).contains(&cfg.vehicle.origin.lon_deg) {
        return err("vehicle.origin.lon_deg", cfg.vehicle.origin.lon_deg.to_string(), "must be -180..180");
    }
    // "alt >= -0.1" for the start position: z down, so pos_ned_m[2] <= 0.1.
    if cfg.vehicle.initial.pos_ned_m[2] > 0.1 {
        return err(
            "vehicle.initial.pos_ned_m[2]",
            cfg.vehicle.initial.pos_ned_m[2].to_string(),
            "start altitude must be >= -0.1 m",
        );
    }

    let d = &cfg.dynamics;
    if d.mass_kg <= 0.0 {
        return err("dynamics.mass_kg", d.mass_kg.to_string(), "must be positive");
    }
    for &j in d.inertia.iter() {
        if j <= 0.0 {
            return err("dynamics.inertia", j.to_string(), "all axes must be positive");
        }
    }
    if d.arm_m <= 0.0 {
        return err("dynamics.arm_m", d.arm_m.to_string(), "must be positive");
    }
    if d.c_T <= 0.0 {
        return err("dynamics.c_T", d.c_T.to_string(), "must be positive");
    }
    if d.c_M <= 0.0 {
        return err("dynamics.c_M", d.c_M.to_string(), "must be positive");
    }
    if d.omega_max <= 0.0 {
        return err("dynamics.omega_max", d.omega_max.to_string(), "must be positive");
    }
    if d.tau_motor <= 0.0 {
        return err("dynamics.tau_motor", d.tau_motor.to_string(), "must be positive");
    }
    if d.k_lin < 0.0 || d.k_quad < 0.0 {
        return err("dynamics.k_lin/k_quad", format!("{}/{}", d.k_lin, d.k_quad), "must be non-negative");
    }
    if d.g_ms2 <= 0.0 {
        return err("dynamics.g_ms2", d.g_ms2.to_string(), "must be positive");
    }
    if d.k_g <= 0.0 || d.c_g < 0.0 || d.mu < 0.0 {
        return err("dynamics.k_g/c_g/mu", format!("{}/{}/{}", d.k_g, d.c_g, d.mu), "must be positive/non-negative");
    }
    if d.battery_capacity_mah == 0 {
        return err("dynamics.battery_capacity_mah", d.battery_capacity_mah.to_string(), "must be positive");
    }

    // Sensor noise parameters are non-negative (§9.2).
    let imu = &cfg.sensors.imu;
    let imu_vals = [
        ("sensors.imu.gyro_noise_density", imu.gyro_noise_density),
        ("sensors.imu.gyro_bias_walk", imu.gyro_bias_walk),
        ("sensors.imu.gyro_turnon_sigma", imu.gyro_turnon_sigma),
        ("sensors.imu.gyro_scale_sigma", imu.gyro_scale_sigma),
        ("sensors.imu.gyro_misalign_deg", imu.gyro_misalign_deg),
        ("sensors.imu.accel_noise_density", imu.accel_noise_density),
        ("sensors.imu.accel_bias_walk", imu.accel_bias_walk),
        ("sensors.imu.accel_turnon_sigma", imu.accel_turnon_sigma),
        ("sensors.imu.accel_scale_sigma", imu.accel_scale_sigma),
        ("sensors.imu.accel_misalign_deg", imu.accel_misalign_deg),
    ];
    for (k, v) in imu_vals {
        if v < 0.0 {
            return err(k, v.to_string(), "must be non-negative");
        }
    }

    let gps = &cfg.sensors.gps;
    if gps.rate_hz <= 0.0 || gps.rate_hz > cfg.sim.rate_hz {
        return err(
            "sensors.gps.rate_hz",
            gps.rate_hz.to_string(),
            "must be > 0 and <= sim.rate_hz",
        );
    }
    if gps.lock_s < 0.0 {
        return err("sensors.gps.lock_s", gps.lock_s.to_string(), "must be non-negative");
    }
    for (k, v) in [
        ("sensors.gps.pos_noise_m", gps.pos_noise_m),
        ("sensors.gps.pos_noise_vert_m", gps.pos_noise_vert_m),
        ("sensors.gps.vel_noise_ms", gps.vel_noise_ms),
    ] {
        if v < 0.0 {
            return err(k, v.to_string(), "must be non-negative");
        }
    }
    if cfg.sensors.baro.noise_m < 0.0 || cfg.sensors.baro.walk_m_per_min < 0.0 {
        return err(
            "sensors.baro.noise_m/walk_m_per_min",
            format!("{}/{}", cfg.sensors.baro.noise_m, cfg.sensors.baro.walk_m_per_min),
            "must be non-negative",
        );
    }
    if cfg.sensors.mag.noise_gauss < 0.0 || cfg.sensors.mag.hard_iron_gauss < 0.0 {
        return err(
            "sensors.mag.noise_gauss/hard_iron_gauss",
            format!("{}/{}", cfg.sensors.mag.noise_gauss, cfg.sensors.mag.hard_iron_gauss),
            "must be non-negative",
        );
    }

    // Wind magnitude bound (§9.1 "abs <= 15").
    let w = cfg.env.wind_steady_ms;
    let mag = (w[0] * w[0] + w[1] * w[1] + w[2] * w[2]).sqrt();
    if mag > 15.0 {
        return err("env.wind_steady_ms", format!("{mag:.2}"), "magnitude must be <= 15 m/s");
    }

    // Fault references: valid motor indices, factor range (§9.2).
    for (i, f) in cfg.faults.iter().enumerate() {
        match f.ftype {
            sitsim_fault::FaultType::MotorEfficiency | sitsim_fault::FaultType::MotorCut => {
                if let Some(m) = f.params.motor {
                    if m > 3 {
                        return err(
                            &format!("fault[{i}].motor"),
                            m.to_string(),
                            "must be 0-3",
                        );
                    }
                }
            }
            _ => {}
        }
        if let Some(fac) = f.params.factor {
            if !(0.0..=1.0).contains(&fac) {
                return err(&format!("fault[{i}].factor"), fac.to_string(), "must be 0-1");
            }
        }
        if f.duration_ms.is_none() && !f.persistent && f.start_ms != 0 {
            // A timeline event with no window and not persistent only fires
            // on the single tick t == start_ms; that is a pulse, legal but
            // almost always a mistake — require an explicit window.
            return err(
                &format!("fault[{i}]"),
                f.id.clone(),
                "timeline faults need duration_ms or persistent = true",
            );
        }
    }

    Ok(())
}

/// SHA-256 of the canonical JSON serialization of the config (§8.2 replay
/// header / §9.2 scenario hash). Returns 32 bytes.
pub fn scenario_hash(cfg: &ScenarioConfig) -> [u8; 32] {
    let canonical = serde_json::to_string(cfg).expect("scenario serializes");
    crate::hash::sha256(canonical.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    const APPENDIX_B: &str = r#"
[io]
tcp_port = 4560
api_port = 8200

[sim]
rate_hz = 200
duration_s = 150
seed = 42

[vehicle]
origin = { lat_deg = 47.397770, lon_deg = 8.545580, alt_m = 500.0 }
initial = { pos_ned_m = [0.0, 0.0, 0.0] }

[dynamics]
mass_kg = 1.5
inertia = [0.02, 0.02, 0.04]
arm_m = 0.225
c_T = 1.10e-5
c_M = 1.25e-7
omega_max = 950.0
tau_motor = 0.03

[sensors.gps]
rate_hz = 5
latency_ms = 120
pos_noise_m = 0.8
vel_noise_ms = 0.2
eph_cm = 100
epv_cm = 150

[env]
wind_steady_ms = [1.5, 0.0, 0.0]
turbulence = "moderate"

[[fault]]
id = "denial_1"
type = "gps_denial"
start_ms = 30000
duration_ms = 20000
"#;

    /// The Appendix B reference scenario parses verbatim (checked into
    /// docs/examples/reference.toml).
    #[test]
    fn appendix_b_reference_parses() {
        let cfg = parse_scenario(APPENDIX_B).expect("appendix B parses");
        assert_eq!(cfg.io.tcp_port, 4560);
        assert_eq!(cfg.sim.rate_hz, 200.0);
        assert_eq!(cfg.sim.duration_s, 150.0);
        assert_eq!(cfg.sim.seed, 42);
        assert_eq!(cfg.dynamics.c_T, 1.10e-5);
        assert_eq!(cfg.dynamics.c_M, 1.25e-7);
        assert_eq!(cfg.env.wind_steady_ms, [1.5, 0.0, 0.0]);
        assert_eq!(cfg.faults.len(), 1);
        assert_eq!(cfg.faults[0].id, "denial_1");
        assert_eq!(cfg.faults[0].ftype, sitsim_fault::FaultType::GpsDenial);
        // Defaults filled in for omitted sections.
        assert_eq!(cfg.sensors.imu.gyro_noise_density, 0.00035);
        assert_eq!(cfg.sensors.battery.enabled, false);
    }

    /// The on-disk docs/examples/reference.toml is identical in effect.
    #[test]
    fn checked_in_reference_toml_parses() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/examples/reference.toml");
        let cfg = load_scenario_file(&path).expect("docs/examples/reference.toml parses");
        assert_eq!(cfg.faults.len(), 1);
    }

    #[test]
    fn empty_toml_gives_all_defaults() {
        let cfg = parse_scenario("").unwrap();
        assert_eq!(cfg.io.tcp_port, 4560);
        assert_eq!(cfg.io.api_port, 8200);
        assert_eq!(cfg.sim.rate_hz, 200.0);
        assert_eq!(cfg.sim.duration_s, 120.0);
        assert_eq!(cfg.vehicle.origin.lat_deg, 47.397770);
        assert_eq!(cfg.dynamics.mass_kg, 1.5);
        assert_eq!(cfg.sensors.gps.latency_ms, 120);
    }

    #[test]
    fn unknown_key_rejected() {
        let e = parse_scenario("[sim]\nrate_hzz = 100\n").unwrap_err();
        assert!(e.contains("unknown field"), "{e}");
        // Naming the offending key: serde includes the field name.
        assert!(e.contains("rate_hzz"), "{e}");
    }

    #[test]
    fn range_violation_names_key_and_value() {
        let e = parse_scenario("[sim]\nrate_hz = 500\n").unwrap_err();
        assert!(e.contains("sim.rate_hz"), "{e}");
        assert!(e.contains("500"), "{e}");
    }

    #[test]
    fn wind_magnitude_bound() {
        let e = parse_scenario("[env]\nwind_steady_ms = [20.0, 0.0, 0.0]\n").unwrap_err();
        assert!(e.contains("wind_steady_ms"), "{e}");
    }

    #[test]
    fn bad_motor_index_rejected() {
        let e = parse_scenario(
            "[[fault]]\nid = \"x\"\ntype = \"motor_cut\"\nmotor = 7\npersistent = true\n",
        )
        .unwrap_err();
        assert!(e.contains("motor"), "{e}");
    }

    #[test]
    fn timeline_fault_needs_window() {
        let e = parse_scenario(
            "[[fault]]\nid = \"x\"\ntype = \"gps_denial\"\nstart_ms = 5000\n",
        )
        .unwrap_err();
        assert!(e.contains("duration_ms"), "{e}");
    }

    #[test]
    fn scenario_hash_is_stable_and_seed_sensitive() {
        let a = parse_scenario(APPENDIX_B).unwrap();
        let mut b = a.clone();
        b.sim.seed = 43;
        assert_eq!(scenario_hash(&a), scenario_hash(&a));
        assert_ne!(scenario_hash(&a), scenario_hash(&b));
        // Canonical round trip: JSON -> parse-equivalent value.
        let json = serde_json::to_string(&a).unwrap();
        let back: ScenarioConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(scenario_hash(&back), scenario_hash(&a));
    }
}
