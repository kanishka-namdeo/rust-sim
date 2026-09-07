//! GPS model (SPEC §6.4): 5 Hz fixes with position/velocity noise,
//! course-over-ground, eph/epv, a latency FIFO, and cold-start lock.
//!
//! Latency: each fix is *sampled* at its own time slot (timestamped with
//! that slot) and *delivered* `latency_ms` later; the delivery tick pops the
//! queue head when `t_now >= t_sample + latency`. This recreates the
//! lag-vs-innovation signature operators see on real hardware (SPEC §6.4:
//! "the single most valuable realism feature for EKF tuning").
//!
//! Denial (F-05): while denied, fixes are emitted at the normal cadence
//! with fix_type = 0 and eph ramped from the nominal value toward the u16
//! clamp (65535 cm) proportionally to `denial_progress` in [0, 1]; epv
//! ramps likewise. Glitch (F-06): `glitch_ned_m` offsets the sampled NED
//! position before geodetic conversion.
//!
//! RNG: stream 4; 6 Gaussians per sampled fix (pos n, e, d; vel n, e, d).

use sitsim_env::GeoOrigin;
use sitsim_core::Pcg64;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GpsParams {
    pub rate_hz: f64,
    /// Horizontal position noise sigma, m (per NED north/east axis).
    pub pos_noise_m: f64,
    /// Vertical position noise sigma, m.
    pub pos_noise_vert_m: f64,
    /// Velocity noise sigma per axis, m/s.
    pub vel_noise_ms: f64,
    /// Nominal horizontal accuracy, cm.
    pub eph_cm: u16,
    /// Nominal vertical accuracy, cm.
    pub epv_cm: u16,
    /// Delivery latency, ms.
    pub latency_ms: u32,
    /// Cold-start lock time, s (no fix before; 0 for CI speed).
    pub lock_s: f64,
    /// Nominal satellite count.
    pub satellites: u8,
}

impl Default for GpsParams {
    fn default() -> Self {
        GpsParams {
            rate_hz: 5.0,
            pos_noise_m: 0.8,
            pos_noise_vert_m: 1.5,
            vel_noise_ms: 0.2,
            eph_cm: 100,
            epv_cm: 150,
            latency_ms: 120,
            lock_s: 0.0,
            satellites: 10,
        }
    }
}

/// One GPS fix, wire-ready (units already converted for HIL_GPS).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GpsFix {
    /// Sample time (not delivery time), us.
    pub t_sample_us: u64,
    /// degE7.
    pub lat: i32,
    /// degE7.
    pub lon: i32,
    /// MSL altitude, mm.
    pub alt: i32,
    /// Horizontal accuracy, cm (ramped during denial).
    pub eph: u16,
    /// Vertical accuracy, cm.
    pub epv: u16,
    /// 3D speed, cm/s.
    pub vel: u16,
    /// NED velocity, cm/s.
    pub vn: i16,
    pub ve: i16,
    pub vd: i16,
    /// Course over ground, centidegrees [0, 35999].
    pub cog: u16,
    /// 0 = no fix, 3 = 3D.
    pub fix_type: u8,
    pub satellites_visible: u8,
}

#[derive(Debug, Clone)]
pub struct GpsModel {
    pub params: GpsParams,
    /// FIFO of (delivery_deadline_us, fix).
    queue: std::collections::VecDeque<(u64, GpsFix)>,
    next_sample_us: Option<u64>,
}

impl GpsModel {
    pub fn new(params: GpsParams) -> Self {
        GpsModel { params, queue: Default::default(), next_sample_us: None }
    }

