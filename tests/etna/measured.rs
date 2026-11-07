//! The hash-pinned measured-data loader and the data-integrity contract.
//!
//! The committed `MeasurementData.json` is byte-exact archive content
//! (hash in `tests/fixtures/etna/README.md`), so the loader owns the one
//! normalization the archive's own JSON needs: the summary band columns
//! carry Python's non-standard `NaN` literal outside the best
//! steady-state windows, which standard JSON grammar forbids; the
//! loader replaces the token with `null` in memory before parsing. No
//! string value in the file contains the substring, so the token
//! replacement is unambiguous, and the integrity tests pin the per-group
//! null counts.

use std::path::PathBuf;
use std::sync::OnceLock;

use serde_json::Value;

use super::kernel::MeasuredWindow;

/// The committed fixtures' directory.
pub fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/etna")
}

/// The committed data files' sha256 pins (the archive's bytes, verified
/// on every test run).
pub mod hashes {
    pub const MEASUREMENT_DATA: &str =
        "a189ddb514d38ec9ce97dca8d22f9b261a5f621b0d5866e52e85482e7dfa42d0";
    pub const ET_CONFIG: &str = "eb02e96bd11e610972c8b0802163bcf83373edebd56a5243efa733ddc0523453";
    pub const EXAMPLE_SOFTWARE_QHTR: &str =
        "ae49e0fded88f392d66acfd0c2b64acae940dcc025625faef294f5ca1b0bd6bd";
    /// The unmodified upstream `ExampleSoftwareData.json` (48 MB, not
    /// committed); the extraction's provenance anchor.
    pub const UPSTREAM_EXAMPLE_SOFTWARE_DATA: &str =
        "6db35b7331d0719b3fbe5778e29bac1fc687c9560d8be917d9f66fba1f823a4f";
    /// The bundle's sha256.
    pub const SECTION13_ANALYSIS_FILES_ZIP: &str =
        "970927ec622edf237b715f3c9fd2649d0bd4a7a421c5720900b10addf38fa058";
    /// The addendum PDF's sha256.
    pub const ADDENDUM_PDF: &str =
        "7257eb68b09faa3c9350cb9bc85a8e6c5b5fd89985c7adc07a070b84aca267df";
}

/// The archive's JSON with the `NaN` literals normalized to `null`.
fn normalized_json_text(raw: &str) -> String {
    // The file's only strings are timestamps ("2000-01-26 12:00:00")
    // and column names (letters, no case), none of which contain the
    // substring, so replacing the bare token is unambiguous. Asserted
    // by the loader's own unit test below.
    raw.replace("NaN", "null")
}

/// The parsed archive shape: pandas' columns/index/data JSON.
pub struct Dataset {
    pub columns: Vec<String>,
    pub index: Vec<String>,
    /// Row-major values; `None` where the archive carries its `NaN`
    /// summary placeholders.
    pub data: Vec<Vec<Option<f64>>>,
}

impl Dataset {
    pub fn column_index(&self, name: &str) -> Result<usize, String> {
        self.columns
            .iter()
            .position(|c| c == name)
            .ok_or_else(|| format!("column {name} is absent from the dataset"))
    }

    /// The window slice between two index labels, inclusive.
    pub fn window(&self, start_label: &str, end_label: &str) -> Result<Vec<usize>, String> {
        let start = self
            .index
            .iter()
            .position(|t| t == start_label)
            .ok_or_else(|| {
                format!("window start {start_label} is absent from the dataset index")
            })?;
        let end = self
            .index
            .iter()
            .position(|t| t == end_label)
            .ok_or_else(|| format!("window end {end_label} is absent from the dataset index"))?;
        if end < start {
            return Err(format!(
                "window end {end_label} precedes start {start_label} in the dataset index"
            ));
        }
        Ok((start..=end).collect())
    }
}

