# Tighten Biquadratic Default Bounds

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-equipment, hares-physics

## Problem

The default biquadratic curve input bounds are `(-100, +100)°C` in `hvac_core.rs:45-46`, allowing wild extrapolation at extreme conditions. Biquadratic polynomials are calibrated over 15-20°C ranges; evaluating them at inputs 50-100°C outside their calibration domain can produce physically nonsensical values (negative capacities, EIR < 0, or capacities > 3× rated).

For example, a typical cooling capacity curve calibrated over indoor WB 19-27°C and outdoor DB 28-46°C may produce `CAP_FT = -0.5` at T_outdoor = -50°C (extrapolated), yielding "negative cooling capacity" which the code then `.max(0.0)` clamps to zero. This is better than allowing negative values but still wrong — the equipment should have its capacity bounded by a physically meaningful range, and the solver should be warned that the inputs are outside the curve's valid domain.

## Current Behavior

In `hvac_core.rs:42-46`:

```rust
/// Biquadratic curve input bounds clamp physically impossible extrapolation.
/// EnergyPlus Engineering Reference §16 (Performance Curves) requires bounded
/// curve objects; ±100°C is not physically meaningful for residential HVAC.
/// Per-curve explicit bounds from equipment CSV files should always be preferred.
const DEFAULT_BIQUADRATIC_X1_BOUNDS: (f64, f64) = (-100.0, 100.0);
const DEFAULT_BIQUADRATIC_X2_BOUNDS: (f64, f64) = (-100.0, 100.0);
```

These bounds are used when no explicit bounds are specified in the equipment configuration. The `BiquadraticCurve::evaluate()` method at `biquadratic.rs:27-31` clamps inputs to these bounds before evaluating the polynomial:

```rust
pub fn evaluate(&self, x1: f64, x2: f64) -> f64 {
    let x1_clamped = x1.clamp(self.x1_bounds.0, self.x1_bounds.1);
    let x2_clamped = x2.clamp(self.x2_bounds.0, self.x2_bounds.1);
    biquadratic(&self.coeffs, x1_clamped, x2_clamped)
}
```

With bounds of ±100°C, the clamping never activates for any realistic temperature, so the polynomial is free to extrapolate to any value.

**OCHRE comparison**: OCHRE's `HVAC.py` lines 43-45 set per-curve bounds from the CSV file (e.g., `min_Twb=13.88, max_Twb=23.88` for cooling curves). When no bounds are specified, OCHRE falls back to ±100°C. HARES must not replicate this fallback: EnergyPlus Engineering Reference §16 (Performance Curves) explicitly requires bounded curve objects; ±100°C is not physically meaningful for residential HVAC and allows unconstrained extrapolation.

**EnergyPlus behavior**: E+ issues a warning when curve inputs are outside the specified min/max range. It does NOT clamp by default — it extrapolates. However, E+ curve objects always have explicit bounds in the IDF; they are not left at ±100°C fallback.

**ASHRAE rating conditions** define the valid operating ranges for DX coils:
- Cooling: indoor WB 19.4-26.7°C, outdoor DB 27.8-46.1°C (AHRI 210/240 Table 1)
- Heating: indoor DB 15-24°C, outdoor DB -20 to 16.7°C (AHRI 210/240 H1-H3 test conditions)

A reasonable residential envelope covers outdoor temperatures from approximately -50°C (extreme cold) to +60°C (extreme heat). The bounds should cover this range with margin but not extend far beyond it.

## Required Behavior

1. Change default biquadratic bounds from `(-100, +100)°C` to per-axis values:
   - **x1 (indoor axis — DB or WB depending on coil type)**: `(−10, +50)°C` for dry-bulb (heating curves), `(−10, +35)°C` for wet-bulb (cooling curves). These bracket AHRI 210/240-2023 Table 9 rating envelopes (cooling indoor WB 19.4-26.7°C, heating indoor DB 15-24°C) with generous margin.
   - **x2 (outdoor DB)**: `(−50, +60)°C`. Covers global residential conditions (South Pole −49°C to Death Valley +57°C) while preventing extreme polynomial extrapolation.

   The default constants in `hvac_core.rs` should be named to reflect their axis intent. Per-curve explicit bounds loaded from equipment CSV files always override these defaults.

2. Add a `warn_on_clamp: bool` field to `BiquadraticCurve` (see Performance requirement in Step 2 below). When `true`, log a `tracing::warn!` on clamped evaluation, mirroring EnergyPlus's out-of-range warning behavior.

