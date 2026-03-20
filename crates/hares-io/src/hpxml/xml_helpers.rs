//! Shared XML helper functions for HPXML parsing.

use hares_physics::units as conv;
use hares_types::FuelType;
use hares_types::{normalize_ascii, parse_trimmed_f64};

use super::building::XmlNode;

pub(crate) fn parse_fuel(raw: Option<&str>) -> FuelType {
    match normalize_ascii(raw.unwrap_or("electricity")).as_str()
    {
        "electricity" | "electric" | "none" => FuelType::Electric,
        "natural gas" | "natural_gas" | "gas" => FuelType::Gas,
        "propane" => FuelType::Propane,
        "oil" | "fuel oil" | "fuel_oil" => FuelType::Oil,
        _ => FuelType::Electric,
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
    Some(
        if units == "f" || units == "degf" || units == "fahrenheit" {
            conv::temperature_f_to_c(value)
        } else {
            value
        },
    )
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

pub(crate) fn children_named<'a>(node: &'a XmlNode, name: &'a str) -> impl Iterator<Item = &'a XmlNode> {
    node.children.iter().filter(move |child| child.name == name)
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
