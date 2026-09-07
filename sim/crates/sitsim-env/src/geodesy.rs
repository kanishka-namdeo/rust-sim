//! Flat-earth-anchored geodesy (SPEC §6.2).
//!
//! The local NED frame is the exact tangent plane at the scenario origin
//! (WGS84). The forward conversion NED -> geodetic goes through ECEF
//! (rotate the tangent-plane offset into the ECEF frame, translate, then
//! exact ECEF -> geodetic) so that the round trip NED -> geodetic -> NED is
//! exact — the SPEC §6.2 requirement of < 1 mm within a 10 km square cannot
//! be met by the M/N linearization alone, whose curvature error reaches
//! ~5 m at the square's corners (kept here as `ned_to_geodetic_linear` for
//! comparison; see ADR-004).
//!
//! The ECEF -> geodetic step uses Bowring's closed form (sub-millimeter
//! everywhere near the surface) and the WGS84 prime-vertical radius of
//! curvature N, which is the "prime-radius" reference of SPEC §6.2.

/// WGS84 semi-major axis (m).
pub const WGS84_A: f64 = 6378137.0;
/// WGS84 first eccentricity squared.
pub const WGS84_E2: f64 = 0.0066943799901413165;
/// WGS84 semi-minor axis (m).
pub const WGS84_B: f64 = WGS84_A * (1.0 - WGS84_F);
/// WGS84 flattening.
pub const WGS84_F: f64 = 1.0 / 298.257223563;
/// Second eccentricity squared, (a^2-b^2)/b^2.
pub const WGS84_EP2: f64 = (WGS84_A * WGS84_A - WGS84_B * WGS84_B) / (WGS84_B * WGS84_B);

/// Local NED origin (geodetic fix; SPEC §9.1 `[vehicle].origin`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeoOrigin {
    pub lat_deg: f64,
    pub lon_deg: f64,
    pub alt_m: f64,
}

impl GeoOrigin {
    /// Default origin: the PX4 test field (SPEC §6.2).
    pub const DEFAULT: GeoOrigin =
        GeoOrigin { lat_deg: 47.397770, lon_deg: 8.545580, alt_m: 500.0 };

    fn lat_rad(&self) -> f64 {
        self.lat_deg.to_radians()
    }

    fn lon_rad(&self) -> f64 {
        self.lon_deg.to_radians()
    }

    /// Meridional (M) and prime-vertical (N) radii of curvature at the
    /// origin latitude.
    pub fn radii(&self) -> (f64, f64) {
        let s = self.lat_rad().sin();
        let denom = 1.0 - WGS84_E2 * s * s;
        let n = WGS84_A / libm::sqrt(denom);
        let m = WGS84_A * (1.0 - WGS84_E2) / (denom * libm::sqrt(denom));
        (m, n)
    }

    /// NED offset -> ECEF position (tangent-plane rotation + translation).
    pub fn ned_to_ecef(&self, ned: &[f64; 3]) -> [f64; 3] {
        let p0 = self.ecef();
        // R^T maps NED -> ECEF (transpose of the ECEF-delta -> NED matrix).
        let lat = self.lat_rad();
        let lon = self.lon_rad();
        let (sl, cl) = (lat.sin(), lat.cos());
        let (so, co) = (lon.sin(), lon.cos());
        [
            p0[0] - sl * co * ned[0] - so * ned[1] - cl * co * ned[2],
            p0[1] - sl * so * ned[0] + co * ned[1] - cl * so * ned[2],
            p0[2] + cl * ned[0] - sl * ned[2],
        ]
    }

    /// ECEF position of the origin.
    pub fn ecef(&self) -> [f64; 3] {
        geodetic_to_ecef(self.lat_deg, self.lon_deg, self.alt_m)
    }

