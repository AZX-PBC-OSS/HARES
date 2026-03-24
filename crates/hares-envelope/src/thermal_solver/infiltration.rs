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
/// This makes infiltration unconditionally stable regardless of ACH or timestep,
/// following the EnergyPlus zone air heat balance (Engineering Reference §13.3,
/// Predictor-Corrector algorithm: `C_z dT/dt = ... + m_inf*cp*(T_out - T_z)`
/// where `m_inf*cp` enters the implicit denominator coefficient).
#[derive(Debug, Clone)]
pub(crate) struct InfiltrationCoupling {
    pub zone: ZoneId,
    /// Infiltration+ventilation sensible conductance [W/K] = m_dot_sens * cp.
    pub h_inf_w_k: f64,
    /// Outdoor temperature driving the sensible forcing [°C].
    pub t_forcing_c: f64,
    /// Latent gain [W] — stays fully explicit (not temperature-dependent).
    /// Retained for diagnostic parity with q_sensible_diagnostic_w.
    #[allow(dead_code)]
    pub q_latent_w: f64,
    /// Diagnostic sensible gain [W] = h_inf * (T_out - T_zone) for reporting.
    #[allow(dead_code)]
    pub q_sensible_diagnostic_w: f64,
    /// Pure infiltration sensible gain [W] — excludes ventilation components.
    pub q_infiltration_w: f64,
    /// Forced mechanical ventilation sensible gain [W] — after recovery efficiency.
    pub q_forced_vent_w: f64,
    /// Natural ventilation sensible gain [W] — operable window stack/wind flow.
    pub q_natural_vent_w: f64,
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

        // Natural ventilation flow (operable windows).
        let q_nat_m3_s = config
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
            .unwrap_or(0.0);

        // Forced mechanical ventilation flow.
        let forced_flow_m3_s = config
            .ventilation
            .zone_flow_m3_s
            .get(&zone.id)
            .copied()
            .unwrap_or(config.ventilation_flow_m3_s);

        // Compute per-component diagnostic gains before flow combination.
        let delta_t = t_out - zone.temperature_c;
        let q_infiltration_w = rho * q_inf_m3_s * CP_DRY_AIR_J_KG_K * delta_t;
        let q_natural_vent_w = rho * q_nat_m3_s * CP_DRY_AIR_J_KG_K * delta_t;
        let forced_eff = if config.ventilation.balanced {
            1.0 - config.ventilation.sensible_recovery_efficiency
        } else {
            1.0
        };
        let q_forced_vent_w = rho * forced_flow_m3_s * forced_eff * CP_DRY_AIR_J_KG_K * delta_t;

        // Combine infiltration + natural ventilation + forced ventilation.
        // OCHRE Envelope.py:59-87:
        //   total_nat_flow = infiltration + natural_ventilation
        //   balanced:   sensible_flow = total_nat + forced * (1 - sens_recovery_eff)
        //               latent_flow   = total_nat + forced * (1 - lat_recovery_eff)
        //   unbalanced: flow = sqrt(total_nat² + forced²)  (quadrature combination)
        let total_nat_flow = q_inf_m3_s + q_nat_m3_s;
        let (sensible_flow_m3_s, latent_flow_m3_s) = if config.ventilation.balanced {
            (
                total_nat_flow
                    + forced_flow_m3_s * (1.0 - config.ventilation.sensible_recovery_efficiency),
                total_nat_flow
                    + forced_flow_m3_s * (1.0 - config.ventilation.latent_recovery_efficiency),
            )
        } else {
            let combined =
                (total_nat_flow * total_nat_flow + forced_flow_m3_s * forced_flow_m3_s).sqrt();
            (combined, combined)
        };

        // Combined flow drives sensible/latent loads against outdoor conditions.
        // Recovery efficiency already accounts for HRV/ERV heat exchange by
        // reducing the effective flow rate (OCHRE model).
        let m_dot_sens = rho * sensible_flow_m3_s;
        let m_dot_lat = rho * latent_flow_m3_s;
        let h_inf = m_dot_sens * CP_DRY_AIR_J_KG_K;
        let q_sensible_diagnostic = h_inf * (t_out - zone.temperature_c);
        let q_latent = m_dot_lat * H_FG_J_PER_KG * (w_out - zone.humidity_ratio);

        couplings.push(InfiltrationCoupling {
            zone: zone.id,
            h_inf_w_k: h_inf,
            t_forcing_c: t_out,
            q_latent_w: q_latent,
            q_sensible_diagnostic_w: q_sensible_diagnostic,
            q_infiltration_w,
            q_forced_vent_w,
            q_natural_vent_w,
        });
        *latent_out.entry(zone.id).or_insert(0.0) += q_latent;
    }
}

#[cfg(test)]
mod tests {
    use hares_physics::air_properties::moist_air_density_kg_m3;

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
}
