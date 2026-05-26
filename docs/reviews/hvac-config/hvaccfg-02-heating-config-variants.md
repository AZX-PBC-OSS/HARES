# HeatingConfig variants: Furnace, Boiler, Baseboard, HeatPumpHeater defaults
**Review ID**: hvaccfg-02
**Category**: hvac-config
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-equipment/src/hvac/heating_config.rs`
- `crates/hares-equipment/src/hvac/heat_pump_config.rs`
- `crates/hares-equipment/src/hvac/core_config.rs`

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/HeatingCoils.hh` / `.cc` (furnace burner efficiency defaults)
- `vendors/EnergyPlus/src/EnergyPlus/Boilers.hh` / `.cc` (boiler efficiency, outlet temp defaults)
- `vendors/EnergyPlus/src/EnergyPlus/BaseboardElectric.hh` / `.cc` (electric baseboard)
- `vendors/EnergyPlus/src/EnergyPlus/ElectricBaseboardRadiator.hh` / `.cc` (radiant electric baseboard)
- `vendors/EnergyPlus/src/EnergyPlus/HWBaseboardRadiator.hh` / `.cc` (hot-water baseboard)
- `vendors/EnergyPlus/src/EnergyPlus/StandardRatings.hh` / `.cc` (HSPF calculation)
- `vendors/EnergyPlus/src/EnergyPlus/DXCoils.hh` / `.cc` (rated COP for heat pump heating)
- `vendors/EnergyPlus/idd/Energy+.idd.in` (input field defaults and constraints)

## Findings

### Finding 1: [Severity: critical] No AFUE bounds validation — Second Law violation possible
**Description**: Neither `GasFurnaceConfig` nor `GasBoilerConfig` validates that `afue` falls within [0, 1]. An AFUE > 1.0 violates the Second Law of Thermodynamics, yet serialization/deserialization accepts such values silently. An AFUE < 0 is physically meaningless. EnergyPlus issues a warning when boiler efficiency exceeds 1.0 (`Boilers.cc:296`). There is no `validate()` method on `GasFurnaceConfig` or `GasBoilerConfig` at all.

**Code Location**:
- `GasFurnaceConfig`: `crates/hares-equipment/src/hvac/heating_config.rs:20` (field declaration), `heating_config.rs:48-64` (Default impl with no validation)
- `GasBoilerConfig`: `crates/hares-equipment/src/hvac/heating_config.rs:122` (field declaration), `heating_config.rs:165-183` (Default impl with no validation)
- HPXML resolver: `crates/hares-io/src/hpxml/resolve_hvac.rs:423-437` (`afue_from_params` — extracts value without range check); `resolve_hvac.rs:640-645` (requires AFUE but does not validate [0,1])

**Root Cause**: Missing validation. `HeatPumpHeaterConfig` has a `validate()` method that checks finiteness and positivity, but the furnace and boiler configs lack equivalent checks. The `afue` field in both structs is declared as plain `f64` with no `#[serde(default)]` (making it required in JSON), but no custom deserialization guard or runtime validation enforces [0, 1].

**Impact**: Physically impossible configurations accepted. A user supplying `afue: 1.5` or `afue: -0.3` in a typed config or HPXML derivative gets a silently broken model where the boiler/furnace "generates" more heat than the fuel combustion provides. Downstream simulation may produce unbounded or negative energy consumption.

### Finding 2: [Severity: medium] GasBoilerConfig contradictory defaults: condensing=false with condensing return temperature
**Description**: `GasBoilerConfig` defaults `condensing: false` (non-condensing) but `return_temp_c: 40.0 °C`, which is explicitly documented in `default_return_temp_c()` at line 364-371 as a condensing-mode operating point ("typical condensing-boiler return temperature… below the ~55 °C flue-gas dewpoint"). A non-condensing boiler operating at 40 °C return would suffer sustained flue-gas condensation and corrosion — real non-condensing boilers maintain ≥ 70–82 °C return to avoid this. EnergyPlus has no equivalent contradiction because it uses `TempDesBoilerOut` (design outlet temperature) and calculates return temperature from the plant loop ΔT.

**Code Location**:
- `crates/hares-equipment/src/hvac/heating_config.rs:165-183` (Default impl: `condensing: false`, `return_temp_c: default_return_temp_c()` = 40.0)
- `crates/hares-equipment/src/hvac/heating_config.rs:364-371` (`default_return_temp_c` — self-described as condensing return temp)

**Root Cause**: The `condensing` default was chosen for safety (non-condensing is the conservative default), but `return_temp_c` was chosen for condensing performance. These two defaults were set independently without cross-consistency checking.

