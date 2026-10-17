//! The `resolved` product: the resolved equipment parameters of a run's
//! homes, read through `DwellingBlueprint::validate` on the config each
//! home runs.
//!
//! Stored in full in the committed golden document, like the metrics:
//! per home, per equipment in model order, the equipment's name and each
//! resolved parameter's name, unit symbol and value as the 16-digit hex
//! of `f64::to_bits`. The writer walks the model's public fields; the
//! engine's `ResolvedModel` gains no serde derive. `compare` compares it
//! value by value and `delta` names each differing parameter.

use hares_core::resolved::ResolvedModel;
use serde::{Deserialize, Serialize};

/// One resolved parameter: the name the read-back reports, the value's
/// unit symbol, and the value's f64 bit pattern in hex.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedParameterDoc {
    pub name: String,
    pub unit: String,
    /// The value as `0x%016x` of `f64::to_bits`.
    pub bits: String,
}

/// One equipment of the model, in model order: its name and its resolved
/// parameters in report order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedEquipmentDoc {
    pub name: String,
    pub parameters: Vec<ResolvedParameterDoc>,
}

/// One home's resolved product: the equipment half of the read-back,
/// in model order.
pub type ResolvedHomeDoc = Vec<ResolvedEquipmentDoc>;

/// Reads the equipment parameters of one home's resolved model. The
/// envelope, site and clock halves are not covered: the product pins
/// equipment parameters only, so a row that touches none of them leaves
/// it bitwise.
#[must_use]
pub fn resolved_home_doc(model: &ResolvedModel) -> ResolvedHomeDoc {
    model
        .equipment
        .iter()
        .map(|equipment| ResolvedEquipmentDoc {
            name: equipment.name.clone(),
            parameters: equipment
                .parameters
                .iter()
                .map(|parameter| ResolvedParameterDoc {
                    name: parameter.name.to_string(),
                    unit: parameter.unit.symbol().to_string(),
                    bits: format!("0x{:016x}", parameter.value.to_bits()),
                })
                .collect(),
        })
        .collect()
}

/// The first differing resolved parameter between two homes' products:
/// `(equipment, parameter, expected, actual)` at the first mismatch, or
/// `None` when the two hold the same equipment and the same values on
/// bits. Extra or missing equipment, extra or missing parameters and
/// unit changes are mismatches too, reported at the parameter that
/// carries them.
#[must_use]
pub fn first_resolved_differences(
    expected: &ResolvedHomeDoc,
    actual: &ResolvedHomeDoc,
) -> Vec<(String, String, String, String)> {
    let mut differences = Vec::new();
    for (index, expected_equipment) in expected.iter().enumerate() {
        let label = |name: &str| format!("[{index}] {name}");
        let Some(actual_equipment) = actual.get(index) else {
            differences.push((
                label(&expected_equipment.name),
                "<absent>".to_string(),
                render_equipment(expected_equipment),
                "absent from the fresh run".to_string(),
            ));
            continue;
        };
        if expected_equipment.name != actual_equipment.name {
            differences.push((
                label(&expected_equipment.name),
                "<name>".to_string(),
                expected_equipment.name.clone(),
                actual_equipment.name.clone(),
            ));
        }
        for (parameter_index, expected_parameter) in
            expected_equipment.parameters.iter().enumerate()
        {
            let Some(actual_parameter) = actual_equipment.parameters.get(parameter_index) else {
                differences.push((
                    label(&expected_equipment.name),
                    expected_parameter.name.clone(),
                    render_parameter(expected_parameter),
                    "absent from the fresh run".to_string(),
                ));
                continue;
            };
            if expected_parameter != actual_parameter {
                differences.push((
                    label(&expected_equipment.name),
                    expected_parameter.name.clone(),
                    render_parameter(expected_parameter),
                    render_parameter(actual_parameter),
                ));
            }
        }
        for parameter_index in
            expected_equipment.parameters.len()..actual_equipment.parameters.len()
        {
            differences.push((
                label(&expected_equipment.name),
                actual_equipment.parameters[parameter_index].name.clone(),
                "absent from the committed golden".to_string(),
                render_parameter(&actual_equipment.parameters[parameter_index]),
            ));
        }
    }
    for index in expected.len()..actual.len() {
        differences.push((
            format!("[{index}] {}", actual[index].name),
            "<absent>".to_string(),
            "absent from the committed golden".to_string(),
            render_equipment(&actual[index]),
        ));
    }
    differences
}

fn render_equipment(equipment: &ResolvedEquipmentDoc) -> String {
    format!(
        "{} ({} parameter(s))",
        equipment.name,
        equipment.parameters.len()
    )
}

fn render_parameter(parameter: &ResolvedParameterDoc) -> String {
    format!("{} {} ({})", parameter.name, parameter.bits, parameter.unit)
}
