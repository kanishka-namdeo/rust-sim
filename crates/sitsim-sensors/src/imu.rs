//! IMU model (SPEC §6.3): BMI088-class MEMS error model per axis.
//!
//! Error composition (per axis, per sample):
//! ```text
//! measured = R_mis * (scale ⊙ truth) + bias + noise
//! ```
//! - `scale`: diagonal scale-factor errors, drawn once at init
//!   (sigma = scale_sigma, stream 1),
//! - `R_mis`: small-angle axis-misalignment rotation, drawn once at init
//!   (sigma = misalign_deg, stream 1),
//! - `bias`: turn-on bias (drawn once, stream 1) + random walk
//!   (per-tick increments of walk_sigma * sqrt(h), stream 0),
//! - `noise`: white, sigma = noise_density * sqrt(rate), stream 0.
//!
//! Stream-0 consumption order per tick: 3 gyro-axis noise draws, 3
//! bias-walk increments (gyro, then accel interleaved? NO: fixed order —
//! gyro noise x,y,z; accel noise x,y,z; then gyro walk x,y,z; accel walk
//! x,y,z), documented and fixed for reproducibility.

use sitsim_core::Pcg64;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImuParams {
    /// Gyro white-noise density, rad/s/sqrt(Hz).
    pub gyro_noise_density: f64,
    /// Gyro bias random walk, rad/s/sqrt(s).
    pub gyro_bias_walk: f64,
    /// Gyro turn-on bias sigma, rad/s.
    pub gyro_turnon_sigma: f64,
    /// Gyro scale-factor error sigma (dimensionless).
    pub gyro_scale_sigma: f64,
    /// Gyro axis-misalignment sigma, degrees.
    pub gyro_misalign_deg: f64,
    /// Accelerometer white-noise density, m/s^2/sqrt(Hz).
    pub accel_noise_density: f64,
    /// Accelerometer bias random walk, m/s^2/sqrt(s).
    pub accel_bias_walk: f64,
    /// Accelerometer turn-on bias sigma, m/s^2.
    pub accel_turnon_sigma: f64,
    /// Accelerometer scale-factor error sigma.
    pub accel_scale_sigma: f64,
    /// Accelerometer axis-misalignment sigma, degrees.
    pub accel_misalign_deg: f64,
}

