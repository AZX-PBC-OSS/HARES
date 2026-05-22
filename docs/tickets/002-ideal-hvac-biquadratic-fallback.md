# IdealHvac Biquadratic Curve Fallback for Non-Ideal Path

**Severity**: High
**Priority**: P1
**Status**: Open
**Areas**: hares-equipment, hares-physics

## Problem

When `use_ideal_cached == false` in `IdealHvac::step()` (lines 518-528), the equipment falls back to using **rated** capacity at current conditions without evaluating biquadratic performance curves. This means a heat pump at −8.3°C outdoor delivers its full rated heating capacity, which is physically wrong — AHRI 210/240-2023 Table 9 defines the H3 rating condition at −8.3°C (17°F) and typical ASHP capacity at H3 is 60-70% of H1 rated (47°F/8.3°C).

The non-ideal path exists for `IdealCapacityMode::Off` or `Auto` at fine timesteps (< 300 s). In these modes, the equipment should still account for capacity degradation at extreme conditions, but currently it does not.

## Current Behavior

In `ideal_hvac.rs:518-528`:

```rust
let mut capacity_w = match self.mode {
    ThermostatMode::Deadband => 0.0,
    ThermostatMode::Heating if self.use_ideal_cached => {
        (self.ideal_capacity_w * self.load_fraction).max(0.0)
    }
    ThermostatMode::Cooling if self.use_ideal_cached => {
        (self.ideal_capacity_w * self.load_fraction).min(0.0)
    }
    ThermostatMode::Heating => self.rated_capacity_w * self.load_fraction,
    ThermostatMode::Cooling => -self.cooling_capacity_w * self.load_fraction,
};
```

Lines 526-527: When `use_ideal_cached` is `false`, the capacity is simply `rated_capacity_w * load_fraction` (heating) or `-cooling_capacity_w * load_fraction` (cooling). No biquadratic curve evaluation occurs. The rated values reflect AHRI test conditions (47°F / 8.3°C outdoor for heating, 95°F / 35°C outdoor for cooling), but the actual operating conditions may be wildly different.

By contrast, the dynamic equipment paths correctly evaluate biquadratic curves:

- **CoolingCore** (`air_conditioner.rs:968-1027`): `curve_inputs()` evaluates capacity and EIR biquadratic curves at the current indoor WB and outdoor DB temperatures via `hvac.evaluate_biquadratic_with_flow()`.
- **HeatPumpHeaterCore** (`heater.rs:866-883`): Evaluates capacity biquadratic at `(zone.temperature_c, outdoor_temp_c)` and EIR biquadratic at the same conditions.
- **HvacEquipment::evaluate_biquadratic_with_flow** (`hvac_core.rs:849-867`): Returns `(raw, flow_adjusted)` from the biquadratic curve at current conditions.

## Required Behavior

When `use_ideal_cached == false`, the non-ideal fallback path must:

1. Evaluate the capacity biquadratic curve at current indoor/outdoor conditions to obtain a capacity correction factor (`cap_ratio`).
2. Evaluate the EIR biquadratic curve at current indoor/outdoor conditions to obtain an EIR correction factor (`eir_ratio`).
3. Apply `cap_ratio` to the rated capacity: `corrected_capacity = rated_capacity * cap_ratio`.
4. Apply `eir_ratio` to the rated EIR for electrical consumption: `corrected_eir = rated_eir * eir_ratio`.
5. Only use raw rated values when the biquadratic coefficients are identity (i.e., `[1.0, 0.0, 0.0, 0.0, 0.0, 0.0]`), which means no correction is needed.

The mathematical form follows EnergyPlus Engineering Reference, DX Coil section:

```
Q_corrected = Q_rated × CAP_FT(T_indoor, T_outdoor)
COP_corrected = COP_rated / EIR_FT(T_indoor, T_outdoor)
```

where `CAP_FT` and `EIR_FT` are the biquadratic capacity and EIR correction functions evaluated at the current entering air conditions.

## Approach

### Step 1: Add biquadratic curve storage to IdealHvac

The `IdealHvac` struct currently has no biquadratic curve fields. Add:

> **Note**: Consider grouping the six biquadratic-related fields into a sub-struct
> `BiquadraticCurveSet { coeffs: [f64; 6], x1_bounds: (f64, f64), x2_bounds: (f64, f64) }`
> rather than adding 6 individual fields to `IdealHvac`. This would give two fields
> (`capacity_curve: BiquadraticCurveSet`, `eir_curve: BiquadraticCurveSet`) instead of six,
> and the `BiquadraticCurveSet` struct can carry an `evaluate()` method and the
> `warned_oob_bounds: AtomicBool` flag from ticket 003. Evaluate this refactoring
> during implementation.

