# Fan Heat Diagnostic Category

**Severity**: Low
**Priority**: P3
**Status**: Open
**Areas**: hares-equipment, hares-types

## Problem

During cooling mode, fan motor heat is added to `sensible_gain_w` and the entire sum is categorized as `ThermalCategory::HvacCooling`. This means the `hvac_cooling_w` diagnostic value is less negative than the true coil cooling output because it includes the positive fan heat offset. Downstream diagnostics comparing "coil capacity" vs "net zone cooling" see a mismatch: the reported cooling appears weaker than the equipment's rated capacity because fan heat is embedded in the same category.

This is physically correct — fan heat IS in the zone and DOES offset cooling. The problem is purely observational: the current category system cannot distinguish "coil sensible cooling" from "fan heat added during cooling" in the same category.

## Current Behavior

### IdealHvac

In `ideal_hvac.rs:552-571`:

```rust
let (sensible_w, latent_w, category) = if capacity_w > 0.0 {
    (capacity_w, 0.0, ThermalCategory::HvacHeating)
} else if capacity_w < 0.0 {
    let sensible = capacity_w * self.shr;
    let latent = capacity_w * (1.0 - self.shr);
    (sensible, latent, ThermalCategory::HvacCooling)
} else {
    (0.0, 0.0, ThermalCategory::HvacHeating)
};
// Fan motor heat always enters the zone as sensible gain (positive
// in both heating and cooling modes).
ports.accumulate(&PortContribution::Thermal {
    zone: self.zone_id,
    sensible_gain_w: sensible_w + fan_power_w,  // ← fan heat folded in
    radiant_gain_w: 0.0,
    latent_gain_w: latent_w,
    category,
})?;
```

At line 567, `sensible_gain_w: sensible_w + fan_power_w` combines the coil's negative sensible cooling with the positive fan heat into a single value under `ThermalCategory::HvacCooling`.

Example: If the coil delivers -5000 W sensible cooling and the fan adds +200 W, the reported `sensible_gain_w` for `HvacCooling` is -4800 W. The coil's actual output is -5000 W, but there's no way to recover that from the accumulator.

### Air Conditioner

In `air_conditioner.rs:800-806`:

```rust
let fan_heat_w = fan_kw * 1000.0;
self.hvac.write_zone_thermal_contributions(
    ports,
    -sensible_cooling_w + fan_heat_w,  // ← fan heat folded in
    -latent_cooling_w,
    ThermalCategory::HvacCooling,
)?;
```

Same pattern: `-sensible_cooling_w + fan_heat_w` is reported as a single `HvacCooling` sensible value.

### Heat Pump Heater

In `heater.rs:696-701`:

```rust
if step.thermal_output_w > 0.0 {
    self.hvac.write_zone_thermal_contributions(
        ports,
        step.thermal_output_w,  // includes fan_power_w
        0.0,
        ThermalCategory::HvacHeating,
    )?;
}
```

For heating, `thermal_output_w` already includes `fan_power_w` (line 1065: `thermal_output_w = hp_capacity_w + er_capacity_w + fan_power_w`). In heating mode this is less of a concern because fan heat ADDS to the desired heating output rather than opposing it. The diagnostic mismatch is primarily a cooling-mode issue.

Heating-mode fan heat embedded in `thermal_output_w` is explicitly out of scope for this ticket. Once the cooling-side separation pattern is established, the heating-side split should be addressed in a follow-up ticket for consistency. No backward-compat shim — just a deferred consistency item.

### ThermalCategory

In `hares-types/src/ports.rs:17-29`:

```rust
pub enum ThermalCategory {
    HvacHeating,     // 0
    HvacCooling,     // 1
    InternalGain,    // 2
    JacketLoss,      // 3
    DuctLoss,        // 4
}
```

No sub-category for fan heat exists.

### OCHRE comparison

OCHRE adds fan power to delivered heat with SHR=1 for fan (same approach). OCHRE reports `delivered_heat` as the net value including fan heat. OCHRE does NOT separate fan heat in its output — it only reports the net delivered value.

### EnergyPlus comparison

EnergyPlus separates "coil sensible cooling rate" from "fan heat addition to zone" in its tabular output (Equipment Summary table). The physics simulation treats them the same (fan heat offsets cooling), but for diagnosing equipment performance, E+ provides both values.

## Required Behavior

**Option (b) is preferred**: Keep the physics identical (fan heat IS in the zone, combined with cooling in the thermal accumulator) but add a telemetry field `coil_cooling_w` (without fan heat) alongside the existing `sensible_cooling_w` (with fan heat) so diagnostics can distinguish the two.

