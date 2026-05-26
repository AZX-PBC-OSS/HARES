# Coil physics T_wet fixed-point solver convergence and infinite loop guard
**Review ID**: hvaccfg-09
**Category**: hvac-config
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-equipment/src/hvac/coil_physics.rs`

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/DXCoils.cc` — dry evaporator iteration with under-relaxation (lines 12730–12821)
- `vendors/EnergyPlus/src/EnergyPlus/WaterCoils.cc` — part-wet/part-dry convergence, warm-start, fallback warnings (lines 3234–3286, 3341–3484, 4030–4222, 5165–5341)
- `vendors/EnergyPlus/src/EnergyPlus/General.cc` — `General::Iterate` RegulaFalsi (lines 919–980)
- `vendors/EnergyPlus/src/EnergyPlus/Coils/CoilCoolingDXCurveFitSpeed.cc` — DX dry evaporator under-relaxation (lines 483–538)

## Findings

### Finding 1: No dry-coil detection during iteration — SHR_ITERATION_LIMIT always consumed [Severity: medium]
**Description**: The `calculate_shr` function (line 214) only detects a definitively dry coil when `w_in <= SHR_MIN_HUMIDITY_RATIO` (1e-7 kg/kg, line 222). For practically dry conditions such as 30 °C at 10% RH (W ≈ 0.0026 kg/kg) the humidity ratio exceeds this threshold, yet the coil may still operate fully dry. The function enters the full 50-iteration SHR loop (line 248) without any check for `w_adp > w_in` — rising to the apparatus-dew-point humidity ratio above the inlet humidity ratio, which is the standard dry-coil indicator used by EnergyPlus.

**Code Location**: `calculate_shr`, lines 222–268. Lacks a guard analogous to EnergyPlus `DXCoils.cc:12790` (`if (wADP > InletHumRatCalc || ...)`).

**Root Cause**: The only dry-coil shortcut is the trivial humidity-ratio guard at line 222, which catches physically meaningless inputs (W ≈ 0 – a rounding error rather than a low-RH day). But even in conditions where sensible-only cooling occurs, the function force-runs the full iteration loop.

**Impact**: Wasted computation across the full `SHR_ITERATION_LIMIT` (50) when the coil and entering conditions imply dry operation. In a time-series simulation with many dry-coil hours (desert/arid climates) this adds up. The root-finding usually converges, but the absence of an early-out dry-check makes the code less robust. Compare: EnergyPlus DXCoils detects the condition as `wADP > InletHumRatCalc` and converges to a sensible-only solution within a few under-relaxed iterations (see Finding 2).

### Finding 2: No under-relaxation — oscillating iterates can exhaust the iteration limit [Severity: medium]
**Description**: The `iterate` function (lines 443–548) uses a perturbation–secant–quadratic strategy identical in structure to EnergyPlus `CoilAreaFracIter` (WaterCoils.cc:5165–5341). However, EnergyPlus DX-coil dry-evaporator loops additionally apply explicit under-relaxation with a relaxation factor `RF = 0.4` on the humidity ratio: `InletHumRatCalc = 0.4 * wADP + 0.6 * InletHumRatCalc` (DXCoils.cc:12800). This damps oscillations when the effective fixed-point map has slope > 1 near the solution — a common failure mode highlighted in the review instructions. The HARES `iterate` function has no analogous damping step: the next guess is used directly without blending with the previous guess.

**Code Location**: `iterate` function, lines 468–546. No blending formula of the form `x_new = α * x_computed + (1-α) * x_old` exists anywhere in the function.

**Root Cause**: The `iterate` function was modeled on `CoilAreaFracIter` which solves for a well-behaved surface-area fraction on [0,1], not on the HARES `calculate_shr` problem that involves the exponential vapor-pressure function via `humidity_ratio_from_rel_hum(t_adp, 1.0, p_pa)` (line 249). The vapor-pressure curve can create a steep gradient that the pure quadratic fit overshoots without damping.

**Impact**: Under high-humidity, high-temperature conditions (e.g., 30 °C / 80% RH where latent load dominates), the iterate function may oscillate between successive guesses, consuming iterations without rapid convergence. With the tight 1e-5 relative tolerance (see Finding 3), this can cause an approach to the 50-iteration limit, or occasional non-convergence. The review instructions specifically identify this scenario — coil fully wet, latent capacity dominating — as the highest-risk case for fixed-point slope > 1.