    /// NED meters (n, e, d) -> geodetic (lat_deg, lon_deg, alt_msl), exact
    /// via ECEF (round-trips with [`Self::geodetic_to_ned`]).
    pub fn ned_to_geodetic(&self, ned: &[f64; 3]) -> (f64, f64, f64) {
        let p = self.ned_to_ecef(ned);
        ecef_to_geodetic(&p)
    }

    /// The M/N linear tangent-plane map (SPEC §6.2's literal wording).
    /// Deviates from the ECEF-exact map by up to ~5 m at 5 km (ellipsoid
    /// curvature; see ADR-004) — kept for comparison and small-offset use.
    pub fn ned_to_geodetic_linear(&self, ned: &[f64; 3]) -> (f64, f64, f64) {
        let (m, n) = self.radii();
        let lat = self.lat_rad() + ned[0] / m;
        let lon = self.lon_rad() + ned[1] / (n * self.lat_rad().cos());
        (lat.to_degrees(), lon.to_degrees(), self.alt_m - ned[2])
    }

    /// Geodetic degrees + MSL altitude -> NED meters relative to this origin
    /// (exact via ECEF; inverse of [`Self::ned_to_geodetic`]).
    pub fn geodetic_to_ned(&self, lat_deg: f64, lon_deg: f64, alt_m: f64) -> [f64; 3] {
        let p0 = self.ecef();
        let p = geodetic_to_ecef(lat_deg, lon_deg, alt_m);
        let d = [p[0] - p0[0], p[1] - p0[1], p[2] - p0[2]];
        let lat = self.lat_rad();
        let lon = self.lon_rad();
        let (sl, cl) = (lat.sin(), lat.cos());
        let (so, co) = (lon.sin(), lon.cos());
        [
            -sl * co * d[0] - sl * so * d[1] + cl * d[2],
            -so * d[0] + co * d[1],
            -cl * co * d[0] - cl * so * d[1] - sl * d[2],
        ]
    }
}

/// Geodetic (degrees, MSL meters) -> ECEF (meters), WGS84.
pub fn geodetic_to_ecef(lat_deg: f64, lon_deg: f64, alt_m: f64) -> [f64; 3] {
    let lat = lat_deg.to_radians();
    let lon = lon_deg.to_radians();
    let (sl, cl) = (lat.sin(), lat.cos());
    let (so, co) = (lon.sin(), lon.cos());
    let s2 = sl * sl;
    let n = WGS84_A / libm::sqrt(1.0 - WGS84_E2 * s2);
    let nx = (n + alt_m) * cl;
    [nx * co, nx * so, (n * (1.0 - WGS84_E2) + alt_m) * sl]
}