```rust
/// Biquadratic capacity correction curve coefficients [a, b, c, d, e, f].
/// Default identity: no correction. Loaded from config "biquadratic_coeffs".
capacity_biquadratic_coeffs: [f64; 6],
/// Biquadratic EIR correction curve coefficients [a, b, c, d, e, f].
eir_biquadratic_coeffs: [f64; 6],
/// Bounds for biquadratic curve inputs (indoor, outdoor).
biquadratic_x1_bounds: (f64, f64),
biquadratic_x2_bounds: (f64, f64),
```

Initialize with identity coefficients (same as `hvac_core.rs:23`):
```rust
const DEFAULT_BIQUADRATIC_COEFFS: [f64; 6] = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0];
```

### Step 2: Load curves from config in `IdealHvac::init()`

Use the existing `load_biquadratic_coeffs` helper from `core_config.rs` (called at `hvac_core.rs:408`, distinct from `load_curve_pair` in `ac_config.rs` which is AC-specific) to load capacity and EIR biquadratic coefficients from the config. The keys should be the same as used by the dynamic equipment:
- `capacity_biquadratic_coeffs`
- `eir_biquadratic_coeffs`
- `biquadratic_x1_min`, `biquadratic_x1_max`
- `biquadratic_x2_min`, `biquadratic_x2_max`

Also load from typed config (`IdealHvacConfig`) if biquadratic coefficients are provided there.

### Step 3: Evaluate curves in the non-ideal fallback

Replace lines 526-527 in `step()`:

```rust
// BEFORE:
ThermostatMode::Heating => self.rated_capacity_w * self.load_fraction,
ThermostatMode::Cooling => -self.cooling_capacity_w * self.load_fraction,

// AFTER:
ThermostatMode::Heating => {
    let cap_ratio = self.evaluate_capacity_curve(t_indoor_c, t_outdoor_c);
    (self.rated_capacity_w * cap_ratio * self.load_fraction).max(0.0)
}
ThermostatMode::Cooling => {
    let cap_ratio = self.evaluate_capacity_curve(t_indoor_c, t_outdoor_c);
    (-self.cooling_capacity_w * cap_ratio * self.load_fraction).min(0.0)
}
```

Add helper method:
```rust
fn evaluate_capacity_curve(&self, t_indoor_c: f64, t_outdoor_c: f64) -> f64 {
    let curve = BiquadraticCurve {
        coeffs: self.capacity_biquadratic_coeffs,
        x1_bounds: self.biquadratic_x1_bounds,
        x2_bounds: self.biquadratic_x2_bounds,
    };
    curve.evaluate(t_indoor_c, t_outdoor_c).max(0.0)
}
```

Similarly for EIR, used in fan power and electrical consumption calculation.

### Step 4: Add EIR curve evaluation for electrical consumption

The current code at line 544-548 computes fan power from `rated_eir`. In the non-ideal path, also apply the EIR correction:

```rust
let eir_correction = if !self.use_ideal_cached {
    self.evaluate_eir_curve(t_indoor_c, t_outdoor_c).max(f64::EPSILON)
} else {
    1.0
};
let effective_eir = self.rated_eir * eir_correction;
```

### Step 5: Pass environment state into step()

Currently `IdealHvac::step()` receives `_env: &EnvironmentState` (underscore-prefixed, unused). Change to use `env` to access temperatures:

```rust
let t_outdoor_c = env.weather.outdoor_temp_c;
let t_indoor_c = env.zones
    .iter()
    .find(|z| z.id == self.zone_id)
    .map(|z| z.temperature_c)
    .unwrap_or(21.0); // fallback to typical indoor temp if zone not found
```

`ZoneState::temperature_c` is the zone dry-bulb temperature (verified: `hares-types/src/environment.rs:82`). `env.weather.outdoor_temp_c` is the outdoor dry-bulb (verified: `hares-types/src/environment.rs` WeatherState). In practice `IdealHvac` already calls `lookup_zone_temp(env, self.zone_id)` inside `update_mode()` — consider caching the result rather than iterating `env.zones` twice.

### Step 6: Update tests

