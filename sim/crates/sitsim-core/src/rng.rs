//! Deterministic RNG (PCG64) and per-stream seeding (SPEC §8.1).
//!
//! The generator is PCG-XSL-RR 128/64 ("PCG64", the same output function as
//! numpy's default PCG64), implemented locally so the sim owns the exact
//! algorithm — bit-exact reproducibility must not depend on a dependency
//! upgrade.
//!
//! Stream allocation (SPEC §8.1): stream 0 IMU noise, 1 IMU turn-on draw,
//! 2 magnetometer, 3 barometer, 4 GPS, 5 wind, 6 initial-state perturbation.
//! Streams are seeded from the master seed with per-stream offsets so that
//! removing one sensor from a scenario changes no other stream.

#[derive(Clone, Debug)]
pub struct Pcg64 {
    state: u128,
    inc: u128,
    /// Box-Muller spare value (Gaussian pairs).
    spare: Option<f64>,
}

/// PCG 128/64 LCG multiplier (from the PCG reference implementation).
const PCG_MULT: u128 = 0x2360_ED05_1FC6_5DA4_4385_DF64_9FCC_F645;

impl Pcg64 {
    /// Deterministic construction (reference `pcg64_init` pattern):
    /// `inc = (stream << 1) | 1`, one step, `state += seed`, one step.
    pub fn new(seed: u64, stream: u64) -> Self {
        let mut r = Pcg64 { state: 0, inc: ((stream as u128) << 1) | 1, spare: None };
        r.step();
        r.state = r.state.wrapping_add(seed as u128);
        r.step();
        r
    }

    fn step(&mut self) {
        self.state = self.state.wrapping_mul(PCG_MULT).wrapping_add(self.inc);
    }

    /// Raw 64-bit output (XSL-RR).
    pub fn next_u64(&mut self) -> u64 {
        self.step();
        let rot = (self.state >> 122) as u32;
        let xsl = (self.state as u64) ^ ((self.state >> 64) as u64);
        xsl.rotate_right(rot)
    }

    /// Uniform in [0, 1).
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / ((1u64 << 53) as f64))
    }

    /// Uniform in [min, max).
    pub fn next_range(&mut self, min: f64, max: f64) -> f64 {
        min + (max - min) * self.next_f64()
    }

    /// Standard normal via Box-Muller; consumes uniforms in pairs, caching
    /// the second value (fixed, documented consumption order for
    /// reproducibility).
    pub fn next_gaussian(&mut self) -> f64 {
        if let Some(s) = self.spare.take() {
            return s;
        }
        // u in (0, 1]: avoid log(0).
        let mut u1 = self.next_f64();
        if u1 <= 0.0 {
            u1 = f64::MIN_POSITIVE;
        }
        let u2 = self.next_f64();
        let mag = libm::sqrt(-2.0 * libm::log(u1));
        let arg = 2.0 * core::f64::consts::PI * u2;
        let (s, c) = (libm::sin(arg), libm::cos(arg));
        let z0 = mag * c;
        self.spare = Some(mag * s);
        z0
    }
}

/// Golden-ratio-of-2 constant used to decorrelate stream seeds.
const PHI2: u64 = 0x9E37_79B9_7F4A_7C15;

/// The per-stream RNG bundle (SPEC §8.1).
#[derive(Clone, Debug)]
pub struct RngStreams {
    /// Stream 0: IMU sample noise.
    pub imu_noise: Pcg64,
    /// Stream 1: IMU turn-on draws (bias, scale factor, misalignment).
    pub imu_turn_on: Pcg64,
    /// Stream 2: magnetometer.
    pub mag: Pcg64,
    /// Stream 3: barometer.
    pub baro: Pcg64,
    /// Stream 4: GPS.
    pub gps: Pcg64,
    /// Stream 5: wind turbulence.
    pub wind: Pcg64,
    /// Stream 6: initial-state perturbation.
    pub initial: Pcg64,
}

impl RngStreams {
    /// Seed each stream i with `master ^ (i * PHI2)` on stream id `i`.
    pub fn new(master: u64) -> Self {
        let s = |i: u64| Pcg64::new(master ^ i.wrapping_mul(PHI2), i);
        RngStreams {
            imu_noise: s(0),
            imu_turn_on: s(1),
            mag: s(2),
            baro: s(3),
            gps: s(4),
            wind: s(5),
            initial: s(6),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcg64_reproducible() {
        let mut a = Pcg64::new(42, 0);
        let mut b = Pcg64::new(42, 0);
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn streams_are_independent() {
        // Different streams must diverge immediately (removing one sensor
        // changes no other stream is the contract; distinct sequences is the
        // observable precondition).
        let mut r = RngStreams::new(42);
        let firsts = [
            r.imu_noise.next_u64(),
            r.imu_turn_on.next_u64(),
            r.mag.next_u64(),
            r.baro.next_u64(),
            r.gps.next_u64(),
            r.wind.next_u64(),
            r.initial.next_u64(),
        ];
        for i in 0..firsts.len() {
            for j in i + 1..firsts.len() {
                assert_ne!(firsts[i], firsts[j], "streams {} and {} collided", i, j);
            }
        }
    }

    #[test]
    fn uniform_bounds_and_mean() {
        let mut r = Pcg64::new(7, 1);
        let n = 100_000;
        let mut sum = 0.0;
        let mut min = f64::INFINITY;
        let mut max = f64::NEG_INFINITY;
        for _ in 0..n {
            let v = r.next_f64();
            sum += v;
            min = min.min(v);
            max = max.max(v);
        }
        assert!(min >= 0.0 && max < 1.0);
        let mean = sum / n as f64;
        assert!((mean - 0.5).abs() < 0.01, "mean {} off", mean);
    }

    #[test]
    fn gaussian_mean_and_variance() {
        let mut r = Pcg64::new(9, 2);
        let n = 200_000;
        let mut sum = 0.0;
        let mut sum2 = 0.0;
        for _ in 0..n {
            let z = r.next_gaussian();
            sum += z;
            sum2 += z * z;
        }
        let mean = sum / n as f64;
        let var = sum2 / n as f64 - mean * mean;
        assert!(mean.abs() < 0.02, "mean {}", mean);
        // 200k samples: sigma of the variance estimate ~ sqrt(2/n) ~ 0.0032.
        assert!((var - 1.0).abs() < 0.02, "variance {}", var);
    }
}
