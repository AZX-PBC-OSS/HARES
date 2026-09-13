//! Shared XML helper functions for HPXML parsing.

use std::collections::HashMap;

use serde_json::{Value, json};

use hares_physics::units as conv;
use hares_types::FuelType;
use hares_types::{normalize_ascii, parse_trimmed_f64};

use super::building::XmlNode;

pub(crate) fn parse_fuel(raw: Option<&str>) -> Result<FuelType, super::HpxmlError> {
    let raw_str =
        raw.ok_or_else(|| super::HpxmlError::Parse("FuelType is required but was missing".into()))?;
    let normalized = normalize_ascii(raw_str);
    match normalized.as_str() {
        "electricity" | "electric" => Ok(FuelType::Electric),
        "natural gas" | "natural_gas" | "gas" => Ok(FuelType::Gas),
        "propane" => Ok(FuelType::Propane),
        "oil" | "fuel oil" | "fuel_oil" | "fuel oil 1" | "fuel oil 2" | "fuel oil 4"
        | "fuel oil 5/6" | "kerosene" | "diesel" => Ok(FuelType::Oil),
        "wood" => Ok(FuelType::Wood),
        "wood pellets" | "wood_pellets" => Ok(FuelType::WoodPellet),
        "coal" | "anthracite coal" | "anthracite_coal" | "bituminous coal" | "bituminous_coal"
        | "coke" => Ok(FuelType::Coal),
        "none" => Ok(FuelType::None),
        other => Err(super::HpxmlError::Parse(
            format!("unsupported FuelType '{other}'").into(),
        )),
    }
}

pub(crate) fn element_id(node: &XmlNode) -> Option<String> {
    node.child("SystemIdentifier")
        .and_then(|id_node| id_node.attrs.get("id"))
        .cloned()
}

/// Resolve a LocalReference child element's `@idref` attribute against a lookup table.
///
/// HPXML LocalReference elements (e.g. `<AttachedToPool idref="Pool1"/>`,
/// `<RelatedHVACSystem idref="boiler1"/>`) reference another element by its
/// `SystemIdentifier/@id`. This helper extracts the `idref` from `node`'s child
/// named `child_name` and looks it up in `lookup`.
///
/// Returns `None` if the child is absent, has no `idref` attribute, or the
/// `idref` is not present in the lookup.
pub(crate) fn resolve_local_ref<'a, T>(
    node: &XmlNode,
    child_name: &str,
    lookup: &'a HashMap<String, T>,
) -> Option<&'a T> {
    let idref = node
        .child(child_name)
        .and_then(|a| a.attrs.get("idref"))?
        .as_str();
    lookup.get(idref)
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
            "unrecognized temperature unit in child_temperature_c; HPXML spec units \
             are F, C, degF, degC, Fahrenheit, Celsius — ignoring value"
        );
        None
    }
}

pub(crate) fn child_energy_kwh(node: &XmlNode, child_name: &str) -> Option<f64> {
    let target = node.child(child_name)?;
    let value = child_f64(target, "Value")?;
    let units = child_text(target, "Units")?.to_ascii_lowercase();
    match units.as_str() {
        "kwh" | "kwh/year" | "kwh/yr" => Some(value),
        "wh" | "wh/year" | "wh/yr" => Some(value / 1000.0),
        "therm" | "therm/year" | "therm/yr" => Some(conv::energy_therms_to_kwh(value)),
        other => {
            tracing::warn!(
                units = other,
                value,
                field = child_name,
                "Unrecognized energy unit; cannot convert child energy to kWh"
            );
            None
        }
    }
}

pub(crate) fn child_load_kwh(node: &XmlNode) -> Option<f64> {
    let load = node.child("Load")?;
    let value = child_f64(load, "Value")?;
    let units_raw = child_text(load, "Units");
    let units = units_raw
        .as_deref()
        .unwrap_or("kwh/year")
        .to_ascii_lowercase();
    match units.as_str() {
        "kwh/year" | "kwh/yr" | "kwh" => Some(value),
        "w" => Some(conv::power_watt_to_kwh_per_year(value)),
        other => {
            tracing::warn!(
                units = other,
                value,
                "Unrecognized load energy unit from PlugLoadUnits/PoolHeaterUnits; \
                 cannot convert to kWh/year. Accepted: kWh/year, W."
            );
            None
        }
    }
}

