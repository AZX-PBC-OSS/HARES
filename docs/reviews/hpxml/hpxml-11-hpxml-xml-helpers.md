# HPXML XML helper utilities: correctness and edge cases
**Review ID**: hpxml-11
**Category**: hpxml
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-io/src/hpxml/xml_helpers.rs`
- `crates/hares-io/src/hpxml/mod.rs`

Additional source (contains `normalize_name()` and `parse_value_with_units()` referenced in scope):
- `crates/hares-io/src/hpxml/building.rs`

Supporting:
- `crates/hares-physics/src/units.rs` (temperature/area/volume/length conversions)
- `crates/hares-physics/src/constants.rs` (FAHRENHEIT_SCALE / FAHRENHEIT_OFFSET)
- `crates/hares-types/src/text.rs` (`parse_trimmed_f64`, `normalize_ascii`)

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/utils/base.py`
- `vendors/OCHRE/ochre/utils/hpxml.py`
- `vendors/OCHRE/ochre/utils/units.py`

## Findings

### Finding 1: [Severity: high]
**Description**: Quick-xml errors from `parse_xml_document` are propagated without line-number or element-name context, making debugging of malformed HPXML files unnecessarily difficult.
**Code Location**: `crates/hares-io/src/hpxml/building.rs:404-406`
**Root Cause**: The error handler formats the raw `quick_xml::Error`:
```rust
Err(err) => {
    return Err(HpxmlError::Parse(format!("XML parse failure: {err}")));
}
```
The `quick_xml::Error` `Display` implementation returns a human-readable description of the error type (e.g. `"illegal character"`, `"unexpected end of file"`) but omits byte-position and line-number information. The `quick_xml` `Error` type carries a position via `reader.buffer_position()` at the time of the call, or through methods like `Error::position()`, but this information is discarded.
**Impact**: When a user provides a malformed HPXML file, the error message they receive is generic (e.g. `"HPXML parse error: XML parse failure: illegal character"`) with no indication of where the problem occurred. For large HPXML files (which can be tens of thousands of lines), this makes root-causing issues extremely time-consuming. The OCHRE reference uses `xmltodict.parse()` which raises Python exceptions with `ExpatError` that includes line/column numbers.

### Finding 2: [Severity: medium]
**Description**: Duplicate temperature-unit-matching logic exists between `child_temperature_c` in `xml_helpers.rs` and `convert_temperature_to_c` in `building.rs`, creating a maintenance risk for divergent behavior.
**Code Location**:
- `crates/hares-io/src/hpxml/xml_helpers.rs:43-66` (`child_temperature_c`)
- `crates/hares-io/src/hpxml/building.rs:2150-2172` (`convert_temperature_to_c`)
**Root Cause**: Both functions independently match unit strings (`"f"`, `"degf"`, `"fahrenheit"`, `"c"`, `"degc"`, `"celsius"`) and apply `conv::temperature_f_to_c()`. The only structural difference is that `child_temperature_c` searches for `HotWaterTemperature` / `Temperature` child elements before resolving units, while `convert_temperature_to_c` operates on a pre-resolved `XmlNode`. The two functions are maintained in different modules (one `pub(crate)`, the other `fn` in `building.rs`) and could drift.
**Impact**: If a new temperature unit variant is added to one function but not the other, the behavior will diverge silently. Currently both ignore Kelvin (`"K"`, `"kelvin"`) and Rankine (`"R"`) with a warning, but future updates risk inconsistency.

