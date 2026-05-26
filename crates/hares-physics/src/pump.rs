//! Ground-loop circulation pump power model.
//!
//! Computes electrical input power for a closed-loop vertical borehole
//! circulation pump using the Darcy-Weisbach equation for pipe friction head
//! loss, minor losses from fittings and U-bends, and pump+motor efficiency.
//!
//! References:
//! - ASHRAE Handbook of HVAC Systems and Equipment 2020, Ch. 9 (Hydronic
//!   Heating and Cooling System Design)
//! - ASHRAE Handbook of Fundamentals 2021, Ch. 22 (Pipe Sizing)
//! - Kavanaugh, S.P. and Rafferty, K. (2014), "Geothermal Heating and Cooling:
//!   Design of Ground-Source Heat Pump Systems", ASHRAE, Ch. 5–6.

use std::f64::consts::PI;

/// Density of water at 10°C [kg/m³] — typical ground-loop entering temperature.
/// Perry's Chemical Engineers' Handbook, 9th Ed., Table 2-1.
const WATER_DENSITY_KG_M3: f64 = 999.7;

/// Gravitational acceleration [m/s²]. NIST CODATA 2022.
const G_ACCELERATION_M_S2: f64 = 9.81;

/// Kinematic viscosity of water at 10°C [m²/s].
/// Perry's Chemical Engineers' Handbook, 9th Ed., Table 2-443.
const WATER_KINEMATIC_VISCOSITY_M2_S: f64 = 1.307e-6;

/// Absolute roughness of HDPE pipe [m].
/// ASHRAE HoF 2021 Ch.22 Table 1: drawn tubing / smooth plastic = 0.0015 mm.
const HDPE_ROUGHNESS_M: f64 = 1.5e-6;

/// Minor-loss multiplier applied to friction head to account for U-bend at the
/// borehole bottom, entry/exit losses, and header fittings. Closed-loop
/// vertical borehole with smooth HDPE pipe per ASHRAE HVAC Systems & Equipment
/// 2020 Ch.9.6.
const MINOR_LOSS_FACTOR: f64 = 0.4;

/// Default additional system head loss [m] for the heat-pump water-to-refrigerant
/// heat exchanger and distribution headers — typical for residential 4-ton GSHP
/// with brazed-plate HX. ASHRAE HVAC Systems & Equipment 2020 Ch.9 Table 7.
pub const DEFAULT_SYSTEM_HEAD_LOSS_M: f64 = 3.0;

// ---------------------------------------------------------------------------
// Public interface
// ---------------------------------------------------------------------------