fn parse_dataset(value: &Value, group: &str) -> Result<Dataset, String> {
    let obj = value
        .get(group)
        .ok_or_else(|| format!("dataset group {group} is absent from the archive JSON"))?;
    let columns: Vec<String> = obj
        .get("columns")
        .and_then(|v| v.as_array())
        .ok_or_else(|| format!("group {group}: missing columns array"))?
        .iter()
        .map(|v| {
            v.as_str()
                .map(|s| s.to_string())
                .ok_or_else(|| format!("group {group}: non-string column label"))
        })
        .collect::<Result<_, _>>()?;
    let index: Vec<String> = obj
        .get("index")
        .and_then(|v| v.as_array())
        .ok_or_else(|| format!("group {group}: missing index array"))?
        .iter()
        .map(|v| {
            v.as_str()
                .map(|s| s.to_string())
                .ok_or_else(|| format!("group {group}: non-string index label"))
        })
        .collect::<Result<_, _>>()?;
    let mut data = Vec::with_capacity(index.len());
    for (row_idx, row) in obj
        .get("data")
        .and_then(|v| v.as_array())
        .ok_or_else(|| format!("group {group}: missing data array"))?
        .iter()
        .enumerate()
    {
        let cells = row
            .as_array()
            .ok_or_else(|| format!("group {group} row {row_idx}: not an array"))?;
        let mut parsed = Vec::with_capacity(cells.len());
        for (col_idx, cell) in cells.iter().enumerate() {
            if cell.is_null() {
                parsed.push(None);
            } else {
                parsed.push(Some(cell.as_f64().ok_or_else(|| {
                    format!("group {group} row {row_idx} col {col_idx}: non-numeric value")
                })?));
            }
        }
        data.push(parsed);
    }
    Ok(Dataset {
        columns,
        index,
        data,
    })
}

/// The archive's four dataset groups, loaded once per test process.
pub struct MeasuredArchive {
    pub groups: std::collections::BTreeMap<String, Dataset>,
}

pub fn archive() -> &'static MeasuredArchive {
    static ARCHIVE: OnceLock<MeasuredArchive> = OnceLock::new();
    ARCHIVE.get_or_init(|| {
        let path = fixture_dir().join("MeasurementData.json");
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
        let normalized = normalized_json_text(&raw);
        let value: Value = serde_json::from_str(&normalized)
            .unwrap_or_else(|err| panic!("parse MeasurementData.json: {err}"));
        let mut groups = std::collections::BTreeMap::new();
        for group in ["ET110A", "ET110B", "ET100A", "ET100B"] {
            let ds = parse_dataset(&value, group).unwrap_or_else(|err| panic!("{err}"));
            groups.insert(group.to_string(), ds);
        }
        MeasuredArchive { groups }
    })
}

/// The extracted example-programs' Qhtr windows, loaded once per test
/// process.
pub struct ExampleSoftwareQhtr {
    pub cases: std::collections::BTreeMap<String, Value>,
}

pub fn example_software() -> &'static ExampleSoftwareQhtr {
    static EXAMPLES: OnceLock<ExampleSoftwareQhtr> = OnceLock::new();
    EXAMPLES.get_or_init(|| {
        let path = fixture_dir().join("ExampleSoftwareQhtr.json");
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
        let value: Value = serde_json::from_str(&raw)
            .unwrap_or_else(|err| panic!("parse ExampleSoftwareQhtr.json: {err}"));
        let cases = value
            .get("cases")
            .and_then(|v| v.as_object())
            .unwrap_or_else(|| panic!("ExampleSoftwareQhtr.json: missing cases object"))
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        ExampleSoftwareQhtr { cases }
    })
}

/// The case-contract file's parsed shape.
pub struct CaseContract {
    pub id: String,
    pub cell: String,
    pub window_insulated: bool,
    pub interior_surface_ht: String,
    pub base_case: String,
    pub specification_section: String,
    pub comparison_section: String,
    pub dataset_group: String,
    pub best_steady_state_start: String,
    pub best_steady_state_end: String,
    pub window_hours: usize,
    pub qhtr_uncertainty_rel: f64,
    pub qhtr_acccrit_cvrmse_max: f64,
    pub qhtr_measured_avg_wh: f64,
    pub qhtr_measured_lower_wh: f64,
    pub qhtr_measured_upper_wh: f64,
}