This does NOT change the thermal coupling — the `ThermalAccumulator` still receives `sensible_gain_w = coil_sensible + fan_heat` under `ThermalCategory::HvacCooling`. The change is purely in the equipment's telemetry output, adding an extra field for observability.

## Approach

### Step 1: Add telemetry keys

In `hares-types/src/telemetry_keys.rs` (or wherever telemetry keys are defined), add:

```rust
pub const COIL_SENSIBLE_COOLING_W: &str = "coil_sensible_cooling_w";
pub const COIL_LATENT_COOLING_W: &str = "coil_latent_cooling_w";
pub const FAN_HEAT_W: &str = "fan_heat_w";
```

### Step 2: Report coil-level values in AirConditioner telemetry

In `air_conditioner.rs`, after line 843 where `SENSIBLE_COOLING_W` is set:

```rust
// Existing: reports NET zone delivery (post-DSE, coil output * dse).
// sensible_cooling_w here is the coil gross output (positive magnitude).
self.telemetry.set(tk::SENSIBLE_COOLING_W, sensible_cooling_w * dse);

// New: gross coil output BEFORE DSE (at the coil, upstream of duct losses).
self.telemetry.set(tk::COIL_SENSIBLE_COOLING_W, sensible_cooling_w);
self.telemetry.set(tk::COIL_LATENT_COOLING_W, latent_cooling_w);
self.telemetry.set(tk::FAN_HEAT_W, fan_heat_w);
```

Where `fan_heat_w = fan_kw * 1000.0` (already computed at line 800).

Semantic distinction:
- `COIL_SENSIBLE_COOLING_W`: gross output at the coil, before duct distribution loss (pre-DSE). This is the value the compressor actually delivers.
- `SENSIBLE_COOLING_W`: net delivery to the conditioned zone (post-DSE = coil × DSE). This equals `COIL_SENSIBLE_COOLING_W * dse`.
- `FAN_HEAT_W`: supply fan waste heat added to the zone (positive). Per EnergyPlus Engineering Reference §16.7 "Fan Heat Modeling", the supply fan heat added to the supply air stream is the standard convention for this quantity.

The observability this enables: `SENSIBLE_COOLING_W - COIL_SENSIBLE_COOLING_W = -(coil * (1 - dse))`, i.e., the duct loss (sensible). Without `COIL_SENSIBLE_COOLING_W`, duct losses are invisible in telemetry.

The invariants that must hold:
```
SENSIBLE_COOLING_W == COIL_SENSIBLE_COOLING_W * dse
COIL_SENSIBLE_COOLING_W * dse + FAN_HEAT_W == |zone_sensible_hvac_cooling|
```

Where `zone_sensible_hvac_cooling` is the (negative) sensible gain reported to `ThermalAccumulator` under `HvacCooling`.

### Step 3: Report coil-level values in IdealHvac telemetry

In `ideal_hvac.rs`, after line 589:

```rust
self.telemetry.set(tk::THERMAL_OUTPUT_W, capacity_w);
// New: separate fan heat from coil output
self.telemetry.set(tk::COIL_SENSIBLE_COOLING_W, capacity_w * self.shr);
self.telemetry.set(tk::FAN_HEAT_W, fan_power_w);
```

### Step 4: Add telemetry field descriptors

In `ideal_hvac.rs` and `air_conditioner.rs` telemetry field lists, add entries for the new keys with appropriate descriptions and units.

### Step 5: Update ThermalAccumulator documentation

In `hares-types/src/ports.rs:155-163`, update the `ThermalAccumulator` doc comment to clarify:

```
/// `sensible_by_category[HvacCooling]` includes fan heat offset.
/// For coil-only cooling output, use equipment telemetry fields
/// `coil_sensible_cooling_w` and `coil_latent_cooling_w`.
```

### Step 6: Verify no physics change

The `PortContribution::Thermal` writes must remain unchanged. Fan heat still enters `sensible_gain_w` under `ThermalCategory::HvacCooling`. The only change is additional telemetry fields for observability.

## Definition of Done

