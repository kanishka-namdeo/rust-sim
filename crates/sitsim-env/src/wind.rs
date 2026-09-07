//! Wind and turbulence (SPEC §6.1).
//!
//! Steady wind (constant NED vector from the scenario) plus a first-order
//! Gauss-Markov turbulence process per axis — an approximation to the
//! Dryden spectra (MIL-HDBK-1797 family) that is a discrete-time *exact*
//! solution of the underlying Ornstein-Uhlenbeck SDE:
//!
//! ```text
//! w_dot = -w / tau + sigma * sqrt(2 / tau) * n(t),  n ~ N(0, 1) white
//! ```
//!
//! Exact discretization at step h:
//! ```text
//! w[k+1] = phi * w[k] + sigma * sqrt(1 - phi^2) * n[k],
//! phi = exp(-h / tau)
//! ```
//! which is stable for any h (SPEC §6.1: "a discrete-time exact solution
//! (stable for any h), which a raw filtered-white-noise approach is not").
//!
//! Horizontal axes use tau = 1.5 s, the vertical axis 1.0 s (low-altitude
//! values). Intensity presets: light 1.0, moderate 2.5, severe 6.0 m/s.

use sitsim_core::Pcg64;

/// Turbulence intensity presets (SPEC §6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Turbulence {
    Off,
    Light,
    Moderate,
    Severe,
}

impl Turbulence {
    /// sigma_d in m/s for the Gauss-Markov process.
    pub fn sigma_ms(&self) -> f64 {
        match self {
            Turbulence::Off => 0.0,
            Turbulence::Light => 1.0,
            Turbulence::Moderate => 2.5,
            Turbulence::Severe => 6.0,
        }
    }
}

/// Horizontal-axis time constant (s).
const TAU_H: f64 = 1.5;
/// Vertical-axis time constant (s).
const TAU_V: f64 = 1.0;

/// Steady wind + turbulence generator. `step` consumes exactly three
/// Gaussians per call from the wind RNG stream (stream 5, SPEC §8.1) in a
/// fixed axis order (north, east, down) — consumption order is part of the
/// determinism contract.
#[derive(Debug, Clone)]
pub struct WindModel {
    steady_ned: [f64; 3],
    sigma: f64,
    turb: [f64; 3],
    // Cached exact-discretization coefficients per axis.
    phi: [f64; 2],
    q: [f64; 2],
}

impl WindModel {
    pub fn new(steady_ned: [f64; 3], turbulence: Turbulence) -> Self {
        WindModel {
            steady_ned,
            sigma: turbulence.sigma_ms(),
            turb: [0.0; 3],
            phi: [1.0; 2],
            q: [0.0; 2],
        }
        .with_step(0.005)
    }

    /// Recompute the exact-discretization coefficients for tick period `h`.
    fn recompute(&mut self, h: f64) {
        let h = h.max(1e-9);
        for (i, &tau) in [TAU_H, TAU_V].iter().enumerate() {
            let phi = libm::exp(-h / tau);
            self.phi[i] = phi;
            self.q[i] = self.sigma * libm::sqrt(1.0 - phi * phi);
        }
    }

    /// Recompute for tick period `h` (builder form). Must be called whenever
    /// the tick rate changes (the first call happens in `new`).
    pub fn with_step(mut self, h: f64) -> Self {
        self.recompute(h);
        self
    }

    /// Advance the turbulence process one step of `h` seconds and return the
    /// total NED wind (steady + turbulence). Gust terms (fault F-08) are
    /// added by the caller (the tick engine), not here.
    pub fn step(&mut self, h: f64, rng: &mut Pcg64) -> [f64; 3] {
        // Coefficients depend on h; recompute lazily if the step changed
        // (only happens on rate reconfiguration; keeps the model honest).
        let need = |phi: f64, tau: f64| (phi - libm::exp(-h / tau)).abs() > 1e-12;
        if need(self.phi[0], TAU_H) || need(self.phi[1], TAU_V) {
            self.recompute(h);
        }
        // Axis order north, east, down: two horizontal (TAU_H) then vertical.
        for (k, coef) in [(0usize, 0usize), (1, 0), (2, 1)] {
            self.turb[k] = self.phi[coef] * self.turb[k] + self.q[coef] * rng.next_gaussian();
        }
        [
            self.steady_ned[0] + self.turb[0],
            self.steady_ned[1] + self.turb[1],
            self.steady_ned[2] + self.turb[2],
        ]
    }

    /// Steady component only (test/inspection).
    pub fn steady(&self) -> [f64; 3] {
        self.steady_ned
    }

    /// Turbulence component only (test/inspection).
    pub fn turbulence_state(&self) -> [f64; 3] {
        self.turb
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stationary statistics: after burn-in, the sample std of each axis is
    /// within a few percent of sigma (500 s at 200 Hz -> tight).
    #[test]
    fn turbulence_stationary_std_matches_preset() {
        let h = 0.005;
        let mut w = WindModel::new([0.0; 3], Turbulence::Moderate).with_step(h);
        let mut rng = Pcg64::new(1234, 5);
        let n = 100_000;
        let mut sum = [0.0f64; 3];
        let mut sum2 = [0.0f64; 3];
        for _ in 0..n {
            let _ = w.step(h, &mut rng);
            let t = w.turbulence_state();
            for k in 0..3 {
                sum[k] += t[k];
                sum2[k] += t[k] * t[k];
            }
        }
        for k in 0..3 {
            let mean = sum[k] / n as f64;
            let var = sum2[k] / n as f64 - mean * mean;
            let sd = libm::sqrt(var);
            assert!(
                (sd - 2.5).abs() < 0.1,
                "axis {k} stationary sd {sd} vs sigma 2.5"
            );
        }
    }

    /// Mean of the total wind over long runs = steady vector (turbulence is
    /// zero-mean; so horizontal drift tests are meaningful).
    #[test]
    fn total_wind_mean_is_steady() {
        let h = 0.005;
        let mut w = WindModel::new([1.5, 0.0, 0.0], Turbulence::Light).with_step(h);
        let mut rng = Pcg64::new(42, 5);
        let n = 200_000;
        let mut sum = [0.0f64; 3];
        for _ in 0..n {
            let v = w.step(h, &mut rng);
            for k in 0..3 {
                sum[k] += v[k];
            }
        }
        for k in 0..3 {
            assert!((sum[k] / n as f64 - w.steady()[k]).abs() < 0.15, "axis {k}");
        }
    }

    /// Off preset: no turbulence, steady only.
    #[test]
    fn off_preset_returns_steady_exactly() {
        let mut w = WindModel::new([2.0, -1.0, 0.0], Turbulence::Off);
        let mut rng = Pcg64::new(1, 1);
        let v = w.step(0.005, &mut rng);
        assert_eq!(v, [2.0, -1.0, 0.0]);
    }

    /// Determinism: same seed -> identical sequence.
    #[test]
    fn wind_stream_is_reproducible() {
        let h = 0.005;
        let run = || {
            let mut w = WindModel::new([0.0; 3], Turbulence::Severe).with_step(h);
            let mut rng = Pcg64::new(7, 5);
            (0..1000).map(|_| w.step(h, &mut rng)).collect::<Vec<_>>()
        };
        assert_eq!(run(), run());
    }
}