pub fn case_contracts() -> &'static Vec<CaseContract> {
    static CONTRACTS: OnceLock<Vec<CaseContract>> = OnceLock::new();
    CONTRACTS.get_or_init(|| {
        let path = fixture_dir().join("et_cases.toml");
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
        let value: toml::Value =
            toml::from_str(&raw).unwrap_or_else(|err| panic!("parse et_cases.toml: {err}"));
        let cases = value
            .get("case")
            .and_then(|v| v.as_array())
            .unwrap_or_else(|| panic!("et_cases.toml: missing [[case]] array"));
        cases
            .iter()
            .map(|c| CaseContract {
                id: c
                    .get("id")
                    .and_then(|v| v.as_str())
                    .expect("case id")
                    .to_string(),
                cell: c
                    .get("cell")
                    .and_then(|v| v.as_str())
                    .expect("case cell")
                    .to_string(),
                window_insulated: c
                    .get("window_insulated")
                    .and_then(|v| v.as_bool())
                    .expect("case window_insulated"),
                interior_surface_ht: c
                    .get("interior_surface_ht")
                    .and_then(|v| v.as_str())
                    .expect("case interior_surface_ht")
                    .to_string(),
                base_case: c
                    .get("base_case")
                    .and_then(|v| v.as_str())
                    .expect("case base_case")
                    .to_string(),
                specification_section: c
                    .get("specification_section")
                    .and_then(|v| v.as_str())
                    .expect("case specification_section")
                    .to_string(),
                comparison_section: c
                    .get("comparison_section")
                    .and_then(|v| v.as_str())
                    .expect("case comparison_section")
                    .to_string(),
                dataset_group: c
                    .get("dataset_group")
                    .and_then(|v| v.as_str())
                    .expect("case dataset_group")
                    .to_string(),
                best_steady_state_start: c
                    .get("best_steady_state_start")
                    .and_then(|v| v.as_str())
                    .expect("case best_steady_state_start")
                    .to_string(),
                best_steady_state_end: c
                    .get("best_steady_state_end")
                    .and_then(|v| v.as_str())
                    .expect("case best_steady_state_end")
                    .to_string(),
                window_hours: c
                    .get("window_hours")
                    .and_then(|v| v.as_integer())
                    .expect("case window_hours") as usize,
                qhtr_uncertainty_rel: c
                    .get("qhtr_uncertainty_rel")
                    .and_then(|v| v.as_float())
                    .expect("case qhtr_uncertainty_rel"),
                qhtr_acccrit_cvrmse_max: c
                    .get("qhtr_acccrit_cvrmse_max")
                    .and_then(|v| v.as_float())
                    .expect("case qhtr_acccrit_cvrmse_max"),
                qhtr_measured_avg_wh: c
                    .get("qhtr_measured_avg_wh")
                    .and_then(|v| v.as_float())
                    .expect("case qhtr_measured_avg_wh"),
                qhtr_measured_lower_wh: c
                    .get("qhtr_measured_lower_wh")
                    .and_then(|v| v.as_float())
                    .expect("case qhtr_measured_lower_wh"),
                qhtr_measured_upper_wh: c
                    .get("qhtr_measured_upper_wh")
                    .and_then(|v| v.as_float())
                    .expect("case qhtr_measured_upper_wh"),
            })
            .collect()
    })
}

/// The measured window for one case contract, sliced from the archive.
pub fn measured_window(contract: &CaseContract) -> Result<MeasuredWindow, String> {
    let ds = archive()
        .groups
        .get(&contract.dataset_group)
        .ok_or_else(|| {
            format!(
                "case {}: dataset group {} is absent",
                contract.id, contract.dataset_group
            )
        })?;
    let rows = ds.window(
        &contract.best_steady_state_start_gmt1(),
        &contract.best_steady_state_end_gmt1(),
    )?;
    let htr = ds.column_index("Qhtr")?;
    let mut hourly = Vec::with_capacity(rows.len());
    for &row in &rows {
        let value = ds.data[row][htr].ok_or_else(|| {
            format!(
                "case {}: the measured Qhtr is null at window hour {}",
                contract.id, ds.index[row]
            )
        })?;
        hourly.push(value);
    }
    let avg_col = if contract.cell == "A" {
        "QhtrA_avg"
    } else {
        "QhtrB_avg"
    };
    let avg_i = ds.column_index(avg_col)?;
    let lower_i = ds.column_index("Qhtr_lower_avg")?;
    let upper_i = ds.column_index("Qhtr_upper_avg")?;
    let first = rows[0];
    let avg = ds.data[first][avg_i].ok_or_else(|| {
        format!(
            "case {}: the {avg_col} summary is null inside the window",
            contract.id
        )
    })?;
    let lower = ds.data[first][lower_i].ok_or_else(|| {
        format!(
            "case {}: the lower bound is null inside the window",
            contract.id
        )
    })?;
    let upper = ds.data[first][upper_i].ok_or_else(|| {
        format!(
            "case {}: the upper bound is null inside the window",
            contract.id
        )
    })?;
    Ok(MeasuredWindow {
        case_id: contract.id.clone(),
        hourly_qhtr_wh: hourly,
        avg_wh: avg,
        lower_wh: lower,
        upper_wh: upper,
    })
}

