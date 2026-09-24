# HPXML HVAC Configuration Wiring Gaps

**Severity**: High
**Priority**: P1
**Status**: Open
**Areas**: hares-io/hpxml, hares-equipment/hvac

## Consolidation Note

This was the original catch-all wiring-gaps ticket. G5 (ChargeDefectRatio) and
G7 (HeatingCapacity17F) have been broken out into dedicated tickets that carry
verified file:line evidence and primary-source citations:
- G5 → ticket 081-charge-defect-ratio-parsed-not-applied.md
- G7 → ticket 080-ashp-heating-capacity-17f-silently-dropped.md

The remaining gaps below (G1, G2, G3, G4, G6) are not covered by any other
ticket and are tracked here.

## Problem

Several HPXML fields that describe real HVAC equipment behavior are not wired
from the HPXML input to HARES's typed config structs. The resolver in
`resolve_hvac.rs` either hardcodes defaults or silently drops the values.
This means HARES ignores user-provided equipment configuration, falling back
to hardcoded defaults that may not match the actual building.

## Current Behavior

### G1: CrankcaseHeaterWatts — NOT WIRED

HPXML `<CrankcaseHeaterWatts>` specifies the parasitic crankcase heater power.
The typed config fields `crankcase_heater_kw` and `crankcase_heater_threshold_c`
EXIST in `CentralAirConditionerConfig` and `RoomAcConfig`, but the resolver in
`resolve_hvac.rs` always sets them to `None`. OCHRE defaults: 50W at 12.78°C
(55°F) for AC/ASHP, 15W at 0°C (32°F) for MSHP.

**Impact**: Continuous parasitic load (~50W when compressor is off and OAT is
below threshold) affects annual energy by 50-100 kWh/year for typical AC.

### G2: DefrostType / DefrostControl — NOT WIRED

HPXML `<DefrostType>` (ReverseCycle/Resistive) and `<DefrostControl>`
(Timed/OnDemand) specify the heat pump defrost strategy. The resolver does
not read these elements. HARES hardcodes `DefrostConfig::on_demand(1.0, 0.0)`
with ReverseCycle at `heater.rs:379`.

**Impact**: Resistive vs ReverseCycle defrost has ~5-15% heating energy impact
in cold climates. Timed vs OnDemand control affects defrost frequency and
annual energy.

### G3: MinimumCapacity / MinimumOutputCapacity — NOT WIRED

HPXML `<MinimumCapacity>` specifies minimum compressor output. The resolver
does not read this element. HARES hardcodes MSHP minimum at 25% of rated
capacity at `heater.rs:522-524`.

**Impact**: Real MSHPs range 20-40% minimum. Incorrect minimum affects
low-load cycling behavior and energy.

### G4: Condensing boiler attribute — NOT WIRED

HPXML does not have an explicit `Condensing` field, but OCHRE infers it from
`AFUE > 0.90`. HARES has no condensing distinction for gas boilers, using
the same EIR curve regardless.

**Impact**: Condensing boilers have 5-10% part-load efficiency gains from
lower return water temperatures. OCHRE uses different biquadratic EIR curves
(`boiler_eff_curve_condensing` vs `boiler_eff_curve_non_condensing`).

### G6: SupplementalHeatingLockoutTemperature — NOT FULLY WIRED

HPXML 4.x distinguishes supplemental lockout from backup lockout. The config
field `max_oat_supplemental_c` exists in `HeatPumpHeaterConfig` but is only
populated from extension fields, not from the standard HPXML element.

**Impact**: Subtle control distinction; low priority.

## Required Behavior

For each gap, the resolver must:
1. Read the HPXML element if present
2. Convert units (°F→°C, Btu/h→W, etc.)
3. Write to the corresponding typed config field
4. Apply OCHRE-compatible defaults when the HPXML element is absent

## Approach

### G1: Wire CrankcaseHeaterWatts (HIGH priority)

In `resolve_hvac.rs`, when building `CentralAirConditionerConfig` or
`RoomAcConfig`:
1. Read `<CrankcaseHeaterWatts>` from HPXML XML
2. Convert W→kW, write to `crankcase_heater_kw`
3. Apply defaults by equipment type when absent:
   - Central AC / ASHP: 0.050 kW (50W) at 12.78°C (55°F)
   - MSHP: 0.015 kW (15W) at 0°C (32°F)
   - Room AC: 0.0 kW (no crankcase heater)