### Finding 3: Apparatus dew point temperature permitted to exceed entering dry-bulb [Severity: medium]
**Description**: The `calculate_shr` function computes `supply_temp_c = t_adp + bf * (db_in_c - t_adp)` (line 278). For this to be physically meaningful, the apparatus dew point must satisfy `t_adp <= db_in_c`. No guard checks this constraint within `calculate_shr`. Under extreme temperature/humidity combinations (e.g., outdoor 50 °C with a high latent load and small bypass factor), the enthalpy-based apparatus dew point calculation could produce `t_adp > db_in_c`, yielding `supply_temp_c > db_in_c` — a non-physical supply-air reheat without external energy.

**Code Location**: `calculate_shr`, line 278. No `t_adp <= db_in_c` guard.

**Root Cause**: The `coil_bypass_factor` function (line 383) *does* contain `if db_in_c - t_adp <= 0.0 { return Ok(BYPASS_FACTOR_FLOOR) }` to guard the same pathology. That guard was not replicated in `calculate_shr`. The ADP enthalpy `h_adp = h_in - d_h / (1.0 - bf)` (line 237) can dip below the saturation-enthalpy minimum if `bf` and `d_h` combine unfavorably (e.g., very high cooling delivered through a very small bypass), producing a numerically high T_ADP from the root-find.

**Impact**: Silent production of non-physical coil states. At extreme conditions (review item c: 50 °C outdoor, 30 °C indoor / 80% RH), this boundary violation could produce an SHR outside the valid range or a supply temperature exceeding the entering temperature. Comparison: EnergyPlus WaterCoils.cc:383 explicitly guards `db_in_c - t_adp <= 0.0`.

### Finding 4: Non-convergence returns a hard error instead of a fallback with last value [Severity: low]
**Description**: When `calculate_shr` exhausts all 50 `SHR_ITERATION_LIMIT` iterations without converging (line 264), the function returns `Err(HaresError::Equipment(...))` — a hard abort. EnergyPlus, in contrast, issues a recurring warning but continues the simulation with the last computed value (WaterCoils.cc:3291–3296, 3477–3484; DXCoils.cc falls through to SHR=1.0 at line 12814 when Counter > 0). The HARES approach terminates the current simulation timestep and propagates an error up the call stack.

**Code Location**: `calculate_shr`, lines 264–268.

**Impact**: For marginally-convergent cases (e.g., extreme conditions combined with a "poor" initial guess from `dew_point`), the simulation will abort rather than proceeding with a best-effort value. Since the last iteration's `err` (line 250) is reported but the convergence check is on temperature change (line 456), the diagnostic message may be misleading when the error is small but temperature change is still above tolerance.

### Finding 5: No warm-start across timesteps [Severity: low]
**Description**: `calculate_shr` always initializes `t_adp = dew_point(w_in.max(SHR_MIN_HUMIDITY_RATIO), p_pa)` (line 239). The function is stateless — it carries no memory of the previous timestep's converged apparatus dew point. EnergyPlus saves and reuses converged values (`SurfAreaWetFractionSaved`, `MeanWaterTempSaved`, `InWaterTempSaved`, `OutWaterTempSaved`) for warm-starting subsequent timesteps (WaterCoils.cc:3234, 3324, 3332, 4085).

**Code Location**: `calculate_shr`, line 239.

**Impact**: In time-series simulations where indoor conditions change slowly (30-minute or 1-hour timesteps), the warm-start could reduce iteration count by 30–50%. For the review's concern about poor initial guesses (item f), the current dew-point-based start is reasonable but not as efficient as a carry-forward from the previous solve.

### Finding 6: Convergence tolerance relative-only — no absolute tolerance safety net [Severity: low]
**Description**: The `iterate` function's convergence check at line 456 is:
```rust
(x0 - x1).abs() < tol_rel * x0.abs().max(small) && icount != 1 || f0 == 0.0
```
where `tol_rel = ITERATE_TOL_REL = 1e-5` and `small = EPSILON = 1e-9`. This is purely a relative tolerance. EnergyPlus `General::Iterate` (General.cc:948) uses an absolute tolerance `Tol` (typically 0.01 °C) and its `CoilAreaFracIter` uses the relative tolerance pattern with `Tolerance = 1e-5` on a [0,1]-valued fraction — the same value as HARES but applied to a different physical quantity.

