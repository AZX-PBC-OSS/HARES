use std::collections::HashMap;

use hares_physics::air_properties::moist_air_density_kg_m3;
use hares_physics::constants::CP_DRY_AIR_J_KG_K;
use hares_physics::infiltration::{
    ach_infiltration, ashrae_wind_stack, duct_leakage_infiltration_m3_s, ela_infiltration,
    natural_ventilation_flow_m3_s,
};
use hares_types::{EnvironmentState, ZoneId};

use super::H_FG_J_PER_KG;
use super::config::{InfiltrationMethod, ThermalSolverConfig};

/// Per-zone infiltration/ventilation coupling for semi-implicit treatment.
///
/// The sensible infiltration load `q = h_inf * (T_out - T_zone)` is split:
///   - Implicit part: `-h_inf * T_zone` added to the A-matrix diagonal
///   - Explicit part: `h_inf * T_out` added as forcing
///
/// The latent infiltration load `q_latent = m_dot_lat * h_fg * (W_out - W_zone)` is
/// split analogously for the humidity solver:
///   - Implicit part: `m_dot_lat * h_fg * W_{n+1}` moves to the denominator
///   - Explicit part: `m_dot_lat * h_fg * W_out` remains as forcing
///
/// Both paths use semi-implicit coupling, following the EnergyPlus zone air
/// moisture balance (Engineering Reference, "Moisture Predictor-Corrector":
/// `C_z dW/dt = ... + m_inf*(W_out - W_z)` where `m_inf` enters the implicit
/// denominator coefficient, just as `m_inf*cp` does for sensible heat in the
/// "Basis for the Zone and Air System Integration" section).
#[derive(Debug, Clone)]
pub(crate) struct InfiltrationCoupling {
    pub zone: ZoneId,
    /// Infiltration+ventilation sensible conductance [W/K] = m_dot_sens * cp.
    pub h_inf_w_k: f64,
    /// Outdoor temperature driving the sensible forcing [°C].
    pub t_forcing_c: f64,
    /// Latent gain [W] computed from current-step humidity ratio (explicit value).
    /// Retained for diagnostic parity with q_sensible_diagnostic_w.
    #[allow(dead_code)]
    pub q_latent_w: f64,
    /// Diagnostic sensible gain [W] = h_inf * (T_out - T_zone) for reporting.
    /// Accessed in thermal_solver/mod.rs for component_gains.
    pub q_sensible_diagnostic_w: f64,
    /// Pure infiltration sensible gain [W] -- excludes ventilation components.
    pub q_infiltration_w: f64,
    /// Forced mechanical ventilation sensible gain [W] -- after recovery efficiency.
    pub q_forced_vent_w: f64,
    /// Natural ventilation sensible gain [W] -- operable window stack/wind flow.
    pub q_natural_vent_w: f64,
    /// Combined sensible flow [m³/s] -- infiltration + ventilation after quadrature.
    pub combined_flow_m3_s: f64,
    /// Latent flow [m³/s] -- may differ from sensible flow when balanced ventilation
    /// has different sensible/latent recovery efficiencies. Retained for diagnostic
    /// inspection; the humidity solver uses m_dot_lat_kg_s directly.
    #[allow(dead_code)]
    pub latent_flow_m3_s: f64,
    /// Latent mass flow rate [kg/s] = rho * latent_flow_m3_s.
    /// Stored directly (not re-derived from volume flow × density) to avoid
    /// recomputing density in format_domain_update, which lacks env access.
    pub m_dot_lat_kg_s: f64,
    /// Outdoor humidity ratio driving the latent forcing [kg/kg].
    /// Passed to the humidity solver for semi-implicit moisture coupling.
    pub w_outdoor: f64,
    /// Raw AIM-2 infiltration flow before ventilation interaction [m³/s].
    pub raw_inf_m3_s: f64,
    /// Forced ventilation flow [m³/s].
    pub forced_flow_m3_s: f64,
    /// Natural ventilation flow [m³/s].
    pub nat_flow_m3_s: f64,
}

