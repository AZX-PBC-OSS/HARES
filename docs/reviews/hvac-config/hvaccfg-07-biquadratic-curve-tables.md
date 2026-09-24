# Default biquadratic curve coefficient tables: capacity and EIR per speed stage
**Review ID**: hvaccfg-07
**Category**: hvac-config
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-equipment/src/hvac/default_curves.rs`
- `crates/hares-equipment/src/hvac/ac_config.rs` (cross-referenced for cooling defaults)
- `crates/hares-equipment/src/hvac/hvac_core.rs` (curve init, substitution, and evaluation)
- `crates/hares-physics/src/biquadratic.rs` (evaluation and clamping)

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/defaults/HVAC Heating/Biquadratic ASHP Heater.csv`
- `vendors/OCHRE/ochre/defaults/HVAC Heating/Biquadratic MSHP Heater.csv`
- `vendors/OCHRE/ochre/defaults/HVAC Cooling/Biquadratic Air Conditioner.csv`
- `vendors/EnergyPlus/src/EnergyPlus/CurveManager.cc` (lines 252–255: BiQuadratic evaluation; lines 242–243: input clamping; lines 3639–3664: `checkCurveIsNormalizedToOne`)
- `vendors/EnergyPlus/src/EnergyPlus/DXCoils.hh` (lines 71–78: rated-condition constants)
- `vendors/EnergyPlus/src/EnergyPlus/VariableSpeedCoils.cc` (lines 94–101: rated-condition constants; lines 392–570: per-speed curve loading)

## Findings

### Finding 1: [Severity: high]
**Description**: The MSHP variable-speed heating capacity curve (`MSHP_VARIABLE_HEATING_CAPACITY`) evaluates to **negative capacity ratios** at outdoor temperatures below approximately −30.2 °C. Because the default biquadratic X2 (outdoor) lower bound is −50 °C, the curve crosses zero well within the unclamped extrapolation region. At an outdoor temperature of −35 °C and indoor dry-bulb of 21.1 °C, the curve evaluates to **−0.1249** — a physically impossible negative capacity multiplier. At the lower bound of −50 °C, it evaluates to **−0.5143**.

**Code Location**: `crates/hares-equipment/src/hvac/default_curves.rs:50–51` — the MSHP heating capacity coefficients stored as `MSHP_VARIABLE_HEATING_CAPACITY`. Evaluated via the path: `hvac_core.rs:722–741` (`evaluate_biquadratic`) → `biquadratic.rs:37–55` (`BiquadraticCurve::evaluate`) with `x2_bounds = (−50.0, 60.0)`.

**Root Cause**: The MSHP heating curve is a linear function of outdoor DB only (`c = 0, e = 0, f = 0`), giving `cap_ratio = 1.0029 − 0.01039·T_indoor + 0.02596·T_outdoor`. At T_indoor = 21.1 °C, the outdoor-temperature coefficient (d = +0.2596) dominates and crosses zero at `T_outdoor = −(1.0029 − 0.2192) / 0.02596 ≈ −30.2 °C`. The curve was fitted by OCHRE with ±100 °C input bounds (see `MSHP Heater.csv` lines 23–26), and HARES’s −50 °C lower bound, while tighter than OCHRE’s −100 °C, is still wide enough to permit negative values. Neither OCHRE nor EnergyPlus clamps the curve *output* to a non-negative floor for this equipment category — output limits are optional user-specified fields in EnergyPlus `Curve:Biquadratic` (fields 11–12) and are absent from OCHRE’s MSHP Heater CSV.

**Impact**: During a cold-climate simulation with outdoor temperatures below −30 °C (e.g., Fairbanks, AK or Yellowknife, NT), the MSHP heating capacity ratio goes negative, inverting the sign of delivered heating. This would produce spurious cooling at a timestep where the thermostat is calling for heating — a physically nonsensical and numerically destabilizing result. The downstream code at `heater.rs:1304` clamps `steady_capacity_w` to 0 via `.max(0.0)`, which prevents negative delivered capacity from propagating to zone loads, but the capacity ratio itself (stored in telemetry as CAP_RATIO) remains negative — telemetry consumers would observe meaningless values.

