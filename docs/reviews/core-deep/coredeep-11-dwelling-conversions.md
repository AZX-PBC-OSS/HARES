# Dwelling conversions: HPXML to equipment/config types, unit conversions, precision loss
**Review ID**: coredeep-11
**Category**: core-deep
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-core/src/dwelling/conversions.rs` (building/zone/boundary/equipment-config conversions)
- `crates/hares-io/src/hpxml/building.rs` (HPXML XML → `Building` struct, unit-annotated value parsing, area/volume/temperature/length/R/U/density/specific-heat/conductivity conversions)
- `crates/hares-io/src/hpxml/xml_helpers.rs` (shared helpers: `parse_fuel`, `child_temperature_c`, `child_energy_kwh`, setpoint parsing)
- `crates/hares-io/src/hpxml/resolve_hvac.rs` (HVAC system-type → canonical-name mapping, fuel-type parsing, capacity conversion)
- `crates/hares-io/src/hpxml/resolve_water_heater.rs` (water-heater fuel parsing, type mapping, UA conversion, tank-volume conversion)
- `crates/hares-io/src/hpxml/equipment.rs` (`build_spec`, `fuel_type_label`, HPXML → `EquipmentSpec`)
- `crates/hares-physics/src/units.rs` (unit-conversion primitives: area, volume, power, energy, R-value, U-value, temperature, density, specific heat, conductivity)
- `crates/hares-types/src/equipment.rs` (`FuelType` enum definition)
- `crates/hares-types/src/text.rs` (`normalize_ascii`, `parse_trimmed_f64`)

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Dwelling.py` (dwelling orchestration, fuel-type handling, equipment initialisation)
- `vendors/OCHRE/ochre/utils/units.py` (Pint-based `convert()`; hardcoded source-unit annotations)
- `vendors/OCHRE/ochre/utils/hpxml.py` (HPXML parsing: fuel strings, equipment-type strings, area/volume/temperature/U-value conversions; 1809 lines)

## Findings

### Finding 1: [Severity: high] `parse_fuel` silently defaults unrecognised fuel strings to Electric
**Description**: The shared `parse_fuel()` in `xml_helpers.rs:9-26` matches known HPXML fuel strings but falls through to a catch-all that issues a `tracing::warn` and returns `FuelType::Electric` (line 23). The water-heater-specific `parse_water_heater_fuel()` in `resolve_water_heater.rs:372-390` correctly returns `Err(HpxmlError::Parse(...))` on the same condition (line 386-388). This inconsistency means a typo in an HPXML FuelType element (e.g. `"natuarl gas"`) on an HVAC system silently defaults to an electric-resistance model with zero gas consumption, while the same typo on a water heater fails cleanly at parse time. OCHRE rejects unknown fuel types by raising `OCHREException` (e.g. `hpxml.py:1293-1294` for clothes dryers, `hpxml.py:1444` for cooking ranges).
**Code Location**: `crates/hares-io/src/hpxml/xml_helpers.rs:21-24`
**Root Cause**: The general-purpose `parse_fuel` was written to be lenient with a default, while `parse_water_heater_fuel` was written to be strict (correct behaviour). The two functions are not unified.
**Impact**: Silent model misconfiguration. An HPXML file with an unrecognised fuel string on HVAC equipment will simulate the wrong equipment class (electric instead of gas/propane/oil), producing incorrect energy consumption, thermal gains, and emissions estimates with no parse error.

### Finding 2: [Severity: high] Missing-unit default for general `parse_fuel` is `"electricity"` rather than error
**Description**: At `xml_helpers.rs:9-10`, `parse_fuel` unwraps a `None` input to `"electricity"`. When an HPXML heating or cooling system omits the `<FuelType>` child element, the parser silently assigns electric fuel. In contrast, `parse_water_heater_fuel` (line 373-375) returns an error when `<FuelType>` is missing. OCHRE does not provide a default for fuel — it reads the HPXML key directly (e.g. `water_heater["FuelType"]`) which would raise `KeyError` if absent, making the problem immediately visible.
**Code Location**: `crates/hares-io/src/hpxml/xml_helpers.rs:10`
**Root Cause**: The default of `"electricity"` was chosen as a convenience, but a missing required field is a signal that the HPXML is incomplete. Defaulting silently hides data quality problems.
**Impact**: HPXML files missing a `<FuelType>` for gas furnaces would be parsed as electric, bypassing gas consumption entirely.