**Impact**: Users who construct a `GasBoilerConfig` with defaults get inconsistent thermodynamic assumptions. If the simulation respects `condensing: false`, it uses a 10-coefficient non-condensing efficiency curve but drives it at a condensing-level return temperature. If it respects `return_temp_c: 40.0`, it predicts unrealistically high efficiency.

### Finding 3: [Severity: medium] condensing field not auto-inferred from AFUE during deserialization
**Description**: The field comment at `heating_config.rs:150-156` states: "Condensing boiler mode. Inferred from AFUE > 0.90 (OCHRE convention)." However, `condensing` has `#[serde(default)]` (defaults to `false`) and there is no deserialization-time inference from `afue`. The inference only occurs in the HPXML resolver at `resolve_hvac.rs:811` (`condensing: afue > 0.90`). If a config is constructed in Rust or deserialized from JSON without explicitly setting `condensing`, the field is `false` regardless of AFUE.

**Code Location**:
- `crates/hares-equipment/src/hvac/heating_config.rs:155-157` (field with `#[serde(default)]`, comment promises inference)
- `crates/hares-equipment/src/hvac/heating_config.rs:180` (default is `false`)
- `crates/hares-io/src/hpxml/resolve_hvac.rs:811` (only place inference actually happens)

**Root Cause**: The inference comment describes intended behavior that the resolver path implements, but the struct-level deserialization path does not. A `#[serde(deserialize_with)]` or a `Deserialize` impl is missing.

**Impact**: Configs constructed outside the HPXML path (e.g. programmatic Rust construction, direct JSON) may silently run with incorrect efficiency curve selection (10-coefficient non-condensing vs. 6-coefficient condensing), yielding wrong fuel consumption estimates for AFUE > 0.90.

### Finding 4: [Severity: low] ElectricBaseboardConfig missing capacity-per-length and thermal time constant fields
**Description**: `ElectricBaseboardConfig` has only `capacity_w` (total), `eir`, and setpoints. The review spec expects a heating capacity per linear meter/foot (typically 500–800 W/m) and a thermal time constant. EnergyPlus's `BaseboardElectric` also lacks a user-configurable time constant and capacity-per-length (instead using W, W/m², or fraction of autosized capacity). However, EnergyPlus's `ElectricBaseboardRadiator` has a `CapacitanceAir` for transient effects. HARES could benefit from:
- A `capacity_per_meter_w` field for zone sizing from room dimensions
- A `time_constant_s` field for transient thermal response

Neither is strictly needed for steady-state simulation, but both improve physical fidelity for room-by-room sizing.

**Code Location**: `crates/hares-equipment/src/hvac/heating_config.rs:252-267`

**Root Cause**: Scope limitation — only electric resistance baseboard is modeled; hot-water baseboard (`BaseboardConfig`) is absent.

**Impact**: Zone-level sizing must be done externally. Transient warm-up/cool-down behavior of baseboard elements is not captured.

### Finding 5: [Severity: low] HeatPumpHeaterConfig has no HSPF field — conversion lives outside config
**Description**: `HeatPumpHeaterConfig` stores efficiency as `heating_eir: Option<f64>` (dimensionless EIR), not as HSPF. The HSPF→EIR conversion happens in the HPXML resolver: `EIR = BTU_PER_HR_PER_W / HSPF = 3.412 / HSPF` (`resolve_hvac.rs:1146`). This conversion is correct: COP = HSPF / 3.412 (per the HSPF definition of BTU/Wh output per Wh input). However, there is no way to set an HSPF value directly in the typed config — users must pre-convert to EIR. The EnergyPlus approach stores the rated COP and computes HSPF via the full AHRI 210/240 methodology, which is more rigorous.

**Code Location**:
- `crates/hares-equipment/src/hvac/heat_pump_config.rs:29` (`heating_eir` field)
- `crates/hares-io/src/hpxml/resolve_hvac.rs:1146` (HSPF→EIR conversion)
- `crates/hares-physics/src/constants.rs:126` (`BTU_PER_HR_PER_W = 3.412_141_633`)

**Root Cause**: Design choice — EIR is the internal representation; HSPF is an input-side convenience.

**Impact**: No loss of correctness; the conversion `3.412 / HSPF` is standard. Users constructing configs programmatically must calculate EIR themselves.

### Finding 6: [Severity: low] GasFurnaceConfig afue field inconsistent between Rust Default and serde requirement
**Description**: The `afue` field has no `#[serde(default)]` annotation (line 20), meaning serde requires it in JSON input. But the Rust `Default` impl sets `afue: 0.80` (line 54). This creates an asymmetry: in Rust code, `GasFurnaceConfig::default()` gives AFUE=0.80; when deserializing from JSON, the field is mandatory. The HPXML resolver correctly requires AFUE (`resolve_hvac.rs:640-645`), so this asymmetry is intentional (no silent default from HPXML). However, the mismatch is confusing for direct-API users and is not documented.