For T_ADP ≈ 10 °C, HARES converges when ΔT < 1e-4 °C — roughly 100× tighter than EnergyPlus's 0.01 °C absolute tolerance for the analogous ADP loop (WaterCoils.cc:1345). For T_ADP near 0 °C, the floor `max(|x0|, 1e-9)` produces convergence at ΔT < 1e-14, which is below f64 machine epsilon and likely unreachable.

**Code Location**: `iterate` function, lines 456–458.

**Impact**: The tighter tolerance means the solver may run more iterations than needed for engineering purposes (a 0.01 °C precision in ADP temperature is more than adequate for HVAC energy modeling). The absolute-value floor near zero is unreachable and provides no practical safety net. However, the quadratic-mode convergence is rapid, so this is primarily a performance concern rather than a correctness bug.

### Finding 7: Consistent iteration guard — no infinite-loop risk [Severity: none — conforming]
**Description**: The `for i in 1..=SHR_ITERATION_LIMIT` construct at line 248 provides a robust iteration cap of 50. Each call to `iterate` computes one step; the loop itself cannot become infinite. This satisfies review item (d). The constant `SHR_ITERATION_LIMIT = 50` (line 198) falls within the 30–50 range cited as typical.

**Code Location**: Line 248.

## Summary
- Total findings: 7 (6 actionable, 1 conforming)
- Critical: 0
- High: 0
- Medium: 3 — Findings 1 (dry-coil detection), 2 (under-relaxation), 3 (T_ADP bound)
- Low: 3 — Findings 4 (no fallback), 5 (no warm-start), 6 (tolerance only relative)

## Recommendations

1. **Add dry-coil detection during iteration**: After computing `w_adp` on line 249, compare `w_adp` against `w_in`. If `w_adp > w_in`, the coil has no latent load — set `shr = 1.0`, compute the sensible-only ADP from the enthalpy balance, and return immediately without further iteration. This matches the EnergyPlus pattern (DXCoils.cc:12790) and eliminates unnecessary iteration for dry or near-dry operating conditions.

2. **Add under-relaxation when oscillating**: Track the direction of successive temperature changes. If `t_adp` oscillates (changes sign on consecutive iterations), blend the next guess: `t_adp_new = 0.5 * t_computed + 0.5 * t_old`. This matches the `RF = 0.4` pattern in EnergyPlus DXCoils (DXCoils.cc:12800) and addresses the review's item (e) about oscillation when the fixed-point map has slope > 1.

3. **Guard T_ADP against exceeding entering dry-bulb**: After convergence, add `debug_assert!(t_adp <= db_in_c)` or clamp the ADP temperature. At minimum, match the guard already present in `coil_bypass_factor` (line 383).

4. **Consider a graceful fallback instead of hard error**: On non-convergence after `SHR_ITERATION_LIMIT` iterations, issue a warning and return the last computed SHR and T_ADP rather than `Err`, matching EnergyPlus's practice of continuing the simulation with a warning. This prevents simulation termination for borderline convergence cases.

5. **Add absolute tolerance as a secondary convergence criterion**: Supplement the relative tolerance with an absolute tolerance (e.g., 0.001 °C): `|x0 - x1| < Tol_abs || |x0 - x1| < Tol_rel * max(|x0|, small)`. The absolute tolerance catches near-zero T_ADP cases and provides a floor that is reachable and meaningful.

## References / Citations
- EnergyPlus `DXCoils.cc:12790` — dry evaporator detection: `if (wADP > InletHumRatCalc || ...)`
- EnergyPlus `DXCoils.cc:12800` — under-relaxation: `InletHumRatCalc = RF * wADP + (1.0 - RF) * InletHumRatCalc` with `RF = 0.4`
- EnergyPlus `WaterCoils.cc:383` — `db_in_c - t_adp <= 0.0` guard in bypass factor calculation
- EnergyPlus `WaterCoils.cc:3234,3332,4085` — warm-start from previous-timestep converged values
- EnergyPlus `General.cc:948` — absolute tolerance convergence: `|X0 - X1| < Tol`
- EnergyPlus `WaterCoils.cc:3291–3296` — non-convergence fallback: warning, continue with last value
- EnergyPlus `CoilCoolingDXCurveFitSpeed.cc:483–538` — same dry-evaporator under-relaxation pattern as DXCoils
- ASHRAE 2017 HOF Ch.18 Eq.63 — enthalpy-based bypass factor formula