### Finding 3: [Severity: high] Missing SI-unit recognition in area/volume/length/temperature conversion functions
**Description**: Each `convert_*_to_*` function in `building.rs:1984-2171` recognises a limited set of IP unit strings and falls back to returning the raw value when the unit string is not matched. None of these functions recognise SI unit aliases for the value they are converting:

| Function | Recognised IP units | Missing SI units |
|---|---|---|
| `convert_area_to_m2` (line 1984) | `"ft2"`, `"ft^2"`, `"ftsq"`, `"ftsq."`, `"square feet"` | `"m2"`, `"m^2"`, `"sq m"`, `"square meters"` |
| `convert_volume_to_m3` (line 2003) | `"ft3"`, `"ft^3"`, `"cubic feet"` | `"m3"`, `"m^3"`, `"cubic meters"`, `"gal"`, `"gallon"` |
| `convert_length_to_m` (line 2090) | `"in"`, `"inch"`, `"inches"`, `"ft"`, `"feet"` | `"m"`, `"meter"`, `"meters"`, `"cm"`, `"centimeters"` |
| `convert_temperature_to_c` (line 2150) | `"F"`/`"f"`/`"degF"`/`"degf"`/`"fahrenheit"` | (already handles `"C"`/`"c"`/etc.) |
| `convert_conductivity_to_w_m_k` (line 2066) | `"btu/hr-ft-f"`, `"btu-in/hr-ft2-f"` | `"W/(m*K)"`, `"W/m-K"` |
| `convert_density_to_kg_m3` (line 2108) | `"lb/ft3"`, `"lb/ft^3"`, `"lbm/ft3"` | `"kg/m3"`, `"kg/m^3"` |
| `convert_specific_heat_to_j_kg_k` (line 2129) | `"btu/lb-f"`, `"btu/(lb*f)"` | `"J/(kg*K)"`, `"kJ/(kg*K)"` |

