//! Distributed energy resource (PV, battery, EV, generator) resolution from HPXML.

use std::collections::HashMap;

use hares_equipment::{BatteryConfig, EvConfig, GeneratorConfig, PvConfig};
use hares_types::FuelType;

use super::HpxmlError;
use super::building::XmlNode;
use super::equipment::{EquipmentSpec, build_typed_spec};
use super::xml_helpers::{child_energy_kwh, child_f64, child_text, element_id, parse_fuel};

use crate::defaults::DefaultsStore;

/// kBtu → kWh: 1 kBtu(IT) = 0.293_071_07 kWh exactly.
const KBTU_TO_KWH: f64 = 0.293_071_07;

pub(super) fn resolve_pv(
    details: &XmlNode,
    defaults: &DefaultsStore,
    specs: &mut Vec<EquipmentSpec>,
) -> std::result::Result<(), HpxmlError> {
    let Some(photovoltaics) = details.path(&["Systems", "Photovoltaics"]) else {
        return Ok(());
    };

    let mut inverter_eff_by_id: HashMap<String, f64> = HashMap::new();
    let mut inverter_cap_kw_by_id: HashMap<String, f64> = HashMap::new();
    for inverter in photovoltaics.children_named("Inverter") {
        let Some(id) = element_id(inverter) else {
            continue;
        };
        if let Some(eff) = child_f64(inverter, "InverterEfficiency") {
            inverter_eff_by_id.insert(id.clone(), eff);
        }
        if let Some(max_power_w) = child_f64(inverter, "MaxPowerOutput") {
            inverter_cap_kw_by_id.insert(id, max_power_w / 1000.0);
        }
    }

    for pv in photovoltaics.children_named("PVSystem") {
        if let Some(tracking) = child_text(pv, "Tracking") {
            if !tracking.trim().eq_ignore_ascii_case("fixed") {
                let id = element_id(pv).unwrap_or_else(|| "PVSystem".to_string());
                return Err(HpxmlError::Parse(format!(
                    "PV system `{id}` uses unsupported tracking mode `{}`; only `fixed` is supported",
                    tracking.trim()
                )));
            }
        }

        let inverter_id = pv
            .child("AttachedToInverter")
            .and_then(|n| n.attrs.get("idref"));
        let inverter_eff = inverter_id
            .and_then(|id| inverter_eff_by_id.get(id))
            .copied();
        let inverter_capacity_kw = inverter_id
            .and_then(|id| inverter_cap_kw_by_id.get(id))
            .copied();

        let cfg = PvConfig {
            equipment_id: None,
            zone_id: None,
            capacity_kw: child_f64(pv, "MaxPowerOutput")
                .map(|watts| watts / 1000.0)
                .unwrap_or(0.0),
            tilt_deg: child_f64(pv, "ArrayTilt"),
            azimuth_deg: child_f64(pv, "ArrayAzimuth"),
            module_type: child_text(pv, "ModuleType"),
            noct_c: None,
            system_losses_fraction: child_f64(pv, "SystemLossesFraction"),
            inverter_efficiency: inverter_eff,
            inverter_capacity_kw,
            power_factor: None,
            surface_resolution_deg: None,
            sam_lut_path: None,
        };

        specs.push(build_typed_spec(
            "PV".to_string(),
            FuelType::Electric,
            cfg,
            defaults,
        ));
    }

    Ok(())
}

