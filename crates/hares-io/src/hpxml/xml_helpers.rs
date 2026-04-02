//! Shared XML helper functions for HPXML parsing.

use hares_physics::units as conv;
use hares_types::FuelType;
use hares_types::{normalize_ascii, parse_trimmed_f64};

use super::building::XmlNode;

pub(crate) fn parse_fuel(raw: Option<&str>) -> FuelType {
    let normalized = normalize_ascii(raw.unwrap_or("electricity"));
    match normalized.as_str() {
        "electricity" | "electric" | "none" => FuelType::Electric,
        "natural gas" | "natural_gas" | "gas" => FuelType::Gas,
        "propane" => FuelType::Propane,
        "oil" | "fuel oil" | "fuel_oil" => FuelType::Oil,
        other => {
            tracing::warn!(fuel = %other, "unrecognized fuel string; defaulting to Electric");
            FuelType::Electric
        }
    }
}

pub(crate) fn element_id(node: &XmlNode) -> Option<String> {
    node.child("SystemIdentifier")
        .and_then(|id_node| id_node.attrs.get("id"))
        .cloned()
}

pub(crate) fn child_text(node: &XmlNode, child_name: &str) -> Option<String> {
    node.child(child_name).map(|n| n.text.trim().to_string())
}

pub(crate) fn child_f64(node: &XmlNode, child_name: &str) -> Option<f64> {
    node.child(child_name)
        .and_then(|n| parse_trimmed_f64(&n.text))
}

pub(crate) fn child_temperature_c(node: &XmlNode) -> Option<f64> {
    let temp = node
        .child("HotWaterTemperature")
        .or_else(|| node.child("Temperature"))?;
    let value = parse_trimmed_f64(&temp.text)?;
    let units = temp
        .attrs
        .get("units")
        .map(String::as_str)
        .unwrap_or("F")
        .to_ascii_lowercase();
    if units == "f" || units == "degf" || units == "fahrenheit" {
        Some(conv::temperature_f_to_c(value))
    } else if units == "c" || units == "degc" || units == "celsius" {
        Some(value)
    } else {
        tracing::warn!(
            unit = %units,
            value,
            "unrecognized temperature unit in child_temperature_c; treating as Celsius"
        );
        Some(value)
    }
}

pub(crate) fn child_energy_kwh(node: &XmlNode, child_name: &str) -> Option<f64> {
    let target = node.child(child_name)?;
    let value = child_f64(target, "Value")?;
    let units = child_text(target, "Units")?.to_ascii_lowercase();
    match units.as_str() {
        "kwh" | "kwh/year" | "kwh/yr" => Some(value),
        "wh" | "wh/year" | "wh/yr" => Some(value / 1000.0),
        _ => None,
    }
}

pub(crate) fn child_load_kwh(node: &XmlNode) -> Option<f64> {
    let load = node.child("Load")?;
    let value = child_f64(load, "Value")?;
    let units = child_text(load, "Units")?.to_ascii_lowercase();
    match units.as_str() {
        "kwh/year" | "kwh/yr" | "kwh" => Some(value),
        _ => None,
    }
}

pub(crate) fn child_load_therms(node: &XmlNode) -> Option<f64> {
    let load = node.child("Load")?;
    let value = child_f64(load, "Value")?;
    let units = child_text(load, "Units")?.to_ascii_lowercase();
    match units.as_str() {
        "therm/year" | "therm/yr" | "therm" => Some(value),
        _ => None,
    }
}

pub(crate) fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut out = String::new();
    out.extend(first.to_uppercase());
    out.push_str(chars.as_str());
    out
}

pub(crate) fn descendants_named<'a>(node: &'a XmlNode, name: &'a str) -> Vec<&'a XmlNode> {
    let mut out = Vec::new();
    collect_descendants(node, name, &mut out);
    out
}

fn collect_descendants<'a>(node: &'a XmlNode, name: &str, out: &mut Vec<&'a XmlNode>) {
    if node.name == name {
        out.push(node);
    }
    for child in &node.children {
        collect_descendants(child, name, out);
    }
}

/// Locate the `HVACControl` node from a `BuildingDetails` subtree.
///
/// Search order: under `HVACPlant`, then `HVAC`, then any descendant.
pub(crate) fn find_hvac_control(details: &XmlNode) -> Option<&XmlNode> {
    descendants_named(details, "HVACPlant")
        .into_iter()
        .find_map(|p| p.child("HVACControl"))
        .or_else(|| {
            descendants_named(details, "HVAC")
                .into_iter()
                .find_map(|p| p.child("HVACControl"))
        })
        .or_else(|| details.first_descendant("HVACControl"))
}

/// Parse a single HVAC setpoint schedule (24h array in °C) from an `HVACControl` node.
///
/// Returns `None` if no setpoint data is found for the given `hvac_type`/`weekday` combination.
/// Used by both `building.rs::parse_hvac_setpoints` and `resolve_hvac.rs::parse_hvac_setpoint_params`.
pub(crate) fn parse_setpoint_from_control(
    control: &XmlNode,
    hvac_type: &str,
    weekday: bool,
) -> Option<Vec<f64>> {
    let day_prefix = if weekday { "Weekday" } else { "Weekend" };
    let ext_key = format!("{day_prefix}SetpointTemps{hvac_type}Season");

    if let Some(ext) = control.child("extension") {
        if let Some(node) = ext.child(&ext_key) {
            let vals: Vec<f64> = node
                .text
                .trim()
                .split(',')
                .filter_map(|s| s.trim().parse::<f64>().ok())
                .map(conv::temperature_f_to_c)
                .collect();
            if vals.len() == 24 {
                return Some(vals);
            }
        }
    }

    // Fallback: single constant value from <SetpointTemp{hvac_type}Season>
    let const_key = format!("SetpointTemp{hvac_type}Season");
    if let Some(node) = control.child(&const_key) {
        if let Ok(f_val) = node.text.trim().parse::<f64>() {
            let c_val = conv::temperature_f_to_c(f_val);
            return Some(vec![c_val; 24]);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn temp_node(tag: &str, value: &str, units: Option<&str>) -> XmlNode {
        let mut attrs = HashMap::new();
        if let Some(u) = units {
            attrs.insert("units".to_string(), u.to_string());
        }
        XmlNode {
            name: tag.to_string(),
            attrs: HashMap::new(),
            text: String::new(),
            children: vec![XmlNode {
                name: "Temperature".to_string(),
                attrs,
                text: value.to_string(),
                children: vec![],
            }],
        }
    }

    #[test]
    fn child_temperature_c_fahrenheit() {
        let node = temp_node("Parent", "212", Some("F"));
        let c = child_temperature_c(&node).unwrap();
        assert!((c - 100.0).abs() < 0.1);
    }

    #[test]
    fn child_temperature_c_celsius() {
        let node = temp_node("Parent", "25", Some("C"));
        let c = child_temperature_c(&node).unwrap();
        assert!((c - 25.0).abs() < 1e-12);
    }

    #[test]
    fn child_temperature_c_unrecognized_unit_returns_raw() {
        let node = temp_node("Parent", "300", Some("kelvin"));
        let c = child_temperature_c(&node).unwrap();
        assert!((c - 300.0).abs() < 1e-12);
    }

    #[test]
    fn child_temperature_c_no_units_assumes_fahrenheit() {
        let node = temp_node("Parent", "212", None);
        let c = child_temperature_c(&node).unwrap();
        assert!((c - 100.0).abs() < 0.1);
    }
}
