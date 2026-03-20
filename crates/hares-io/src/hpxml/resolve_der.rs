//! Distributed energy resource (PV, battery, EV) resolution from HPXML.

use std::collections::HashMap;

use serde_json::{Map, Value, json};

use hares_types::FuelType;

use super::building::XmlNode;
use super::equipment::{EquipmentSpec, build_spec};
use super::xml_helpers::{child_energy_kwh, child_f64, child_text, children_named, element_id};

use crate::defaults::DefaultsStore;

pub(super) fn resolve_pv(details: &XmlNode, defaults: &DefaultsStore, specs: &mut Vec<EquipmentSpec>) {
    let Some(photovoltaics) = details.path(&["Systems", "Photovoltaics"]) else {
        return;
    };

    let mut inverter_eff_by_id: HashMap<String, f64> = HashMap::new();
    for inverter in children_named(photovoltaics, "Inverter") {
        if let (Some(id), Some(eff)) = (
            element_id(inverter),
            child_f64(inverter, "InverterEfficiency"),
        ) {
            inverter_eff_by_id.insert(id, eff);
        }
    }

    for pv in children_named(photovoltaics, "PVSystem") {
        let mut params = Map::new();
        if let Some(kw) = child_f64(pv, "MaxPowerOutput") {
            params.insert("system_capacity_kw".to_string(), json!(kw));
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

pub(super) fn resolve_batteries(details: &XmlNode, defaults: &DefaultsStore, specs: &mut Vec<EquipmentSpec>) {
    let Some(batteries) = details.path(&["Systems", "Batteries"]) else {
        return;
    };

    for battery in children_named(batteries, "Battery") {
        let mut params = Map::new();
        if let Some(kwh) = child_energy_kwh(battery, "NominalCapacity") {
            params.insert("capacity_kwh".to_string(), json!(kwh));
        }
        if let Some(kw) = child_f64(battery, "RatedPowerOutput") {
            params.insert("max_charge_kw".to_string(), json!(kw));
            params.insert("max_discharge_kw".to_string(), json!(kw));
        }
        if let Some(rte) = child_f64(battery, "RoundTripEfficiency") {
            // Round-trip efficiency is the product of charge and discharge
            // efficiencies; each one-way efficiency is the square root.
            params.insert("inverter_efficiency".to_string(), json!(rte.sqrt()));
        }
        specs.push(build_spec(
            "Battery".to_string(),
            FuelType::Electric,
            params,
            defaults,
        ));
    }
}

pub(super) fn resolve_ev(details: &XmlNode, defaults: &DefaultsStore, specs: &mut Vec<EquipmentSpec>) {
    if let Some(evs) = details.path(&["Systems", "ElectricVehicles"]) {
        for ev in children_named(evs, "ElectricVehicle") {
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