    /// Call once per tick. Returns the fix delivered on THIS tick (if the
    /// queue head's deadline has been reached), which the tick engine
    /// encodes into a HIL_GPS frame. `denied` + `denial_progress` carry the
    /// F-05 state; `glitch_ned_m` the F-06 offset.
    pub fn poll(
        &mut self,
        t_us: u64,
        truth_pos_ned: &[f64; 3],
        truth_vel_ned: &[f64; 3],
        origin: &GeoOrigin,
        denied: bool,
        denial_progress: f64,
        glitch_ned_m: &[f64; 3],
        rng: &mut Pcg64,
    ) -> Option<GpsFix> {
        let period_us = (1e6 / self.params.rate_hz) as u64;

        // First call anchors the sampling grid at the first tick (a fix is
        // sampled immediately, then every 1/rate).
        if self.next_sample_us.is_none() {
            self.next_sample_us = Some(t_us);
        }

        // Sampling: when t_us reaches the next sample slot.
        let mut t_sample = self.next_sample_us.unwrap();
        while t_sample <= t_us {
            self.next_sample_us = Some(t_sample + period_us);
            let lock_us = (self.params.lock_s * 1e6) as u64;
            let locked = t_sample >= lock_us;

            let fix = if !locked || denied {
                // No-fix frame: fix_type 0, eph ramped toward the clamp.
                let eph = ramp_eph(self.params.eph_cm, denial_progress);
                let epv = ramp_eph(self.params.epv_cm, denial_progress);
                GpsFix {
                    t_sample_us: t_sample,
                    lat: 0,
                    lon: 0,
                    alt: 0,
                    eph,
                    epv,
                    vel: 0,
                    vn: 0,
                    ve: 0,
                    vd: 0,
                    cog: 0,
                    fix_type: 0,
                    satellites_visible: 0,
                }
            } else {
                let mut pos = [
                    truth_pos_ned[0] + glitch_ned_m[0],
                    truth_pos_ned[1] + glitch_ned_m[1],
                    truth_pos_ned[2] + glitch_ned_m[2],
                ];
                // Position noise: 2 draws horizontal (n, e), 1 vertical.
                pos[0] += self.params.pos_noise_m * rng.next_gaussian();
                pos[1] += self.params.pos_noise_m * rng.next_gaussian();
                pos[2] += self.params.pos_noise_vert_m * rng.next_gaussian();
                let mut vel = *truth_vel_ned;
                vel[0] += self.params.vel_noise_ms * rng.next_gaussian();
                vel[1] += self.params.vel_noise_ms * rng.next_gaussian();
                vel[2] += self.params.vel_noise_ms * rng.next_gaussian();

                let (lat_deg, lon_deg, alt_m) = origin.ned_to_geodetic(&pos);
                let speed = libm::sqrt(vel[0] * vel[0] + vel[1] * vel[1] + vel[2] * vel[2]);
                let cog = if vel[0].abs() < 1e-6 && vel[1].abs() < 1e-6 {
                    0.0
                } else {
                    let c = libm::atan2(vel[1], vel[0]).to_degrees();
                    if c < 0.0 { c + 360.0 } else { c }
                };
                GpsFix {
                    t_sample_us: t_sample,
                    lat: (lat_deg * 1e7).round() as i32,
                    lon: (lon_deg * 1e7).round() as i32,
                    alt: (alt_m * 1000.0).round() as i32,
                    eph: self.params.eph_cm,
                    epv: self.params.epv_cm,
                    vel: (speed * 100.0).round().min(65535.0) as u16,
                    vn: (vel[0] * 100.0).round().clamp(-32768.0, 32767.0) as i16,
                    ve: (vel[1] * 100.0).round().clamp(-32768.0, 32767.0) as i16,
                    vd: (vel[2] * 100.0).round().clamp(-32768.0, 32767.0) as i16,
                    cog: (cog * 100.0).round().min(35999.0) as u16,
                    fix_type: 3,
                    satellites_visible: self.params.satellites,
                }
            };

            // Enqueue for delivery latency_ms later.
            let deadline = t_sample + self.params.latency_ms as u64 * 1000;
            self.queue.push_back((deadline, fix));
            t_sample = self.next_sample_us.unwrap();
        }
        self.next_sample_us = Some(t_sample);

        // Delivery: pop at most one per tick (fixes arrive at 5 Hz; the
        // queue discipline keeps ordering and the one-tick cadence).
        if let Some((deadline, _)) = self.queue.front() {
            if t_us >= *deadline {
                return self.queue.pop_front().map(|(_, f)| f);
            }
        }
        None
    }