- Add a test: heat pump at −8.3°C outdoor (AHRI H3 condition, AHRI 210/240-2023 Table 9) with a realistic capacity curve delivers ~60-70% of rated capacity, not 100%.
- Add a test: AC at 46°C outdoor with a realistic cooling curve delivers reduced capacity.
- Verify that identity curves (`[1,0,0,0,0,0]`) produce rated capacity exactly (no regression).

## HPXML Wiring

`capacity_biquadratic_coeffs` and `eir_biquadratic_coeffs` are loaded from
defaults CSV via `apply_multispeed_parameters` in `resolve_hvac.rs`. When
HPXML provides biquadratic curves, those take precedence. When no curves
are provided (identity default), the defaults from ticket 010 are substituted.
No additional HPXML wiring needed beyond ticket 010's default curve infrastructure.

## Definition of Done

- [ ] `IdealHvac` struct has biquadratic curve coefficient fields
- [ ] Non-ideal fallback path evaluates capacity curve at current indoor/outdoor conditions
- [ ] Non-ideal fallback path evaluates EIR curve for electrical consumption
- [ ] Identity curves produce rated capacity (no regression for configs without curves)
- [ ] Realistic ASHP curve at −8.3°C (AHRI H3, AHRI 210/240-2023 Table 9) produces ~60-70% of rated heating capacity
- [ ] Realistic AC curve at 46°C produces reduced cooling capacity vs rated
- [ ] `step()` uses `env` parameter instead of ignoring it
- [ ] `tracing::debug!` when non-ideal fallback evaluates curves: `cap_ratio={cap_ratio:.4}, eir_ratio={eir_ratio:.4} at t_indoor={t_in:.1}, t_outdoor={t_out:.1}`
- [ ] Telemetry keys `CAP_RATIO` and `EIR_RATIO` added for verifying curve evaluation
- [ ] All existing tests pass

## Verification

1. **Regression test**: Existing `ideal_capacity_mode_off_step_uses_rated_capacity` test at `ideal_hvac.rs:1181` must still pass when curves are identity.

2. **New test — heating capacity degradation at AHRI H3**: Configure IdealHvac with an ASHP heating capacity curve. At −8.3°C outdoor (AHRI 210/240-2023 Table 9 H3 condition), verify `capacity_w` is in the range 60-70% of `rated_capacity_w`.

3. **New test — cooling capacity degradation**: At 46°C outdoor with a realistic cooling curve, verify capacity < rated.

4. **Numerical check**: For a linearized capacity curve `CAP_FT = 1.0 + 0.02 * (T_out - 8.3)` (rough ASHP approximation), at T_out = −8.3°C: `CAP_FT = 1.0 + 0.02 * (−16.6) = 0.668`. This is within the AHRI H3 60-70% range. Verify the computed capacity matches `rated * 0.668`.

## References

- EnergyPlus Engineering Reference, *DX Heating Coil* subsection for `Coil:Heating:DX:SingleSpeed`, equations for temperature correction using `Curve:Biquadratic` objects. See also I/O Reference for `Coil:Heating:DX:SingleSpeed` fields `Heating_Capacity_Function_of_Temperature_Curve_Name` and `Heating_EIR_Function_of_Temperature_Curve_Name`.
- AHRI Standard 210/240-2023: H1 heating test condition is 8.3°C (47°F) outdoor; H3 is -8.3°C (17°F). Capacity at H3 is typically 60-70% of H1 rated.
- OCHRE `HVAC.py` `_biquadratic()` method: always evaluates curves regardless of ideal/non-ideal mode
- `hares-equipment/src/hvac/air_conditioner.rs:968-1027`: Dynamic path correctly evaluates curves
- `hares-equipment/src/hvac/heat_pump/heater.rs:866-883`: HP heater correctly evaluates curves

## Related Tickets

- 001-unify-hfg-add-humidity-port.md (IdealHvac also writes latent without humidity_ratio_delta — the humidity port addition belongs to 001; implementers of 002 must not duplicate that work)
- 003-tighten-biquadratic-default-bounds.md (curve bounds affect the correction factor evaluation)
- 005-fan-heat-diagnostic-category.md (EIR curve also affects reported COP)

---

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-20

### Code Confirmation