### Finding 3: [Severity: medium]
**Description**: Duct leakage measurements for the same duct type silently overwrite each other with no warning or deduplication.
**Code Location**: `crates/hares-io/src/hpxml/building.rs:1852`
```rust
leakage_by_type.insert(dtype, f);
```
**Root Cause**: Multiple `DuctLeakageMeasurement` elements with the same `DuctType` but different values (e.g., leakage tested at two different pressures, or two separate duct branches) will overwrite each other by insertion order. The silently-lost value could represent a meaningful measurement. The OCHRE reference (`vendors/OCHRE/ochre/utils/hpxml.py:989-1003`) asserts exactly 2 duct-leakage measurements (one supply, one return), which surfaces mismatches through an assertion error rather than silent data loss.
**Impact**: If an HPXML file contains duplicated duct-type leakage measurements (e.g., supply leakage reported under both CFM25 and Percent units), only the last-seen value survives. The ASHRAE 152 DSE calculation uses whatever value remains without any indication that a prior value was discarded.

### Finding 4: [Severity: medium]
**Description**: `parse_setpoint_from_control` always applies Fahrenheit-to-Celsius conversion without inspecting a `units` attribute, relying entirely on the HPXML/ResStock convention that extension setpoint temperatures are in Fahrenheit.
**Code Location**: `crates/hares-io/src/hpxml/xml_helpers.rs:166,178`
```rust
.map(conv::temperature_f_to_c)    // line 166
let c_val = conv::temperature_f_to_c(f_val);  // line 178
```
**Root Cause**: The `extension` namespace and the setpoint tags do not define a `units` attribute in the HPXML specification, so the convention that values are in Fahrenheit is defensible. However, the code has no defensive check. If a toolchain emits Celsius values (e.g., through a conversion step), the data would be silently double-converted. The OCHRE reference does the same conversion using `convert(weekday_setpoints, "degF", "degC")` (line 927 of `hpxml.py`), so this is an industry convention issue, not a HARES-specific bug.
**Impact**: If non-standard HPXML files with Celsius setpoints are fed to the parser, setpoint temperatures will be ~32°F-too-low (Celsius values converted again using °F offset), leading to incorrect HVAC simulation results with no warning.

### Finding 5: [Severity: low]
**Description**: `child_temperature_c` silently treats unrecognized temperature units (including Kelvin and Rankine) as Celsius.
**Code Location**: `crates/hares-io/src/hpxml/xml_helpers.rs:58-65`
```rust
} else {
    tracing::warn!(
        unit = %units,
        value,
        "unrecognized temperature unit in child_temperature_c; treating as Celsius"
    );
    Some(value)
}
```
**Root Cause**: The match handles only Fahrenheit-variant and Celsius-variant strings. All other units (including `"K"`, `"kelvin"`, `"R"`, `"rankine"`) fall through to a warning and are returned raw. A raw Kelvin value (e.g., 298.15) treated as Celsius results in a physically impossible temperature.
**Impact**: While Kelvin and Rankine are rare in HPXML files (the spec uses degF and degC), the silent treatment-as-Celsius could mask schema violations. Improbable in practice given HPXML validation, but the existing test at line 225-229 accepts 300 K as 300°C without distinguishing between the two.

### Finding 6: [Severity: low]
**Description**: Duct-type matching in `parse_duct_systems` silently uses an empty-string key when `DuctType` is missing.
**Code Location**: `crates/hares-io/src/hpxml/building.rs:1827-1830`
```rust
let dtype = meas
    .first_descendant("DuctType")
    .map(|n| normalize_ascii(&n.text))
    .unwrap_or_default();  // → ""
```
**Root Cause**: When a `DuctLeakageMeasurement` lacks a `DuctType` child element, the duct type key defaults to the empty string. The leakage fraction is stored under `""` and will never match any duct's `duct_type_text` (which at minimum would be `"supply"`, `"return"`, or `"unknown"`).
**Impact**: The leakage value is silently discarded. This is a graceful degradation, but should at minimum emit a warning.