3. Document in code comments that per-curve explicit bounds from equipment specs or CSV files must always be preferred over these defaults.

## Approach

### Step 1: Change default bounds

In `hvac_core.rs:45-46`, change to per-axis bounds:

```rust
// BEFORE:
const DEFAULT_BIQUADRATIC_X1_BOUNDS: (f64, f64) = (-100.0, 100.0);
const DEFAULT_BIQUADRATIC_X2_BOUNDS: (f64, f64) = (-100.0, 100.0);

// AFTER:
/// Default biquadratic x1 (indoor) input bounds.
/// DB-based curves (heating): covers indoor DB −10 to +50°C.
/// WB-based curves (cooling): caller should supply tighter per-curve bounds (AHRI: 19.4-26.7°C).
/// Fallback only; per-curve explicit bounds from equipment CSV/specs must be preferred.
/// Justified by AHRI 210/240-2023 Table 9 rating envelopes plus margin.
const DEFAULT_BIQUADRATIC_X1_BOUNDS: (f64, f64) = (-10.0, 50.0);
/// Default biquadratic x2 (outdoor DB) input bounds.
/// Covers global residential outdoor conditions: −50°C (polar extreme) to +60°C (desert extreme).
/// EnergyPlus Engineering Reference §16 requires bounded Performance Curve objects;
/// ±100°C is not physically meaningful for residential HVAC.
/// Fallback only; per-curve explicit bounds from equipment CSV/specs must be preferred.
const DEFAULT_BIQUADRATIC_X2_BOUNDS: (f64, f64) = (-50.0, 60.0);
```

### Step 2: Add bound-hit warning in BiquadraticCurve::evaluate

Add a `warn_on_clamp: bool` field to `BiquadraticCurve` (default `false`) and gate the warning on it. Update `biquadratic.rs`:

```rust
pub struct BiquadraticCurve {
    pub coeffs: [f64; 6],
    pub x1_bounds: (f64, f64),
    pub x2_bounds: (f64, f64),
    pub warn_on_clamp: bool,
}

impl BiquadraticCurve {
    pub fn evaluate(&self, x1: f64, x2: f64) -> f64 {
        let x1_clamped = x1.clamp(self.x1_bounds.0, self.x1_bounds.1);
        let x2_clamped = x2.clamp(self.x2_bounds.0, self.x2_bounds.1);
        if self.warn_on_clamp
            && ((x1 - x1_clamped).abs() > f64::EPSILON
                || (x2 - x2_clamped).abs() > f64::EPSILON)
        {
            tracing::warn!(
                "biquadratic input outside bounds: x1={x1:.1} clamped to [{}, {}], \
                 x2={x2:.1} clamped to [{}, {}]",
                self.x1_bounds.0, self.x1_bounds.1,
                self.x2_bounds.0, self.x2_bounds.1,
            );
        }
        biquadratic(&self.coeffs, x1_clamped, x2_clamped)
    }
}
```

**Performance requirement**: `tracing::warn!` is not gated by build profile — it fires in release builds on every clamped evaluation. A bare `warn!` in a hot loop would flood the log and add measurable overhead. The required design is: add a `warn_on_clamp: bool` field to `BiquadraticCurve` (default `false`). Hot-loop callers (equipment `step()`) leave it `false`, so no warning fires. Validation paths and init-time curve checks set it `true`. This gives zero per-call overhead in the hot path and targeted warnings during setup or offline analysis. This is not optional — it is part of the required implementation (see DoD below).

### Step 3: Verify dehumidifier bounds are unaffected

The dehumidifier at `dehumidifier.rs:33-34, 118-126` has its own bounds:
```rust
const DEFAULT_DB_BOUNDS_C: (f64, f64) = (10.0, 40.0);
const DEFAULT_RH_BOUNDS: (f64, f64) = (RH_MIN_FRACTION, RH_MAX_FRACTION);
```
These are per-curve explicit bounds already within the new default range, so no change is needed.

### Step 4: Update documentation

Update the comment at `hvac_core.rs:42-44` to explain:
- Default bounds are for safety only; EnergyPlus Engineering Reference §16 requires bounded curve objects
- Per-curve bounds from equipment CSV/specs should be loaded via config
- The `load_bounds_pair()` function at `hvac_core.rs:443-454` handles per-curve bound loading from config keys

