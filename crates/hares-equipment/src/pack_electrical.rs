//! Pack-level electrical model shared by the stationary Battery and the EV.
//!
//! Cell-level quantities (OCV from the shared per-chemistry tables, cell
//! internal resistance) are the ones the electrochemistry literature reports;
//! pack voltage and resistance follow from the series/parallel topology.
//! Both equipment families solve the same terminal-voltage quadratic and
//! compute the same I²R ohmic heating, so the solve lives here exactly once.
//!
//! Charging-loss attribution rule enforced by this module's shape: the only
//! pack heating it produces is I²R through the cell resistance. The AC→DC
//! (or DC→AC) *conversion* loss — the difference the charger's efficiency
//! makes — dissipates in the charger's power electronics, not in the cells:
//! attributing it to a pack thermal mass (e.g. (1−η)·P ≈ 1.15 kW at Level 2
//! into a ~480 kJ/K pack) drives the pack to 165–281 °C and the Arrhenius
//! degradation fit out of its validated domain.

/// Pack topology and cell resistance.
///
/// Defaults per family live with each equipment (the Battery and EV carry
/// their own cited values); both construct this from their resolved
/// topology so the solve is shared.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PackElectrical {
    /// Series cell count (sets pack voltage: `pack_ocv = cell_ocv × n_series`).
    pub n_series: u32,
    /// Parallel string count (sets pack current sharing and resistance).
    pub n_parallel: u32,
    /// Cell internal resistance [Ω] at mid-SOC, 25 °C.
    pub cell_resistance_ohm: f64,
}

/// Result of the terminal-voltage solve for one operating point.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PackTerminal {
    /// Terminal voltage [V] (rises above OCV when charging, sags when
    /// discharging).
    pub terminal_v: f64,
    /// Actual pack-terminal DC power [W] after any matched-impedance clamp
    /// (same sign as the requested power).
    pub actual_dc_power_w: f64,
    /// Pack current [A] (positive = charging into the pack).
    pub current_a: f64,
    /// I²R ohmic heating [W] in the cell resistance — the only cell heat
    /// this model produces.
    pub ohmic_loss_w: f64,
    /// True when the requested power exceeded the matched-impedance maximum
    /// `P_max = Voc²/(4R)` and was clamped.
    pub clamped_to_p_max: bool,
}

impl PackElectrical {
    /// Pack open-circuit voltage [V] at the given cell OCV [V].
    pub(crate) fn pack_ocv_v(&self, cell_ocv_v: f64) -> f64 {
        cell_ocv_v * self.n_series as f64
    }

    /// Pack terminal resistance [Ω]:
    /// `R_pack = R_cell × n_series / n_parallel`
    /// (series resistances add; parallel strings share current).
    pub(crate) fn pack_resistance_ohm(&self) -> f64 {
        self.cell_resistance_ohm * self.n_series as f64 / self.n_parallel.max(1) as f64
    }

    /// Solve the terminal voltage for a requested pack-terminal DC power.
    ///
    /// Quadratic terminal-voltage formula (OCHRE method, Battery.py:295):
    ///   `V = Voc/2 + sqrt((Voc/2)² + P_dc·R)`
    /// Sign convention: `dc_power_w > 0` = charging (consuming), < 0 =
    /// discharging. Charging raises the terminal voltage above OCV;
    /// discharging sags it below.
    ///
    /// When the discriminant goes negative the requested discharge exceeds
    /// the matched-impedance maximum `P_max = Voc²/(4R)`; the power is
    /// clamped to `P_max` at terminal voltage `Voc/2`.
    pub(crate) fn solve(&self, cell_ocv_v: f64, dc_power_w: f64) -> PackTerminal {
        let pack_ocv = self.pack_ocv_v(cell_ocv_v);
        let pack_resistance = self.pack_resistance_ohm();

        if pack_ocv < f64::EPSILON || pack_resistance <= 0.0 {
            return PackTerminal {
                terminal_v: 0.0,
                actual_dc_power_w: 0.0,
                current_a: 0.0,
                ohmic_loss_w: 0.0,
                clamped_to_p_max: false,
            };
        }

        let half_voc = pack_ocv / 2.0;
        let discriminant = half_voc * half_voc + dc_power_w * pack_resistance;

        let (terminal_v, actual_dc_power_w, clamped) = if discriminant >= 0.0 {
            (half_voc + discriminant.sqrt(), dc_power_w, false)
        } else {
            // Maximum extractable power P_max = Voc²/(4R); at this limit
            // terminal voltage = Voc/2 (matched-impedance condition).
            let p_max_w = pack_ocv * pack_ocv / (4.0 * pack_resistance);
            let clamped = dc_power_w.abs().min(p_max_w) * dc_power_w.signum();
            (half_voc, clamped, true)
        };

        let current_a = if terminal_v.abs() > f64::EPSILON {
            actual_dc_power_w / terminal_v
        } else {
            0.0
        };
        let ohmic_loss_w = current_a * current_a * pack_resistance;

        PackTerminal {
            terminal_v,
            actual_dc_power_w,
            current_a,
            ohmic_loss_w,
            clamped_to_p_max: clamped,
        }
    }
}