- [x] **Referenced line numbers still match** — `ideal_hvac.rs:518-528` is confirmed correct as of this audit. The `capacity_w` match block is at lines 518-528; the non-ideal arms are at lines 526-527 exactly as cited.
- [x] **Described logic matches current implementation** — `IdealHvac::step()` (line 512) receives `_env: &EnvironmentState` (underscore-prefixed, unused). Lines 526-527 confirm `ThermostatMode::Heating => self.rated_capacity_w * self.load_fraction` and `ThermostatMode::Cooling => -self.cooling_capacity_w * self.load_fraction` with no biquadratic evaluation. The `IdealHvac` struct (lines 30-68) has no biquadratic coefficient fields. The bug is present and unaddressed.
- [x] **OCHRE cross-check: partially matches the ticket's claim, with an important nuance**
  - The base `HVAC.update_capacity()` (HVAC.py:431-456): In `use_ideal_capacity=True` mode, calls `solve_ideal_capacity()` (envelope solver) and clips to `capacity_max`. It does **not** call `_biquadratic()` for capacity in this path. In `use_ideal_capacity=False` mode it returns `self.capacity_list[self.speed_idx]` — a static rated value — also no biquadratic.
  - `DynamicHVAC.update_capacity()` (HVAC.py:990-1027) overrides the base: it calls `calculate_biquadratic_param('cap', speed_idx=max_speed)` to compute `capacity_max` **always** (line 993), and in the ideal path (line 995-1020) evaluates biquadratic for each speed to set `speed_idx`. In the non-ideal path (line 1021-1027) it evaluates biquadratic for `speed_idx`. So `DynamicHVAC` (which covers AC, HP, etc.) **always** evaluates biquadratic curves.
  - **Verdict**: The ticket's claim "OCHRE `_biquadratic()` method: always evaluates curves regardless of ideal/non-ideal mode" is correct for `DynamicHVAC` (the class that models ASHP, AC) but is inaccurate for the base `HVAC` class. HARES `IdealHvac` is modelled closer to the base `HVAC` class, which makes the omission analogous to what OCHRE would do for single-speed `DynamicHVAC` equipment in non-ideal mode — which **does** apply biquadratic corrections. The divergence is **accidental** (not intentional).
  - File+line evidence: `vendors/OCHRE/ochre/Equipment/HVAC.py:990-1027` (DynamicHVAC), `:431-456` (base HVAC), `:24-60` (`_biquadratic()` function).
- [x] **EnergyPlus cross-check: confirmed**
  - Source: DesignBuilder v7.2 DX Heating Coil documentation (mirrors EnergyPlus I/O Reference for `Coil:Heating:DX:SingleSpeed`), fetched from `https://designbuilder.co.uk/helpv7.2/Content/HeatingCoilDX.htm`
  - Quoted passage — **Capacity curve**: *"The Bi-quadratic, Quadratic or Cubic performance curve that parameterises the variation of the total heating capacity as a function of the both the indoor and outdoor air dry-bulb temperature or just the outdoor air dry-bulb temperature depending on the type of curve selected. The output of this curve is multiplied by the rated total heating capacity to give the total heating capacity at specific temperature operating conditions (i.e., at an indoor air dry-bulb temperature or outdoor air dry-bulb temperature different from the rating point temperature). The curve is normalised to have the value of 1.0 at the rating point."*
  - Quoted passage — **EIR curve**: *"The Bi-quadratic, Quadratic or Cubic performance curve that parameterises the variation of the energy input ratio (EIR) as a function of the both the indoor and outdoor air dry-bulb temperature or just the outdoor air dry-bulb temperature depending on the type of curve selected. The output of this curve is multiplied by the rated EIR (inverse of rated COP) to give the EIR at specific temperature operating conditions (i.e., at an indoor air dry-bulb temperature or outdoor air dry-bulb temperature different from the rating point temperature). The curve is normalised to have the value of 1.0 at the rating point."*
  - The biquadratic form `z = a + b*x1 + c*x1² + d*x2 + e*x2² + f*x1*x2` is confirmed by EnergyPlus 9.0 Engineering Reference, Performance Curves section.
  - HARES `BiquadraticCurve::evaluate()` (`hares-physics/src/biquadratic.rs:27-31`) implements exactly this form and is already used by `HvacEquipment::evaluate_biquadratic_with_flow()` for the dynamic paths.

### Web-Verified Citations

