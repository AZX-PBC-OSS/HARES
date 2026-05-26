# Multi-speed biquadratic curve interleaving across compressor speeds
**Review ID**: equip-hvac-08
**Category**: equipment-hvac
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-equipment/src/hvac/heat_pump/heater.rs`
- `crates/hares-equipment/src/hvac/heat_pump/cooler.rs`
- `crates/hares-physics/src/biquadratic.rs`
- `crates/hares-equipment/src/hvac/default_curves.rs`

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Equipment/HVAC.py`

## Findings

### Finding 1: [Severity: high]
**Description**: No init-time validation that the interleaved `biquadratic_coeffs` array count matches the number of compressor speed stages. When `biquadratic_coeffs.len() / 2 < heating_capacities_w.len()`, higher speed indices access out-of-bounds curve entries. The `evaluate_biquadratic` fallback at `hvac_core.rs:733` silently substitutes the last stored curve, which can be an EIR curve evaluated as a capacity curve (or vice versa), producing physically impossible COP values without any warning.
**Code Location**:
- `crates/hares-equipment/src/hvac/hvac_core.rs:497-550` — loads `biquadratic_coeffs` and interleaves split capacity/EIR curves but never validates `biquadratic_coeffs.len()` against `heating_capacities_w.len()` or `cooling_capacities_w.len()`.
- `crates/hares-equipment/src/hvac/hvac_core.rs:728-734` — `evaluate_biquadratic` silently falls back to `biquadratic_coeffs.last()` when `curve_index` is out of bounds.
- `crates/hares-equipment/src/hvac/heat_pump/heater.rs:1283-1288` — uses `speed_index * 2` for capacity curves; `heater.rs:1334-1339` uses `speed_index * 2 + 1` for EIR curves. Neither checks bounds.
- `crates/hares-equipment/src/hvac/air_conditioner.rs:1110-1121` — identical pattern for cooling: `speed_index * 2` / `speed_index * 2 + 1` with no bounds check.
**Root Cause**: The curve interleaving convention `[cap_0, eir_0, cap_1, eir_1, ...]` was adopted with an assumption that per-speed curve pairs always exist. The generic fallback `|| self.config.biquadratic_coeffs.last()` in `evaluate_biquadratic` was designed for graceful degradation when a single indexed curve is missing, but it doesn't distinguish between capacity and EIR curves — an EIR curve silently substituted as capacity (or vice versa) produces nonsense ratios.
**Impact**: A user-provided multi-speed HP config with per-stage capacities but only single-speed biquadratic curves would run higher speed stages using EIR coefficients as capacity multipliers, yielding wildly incorrect COP values (e.g., COP > 50 or < 0.1 at certain operating conditions). The error is silent — no log, no error return, no telemetry flag. OCHRE (`HVAC.py:814-818`) raises an exception when speed count and biquadratic equation count differ.

### Finding 2: [Severity: high]
**Description**: Out-of-bounds curve index during `MultiSpeedInterpolated` high-side evaluation. When `speed_frac > 0` at the highest speed stage, the code computes `(speed_index + 1) * 2` for the high-side capacity curve and `(speed_index + 1) * 2 + 1` for the high-side EIR curve. These indices can exceed the interleaved array bounds, triggering the same silent fallback described in Finding 1.
**Code Location**:
- `crates/hares-equipment/src/hvac/heat_pump/heater.rs:1290-1300` — capacity high-side interpolation: `(speed_index + 1) * 2` used as curve index.
- `crates/hares-equipment/src/hvac/heat_pump/heater.rs:1341-1351` — EIR high-side interpolation: `(speed_index + 1) * 2 + 1` used as curve index.
- `crates/hares-equipment/src/hvac/air_conditioner.rs:1147-1162` — identical pattern in cooler: `curve_inputs(speed_index + 1, ...)` passes unbounded index.
**Root Cause**: `multi_speed_ideal.rs`'s `select_speed_heating` / `select_speed_cooling` can return `speed_index` at the maximum stage with `speed_frac > 0` (e.g., load_ratio = 1.0 requesting above-rated capacity). The capacity and EIR fields in `staging.rs:376-393` (`interpolated_capacity` / `interpolated_eir`) clamp within `capacities.len()` via `capacity_at_stage` / `eir_at_stage`, but the biquadratic interpolation path in `heater.rs` performs no equivalent clamping on the curve index.
**Impact**: At very high load conditions (load_ratio → 1.0), the last-stage EIR curve is silently evaluated as a capacity curve for the "beyond-rated" interpolation point. This degrades COP accuracy near maximum capacity, exactly the regime where building peak load simulations are most sensitive.

