//! Sensor models (SPEC §6.3-6.5).
//!
//! Every model is a pure function of (truth, RNG stream, time step); fault
//! effects (bias ramps, saturation, denial, glitch) are composed by the tick
//! engine in `sitsim-sdk` so this crate stays fault-agnostic.
//!
//! RNG stream ownership (SPEC §8.1): IMU noise = stream 0, IMU turn-on
//! draws = stream 1, magnetometer = stream 2, barometer = stream 3, GPS =
//! stream 4. Consumption order per tick is fixed and documented on each
//! `sample` method.

pub mod baro;
pub mod battery;
pub mod gps;
pub mod imu;
pub mod mag;

pub use baro::{BaroModel, BaroParams, BaroSample};
pub use battery::{BatteryModel, BatteryParams};
pub use gps::{GpsFix, GpsModel, GpsParams};
pub use imu::{ImuModel, ImuParams, ImuSample};
pub use mag::{MagModel, MagParams};

/// Everything a produced sensor sample (last emitted values) carries.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SensorOutputs {
    /// Noisy specific force, body FRD, m/s^2 (includes the gravity reaction
    /// by convention: at rest, level, (0, 0, -9.80665)).
    pub accel_body_ms2: [f64; 3],
    /// Noisy body rates, rad/s.
    pub gyro_body_rads: [f64; 3],
    /// Noisy magnetic field, body frame, gauss.
    pub mag_body_gauss: [f64; 3],
    /// Absolute pressure in HIL_SENSOR field units (see sitsim-env
    /// atmosphere docs / ADR-002).
    pub baro_pressure: f64,
    /// Pressure-derived altitude above the origin, m.
    pub baro_alt_m: f64,
    /// Ambient temperature, degC.
    pub temperature_c: f64,
    /// GPS fix delivered on this tick (already latency-delayed), if any.
    pub gps_fix: Option<GpsFix>,
    /// Last GPS fix type seen (for the telemetry plane).
    pub gps_fix_type: u8,
    pub gps_sats: u8,
}

impl Default for SensorOutputs {
    fn default() -> Self {
        SensorOutputs {
            accel_body_ms2: [0.0; 3],
            gyro_body_rads: [0.0; 3],
            mag_body_gauss: [0.0; 3],
            baro_pressure: 0.0,
            baro_alt_m: 0.0,
            temperature_c: 20.0,
            gps_fix: None,
            gps_fix_type: 0,
            gps_sats: 0,
        }
    }
}