**Citation 1: AHRI 210/240-2023 H3 condition = −8.3°C (17°F)**
- **Source found**: ANSI/AHRI Standard 210/240-2008 (with addenda), referenced in search results from `ahrinet.org` and `bsesc.energy.gov`; confirmed by multiple secondary sources.
- **Quoted passage**: From web search result (ahrinet.org/bsesc.energy.gov): "H3 Test is a required, steady heating test with indoor conditions of 70.0°F dry bulb (21.1°C) and 60.0°F maximum (15.6°C maximum) wet bulb, outdoor temperature of 17.0°F (−8.33°C)." The AHRI certificate lists ratings at 47°F (H1) and 17°F (H3). The 2023 revision uses the same temperature ladder.
- **Verdict**: **Confirmed**. H3 = −8.3°C (17°F), H1 = 8.3°C (47°F). Direct PDF access to AHRI 210/240-2023 was blocked (HTTP 403); the temperatures are confirmed through multiple secondary sources including DOE and NEEP documentation.

**Citation 2: AHRI H3 capacity = 60-70% of H1 rated**
- **Source found**: `learnmetrics.com/heat-pump-efficiency-vs-temperature-graph/` (independently fetched); NREL OSTI documents; NEEP cold-climate ASHP specification v4.0.
- **Quoted passage**: From learnmetrics.com fetch: "At 17°F: the article calculates approximately 15,568 BTU output, representing a reduction to roughly 65% of rated capacity (or a 35% loss from the 47°F baseline)." NREL search results confirm "approximately 32% capacity degradation from 47°F to 17°F" for standard ASHP, placing H3 capacity at ~68% of H1.
- **Verdict**: **Confirmed as an approximate range for standard (non-cold-climate) ASHP**. The 60-70% figure is physically plausible and consistent with independently retrieved data. The exact range depends on specific equipment; cold-climate ASHP can maintain higher capacity at H3. For standard ASHP the ticket's claim is correct.

**Citation 3: EnergyPlus Engineering Reference — DX Coil biquadratic equations**
- **Source found**: `https://designbuilder.co.uk/helpv7.2/Content/HeatingCoilDX.htm` (mirrors EnergyPlus I/O Reference); `https://bigladdersoftware.com/epx/docs/9-0/engineering-reference/performance-curves.html`
- **Quoted passage**: See Code Confirmation → EnergyPlus cross-check above. The formulas `Q_corrected = Q_rated × CAP_FT(T_indoor, T_outdoor)` and `EIR_corrected = EIR_rated × EIR_FT(T_indoor, T_outdoor)` match EnergyPlus documentation exactly.
- **Verdict**: **Confirmed**. The mathematical form stated in the ticket is correct.

**Citation 4: OCHRE `HVAC.py` `_biquadratic()` always evaluates curves**
- **Source found**: `vendors/OCHRE/ochre/Equipment/HVAC.py` (local submodule, read directly)
- **Quoted passage**: `_biquadratic()` at HVAC.py:24-60 is the core evaluation function. `DynamicHVAC.update_capacity()` at HVAC.py:993 calls `calculate_biquadratic_param(param='cap', speed_idx=max_speed)` unconditionally to compute `capacity_max`, and the non-ideal branch at HVAC.py:1021-1027 calls `calculate_biquadratic_param(param='cap', speed_idx=self.speed_idx)` to get the operating capacity.
- **Verdict**: **Partially correct**. For `DynamicHVAC` (the class that models ASHP, AC, HP), biquadratic is always evaluated. The base `HVAC` class does not call biquadratic in either path. The ticket's claim is accurate for the equipment types relevant to `IdealHvac`.

**Citation 5: Dynamic equipment paths — `air_conditioner.rs:968-1027` and `heater.rs:866-883`**
- **Source found**: `crates/hares-equipment/src/hvac/air_conditioner.rs` and `crates/hares-equipment/src/hvac/heat_pump/heater.rs` (read directly)
- **Quoted passage**: `air_conditioner.rs:968-1027` defines the `curve_inputs()` closure which calls `hvac.evaluate_biquadratic_with_flow()` twice (cap + EIR). `heater.rs:866-883` calls `self.hvac.evaluate_biquadratic_with_flow(speed_index * 2, zone.temperature_c, env.weather.outdoor_temp_c, 1.0)`.
- **Verdict**: **Confirmed**. Line numbers are accurate; both dynamic paths correctly evaluate biquadratic curves.

**Citation 6: `hvac_core.rs:849-867` — `evaluate_biquadratic_with_flow()`**
- **Source found**: `crates/hares-equipment/src/hvac/hvac_core.rs` (read directly)
- **Quoted passage**: Method `evaluate_biquadratic_with_flow` at lines 849-867 returns `(raw, flow_adjusted)` from the biquadratic curve.
- **Verdict**: **Confirmed**. Line numbers match.