pub(crate) fn child_load_therms(node: &XmlNode) -> Option<f64> {
    let load = node.child("Load")?;
    let value = child_f64(load, "Value")?;
    let units_raw = child_text(load, "Units");
    let units = units_raw
        .as_deref()
        .unwrap_or("therm/year")
        .to_ascii_lowercase();
    match units.as_str() {
        "therm/year" | "therm/yr" | "therm" => Some(value),
        "btuh" => Some(conv::power_btuh_to_therms_per_year(value)),
        other => {
            tracing::warn!(
                units = other,
                value,
                "Unrecognized load energy unit from PoolHeaterUnits; \
                 cannot convert to therms/year. Accepted: therm/year, Btuh."
            );
            None
        }
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
    // ResStock-style: Systems > HVAC > HVACControl
    details
        .path(&["Systems", "HVAC", "HVACControl"])
        .or_else(|| {
            // NRELCAMP-style: HVACPlant > HVACControl
            descendants_named(details, "HVACPlant")
                .into_iter()
                .find_map(|p| p.child("HVACControl"))
        })
        .or_else(|| {
            // Any HVAC > HVACControl pairing
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
                .filter_map(parse_trimmed_f64)
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
        if let Some(f_val) = parse_trimmed_f64(&node.text) {
            let c_val = conv::temperature_f_to_c(f_val);
            return Some(vec![c_val; 24]);
        }
    }

    None
}

/// Parse schedule extension parameters from an `<extension>` child.
///
/// Extracts `WeekdayScheduleFractions`, `WeekendScheduleFractions`,
/// `MonthlyScheduleMultipliers`, `UsageMultiplier`, `FracSensible`,
/// `FracLatent`, and `FracRadiant` from `<extension>` children.
/// The `prefix` parameter disambiguates keys when a node has multiple
/// extension sections (e.g. `"LightingWeekdayScheduleFractions"`).
pub(crate) fn parse_schedule_extension_params(
    node: &XmlNode,
    prefix: &str,
) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    let Some(ext) = node.child("extension") else {
        return out;
    };

    let weekday_key = if prefix.is_empty() {
        "WeekdayScheduleFractions".to_string()
    } else {
        format!("{prefix}WeekdayScheduleFractions")
    };
    let weekend_key = if prefix.is_empty() {
        "WeekendScheduleFractions".to_string()
    } else {
        format!("{prefix}WeekendScheduleFractions")
    };
    let multiplier_key = if prefix.is_empty() {
        "UsageMultiplier".to_string()
    } else {
        format!("{prefix}UsageMultiplier")
    };

    if let Some(frac_node) = ext.child(&weekday_key) {
        let vals: Vec<f64> = frac_node
            .text
            .trim()
            .split(',')
            .filter_map(parse_trimmed_f64)
            .collect();
        if vals.len() == 24 {
            out.push(("weekday_schedule_fractions".to_string(), json!(vals)));
        } else if !vals.is_empty() {
            tracing::warn!(
                key = %weekday_key,
                count = vals.len(),
                "WeekdayScheduleFractions has unexpected number of values (expected 24); ignoring"
            );
        }
    }
    if let Some(frac_node) = ext.child(&weekend_key) {
        let vals: Vec<f64> = frac_node
            .text
            .trim()
            .split(',')
            .filter_map(parse_trimmed_f64)
            .collect();
        if vals.len() == 24 {
            out.push(("weekend_schedule_fractions".to_string(), json!(vals)));
        } else if !vals.is_empty() {
            tracing::warn!(
                key = %weekend_key,
                count = vals.len(),
                "WeekendScheduleFractions has unexpected number of values (expected 24); ignoring"
            );
        }
    }
    if let Some(mult) = child_f64(ext, &multiplier_key) {
        out.push(("usage_multiplier".to_string(), json!(mult)));
    }

    let month_key = if prefix.is_empty() {
        "MonthlyScheduleMultipliers".to_string()
    } else {
        format!("{prefix}MonthlyScheduleMultipliers")
    };
    if let Some(node) = ext.child(&month_key) {
        let vals: Vec<f64> = node
            .text
            .trim()
            .split(',')
            .filter_map(parse_trimmed_f64)
            .collect();
        if vals.len() == 12 {
            out.push(("month_multipliers".to_string(), json!(vals)));
        } else if !vals.is_empty() {
            tracing::warn!(
                key = %month_key,
                count = vals.len(),
                "MonthlyScheduleMultipliers has unexpected number of values (expected 12); ignoring"
            );
        }
    }

    if let Some(frac) = child_f64(ext, "FracSensible") {
        out.push(("frac_sensible".to_string(), json!(frac)));
    }
    if let Some(frac) = child_f64(ext, "FracLatent") {
        out.push(("frac_latent".to_string(), json!(frac)));
    }
    if let Some(frac) = child_f64(ext, "FracRadiant") {
        out.push(("radiative_gain_fraction".to_string(), json!(frac)));
    }

    // Passthrough keys for event-based load equipment configuration.
    // EventBasedLoad::init() reads these keys directly from the config;
    // synthetic TOML fixtures set them via `<extension>` children so
    // they flow through the HPXML resolver into the EquipmentConfig.
    for (ext_key, config_key) in &[
        ("active_power_kw", "active_power_kw"),
        ("active_duration_s", "active_duration_s"),
        ("cooldown_duration_s", "cooldown_duration_s"),
        ("event_window_source", "event_window_source"),
        ("event_probability_source", "event_probability_source"),
        ("event_probability_constant", "event_probability_constant"),
        ("event_window_schedule_col", "event_window_schedule_col"),
        (
            "event_probability_schedule_col",
            "event_probability_schedule_col",
        ),
        ("sensible_gain_fraction", "sensible_gain_fraction"),
        ("latent_gain_fraction", "latent_gain_fraction"),
    ] {
        if let Some(val) = child_f64(ext, ext_key) {
            out.push((config_key.to_string(), json!(val)));
        } else if let Some(val) = child_text(ext, ext_key) {
            out.push((config_key.to_string(), json!(val)));
        }
    }

    out
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
    fn child_temperature_c_unrecognized_unit_returns_none() {
        let node = temp_node("Parent", "300", Some("kelvin"));
        assert!(child_temperature_c(&node).is_none());
    }

    #[test]
    fn child_temperature_c_no_units_assumes_fahrenheit() {
        let node = temp_node("Parent", "212", None);
        let c = child_temperature_c(&node).unwrap();
        assert!((c - 100.0).abs() < 0.1);
    }

    // --- child_load_kwh / child_load_therms tests ---

    fn load_node(value: f64, units: Option<&str>) -> XmlNode {
        let mut children = vec![XmlNode {
            name: "Value".to_string(),
            attrs: HashMap::new(),
            text: value.to_string(),
            children: vec![],
        }];
        if let Some(u) = units {
            children.push(XmlNode {
                name: "Units".to_string(),
                attrs: HashMap::new(),
                text: u.to_string(),
                children: vec![],
            });
        }
        XmlNode {
            name: "Parent".to_string(),
            attrs: HashMap::new(),
            text: String::new(),
            children: vec![XmlNode {
                name: "Load".to_string(),
                attrs: HashMap::new(),
                text: String::new(),
                children,
            }],
        }
    }

    #[test]
    fn child_load_kwh_parses_kwh_year() {
        let node = load_node(2700.0, Some("kWh/year"));
        assert!((child_load_kwh(&node).unwrap() - 2700.0).abs() < 1e-10);
    }

    #[test]
    fn child_load_kwh_converts_watts() {
        // 1000 W * 8760 / 1000 = 8760 kWh/year
        let node = load_node(1000.0, Some("W"));
        assert!((child_load_kwh(&node).unwrap() - 8760.0).abs() < 1e-10);
    }

    #[test]
    fn child_load_kwh_defaults_to_kwh_year_when_units_absent() {
        let node = load_node(500.0, None);
        assert!((child_load_kwh(&node).unwrap() - 500.0).abs() < 1e-10);
    }

    #[test]
    fn child_load_kwh_rejects_unrecognized_unit() {
        let node = load_node(100.0, Some("gigajoules"));
        assert!(child_load_kwh(&node).is_none());
    }

    #[test]
    fn child_load_therms_parses_therm_year() {
        let node = load_node(500.0, Some("therm/year"));
        assert!((child_load_therms(&node).unwrap() - 500.0).abs() < 1e-10);
    }

    #[test]
    fn child_load_therms_converts_btuh() {
        // 100000 Btuh * 8760 / 100000 = 8760 therms/year
        let node = load_node(100_000.0, Some("Btuh"));
        assert!((child_load_therms(&node).unwrap() - 8760.0).abs() < 1e-10);
    }

    #[test]
    fn child_load_therms_rejects_unrecognized_unit() {
        let node = load_node(100.0, Some("MW"));
        assert!(child_load_therms(&node).is_none());
    }

    #[test]
    fn child_load_kwh_rejects_therm_units() {
        // cross-unit conversion intentionally NOT added here:
        // callers use child_load_kwh→Electric, child_load_therms→Gas for fuel type detection.
        let node = load_node(100.0, Some("therm/year"));
        assert!(child_load_kwh(&node).is_none());
    }

    #[test]
    fn child_load_therms_rejects_kwh_units() {
        // cross-unit conversion intentionally NOT added here:
        // callers use child_load_kwh→Electric, child_load_therms→Gas for fuel type detection.
        let node = load_node(2930.0, Some("kWh"));
        assert!(child_load_therms(&node).is_none());
    }

    // --- child_energy_kwh tests ---

    fn energy_node(child_name: &str, value: f64, units: Option<&str>) -> XmlNode {
        let mut children = vec![XmlNode {
            name: "Value".to_string(),
            attrs: HashMap::new(),
            text: value.to_string(),
            children: vec![],
        }];
        if let Some(u) = units {
            children.push(XmlNode {
                name: "Units".to_string(),
                attrs: HashMap::new(),
                text: u.to_string(),
                children: vec![],
            });
        }
        XmlNode {
            name: "Parent".to_string(),
            attrs: HashMap::new(),
            text: String::new(),
            children: vec![XmlNode {
                name: child_name.to_string(),
                attrs: HashMap::new(),
                text: String::new(),
                children,
            }],
        }
    }

    #[test]
    fn child_energy_kwh_parses_kwh() {
        let node = energy_node("Usage", 100.0, Some("kWh"));
        assert!((child_energy_kwh(&node, "Usage").unwrap() - 100.0).abs() < 1e-10);
    }

    #[test]
    fn child_energy_kwh_parses_wh() {
        let node = energy_node("Usage", 5000.0, Some("Wh"));
        assert!((child_energy_kwh(&node, "Usage").unwrap() - 5.0).abs() < 1e-10);
    }

    #[test]
    fn child_energy_kwh_converts_therms() {
        let node = energy_node("Usage", 10.0, Some("therm"));
        let expected = conv::energy_therms_to_kwh(10.0);
        assert!((child_energy_kwh(&node, "Usage").unwrap() - expected).abs() < 1e-10);
    }

    #[test]
    fn child_energy_kwh_converts_therm_per_year() {
        let node = energy_node("Usage", 10.0, Some("therm/year"));
        let expected = conv::energy_therms_to_kwh(10.0);
        assert!((child_energy_kwh(&node, "Usage").unwrap() - expected).abs() < 1e-10);
    }

    #[test]
    fn child_energy_kwh_returns_none_for_unrecognized_unit() {
        let node = energy_node("Usage", 100.0, Some("gigajoules"));
        assert!(child_energy_kwh(&node, "Usage").is_none());
    }

    #[test]
    fn month_multipliers_emitted_only_once() {
        let xml = r#"<node>
          <extension>
            <MonthlyScheduleMultipliers>1.0,1.0,1.0,1.0,1.0,1.0,1.0,1.0,1.0,1.0,1.0,1.0</MonthlyScheduleMultipliers>
            <WeekdayScheduleFractions>0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1,0.1</WeekdayScheduleFractions>
          </extension>
        </node>"#;
        let root = crate::hpxml::building::parse_xml_document(xml).expect("parse test XML");

        let result = parse_schedule_extension_params(&root, "");
        let month_count = result
            .iter()
            .filter(|(k, _)| k == "month_multipliers")
            .count();
        assert_eq!(
            month_count, 1,
            "month_multipliers should appear exactly once in output, found {month_count}"
        );
    }

    // ── parse_fuel ──────────────────────────────────────────────────────

    #[test]
    fn parse_fuel_none_returns_err() {
        assert!(parse_fuel(None).is_err());
    }

    #[test]
    fn parse_fuel_valid_strings() {
        assert_eq!(parse_fuel(Some("electricity")).unwrap(), FuelType::Electric);
        assert_eq!(parse_fuel(Some("natural gas")).unwrap(), FuelType::Gas);
        assert_eq!(parse_fuel(Some("propane")).unwrap(), FuelType::Propane);
        assert_eq!(parse_fuel(Some("fuel oil")).unwrap(), FuelType::Oil);
        assert_eq!(parse_fuel(Some("wood")).unwrap(), FuelType::Wood);
        assert_eq!(
            parse_fuel(Some("wood pellets")).unwrap(),
            FuelType::WoodPellet
        );
        assert_eq!(parse_fuel(Some("coal")).unwrap(), FuelType::Coal);
    }

    #[test]
    fn parse_fuel_unrecognized_returns_err() {
        let result = parse_fuel(Some("district heating"));
        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(err.contains("unsupported FuelType"), "got: {err}");
    }

    #[test]
    fn parse_fuel_solar_returns_err() {
        let result = parse_fuel(Some("solar"));
        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(err.contains("unsupported FuelType"), "got: {err}");
    }

    #[test]
    fn parse_fuel_misspelled_returns_err() {
        let result = parse_fuel(Some("natuarl gas"));
        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(err.contains("unsupported FuelType"), "got: {err}");
    }

    #[test]
    fn parse_fuel_hydrogen_returns_err() {
        let result = parse_fuel(Some("hydrogen"));
        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(err.contains("unsupported FuelType"), "got: {err}");
    }

    #[test]
    fn parse_fuel_none_maps_to_none() {
        let result = parse_fuel(Some("none"));
        assert!(
            result.is_ok(),
            "\"none\" is a valid HPXML fuel type and must map to FuelType::None"
        );
        assert_eq!(result.unwrap(), FuelType::None);
    }
}