When the HPXML `units` attribute carries an SI annotation (valid per HPXML v4.x schema), the raw value passes through without the identity conversion. For `convert_area_to_m2`, an area value with `units="m2"` would be returned as-is (correct, since it's already in m²). But for `convert_volume_to_m3`, a volume with `units="gal"` (US gallons) would be returned raw, treating a gallon value as if it were already in m³ — an error of the factor ~264×.

OCHRE avoids this class of bug by hardcoding the source unit at every `convert()` call site (line 13-16 of `units.py`) rather than reading the HPXML units attribute dynamically, so it never encounters an unrecognised unit string at runtime.
**Code Location**: `crates/hares-io/src/hpxml/building.rs:1989-1991` (area), `2005-2008` (volume), `2094-2097` (length), `2156-2162` (temperature), `2072-2078` (conductivity), `2111-2117` (density), `2132-2138` (specific heat)
**Root Cause**: The conversion functions attempt to read HPXML `units` attributes, but the match arms only cover IP unit strings. The unrecognised fallback returns the raw value regardless of whether the raw value is in a different unit system than the function's output domain.
**Impact**: When an HPXML file uses SI unit annotations (which the HPXML schema permits for any value element), some values are correctly treated as already in SI (area, temperature), while others are silently misinterpreted as SI (volume in gallons, length in metres when labelled `m` and returned raw as if already in m). This produces silent magnitude errors.

### Finding 4: [Severity: medium] `normalize_ascii` cannot normalise non-ASCII Unicode fuel strings
**Description**: The `normalize_ascii` function at `hares-types/src/text.rs:12` calls `s.trim().to_ascii_lowercase()`. While HPXML v4.x fuel strings are plain ASCII, `to_ascii_lowercase()` does not handle full-width characters, accented characters, or confusable Unicode homoglyphs. A non-ASCII fuel string passes through without lowercasing, causing a legitimate fuel like `"Electricity"` to match (because `parse_fuel` normalises first), while a non-ASCII `"ｅｌｅｃｔｒｉｃｉｔｙ"` (full-width) would fail to match. This is unlikely in practice but is a latent robustness gap.
**Code Location**: `crates/hares-types/src/text.rs:11-13`
**Root Cause**: `to_ascii_lowercase()` is used in a context where strings from user-supplied XML may contain non-ASCII characters.
**Impact**: Low risk for current HPXML files, but a fragile assumption for a parser ingesting third-party XML.

### Finding 5: [Severity: medium] Duplicate fuel-parsing logic with divergent error behaviour
**Description**: There are two nearly identical implementations of fuel-type parsing:
1. `parse_fuel` in `xml_helpers.rs:9-26` (used for HVAC, loads, generators, DER)
2. `parse_water_heater_fuel` in `resolve_water_heater.rs:372-390`

Both map the same set of HPXML fuel strings to the same `FuelType` variants, but differ in:
- Default for missing input: `parse_fuel` defaults to `"electricity"` (line 10); `parse_water_heater_fuel` returns `Err` (line 373-375)
- Default for unrecognised input: `parse_fuel` defaults to `Electric` with a warn (line 23); `parse_water_heater_fuel` returns `Err` (line 386-388)
- Supported aliases differ subtly: `parse_fuel` accepts `"electric"` as an alias for `"electricity"` (line 12); `parse_water_heater_fuel` does the same (line 377). `parse_fuel` does NOT accept `"diesel"` (line 16 lists `"kerosene"` and `"diesel"` as oil subtypes); `parse_water_heater_fuel` does accept them (line 381).

OCHRE has no fuel lookup table at all — it uses HPXML strings directly with a `.capitalize()` transformation and a small set of explicit `in [...]` checks (e.g. `hpxml.py:1289-1292`), which is simpler and avoids duplication.
**Code Location**: `crates/hares-io/src/hpxml/xml_helpers.rs:9-26` and `crates/hares-io/src/hpxml/resolve_water_heater.rs:372-390`
**Root Cause**: The water heater resolver was written as a separate module and independently re-implemented fuel parsing.
**Impact**: Maintenance risk — any new fuel type or HPXML alias added to one function must be mirrored in the other. Divergent error behaviour means the same HPXML fuel typo produces different outcomes depending on equipment type.

### Finding 6: [Severity: medium] `child_temperature_c` duplicates `convert_temperature_to_c` with different unit string handling
**Description**: Temperature conversion from HPXML is implemented in two places:
1. `child_temperature_c` in `xml_helpers.rs:43-66` — used for water heater setpoints and equipment-level temperatures
2. `convert_temperature_to_c` in `building.rs:2150-2172` — used for building-level temperature values via `parse_value_with_units`

Both default to Fahrenheit (line 52 in xml_helpers, line 2164-2169 in building) and both handle Fahrenheit/Celsius unit strings. However, `child_temperature_c` adds support for `"degf"`/`"degc"` (lines 54, 56) which `convert_temperature_to_c` does not recognise. Conversely, `convert_temperature_to_c` handles uppercase `"F"`/`"C"` on lines 2152/2155 which `child_temperature_c` only handles via the default `unwrap_or("F")` and the `to_ascii_lowercase()` transformation on line 53.

Notably, both functions handle the `None` (missing units) case the same way — assume °F — which is consistent with HPXML convention.
**Code Location**: `crates/hares-io/src/hpxml/xml_helpers.rs:43-66` and `crates/hares-io/src/hpxml/building.rs:2150-2172`
**Root Cause**: Different parsing paths for different HPXML sections implemented temperature-reading independently.
**Impact**: Maintenance burden; two code paths for the same semantic operation. Subtle differences in unit-string matching could cause one path to treat a value as °C while the other treats it as °F.

### Finding 7: [Severity: medium] `child_energy_kwh` and `child_load_kwh` silently return `None` on unrecognised units
**Description**: `child_energy_kwh` in `xml_helpers.rs:68-77` matches only `"kwh"`, `"kwh/year"`, `"kwh/yr"`, `"wh"`, `"wh/year"`, `"wh/yr"` and returns `None` for all other unit strings (line 73-76). Similarly `child_load_kwh` matches only `"kwh/year"`, `"kwh/yr"`, `"kwh"` (line 84). `child_load_therms` matches only `"therm/year"`, `"therm/yr"`, `"therm"` (line 94). If an HPXML file uses `"therm"` as the units on a load that the parser expects in kWh (or vice versa), the load is silently dropped rather than triggering a conversion or error. In OCHRE, the equivalent patterns (e.g. `hpxml.py:1536` for MGL loads checking `"therm/year"`) are assertions that raise exceptions rather than silently skipping.
**Code Location**: `crates/hares-io/src/hpxml/xml_helpers.rs:68-97`
**Root Cause**: The functions accept a narrow set of unit strings and the callers treat `None` as "field absent" rather than "field present but in wrong units".
**Impact**: HPXML loads specified in the wrong energy unit are silently dropped, producing zero energy consumption for that end-use without a parse error.

### Finding 8: [Severity: medium] `parse_fuel` treats `"none"` as `FuelType::Electric`
**Description**: At `xml_helpers.rs:12`, the string `"none"` maps to `FuelType::Electric`. The HPXML schema defines `<FuelType>none</FuelType>` as a valid value for equipment that consumes no fuel (e.g. passive solar thermal). Mapping it to `Electric` is semantically incorrect — the Rust `FuelType` enum has a `None` variant (the `#[default]` at hares-types/src/equipment.rs:156) which would be the correct mapping. The water heater variant correctly handles this (line 377 maps `"none"` to `Ok(FuelType::Electric)`, which is also wrong for the same reason). OCHRE does not explicitly handle `"none"` in fuel-type checks — it would pass through `.capitalize()` to become `"None"` and likely cause downstream issues.
**Code Location**: `crates/hares-io/src/hpxml/xml_helpers.rs:12` and `crates/hares-io/src/hpxml/resolve_water_heater.rs:377`
**Root Cause**: The `FuelType::None` variant was added later as a default and the parser mapping for the HPXML string `"none"` was not updated.
**Impact**: Equipment explicitly marked as fuel-less in HPXML would be modelled as electric equipment, consuming phantom electricity.

### Finding 9: [Severity: medium] `btu_hr_per_f_to_w_per_k` is algebraically correct but `UA` pass-through reverses the temperature delta when HPXML UA is in `Btu/hr-°F`
**Description**: The water heater `UA` calculation at `resolve_water_heater.rs:89` uses `ua_from_energy_factor()` which optionally produces a `ua_w_per_k` value. Separately, HPXML may carry a `<UA>` element in `Btu/hr-°F`. When this value is read and converted (line `1159` is an example of the pattern), the conversion `btu_hr_per_f_to_w_per_k` uses `BTU_PER_HOUR_TO_WATT * 9.0 / 5.0` (units.rs:202-204). The factor `9/5` is the correct factor for converting a **per-degree-difference** quantity from °F basis to K/°C basis (since Δ°C = (5/9)Δ°F, so per-Δ°F × 9/5 = per-Δ°C). This is mathematically correct — verified against the ASHRAE Handbook convention for converting UA.
**Code Location**: `crates/hares-physics/src/units.rs:202-204` and `crates/hares-io/src/hpxml/resolve_water_heater.rs:89`
**Root Cause**: N/A — this is a verification finding confirming correctness.
**Impact**: None. The UA conversion is correct.

### Finding 10: [Severity: low] `EMISSIVITY_WINDOW` constant used for window radiation coupling is 0.84 as documented, matching OCHRE
**Description**: The `conversions.rs:317-318` uses `EMISSIVITY_WINDOW` (value 0.84 from `hares_envelope::longwave_radiation`) for window interior emissivity. The comment at lines 300-316 documents the rationale: the E+ Simple Window Model polynomial was derived at ε = 0.84 (NFRC rating value), and using the default 0.9 would over-couple window radiation by ~0.34 W/(m²·K). This is confirmed at `conversions.rs:316` with the reference "OCHRE Envelope.py uses ε = 0.84 for window radiation_frac." ✓ Verified consistent with OCHRE.
**Code Location**: `crates/hares-core/src/dwelling/conversions.rs:317-318`
**Root Cause**: N/A — verification finding.
**Impact**: None. Correct implementation.

### Finding 11: [Severity: low] Water heater `TankVolume` uses `volume_gal_to_m3` (US gallon) — correct for HPXML
**Description**: The water heater parser at `resolve_water_heater.rs:53` converts rated tank volume via `conv::volume_gal_to_m3(gal * volume_correction)`. The `uom::si::volume::gallon` type used in `units.rs:169-170` is the US liquid gallon (≈3.785 L). HPXML uses US customary units, so this is correct. OCHRE uses `convert(volume, "gallon", "L")` which also targets US gallons (Pint defaults to US liquid gallon). ✓ Consistent.
**Code Location**: `crates/hares-physics/src/units.rs:169-170` and `crates/hares-io/src/hpxml/resolve_water_heater.rs:52-53`
**Root Cause**: N/A — verification finding.
**Impact**: None. Correct.

### Finding 12: [Severity: low] `first_hour_rating_gal` uses raw `conv::volume_gal_to_m3` without volume correction
**Description**: At `resolve_water_heater.rs:58`, the first-hour rating gallon value is converted directly: `first_hour_rating_gal.map(conv::volume_gal_to_m3)`. The rated tank volume on line 53 applies a volume correction (0.9 for electric, 0.95 for gas) before converting: `gal * volume_correction`. The first-hour rating does not receive this correction. FHR represents the total hot water available in a one-hour period and is a performance rating, not a physical volume, so the correction would not apply. ✓ Correct — the correction is for rated volume only per ASHRAE testing standards.
**Code Location**: `crates/hares-io/src/hpxml/resolve_water_heater.rs:53` vs `58`
**Root Cause**: N/A — verification finding.
**Impact**: None. Correct distinction between rated volume and performance rating.

### Finding 13: [Severity: low] Heating/Cooling capacity is read in `kBTU/h` and converted to both kBTU/h (stored as parameter) and W (used internally)
**Description**: The `insert_capacity_kbtu_h` function at `resolve_hvac.rs:2230-2246` stores the raw kBTU/h value as a parameter (`"{tag}_kbtu_h"`) AND converts to watts via `power_btu_h_to_w`. This is double-storage of the same value and could lead to inconsistency if one of the two values is subsequently modified. However, it provides backward compatibility for equipment models that access the kBTU/h parameter directly. ✓ Design trade-off documented.
**Code Location**: `crates/hares-io/src/hpxml/resolve_hvac.rs:2230-2246`
**Root Cause**: N/A — design observation.
**Impact**: Low maintenance burden. If the canonical value is the Watt version, the kBTU/h parameter could eventually become stale if updated independently.

### Finding 14: [Severity: low] Setpoint temperatures from HPXML extensions are parsed as °F via comma-delimited string
**Description**: The `parse_setpoint_from_control` function at `xml_helpers.rs:151-184` reads 24-hour setpoint schedules from HPXML `<extension>` elements. The values are comma-separated strings parsed as `f64` and then converted via `conv::temperature_f_to_c` (line 166). The fallback path for single-value setpoints also does `conv::temperature_f_to_c` (line 178). Both assume °F with no unit annotation. This matches HPXML convention (setpoints are always in °F in HPXML) and OCHRE's approach (`convert(..., "degF", "degC")` at `hpxml.py:927-928`). ✓ Consistent.
**Code Location**: `crates/hares-io/src/hpxml/xml_helpers.rs:166, 178`
**Root Cause**: N/A — verification finding.
**Impact**: None. Correct assumption per HPXML spec.

### Finding 15: [Severity: medium] `energy_therms_to_kwh` lacks inverse function for kwh-to-therms used in OCHRE
**Description**: The units module at `units.rs:115-117` provides `energy_therms_to_kwh` for converting therms to kWh. OCHRE calculates fuel fractions using the reverse conversion (`kwh_to_therms` at `hpxml.py:1321`) to normalise electric and gas loads into common units for gain-fraction computation. The HARES codebase does not contain an analogous `energy_kwh_to_therms` function. Gas equipment gain fractions (sensible/latent) at `resolve_loads.rs:204` use hardcoded factors (0.90 for electric, 0.8894 for gas) rather than computing them from therms/kWh equivalence. This is mechanically correct but not directly comparable to the OCHRE approach.
**Code Location**: `crates/hares-physics/src/units.rs:115-117` and `crates/hares-io/src/hpxml/resolve_loads.rs:204`
**Root Cause**: Different architectural approaches to gain-fraction calculation.
**Impact**: Minor — the hardcoded factors must be validated against the energy-equivalent calculation that OCHRE performs.

## Summary
- Total findings: 15
- Critical: 0
- High: 3 (Findings 1, 2, 3)
- Medium: 7 (Findings 4, 5, 6, 7, 8, 9, 15)
- Low: 5 (Findings 10, 11, 12, 13, 14)

## Recommendations
1. **Unify fuel parsing**: Replace the two `parse_fuel` / `parse_water_heater_fuel` functions with a single implementation that returns `Result<FuelType, HpxmlError>` (never silently defaulting). Route all HPXML fuel-type parsing through this single function.
2. **Add SI unit recognition**: Extend each `convert_*_to_*` function in `building.rs` to recognise SI unit strings for its target domain (e.g. `convert_area_to_m2` should pass through `"m2"`, `convert_length_to_m` should pass through `"m"`, etc.). For volume specifically, add `"gal"`/`"gallon"` recognition to `convert_volume_to_m3` to guard against misparse.
3. **Map HPXML `"none"` to `FuelType::None`**: The string `"none"` in HPXML `<FuelType>` should produce `FuelType::None` rather than `FuelType::Electric`.
4. **Fail on unrecognised units**: Consider changing the unrecognised-unit fallbacks in `convert_*_to_*` functions from `tracing::warn` + raw-value return to a hard error (`Err`) at the `parse_value_with_units` layer, so the dwelling builder can decide whether to abort or substitute a default.
5. **Deduplicate temperature parsing**: Unify `child_temperature_c` and `convert_temperature_to_c` so that HPXML temperature unit recognition is consistent across all parsing paths.
6. **Add energy unit detection for loads**: `child_energy_kwh` and `child_load_kwh` / `child_load_therms` should issue a warning (at minimum) when the units field is present but unrecognised rather than silently returning `None`.

## References / Citations
- OCHRE `units.py`: `convert()` using Pint; hardcoded source units at each call site (lines 13-16)
- OCHRE `hpxml.py`: fuel string mapping (lines 839-846, 1027, 1288-1294, 1438-1445), water heater type matching (lines 1026-1129), area/volume/temperature conversions (lines 140, 253-254, 927-928, 951, 1028, 1048, 1158, 1165)
- OCHRE `hpxml.py`: zone-name normalisation (lines 12-28, 65-84)
- `uom` crate: `si::volume::gallon` = US liquid gallon (3.785411784 L per NIST)
- NIST SP 811: `BTU(IT)/h → W` = `0.29307107` (used at `units.rs:96`)
- ASHRAE HoF 2021 Ch. 18.31: F-factor slab method (referenced at `conversions.rs:158-165`)
- E+ Eng.Ref Window Heat Transfer Calculations: ε = 0.84 for NFRC-rated glass (referenced at `conversions.rs:300-316`)
- HPXML v4.0 XSD: FuelType enumeration values; unit annotation on value elements
