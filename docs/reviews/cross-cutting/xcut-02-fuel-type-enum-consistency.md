# FuelType enum consistency: HPXML fuel strings map to correct enum, all fuel types in port accumulation

**Review ID**: xcut-02
**Category**: cross-cutting
**Date**: 2026-05-26

## Files Reviewed

- `crates/hares-types/src/equipment.rs` — FuelType enum definition (lines 145–157), FuelPower struct (lines 863–883), CoreFlows (lines 886–901)
- `crates/hares-types/src/ports.rs` — fuel_index() dispatch (lines 273–287), FuelAccumulator (lines 289–318), PortSlots::accumulate() (lines 500–570), PortContribution::Fuel (lines 72–75)
- `crates/hares-io/src/hpxml/xml_helpers.rs` — parse_fuel() (lines 9–26)
- `crates/hares-io/src/hpxml/resolve_water_heater.rs` — parse_water_heater_fuel() (lines 372–390)
- `crates/hares-equipment/src/hvac/helpers.rs` — parse_fuel_type() (lines 97–111)
- `crates/hares-io/src/hpxml/equipment.rs` — fuel_type_label() (lines 149–161)
- `crates/hares-core/src/observer_capture.rs` — diff_ports() (lines 63–127), capture_ports() (lines 129–170)
- `crates/hares-physics/src/units.rs` — BTU/h↔W, therms→kWh conversions (lines 92–117)
- `crates/hares-physics/src/constants.rs` — GAS_THERMS_PER_HOUR_TO_W (line 139)
- `crates/hares-equipment/src/hvac/heat_pump/heater.rs` — dual-fuel backup handling (lines 42–67, 1031–1035, 1588–1601)
- `crates/hares-equipment/src/scheduled_load.rs` — simultaneous electric+gas port writes (lines 515–531)
- `crates/hares-equipment/src/generator.rs` — hardcoded Gas fuel port (lines 784–790)
- `crates/hares-types/src/text.rs` — normalize_ascii() (line 11–13)
- `crates/hares-core/tests/port_accumulation_tests.rs` — fuel accumulation tests (lines 172–205)
- `crates/hares-types/src/ports.rs` — FuelAccumulator unit tests (lines 877–943)

## Vendor/Reference Files Consulted

None

## Findings

### Finding 1: Unrecognized HPXML fuel strings silently default to Electric [Severity: critical]

**Description**: The `parse_fuel()` function in `xml_helpers.rs` catches all unrecognized fuel strings in a catch-all branch and maps them to `FuelType::Electric` with only a `tracing::warn!` log message. This means any unrecognized or mis-typed fuel source in HPXML data is silently treated as zero-emission grid electricity.

**Code Location**: `crates/hares-io/src/hpxml/xml_helpers.rs:21–24`

```rust
other => {
    tracing::warn!(fuel = %other, "unrecognized fuel string; defaulting to Electric");
    FuelType::Electric
}
```

**Root Cause**: The function uses `unwrap_or("electricity")` on line 10 and has a catch-all default of `FuelType::Electric` on line 23. It treats "electricity" as the zero-information default, assuming that unknown fuels are safe to default to electric. This is a reasonable default for a missing `FuelType` element, but it is unsound for an explicitly specified but unrecognized fuel string.

**Impact**: If an HPXML file specifies `"district heating"`, `"solar"`, `"biogas"`, or a mis-capitalized variant (which survives `normalize_ascii`), the fossil or renewable heating consumption is attributed as zero-emission grid electricity in all downstream calculations — emissions estimates, energy costs, and fuel mix reports are all incorrect. The same applies to the `parse_water_heater_fuel()` function (`resolve_water_heater.rs:386–388`), though that variant correctly returns an `Err` rather than silently defaulting.

Note: `"kerosene"`, `"diesel"`, and `"anthracite coal"` are already handled correctly and map to `FuelType::Oil` and `FuelType::Coal` respectively. The issue is the unbounded catch-all.

**Recommendation**: Return an error from `parse_fuel()` for unrecognized fuel strings (as `parse_water_heater_fuel` does), or explicitly reject the HPXML file during import with a descriptive error message. The fallback to Electric should only apply when the `FuelType` element is entirely absent (`None`), not when it is present but unrecognized.

---

### Finding 2: `"none"` maps to `FuelType::Electric` in HPXML parsers, inconsistent with equipment helper [Severity: medium]

