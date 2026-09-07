//! ISA atmosphere (SPEC §6.2).
//!
//! The reference datum is the **scenario origin**, not mean sea level: the
//! pressure at the origin altitude is 101325 (field units) by construction,
//! matching the observed-working prototype value (see ADR-002 for the unit
//! discussion: PX4's `SimulatorMavlink` multiplies `abs_pressure` by 100 and
//! treats the result as Pa; the empirically verified bring-up stream carried
//! 101325 in the field and PX4 booted and converged with it, so the field
//! carries the standard-atmosphere number and `pressure_alt` is defined
//! consistently above the origin).

/// ISA constants (ICAO standard atmosphere).
pub const P0: f64 = 101325.0; // field units (see module docs / ADR-002)
pub const T0_K: f64 = 288.15;
pub const LAPSE_K_PER_M: f64 = 0.0065;
pub const G0_M_S2: f64 = 9.80665;
pub const R_AIR: f64 = 287.05287;

/// ISA exponent g/(R L) = 5.2558797.
const ISA_EXP: f64 = G0_M_S2 / (R_AIR * LAPSE_K_PER_M);

/// Absolute pressure at `height_above_origin_m`, in HIL_SENSOR field units.
/// Clamps the lapse ratio away from zero (below ~44 km the ratio stays
/// positive; the clamp only guards pathological inputs).
pub fn pressure_field_units(height_above_origin_m: f64) -> f64 {
    let ratio = (1.0 - LAPSE_K_PER_M * height_above_origin_m / T0_K).max(1e-9);
    P0 * libm::pow(ratio, ISA_EXP)
}

/// Inverse ISA: height above the origin (m) implied by a pressure in field
/// units. Inverse of [`pressure_field_units`] by construction.
pub fn pressure_alt_m(pressure_field_units: f64) -> f64 {
    let ratio = (pressure_field_units / P0).max(1e-12);
    (T0_K / LAPSE_K_PER_M) * (1.0 - libm::pow(ratio, 1.0 / ISA_EXP))
}

/// ISA temperature at `height_above_origin_m`, degrees Celsius.
pub fn temperature_c(height_above_origin_m: f64) -> f64 {
    T0_K - LAPSE_K_PER_M * height_above_origin_m - 273.15
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_pressure_is_p0_and_inverse_round_trips() {
        assert_eq!(pressure_field_units(0.0), P0);
        for &h in &[0.0, 10.0, 100.0, 500.0, -50.0, 1234.5] {
            let p = pressure_field_units(h);
            let h2 = pressure_alt_m(p);
            assert!((h - h2).abs() < 1e-6, "round trip {h} -> {p} -> {h2}");
        }
    }

    #[test]
    fn pressure_decreases_with_height() {
        assert!(pressure_field_units(100.0) < P0);
        // Standard-atmosphere table: p(1000 m) = 89 874.6 Pa -> drop ~11 451.
        let dp = P0 - pressure_field_units(1000.0);
        assert!((dp - 11_450.4).abs() < 60.0, "1000 m pressure drop {dp}");
    }

    #[test]
    fn lapse_rate_temperature() {
        // 11.75 °C at 500 m above origin in the standard atmosphere.
        let t = temperature_c(500.0);
        assert!((t - 11.75).abs() < 0.5, "T(500 m) = {t}");
    }
}
