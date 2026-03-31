//! Distributed energy resource (PV, battery, EV, generator) resolution from HPXML.

use std::collections::HashMap;

use serde_json::{Map, Value, json};

use hares_types::FuelType;

use super::building::XmlNode;
use super::equipment::{EquipmentSpec, build_spec};
use super::xml_helpers::{child_energy_kwh, child_f64, child_text, element_id, parse_fuel};

use crate::defaults::DefaultsStore;

/// kBtu → kWh: 1 kBtu(IT) = 0.293_071_07 kWh exactly.
const KBTU_TO_KWH: f64 = 0.293_071_07;

pub(super) fn resolve_pv(
    details: &XmlNode,
    defaults: &DefaultsStore,
    specs: &mut Vec<EquipmentSpec>,
) {
    let Some(photovoltaics) = details.path(&["Systems", "Photovoltaics"]) else {
        return;
    };

    let mut inverter_eff_by_id: HashMap<String, f64> = HashMap::new();
    for inverter in photovoltaics.children_named("Inverter") {
        if let (Some(id), Some(eff)) = (
            element_id(inverter),
            child_f64(inverter, "InverterEfficiency"),
        ) {
            inverter_eff_by_id.insert(id, eff);
        }
    }

    for pv in photovoltaics.children_named("PVSystem") {
        let mut params = Map::new();
        if let Some(watts) = child_f64(pv, "MaxPowerOutput") {
            let kw = watts / 1000.0;
            params.insert("capacity_kw".to_string(), json!(kw));
        }
        if let Some(tilt) = child_f64(pv, "ArrayTilt") {
            params.insert("tilt_deg".to_string(), json!(tilt));
        }
        if let Some(az) = child_f64(pv, "ArrayAzimuth") {
            params.insert("azimuth_deg".to_string(), json!(az));
        }
        if let Some(module_type) = child_text(pv, "ModuleType") {
            params.insert("module_type".to_string(), Value::String(module_type));
        }
        if let Some(losses) = child_f64(pv, "SystemLossesFraction") {
            params.insert("system_losses_fraction".to_string(), json!(losses));
        }

        let inverter_eff = pv
            .child("AttachedToInverter")
            .and_then(|n| n.attrs.get("idref"))
            .and_then(|id| inverter_eff_by_id.get(id))
            .copied();
        if let Some(eff) = inverter_eff {
            params.insert("inverter_efficiency".to_string(), json!(eff));
        }

        specs.push(build_spec(
            "PV".to_string(),
            FuelType::Electric,
            params,
            defaults,
        ));
    }
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
        let mut params = Map::new();
        if let Some(kwh) = child_energy_kwh(battery, "NominalCapacity") {
            params.insert("capacity_kwh".to_string(), json!(kwh));
        }
        if let Some(kw) = child_f64(battery, "RatedPowerOutput") {
            params.insert("max_charge_kw".to_string(), json!(kw));
            params.insert("max_discharge_kw".to_string(), json!(kw));
        }
        if let Some(rte) = child_f64(battery, "RoundTripEfficiency") {
            // Store the raw round-trip efficiency under "inverter_efficiency".
            // Battery::init reads this key and applies sqrt() per direction,
            // so the final per-direction efficiency is sqrt(rte) each way.
            // Do NOT pre-apply sqrt() here — that would cause a double-sqrt,
            // making the effective RTE = rte^0.5 instead of rte (CW-016 F-1).
            params.insert("inverter_efficiency".to_string(), json!(rte));
        }
        specs.push(build_spec(
            "Battery".to_string(),
            FuelType::Electric,
            params,
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
            let mut params = Map::new();
            if let Some(level) = child_text(ev, "ChargingLevel") {
                params.insert("ChargingLevel".to_string(), Value::String(level));
            }
            if let Some(power_kw) = child_f64(ev, "MaxChargingPower") {
                params.insert("MaxChargingPower".to_string(), json!(power_kw));
            }
            if let Some(kwh) = child_energy_kwh(ev, "BatteryCapacity") {
                params.insert("BatteryCapacity".to_string(), json!(kwh));
            }
            specs.push(build_spec(
                "Electric Vehicle".to_string(),
                FuelType::Electric,
                params,
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
        let mut params = Map::new();

        if let Some(kw) = child_f64(generator, "ElectricalPowerOutput") {
            params.insert("rated_power_kw".to_string(), json!(kw));
        }

        // Derive electrical efficiency from annual energy figures when present.
        // eta = AnnualOutputkWh / (AnnualConsumptionkBtu * KBTU_TO_KWH)
        let annual_output_kwh = child_f64(generator, "AnnualOutputkWh");
        let annual_consumption_kbtu = child_f64(generator, "AnnualConsumptionkBtu");
        if let (Some(out_kwh), Some(cons_kbtu)) = (annual_output_kwh, annual_consumption_kbtu) {
            let cons_kwh = cons_kbtu * KBTU_TO_KWH;
            if cons_kwh > 0.0 {
                let eta = (out_kwh / cons_kwh).clamp(0.0, 1.0);
                params.insert("eta_electric".to_string(), json!(eta));
            }
        }

        specs.push(build_spec(
            "Gas Generator".to_string(),
            fuel,
            params,
            defaults,
        ));
    }
}