**Description**: Two of the three fuel-string parsers treat the string `"none"` as `FuelType::Electric`, while the third correctly maps it to `FuelType::None`. This creates different behaviour depending on which code path processes the fuel string.

**Code Locations**:

- `crates/hares-io/src/hpxml/xml_helpers.rs:12` — `"electricity" | "electric" | "none" => FuelType::Electric`
- `crates/hares-io/src/hpxml/resolve_water_heater.rs:377` — `"electricity" | "electric" | "none" => Ok(FuelType::Electric)`
- `crates/hares-equipment/src/hvac/helpers.rs:108` — `"none" | "no_fuel" | "no fuel" => Some(FuelType::None)`

**Root Cause**: The HPXML parsers were written defensively to accept `"none"` as a synonym for electric (matching OCHRE's convention), while the equipment helper treats `"none"` as the `FuelType::None` variant. The `"none"` string is not a valid HPXML fuel type — standard HPXML fuel types are enumerated values like `"electricity"`, `"natural gas"`, `"propane"`, `"fuel oil"`, etc.

**Impact**: If an HPXML file explicitly contains `FuelType="none"` (malformed but possible), the two HPXML parsers would create equipment with `fuel: Electric` while the equipment helper would return `None`. This discrepancy makes the system inconsistent — the same input produces different results depending on which parse function is called. Additionally, `parse_fuel("none")` → `FuelType::Electric` is not round-trip safe with `fuel_type_label(FuelType::None)` → `"none"`.

**Recommendation**: Align all three parsers. Either map `"none"` to `FuelType::None` everywhere (revised HPXML parsers), or remove `"none"` from HPXML parsers entirely since it is not a valid HPXML fuel type value. In standard HPXML, heat pump water heaters use `FuelType="electricity"`, never `"none"`.

---

### Finding 3: Observer capture omits Wood, Coal, and WoodPellet fuel types [Severity: medium]

**Description**: The observer snapshot and diff functions enumerate a subset of fuel types that excludes Wood, Coal, and WoodPellet. These fuel types are fully supported in the `FuelType` enum, HPXML parsers, equipment config, and `FuelAccumulator`, but their consumption data is invisible to the observer.

**Code Locations**:

- `crates/hares-core/src/observer_capture.rs:77–82` — `diff_ports()` only enumerates `[FuelType::Gas, FuelType::Propane, FuelType::Oil]` (also omits Electric, but Electric is captured through the ElectricalAccumulator path)
- `crates/hares-core/src/observer_capture.rs:137–146` — `capture_ports()` enumerates `[FuelType::Electric, FuelType::Gas, FuelType::Propane, FuelType::Oil]`, omitting Wood, Coal, and WoodPellet

**Root Cause**: The fuel type lists in the observer capture code were hardcoded when the codebase only modeled Gas/Propane/Oil combustion equipment. The Wood, Coal, and WoodPellet variants were added to the `FuelType` enum and parsers later but were never added to the observer capture enumeration.

**Impact**: If a wood furnace, coal boiler, or pellet stove is configured via HPXML import (which the parsers fully support — see `canonical_hvac_heating_name` at `resolve_hvac.rs:2177–2217`), the fuel consumption accumulates correctly in the `FuelAccumulator` but is silently omitted from observer snapshots, per-step diffs, and any downstream analysis based on observer data. This means CSV output columns, cost calculations, and emissions estimates based on observer data would show zero consumption for these equipment types despite correct port-level accumulation.

**Recommendation**: Replace the hardcoded fuel type arrays in `diff_ports()` and `capture_ports()` with the full set of fuel types (all seven values) or iterate over the `FuelIndex` range dynamically. A const array `[Electric, Gas, Propane, Oil, Wood, Coal, WoodPellet]` or dynamic iteration over `0..FUEL_TYPE_COUNT` would ensure new fuel types are automatically captured.

---

### Finding 4: Generator hardcodes `FuelType::Gas` regardless of HPXML configuration [Severity: low]

**Description**: The `Generator` equipment model writes `PortContribution::Fuel` with a hardcoded `FuelType::Gas`, ignoring the `FuelType` parsed from the HPXML `<Generator>/<FuelType>` element.

**Code Location**: `crates/hares-equipment/src/generator.rs:786–788`

```rust
ports.accumulate(&PortContribution::Fuel {
    fuel_type: FuelType::Gas,
    consumption_w: fuel_w,
})?;
```

**Root Cause**: The generator HPXML resolution code at `crates/hares-io/src/hpxml/resolve_der.rs:252` does parse the fuel type from the XML (`let fuel = parse_fuel(child_text(generator, "FuelType").as_deref())`), but the `Generator` equipment model does not store or use this value. It always emits `FuelType::Gas`.

**Impact**: If an HPXML file specifies a propane, diesel, or dual-fuel generator, the fuel consumption is attributed to natural gas in port accumulation, observer data, and downstream cost/emissions calculations. This is primarily a concern for propane generators (common in residential backup systems), where propane is more expensive and has different emissions factors than natural gas. Diesel generators are uncommon in residential but could appear in commercial HPXML files.

**Recommendation**: Store the parsed fuel type in the `GeneratorConfig` struct and use it when writing `PortContribution::Fuel`. Fall back to `FuelType::Gas` only when no fuel type is specified in the HPXML.

---

### Finding 5: Electric fuel accumulator slot is allocated but never populated [Severity: low]

**Description**: The `FuelAccumulator` allocates index 0 for `FuelType::Electric`, but no equipment writes `PortContribution::Fuel { fuel_type: FuelType::Electric, .. }`. All electric equipment writes to `PortContribution::Electrical`, which routes to `ElectricalAccumulator`, not `FuelAccumulator`.

**Code Locations**:

- `crates/hares-types/src/ports.rs:278` — `FuelType::Electric => Some(0)` (slot allocated)
- All 9 equipment types emitting `PortContribution::Fuel` use `FuelType::Gas` or their configured non-electric fuel type. None use `FuelType::Electric`.

**Root Cause**: The `FuelAccumulator` was designed with a complete index for all seven fuel types including Electric, but the architecture routes electric power through a separate `ElectricalAccumulator` for real power, reactive power, and net metering. The electric slot in `FuelAccumulator` is a dead slot.

**Impact**: This is a design clarity issue rather than a functional bug. The electric slot always returns 0.0 from `FuelAccumulator::get(FuelType::Electric)`, which could mislead someone reading the code into thinking electric fuel consumption is not being tracked (when it is, just in a different accumulator). There is no risk of double-counting because the slot is never written.

**Recommendation**: Either document explicitly that the electric slot in `FuelAccumulator` is a reserved dead slot (consumption flows through `ElectricalAccumulator` instead), or reduce `FUEL_TYPE_COUNT` to 6 and shift the indices to exclude Electric. Reducing the count would be a breaking change for serialization and is not recommended without a migration plan.

---

## Non-Findings (verified correct)

### (d) Dual-fuel equipment port contributions

The heat pump backup heater correctly separates electric and combustion fuel contributions. In the step function at `heater.rs:1588–1601`, the code detects whether backup is combustion-fueled and splits `er_power_w` between `er_electric_w` (zero when backup is fuel) and `fuel_w` (zero when backup is electric). The electric consumption goes to the electrical accumulator and the backup fuel goes to the fuel accumulator via `PortContribution::Fuel` at `heater.rs:1031–1035`. Both contributions can be emitted in the same timestep, and the `FuelAccumulator` sums them in independent index buckets — no double-counting occurs.

The `ScheduledLoad` also correctly writes both `PortContribution::Electrical` and `PortContribution::Fuel { fuel_type: FuelType::Gas }` in a single step at `scheduled_load.rs:515–531`, with independent accumulator dispatch.

### (e) Electricity source subdivision (grid vs. solar)

Grid vs. solar electricity distinction is handled at the `ElectricalAccumulator` level, not through `FuelType` variants. The `ElectricalAccumulator` tracks `load_power_kw` (consumption) and `generation_power_kw` (generation, negative) separately at `ports.rs:248–259`. Solar PV is modeled as equipment with `EndUse::PV` that writes negative `generation_power_kw`. The `net_active_kw()` method provides net-metered consumption. This design is correct — `FuelType::Electric` represents the energy carrier, while grid vs. self-consumed solar is a routing/tariff concern handled in the electrical domain.

### (f) Unit conversions: Btu/therm ↔ Joules/Watts

All unit conversions use NIST-conformant constants:
- `crates/hares-physics/src/units.rs:96`: `BTU_PER_HOUR_TO_WATT = 0.293_071_07` (NIST: 1 BTU(IT)/h = 0.29307107 W)
- `crates/hares-physics/src/constants.rs:139`: `GAS_THERMS_PER_HOUR_TO_W = 29_307.107_017_222_2` derived as `100,000 × 1055.05585262 / 3600`, matching the NIST value for 1 BTU_IT = 1055.05585262 J
- `crates/hares-physics/src/units.rs:114–117`: `energy_therms_to_kwh()` uses `100,000 * BTU_IT` through the uom library, correct

All fuel port contributions use watts (`consumption_w`), maintaining unit consistency throughout the port system. The review instruction's values of 1055.06 J/Btu and 105,506,000 J/therm are approximate; the code uses the higher-precision NIST SP 811 values, which are more accurate.

### HPXML string round-trip integrity

The `parse_fuel()` → `fuel_type_label()` round-trip is correct for all standard HPXML fuel types:
- `"electricity"` → `FuelType::Electric` → `"electricity"`
- `"natural gas"` → `FuelType::Gas` → `"natural gas"`
- `"propane"` → `FuelType::Propane` → `"propane"`
- `"fuel oil"` → `FuelType::Oil` → `"fuel oil"`
- `"wood"` → `FuelType::Wood` → `"wood"`
- `"coal"` → `FuelType::Coal` → `"coal"`
- `"wood pellets"` → `FuelType::WoodPellet` → `"wood pellets"`

Both `"natural gas"` (with space) and `"natural_gas"` (with underscore) are accepted, accommodating both HPXML standard format and common config-file variants.

### FuelAccumulator correct operation

The `fuel_index()` function correctly maps all 7 fuel types to contiguous indices 0–6. The `add()` method rejects `FuelType::None` with an error. Unit tests at `ports.rs:878–943` verify that individual fuel type tracking, independence, `None` rejection, and `zero()` reset all work correctly. Integration tests at `port_accumulation_tests.rs:172–205` confirm that fuel types are independently accumulated.

## Summary

- Total findings: 5
- Critical: 1 (unrecognized fuels silently default to Electric)
- High: 0
- Medium: 2 (`"none"` fuel string mapping inconsistency; observer capture omits Wood/Coal/WoodPellet)
- Low: 2 (generator hardcodes Gas; dead electric slot in FuelAccumulator)

## Recommendations

1. **Critical fix — `parse_fuel()` catch-all**: Replace the silent `FuelType::Electric` default for unrecognized fuel strings in `xml_helpers.rs:21–24` with an error return. The fallback to Electric should only apply when the fuel element is absent (`None`), not when it is present but unrecognized. Align with `parse_water_heater_fuel()` which already returns `Err` for unrecognized fuels.

2. **Align `"none"` mapping across all parsers**: Either remove `"none"` from the HPXML parsers (since it is not a standard HPXML value) or map it consistently to `FuelType::None` in all three parse functions. The current inconsistency means the same input can produce different results depending on which code path processes it.

3. **Expand observer fuel type enumeration**: Replace the hardcoded fuel type arrays in `diff_ports()` (line 77) and `capture_ports()` (lines 137–142) in `observer_capture.rs` with the full set of all seven fuel types so that Wood, Coal, and WoodPellet consumption is visible in observer data.

4. **Store and use generator fuel type**: Add a `fuel_type` field to the `GeneratorConfig` struct that respects the HPXML-parsed value, and use it when emitting `PortContribution::Fuel` in `generator.rs:786–788` instead of hardcoding `FuelType::Gas`.

5. **Document the electric slot in FuelAccumulator**: Add a doc comment clarifying that `FuelAccumulator` index 0 (Electric) is intentionally unused because electric power flows through `ElectricalAccumulator` instead. This prevents future developers from assuming electric fuel tracking is broken.

## References / Citations

- HPXML v4.0 schema: `<FuelType>` enumeration includes `"electricity"`, `"natural gas"`, `"fuel oil"`, `"fuel oil 1"`, `"fuel oil 2"`, `"fuel oil 4"`, `"fuel oil 5/6"`, `"propane"`, `"kerosene"`, `"diesel"`, `"coal"`, `"anthracite coal"`, `"bituminous coal"`, `"coke"`, `"wood"`, `"wood pellets"`
- NIST SP 811 (2008): 1 BTU(IT) = 1055.05585262 J
- OCHRE HVAC.py: dual-fuel heat pump implementation separates compressor electric power from backup fuel power (reference at `heater.rs:42–67`)