- [ ] Telemetry keys `COIL_SENSIBLE_COOLING_W`, `COIL_LATENT_COOLING_W`, `FAN_HEAT_W` exist
- [ ] `AirConditioner` telemetry reports coil-level cooling and fan heat separately
- [ ] `IdealHvac` telemetry reports coil-level cooling and fan heat separately
- [ ] `ThermalAccumulator` doc comment explains that `HvacCooling` includes fan heat
- [ ] No changes to `PortContribution::Thermal` or `ThermalAccumulator` fields
- [ ] No changes to the thermal coupling physics
- [ ] All existing tests pass
- [ ] New test: `SENSIBLE_COOLING_W == COIL_SENSIBLE_COOLING_W * dse` for AC
- [ ] New test: `COIL_SENSIBLE_COOLING_W * dse + FAN_HEAT_W == |accumulator.sensible_for_category(HvacCooling)|` for AC
- [ ] New test: same invariants for IdealHvac

## Verification

1. **Telemetry invariant test**: For a cooling step with `dse` applied, verify all of:
   ```
   telemetry[SENSIBLE_COOLING_W] == telemetry[COIL_SENSIBLE_COOLING_W] * dse
   telemetry[COIL_SENSIBLE_COOLING_W] * dse + telemetry[FAN_HEAT_W] == |accumulator.sensible_for_category(HvacCooling)|
   ```
   Both must hold within floating-point tolerance. `COIL_SENSIBLE_COOLING_W` is the gross coil output before DSE; `SENSIBLE_COOLING_W` is the zone delivery after DSE. The difference `COIL_SENSIBLE_COOLING_W - SENSIBLE_COOLING_W` is the observable duct loss (sensible).

2. **Numerical example**: Configure IdealHvac with `cooling_capacity_w = 8000`, `shr = 0.75`, `rated_fan_power_w = 200`. During cooling:
   - `sensible_w = -8000 * 0.75 = -6000 W`
   - `latent_w = -8000 * 0.25 = -2000 W`
   - `fan_power_w = +200 W`
   - `sensible_gain_w reported = -6000 + 200 = -5800 W`
   - `coil_sensible_cooling_w = -6000 W`
   - `fan_heat_w = +200 W`
   Verify: `-6000 + 200 = -5800` ✓

3. **AC numerical example**: With `sensible_cooling_w = 4000` (coil gross output, positive magnitude), `fan_kw = 0.15`, `dse = 0.85`:
   - `fan_heat_w = 150 W`
   - `COIL_SENSIBLE_COOLING_W = 4000 W` (pre-DSE)
   - `SENSIBLE_COOLING_W = 4000 * 0.85 = 3400 W` (post-DSE zone delivery)
   - Net sensible to zone (in ThermalAccumulator, negative = cooling): `-4000 * 0.85 + 150 = -3250 W`
   Verify: `COIL_SENSIBLE_COOLING_W * dse + FAN_HEAT_W = 3400 - 150 ... wait` — sign convention: `sensible_cooling_w` is a positive magnitude, cooling contribution to the zone is `-sensible_cooling_w * dse + fan_heat_w = -3250 W`. Check: `COIL_SENSIBLE_COOLING_W * dse == SENSIBLE_COOLING_W`: `4000 * 0.85 = 3400` ✓

4. **No physics regression**: The `ThermalAccumulator` values must be identical before and after this change. Run the existing `mixed_category_contributions_route_to_correct_slots` test in `ports.rs:552-631` — must pass unchanged.

## References

- EnergyPlus Engineering Reference §16.7 "Fan Heat Modeling": supply fan heat added to the supply air stream is treated as a positive sensible load on the zone; reported separately from coil cooling output in E+ tabular reports (`Coil Sensible Cooling Rate` vs `Fan Electric Energy`).
- OCHRE `HVAC.py` line 543: `delivered_heat = heat_gain * shr + fan_power` — same physics (fan heat offsets cooling), no separate reporting
- `hares-equipment/src/hvac/ideal_hvac.rs:562-571`: Fan heat added to sensible_gain_w
- `hares-equipment/src/hvac/air_conditioner.rs:800-806`: Fan heat added to sensible
- `hares-types/src/ports.rs:17-29`: ThermalCategory enum (no fan heat sub-category)

## Related Tickets

- 001-unify-hfg-add-humidity-port.md (latency cooling split also depends on correct sensible/latent separation)
- 002-ideal-hvac-biquadratic-fallback.md (capacity curves affect the coil cooling values reported here)

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-20

### Code Confirmation

- [x] Referenced line numbers still match (with minor shifts noted below)
- [x] Described logic matches current implementation
- [x] OCHRE cross-check result: **matches** — `vendors/OCHRE/ochre/Equipment/HVAC.py:543` confirms `self.delivered_heat = heat_gain * self.shr + self.fan_power  # SHR=1 for fan`. Fan power is reported separately only at verbosity ≥ 7 (`Fan Power (kW)` key at line 593); the `delivered_heat` output (line 582) always includes it combined.
- [x] EnergyPlus cross-check result: **partially matches** — see Web-Verified Citations below. E+ does expose a separate `Cooling Coil Sensible Cooling Rate [W]` output variable (coil-only, no fan heat) and a separate `Fan Electric Power [W]` variable, consistent with the ticket's claim that E+ separates these diagnostically. However, the specific section reference "§16.7 Fan Heat Modeling" does not exist.