impl CaseContract {
    /// The contract's window bounds in the archive index's label form
    /// ("YYYY-MM-DD HH:MM:SS", GMT+1).
    fn best_steady_state_start_gmt1(&self) -> String {
        self.best_steady_state_start
            .replace("T", " ")
            .replace("+01:00", "")
    }
    fn best_steady_state_end_gmt1(&self) -> String {
        self.best_steady_state_end
            .replace("T", " ")
            .replace("+01:00", "")
    }
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};

    use super::*;

    fn sha256_of(path: &std::path::Path) -> String {
        let bytes = std::fs::read(path).unwrap_or_else(|err| panic!("read {path:?}: {err}"));
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let out = hasher.finalize();
        out.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn the_nan_token_replacement_is_unambiguous_in_the_archive_content() {
        // No string value in the archive carries the "NaN" substring:
        // the committed bytes' only strings are timestamps and column
        // labels, so the loader's token replacement normalizes exactly
        // the summary placeholders and nothing else.
        let raw = std::fs::read_to_string(fixture_dir().join("MeasurementData.json"))
            .expect("the archive JSON reads");
        let occurrences = raw.matches("NaN").count();
        // ET110A 364x3, ET110B 364x3, ET100A 184x3, ET100B 185x3.
        assert_eq!(
            occurrences, 3291,
            "the archive's NaN token count changed; the loader's \
             normalization contract must be re-derived from the new bytes"
        );
        let value: Value = serde_json::from_str(&normalized_json_text(&raw))
            .expect("the normalized text parses as strict JSON");
        let groups = value.as_object().expect("the archive's top level");
        assert_eq!(
            groups.len(),
            4,
            "the archive carries exactly four dataset groups"
        );
    }

    #[test]
    fn the_committed_data_files_match_their_pinned_hashes() {
        for (name, pin) in [
            ("MeasurementData.json", hashes::MEASUREMENT_DATA),
            ("ET_config.yaml", hashes::ET_CONFIG),
            ("ExampleSoftwareQhtr.json", hashes::EXAMPLE_SOFTWARE_QHTR),
        ] {
            let actual = sha256_of(&fixture_dir().join(name));
            assert_eq!(
                actual, pin,
                "{name}'s bytes moved from the pinned archive content; \
                 re-derive the pin from the upstream bundle or fix the file"
            );
        }
    }

    #[test]
    fn the_archive_datasets_carry_their_full_measured_durations() {
        // The full hourly series (the case INPUT data per the addendum's
        // Table 13-2): ET110A/B 01/26/2000 12 through 02/11/2000 09;
        // ET100A/B 09/08/2000 16 through 09/18/2000 14 (15 for B).
        let expected: &[(&str, usize)] = &[
            ("ET110A", 382),
            ("ET110B", 382),
            ("ET100A", 239),
            ("ET100B", 240),
        ];
        for (group, rows) in expected {
            let ds = archive()
                .groups
                .get(*group)
                .unwrap_or_else(|| panic!("group {group} is loaded"));
            assert_eq!(
                ds.index.len(),
                *rows,
                "group {group}'s hourly row count is the archive's duration"
            );
        }
    }

    #[test]
    fn the_summary_band_columns_are_null_exactly_outside_the_windows() {
        // The archive's summary placeholders: null counts per group.
        let expected_nulls: &[(&str, usize)] = &[
            ("ET110A", 364 * 3),
            ("ET110B", 364 * 3),
            ("ET100A", 184 * 3),
            ("ET100B", 185 * 3),
        ];
        for (group, nulls) in expected_nulls {
            let ds = archive().groups.get(*group).unwrap();
            let actual = ds.data.iter().flatten().filter(|c| c.is_none()).count();
            assert_eq!(
                actual, *nulls,
                "group {group}'s null summary cells are exactly the \
                 out-of-window placeholders"
            );
        }
    }
}