### Finding 2: [Severity: medium]
**Description**: The default biquadratic curves in `default_curves.rs` are **heating-only**. The `default_biquadratic_coeffs` function returns `Some(...)` for `AshpHeatPumpOnly`, `AshpHeatPumpAux`, and `MiniSplitHeat`, but returns `None` for all cooling equipment types (`AcCooler`, `MiniSplitCool`, `GshpHeatPumpCooling`). Cooling default curves exist as separate constants in `ac_config.rs` (`DEFAULT_AC_CAPACITY_CURVE`, `DEFAULT_AC_EIR_CURVE`, `DEFAULT_ROOM_AC_CAPACITY_CURVE`, `DEFAULT_ROOM_AC_EIR_CURVE`) and are loaded by a separate code path (`ac_config::load_curve_pair`). This split creates a **gap**: the `maybe_substitute_defaults` mechanism in `default_curves.rs` cannot substitute cooling defaults if a cooling equipment config enters the generic init path with identity curves.

**Code Location**:
- `default_curves.rs:96–115` — `default_biquadratic_coeffs` returns `None` for all cooling types.
- `ac_config.rs:13–20` — cooling default constants exist but are not integrated with `maybe_substitute_defaults`.

**Root Cause**: The `default_biquadratic_coeffs` function was designed for the heating-only equipment init gap identified in ticket 010. Cooling equipment was added later with its own default loading path, but the generic init path (`hvac_core.rs:497–557`) and the cooling-specific path (`ac_config.rs:232–304`) are not unified.

**Impact**: Low. Under normal HPXML import, cooling curves are provided by `apply_multispeed_parameters` in `resolve_hvac.rs:2781–2830` or fall through to `DEFAULT_AC_CAPACITY_CURVE` / `DEFAULT_AC_EIR_CURVE` in `load_curve_pair`. The gap only affects configs that manually construct an `AcCooler` via the generic init path without providing any curve coefficients. In practice, the cooling path always goes through `load_curve_pair` which provides its own defaults.

### Finding 3: [Severity: medium]
**Description**: The **A-rated condition** (used by the review instruction) is incorrectly stated as "26.7 °C indoor WB." The correct ARI/AHRI 210/240-2023 rated condition for cooling is **indoor dry-bulb 26.7 °C (80 °F), indoor wet-bulb 19.44 °C (67 °F), outdoor dry-bulb 35.0 °C (95 °F)**. EnergyPlus (`DXCoils.hh:71–74`) defines `RatedInletAirTemp = 26.6667 °C` (DB) and `RatedInletWetBulbTemp = 19.4444 °C` (WB) separately. The biquadratic CAPFT and EIRFT curves use **indoor wet-bulb** (not dry-bulb) as the first independent variable for cooling. Evaluating the default AC capacity curve at 26.7 °C WB / 35 °C DB yields 1.378 (not 1.0), but this is spurious — the correct evaluation at 19.44 °C WB / 35.0 °C DB yields **0.994**, which is within EnergyPlus’s ±10 % tolerance band for `checkCurveIsNormalizedToOne`.

**Code Location**:
- `vendors/EnergyPlus/src/EnergyPlus/DXCoils.hh:71–74` — rated condition constants confirm WB = 19.44 °C as the independent variable.
- `ac_config.rs:13–14` — `DEFAULT_AC_CAPACITY_CURVE` coefficients.
- `air_conditioner.rs:1110–1114` — cooling curve evaluated with `x1 = coil_entering_wb_c` (indoor WB).

**Impact**: Informational only. No code defect exists here. The confusion arises because the industry rating point specifies both DB (26.7 °C) and WB (19.44 °C) for indoor air, and the CAPFT / EIRFT biquadratic curve formulation uses WB as the moisture-sensitive input. All HARES code paths correctly use indoor WB for cooling curves and indoor DB for heating curves.