**Line number corrections**:
- `ideal_hvac.rs:552-571` ticket description: actual code at lines 551–571 (off by one in the `if` guard, functionally equivalent). The critical `sensible_gain_w: sensible_w + fan_power_w` is at line **567** ✓.
- `ideal_hvac.rs:562-571` (Step 3 of approach): code block starts at 552, not 562. The telemetry `tk::THERMAL_OUTPUT_W` is set at line **589** ✓.
- `air_conditioner.rs:800-806`: **confirmed** — `fan_heat_w = fan_kw * 1000.0` at line 800; `write_zone_thermal_contributions` call at lines 801–806 ✓.
- `hares-types/src/ports.rs:17-29`: **confirmed** — `ThermalCategory` enum at lines 17–29, five variants, no fan heat sub-category ✓.

**Architecture note**: `write_zone_thermal_contributions` receives the pre-DSE gross value. DSE is embedded in `zone_heat_fractions` (conditioned zone fraction = `duct_dse`, set in `update_zone_heat_fractions()`). This means the conditioned zone port receives `(-sensible_cooling_w + fan_heat_w) * duct_dse`, not `(-sensible_cooling_w + fan_heat_w)` directly. This is correct physics but affects the second invariant formula stated in the ticket (see Invariant Correction below).

### Web-Verified Citations

**Citation 1: EnergyPlus Engineering Reference §16.7 "Fan Heat Modeling"**

- **Source searched**: bigladdersoftware.com EnergyPlus Engineering Reference (versions 8.0–24.1), EnergyPlus 9.0 Air System Fans page, EnergyPlus 9.4 Table of Contents
- **Quoted passage from Air System Fans (EnergyPlus 9.4)**: "The user also needs to specify the fraction of the fan motor's waste heat that will enter the air stream (usually 0 or 1). If the fan is indoors, the name of a Zone and a fraction for the split between thermal radiation and convection can be entered so that the portion of fan motor waste heat that does not enter the air stream can be added to the thermal zone surrounding the fan."
- **Quoted passage from I/O Reference cooling coil output variables**: "Cooling Coil Sensible Cooling Rate is the Rate of Sensible heat transfer taking place in the coil at the operating conditions" — this is the coil-only value, separately from fan electrical output.
- **Verdict**: **Incorrect section reference**. The EnergyPlus Engineering Reference does not have a section numbered "§16.7" in any version reviewed (8.0 through 24.1). The document uses unnumbered subsections (e.g., "Air System Fans > Model > Simulation > Simple (Single Speed) Fan Model"). There is no section titled "Fan Heat Modeling" at any level. The *substance* of the citation is correct — E+ does model supply fan heat as a positive sensible zone load and does expose `Cooling Coil Sensible Cooling Rate` and `Fan Electric Power` as separate diagnostic variables — but the section number "§16.7" is fabricated and does not appear in EnergyPlus documentation.

**Citation 2: OCHRE `HVAC.py` line 543**

- **Source found**: `vendors/OCHRE/ochre/Equipment/HVAC.py` (read directly from submodule)
- **Quoted passage**: `self.delivered_heat = heat_gain * self.shr + self.fan_power  # SHR=1 for fan` (line 543). Context: `heat_gain = self.hvac_mult * self.capacity` (line 540); `self.sensible_gain = self.delivered_heat` (line 544); `self.latent_gain = heat_gain * (1 - self.shr)  # no latent gains from fan` (line 545).
- **Verdict**: **Confirmed**. The formula is exact. Fan power is added to the sensible delivery with SHR=1. OCHRE does report `Fan Power (kW)` separately at verbosity ≥ 7 (line 593) but does not expose a `coil_sensible_cooling` field distinct from `delivered_heat`.

**Citation 3: Code locations (`ideal_hvac.rs:562-571`, `air_conditioner.rs:800-806`, `ports.rs:17-29`)**

- **Source found**: Direct file reads of all three files.
- **Verdict**: **Confirmed with minor line-number shift** (ideal_hvac block starts at 551, not 562 as cited in Step 3; the 552-571 range in the Problem description is correct). All described logic is present and unmodified.

### Legitimacy