/// ECEF (meters) -> geodetic (degrees, MSL meters), Bowring's closed form.
/// Accuracy is sub-millimeter for points near the ellipsoid surface.
pub fn ecef_to_geodetic(p: &[f64; 3]) -> (f64, f64, f64) {
    let x = p[0];
    let y = p[1];
    let z = p[2];
    let lon = libm::atan2(y, x);
    let pr = libm::sqrt(x * x + y * y);
    let theta = libm::atan2(z * WGS84_A, pr * WGS84_B);
    let st = theta.sin();
    let ct = theta.cos();
    let num = z + WGS84_EP2 * WGS84_B * st * st * st;
    let den = pr - WGS84_E2 * WGS84_A * ct * ct * ct;
    let lat = libm::atan2(num, den);
    let s2 = lat.sin() * lat.sin();
    let n = WGS84_A / libm::sqrt(1.0 - WGS84_E2 * s2);
    let alt = if lat.cos().abs() > 1e-12 { pr / lat.cos() - n } else { z / lat.sin() - n * (1.0 - WGS84_E2) };
    (lat.to_degrees(), lon.to_degrees(), alt)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SPEC §6.2/§10.2: NED -> geodetic -> NED round trip under 1 mm within
    /// a 10 km square (verified through the ECEF intermediate form).
    #[test]
    fn round_trip_under_1mm_over_10km_square() {
        let origin = GeoOrigin::DEFAULT;
        let step = 1000.0;
        let mut worst: f64 = 0.0;
        for n in -5..=5 {
            for e in -5..=5 {
                for d in [-5.0, 0.0, 20.0, -100.0] {
                    let ned = [n as f64 * step, e as f64 * step, d];
                    let (lat, lon, alt) = origin.ned_to_geodetic(&ned);
                    let back = origin.geodetic_to_ned(lat, lon, alt);
                    for k in 0..3 {
                        let err = (back[k] - ned[k]).abs();
                        worst = worst.max(err);
                    }
                }
            }
        }
        assert!(worst < 1e-3, "worst round-trip error {worst:.6} m >= 1 mm");
    }

    #[test]
    fn origin_maps_to_itself() {
        let o = GeoOrigin::DEFAULT;
        let (lat, lon, alt) = o.ned_to_geodetic(&[0.0, 0.0, 0.0]);
        assert!((lat - o.lat_deg).abs() < 1e-9);
        assert!((lon - o.lon_deg).abs() < 1e-9);
        assert!((alt - o.alt_m).abs() < 1e-4); // Bowring closed form: sub-0.1 mm
    }

    /// 1 degree of latitude is ~111.13 km at 47.4° N (WGS84 meridional arc).
    #[test]
    fn north_meter_scale_matches_curvature_radius() {
        let o = GeoOrigin::DEFAULT;
        let (m, _) = o.radii();
        // 1 arcminute of latitude = 1 nautical mile by the meridional radius.
        let arc_min = (1.0f64 / 60.0).to_radians() * m;
        assert!((arc_min - 1853.2).abs() < 15.0, "1' of latitude = {arc_min} m");
    }

    /// Down-positive: 100 m up (ned z = -100) maps to alt = origin + 100.
    #[test]
    fn altitude_sign_convention() {
        let o = GeoOrigin::DEFAULT;
        let (_, _, alt) = o.ned_to_geodetic(&[0.0, 0.0, -100.0]);
        assert!((alt - (o.alt_m + 100.0)).abs() < 1e-6);
    }

    /// ECEF<->geodetic self-consistency at a reference point (Zurich-ish).
    #[test]
    fn ecef_geodetic_round_trip() {
        let (lat, lon, alt) = (47.397770, 8.545580, 488.0);
        let p = geodetic_to_ecef(lat, lon, alt);
        let (lat2, lon2, alt2) = ecef_to_geodetic(&p);
        assert!((lat - lat2).abs() < 1e-12);
        assert!((lon - lon2).abs() < 1e-12);
        assert!((alt - alt2).abs() < 1e-6);
    }

    /// The linear M/N map tracks the exact conversion to ~mm near the
    /// origin and diverges at the curvature scale (~meters at 5 km) — the
    /// documented reason the ECEF path is used on the wire (ADR-004).
    #[test]
    fn linear_map_local_accuracy_and_curvature_scale() {
        let o = GeoOrigin::DEFAULT;
        let near = [50.0, -30.0, 10.0];
        let (la, lo, al) = o.ned_to_geodetic_linear(&near);
        let back = o.geodetic_to_ned(la, lo, al);
        let err_near: f64 = (0..3).map(|k| (back[k] - near[k]).abs()).fold(0.0, f64::max);
        assert!(err_near < 0.02, "near-origin linear error {err_near}");

        let far = [5000.0, 5000.0, 0.0];
        let (la, lo, al) = o.ned_to_geodetic_linear(&far);
        let back = o.geodetic_to_ned(la, lo, al);
        let err_far: f64 = (0..3).map(|k| (back[k] - far[k]).abs()).fold(0.0, f64::max);
        assert!(err_far > 1.0, "curvature error expected at 5 km, got {err_far}");
        assert!(err_far < 10.0, "curvature error larger than expected: {err_far}");
    }
}