pub(super) fn resolve_batteries(
    details: &XmlNode,
    defaults: &DefaultsStore,
    specs: &mut Vec<EquipmentSpec>,
) {
    let Some(batteries) = details.path(&["Systems", "Batteries"]) else {
        return;
    };

    for battery in batteries.children_named("Battery") {
        let rated_power_kw = child_f64(battery, "RatedPowerOutput").unwrap_or(5.0);
        let cfg = BatteryConfig {
            equipment_id: None,
            zone_id: None,
            capacity_kwh: child_energy_kwh(battery, "NominalCapacity").unwrap_or(13.5),
            max_charge_kw: rated_power_kw,
            max_discharge_kw: rated_power_kw,
            n_series: None,
            n_parallel: None,
            ah_cell: None,
            v_cell: None,
            cell_resistance_ohm: None,
            pack_voltage_v: None,
            chemistry: None,
            standby_power_w: None,
            self_discharge_pct_per_day: None,
            min_soc: None,
            max_soc: None,
            initial_soc: None,
            import_limit_w: None,
            export_limit_w: None,
            heater_power_w: None,
            heater_threshold_c: None,
            heater_on_discharge: None,
            min_discharge_temp_c: None,
            full_power_temp_c: None,
            min_charge_temp_c: None,
            cell_thermal_mass_j_per_k: None,
            cell_ua_w_per_k: None,
            inverter_efficiency: child_f64(battery, "RoundTripEfficiency").map(f64::sqrt),
            charge_efficiency: None,
            discharge_efficiency: None,
            bms_mode: None,
            grid_export_rule: None,
        };
        specs.push(build_typed_spec(
            "Battery".to_string(),
            FuelType::Electric,
            cfg,
            defaults,
        ));
    }
}

pub(super) fn resolve_ev(
    details: &XmlNode,
    defaults: &DefaultsStore,
    specs: &mut Vec<EquipmentSpec>,
) {
    if let Some(evs) = details.path(&["Systems", "ElectricVehicles"]) {
        for ev in evs.children_named("ElectricVehicle") {
            let cfg = EvConfig {
                equipment_id: None,
                capacity_kwh: child_energy_kwh(ev, "BatteryCapacity").unwrap_or(75.0),
                charging_level: child_text(ev, "ChargingLevel"),
                max_charging_power_kw: child_f64(ev, "MaxChargingPower").unwrap_or(11.5),
                charging_efficiency: None,
                l1_current_a: None,
                l1_voltage_v: None,
                soc_max: None,
                initial_soc: None,
                battery_temp_c: None,
                min_charge_temp_c: None,
                full_power_temp_c: None,
                heater_power_w: None,
                heater_threshold_c: None,
                thermal_mass_j_per_k: None,
                ua_w_per_k: None,
                v2l_enabled: None,
                v2l_soc_reserve: None,
                v2l_max_discharge_kw: None,
                v2g_enabled: None,
                v2g_soc_reserve: None,
                v2g_max_discharge_kw: None,
                chemistry: None,
                fuel_economy_kwh_per_mi: None,
                ready_soc: None,
                charging_strategy: None,
                plug_in_policy: None,
                power_limit_kw: None,
                initial_connection_state: None,
            };
            specs.push(build_typed_spec(
                "Electric Vehicle".to_string(),
                FuelType::Electric,
                cfg,
                defaults,
            ));
        }
    }
}

pub(super) fn resolve_generators(
    details: &XmlNode,
    defaults: &DefaultsStore,
    specs: &mut Vec<EquipmentSpec>,
) {
    let Some(generators) = details.path(&["Systems", "extension", "Generators"]) else {
        return;
    };

    for generator in generators.children_named("Generator") {
        let fuel = parse_fuel(child_text(generator, "FuelType").as_deref());
        let annual_output_kwh = child_f64(generator, "AnnualOutputkWh");
        let annual_consumption_kbtu = child_f64(generator, "AnnualConsumptionkBtu");
        let eta_electric = if let (Some(out_kwh), Some(cons_kbtu)) =
            (annual_output_kwh, annual_consumption_kbtu)
        {
            let cons_kwh = cons_kbtu * KBTU_TO_KWH;
            if cons_kwh > 0.0 {
                Some((out_kwh / cons_kwh).clamp(0.0, 1.0))
            } else {
                None
            }
        } else {
            None
        };

        let cfg = GeneratorConfig {
            equipment_id: None,
            zone_id: None,
            fuel_type: Some(fuel),
            rated_power_kw: child_f64(generator, "ElectricalPowerOutput").unwrap_or(10.0),
            eta_electric,
            eta_thermal: None,
            efficiency_type: None,
            efficiency_curve_points: None,
            delta_kw_per_s: None,
            capacity_min_kw: None,
            grid_import_limit_kw: None,
            export_limit_kw: None,
            loop_id: None,
            flow_rate_kg_s: None,
            supply_temp_c: None,
            return_temp_c: None,
        };

        specs.push(build_typed_spec(
            "Gas Generator".to_string(),
            fuel,
            cfg,
            defaults,
        ));
    }
}