/// Computes infiltration and ventilation coupling terms and accumulates latent loads
/// into `latent_out` (which the caller has already cleared).
///
/// `hvac_active` indicates whether the HVAC fan is running this timestep.  When
/// true and the config carries non-zero duct leakage flows, the conditioned-zone
/// infiltration rate is adjusted per ASHRAE 152 §9.3.
///
/// Returns per-zone `InfiltrationCoupling` structs for semi-implicit treatment.
pub(crate) fn apply_infiltration_and_ventilation(
    config: &ThermalSolverConfig,
    env: &EnvironmentState,
    hvac_active: bool,
    latent_out: &mut HashMap<ZoneId, f64>,
    couplings: &mut Vec<InfiltrationCoupling>,
) {
    couplings.clear();
    let p_pa = env.weather.pressure_pa();
    let t_out = env.weather.outdoor_temp_c;
    let w_out = env.weather.outdoor_humidity_ratio;
    // moist_air_density_kg_m3 inverts ASHRAE HOF 2021 Ch.1 Eq.28 specific volume
    // (v = R_da·T·(1+W/ε)/p [m³/kg_da]), so it already yields kg_da/m³.
    let rho = moist_air_density_kg_m3(p_pa, t_out, w_out);

    for zone in &env.zones {
        // Look up per-zone infiltration; default to zero ACH if not configured.
        let method = config
            .infiltration
            .iter()
            .find(|(id, _)| *id == zone.id)
            .map(|(_, m)| *m)
            .unwrap_or_default();

        let mut q_inf_m3_s = match method {
            InfiltrationMethod::AshraeWindStack {
                c_s,
                c_w,
                shielding_coeff,
                n_i,
            } => ashrae_wind_stack(
                c_s,
                c_w,
                t_out - zone.temperature_c,
                env.weather.wind_speed_m_s,
                shielding_coeff,
                n_i,
            ),
            InfiltrationMethod::Ela {
                ela_m2,
                stack_coeff,
                wind_coeff,
            } => ela_infiltration(
                ela_m2,
                stack_coeff,
                wind_coeff,
                t_out - zone.temperature_c,
                env.weather.wind_speed_m_s,
            ),
            InfiltrationMethod::Ach { ach } => ach_infiltration(ach, zone.volume_m3),
        };

        // ASHRAE 152 §9.3: duct leakage imbalance adjusts infiltration when the
        // HVAC fan is running.  Only applied to the conditioned zone (duct leakage
        // does not directly pressurise unconditioned zones).
        if hvac_active
            && zone.id == config.indoor_zone_id
            && (config.supply_duct_leakage_m3_s > 0.0 || config.return_duct_leakage_m3_s > 0.0)
        {
            q_inf_m3_s = duct_leakage_infiltration_m3_s(
                q_inf_m3_s,
                config.supply_duct_leakage_m3_s,
                config.return_duct_leakage_m3_s,
                zone.volume_m3,
            );
        }

        // Natural ventilation is only applied to the indoor/conditioned zone.
        let q_nat_m3_s = if zone.id == config.indoor_zone_id {
            config
                .natural_ventilation
                .as_ref()
                .map(|nv| {
                    natural_ventilation_flow_m3_s(
                        nv.open_area_m2,
                        zone.temperature_c,
                        t_out,
                        nv.t_base_c,
                        w_out,
                        nv.max_outdoor_humidity_ratio,
                        env.weather.wind_speed_m_s,
                        nv.stack_coeff,
                        nv.wind_coeff,
                        zone.volume_m3,
                    )
                })
                .unwrap_or(0.0)
        } else {
            0.0
        };

        // Forced mechanical ventilation flow -- only for zones with explicit flow
        // or the indoor zone (which gets the global ventilation rate).
        let forced_flow_m3_s = config
            .ventilation
            .zone_flow_m3_s
            .get(&zone.id)
            .copied()
            .unwrap_or(if zone.id == config.indoor_zone_id {
                config.ventilation_flow_m3_s
            } else {
                0.0
            });

        // Combine infiltration + natural ventilation + forced ventilation.
        // OCHRE Envelope.py:59-87:
        //   total_nat_flow = infiltration + natural_ventilation
        //   balanced:   sensible_flow = total_nat + forced * (1 - sens_recovery_eff)
        //               latent_flow   = total_nat + forced * (1 - lat_recovery_eff)
        //   unbalanced: flow = sqrt(total_nat² + forced²)  (quadrature combination)
        let total_nat_flow = q_inf_m3_s + q_nat_m3_s;
        let (
            sensible_flow_m3_s,
            latent_flow_m3_s,
            scaled_q_inf_m3_s,
            scaled_q_nat_m3_s,
            scaled_forced_m3_s,
        ) = if config.ventilation.balanced {
            (
                total_nat_flow
                    + forced_flow_m3_s * (1.0 - config.ventilation.sensible_recovery_efficiency),
                total_nat_flow
                    + forced_flow_m3_s * (1.0 - config.ventilation.latent_recovery_efficiency),
                q_inf_m3_s,
                q_nat_m3_s,
                forced_flow_m3_s,
            )
        } else {
            let combined =
                (total_nat_flow * total_nat_flow + forced_flow_m3_s * forced_flow_m3_s).sqrt();
            // OCHRE Envelope.py:78-81: For unbalanced ventilation, scale individual
            // flow components proportionally to maintain mass balance.
            let nat_flow_ratio = if total_nat_flow > 0.0 {
                (combined - forced_flow_m3_s) / total_nat_flow
            } else {
                1.0
            };
            (
                combined,
                combined,
                q_inf_m3_s * nat_flow_ratio,
                q_nat_m3_s * nat_flow_ratio,
                forced_flow_m3_s, // Forced flow is not scaled in OCHRE
            )
        };

        // Combined flow drives sensible/latent loads against outdoor conditions.
        // Recovery efficiency already accounts for HRV/ERV heat exchange by
        // reducing the effective flow rate (OCHRE model).
        let m_dot_sens = rho * sensible_flow_m3_s;
        let m_dot_lat = rho * latent_flow_m3_s;
        let h_inf = m_dot_sens * CP_DRY_AIR_J_KG_K;
        let q_sensible_diagnostic = h_inf * (t_out - zone.temperature_c);
        let q_latent = m_dot_lat * H_FG_J_PER_KG * (w_out - zone.humidity_ratio);

        // Recompute diagnostic gains using scaled flows (for OCHRE parity in reporting).
        // OCHRE reports the scaled infiltration component, not the raw AIM-2 flow.
        let dt_c = t_out - zone.temperature_c;
        let q_infiltration_w_scaled = rho * scaled_q_inf_m3_s * CP_DRY_AIR_J_KG_K * dt_c;
        let q_natural_vent_w_scaled = rho * scaled_q_nat_m3_s * CP_DRY_AIR_J_KG_K * dt_c;
        let forced_sens_eff = if config.ventilation.balanced {
            1.0 - config.ventilation.sensible_recovery_efficiency
        } else {
            1.0 // Unbalanced: no heat recovery, quadrature combination already applied
        };
        let q_forced_vent_w_scaled =
            rho * scaled_forced_m3_s * forced_sens_eff * CP_DRY_AIR_J_KG_K * dt_c;

        couplings.push(InfiltrationCoupling {
            zone: zone.id,
            h_inf_w_k: h_inf,
            t_forcing_c: t_out,
            q_latent_w: q_latent,
            q_sensible_diagnostic_w: q_sensible_diagnostic,
            q_infiltration_w: q_infiltration_w_scaled,
            q_forced_vent_w: q_forced_vent_w_scaled,
            q_natural_vent_w: q_natural_vent_w_scaled,
            combined_flow_m3_s: sensible_flow_m3_s,
            latent_flow_m3_s,
            m_dot_lat_kg_s: m_dot_lat,
            w_outdoor: w_out,
            raw_inf_m3_s: q_inf_m3_s,
            forced_flow_m3_s,
            nat_flow_m3_s: q_nat_m3_s,
        });
        *latent_out.entry(zone.id).or_insert(0.0) += q_latent;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use chrono::{FixedOffset, TimeZone};
    use hares_physics::air_properties::moist_air_density_kg_m3;
    use hares_types::{
        ElectricalSummary, EnvironmentState, GridState, PriceSignal, SurfaceIrradiance,
        WeatherState, ZoneId, ZoneState,
    };

    use super::{InfiltrationCoupling, apply_infiltration_and_ventilation};
    use crate::thermal_solver::config::{InfiltrationMethod, ThermalSolverConfig};

    fn make_env(t_out_c: f64, wind_m_s: f64, zone_temp_c: f64, volume_m3: f64) -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: zone_temp_c,
                humidity_ratio: 0.007,
                relative_humidity: 0.45,
                wet_bulb_c: zone_temp_c - 3.0,
                volume_m3,
            }],
            weather: WeatherState {
                outdoor_temp_c: t_out_c,
                outdoor_humidity_ratio: 0.005,
                wind_speed_m_s: wind_m_s,
                wind_dir_deg: 180.0,
                ground_temp_c: t_out_c,
                sky_temp_c: t_out_c - 5.0,
                pressure_kpa: 101.325,
                solar_irradiance: vec![SurfaceIrradiance {
                    surface_id: 1,
                    direct_w_m2: 0.0,
                    diffuse_w_m2: 0.0,
                    reflected_w_m2: 0.0,
                    angle_of_incidence_rad: 0.0,
                }],
                outdoor_wet_bulb_c: 0.0,
                outdoor_enthalpy_j_kg: 0.0,
                ghi_w_m2: 0.0,
                dni_w_m2: 0.0,
                dhi_w_m2: 0.0,
                solar_altitude_deg: 0.0,
                solar_azimuth_deg: 180.0,
                mains_temp_c: 15.0,
                rainfall_m: 0.0,
                ground_albedo: 0.2,
            },
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
            },
            custom_domains: vec![],
            equipment_telemetry: HashMap::new(),
            equipment_core: Default::default(),
            current_time: FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2026, 3, 18, 12, 0, 0)
                .single()
                .expect("valid time"),
            time_res: chrono::Duration::seconds(60),
            price_signal: PriceSignal::default(),
            electrical: ElectricalSummary::default(),
        }
    }

    fn make_two_zone_env(
        t_out_c: f64,
        wind_m_s: f64,
        indoor_temp_c: f64,
        other_temp_c: f64,
        volume_m3: f64,
        other_volume_m3: f64,
    ) -> EnvironmentState {
        let mut env = make_env(t_out_c, wind_m_s, indoor_temp_c, volume_m3);
        env.zones.push(ZoneState {
            id: ZoneId(2),
            temperature_c: other_temp_c,
            humidity_ratio: 0.007,
            relative_humidity: 0.45,
            wet_bulb_c: other_temp_c - 3.0,
            volume_m3: other_volume_m3,
        });
        env
    }

    #[test]
    fn density_is_dry_air_basis_kg_da_per_m3() {
        // psychrolib.GetMoistAirVolume(20, 0.010, 101325) = 0.84380 m³/kg_da
        // → 1/v = 1.18510 kg_da/m³   (reference from psychrolib 2.5)
        let rho = moist_air_density_kg_m3(101_325.0, 20.0, 0.010);
        assert!(
            (rho - 1.18510).abs() < 0.001,
            "expected ~1.185 kg_da/m³, got {rho}",
        );
    }

    /// ACH method: Q = ACH × V / 3600
    /// For a 300 m³ zone at 0.5 ACH: Q = 0.5 × 300 / 3600 = 0.04167 m³/s
    #[test]
    fn infiltration_constant_ach() {
        let volume_m3 = 300.0;
        let ach = 0.5_f64;
        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            infiltration: vec![(ZoneId(1), InfiltrationMethod::Ach { ach })],
            ..ThermalSolverConfig::default()
        };
        let env = make_env(5.0, 3.0, 21.0, volume_m3);
        let mut latent: HashMap<ZoneId, f64> = HashMap::new();
        let mut couplings: Vec<InfiltrationCoupling> = Vec::new();

        apply_infiltration_and_ventilation(&config, &env, false, &mut latent, &mut couplings);

        assert_eq!(couplings.len(), 1);
        let coupling = &couplings[0];

        let expected_flow_m3_s = ach * volume_m3 / 3600.0;
        assert!(
            (coupling.raw_inf_m3_s - expected_flow_m3_s).abs() < 1e-10,
            "ACH flow: expected {expected_flow_m3_s:.6} m³/s, got {:.6}",
            coupling.raw_inf_m3_s,
        );
        // Without forced or natural ventilation, combined flow equals raw infiltration.
        assert!(
            (coupling.combined_flow_m3_s - expected_flow_m3_s).abs() < 1e-10,
            "combined flow must equal raw ACH flow when no ventilation",
        );
    }

    /// AIM-2 with zero wind: only stack effect drives infiltration.
    /// Q_stack = c_s × |ΔT|^n_i must be non-zero when T_zone ≠ T_out.
    /// Q = sqrt(Q_stack² + 0²) = Q_stack -- no quadrature cancellation.
    #[test]
    fn infiltration_zero_wind_nonzero_stack() {
        // Typical single-storey residential coefficients from OCHRE defaults:
        // c_s = 0.000290 [m³/s / K^0.65] (stack coefficient with n_stories=1 baked in)
        // n_i = 0.65 (OCHRE/ResStock default pressure exponent)
        let c_s = 0.000_290_f64;
        let c_w = 0.000_150_f64; // non-zero but wind is zero, so this term vanishes
        let n_i = 0.65_f64;
        let t_zone = 21.0_f64;
        let t_out = 0.0_f64;
        let delta_t = (t_zone - t_out).abs(); // 21 K

        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            infiltration: vec![(
                ZoneId(1),
                InfiltrationMethod::AshraeWindStack {
                    c_s,
                    c_w,
                    shielding_coeff: 0.5,
                    n_i,
                },
            )],
            ..ThermalSolverConfig::default()
        };
        // wind_speed = 0.0 so wind term is zero
        let env = make_env(t_out, 0.0, t_zone, 300.0);
        let mut latent: HashMap<ZoneId, f64> = HashMap::new();
        let mut couplings: Vec<InfiltrationCoupling> = Vec::new();

        apply_infiltration_and_ventilation(&config, &env, false, &mut latent, &mut couplings);

        assert_eq!(couplings.len(), 1);
        let flow = couplings[0].raw_inf_m3_s;

        // Stack-only expected: Q = c_s × |ΔT|^n_i
        let expected = c_s * delta_t.powf(n_i);
        assert!(
            flow > 0.0,
            "stack infiltration must be non-zero for 21 K temperature difference, got {flow}",
        );
        assert!(
            (flow - expected).abs() < 1e-9,
            "AIM-2 zero-wind flow: expected {expected:.8} m³/s, got {flow:.8} m³/s",
        );
    }

    /// ELA variant: nominal conditions (non-zero wind and stack effect).
    /// Q = ela_cm2 * sqrt(stack_coeff * |ΔT| + wind_coeff * v²) * LPS_PER_CM2_TO_M3PS_PER_CM2
    /// Verify the solver returns a positive, non-trivial flow for known ELA inputs.
    #[test]
    fn infiltration_ela_nominal() {
        // ELA = 100 cm² = 0.01 m²; typical single-family home (LBNL/ASHRAE 136)
        let ela_m2 = 0.01_f64;
        let stack_coeff = 0.000_290_f64;
        let wind_coeff = 0.000_150_f64;
        let t_zone = 21.0_f64;
        let t_out = 0.0_f64;
        let wind_m_s = 3.0_f64;

        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            infiltration: vec![(
                ZoneId(1),
                InfiltrationMethod::Ela {
                    ela_m2,
                    stack_coeff,
                    wind_coeff,
                },
            )],
            ..ThermalSolverConfig::default()
        };
        let env = make_env(t_out, wind_m_s, t_zone, 300.0);
        let mut latent: HashMap<ZoneId, f64> = HashMap::new();
        let mut couplings: Vec<InfiltrationCoupling> = Vec::new();

        apply_infiltration_and_ventilation(&config, &env, false, &mut latent, &mut couplings);

        assert_eq!(couplings.len(), 1);
        let flow = couplings[0].raw_inf_m3_s;
        assert!(
            flow > 0.0,
            "ELA infiltration must be positive under stack+wind, got {flow}"
        );

        // Hand-calculate expected: ela_cm2 = 0.01 * 10_000 = 100 cm²
        // driver = stack_coeff * |ΔT| + wind_coeff * v² = 0.000290*21 + 0.000150*9
        // flow_lps_per_cm2 = sqrt(driver); flow_m3_s = ela_cm2 * flow_lps_per_cm2 / 1000
        // (LPS_PER_CM2_TO_M3PS_PER_CM2 = 1/1000)
        let ela_cm2 = ela_m2 * 10_000.0;
        let driver = stack_coeff * (t_zone - t_out).abs() + wind_coeff * wind_m_s.powi(2);
        let expected = ela_cm2 * driver.sqrt() / 1000.0;
        assert!(
            (flow - expected).abs() < 1e-9,
            "ELA flow: expected {expected:.8} m³/s, got {flow:.8} m³/s"
        );
    }

    /// ELA variant: zero wind -- only stack effect drives infiltration.
    /// Q = ela_cm2 * sqrt(stack_coeff * |ΔT|) / 1000
    #[test]
    fn infiltration_ela_zero_wind() {
        let ela_m2 = 0.01_f64;
        let stack_coeff = 0.000_290_f64;
        let wind_coeff = 0.000_150_f64;
        let t_zone = 21.0_f64;
        let t_out = 0.0_f64;

        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            infiltration: vec![(
                ZoneId(1),
                InfiltrationMethod::Ela {
                    ela_m2,
                    stack_coeff,
                    wind_coeff,
                },
            )],
            ..ThermalSolverConfig::default()
        };
        let env = make_env(t_out, 0.0, t_zone, 300.0);
        let mut latent: HashMap<ZoneId, f64> = HashMap::new();
        let mut couplings: Vec<InfiltrationCoupling> = Vec::new();

        apply_infiltration_and_ventilation(&config, &env, false, &mut latent, &mut couplings);

        let flow = couplings[0].raw_inf_m3_s;
        let ela_cm2 = ela_m2 * 10_000.0;
        let expected = ela_cm2 * (stack_coeff * (t_zone - t_out).abs()).sqrt() / 1000.0;
        assert!(
            flow > 0.0,
            "stack-only ELA infiltration must be positive, got {flow}"
        );
        assert!(
            (flow - expected).abs() < 1e-9,
            "ELA zero-wind: expected {expected:.8} m³/s, got {flow:.8} m³/s"
        );
    }

    /// Duct leakage interaction with HVAC active: supply > return pressurises the house.
    ///
    /// ASHRAE 152 §9.3 (pressurisation branch):
    ///   infil_fan_off = 0.35 × V / 60 = 0.35 × 300 / 60 = 1.75 m³/s
    ///   imb           = |supply − return| = |0.02 − 0.01| = 0.01 m³/s
    ///   adjusted      = (1.75^1.5 + 0.01^1.5)^0.67
    ///   result        = base × (adjusted / infil_fan_off)
    #[test]
    fn infiltration_duct_leakage_interaction_hvac_on() {
        let volume_m3 = 300.0_f64;
        let ach = 0.5_f64;
        let supply = 0.02_f64;
        let ret = 0.01_f64;

        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            infiltration: vec![(ZoneId(1), InfiltrationMethod::Ach { ach })],
            supply_duct_leakage_m3_s: supply,
            return_duct_leakage_m3_s: ret,
            ..ThermalSolverConfig::default()
        };
        let env = make_env(5.0, 3.0, 21.0, volume_m3);

        let mut latent: HashMap<ZoneId, f64> = HashMap::new();
        let mut couplings: Vec<InfiltrationCoupling> = Vec::new();
        apply_infiltration_and_ventilation(&config, &env, true, &mut latent, &mut couplings);
        let adjusted_flow = couplings[0].raw_inf_m3_s;

        // Hand-calculated expected value from ASHRAE 152 §9.3 pressurisation formula.
        let base_infil = ach * volume_m3 / 3600.0;
        let infil_fan_off = 0.35 * volume_m3 / 60.0;
        let imb = (supply - ret).abs();
        let adjusted = (infil_fan_off.powf(1.5) + imb.powf(1.5)).powf(0.67);
        let expected = base_infil * (adjusted / infil_fan_off);

        assert!(
            (adjusted_flow - expected).abs() < 1e-9,
            "ASHRAE 152 pressurisation: expected {expected:.9} m³/s, got {adjusted_flow:.9} m³/s"
        );
    }

    /// Duct leakage config present but hvac_active = false: no adjustment applied.
    /// raw_inf_m3_s must equal base ACH flow.
    #[test]
    fn infiltration_duct_leakage_hvac_off() {
        let volume_m3 = 300.0_f64;
        let ach = 0.5_f64;

        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            infiltration: vec![(ZoneId(1), InfiltrationMethod::Ach { ach })],
            supply_duct_leakage_m3_s: 0.02,
            return_duct_leakage_m3_s: 0.01,
            ..ThermalSolverConfig::default()
        };
        let env = make_env(5.0, 3.0, 21.0, volume_m3);
        let mut latent: HashMap<ZoneId, f64> = HashMap::new();
        let mut couplings: Vec<InfiltrationCoupling> = Vec::new();

        apply_infiltration_and_ventilation(&config, &env, false, &mut latent, &mut couplings);

        let flow = couplings[0].raw_inf_m3_s;
        let expected = ach * volume_m3 / 3600.0;
        assert!(
            (flow - expected).abs() < 1e-10,
            "HVAC off: duct leakage must not affect infiltration; \
             expected {expected:.6} m³/s, got {flow:.6} m³/s"
        );
    }

    /// Natural ventilation open: zone is warm (26°C), outdoor is cool and dry (15°C, W=0.005).
    ///
    /// Hand-calculated from `natural_ventilation_flow_m3_s` formula:
    ///   A_eff   = 0.5 × 0.6 × 10000 = 3000 cm²
    ///   adj     = (26 − 22.778) / (26 − 15) = 3.222 / 11 ≈ 0.29291
    ///   driver  = 0.000290 × 11 + 0.000150 × 4 = 0.003790
    ///   q_nat   = 3000 × adj × √0.003790 / 1000
    ///   (capped at 20 ACH = 20 × 300 / 3600 = 1.667 m³/s, not binding here)
    #[test]
    fn infiltration_natural_ventilation_open() {
        use crate::thermal_solver::config::NaturalVentilationConfig;

        let open_area_m2 = 0.5_f64;
        let stack_coeff = 0.000_290_f64;
        let wind_coeff = 0.000_150_f64;
        let t_base_c = NaturalVentilationConfig::DEFAULT_T_BASE_C;
        let t_zone = 26.0_f64;
        let t_out = 15.0_f64;
        let wind_m_s = 2.0_f64;
        let volume_m3 = 300.0_f64;

        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            infiltration: vec![(ZoneId(1), InfiltrationMethod::Ach { ach: 0.0 })],
            natural_ventilation: Some(NaturalVentilationConfig {
                open_area_m2,
                stack_coeff,
                wind_coeff,
                t_base_c,
                max_outdoor_humidity_ratio:
                    NaturalVentilationConfig::DEFAULT_MAX_OUTDOOR_HUMIDITY_RATIO,
            }),
            ..ThermalSolverConfig::default()
        };
        // zone=26°C > t_base=22.778°C > outdoor=15°C; w_out=0.005 < 0.0115
        let mut env = make_env(t_out, wind_m_s, t_zone, volume_m3);
        env.weather.outdoor_humidity_ratio = 0.005;

        let mut latent: HashMap<ZoneId, f64> = HashMap::new();
        let mut couplings: Vec<InfiltrationCoupling> = Vec::new();

        apply_infiltration_and_ventilation(&config, &env, false, &mut latent, &mut couplings);

        let nat_flow = couplings[0].nat_flow_m3_s;

        // Hand-calculated expected value from the implementation formula.
        let nat_vent_area_cm2 = open_area_m2 * 0.6 * 10_000.0;
        let adj = ((t_zone - t_base_c) / (t_zone - t_out)).clamp(0.0, 1.0);
        let driver = stack_coeff * (t_zone - t_out).abs() + wind_coeff * wind_m_s.powi(2);
        let q_expected =
            (nat_vent_area_cm2 * adj * driver.sqrt() / 1000.0).min(20.0 * volume_m3 / 3600.0);

        assert!(
            (nat_flow - q_expected).abs() < 1e-9,
            "natural ventilation: expected {q_expected:.9} m³/s, got {nat_flow:.9} m³/s"
        );
    }

    /// Natural ventilation closed: zone temp ≤ outdoor temp -- stack effect cannot drive
    /// outward exhaust, so nat_flow must be zero.
    #[test]
    fn infiltration_natural_ventilation_closed() {
        use crate::thermal_solver::config::NaturalVentilationConfig;

        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            infiltration: vec![(ZoneId(1), InfiltrationMethod::Ach { ach: 0.0 })],
            natural_ventilation: Some(NaturalVentilationConfig {
                open_area_m2: 0.5,
                stack_coeff: 0.000_290,
                wind_coeff: 0.000_150,
                t_base_c: NaturalVentilationConfig::DEFAULT_T_BASE_C,
                max_outdoor_humidity_ratio:
                    NaturalVentilationConfig::DEFAULT_MAX_OUTDOOR_HUMIDITY_RATIO,
            }),
            ..ThermalSolverConfig::default()
        };
        // zone=18°C < outdoor=20°C → gating condition fails
        let env = make_env(20.0, 2.0, 18.0, 300.0);

        let mut latent: HashMap<ZoneId, f64> = HashMap::new();
        let mut couplings: Vec<InfiltrationCoupling> = Vec::new();

        apply_infiltration_and_ventilation(&config, &env, false, &mut latent, &mut couplings);

        let nat_flow = couplings[0].nat_flow_m3_s;
        assert!(
            nat_flow.abs() < 1e-12,
            "natural ventilation must be zero when zone ≤ outdoor, got {nat_flow}"
        );
    }

    /// Natural ventilation is scoped to the indoor zone only.
    #[test]
    fn infiltration_natural_ventilation_indoor_only() {
        use crate::thermal_solver::config::NaturalVentilationConfig;

        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            infiltration: vec![
                (ZoneId(1), InfiltrationMethod::Ach { ach: 0.0 }),
                (ZoneId(2), InfiltrationMethod::Ach { ach: 0.0 }),
            ],
            natural_ventilation: Some(NaturalVentilationConfig {
                open_area_m2: 0.5,
                stack_coeff: 0.000_290,
                wind_coeff: 0.000_150,
                t_base_c: NaturalVentilationConfig::DEFAULT_T_BASE_C,
                max_outdoor_humidity_ratio:
                    NaturalVentilationConfig::DEFAULT_MAX_OUTDOOR_HUMIDITY_RATIO,
            }),
            ..ThermalSolverConfig::default()
        };
        let env = make_two_zone_env(15.0, 2.0, 26.0, 26.0, 300.0, 250.0);

        let mut latent: HashMap<ZoneId, f64> = HashMap::new();
        let mut couplings: Vec<InfiltrationCoupling> = Vec::new();

        apply_infiltration_and_ventilation(&config, &env, false, &mut latent, &mut couplings);

        assert_eq!(couplings.len(), 2);
        assert!(
            couplings
                .iter()
                .find(|c| c.zone == ZoneId(1))
                .expect("indoor coupling expected")
                .nat_flow_m3_s
                > 0.0,
            "indoor zone should receive natural ventilation"
        );
        assert_eq!(
            couplings
                .iter()
                .find(|c| c.zone == ZoneId(2))
                .expect("other zone coupling expected")
                .nat_flow_m3_s,
            0.0,
            "non-indoor zone must not receive natural ventilation"
        );
    }

    /// Latent output: outdoor humidity ratio > indoor → latent_out must be positive
    /// (moisture flows into the zone).  The magnitude must match
    /// m_dot_lat * H_FG * (w_out - w_zone).
    #[test]
    fn infiltration_latent_output() {
        use super::super::H_FG_J_PER_KG;
        use hares_physics::air_properties::moist_air_density_kg_m3;

        let volume_m3 = 300.0_f64;
        let ach = 0.5_f64;
        let w_indoor = 0.005_f64;
        let w_outdoor = 0.012_f64;
        let t_zone = 21.0_f64;
        let t_out = 10.0_f64;
        let p_pa = 101_325.0_f64;

        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            infiltration: vec![(ZoneId(1), InfiltrationMethod::Ach { ach })],
            ..ThermalSolverConfig::default()
        };

        let mut env = make_env(t_out, 0.0, t_zone, volume_m3);
        env.weather.outdoor_humidity_ratio = w_outdoor;
        env.zones[0].humidity_ratio = w_indoor;
        env.weather.pressure_kpa = p_pa / 1000.0;

        let mut latent: HashMap<ZoneId, f64> = HashMap::new();
        let mut couplings: Vec<InfiltrationCoupling> = Vec::new();

        apply_infiltration_and_ventilation(&config, &env, false, &mut latent, &mut couplings);

        let q_latent = latent.get(&ZoneId(1)).copied().unwrap_or(0.0);
        assert!(
            q_latent > 0.0,
            "outdoor humidity > indoor → positive latent gain, got {q_latent}"
        );

        // First-principles check: q_latent = rho * Q * H_FG * (w_out - w_in)
        let rho = moist_air_density_kg_m3(p_pa, t_out, w_outdoor);
        let q_flow = ach * volume_m3 / 3600.0;
        let m_dot = rho * q_flow;
        let expected = m_dot * H_FG_J_PER_KG * (w_outdoor - w_indoor);
        assert!(
            (q_latent - expected).abs() / expected < 0.01,
            "latent output: expected ~{expected:.2} W, got {q_latent:.2} W"
        );
    }

    /// Energy decomposition: infiltration + forced_vent + natural_vent ≈ sensible_diagnostic.
    ///
    /// The three scaled components are computed from the combined sensible flow split back
    /// through proportional scaling. Their sum must equal h_inf × (T_out - T_zone)
    /// (the q_sensible_diagnostic_w field), confirming mass conservation through the
    /// flow decomposition logic.
    #[test]
    fn infiltration_energy_decomposition_sums() {
        // Setup: balanced ventilation with known flows so decomposition is deterministic.
        // Forced flow = 0.02 m³/s (balanced ERV, 75% sensible recovery efficiency).
        // Infiltration via ACH=1.0 on 300 m³ zone → 300/3600 = 0.0833 m³/s.
        // With balanced ventilation: sensible_flow = nat_flow + forced × (1 - 0.75).
        let volume_m3 = 300.0_f64;
        let ach = 1.0_f64;
        let forced_m3_s = 0.02_f64;
        let sens_recovery = 0.75_f64;

        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            infiltration: vec![(ZoneId(1), InfiltrationMethod::Ach { ach })],
            ventilation_flow_m3_s: forced_m3_s,
            ventilation: crate::thermal_solver::config::MechanicalVentilationParams {
                balanced: true,
                sensible_recovery_efficiency: sens_recovery,
                latent_recovery_efficiency: 0.0,
                zone_flow_m3_s: HashMap::new(),
            },
            ..ThermalSolverConfig::default()
        };

        // Cold outdoor → large ΔT to produce a measurable sensible gain.
        let env = make_env(-10.0, 3.0, 21.0, volume_m3);
        let mut latent: HashMap<ZoneId, f64> = HashMap::new();
        let mut couplings: Vec<InfiltrationCoupling> = Vec::new();

        apply_infiltration_and_ventilation(&config, &env, false, &mut latent, &mut couplings);

        assert_eq!(couplings.len(), 1);
        let c = &couplings[0];

        // The three diagnostic components must sum to the overall sensible diagnostic.
        // q_sensible_diagnostic = h_inf × (T_out - T_zone); components are scaled
        // re-attributions of the same combined flow, so their sum must equal it.
        let decomposition_sum = c.q_infiltration_w + c.q_forced_vent_w + c.q_natural_vent_w;
        assert!(
            (decomposition_sum - c.q_sensible_diagnostic_w).abs() < 0.01,
            "energy decomposition sum {decomposition_sum:.4} W ≠ q_sensible_diagnostic \
             {:.4} W (diff = {:.6} W)",
            c.q_sensible_diagnostic_w,
            (decomposition_sum - c.q_sensible_diagnostic_w).abs(),
        );

        // Sanity: sensible diagnostic must be negative (cold outdoor → heat loss).
        assert!(
            c.q_sensible_diagnostic_w < 0.0,
            "cold outdoor must produce heat loss, got {:.2} W",
            c.q_sensible_diagnostic_w,
        );
    }

    // -------------------------------------------------------------------------
    // Regression test for ERV/HRV bypass/defrost effectiveness not
    // propagated to the thermal solver.
    //
    // The bug: `ThermalSolverConfig.ventilation.sensible_recovery_efficiency` is
    // set once at init from rated values and never updated per-timestep.  When
    // bypass is active the infiltration solver should receive 0.0 effectiveness
    // (full outdoor load), but instead it sees the rated 0.75 value — a 4×
    // underestimate of the ventilation sensible load.
    //
    // These tests confirm the *solver* contract: if the caller correctly propagates
    // effectiveness = 0.0 (bypass) the sensible flow is forced_flow × 1.0;
    // with effectiveness = 0.75 (rated) it is forced_flow × 0.25.  The 4× ratio
    // is the energy-balance error that occurs whenever the propagation is absent.
    // -------------------------------------------------------------------------

    /// Bypass active (effectiveness = 0.0): ventilation sensible flow must equal
    /// the full forced flow rate (no heat recovery).
    ///
    /// This is the *correct* path — it verifies that the solver produces the right
    /// answer when the orchestration layer has propagated `eff_s = 0.0`.  The
    /// complementary test below shows the wrong answer produced by the stale rated
    /// value.
    #[test]
    fn bypass_zero_effectiveness_gives_full_ventilation_load() {
        let forced_m3_s = 0.03_f64;

        // Bypass active: effectiveness = 0.0
        let config_bypass = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            infiltration: vec![],
            ventilation_flow_m3_s: forced_m3_s,
            ventilation: crate::thermal_solver::config::MechanicalVentilationParams {
                balanced: true,
                sensible_recovery_efficiency: 0.0, // bypass: no recovery
                latent_recovery_efficiency: 0.0,
                zone_flow_m3_s: HashMap::new(),
            },
            ..ThermalSolverConfig::default()
        };

        // Cold outdoor → large ΔT for a measurable load.
        let env = make_env(-10.0, 0.0, 20.0, 200.0);
        let mut latent: HashMap<ZoneId, f64> = HashMap::new();
        let mut couplings: Vec<InfiltrationCoupling> = Vec::new();
        apply_infiltration_and_ventilation(
            &config_bypass,
            &env,
            false,
            &mut latent,
            &mut couplings,
        );

        assert_eq!(couplings.len(), 1, "expected one zone coupling");
        let c = &couplings[0];

        // With effectiveness = 0.0: sensible_flow = forced_flow × (1 - 0.0) = forced_flow
        // h_inf_w_k = rho × forced_flow × cp (rho depends on moist air, ~1.2–1.33 kg/m³)
        // Just confirm it is in the expected ballpark: rho ∈ [1.1, 1.4], so h_inf ∈ [33, 51] W/K.
        let expected_h_inf_approx = 1.2 * forced_m3_s * hares_physics::constants::CP_DRY_AIR_J_KG_K;
        assert!(
            c.h_inf_w_k > expected_h_inf_approx * 0.85
                && c.h_inf_w_k < expected_h_inf_approx * 1.20,
            "bypass: h_inf_w_k should be ≈ forced_flow × rho × cp ≈ {expected_h_inf_approx:.2} W/K, got {:.2} W/K",
            c.h_inf_w_k
        );

        // q_forced_vent_w should dominate (no infiltration configured)
        assert!(
            c.q_forced_vent_w < -1.0,
            "bypass: forced vent sensible gain must be a significant heat loss (cold outdoor), got {:.2} W",
            c.q_forced_vent_w
        );
    }

    /// Rated effectiveness (0.75) gives a 4× smaller forced-ventilation sensible load
    /// than bypass effectiveness (0.0) at the same conditions.
    ///
    /// This is the ratio that the bug silently applies when bypass is active but the
    /// orchestration layer has NOT updated `sensible_recovery_efficiency`.  Confirming
    /// the 4× ratio documents exactly what goes wrong when the fix is absent.
    #[test]
    fn rated_vs_bypass_effectiveness_ratio_is_4x() {
        let forced_m3_s = 0.03_f64;

        let make_config = |eff: f64| ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            infiltration: vec![],
            ventilation_flow_m3_s: forced_m3_s,
            ventilation: crate::thermal_solver::config::MechanicalVentilationParams {
                balanced: true,
                sensible_recovery_efficiency: eff,
                latent_recovery_efficiency: 0.0,
                zone_flow_m3_s: HashMap::new(),
            },
            ..ThermalSolverConfig::default()
        };

        let env = make_env(-10.0, 0.0, 20.0, 200.0);

        let mut lat_bypass: HashMap<ZoneId, f64> = HashMap::new();
        let mut coup_bypass: Vec<InfiltrationCoupling> = Vec::new();
        apply_infiltration_and_ventilation(
            &make_config(0.0),
            &env,
            false,
            &mut lat_bypass,
            &mut coup_bypass,
        );

        let mut lat_rated: HashMap<ZoneId, f64> = HashMap::new();
        let mut coup_rated: Vec<InfiltrationCoupling> = Vec::new();
        apply_infiltration_and_ventilation(
            &make_config(0.75),
            &env,
            false,
            &mut lat_rated,
            &mut coup_rated,
        );

        let h_bypass = coup_bypass[0].h_inf_w_k;
        let h_rated = coup_rated[0].h_inf_w_k;

        // With balanced ventilation and no infiltration:
        //   bypass  (eff=0.00): sensible_flow = forced * (1 - 0.00) = 1.00 × forced
        //   rated   (eff=0.75): sensible_flow = forced * (1 - 0.75) = 0.25 × forced
        // Ratio = 4.0
        let ratio = h_bypass / h_rated;
        assert!(
            (ratio - 4.0).abs() < 0.01,
            "bypass/rated h_inf ratio must be 4.0 (energy-balance error), got {ratio:.4}"
        );
    }
}