/// Charging-LUT c-rate: the charge power relative to the degradation-
/// adjusted pack capacity (`rated × SOH`) — the temperature-independent
/// rating, the same convention OCHRE Battery.py's `capacity_kwh_nominal`
/// follows. One rule for both pack models so the siblings cannot diverge:
/// the LUT's own `[soc, temp, c_rate, soh]` axes already carry the
/// temperature and degradation effects, so the divisor must not pre-apply
/// either — dividing by the temperature-derated usable capacity (the
/// pre-alignment EV behavior) folded the reversible derate into a
/// dimension the LUT's temperature axis already encodes, inflating the
/// c-rate ≈1.3× at 0 °C (≈1.5× at −7 °C) and bin-shifting every
/// configured lookup.
pub(crate) fn charging_lut_c_rate(power_kw: f64, soh_adjusted_kwh: f64) -> f64 {
    if soh_adjusted_kwh > 0.0 {
        power_kw / soh_adjusted_kwh
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack() -> PackElectrical {
        // 96S43P, 5 mΩ cells — the 75 kWh EV default topology.
        PackElectrical {
            n_series: 96,
            n_parallel: 43,
            cell_resistance_ohm: 0.005,
        }
    }

    #[test]
    fn charging_at_level2_produces_i2r_magnitude_heat_only() {
        // 11.5 kW AC × η 0.9 = 10.35 kW DC at ~355 V pack → ~29 A → I²R ≈ 9.6 W.
        let cell_ocv = 3.7;
        let dc_w = 10_350.0;
        let t = pack().solve(cell_ocv, dc_w);
        assert!(!t.clamped_to_p_max);
        assert!(t.terminal_v > pack().pack_ocv_v(cell_ocv));
        assert!(
            (t.current_a - 29.0).abs() < 1.0,
            "current ~29 A, got {}",
            t.current_a
        );
        // The loss-attribution rule: the *only* heat is I²R — two orders of
        // magnitude below the charger conversion loss (1.15 kW), which heats
        // the charger, not the cells.
        assert!(
            t.ohmic_loss_w < 20.0 && t.ohmic_loss_w > 1.0,
            "I2R at Level 2 is O(10 W), got {} W",
            t.ohmic_loss_w
        );
    }

    #[test]
    fn discharge_sags_voltage_and_clamps_at_matched_impedance() {
        let cell_ocv = 3.7;
        let p = pack();
        let t = p.solve(cell_ocv, -10_000.0);
        assert!(t.terminal_v < p.pack_ocv_v(cell_ocv));
        assert!(!t.clamped_to_p_max);

        // Far beyond P_max = Voc²/(4R) ≈ 3.9 MW for this pack: clamped.
        let t2 = p.solve(cell_ocv, -1.0e9);
        assert!(t2.clamped_to_p_max);
        let p_max = p.pack_ocv_v(cell_ocv).powi(2) / (4.0 * p.pack_resistance_ohm());
        assert!((t2.actual_dc_power_w.abs() - p_max).abs() / p_max < 1e-9);
        assert!((t2.terminal_v - p.pack_ocv_v(cell_ocv) / 2.0).abs() < 1e-6);
    }

    #[test]
    fn zero_power_produces_zero_current_and_heat() {
        let t = pack().solve(3.7, 0.0);
        assert_eq!(t.current_a, 0.0);
        assert_eq!(t.ohmic_loss_w, 0.0);
        assert!((t.terminal_v - pack().pack_ocv_v(3.7)).abs() < 1e-9);
    }
}
