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
        let pv_id_opt = element_id(pv);
        let pv_id = pv_id_opt.clone().unwrap_or_else(|| "unknown".to_string());
        if let Some(tracking) = child_text(pv, "Tracking") {
            if !tracking.trim().eq_ignore_ascii_case("fixed") {
                return Err(HpxmlError::Parse(format!(
                    "PV system `{pv_id}` uses unsupported tracking mode `{}`; only `fixed` is supported",
                    tracking.trim()
                ).into()));
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

        let capacity_kw = child_f64(pv, "MaxPowerOutput")
            .map(|watts| watts / 1000.0)
            .ok_or_else(|| HpxmlError::MissingField {
                path: "PVSystem/MaxPowerOutput",
                system_kind: "PV",
                system_id: pv_id.clone(),
                reason:
                    "DC nameplate power (W) is required to size the array; no silent default permitted",
            })?;

        let cfg = PvConfig {
            equipment_id: None,
            zone_id: None,
            capacity_kw,
            tilt_deg: child_f64(pv, "ArrayTilt"),
            azimuth_deg: child_f64(pv, "ArrayAzimuth"),
            module_type: child_text(pv, "ModuleType"),
            noct_c: None,
            array_type: None,
            system_losses_fraction: child_f64(pv, "SystemLossesFraction"),
            inverter_efficiency: inverter_eff,
            inverter_capacity_kw,
            power_factor: None,
            surface_resolution_deg: None,
            sam_lut_path: None,
            arrays: None,
        };

        let mut spec = build_typed_spec("PV".to_string(), FuelType::Electric, cfg, defaults)?;
        spec.system_id = pv_id_opt;
        specs.push(spec);
    }

    Ok(())
}

pub(super) fn resolve_batteries(
    details: &XmlNode,
    defaults: &DefaultsStore,
    specs: &mut Vec<EquipmentSpec>,
) -> std::result::Result<(), HpxmlError> {
    let Some(batteries) = details.path(&["Systems", "Batteries"]) else {
        return Ok(());
    };

    for battery in batteries.children_named("Battery") {
        let battery_id_opt = element_id(battery);
        let battery_id = battery_id_opt
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        let rated_power_kw = child_f64(battery, "RatedPowerOutput")
            .map(|w| w / 1000.0)
            .ok_or_else(|| HpxmlError::MissingField {
                path: "Battery/RatedPowerOutput",
                system_kind: "Battery",
                system_id: battery_id.clone(),
                reason:
                    "rated charge/discharge power (W) is required to size the inverter; no silent default permitted",
            })?;
        let capacity_kwh = child_energy_kwh(battery, "NominalCapacity").ok_or_else(|| {
            HpxmlError::MissingField {
                path: "Battery/NominalCapacity",
                system_kind: "Battery",
                system_id: battery_id.clone(),
                reason:
                    "nominal energy capacity (kWh) is required to size the pack; no silent default permitted",
            }
        })?;
        let cfg = BatteryConfig {
            equipment_id: None,
            zone_id: None,
            capacity_kwh,
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
            initial_cell_temp_c: None,
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
            power_factor: None,
            inverter_capacity_kva: None,
            min_dwell_steps: 0,
        };
        let mut spec = build_typed_spec("Battery".to_string(), FuelType::Electric, cfg, defaults)?;
        spec.system_id = battery_id_opt;
        specs.push(spec);
    }
    Ok(())
}

pub(super) fn resolve_ev(
    details: &XmlNode,
    defaults: &DefaultsStore,
    specs: &mut Vec<EquipmentSpec>,
) -> std::result::Result<(), HpxmlError> {
    let Some(evs) = details.path(&["Systems", "ElectricVehicles"]) else {
        return Ok(());
    };
    for ev in evs.children_named("ElectricVehicle") {
        let ev_id_opt = element_id(ev);
        let ev_id = ev_id_opt.clone().unwrap_or_else(|| "unknown".to_string());
        let capacity_kwh = child_energy_kwh(ev, "BatteryCapacity").ok_or_else(|| {
            HpxmlError::MissingField {
                path: "ElectricVehicle/BatteryCapacity",
                system_kind: "EV",
                system_id: ev_id.clone(),
                reason:
                    "battery energy capacity (kWh) is required to simulate charging sessions; no silent default permitted",
            }
        })?;
        let max_charging_power_kw = child_f64(ev, "MaxChargingPower").ok_or_else(|| {
            HpxmlError::MissingField {
                path: "ElectricVehicle/MaxChargingPower",
                system_kind: "EV",
                system_id: ev_id.clone(),
                reason:
                    "maximum charging power (kW) is required to size the EVSE; no silent default permitted",
            }
        })?;
        let cfg = EvConfig {
            equipment_id: None,
            capacity_kwh,
            charging_level: child_text(ev, "ChargingLevel"),
            max_charging_power_kw,
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
            power_factor: None,
            charger_capacity_kva: None,
        };
        let mut spec = build_typed_spec("EV".to_string(), FuelType::Electric, cfg, defaults)?;
        spec.system_id = ev_id_opt;
        specs.push(spec);
    }
    Ok(())
}