impl Default for ImuParams {
    fn default() -> Self {
        // SPEC §6.3 table (BMI088-class defaults).
        ImuParams {
            gyro_noise_density: 0.00035,
            gyro_bias_walk: 0.0002,
            gyro_turnon_sigma: 0.003,
            gyro_scale_sigma: 0.002,
            gyro_misalign_deg: 0.1,
            accel_noise_density: 0.0025,
            accel_bias_walk: 0.0004,
            accel_turnon_sigma: 0.05,
            accel_scale_sigma: 0.002,
            accel_misalign_deg: 0.1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImuSample {
    pub accel_body_ms2: [f64; 3],
    pub gyro_body_rads: [f64; 3],
}

#[derive(Debug, Clone)]
pub struct ImuModel {
    pub params: ImuParams,
    // Turn-on draws (stream 1).
    gyro_scale: [f64; 3],
    accel_scale: [f64; 3],
    gyro_mis: [[f64; 3]; 3], // small-angle rotation matrix
    accel_mis: [[f64; 3]; 3],
    // Walk state.
    gyro_bias: [f64; 3],
    accel_bias: [f64; 3],
}

impl ImuModel {
    /// Draw the turn-on errors from the IMU turn-on stream (stream 1).
    pub fn new(params: ImuParams, turn_on: &mut Pcg64) -> Self {
        let mut gs = [0.0; 3];
        let mut as_ = [0.0; 3];
        for k in 0..3 {
            gs[k] = 1.0 + params.gyro_scale_sigma * turn_on.next_gaussian();
            as_[k] = 1.0 + params.accel_scale_sigma * turn_on.next_gaussian();
        }
        let gm = small_angle_rotation(params.gyro_misalign_deg, turn_on);
        let am = small_angle_rotation(params.accel_misalign_deg, turn_on);
        let mut gb = [0.0; 3];
        let mut ab = [0.0; 3];
        for k in 0..3 {
            gb[k] = params.gyro_turnon_sigma * turn_on.next_gaussian();
            ab[k] = params.accel_turnon_sigma * turn_on.next_gaussian();
        }
        ImuModel {
            params,
            gyro_scale: gs,
            accel_scale: as_,
            gyro_mis: gm,
            accel_mis: am,
            gyro_bias: gb,
            accel_bias: ab,
        }
    }

    /// Sample once. `h` is the tick period; `noise` is the IMU noise stream
    /// (stream 0). Fixed draw order: gyro noise (x,y,z), accel noise
    /// (x,y,z), gyro walk (x,y,z), accel walk (x,y,z).
    pub fn sample(&mut self, true_accel: &[f64; 3], true_gyro: &[f64; 3], h: f64, rate: f64, noise: &mut Pcg64) -> ImuSample {
        let gyro_noise_sigma = self.params.gyro_noise_density * libm::sqrt(rate);
        let accel_noise_sigma = self.params.accel_noise_density * libm::sqrt(rate);
        let walk_h = libm::sqrt(h);

        let mut gn = [0.0; 3];
        let mut an = [0.0; 3];
        for k in 0..3 {
            gn[k] = gyro_noise_sigma * noise.next_gaussian();
        }
        for k in 0..3 {
            an[k] = accel_noise_sigma * noise.next_gaussian();
        }
        // Bias random walk increments (order: gyro x,y,z then accel x,y,z).
        for k in 0..3 {
            self.gyro_bias[k] += self.params.gyro_bias_walk * walk_h * noise.next_gaussian();
        }
        for k in 0..3 {
            self.accel_bias[k] += self.params.accel_bias_walk * walk_h * noise.next_gaussian();
        }

        let mut gyro = [0.0f64; 3];
        let mut accel = [0.0f64; 3];
        let mut scaled_g = [0.0f64; 3];
        let mut scaled_a = [0.0f64; 3];
        for k in 0..3 {
            scaled_g[k] = true_gyro[k] * self.gyro_scale[k];
            scaled_a[k] = true_accel[k] * self.accel_scale[k];
        }
        for k in 0..3 {
            gyro[k] = mul_row(&self.gyro_mis, k, &scaled_g) + self.gyro_bias[k] + gn[k];
            accel[k] = mul_row(&self.accel_mis, k, &scaled_a) + self.accel_bias[k] + an[k];
        }
        ImuSample { accel_body_ms2: accel, gyro_body_rads: gyro }
    }

    /// Current biases (test/inspection).
    pub fn biases(&self) -> ([f64; 3], [f64; 3]) {
        (self.gyro_bias, self.accel_bias)
    }
}

fn mul_row(m: &[[f64; 3]; 3], row: usize, v: &[f64; 3]) -> f64 {
    m[row][0] * v[0] + m[row][1] * v[1] + m[row][2] * v[2]
}

/// Small-angle rotation matrix from three independent axis errors (degrees).
fn small_angle_rotation(sigma_deg: f64, rng: &mut Pcg64) -> [[f64; 3]; 3] {
    let rx = (sigma_deg * rng.next_gaussian()).to_radians();
    let ry = (sigma_deg * rng.next_gaussian()).to_radians();
    let rz = (sigma_deg * rng.next_gaussian()).to_radians();
    let (sx, cx) = (rx.sin(), rx.cos());
    let (sy, cy) = (ry.sin(), ry.cos());
    let (sz, cz) = (rz.sin(), rz.cos());
    // R = Rx * Ry * Rz (small angles: order is immaterial to first order).
    [
        [cy * cz, -cy * sz, sy],
        [sx * sy * cz + cx * sz, -sx * sy * sz + cx * cz, -sx * cy],
        [-cx * sy * cz + sx * sz, cx * sy * sz + sx * cz, cx * cy],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Zero-error IMU (all sigmas zero): measured == truth exactly.
    #[test]
    fn zero_params_pass_truth_through() {
        let p = ImuParams {
            gyro_noise_density: 0.0,
            gyro_bias_walk: 0.0,
            gyro_turnon_sigma: 0.0,
            gyro_scale_sigma: 0.0,
            gyro_misalign_deg: 0.0,
            accel_noise_density: 0.0,
            accel_bias_walk: 0.0,
            accel_turnon_sigma: 0.0,
            accel_scale_sigma: 0.0,
            accel_misalign_deg: 0.0,
        };
        let mut rng = Pcg64::new(42, 1);
        let mut imu = ImuModel::new(p, &mut rng);
        let s = imu.sample(&[0.11, -0.22, -9.71], &[0.001, -0.002, 0.003], 0.005, 200.0, &mut rng);
        assert_eq!(s.accel_body_ms2, [0.11, -0.22, -9.71]);
        assert_eq!(s.gyro_body_rads, [0.001, -0.002, 0.003]);
    }

    /// SPEC §10.2: sample variance of 100k noise draws falls within the
    /// chi-square 99% band of the configured sigma. Isolate the white-noise
    /// path with zero walk/turn-on and unit scale, zero truth.
    #[test]
    fn accel_noise_variance_within_chi_square_99_band() {
        let p = ImuParams {
            gyro_noise_density: 0.0,
            gyro_bias_walk: 0.0,
            gyro_turnon_sigma: 0.0,
            gyro_scale_sigma: 0.0,
            gyro_misalign_deg: 0.0,
            accel_noise_density: 0.0025,
            accel_bias_walk: 0.0,
            accel_turnon_sigma: 0.0,
            accel_scale_sigma: 0.0,
            accel_misalign_deg: 0.0,
        };
        let mut turn_on = Pcg64::new(42, 1);
        let mut imu = ImuModel::new(p, &mut turn_on);
        let mut noise = Pcg64::new(42, 0);
        let n = 100_000_usize;
        let rate = 200.0;
        // Expected per-sample sigma at 200 Hz.
        let sigma = 0.0025 * libm::sqrt(rate);
        let mut sum = 0.0;
        let mut sum2 = 0.0;
        for _ in 0..n {
            let s = imu.sample(&[0.0; 3], &[0.0; 3], 0.005, rate, &mut noise);
            let x = s.accel_body_ms2[0];
            sum += x;
            sum2 += x * x;
        }
        let mean = sum / n as f64;
        let var = sum2 / n as f64 - mean * mean;
        // Var(sample variance) ~ 2 sigma^4 / N (normal); 99% band ~ 2.576 sd.
        let sd_of_var = sigma * sigma * libm::sqrt(2.0 / n as f64);
        let lo = sigma * sigma - 2.576 * sd_of_var;
        let hi = sigma * sigma + 2.576 * sd_of_var;
        assert!(var >= lo && var <= hi, "var {var:.3e} outside [{lo:.3e},{hi:.3e}]");
    }

    /// At rest, level: the noisy accel is centered on (0, 0, -g) and the
    /// gyro on zero (FRD sign convention, SPEC §3.6/§6.3; ADR-005).
    #[test]
    fn at_rest_reads_minus_g_on_z() {
        let mut turn_on = Pcg64::new(7, 1);
        let mut imu = ImuModel::new(ImuParams::default(), &mut turn_on);
        let mut noise = Pcg64::new(7, 0);
        let n = 20_000;
        let mut acc = [0.0f64; 3];
        let mut gyr = [0.0f64; 3];
        for _ in 0..n {
            let s = imu.sample(&[0.0, 0.0, -9.80665], &[0.0; 3], 0.005, 200.0, &mut noise);
            for k in 0..3 {
                acc[k] += s.accel_body_ms2[k];
                gyr[k] += s.gyro_body_rads[k];
            }
        }
        for k in 0..3 {
            acc[k] /= n as f64;
            gyr[k] /= n as f64;
        }
        assert!(acc[2] < -9.5 && acc[2] > -10.1, "z mean {}", acc[2]);
        assert!(acc[0].abs() < 0.05, "x mean {}", acc[0]);
        assert!(acc[1].abs() < 0.05, "y mean {}", acc[1]);
        assert!(gyr.iter().all(|&g| g.abs() < 0.005), "gyro mean {gyr:?}");
    }

    /// Determinism: identical seeds -> identical sample sequences.
    #[test]
    fn imu_stream_is_reproducible() {
        let run = || {
            let mut t = Pcg64::new(99, 1);
            let mut imu = ImuModel::new(ImuParams::default(), &mut t);
            let mut n = Pcg64::new(99, 0);
            (0..500)
                .map(|_| imu.sample(&[0.1, 0.0, -9.8], &[0.0, 0.01, 0.0], 0.005, 200.0, &mut n))
                .collect::<Vec<_>>()
        };
        assert_eq!(run(), run());
    }

    /// Different noise-stream seeds -> different sequences (streams are
    /// actually random).
    #[test]
    fn different_seed_diverges() {
        let mut t1 = Pcg64::new(99, 1);
        let mut imu1 = ImuModel::new(ImuParams::default(), &mut t1);
        let mut n1 = Pcg64::new(99, 0);
        let mut t2 = Pcg64::new(100, 1);
        let mut imu2 = ImuModel::new(ImuParams::default(), &mut t2);
        let mut n2 = Pcg64::new(100, 0);
        let s1 = imu1.sample(&[0.0; 3], &[0.0; 3], 0.005, 200.0, &mut n1);
        let s2 = imu2.sample(&[0.0; 3], &[0.0; 3], 0.005, 200.0, &mut n2);
        assert_ne!(s1, s2);
    }
}