- **Verdict**: **Partially Legitimate**

- **Rationale**: The core diagnostic gap is real and confirmed by code inspection: neither `AirConditioner` nor `IdealHvac` emits `coil_sensible_cooling_w` or `fan_heat_w` as separate telemetry keys, making it impossible for downstream analysis to recover the true coil gross output or isolate fan waste heat. The OCHRE cross-check (line 543) confirms the physics match. The EnergyPlus parallel — that E+ exposes `Cooling Coil Sensible Cooling Rate` separately from fan electrical — is substantively correct and independently confirmed via the I/O Reference documentation. The fix approach (add telemetry-only keys, leave port physics unchanged) is sound.

  Two issues lower the rating to "Partially Legitimate":
  1. **Fabricated section reference**: EnergyPlus Engineering Reference §16.7 "Fan Heat Modeling" does not exist in any version of the document. The correct citation is the EnergyPlus Input/Output Reference, output variable `Cooling Coil Sensible Cooling Rate [W]` for DX coil objects, plus the Air System Fans engineering reference section on motor-in-airstream fraction. This is a citation integrity problem, not a physics error.
  2. **Invariant 2 formula error (DSE ≠ 1 case)**: The ticket states `COIL_SENSIBLE_COOLING_W * dse + FAN_HEAT_W == |accumulator.sensible_for_category(HvacCooling)|`. This is only correct when `dse = 1.0`. In the general case, `write_zone_thermal_contributions` passes the gross value `(-sensible_cooling_w + fan_heat_w)` and the zone receives it scaled by `zone_heat_fractions[conditioned] = duct_dse`. So the actual port = `(-sensible_cooling_w + fan_heat_w) * duct_dse`, and the correct invariant is: `(COIL_SENSIBLE_COOLING_W - FAN_HEAT_W) * dse == |accumulator.sensible_for_category(HvacCooling)|`. The ticket's formula conflates the `dse` scaling of fan heat with the unscaled `FAN_HEAT_W` telemetry value.

### Proposed Fix Summary

Add three string constants to `hares-types/src/telemetry_keys.rs`:
```rust
pub const COIL_SENSIBLE_COOLING_W: &str = "coil_sensible_cooling_w";
pub const COIL_LATENT_COOLING_W: &str = "coil_latent_cooling_w";
pub const FAN_HEAT_W: &str = "fan_heat_w";
```

In `AirConditioner::step()` (after the existing `SENSIBLE_COOLING_W` and `LATENT_COOLING_W` lines ~845–847), emit:
```rust
self.telemetry.set(tk::COIL_SENSIBLE_COOLING_W, sensible_cooling_w); // pre-DSE
self.telemetry.set(tk::COIL_LATENT_COOLING_W, latent_cooling_w);    // pre-DSE
self.telemetry.set(tk::FAN_HEAT_W, fan_heat_w);
```

In `IdealHvac::step()` (after line 589, the existing `tk::THERMAL_OUTPUT_W` set), emit:
```rust
self.telemetry.set(tk::COIL_SENSIBLE_COOLING_W, capacity_w * self.shr);
self.telemetry.set(tk::FAN_HEAT_W, fan_power_w);
```

Update `ThermalAccumulator` doc comment in `ports.rs:155–163` to clarify that `sensible_by_category[HvacCooling]` includes fan heat offset.

The correct invariant to test (replacing the erroneous formula in the ticket) is:
```
(COIL_SENSIBLE_COOLING_W - FAN_HEAT_W) * dse == |accumulator.sensible_by_category[HvacCooling]|
```
which reduces to the simpler form when `dse=1`:
```
COIL_SENSIBLE_COOLING_W - FAN_HEAT_W == |accumulator.sensible_by_category[HvacCooling]|
```

No physics change is required. `PortContribution::Thermal` writes remain identical.

### Test Written

- **File**: `crates/hares-equipment/tests/hvac_parity.rs` (new function `ac_coil_sensible_and_fan_heat_telemetry_present`, inserted before test 13)
- **What it tests**: Asserts that `coil_sensible_cooling_w` and `fan_heat_w` are present in `AirConditioner` telemetry after a cooling step, and verifies three invariants: (1) with dse=1, `coil_sensible_cooling_w == sensible_cooling_w`; (2) `fan_heat_w ≥ 0`; (3) the port's `HvacCooling` bucket equals `-(coil_sens - fan_heat)` (the correct dse=1 invariant).
- **Status**: Currently **failing** (`coil_sensible_cooling_w telemetry key missing from AirConditioner`), confirming the bug is present and not yet fixed.