### Finding 3: [Severity: medium]
**Description**: MSHP auto-generated 4-speed stages share identical biquadratic capacity and EIR curves for all speeds. When a mini-split heating config provides only one capacity value, `heater.rs:654-689` auto-generates 4 evenly-spaced capacity stages but does not generate corresponding per-stage biquadratic curves — all 4 stages evaluate the same single pair of capacity/EIR coefficients. This means the temperature-dependent correction ratios are identical at every speed, diverging from OCHRE which loads distinct per-stage biquadratic parameters from `HVAC Multispeed Parameters.csv`.
**Code Location**:
- `crates/hares-equipment/src/hvac/heat_pump/heater.rs:654-689` — MSHP stage generation loop builds 4 capacities and 4 EIRs but does not replicate or scale the biquadratic coefficient array.
- `crates/hares-equipment/src/hvac/hvac_core.rs:3305-3335` (test) — tests acknowledge that default curves are single-speed only.
**Root Cause**: The auto-generation logic was designed to handle the case where HPXML provides a single rated heating capacity for MSHP equipment. Capacity and EIR per-stage values are computed from `min_compressor_fraction`, but the biquadratic curves (`heater.rs:1283`, `heater.rs:1334`) reference `biquadratic_coeffs` at `speed_index * 2` which — for 4 speeds — requires 8 coefficients (4 pairs). The init path in `hvac_core.rs:497-557` loads curves from config and substitutes defaults, but neither path ensures 4×2 = 8 entries exist. The fallback at `hvac_core.rs:733` (`self.config.biquadratic_coeffs.last()`) means all speeds 1-3 silently reuse the speed-0 curves.
**Impact**: Temperature-dependent COP for mini-split heaters is flattened across speeds, underestimating the efficiency benefit of running at part-load / lower speeds at moderate outdoor temperatures. Inverter-driven MSHPs typically show COP improvement at lower speeds, and this flattening produces a conservative (pessimistic) bias. Energy consumption at part-load is overestimated relative to OCHRE.

### Finding 4: [Severity: medium]
**Description**: `default_curves.rs` provides only single-speed default curve pairs. When HPXML supplies multi-speed capacity data without explicit multi-speed curves, the `maybe_substitute_defaults` path at `hvac_core.rs:554-557` replaces identity coefficients with 1-speed defaults (ASHP: 2 curves; MSHP: 2 curves), but the equipment may have >1 speed. The mismatch is not flagged beyond the default-vs-user distinction in `BiquadraticCurveSource`.
**Code Location**:
- `crates/hares-equipment/src/hvac/default_curves.rs:96-115` — `default_biquadratic_coeffs` returns at most 2 curves per equipment type.
- `crates/hares-equipment/src/hvac/hvac_core.rs:554-557` — `maybe_substitute_defaults` replaces identity with defaults regardless of speed count.
- `crates/hares-equipment/src/hvac/default_curves.rs:131-148` — `maybe_substitute_defaults` checks only identity vs. non-identity, not adequacy for the speed count.
**Root Cause**: The `default_biquadratic_coeffs` function was designed for the single-speed equipment init gap and was not extended when multi-speed equipment was added. The default store (`DefaultsStore` / `apply_multispeed_parameters`) handles multi-speed defaults from HPXML, but the `maybe_substitute_defaults` path predates this and runs independently.
**Impact**: Multi-speed equipment that enters the identity-substitution path (e.g., equipment whose HPXML has no explicit biquadratic curves) gets 1-speed defaults regardless of stage count. Combined with Finding 1, higher speeds fall back to the single EIR curve used as a capacity curve. This is a latent correctness issue for any HPXML import workflow that supplies multi-speed capacities without explicit per-speed biquadratic parameters.

