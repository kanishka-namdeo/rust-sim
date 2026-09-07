//! Local magnetic-field model (SPEC §6.2): dipole-style field from
//! inclination, declination, and horizontal intensity.

/// NED magnetic field vector from (inclination, declination, H).
///
/// - inclination: degrees below horizontal (northern hemisphere: field
///   points INTO the earth, so the down component is positive),
/// - declination: degrees east of true north,
/// - h_gauss: horizontal intensity in gauss.
///
/// Default model (67°, 2°, 0.5 gauss) yields roughly (0.19, 0.01, 0.46)
/// gauss in NED (SPEC §6.2).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MagFieldModel {
    pub incl_deg: f64,
    pub decl_deg: f64,
    pub h_gauss: f64,
}

impl Default for MagFieldModel {
    fn default() -> Self {
        MagFieldModel { incl_deg: 67.0, decl_deg: 2.0, h_gauss: 0.5 }
    }
}

impl MagFieldModel {
    /// Field vector in NED, gauss.
    pub fn field_ned_gauss(&self) -> [f64; 3] {
        let incl = self.incl_deg.to_radians();
        let decl = self.decl_deg.to_radians();
        let h = self.h_gauss;
        [
            h * incl.cos() * decl.cos(),
            h * incl.cos() * decl.sin(),
            h * incl.sin(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_field_matches_spec_reference() {
        let f = MagFieldModel::default().field_ned_gauss();
        assert!((f[0] - 0.19).abs() < 0.01, "north {f:?}");
        assert!(f[1].abs() < 0.02, "east {f:?}");
        assert!((f[2] - 0.46).abs() < 0.02, "down {f:?}");
        // ADR-003: `h_gauss` is applied as the total field intensity so the
        // reference vector (0.19, 0.01, 0.47) of SPEC §6.2 is reproduced.
        let b = libm::sqrt(f[0] * f[0] + f[1] * f[1] + f[2] * f[2]);
        assert!((b - 0.5).abs() < 1e-12);
    }

    #[test]
    fn zero_inclination_is_purely_horizontal_north() {
        let f = MagFieldModel { incl_deg: 0.0, decl_deg: 0.0, h_gauss: 1.0 }.field_ned_gauss();
        assert_eq!(f, [1.0, 0.0, 0.0]);
    }
}