4. Write threshold to `crankcase_heater_threshold_c`

### G2: Wire DefrostType / DefrostControl (HIGH priority)

Requires ticket 014 (typed DefrostConfig) to be completed first.

In `resolve_hvac.rs`, when building `HeatPumpHeaterConfig`:
1. Read `<DefrostType>` → map to `DefrostStrategy` enum
2. Read `<DefrostControl>` → map to `DefrostControl` enum
3. Read `<DefrostTimePeriodFraction>` if present → `defrost_time_fraction`
4. Default: OnDemand / ReverseCycle / 0.058

### G3: Wire MinimumCapacity (MEDIUM priority)

Requires ticket 013 (min_compressor_fraction) to be completed first.

In `resolve_hvac.rs`:
1. Read `<MinimumCapacity>` from HPXML
2. Compute `min_compressor_fraction = MinimumCapacity / HeatingCapacity`
3. Write to `min_compressor_fraction` field

### G4: Add condensing boiler detection (MEDIUM priority)

In `resolve_hvac.rs`, when building boiler config:
1. If `afue > 0.90`, set `condensing: true`
2. This requires adding a `condensing: bool` field to `GasBoilerConfig`
3. Use different efficiency curve coefficients (from OCHRE defaults)

### G6: Wire SupplementalHeatingLockoutTemperature (LOW priority)

In `resolve_hvac.rs`:
1. Read `<SupplementalHeatingLockoutTemperature>` from HPXML
2. Convert °F→°C
3. Write to `max_oat_supplemental_c`

## Definition of Done

- [ ] G1: `CrankcaseHeaterWatts` wired from HPXML to typed config with OCHRE-compatible defaults
- [ ] G1: Crankcase heater power appears in `COMPRESSOR_KW` telemetry when active
- [ ] G2: `DefrostType` and `DefrostControl` wired from HPXML to `DefrostConfig` (depends on ticket 014)
- [ ] G3: `MinimumCapacity` wired from HPXML to `min_compressor_fraction` (depends on ticket 013)
- [ ] G4: Gas boiler detects condensing mode from AFUE > 0.90 and applies appropriate efficiency curves
- [ ] G6: `SupplementalHeatingLockoutTemperature` wired to `max_oat_supplemental_c`
- [ ] All changes have `tracing::debug!` for new field values at equipment init
- [ ] All changes tested with HPXML fixtures that provide these fields

## Verification

```bash
cargo test -p hares-io resolve_hvac   # test HPXML resolver
cargo test -p hares-equipment hvac     # test equipment with wired values
```

Compare HARES annual energy against OCHRE for buildings with known crankcase
heater and defrost configurations.

## References

- HPXML Specification v4.2: https://github.com/hpxmlwg/hpxml/releases/tag/v4.2
- HPXML Schema: https://github.com/hpxmlwg/hpxml/tree/master/schemas
- OCHRE HPXML parser: `vendors/OCHRE/ochre/utils/hpxml.py`
- HARES resolver: `crates/hares-io/src/hpxml/resolve_hvac.rs`
- OCHRE HVAC defaults: `vendors/OCHRE/ochre/defaults/`

## Related Tickets

- 010 (default biquadratic curves — coordinates with defaults loading)
- 013 (min_compressor_fraction — provides the config field G3 writes to)
- 014 (defrost typed config — provides the DefrostConfig struct G2 writes to)
- 015 (ER on/off modeling — coordinates with backup heat wiring)
- 080 (HeatingCapacity17F — extracted from G7 of this ticket)
- 081 (ChargeDefectRatio — extracted from G5 of this ticket)

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-22

### Code Confirmation

- [x] Referenced line numbers still match (with minor shifts noted below)
- [x] Described logic matches current implementation
- [x] OCHRE cross-check result: **matches** — OCHRE HVAC.py confirms all defaults; one claim (defrost 0.058 time fraction) requires qualification (see below)
- [x] EnergyPlus cross-check result: **matches** — EnergyPlus I/O Reference confirms defrost time period fraction default 0.058333, defrost strategy/control field names, and crankcase heater field structure