### Step 5: Test impact

Run existing biquadratic tests:
- `biquadratic.rs:80-90` (curve_clamps_both_axes_when_out_of_bounds) — uses custom bounds, unaffected
- `biquadratic.rs:110-152` (OCHRE reference) — uses per-curve bounds, unaffected
- `hvac_core.rs` tests — some may use default bounds

For the OCHRE cooling curve test at `biquadratic.rs:117-118`:
```rust
let twb_bounds = (13.88, 23.88);
let tdb_bounds = (18.33, 51.66);
```
These per-curve bounds are much tighter than even the new defaults, so they will continue to work correctly.

## Definition of Done

- [ ] Default x1 (indoor) bounds changed to `(−10, +50)°C` in `hvac_core.rs`
- [ ] Default x2 (outdoor DB) bounds changed to `(−50, +60)°C` in `hvac_core.rs`
- [ ] `BiquadraticCurve` has a `warn_on_clamp: bool` field (default `false`)
- [ ] `BiquadraticCurve::evaluate()` logs a `tracing::warn!` when inputs are clamped AND `warn_on_clamp == true`
- [ ] Hot-loop equipment callers (`step()`) construct `BiquadraticCurve` with `warn_on_clamp: false` — zero per-call logging overhead
- [ ] Validation/init paths set `warn_on_clamp: true` to surface out-of-range conditions
- [ ] Dehumidifier per-curve bounds (10-40°C DB, 0-100% RH) are unaffected
- [ ] All existing biquadratic tests pass
- [ ] New test: curve at −60°C outdoor is clamped to −50°C (new x2 lower bound)
- [ ] New test: curve at +70°C outdoor is clamped to +60°C (new x2 upper bound)
- [ ] New test: curve at −20°C indoor is clamped to −10°C (new x1 lower bound)
- [ ] No test relies on the old ±100°C bounds

## Verification

1. **Unit test — new bounds are enforced**: Create a curve with identity coefficients and `x1_bounds = (−10, 50)`, `x2_bounds = (−50, 60)`. Evaluate at `x1 = −80°C` and verify the result matches evaluation at `x1 = −10°C` (clamped). Evaluate at `x2 = +100°C` and verify it matches `x2 = +60°C` (clamped).

2. **Regression — OCHRE curves**: The OCHRE curve tests in `biquadratic.rs` use per-curve bounds `(13.88, 23.88)` and `(18.33, 51.66)` which are narrower than the new defaults. These must still pass unchanged.

3. **Integration — equipment step**: Run a heating step at −40°C outdoor (within new x2 bounds). Verify the biquadratic capacity correction factor is physically reasonable (between 0.3 and 1.2 for typical curves). Run at −60°C outdoor and verify the input is clamped to −50°C; when `warn_on_clamp = true`, a `tracing::warn!` must be emitted.

4. **Log verification**: Configure tracing at WARN level and run a simulation at extreme conditions. Verify that bound-hit warnings appear in the log output.

## References

- ASHRAE Handbook of Fundamentals 2021, Chapter 18: DX coil rating conditions
- AHRI Standard 210/240-2023 Table 1: Cooling rated conditions (67°F/19.4°C indoor WB, 95°F/35°C outdoor DB)
- AHRI 210/240 H1 heating: 47°F/8.3°C outdoor; H3: 17°F/-8.3°C outdoor; extreme: -5°F/-20.6°C
- EnergyPlus I/O Reference, *Curve:Biquadratic* object, fields `Minimum_Value_of_x1`, `Maximum_Value_of_x1`, `Minimum_Value_of_x2`, `Maximum_Value_of_x2`.
- EnergyPlus Engineering Reference §16 "Performance Curves": explicitly requires bounded curve objects; out-of-range inputs trigger a warning in E+
- OCHRE `HVAC.py` lines 43-45: `min_Twb`, `max_Twb`, `min_Tdb`, `max_Tdb` loaded from CSV; OCHRE's ±100°C fallback is a known gap, not a target
- `hares-physics/src/biquadratic.rs:27-31`: Current clamping implementation
- `hares-equipment/src/hvac/hvac_core.rs:42-46`: Current default bounds

## Related Tickets

- 002-ideal-hvac-biquadratic-fallback.md (IdealHvac will also evaluate curves and needs correct bounds)
- 001-unify-hfg-add-humidity-port.md (dehumidifier has its own curve bounds)
