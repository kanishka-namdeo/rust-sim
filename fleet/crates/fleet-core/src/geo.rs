//! Geodesy for the operator map control plane (ADR-0017).
//!
//! Ported from rustsitsim's `sitsim-env/src/geodesy.rs` (same repo, proven
//! live against PX4 SITL): the local NED frame is the exact tangent plane at
//! the origin (WGS84); NED <-> geodetic goes through ECEF with Bowring's
//! closed form, so the round trip is exact (< 1 mm over a 10 km square).
//! std-only — no `libm`, no new dependencies (the fleet never needs the
//! linearized M/N map, so it is not ported).

/// WGS84 semi-major axis (m).
pub const WGS84_A: f64 = 6378137.0;
/// WGS84 first eccentricity squared.
pub const WGS84_E2: f64 = 0.0066943799901413165;
/// WGS84 flattening.
pub const WGS84_F: f64 = 1.0 / 298.257223563;
/// WGS84 semi-minor axis (m).
pub const WGS84_B: f64 = WGS84_A * (1.0 - WGS84_F);
/// Second eccentricity squared, (a^2-b^2)/b^2.
pub const WGS84_EP2: f64 = (WGS84_A * WGS84_A - WGS84_B * WGS84_B) / (WGS84_B * WGS84_B);

/// Local NED origin — the geodetic fix every vehicle's sim reports from
/// (`[env] origin` in the scenario; default: the PX4 test field).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct GeoOrigin {
    pub lat_deg: f64,
    pub lon_deg: f64,
    pub alt_m: f64,
}

impl GeoOrigin {
    /// Default origin: the PX4 test field — the same constant
    /// rustsitsim's HIL_GPS anchors to (`GeoOrigin::DEFAULT` there).
    pub const DEFAULT: GeoOrigin =
        GeoOrigin { lat_deg: 47.397770, lon_deg: 8.545580, alt_m: 500.0 };

    fn lat_rad(&self) -> f64 {
        self.lat_deg.to_radians()
    }

    fn lon_rad(&self) -> f64 {
        self.lon_deg.to_radians()
    }

    /// ECEF position of the origin.
    pub fn ecef(&self) -> [f64; 3] {
        geodetic_to_ecef(self.lat_deg, self.lon_deg, self.alt_m)
    }

    /// NED offset -> ECEF position (tangent-plane rotation + translation).
    pub fn ned_to_ecef(&self, ned: &[f64; 3]) -> [f64; 3] {
        let p0 = self.ecef();
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

    /// NED meters (n, e, d) -> geodetic (lat_deg, lon_deg, alt_msl), exact
    /// via ECEF (round-trips with [`Self::geodetic_to_ned`]).
    pub fn ned_to_geodetic(&self, ned: &[f64; 3]) -> (f64, f64, f64) {
        let p = self.ned_to_ecef(ned);
        ecef_to_geodetic(&p)
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
    let n = WGS84_A / (1.0 - WGS84_E2 * s2).sqrt();
    let nx = (n + alt_m) * cl;
    [nx * co, nx * so, (n * (1.0 - WGS84_E2) + alt_m) * sl]
}

/// ECEF (meters) -> geodetic (degrees, MSL meters), Bowring's closed form.
/// Sub-millimeter for points near the ellipsoid surface.
pub fn ecef_to_geodetic(p: &[f64; 3]) -> (f64, f64, f64) {
    let x = p[0];
    let y = p[1];
    let z = p[2];
    let lon = y.atan2(x);
    let pr = (x * x + y * y).sqrt();
    let theta = (z * WGS84_A).atan2(pr * WGS84_B);
    let st = theta.sin();
    let ct = theta.cos();
    let num = z + WGS84_EP2 * WGS84_B * st * st * st;
    let den = pr - WGS84_E2 * WGS84_A * ct * ct * ct;
    let lat = num.atan2(den);
    let s2 = lat.sin() * lat.sin();
    let n = WGS84_A / (1.0 - WGS84_E2 * s2).sqrt();
    let alt = if lat.cos().abs() > 1e-12 {
        pr / lat.cos() - n
    } else {
        z / lat.sin() - n * (1.0 - WGS84_E2)
    };
    (lat.to_degrees(), lon.to_degrees(), alt)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ADR-0017: NED -> geodetic -> NED round trip under 1 mm within a
    /// 10 km square (the same acceptance the sim's geodesy holds).
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
        assert!((alt - o.alt_m).abs() < 1e-4);
    }

    /// Down-positive NED: 100 m up (z = -100) maps to alt = origin + 100 —
    /// the operator mission upload and go-to conversions depend on this sign.
    #[test]
    fn altitude_sign_convention() {
        let o = GeoOrigin::DEFAULT;
        let (_, _, alt) = o.ned_to_geodetic(&[0.0, 0.0, -100.0]);
        assert!((alt - (o.alt_m + 100.0)).abs() < 1e-6);
        let ned = o.geodetic_to_ned(o.lat_deg, o.lon_deg, o.alt_m + 100.0);
        assert!((ned[2] + 100.0).abs() < 1e-6);
    }

    /// The operator mission contract: a waypoint entered as (origin lat,
    /// lon, origin alt + 12 m AGL) converts to NED [0, 0, -12].
    #[test]
    fn waypoint_agl_convention() {
        let o = GeoOrigin::DEFAULT;
        let ned = o.geodetic_to_ned(o.lat_deg, o.lon_deg, o.alt_m + 12.0);
        assert!(ned[0].abs() < 1e-6);
        assert!(ned[1].abs() < 1e-6);
        assert!((ned[2] + 12.0).abs() < 1e-6);
    }

    #[test]
    fn ecef_geodetic_round_trip() {
        let (lat, lon, alt) = (47.397770, 8.545580, 488.0);
        let p = geodetic_to_ecef(lat, lon, alt);
        let (lat2, lon2, alt2) = ecef_to_geodetic(&p);
        assert!((lat - lat2).abs() < 1e-12);
        assert!((lon - lon2).abs() < 1e-12);
        assert!((alt - alt2).abs() < 1e-6);
    }
}