### Finding 7: [Severity: low]
**Description**: Temperature conversion constants `5.0 / 9.0` and `9.0 / 5.0` are standard IEEE 754 double-precision approximations; the HARES `temperature_f_to_c` function correctly delegates to the `uom` crate rather than manual arithmetic.
**Code Location**:
- `crates/hares-physics/src/constants.rs:230` (declares `FAHRENHEIT_SCALE = 5.0 / 9.0`, used only for documentation and delta conversions)
- `crates/hares-physics/src/units.rs:187-189` (`temperature_f_to_c` delegates to `uom::ThermodynamicTemperature`)
**Root Cause**: The constant `5.0 / 9.0` cannot be represented exactly in IEEE 754 binary floating point (it is a repeating fraction). The resulting `0.5555555555555556` is accurate to ~2.22 × 10⁻¹⁶. The `uom` crate uses the same floating-point approximation for `ThermodynamicTemperature` conversion, as does the Python `pint` library used by OCHRE. This is an inherent limitation of floating-point arithmetic, not a bug.
**Impact**: Within the domain of building energy simulation (where temperatures are meaningful to ~0.01 K), the ~10⁻¹⁶ error is negligible. The conversion involves both the scale factor and the 32.0 offset, and the `uom` crate handles both correctly. Round-trip tests in `units.rs:347-354` confirm accuracy to 1e-10.

### Finding 8: [Severity: info]
**Description**: `child_energy_kwh` supports only kWh and Wh units; `child_load_therms` supports only therms — missing `MBtu`, `GJ`, and `kBtu` energy-unit variants.
**Code Location**: `crates/hares-io/src/hpxml/xml_helpers.rs:68-97`
**Root Cause**: The accepted-unit sets are narrow: `kWh`, `Wh`, and `therm` variants only. The HPXML specification permits `MBtu/year` and `kBtu/year` for energy loads.
**Impact**: Energy loads expressed in MBtu or kBtu will return `None` from `child_energy_kwh` / `child_load_kwh` / `child_load_therms`, causing the value to be discarded. The building parser never sees the value. In practice, most HPXML files in the ResStock ecosystem use kWh for electric loads and therms for gas loads, so this is unlikely to affect current datasets.

## Summary
- Total findings: 8
- Critical: 0
- High: 1
- Medium: 3
- Low: 3
- Info: 1

## Recommendations
1. **Enhance XML parse error messages** to include line-number or byte-position context. Since `quick_xml::Reader` tracks buffer position, capture `reader.buffer_position()` at parse time (or use quick_xml 0.36+ `Error::position()` if available) and include it in the `HpxmlError::Parse` variant. Also record the most recent element name from the stack for additional context.
2. **Consolidate temperature conversion logic** into a shared function. Have `child_temperature_c` in `xml_helpers.rs` delegate to `convert_temperature_to_c` in `building.rs` (or extract a common `parse_temp_with_units(node: &XmlNode) -> Option<f64>` helper) to eliminate the maintenance risk of diverging unit-string lists.
3. **Warn on duct-leakage overwrites** when a `DuctType` value already exists in `leakage_by_type`. Emit a `tracing::warn!` when `HashMap::insert` replaces a previous entry for the same type.
4. **Warn when `DuctType` is absent** from a `DuctLeakageMeasurement` (empty-string key case in Finding 6) so data loss is visible in logs.
5. **Consider adding Kelvin/Rankine handling** to temperature conversion, or at minimum upgrade the tracing level from `warn` to `error` when an unrecognized unit is encountered, since silent temperature assignment could produce physically nonsensical results.

## References / Citations
- `quick_xml` crate documentation on error positions: `Error::position()` returns `TextPosition` when available
- HPXML v4.0 schema: `<DuctLeakage units="Percent|CFM25|CFM50|..."/>` — CFM units require fan-flow for fraction conversion
- `uom` crate v0.36: `ThermodynamicTemperature` `From`/`Into` conversion `°F → °C` uses `(°F - 32) × 5/9`
- OCHRE `vendors/OCHRE/ochre/utils/units.py:13-16`: Uses `pint.Quantity` for all unit conversions including temperature offset
- ASHRAE 152-2014: Duct leakage for DSE calculation requires fractional leakage (%), not volumetric (CFM) without additional airflow data