**Citation 7: `hvac_core.rs:23` — `DEFAULT_BIQUADRATIC_COEFFS`**
- **Source found**: `crates/hares-equipment/src/hvac/hvac_core.rs:23` (read directly)
- **Quoted passage**: `pub(super) const DEFAULT_BIQUADRATIC_COEFFS: [f64; 6] = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0];`
- **Verdict**: **Confirmed**. Identity coefficients match the ticket's specification.

**Citation 8: `hares-types/src/environment.rs:82` — `ZoneState::temperature_c`**
- **Source found**: Not individually read (not cited with a claim that could be wrong). The field exists and is used in numerous tests and the air_conditioner curve_inputs closure. Accepted as correct.
- **Verdict**: **Plausible, not independently verified at line level** — but inconsequential to the core bug claim.

### Legitimacy

- **Verdict**: **Legitimate**

- **Rationale**: The bug is real and confirmed by direct code inspection. `IdealHvac::step()` at lines 526-527 returns `rated_capacity_w * load_fraction` (heating) and `-cooling_capacity_w * load_fraction` (cooling) with no reference to outdoor temperature or any biquadratic curve. The `IdealHvac` struct has no biquadratic coefficient fields. This is physically wrong: a heat pump's capacity degrades significantly at cold outdoor temperatures, and both OCHRE's `DynamicHVAC` and EnergyPlus's `Coil:Heating:DX:SingleSpeed` model this via temperature-dependent biquadratic correction curves. The EnergyPlus formula is confirmed from the DesignBuilder/EnergyPlus I/O Reference: "The output of this curve is multiplied by the rated total heating capacity." The AHRI H3/H1 temperature claims (−8.3°C / 8.3°C) are confirmed from standard secondary sources. The 60-70% capacity ratio at H3 vs H1 is consistent with independently retrieved empirical data (~65% per learnmetrics; ~68% per NREL search results). All cited HARES file locations are accurate. One minor inaccuracy: the OCHRE claim "always evaluates curves regardless of ideal/non-ideal mode" is technically true for `DynamicHVAC` but not for the base `HVAC` class; this does not affect the legitimacy of the ticket.

### Proposed Fix Summary

1. Add `capacity_biquadratic_coeffs: [f64; 6]`, `eir_biquadratic_coeffs: [f64; 6]`, `biquadratic_x1_bounds: (f64, f64)`, and `biquadratic_x2_bounds: (f64, f64)` fields to `IdealHvac` (or a `BiquadraticCurveSet` sub-struct as the ticket suggests). Default to identity coefficients `[1.0, 0.0, 0.0, 0.0, 0.0, 0.0]`.
2. Load the curves from config in `IdealHvac::init()` using the existing `load_biquadratic_coeffs()` helper from `core_config.rs`.
3. In `step()`, rename `_env` to `env`. Extract `t_outdoor_c = env.weather.outdoor_temp_c` and `t_indoor_c` from the zone (reusing `lookup_zone_temp` already called in `update_mode()`).
4. Replace lines 526-527 to evaluate `BiquadraticCurve::evaluate(t_indoor_c, t_outdoor_c)` for the capacity correction, and apply it: `rated_capacity_w * cap_ratio * load_fraction`.
5. Apply EIR correction to fan power calculation at lines 544-548.
6. When coefficients are identity, the correction factor is 1.0 and the output is unchanged (no regression).

**Do NOT implement this fix** — this audit section only.

### Test Written

- **File**: `crates/hares-equipment/src/hvac/ideal_hvac.rs` (in-module `#[cfg(test)]` block, near end of file)
- **Test 1** (`non_ideal_heating_capacity_degrades_at_ahri_h3_condition`, marked `#[ignore]`): Configures `IdealHvac` with `ideal_capacity_mode = off`, a linearised ASHP capacity curve (`a=0.834, d=0.02`), outdoor temp −8.3°C (H3), and asserts that the thermal output is between 55% and 75% of rated capacity. Currently **ignored** because the required config fields do not exist yet; must be enabled (remove `#[ignore]`) as part of the ticket 002 fix.
- **Test 2** (`non_ideal_heating_identity_curve_produces_rated_capacity`): Configures the same setup with identity coefficients and asserts the output equals `rated_capacity_w` exactly. This test **passes today** and must continue to pass after the fix (no-regression guard).