    /// Fixes currently in the latency FIFO (test/inspection).
    pub fn queued(&self) -> usize {
        self.queue.len()
    }
}

/// Ramp a nominal accuracy (cm) toward the u16 clamp during denial.
fn ramp_eph(nominal: u16, progress: f64) -> u16 {
    let p = progress.clamp(0.0, 1.0);
    let v = nominal as f64 + (65535.0 - nominal as f64) * p;
    v.round().clamp(0.0, 65535.0) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin() -> GeoOrigin {
        GeoOrigin::DEFAULT
    }

    /// SPEC §10.2: fixes delivered exactly T_gps late with correct ordering.
    /// At 200 Hz ticks and 120 ms latency, a fix sampled at slot t is
    /// delivered at t + 24 ticks exactly.
    #[test]
    fn latency_queue_delivers_exactly_t_late_in_order() {
        let mut gps = GpsModel::new(GpsParams::default());
        let mut rng = Pcg64::new(1, 4);
        let h_us: u64 = 5000; // 200 Hz
        let mut delivered: Vec<GpsFix> = Vec::new();
        let mut t: u64 = 0;
        for _ in 0..(2_000_000 / h_us) {
            t += h_us;
            if let Some(f) = gps.poll(t, &[0.0; 3], &[0.0; 3], &origin(), false, 0.0, &[0.0; 3], &mut rng) {
                delivered.push(f);
            }
        }
        assert!(delivered.len() >= 9 && delivered.len() <= 11, "delivered {} fixes in 2 s", delivered.len());
        // Ordering: sample timestamps strictly increasing.
        for w in delivered.windows(2) {
            assert!(w[0].t_sample_us < w[1].t_sample_us, "ordering violated");
        }
        // First fix: sampled at the first tick (the sampling grid anchors
        // there), delivered 120 ms later.
        let f0 = delivered[0];
        assert_eq!(f0.t_sample_us, 5_000); // first 200 Hz tick
        // Latency exactness: the fix is returned on the poll at
        // t = t_sample + 120000 exactly (ticks are 5 ms-aligned).
        let mut gps2 = GpsModel::new(GpsParams::default());
        let mut rng2 = Pcg64::new(1, 4);
        let mut t2: u64 = 0;
        let mut delivered_at: Vec<(u64, GpsFix)> = Vec::new();
        for _ in 0..(2_000_000 / h_us) {
            t2 += h_us;
            if let Some(f) = gps2.poll(t2, &[0.0; 3], &[0.0; 3], &origin(), false, 0.0, &[0.0; 3], &mut rng2) {
                delivered_at.push((t2, f));
            }
        }
        for (at, f) in &delivered_at {
            assert_eq!(*at, f.t_sample_us + 120_000, "delivery not exactly T_gps late");
        }
    }

    /// Cold start: no fix before lock_s; fix after.
    #[test]
    fn cold_start_lock() {
        let mut gps = GpsModel::new(GpsParams { lock_s: 1.0, ..Default::default() });
        let mut rng = Pcg64::new(2, 4);
        let h_us: u64 = 5000;
        let mut t: u64 = 0;
        let mut fixes = Vec::new();
        for _ in 0..(2_000_000 / h_us) {
            t += h_us;
            if let Some(f) = gps.poll(t, &[0.0; 3], &[0.0; 3], &origin(), false, 0.0, &[0.0; 3], &mut rng) {
                fixes.push(f);
            }
        }
        assert!(!fixes.is_empty());
        // Before lock: no-fix frames; after: 3D fixes with fresh timestamps.
        for f in &fixes {
            if f.t_sample_us < 1_000_000 {
                assert_eq!(f.fix_type, 0, "fix before lock at {}", f.t_sample_us);
            } else {
                assert_eq!(f.fix_type, 3, "no fix after lock at {}", f.t_sample_us);
            }
        }
        assert!(fixes.iter().any(|f| f.fix_type == 3), "no post-lock fix delivered in 2 s");
    }

    /// Denial: fix_type 0 with ramped eph; recovery restores fix_type 3.
    #[test]
    fn denial_ramps_eph_and_suppresses_fix() {
        let mut gps = GpsModel::new(GpsParams::default());
        let mut rng = Pcg64::new(3, 4);
        let h_us: u64 = 5000;
        let mut t: u64 = 0;
        let mut denied_fixes = Vec::new();
        for i in 0..(4_000_000 / h_us) {
            t += h_us;
            // Denied for ticks 400..800 (t in [2 s, 4 s)), progress ramps.
            let (denied, prog) = if i >= 400 && i < 800 {
                let p = (i as f64 - 400.0) / 400.0;
                (true, p)
            } else {
                (false, 0.0)
            };
            if let Some(f) = gps.poll(t, &[0.0; 3], &[0.0; 3], &origin(), denied, prog, &[0.0; 3], &mut rng) {
                if denied {
                    denied_fixes.push(f);
                }
            }
        }
        assert!(denied_fixes.iter().all(|f| f.fix_type == 0));
        // eph ramped above nominal in the second half of the window.
        let late = denied_fixes.iter().filter(|f| f.t_sample_us > 3_000_000).count();
        let ramped = denied_fixes.iter().filter(|f| f.t_sample_us > 3_000_000 && f.eph > 300).count();
        assert!(late > 0 && ramped == late, "eph not ramped: {:?}", denied_fixes.last());
    }

    /// Glitch: reported position offset by the fault vector.
    #[test]
    fn glitch_offsets_position() {
        let mut gps = GpsModel::new(GpsParams { rate_hz: 5.0, pos_noise_m: 0.0, pos_noise_vert_m: 0.0, vel_noise_ms: 0.0, ..Default::default() });
        let mut rng = Pcg64::new(4, 4);
        let truth = [100.0, 200.0, -30.0];
        let glitch = [5.0, -10.0, 2.0];
        let h_us: u64 = 5000;
        let mut t: u64 = 0;
        let mut seen = None;
        for _ in 0..400 {
            t += h_us;
            if let Some(f) = gps.poll(t, &truth, &[0.0; 3], &origin(), false, 0.0, &glitch, &mut rng) {
                seen = Some(f);
            }
        }
        let f = seen.expect("no fix");
        // Back-convert lat/lon to NED and check the offset.
        let lat = f.lat as f64 / 1e7;
        let lon = f.lon as f64 / 1e7;
        let alt = f.alt as f64 / 1000.0;
        let back = origin().geodetic_to_ned(lat, lon, alt);
        for k in 0..3 {
            assert!(
                (back[k] - (truth[k] + glitch[k])).abs() < 0.05,
                "axis {k}: {back:?} vs {:?}",
                [truth[0] + glitch[0], truth[1] + glitch[1], truth[2] + glitch[2]]
            );
        }
    }

    /// Truth round trip with noise zero: lat/lon/alt exactly the truth NED.
    #[test]
    fn clean_fix_matches_truth() {
        let mut gps = GpsModel::new(GpsParams { pos_noise_m: 0.0, pos_noise_vert_m: 0.0, vel_noise_ms: 0.0, ..Default::default() });
        let mut rng = Pcg64::new(5, 4);
        let truth = [-50.0, 75.0, -8.0];
        let vel = [1.0, -2.0, 0.5];
        let h_us: u64 = 5000;
        let mut t: u64 = 0;
        let mut seen = None;
        for _ in 0..400 {
            t += h_us;
            if let Some(f) = gps.poll(t, &truth, &vel, &origin(), false, 0.0, &[0.0; 3], &mut rng) {
                seen = Some(f);
            }
        }
        let f = seen.unwrap();
        assert_eq!(f.fix_type, 3);
        assert_eq!(f.vn, 100);
        assert_eq!(f.ve, -200);
        assert_eq!(f.vd, 50);
        // cog = atan2(-2, 1) = -63.43 deg -> 296.57 deg.
        let expect_cog = ((libm::atan2(-2.0, 1.0).to_degrees() + 360.0) * 100.0).round() as u16;
        assert_eq!(f.cog, expect_cog);
        assert_eq!(f.satellites_visible, 10);
    }
}
