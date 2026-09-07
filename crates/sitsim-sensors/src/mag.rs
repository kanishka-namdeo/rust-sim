//! Magnetometer model (SPEC §6.4): body-frame truth field plus hard-iron
//! offset, soft-iron perturbation, and white noise.
//!
//! Turn-on draws (stream 2): hard-iron 3-vector (sigma `hard_iron_gauss`),
//! soft-iron symmetric perturbation (sigma 0.005 per element, spec: "small
//! random symmetric perturbation"). Per-sample: 3 white-noise draws
//! (sigma `noise_gauss`).

use sitsim_core::Pcg64;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MagParams {
    /// White noise sigma, gauss.
    pub noise_gauss: f64,
    /// Hard-iron offset draw sigma, gauss.
    pub hard_iron_gauss: f64,
}

impl Default for MagParams {
    fn default() -> Self {
        MagParams { noise_gauss: 0.002, hard_iron_gauss: 0.01 }
    }
}

#[derive(Debug, Clone)]
pub struct MagModel {
    pub params: MagParams,
    hard_iron: [f64; 3],
    /// Soft-iron = I + small symmetric perturbation.
    soft_iron: [[f64; 3]; 3],
}

impl MagModel {
    pub fn new(params: MagParams, rng: &mut Pcg64) -> Self {
        let mut hi = [0.0; 3];
        for k in 0..3 {
            hi[k] = params.hard_iron_gauss * rng.next_gaussian();
        }
        let mut si = [[0.0f64; 3]; 3];
        // Symmetric perturbation: draw the upper triangle, mirror it.
        for i in 0..3 {
            for j in i..3 {
                let v = 0.005 * rng.next_gaussian();
                si[i][j] = v;
                si[j][i] = v;
            }
        }
        for i in 0..3 {
            si[i][i] += 1.0;
        }
        MagModel { params, hard_iron: hi, soft_iron: si }
    }

    /// One sample; consumes 3 Gaussians (x, y, z order) from the mag stream.
    pub fn sample(&mut self, truth_body_gauss: &[f64; 3], rng: &mut Pcg64) -> [f64; 3] {
        let mut out = [0.0f64; 3];
        for i in 0..3 {
            out[i] = self.soft_iron[i][0] * truth_body_gauss[0]
                + self.soft_iron[i][1] * truth_body_gauss[1]
                + self.soft_iron[i][2] * truth_body_gauss[2]
                + self.hard_iron[i]
                + self.params.noise_gauss * rng.next_gaussian();
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_error_passthrough() {
        let p = MagParams { noise_gauss: 0.0, hard_iron_gauss: 0.0 };
        let mut rng = Pcg64::new(3, 2);
        let mut m = MagModel::new(p, &mut rng);
        // Soft-iron perturbation is still drawn (spec: "identity plus small
        // random symmetric perturbation"); with hard-iron and noise zero, the
        // output is the perturbed field — verify it stays within ~2% of truth.
        let out = m.sample(&[0.19, 0.01, 0.46], &mut rng);
        for k in 0..3 {
            assert!((out[k] - [0.19, 0.01, 0.46][k]).abs() < 0.02, "axis {k}: {out:?}");
        }
    }

    /// Mean over many samples converges to the soft/hard-iron-transformed
    /// truth; noise sigma shows up in the sample std (loose check).
    #[test]
    fn noise_statistics() {
        let p = MagParams { noise_gauss: 0.002, hard_iron_gauss: 0.0 };
        let mut rng = Pcg64::new(11, 2);
        let mut m = MagModel::new(p, &mut rng);
        let truth = [0.19, 0.01, 0.46];
        let n = 100_000;
        let mut sum = 0.0;
        let mut sum2 = 0.0;
        for _ in 0..n {
            let s = m.sample(&truth, &mut rng);
            sum += s[1];
            sum2 += s[1] * s[1];
        }
        let mean = sum / n as f64;
        let var = sum2 / n as f64 - mean * mean;
        let sd = libm::sqrt(var);
        // Soft-iron shifts the mean by a few milligauss; noise dominates sd.
        assert!((mean - truth[1]).abs() < 0.006, "y mean {mean}");
        assert!((sd - 0.002).abs() < 0.0002, "y sd {sd}");
    }

    #[test]
    fn reproducible() {
        let run = || {
            let mut rng = Pcg64::new(5, 2);
            let mut m = MagModel::new(MagParams::default(), &mut rng);
            (0..100).map(|_| m.sample(&[0.2, 0.0, 0.4], &mut rng)).collect::<Vec<_>>()
        };
        assert_eq!(run(), run());
    }
}