### Finding 4: [Severity: low]
**Description**: All **coefficient values** in `default_curves.rs` match their **OCHRE CSV sources** exactly. Row-by-row verification against `vendors/OCHRE/ochre/defaults/HVAC Heating/Biquadratic ASHP Heater.csv` column `Single_1` and `vendors/OCHRE/ochre/defaults/HVAC Heating/Biquadratic MSHP Heater.csv` column `Variable_1` confirms zero transcription errors. The interleaved `[cap, eir]` layout is correctly consumed: `speed_index * 2` accesses capacity coefficients, `speed_index * 2 + 1` accesses EIR coefficients (see `heater.rs:1283–1334` and `air_conditioner.rs:1110–1121`). The coefficient order `[a, b, c, d, e, f]` matches EnergyPlus’s `Curve:Biquadratic` storage (`CurveManager.cc:253–255`).

**However**, the default curves provide only a **single pair** (one capacity + one EIR) per equipment type. The function `default_biquadratic_coeffs` returns `vec![CAPACITY_COEFFS, EIR_COEFFS]` regardless of how many speed stages the equipment has. For multi-speed equipment all speeds share the same curve pair; when `speed_index > 0`, the `evaluate_biquadratic` fallback at `hvac_core.rs:733` (`|| self.config.biquadratic_coeffs.last()`) silently reuses the same curve pair. This limitation is documented in finding 3 of the earlier review `equip-hvac-08-multi-speed-biquadratic-interleaving.md`.

**Code Location**: `default_curves.rs:96–115` — returns at most 2 entries.

**Impact**: Low for single-speed equipment (the primary target). For multi-speed heating equipment that passes through the identity-substitution path (non-HPXML configs), all speed stages evaluate the same temperature-dependent correction. This matches the OCHRE CSV pattern where `MSHP Heater.csv` has identical coefficients across `Variable_1` through `Variable_4` columns — so the flattening is present at the source-data level, not a HARES-specific defect.

### Finding 5: [Severity: low]
**Description**: No **runtime check** normalizes the default curves to **exactly 1.0** at rated conditions before use. EnergyPlus’s `checkCurveIsNormalizedToOne` (`CurveManager.cc:3639–3664`) issues a warning — not an error — if the curve output deviates by more than ±10 % from 1.0 at the rated condition, but does not rescale. HARES applies no equivalent warning or scaling. The computed values at rated conditions are all within tolerance:

| Curve | Rated condition | Computed value | Deviation |
|---|---|---|---|
| `ASHP_SINGLE_HEATING_CAPACITY` | H1 (21.1 °C DB / 8.3 °C DB) | 0.9951 | −0.49 % |
| `MSHP_VARIABLE_HEATING_CAPACITY` | H1 (21.1 °C DB / 8.3 °C DB) | 0.9993 | −0.07 % |
| `DEFAULT_AC_CAPACITY_CURVE` | E+ rated (19.44 °C WB / 35 °C DB) | 0.9936 | −0.64 % |
| `DEFAULT_AC_EIR_CURVE` | E+ rated (19.44 °C WB / 35 °C DB) | 1.0168 | +1.68 % |

Unit tests in `default_curves.rs:154–163` and `hvac_core.rs` verify H1 capacity is within 5 % of 1.0, but no equivalent verification exists for cooling defaults.

**Code Location**: `hvac_core.rs:497–557` — curve loading and substitution path; no normalization check. `default_curves.rs:154–163` — heating-only H1 test with ±5 % tolerance.

**Impact**: Trivially low for the default curves (all within 2 %). However, user-provided curves imported via HPXML could deviate substantially with no warning. The only scaling applied is the `HeatingCapacity17F` ratio correction in `heater.rs:835–887`, which correctly scales all capacity curves by a single multiplicative factor to match a manufacturer-specified low-ambient capacity ratio.

## Summary
- **Total findings**: 5
- **Critical**: 0
- **High**: 1 (Finding 1 — MSHP negative capacity ratio)
- **Medium**: 2 (Findings 2, 3)
- **Low**: 2 (Findings 4, 5)