#### Line-number notes (audited 2026-05-22)

| Ticket claim | Actual location |
|---|---|
| `heater.rs:379` — hardcoded `DefrostConfig::on_demand(1.0, 0.0)` | **Confirmed at line 379** exactly |
| `heater.rs:522-524` — MSHP 25% hardcode | **Confirmed at lines 517-524** (block starts at 517) |
| `resolve_hvac.rs` crankcase fields `None` | **Confirmed at lines 833-835** (Central AC) and **893-895** (Room AC) |
| `resolve_hvac.rs:1022` — `max_oat_supplemental_c` only from extension | **Confirmed at line 1022**: reads from `params` map, but `SupplementalHeatingLockoutTemperature` is never inserted into `params` anywhere |

### Web-Verified Citations

#### Citation 1 — HPXML elements CrankcaseHeaterWatts, DefrostType, DefrostControl, MinimumCapacity, SupplementalHeatingLockoutTemperature

- **Citation**: Ticket implies these are standard HPXML 4.x elements under `HeatPump` or `CoolingSystem`
- **Source found**: GitHub search `hpxmlwg/hpxml` repository (schema, examples, all files); HPXML Data Dictionary v4.0.0, v4.1.0, v4.2.0 at `hpxml.nlr.gov`; OpenStudio-HPXML Workflow Inputs docs
- **Findings**:
  - `CrankcaseHeaterWatts` — **0 results** in `hpxmlwg/hpxml` GitHub search. HTTP 404 at `hpxml.nlr.gov/datadictionary/4.2.0/Building/BuildingDetails/Systems/HVAC/HVACPlant/HeatPump/CrankcaseHeaterWatts`. The element does **not** exist as a standard HPXML schema element. OpenStudio-HPXML uses `extension/CrankcaseHeaterPowerWatts` (extension namespace).
  - `DefrostType` / `DefrostControl` — **0 results** in `hpxmlwg/hpxml` GitHub search. These are **not** standard HPXML schema elements. EnergyPlus uses these field names for `Coil:Heating:DX:SingleSpeed` objects internally, but HPXML does not expose them.
  - `MinimumCapacity` — **0 results** in `hpxmlwg/hpxml` GitHub search. Not a standard HPXML element.
  - `SupplementalHeatingLockoutTemperature` — **0 results** in `hpxmlwg/hpxml` GitHub search. HTTP 404 at HPXML data dictionary v4.0.0, v4.1.0, v4.2.0. Not a standard HPXML element. The HPXML schema has `CompressorLockoutTemperature` and `BackupHeatingLockoutTemperature` (added in PR #309, merged June 2022), but no supplemental lockout temperature element.
- **Verdict**: **Partially correct** — the ticket correctly identifies that these fields are *not wired*, but the framing "HPXML `<CrankcaseHeaterWatts>`", "HPXML `<DefrostType>`", etc. implies these are standard HPXML schema elements. They are **not**. They either live in the HPXML extension namespace (crankcase heater) or do not exist in HPXML at all (DefrostType, DefrostControl, MinimumCapacity, SupplementalHeatingLockoutTemperature). The gap is real, but the mechanism for "wiring from HPXML" may need to be from extension fields or from defaults tables rather than standard elements.

#### Citation 2 — OCHRE crankcase heater defaults (50 W at 12.78°C for AC/ASHP; 15 W at 0°C for MSHP)

- **Citation**: "OCHRE defaults: 50W at 12.78°C (55°F) for AC/ASHP, 15W at 0°C (32°F) for MSHP"
- **Source found**: `vendors/OCHRE/ochre/Equipment/HVAC.py` (OCHRE submodule in repo)
- **Quoted passage** (OCHRE HVAC.py, AirConditioner class, lines ~1059-1060):
  ```python
  crankcase_kw = 0.050  # 50W crankcase for AC and ASHP
  crankcase_temp = convert(55, "degF", "degC")  # = 12.78°C
  ```
  And (MinisplitAHSPCooler class, lines ~1108-1109):
  ```python
  crankcase_kw = 0.015  # 15W
  crankcase_temp = convert(32, "degF", "degC")  # = 0°C
  ```
  Activation logic (lines ~1070-1077): crankcase heater activates when `mode == "Off"` and `OAT < crankcase_temp`.
- **Verdict**: **Confirmed** exactly.

#### Citation 3 — OCHRE defrost defaults (OnDemand / ReverseCycle / 0.058)

- **Citation**: "Default: OnDemand / ReverseCycle / 0.058"
- **Source found**: `vendors/OCHRE/ochre/Equipment/HVAC.py`; EnergyPlus I/O Reference via bigladdersoftware.com (GroupHeatingCoolingCoils, VRF Equipment)
- **Findings**:
  - OCHRE defrost trigger: temperature-based at 4.4445°C (40°F) — **on-demand** strategy confirmed.
  - OCHRE defrost strategy: **reverse-cycle** (heating cycle reversal) — confirmed.
  - OCHRE **does not use a fixed 0.058 fraction**. Instead, it dynamically calculates `defrost_time_frac = 1.0 / (1 + 0.01446 / delta_omega_coil_out)`, a humidity-ratio–dependent formula. This is an on-demand dynamic calculation, not a timed 0.058.
  - The **0.058333** value (= 3.5 min / 60 min) is the EnergyPlus **default for timed defrost control** on `Coil:Heating:DX:SingleSpeed` (confirmed: "if the defrost cycle is active for 3.5 minutes for every 60 minutes of compressor runtime, then the user should enter 3.5/60 = 0.058333. If left blank, the default value is 0.058333").
  - HARES hardcodes `DefrostConfig::on_demand(1.0, 0.0)` at `heater.rs:379`. The `1.0` and `0.0` arguments are not the 0.058 EnergyPlus default; the 0.058 is only relevant for timed defrost.
- **Verdict**: **Partially correct** — the on-demand and reverse-cycle defaults are confirmed. The "0.058" default is the EnergyPlus **timed-defrost** default, not used by OCHRE's on-demand model. The ticket's "Default: OnDemand / ReverseCycle / 0.058" conflates EnergyPlus timed-defrost input default with OCHRE's on-demand dynamic formula. This is a minor citation inaccuracy but does not affect the core gap finding.

#### Citation 4 — OCHRE condensing boiler detection (AFUE > 0.90, different biquadratic curves)

- **Citation**: "OCHRE infers it from `AFUE > 0.90`. OCHRE uses different biquadratic EIR curves (`boiler_eff_curve_condensing` vs `boiler_eff_curve_non_condensing`)"
- **Source found**: `vendors/OCHRE/ochre/Equipment/HVAC.py` (GasBoiler class, lines ~687-732)
- **Quoted passage**:
  ```python
  self.condensing = self.eir_max < 1 / 0.9  # Condensing if efficiency (AFUE) > 90%
  # Condensing boiler: 6-coefficient curve
  if self.condensing:
      self.outlet_temp = 65.56  # 150°F
      self.efficiency_coeff = np.array(
          [1.058343061, -0.052650153, -0.0087272, -0.001742217, 0.00000333715, 0.000513723])
  # Non-condensing: 10-coefficient curve
  else:
      self.outlet_temp = 82.22  # 180°F
      self.efficiency_coeff = np.array(
          [1.111720116, 0.078614078, -0.400425756, 0, -0.000156783, ...])
  ```
- **Verdict**: **Confirmed** — AFUE > 0.90 detection and separate efficiency curves are real. Minor naming inaccuracy: the ticket calls them `boiler_eff_curve_condensing` / `boiler_eff_curve_non_condensing`, but in OCHRE they are inline coefficient arrays on `self.efficiency_coeff`, not separately named. The functional claim is correct.

#### Citation 5 — HPXML Specification v4.2 reference URL

- **Citation**: "HPXML Specification v4.2: https://github.com/hpxmlwg/hpxml/releases/tag/v4.2"
- **Source found**: GitHub releases page for `hpxmlwg/hpxml`
- **Verdict**: **Plausible but unverified by direct fetch** — the URL format is correct and the repo exists. The latest confirmed release via BPI news is HPXML v4.1 (April 2025). Whether v4.2 exists as a released tag could not be confirmed via web search. The HPXML Data Dictionary at `hpxml.nlr.gov` confirmed v4.2.0 exists (schema navigation worked), so the reference is likely valid.

### Legitimacy

- **Verdict**: **Partially Legitimate**
- **Rationale**: The core gap claims (G1–G4, G6) are all **real and confirmed** by direct code inspection: `crankcase_heater_kw` is hardcoded to `None` (lines 833-835, 893-895 of `resolve_hvac.rs`); defrost is hardcoded at `heater.rs:379`; MinimumCapacity is not read; GasBoilerConfig lacks a `condensing` field; and `SupplementalHeatingLockoutTemperature` is not parsed from standard HPXML elements. The bugs are genuine and represent meaningful energy modeling errors. However, several details in the ticket require correction: (1) `CrankcaseHeaterWatts`, `DefrostType`, `DefrostControl`, `MinimumCapacity`, and `SupplementalHeatingLockoutTemperature` are **not** standard HPXML schema elements — `CrankcaseHeaterWatts` lives in the extension namespace; the rest have no HPXML element at all. The ticket's "Required Behavior — Read the HPXML element if present" is therefore aspirational for some gaps. (2) The "0.058" defrost fraction default is the EnergyPlus timed-defrost input default, not the OCHRE on-demand default; OCHRE calculates defrost time dynamically. The G1 crankcase heater and G4 condensing boiler gaps have the highest quantitative impact and are most actionable.

### Proposed Fix Summary

**G1 (HIGH)**: In `resolve_hvac.rs`, when building `CentralAirConditionerConfig`:
  - Check for `extension/CrankcaseHeaterPowerWatts` (OpenStudio-HPXML extension name) in the HPXML extension block
  - If absent, apply OCHRE-compatible defaults: 0.050 kW at 12.78°C for central AC/ASHP, 0.015 kW at 0°C for MSHP, 0.0 kW for Room AC
  - Do NOT implement as "read `<CrankcaseHeaterWatts>` standard element" — that element does not exist in HPXML schema

**G2 (HIGH, depends on ticket 014)**: Wire defrost strategy/control from extension fields or defaults. Note the 0.058 timed-defrost default is EnergyPlus-specific; OCHRE uses on-demand dynamic calculation. Confirm which model HARES intends before choosing a default.

**G3 (MEDIUM, depends on ticket 013)**: No standard HPXML element exists for MinimumCapacity. Source the minimum fraction from OCHRE defaults tables or equipment-specific defaults rather than an HPXML parse.

**G4 (MEDIUM)**: Add `condensing: bool` field to `GasBoilerConfig`. In `try_build_gas_boiler_config`, add `condensing: afue > 0.90`. Update the efficiency curve selection in the equipment model to use OCHRE's 6-coeff (condensing) vs 10-coeff (non-condensing) curve.

**G6 (LOW)**: The HPXML element `SupplementalHeatingLockoutTemperature` does not exist in the schema. The wiring must come from an extension field or remain extension-only. Low priority; the existing `max_oat_supplemental_c` field is accessible via the extension mechanism already present in the resolver.

### Test Written

- **File**: `crates/hares-io/tests/ticket_023_hvac_wiring_regressions.rs`
- **What it tests**:
  - `g1_crankcase_heater_watts_wired_from_hpxml` — **FAILS** (bug): typed config must carry `crankcase_heater_kw = Some(0.075)` when `CrankcaseHeaterWatts=75` is in HPXML extension; currently `None`
  - `g1_crankcase_heater_absent_uses_ochre_default` — **FAILS** (bug): absent crankcase config must default to OCHRE's 50 W / 12.78°C; currently `None`
  - `g1_crankcase_heater_kw_is_currently_none_documents_gap` — **PASSES** (gap confirmed): verifies that the current output is `None`, will flip to failing when the fix lands
  - `g3_resolver_does_not_wire_min_compressor_fraction` — **PASSES** (gap confirmed): verifies `min_compressor_fraction` is absent from resolver output
  - `g4_gas_boiler_config_has_no_condensing_field` — **PASSES** (gap confirmed): verifies no `condensing` key in params, and AFUE is correctly carried
  - `g6_supplemental_heating_lockout_temperature_not_read_from_standard_element` — **PASSES** (gap confirmed): verifies `max_oat_supplemental_c` is `None` when `SupplementalHeatingLockoutTemperature` is in standard HPXML position
