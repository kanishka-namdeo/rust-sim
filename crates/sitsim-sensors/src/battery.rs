//! Battery model (SPEC §6.5): voltage curve + SoC bookkeeping.
//!
//! SoC integration lives in the dynamics (sitsim-core, `soc` state slot);
//! this model converts SoC to the two-segment voltage curve and the
//! percent telemetry. Current draw: I = i_base + k_p * total_thrust
//! (hover ~16 A on the default airframe).

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BatteryParams {
    pub enabled: bool,
    pub capacity_mah: u32,
    /// Full-charge voltage, V (4S).
    pub v_full: f64,
    /// Voltage at 20% SoC, V.
    pub v_20: f64,
    /// Voltage at empty, V.
    pub v_empty: f64,
}

impl Default for BatteryParams {
    fn default() -> Self {
        BatteryParams {
            enabled: false,
            capacity_mah: 5200,
            v_full: 16.8,
            v_20: 14.0,
            v_empty: 13.2,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BatteryModel {
    pub params: BatteryParams,
}

impl BatteryModel {
    pub fn new(params: BatteryParams) -> Self {
        BatteryModel { params }
    }

    /// Two-segment voltage curve: linear 16.8 V (full) -> 14.0 V at 20%,
    /// then steeper to 13.2 V empty (SPEC §6.5).
    pub fn voltage_v(&self, soc: f64) -> f64 {
        let soc = soc.clamp(0.0, 1.0);
        if soc >= 0.2 {
            let t = (soc - 0.2) / 0.8;
            self.params.v_20 + t * (self.params.v_full - self.params.v_20)
        } else {
            let t = soc / 0.2;
            self.params.v_empty + t * (self.params.v_20 - self.params.v_empty)
        }
    }

    /// Percent for the telemetry plane (0-100, 100 when disabled and SoC
    /// pinned at 1).
    pub fn percent(&self, soc: f64) -> f64 {
        (soc.clamp(0.0, 1.0) * 100.0 * 10.0).round() / 10.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voltage_curve_segments() {
        let b = BatteryModel::new(BatteryParams::default());
        assert_eq!(b.voltage_v(1.0), 16.8);
        assert_eq!(b.voltage_v(0.2), 14.0);
        assert_eq!(b.voltage_v(0.0), 13.2);
        // Segment slopes: upper 2.8 V over 0.8 SoC, lower 0.8 V over 0.2.
        assert!((b.voltage_v(0.6) - 15.4).abs() < 1e-9);
        assert!((b.voltage_v(0.1) - 13.6).abs() < 1e-9);
    }

    #[test]
    fn percent_clamps() {
        let b = BatteryModel::new(BatteryParams::default());
        assert_eq!(b.percent(1.0), 100.0);
        assert_eq!(b.percent(0.0), 0.0);
        assert_eq!(b.percent(1.5), 100.0);
        assert_eq!(b.percent(0.872), 87.2);
    }

    /// Hover current sanity through the dynamics-side constants: with the
    /// default QuadParams, I = 0.4 + 1.06 * 14.71 = ~16.0 A (SPEC §6.5).
    #[test]
    fn hover_current_around_16a() {
        let i = 0.4 + 1.06 * (1.5 * 9.80665);
        assert!((i - 16.0_f64).abs() < 0.05, "hover current {i} A");
        // Full-battery hover endurance with 5200 mAh: 5.2 Ah / 16 A ~ 0.33 h.
        let hours = 5.2 / i;
        assert!(hours > 0.25 && hours < 0.4);
    }
}
