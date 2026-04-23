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
