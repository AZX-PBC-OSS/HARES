# HPXML HVAC Configuration Wiring Gaps

**Severity**: High
**Priority**: P1
**Status**: Open
**Areas**: hares-io/hpxml, hares-equipment/hvac

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

### G5: ChargeDefectRatio — READ BUT DROPPED

HPXML `<extension/ChargeDefectRatio>` is read into the raw params map by
`resolve_hvac.rs` but never consumed by any equipment model. The value is
silently dropped.

**Impact**: Charge degradation can reduce cooling capacity 5-20% and increase
power consumption. OCHRE applies a charge defect correction to capacity and
EIR.

### G6: SupplementalHeatingLockoutTemperature — NOT FULLY WIRED

HPXML 4.x distinguishes supplemental lockout from backup lockout. The config
field `max_oat_supplemental_c` exists in `HeatPumpHeaterConfig` but is only
populated from extension fields, not from the standard HPXML element.

**Impact**: Subtle control distinction; low priority.

### G7: HeatingCapacity at 17°F (H3 test) — NOT WIRED

HPXML can specify low-temperature heating capacity separately from rated
(47°F) capacity. HARES only reads rated capacity. Low-OAT capacity is derived
from biquadratic curves, but without explicit H3 data the curve extrapolation
may be inaccurate.

**Impact**: Cold-climate heating capacity accuracy depends on curve quality.

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

### G5: Consume charge_defect_ratio (MEDIUM priority)

In `air_conditioner.rs` or `coil_physics.rs`:
1. Read `charge_defect_ratio` from raw params
2. Apply capacity and EIR corrections per OCHRE model
3. OCHRE applies: `capacity *= 1 - 0.05 * defect_ratio` and
   `eir *= 1 + 0.05 * defect_ratio` (approximate — verify against OCHRE source)

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
- [ ] G5: `charge_defect_ratio` consumed by cooling equipment model with capacity/EIR corrections
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