**Code Location**:
- `crates/hares-equipment/src/hvac/heating_config.rs:20` (no serde default)
- `heating_config.rs:54` (Rust default is 0.80)

**Root Cause**: The field is declared required for JSON (HPXML data integrity) but the Rust Default needs a value for struct construction ergonomics.

**Impact**: Low — the required-JSON pattern is correct for HPXML ingestion. Confusion risk for Rust API users is minor.

### Finding 7: [Severity: low] No water outlet temperature setpoint on GasBoilerConfig
**Description**: EnergyPlus's `Boiler:HotWater` object has `Water Outlet Upper Temperature Limit` (default 99.9 °C) and `Design Water Outlet Temperature`. HARES's `GasBoilerConfig` stores only `return_temp_c` (return temperature) and has no outlet/supply temperature field. The boiler outlet temperature is needed for hydronic loop temperature control, especially for baseboard radiation systems where the typical outlet setpoint is ~82 °C (180 °F). The hydronic loop may determine outlet temperature from demand, but an upper limit or design outlet temperature is standard.

**Code Location**: `crates/hares-equipment/src/hvac/heating_config.rs:115-157`

**Root Cause**: Outlet temperature is deferred to the hydronic loop model rather than exposed on the boiler config. EnergyPlus places it on the boiler object itself.

**Impact**: Without an upper limit, the boiler could model outlet temperatures that exceed safe operating limits or produce steam in a hot-water loop.

## Summary
- Total findings: 7
- Critical: 1 (no AFUE bounds validation)
- High: 0
- Medium: 2 (contradictory boiler defaults, condensing not auto-inferred on deserialization)
- Low: 4 (missing baseboard fields, no HSPF field, AFUE default asymmetry, no outlet temp setpoint)

## Recommendations
1. **Add AFUE bounds validation** to both `GasFurnaceConfig` and `GasBoilerConfig`. Implement a `validate()` method (like `HeatPumpHeaterConfig::validate()`) that checks `afue` in `[0, 1]` and rejects values outside this range. EnergyPlus's warning-on-efficiency->1.0 pattern is a good reference.
2. **Resolve contradictory boiler defaults**: Either change `condensing` default to `true` (matching the condensing-temperature return default) or change `default_return_temp_c()` to ~70 °C (safe non-condensing return). The boiler comment at `heating_config.rs:150-156` suggests the OCHRE convention of inferring condensing from AFUE > 0.90; if that convention is adopted, the default `condensing=false` + `AFUE=0.80` is consistent, but then `return_temp_c` should be non-condensing (70+ °C).
3. **Auto-infer `condensing` from AFUE in the struct**: Either implement `Deserialize` for `GasBoilerConfig` that sets `condensing = afue > 0.90` when not explicitly provided, or add post-deserialization validation that warns when `afue > 0.90` and `condensing == false`.
4. **Add water outlet temperature setpoint** to `GasBoilerConfig` (e.g., `outlet_temp_c` with a default of 82 °C for baseboard systems, or 99.9 °C upper limit per EnergyPlus convention).

## References / Citations
- EnergyPlus `Coil:Heating:Fuel` Burner Efficiency default: 0.8 (range 0.0–1.0) — `vendors/EnergyPlus/idd/Energy+.idd.in`
- EnergyPlus `Boiler:HotWater` Nominal Thermal Efficiency: required, no default; Water Outlet Upper Temperature Limit default: 99.9 °C — `vendors/EnergyPlus/src/EnergyPlus/Boilers.cc:296`
- EnergyPlus HSPF-to-COP conversion constant: `ConvFromSIToIP = 3.412141633` — `vendors/EnergyPlus/src/EnergyPlus/StandardRatings.hh`
- EnergyPlus DX coil rated heating COP: evaluated at 21.11 °C indoor / 8.33 °C outdoor (AHRI H1 test condition) — `vendors/EnergyPlus/src/EnergyPlus/DXCoils.hh`
- OCHRE condensing boiler convention: `condensing = eir_max < 1/0.9` — referenced in `heating_config.rs:154`
- ASHRAE HVAC Systems and Equipment Ch.13: hydronic flow rate sizing (1 gpm per 10,000 Btu/h) — cited in `default_flow_rate_kg_s()`
- ASHRAE HVAC Systems and Equipment Ch.32: condensing boiler return temperature below ~55 °C flue-gas dewpoint — cited in `default_return_temp_c()`
- AHRI 210/240-2023: HSPF2 test procedure, 6 climate regions, H1/H2/H3 rated conditions — `vendors/EnergyPlus/src/EnergyPlus/StandardRatings.cc`
