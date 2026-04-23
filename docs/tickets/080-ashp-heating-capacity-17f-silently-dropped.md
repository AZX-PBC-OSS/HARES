# ASHP HeatingCapacity17F Silently Dropped — Cold-Climate Biquadratic Not Anchored

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-io, hares-equipment

## Problem

HPXML `<HeatPump>` elements may contain `<HeatingCapacity17F>` (capacity at 17°F /
-8.33°C), which is the AHRI 210/240 low-ambient rating point for ASHPs. This value
anchors the heating capacity-versus-temperature curve at the design cold condition.
HARES never reads `HeatingCapacity17F`; the field is silently dropped. The
biquadratic capacity-vs-temperature curve is therefore fit entirely from the rated
(47°F) condition and the curve defaults, producing unreliable extrapolation to
sub-freezing outdoor temperatures.

For cold-climate heat pumps (e.g., rated at 100% capacity at 5°F), ignoring the
17°F point can overstate or understate heating capacity at the temperatures where
backup ER activation is decided, directly biasing annual ER electricity consumption.

## Evidence

HPXML sample (`base-hvac-air-to-air-heat-pump-1-speed-heating-capacity-17f.xml:330`):

```xml
<HeatingCapacity17F>21600.0</HeatingCapacity17F>
```

No code in `resolve_hvac.rs` reads `HeatingCapacity17F`. The `HeatPump` loop
at lines 1444–1615 does not call `child_f64(heat_pump, "HeatingCapacity17F")`.

`HeatPumpHeaterConfig` has no field for the 17°F capacity data point.

## OCHRE Cross-check

OCHRE `hpxml.py` reads `HeatingCapacity17F` and stores it as a ratio to the 47°F
rated capacity. This ratio is used to validate or adjust the biquadratic curve shape
so that the modelled capacity at -8.33°C matches the manufacturer specification.

## Required Behavior

1. Read `HeatingCapacity17F` from the HPXML `<HeatPump>` element via `child_f64(heat_pump, "HeatingCapacity17F")`.
2. Convert from BTU/h to W using `conv::power_btu_h_to_w(cap_btu)` and compute the ratio:
   `capacity_ratio_17f = capacity_17f_w / heating_capacity_w`
3. Store in `HeatPumpHeaterConfig` as `capacity_ratio_at_17f: Option<f64>`.
4. In the heater `init` path, if this ratio is present, validate that the loaded
   biquadratic curve evaluated at (-8.33°C outdoor, indoor rated WB) yields
   approximately this ratio. If the discrepancy exceeds 10%, log a `tracing::warn!`. Future
   work: use the ratio to scale the curve or select an alternative curve set.

## Approach

- Parse change: `resolve_hvac.rs:1480` area — add after the `BackupHeatingCapacity` block:
  ```
  if let Some(cap_btu) = child_f64(heat_pump, "HeatingCapacity17F") {
      let cap_17f_w = conv::power_btu_h_to_w(cap_btu);
      if let Some(cap_w) = params.get("heating_capacity_w").and_then(Value::as_f64) {
          params.insert("capacity_ratio_at_17f".to_string(), json!(cap_17f_w / cap_w));
      }
  }
  ```
- Config change: add `capacity_ratio_at_17f: Option<f64>` to `HeatPumpHeaterConfig` in `hares-equipment`.
- Equipment change: in `heater.rs` `init_from_typed`, read `capacity_ratio_at_17f` and emit a warning if biquadratic disagrees by > 10%.

## Citation

- AHRI Standard 210/240-2023 §6.1.3: "Heating Rating at 17°F" is a required test
  point for ASHP performance characterization (applies to all split-system ASHPs)
- EnergyPlus Engineering Reference §16.1.3: `RatedHeatingCOP` and `HeatingCapacity17F`
  are both used to characterise the heating coil capacity-vs-temperature relationship
- HPXML 4.x schema §HeatPump/HeatingCapacity17F (hpxml.nrel.gov)

## Annual kWh Impact Rank

**Medium.** For climates where OAT routinely drops below 17°F, the capacity
extrapolation error propagates to more frequent ER backup activation decisions.
Annual ER energy error can reach 15–30% in cold climates (Climate Zones 6–7).

## Definition of Done

- [ ] `HeatingCapacity17F` read from HPXML and converted to W (`resolve_hvac.rs` around line 1480)
- [ ] `capacity_ratio_at_17f: Option<f64>` field added to `HeatPumpHeaterConfig` in `hares-equipment/src/hvac/heat_pump_config.rs`
- [ ] Resolver computes `capacity_17f_w / heating_capacity_w` and stores result in params
- [ ] `try_build_heat_pump_heater_config` reads `capacity_ratio_at_17f` from params into typed config
- [ ] `heater.rs` `init_from_typed` emits `tracing::warn!` when ratio deviates > 10% from biquadratic at (-8.33°C, rated WB)
- [ ] Test: given `<HeatingCapacity17F>21600.0</HeatingCapacity17F>` and `<HeatingCapacity>36000.0</HeatingCapacity>` → `capacity_ratio_at_17f = Some(0.600)` (within 0.01)
- [ ] Test: HPXML without `HeatingCapacity17F` → field is `None`, no error or warning emitted

## Verification

```bash
cargo test -p hares-io -- resolve_hvac::tests::capacity_17f
cargo test -p hares-equipment -- heat_pump::heater
```

Reference HPXML sample: `base-hvac-air-to-air-heat-pump-1-speed-heating-capacity-17f.xml:330` (`<HeatingCapacity17F>21600.0</HeatingCapacity17F>`).