### Finding 5: [Severity: low]
**Description**: Default curve operating bounds in `default_curves.rs` lack explicit per-equipment-type documentation of valid temperature ranges. The ASHP single-speed heating capacity curve (`ASHP_SINGLE_HEATING_CAPACITY`) is verified at AHRI H1 (21.1°C/8.3°C) and H3 (21.1°C/−8.3°C) conditions, but no guard ensures the curve is evaluated within its physically meaningful range. The generic default bounds `(-10, +50)` indoor and `(-50, +60)` outdoor at `hvac_core.rs:49-56` are fallback values, not curve-specific operating envelopes.
**Code Location**:
- `crates/hares-equipment/src/hvac/default_curves.rs:22-29` — ASHP capacity coefficients with AHRI verification comments but no per-curve bound documentation.
- `crates/hares-equipment/src/hvac/default_curves.rs:37-43` — ASHP EIR coefficients, same gap.
- `crates/hares-equipment/src/hvac/default_curves.rs:50-61` — MSHP coefficients, same gap.
- `crates/hares-equipment/src/hvac/hvac_core.rs:49-56` — default bounds apply globally, not per-curve.
**Comparison with OCHRE**: OCHRE's `initialize_biquad_params` (`HVAC.py:806-839`) reads `min_Twb`, `max_Twb`, `min_Tdb`, `max_Tdb` per speed stage from CSV files and passes them to `_biquadratic` for per-evaluation clamping. HARES uses a single pair of bounds for all curves in a given equipment instance (`biquadratic_x1_bounds` / `biquadratic_x2_bounds` on `HvacConfig`).
**Impact**: Low — the default bounds `(-50, +60)` outdoor are adequate for global residential use. However, when user-supplied HPXML curves come with tighter per-curve bounds, only two global bounds are propagated from config (via `biquadratic_x1_min/max` and `biquadratic_x2_min/max`), discarding per-speed boundary information present in the source data.

## Summary
- **Total findings**: 5
- **Critical**: 0
- **High**: 2 (Findings 1, 2)
- **Medium**: 2 (Findings 3, 4)
- **Low**: 1 (Finding 5)

## Recommendations
1. **[Finding 1 & 2]** Add an init-time validation in `hvac_core.rs` after curve loading that asserts `biquadratic_coeffs.len() >= max(heating_capacities_w.len(), cooling_capacities_w.len()) * 2`. Reject mismatched configs with a descriptive error, matching OCHRE's strict validation. Additionally, add a bounds check before each `evaluate_biquadratic_with_flow` call in hot paths that clamps `curve_index` to the valid even/odd range for capacity/EIR respectively, rather than falling back to `.last()` which can swap curve types.

2. **[Finding 3]** When auto-generating 4 MSHP stages from a single rated capacity, also replicate or scale the biquadratic coefficient arrays by the speed fraction so that each stage has a distinct, appropriate curve pair. As a minimum, replicate the existing single pair to 4 pairs (yielding identical correction ratios, which is the current behavior) but with correct indexing — so higher speeds don't fall through to the fallback path.

3. **[Finding 4]** Extend `default_biquadratic_coeffs` in `default_curves.rs` to accept a speed count parameter and return `ceil(speed_count / 2) * 2` interleaved entries (replicating the single-speed defaults as needed). Add a `tracing::warn!` when multi-speed equipment receives single-speed defaults.

4. **[Finding 5]** Consider adding per-curve bounds to the `BiquadraticCurve` struct alongside coefficients. When HPXML provides per-speed `min_Twb`/`max_Twb`/`min_Tdb`/`max_Tdb`, store them at load time and use per-curve bounds during evaluation. For the global generic bounds, add comments documenting the rationale: `(-50, +60)` outdoor covers the range of residential global weather; `(-10, +50)` indoor covers conditioned spaces.

## References / Citations
- OCHRE `HVAC.py:806-839` — `initialize_biquad_params`: loads per-speed speed-type-matched biquadratic parameters with individual per-curve bounds; raises exception on speed/count mismatch.
- OCHRE `HVAC.py:43-45` — `_biquadratic`: input clamping with `min(m, max(x, l), h)` pattern applied to `t_in`, `t_ext`, and `ff`.
- OCHRE `HVAC.py:990-1054` — `update_capacity` / `update_eir`: per-speed biquadratic evaluation with part-load ratio interpolation between bracket stages.
- EnergyPlus I/O Reference (Curve:Biquadratic) — expects bounded min/max per axis; HARES implements this via `BiquadraticCurve` struct in `biquadratic.rs:37-55`.
- AHRI Standard 210/240-2023 Table 1 — rated conditions used for curve verification in `default_curves.rs` tests.