pub(super) fn resolve_generators(
    details: &XmlNode,
    defaults: &DefaultsStore,
    specs: &mut Vec<EquipmentSpec>,
) -> std::result::Result<(), HpxmlError> {
    let Some(generators) = details.path(&["Systems", "extension", "Generators"]) else {
        return Ok(());
    };

    for generator in generators.children_named("Generator") {
        let gen_id_opt = element_id(generator);
        let generator_id = gen_id_opt.clone().unwrap_or_else(|| "unknown".to_string());
        let fuel = parse_fuel(child_text(generator, "FuelType").as_deref())?;
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

        let rated_power_kw = child_f64(generator, "ElectricalPowerOutput").ok_or_else(|| {
            HpxmlError::MissingField {
                path: "Generator/ElectricalPowerOutput",
                system_kind: "Generator",
                system_id: generator_id.clone(),
                reason:
                    "rated electrical output (kW) is required to size the generator; no silent default permitted",
            }
        })?;

        let efficiency_curve_points = defaults.generator_efficiency_curve_points();

        let cfg = GeneratorConfig {
            equipment_id: None,
            zone_id: None,
            fuel_type: Some(fuel),
            rated_power_kw,
            eta_electric,
            eta_thermal: None,
            eta_jacket_water: None,
            eta_lube_oil: None,
            eta_exhaust: None,
            efficiency_type: None,
            efficiency_curve_points,
            delta_kw_per_s: None,
            capacity_min_kw: None,
            grid_import_limit_kw: None,
            export_limit_kw: None,
            loop_id: None,
            flow_rate_kg_s: None,
            supply_temp_c: None,
            return_temp_c: None,
            inverter_efficiency: None,
            stack_temp_c: None,
            stack_cooler_r0: None,
            stack_cooler_r1: None,
            stack_cooler_r2: None,
            stack_cooler_r3: None,
            stack_nominal_temp_c: None,
            heat_rec_max_temp_c: None,
            no_load_fuel_fraction: None,
        };

        let mut spec = build_typed_spec("Gas Generator".to_string(), fuel, cfg, defaults)?;
        spec.system_id = gen_id_opt;
        specs.push(spec);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defaults::DefaultsStore;
    use crate::hpxml::parse_xml_document;
    use hares_equipment::BatteryConfig;

    #[test]
    fn battery_rated_power_output_watts_converts_to_kw() {
        let xml = r#"
            <HPXML>
              <Building>
                <BuildingDetails>
                  <Systems>
                    <Batteries>
                      <Battery>
                        <SystemIdentifier id="bat1"/>
                        <RatedPowerOutput>5000</RatedPowerOutput>
                        <NominalCapacity>
                          <Units>kWh</Units>
                          <Value>13.5</Value>
                        </NominalCapacity>
                      </Battery>
                    </Batteries>
                  </Systems>
                </BuildingDetails>
              </Building>
            </HPXML>
        "#;
        let root = parse_xml_document(xml).expect("XML must parse");
        let details = root
            .path(&["Building", "BuildingDetails"])
            .expect("details must exist");
        let defaults = DefaultsStore::empty();
        let mut specs = Vec::new();
        resolve_batteries(details, &defaults, &mut specs).expect("battery resolution must succeed");
        assert_eq!(specs.len(), 1, "expected one battery spec");
        let cfg: BatteryConfig = specs[0]
            .typed_config
            .as_ref()
            .expect("typed_config must be present")
            .typed()
            .expect("must deserialize to BatteryConfig");
        assert!(
            (cfg.max_charge_kw - 5.0).abs() < 1e-9,
            "RatedPowerOutput=5000 W must parse as max_charge_kw=5.0 kW, got {}",
            cfg.max_charge_kw
        );
    }
}
