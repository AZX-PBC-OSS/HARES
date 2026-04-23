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
