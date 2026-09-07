//! Barometer model (SPEC §6.4): pressure + pressure-altitude at 50 Hz with
//! altitude white noise and a slow bias random walk, consistent through the
//! ISA relation (sitsim-env).
//!
//! The model adds noise to the *altitude* and derives pressure from the
//! noisy altitude via ISA, keeping abs_pressure and pressure_alt exactly
//! consistent (SPEC §6.4: "keeping the two fields consistent").
//! RNG: stream 3; 1 Gaussian per sample (altitude noise) + 1 per sample
//! (bias walk). Baro-drift faults (F-07) add `extra_bias_m`, composed by
//! the tick engine.

use sitsim_core::Pcg64;
use sitsim_env::pressure_field_units;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BaroParams {
    /// Altitude white-noise sigma, m.
    pub noise_m: f64,
    /// Bias random walk, m per sqrt(minute).
    pub walk_m_per_min: f64,
}

impl Default for BaroParams {
    fn default() -> Self {
        BaroParams { noise_m: 0.12, walk_m_per_min: 0.01 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BaroSample {
    /// Absolute pressure, HIL_SENSOR field units.
    pub pressure: f64,
    /// Pressure-derived altitude above the origin, m (noisy + biased).
    pub pressure_alt_m: f64,
}

#[derive(Debug, Clone)]
pub struct BaroModel {
    pub params: BaroParams,
    bias_m: f64,
}

impl BaroModel {
    pub fn new(params: BaroParams) -> Self {
        BaroModel { params, bias_m: 0.0 }
    }

    /// One sample at true altitude `alt_m` above the origin.
    /// `h` is the sample period (used for the walk increment).
    /// `extra_bias_m` is the fault-engine baro drift bias (F-07).
    /// Consumes 2 Gaussians from the baro stream: noise, then walk.
    pub fn sample(&mut self, alt_m: f64, h: f64, extra_bias_m: f64, rng: &mut Pcg64) -> BaroSample {
        // Bias walk in m per sqrt(minute) -> per sqrt(second) increment.
        let walk_per_s = self.params.walk_m_per_min / 60.0_f64.sqrt();
        self.bias_m += walk_per_s * libm::sqrt(h) * rng.next_gaussian();
        let noisy_alt = alt_m + self.bias_m + extra_bias_m
            + self.params.noise_m * rng.next_gaussian();
        BaroSample {
            pressure: pressure_field_units(noisy_alt),
            pressure_alt_m: noisy_alt,
        }
    }

    /// Current internal bias (test/inspection).
    pub fn bias(&self) -> f64 {
        self.bias_m
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sitsim_env::pressure_alt_m;

    /// Consistency: pressure_alt inverts pressure exactly.
    #[test]
    fn pressure_and_altitude_consistent() {
        let mut rng = Pcg64::new(21, 3);
        let mut b = BaroModel::new(BaroParams { noise_m: 0.5, walk_m_per_min: 0.0 });
        for &alt in &[0.0, 5.0, 50.0, -10.0] {
            let s = b.sample(alt, 0.02, 0.0, &mut rng);
            let back = pressure_alt_m(s.pressure);
            assert!((back - s.pressure_alt_m).abs() < 1e-6, "alt {alt}: {back} vs {}", s.pressure_alt_m);
        }
    }

    /// Noise sigma: std of the reported altitude ~ noise_m (walk off).
    #[test]
    fn altitude_noise_statistics() {
        let mut rng = Pcg64::new(33, 3);
        let mut b = BaroModel::new(BaroParams { noise_m: 0.12, walk_m_per_min: 0.0 });
        let n = 100_000;
        let mut sum = 0.0;
        let mut sum2 = 0.0;
        for _ in 0..n {
            let s = b.sample(0.0, 0.02, 0.0, &mut rng);
            sum += s.pressure_alt_m;
            sum2 += s.pressure_alt_m * s.pressure_alt_m;
        }
        let mean = sum / n as f64;
        let sd = libm::sqrt(sum2 / n as f64 - mean * mean);
        assert!(mean.abs() < 0.01, "mean {mean}");
        assert!((sd - 0.12).abs() < 0.005, "sd {sd}");
    }

    /// Fault bias (F-07) passes straight through to the reported altitude.
    #[test]
    fn fault_bias_adds_to_altitude() {
        let mut rng = Pcg64::new(9, 3);
        let mut b = BaroModel::new(BaroParams { noise_m: 0.0, walk_m_per_min: 0.0 });
        let s = b.sample(0.0, 0.02, 2.5, &mut rng);
        assert!((s.pressure_alt_m - 2.5).abs() < 1e-12);
    }
}