/// Compute ground-loop circulation pump electrical input power [kW].
///
/// The pump runs any time the heat pump compressor is active to maintain
/// water flow through the ground loop.  Power is computed from the
/// Darcy-Weisbach pipe friction head plus user-supplied system head
/// (heat exchanger, headers, valves), divided by pump and motor efficiency.
///
/// # Arguments
///
/// * `loop_depth_m` — Vertical borehole depth below ground surface [m].
///   Total pipe length = 2 × depth (supply + return).
/// * `pipe_diameter_m` — Inner diameter of the HDPE U-bend pipe [m].
///   Typical: 0.025 m (1" nominal SDR11).
/// * `flow_rate_m3_per_s` — Design volumetric flow rate through the
///   circulation loop [m³/s]. Typical: 0.00019 m³/s (≈ 3 US GPM) per
///   ton of heat-pump capacity.
/// * `pump_efficiency` — Hydraulic-to-shaft efficiency of the circulator
///   pump [-]. Typical small wet-rotor circulator: 0.35–0.55.
/// * `motor_efficiency` — Electrical-to-shaft efficiency of the pump
///   motor [-]. PSC motor: 0.40–0.60; ECM: 0.70–0.85.
/// * `system_head_loss_m` — Additional head loss from the water-to-refrigerant
///   heat exchanger, distribution headers, isolation valves, and strainer [m].
///   Use [`DEFAULT_SYSTEM_HEAD_LOSS_M`] for a typical residential brazed-plate
///   HX at design flow. Set to 0.0 for borehole loop only.
#[must_use]
pub fn compute_ground_loop_pump_power_kw(
    loop_depth_m: f64,
    pipe_diameter_m: f64,
    flow_rate_m3_per_s: f64,
    pump_efficiency: f64,
    motor_efficiency: f64,
    system_head_loss_m: f64,
) -> f64 {
    // Guard against degenerate inputs.
    if loop_depth_m <= 0.0
        || pipe_diameter_m <= 0.0
        || flow_rate_m3_per_s <= 0.0
        || pump_efficiency <= 0.0
        || motor_efficiency <= 0.0
        || !loop_depth_m.is_finite()
        || !pipe_diameter_m.is_finite()
        || !flow_rate_m3_per_s.is_finite()
        || !pump_efficiency.is_finite()
        || !motor_efficiency.is_finite()
        || !system_head_loss_m.is_finite()
    {
        return 0.0;
    }

    // Total pipe length: supply-down + return-up.
    let pipe_length_m = 2.0 * loop_depth_m;

    // Cross-sectional flow area [m²].
    let area_m2 = PI * pipe_diameter_m * pipe_diameter_m / 4.0;

    // Flow velocity [m/s].
    let velocity_m_s = flow_rate_m3_per_s / area_m2;

    // Reynolds number.
    let reynolds = velocity_m_s * pipe_diameter_m / WATER_KINEMATIC_VISCOSITY_M2_S;

    // Darcy friction factor: laminar or turbulent (Swamee-Jain 1976).
    // Swamee-Jain is explicit and valid for 10⁻⁶ ≤ ε/D ≤ 10⁻², 5000 ≤ Re ≤ 10⁸.
    let friction_factor = if reynolds <= 2300.0 {
        64.0 / reynolds
    } else {
        let rel_roughness = HDPE_ROUGHNESS_M / (3.7 * pipe_diameter_m);
        // Swamee-Jain uses log10, not ln.
        let denom = (rel_roughness + 5.74 / reynolds.powf(0.9)).log10();
        // denom is negative (log10 of a small number), squared produces positive f.
        0.25 / (denom * denom)
    };

    // Darcy-Weisbach friction head loss [m].
    let h_friction_m = friction_factor * pipe_length_m * velocity_m_s * velocity_m_s
        / (2.0 * G_ACCELERATION_M_S2 * pipe_diameter_m);

    // Minor losses (U-bend, entry/exit, headers) proportional to friction.
    let h_minor_m = h_friction_m * MINOR_LOSS_FACTOR;

    // Total dynamic head [m].
    let total_head_m = h_friction_m + h_minor_m + system_head_loss_m.max(0.0);

    // Hydraulic power [W] = ρ g H Q.
    let hydraulic_power_w =
        WATER_DENSITY_KG_M3 * G_ACCELERATION_M_S2 * total_head_m * flow_rate_m3_per_s;

    // Combined wire-to-water efficiency.
    let total_efficiency = pump_efficiency * motor_efficiency;

    // Electrical input power [kW].
    hydraulic_power_w / (total_efficiency * 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test helper: typical residential single-borehole GSHP pump parameters.
    fn typical_residential_params() -> (f64, f64, f64, f64, f64, f64) {
        // 60 m borehole, 1" HDPE, 0.19 L/s (3 GPM per borehole),
        // pump η=0.35, motor η=0.40, system head=3.0 m → ~62 W per borehole.
        // For a 4-ton system with 4 parallel boreholes: 4 × 62 = 248 W ≈ 62 W/ton.
        (60.0, 0.025, 0.00019, 0.35, 0.40, 3.0)
    }

    #[test]
    fn zero_power_for_zero_flow() {
        let kw = compute_ground_loop_pump_power_kw(60.0, 0.025, 0.0, 0.5, 0.5, 3.0);
        assert_eq!(kw, 0.0, "zero flow must produce zero pump power");
    }

    #[test]
    fn zero_power_for_zero_depth() {
        let kw = compute_ground_loop_pump_power_kw(0.0, 0.025, 0.00019, 0.5, 0.5, 3.0);
        assert_eq!(kw, 0.0, "zero depth must produce zero pump power");
    }

    #[test]
    fn zero_power_for_zero_efficiency() {
        let kw = compute_ground_loop_pump_power_kw(60.0, 0.025, 0.00019, 0.0, 0.5, 3.0);
        assert_eq!(kw, 0.0, "zero pump efficiency must produce zero power");
    }

    #[test]
    fn zero_power_for_negative_inputs() {
        assert_eq!(
            compute_ground_loop_pump_power_kw(-10.0, 0.025, 0.00019, 0.5, 0.5, 3.0),
            0.0
        );
        assert_eq!(
            compute_ground_loop_pump_power_kw(60.0, -0.001, 0.00019, 0.5, 0.5, 3.0),
            0.0
        );
    }

    #[test]
    fn typical_residential_pump_in_expected_range() {
        let (depth, diam, flow, pump_eta, motor_eta, sys_head) = typical_residential_params();
        let kw =
            compute_ground_loop_pump_power_kw(depth, diam, flow, pump_eta, motor_eta, sys_head);
        // For a single borehole at 3 GPM: expect 40–120 W.
        // 0.35 × 0.40 = 0.14 wire-to-water; ~4.7 m total head → ~62 W.
        assert!(
            kw >= 0.04 && kw <= 0.12,
            "single-borehole pump power must be 0.04–0.12 kW, got {kw:.4} kW"
        );
    }

    #[test]
    fn pump_power_scales_linearly_with_system_head() {
        let (depth, diam, flow, pump_eta, motor_eta, _) = typical_residential_params();
        let kw_no_hx =
            compute_ground_loop_pump_power_kw(depth, diam, flow, pump_eta, motor_eta, 0.0);
        let kw_with_hx =
            compute_ground_loop_pump_power_kw(depth, diam, flow, pump_eta, motor_eta, 3.0);
        assert!(
            kw_with_hx > kw_no_hx,
            "adding system head must increase pump power"
        );
    }

    #[test]
    fn pump_power_increases_with_depth() {
        let kw_shallow = compute_ground_loop_pump_power_kw(30.0, 0.025, 0.00019, 0.5, 0.5, 0.0);
        let kw_deep = compute_ground_loop_pump_power_kw(60.0, 0.025, 0.00019, 0.5, 0.5, 0.0);
        assert!(
            kw_deep > kw_shallow,
            "deeper boreholes must increase pump power (more pipe friction)"
        );
    }

    #[test]
    fn pump_power_decreases_with_larger_pipe() {
        let kw_small = compute_ground_loop_pump_power_kw(60.0, 0.02, 0.00019, 0.5, 0.5, 0.0);
        let kw_large = compute_ground_loop_pump_power_kw(60.0, 0.03, 0.00019, 0.5, 0.5, 0.0);
        assert!(
            kw_large < kw_small,
            "larger pipe diameter must reduce pump power (lower velocity, lower friction)"
        );
    }

    #[test]
    fn pump_power_increases_with_flow_rate() {
        let kw_low = compute_ground_loop_pump_power_kw(60.0, 0.025, 0.0001, 0.5, 0.5, 0.0);
        let kw_high = compute_ground_loop_pump_power_kw(60.0, 0.025, 0.0003, 0.5, 0.5, 0.0);
        assert!(
            kw_high > kw_low,
            "higher flow rate must increase pump power"
        );
    }

    #[test]
    fn pump_power_decreases_with_better_efficiency() {
        let kw_poor = compute_ground_loop_pump_power_kw(60.0, 0.025, 0.00019, 0.2, 0.3, 0.0);
        let kw_good = compute_ground_loop_pump_power_kw(60.0, 0.025, 0.00019, 0.6, 0.8, 0.0);
        assert!(
            kw_good < kw_poor,
            "better pump+motor efficiency must reduce electrical power"
        );
    }

    #[test]
    fn laminar_flow_power_positive() {
        // Very low flow → Re < 2300 → laminar, f = 64/Re
        let kw = compute_ground_loop_pump_power_kw(60.0, 0.025, 0.00001, 0.5, 0.5, 0.0);
        assert!(
            kw > 0.0,
            "laminar flow must still produce positive pump power"
        );
    }
}