## Recommendations
1. **[Finding 1]** Add a **non-negative output clamp** at the curve evaluation level for capacity curves. In `biquadratic.rs`, add an optional `output_min` field to `BiquadraticCurve` (analogous to EnergyPlus’s `outputLimits.min`) and set it to `0.0` for capacity curves. Alternatively, tighten the MSHP x2 lower bound to approximately −25 °C so the curve is clamped before crossing zero. The downstream `.max(0.0)` on `steady_capacity_w` in `heater.rs:1304` already protects delivered capacity, but telemetry and intermediate calculations remain corrupted.

2. **[Finding 2]** Move the cooling default curves from `ac_config.rs` into `default_curves.rs` and extend `default_biquadratic_coeffs` to cover all equipment types, unifying the default-curve dispatch in one place. Add cooling types (`AcCooler`, `MiniSplitCool`, `GshpHeatPumpCooling`) to the `match` statement with the correct per-type defaults.

3. **[Finding 5]** Add an init-time rated-condition verification similar to EnergyPlus’s `checkCurveIsNormalizedToOne`. Evaluate each curve at its equipment-type-specific rated condition and emit a `tracing::warn!` if the deviation exceeds 10 %. This would surface faulty user-provided manufacturer curves before simulation begins.

4. **[Documentation]** Clarify in the `default_curves.rs` module doc that these curves are heating-only and reference `ac_config.rs` for the cooling defaults. Document the rated condition used for each curve type (AHRI H1 for heating, E+ 19.44 °C WB / 35 °C DB for cooling) with the computed value at that point.

## References / Citations
- EnergyPlus `CurveManager.cc:252–255` — BiQuadratic evaluation formula: `coeff[0] + V1*(coeff[1] + V1*coeff[2]) + V2*(coeff[3] + V2*coeff[4]) + V1*V2*coeff[5]`
- EnergyPlus `CurveManager.cc:242–243` — input clamping (piecewise constant continuation/bounded evaluation)
- EnergyPlus `CurveManager.cc:282–287` — optional output limits (`minPresent` / `maxPresent`)
- EnergyPlus `CurveManager.cc:3639–3664` — `checkCurveIsNormalizedToOne` (±10 % tolerance band)
- EnergyPlus `DXCoils.hh:71–78` — rated-condition constants (cooling: 26.67 °C DB, 19.44 °C WB indoor; 35.0 °C DB outdoor)
- EnergyPlus `VariableSpeedCoils.hh:182–193` — per-speed curve index arrays (`MSCCapFTemp`, `MSEIRFTemp`)
- OCHRE `HVAC.py:806–839` — `initialize_biquad_params`: per-speed curve loading from CSV with `min_Twb`/`max_Twb`/`min_Tdb`/`max_Tdb` bounds
- OCHRE `Biquadratic ASHP Heater.csv` — Single_1 column: cap = `[0.878143655, −0.002914855, −0.00003337, 0.022386661, 0.000163944, −0.00002187]`; eir = `[0.716518071, 0.010275901, 0.000460734, −0.006480365, 0.000456354, −0.00069764]`
- OCHRE `Biquadratic MSHP Heater.csv` — Variable_1 column: cap = `[1.002928121, −0.010386676, 0, 0.025961538, 0, 0]`; eir = `[0.966475473, 0.00591495, 0.000191202, −0.012965668, 0.00004225, −0.000524003]`
- OCHRE `Biquadratic Air Conditioner.csv` — Single_1 column: cap = `[1.5509, −0.07505, 0.0031, 0.0024, −0.00005, −0.00043]`; eir = `[−0.30428, 0.11805, −0.00342, −0.00626, 0.0007, −0.00047]`
- AHRI Standard 210/240-2023 Table 1 — H1 heating test (21.1 °C DB indoor / 8.3 °C DB outdoor); cooling rated (26.7 °C DB / 19.44 °C WB indoor / 35 °C DB outdoor)
- `docs/reviews/equipment-hvac/equip-hvac-08-multi-speed-biquadratic-interleaving.md` — prior review documenting multi-speed interleaving gaps and single-speed-only default curve limitation
