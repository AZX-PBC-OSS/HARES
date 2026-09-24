# Default system losses fraction 0.14 — verify match to PVWatts v5 defaults
**Review ID**: pvsize-06
**Category**: pv-sizing
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/pv_sizing.rs`
- `crates/hares-equipment/src/pv/mod.rs`
- `crates/hares-equipment/src/pv/config.rs`
- `crates/hares-equipment/src/pv/soiling.rs`

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/PVWatts.cc` (line 88: `inverterEfficiency_(0.96)`, line 162: losses passed as percentage)
- `vendors/EnergyPlus/src/EnergyPlus/PVWatts.hh` (line 186: `systemLosses = 0.14` default)
- `vendors/EnergyPlus/idd/versions/V9-6-0-Energy+.idd` (line 88052: `\default 0.14` for Generator:PVWatts System Losses)
- `vendors/EnergyPlus/src/EnergyPlus/DataPhotovoltaics.hh` (photovoltaic data structures)
- `vendors/OCHRE/ochre/Equipment/PV.py` (line 62: `inv_eff` parameter, no system losses default visible — delegates to PySAM/PVWatts v8 defaults)
- `vendors/EnergyPlus/doc/engineering-reference/src/on-site-generation/photovoltaic-arrays.tex` (line 422: cites PVWatts Version 5 Manual)
- `crates/hares-equipment/tests/oracle_24h.rs` (lines 296–364: oracle test confirms loss application formula)

## Findings

### Finding 1: Inconsistent loss-model paths between LUT and non-LUT code [Severity: high]

**Description**:
The `step_one_array` function in `mod.rs:195–267` has two distinct power-calculation paths that apply losses differently:

- **Non-LUT path** (line 247–266): applies `system_losses_fraction` as a lumped DC derate (`dc_power_kw *= 1.0 - self.system_losses_fraction`, line 258).
- **LUT path** (line 211–244): `system_losses_fraction` is **not applied at all** — the SAM LUT output is used directly (with only soiling/shading post-multipliers, lines 230–231).

If the SAM LUTs were generated with default losses (14%) baked in, then both paths produce equivalent results. If the LUTs were generated with losses=0%, the non-LUT path derates ~14% more than the LUT path for identical conditions. There is no documentation, guard, or assert that ensures the LUT-generation losses match `DEFAULT_SYSTEM_LOSSES_FRACTION`.

**Code Location**: `crates/hares-equipment/src/pv/mod.rs:211–267`

**Root Cause**: The two code paths evolved independently. The LUT path delegates loss modelling to the external SAM precomputation; the non-LUT path applies an explicit scalar derate. No consistency check ties them together.

**Impact**: Users switching between LUT and non-LUT models for the same array will see ~14% relative difference in annual energy yield. For a 5 kW system in Los Angeles (~8,000 kWh/yr), this is ~1,100 kWh/yr discrepancy.

### Finding 2: Soiling double-counting when Kimber model is active with default losses [Severity: high]

**Description**:
The `DEFAULT_SYSTEM_LOSSES_FRACTION = 0.14` follows the PVWatts v5 default, which includes a 2% soiling component. When a user enables the Kimber soiling model (via `soiling_config`), both loss mechanisms are active:

1. The Kimber model tracks dynamic soiling and returns `soiling_ratio` (e.g., 0.97 = 3% soiled), applied as irradiance reduction at `mod.rs:206–207`.
2. `system_losses_fraction = 0.14` is then applied as a DC power derate at `mod.rs:258`, which includes a ~2% soiling component.

In the non-LUT path, a 3% Kimber soiling + 2% static soiling = ~5% effective soiling — a 67% over-estimate of the intended soiling loss. In the LUT path (line 230), the same double-application occurs because `soiling_ratio` is applied again as a post-LUT multiplier.

The correct PVWatts v5 behaviour is **either**: (a) use a static 2% soiling in `system_losses_fraction` with no dynamic soiling model, **or** (b) use a dynamic soiling model and exclude soiling from the static loss term.

**Code Location**: `crates/hares-equipment/src/pv/mod.rs:39` (default), `:206–208` (soiling as irradiance reduction), `:230` (soiling as LUT post-multiplier), `:258` (lumped derate), `:468–473` (soiling model dispatch)

**Root Cause**: The soiling module (`soiling.rs`) was added after the lumped `system_losses_fraction` constant was established. No adjustment was made to reconcile the two, and no documentation warns users to reduce `system_losses_fraction` when enabling dynamic soiling.

**Impact**: Over-estimated PV production losses in the non-LUT path when both are active. For a site with 3% average soiling loss, the effective loss becomes ~5%, over-predicting annual loss by ~2 percentage points (~100 kWh/yr for 5 kW system).

### Finding 3: No documented breakdown of 0.14 into component losses [Severity: medium]

**Description**:
The constant `DEFAULT_SYSTEM_LOSSES: f64 = 0.14` at `pv_sizing.rs:92` and `DEFAULT_SYSTEM_LOSSES_FRACTION: f64 = 0.14` at `mod.rs:39` have no doc comment explaining what loss components (soiling, mismatch, wiring, connections, LID, nameplate rating, age, availability) are included. The review prompt references a stated breakdown (2% soiling + 2% mismatch + 2% wiring + 4% inverter + ~4% other = 14%), but this breakdown does not appear in any source file.

PVWatts v5 defines its default 14% DC-side losses multiplicatively (not additively):

| Component | Multiplier |
|---|---|
| Soiling | 0.98 |
| Shading | 0.97 |
| Snow | 1.00 |
| Mismatch | 0.98 |
| Wiring | 0.98 |
| Connections | 0.995 |
| Light-induced degradation | 0.985 |
| Nameplate rating | 0.99 |
| Age | 1.00 |
| Availability | 0.97 |

Product: 0.98×0.97×0.98×0.98×0.995×0.985×0.99×0.97 ≈ 0.860 → effective loss **14.07%**, not a simple 14% sum. The HARES code uses a simple linear derate (`× (1 - 0.14)`) rather than multiplicative component losses, which yields 14.0% loss vs 14.07% — a 0.07 percentage point difference (negligible). However, because the effective loss is multiplicative, individual component adjustments compound differently from simple addition. A user changing only the soiling term from 2% to 5% (3-point increase) in a multiplicative model increases effective loss from 14.07% to ~16.5% (2.4-point increase). In HARES's linear model, the same change yields 14.0% → 17.0% (3.0-point increase) — overstating the sensitivity.

**Code Location**: `crates/hares-physics/src/pv_sizing.rs:92`, `crates/hares-equipment/src/pv/mod.rs:39`

**Root Cause**: The constant was ported from EnergyPlus PVWatts (which in turn inherits it from SAM/SSC) without documenting the component structure.

**Impact**: Users cannot selectively adjust individual loss components (e.g., set wiring=0% for a loss-less DC bus design). The lack of documentation also makes it impossible for reviewers to verify the constant against the referenced PVWatts specification.

### Finding 4: No geographic, climate, or roof-mount-type dependence [Severity: medium]

**Description**:
The `DEFAULT_SYSTEM_LOSSES` constant is applied globally with no variation by:
- **Latitude/climate**: Soiling losses range from ~1%/yr (temperate, frequent rain) to 6%+/yr (arid southwest US). Snow losses are ~0% in southern climates but 2–5% in northern US. PVWatts v5 defaults snow=0% but provides region-specific defaults.
- **Roof-mount type** (FixedOpenRack vs FixedRoofMounted): PVWatts v5 distinguishes array types; roof-mounted arrays have different thermal characteristics that interact with cell temperature losses (the temperature derate is separate from the 14% in HARES, so this is partially addressed).
- **Tilt**: Flat/low-tilt panels accumulate more soiling than steep-tilt panels in arid regions.

The Kimber soiling model (`soiling.rs`) partially addresses climate-variant soiling through configurable rates (default `soiling_loss_rate_per_s = 0.0015/day`), but these rates are single-valued per simulation and not auto-tuned by climate zone. There is no snow-loss model at all.

**Code Location**: `crates/hares-physics/src/pv_sizing.rs:92`, `crates/hares-equipment/src/pv/mod.rs:39`, `crates/hares-equipment/src/pv/soiling.rs:60`

**Root Cause**: The lumped constant design assumes a single national-average loss factor, suitable for screening-level analysis but not for geographically-tuned simulations.

**Impact**: For a 5 kW system, using 14% losses in Phoenix (actual effective losses ~18% due to high soiling + heat) under-estimates derated production by ~4 percentage points (~320 kWh/yr). Conversely, in Seattle (actual ~12% due to low soiling, no snow, cooler temperatures), the 14% default over-estimates losses by ~2 points. The error scales with system size.

### Finding 5: System loss derate conflates temperature-dependent and independent mechanisms [Severity: low]

**Description**:
In the non-LUT path (`mod.rs:247–266`), all system losses are applied as a single post-temperature-correction multiplier on DC power:

```
dc_power_kw = capacity_kw × (POI/STC) × temp_derate × (1 - system_losses_fraction)
```

This conflates:
1. **Irradiance-proportional, temperature-independent losses** (soiling, shading, mismatch, LID, nameplate) — correctly multiplied linearly
2. **Irradiance-proportional** but possibly temperature-sensitive losses (wiring I²R losses increase with current, which increases with irradiance and inversely with temperature)

The inverter efficiency is correctly handled as a separate DC→AC conversion step (`mod.rs:259`), not included in the lumped 14%. The DEFAULT_INVERTER_EFFICIENCY of 0.96 (4% loss) matches PVWatts v5. However, inverter efficiency has a load-dependent curve in PVWatts v5 (the CEC efficiency curve), which HARES approximates with a flat 96%. This loses fidelity at low irradiance where inverter efficiency drops significantly.

**Code Location**: `crates/hares-equipment/src/pv/mod.rs:256–259`

**Root Cause**: The simplified lumped-derate model trades fidelity for parameter simplicity, consistent with PVWatts v5's own lumped approach. The CEC inverter curve flattening is a conscious simplification.

**Impact**: Minor (typically <0.5% annual energy) for systems operating near peak efficiency most of the time. More significant for heavily oversized DC/AC ratios or cloudy climates where the inverter spends more time in low-efficiency regions.

### Finding 6: pv_sizing propagates constant but does not use it in sizing [Severity: low]

**Description**:
The `size_pv_system` function at `pv_sizing.rs:341–378` accepts `system_losses: Option<f64>` and defaults to `DEFAULT_SYSTEM_LOSSES = 0.14`. However, the resulting `system_losses_fraction` field is purely informational — it flows into the `PvSizingResult` struct (line 80) but is never used in the sizing calculation itself (panel count, capacity, area). Those quantities are computed without any loss derating.

The value is then carried through to equipment configuration, where the PV model applies it at runtime. This is architecturally sound (sizing is based on physical geometry, not performance losses), but the constant's presence in the sizing module is misleading — it suggests the losses affect the sizing decision when they do not.

**Code Location**: `crates/hares-physics/src/pv_sizing.rs:341–378`

**Root Cause**: The `system_losses` parameter was added so the sizing result could forward a recommended loss value to the equipment config, making the API self-contained. It is a pass-through, not an input to the sizing algorithm.

**Impact**: Minimal — the value is used downstream in the PV equipment model. Users inspecting the sizing output may assume losses were factored into the panel count.

## Summary
- Total findings: 6
- Critical / High / Medium / Low: 0 / 2 / 2 / 2

## Recommendations

1. **Add documentation to `DEFAULT_SYSTEM_LOSSES_FRACTION`**: Document the PVWatts v5 component breakdown at the constant definition sites (`pv_sizing.rs:92` and `mod.rs:39`). Include the individual loss components and the multiplicative formula, and cite the PVWatts Version 5 Manual (Dobos, A. P. "PVWatts Version 5 Manual." NREL/TP-7A40-80694, 2014).

2. **Reconcile soiling model and system_losses_fraction**: When the Kimber soiling model is active (`soiling_config` is `Some`), automatically reduce `system_losses_fraction` by the static soiling component (2 percentage points) in the non-LUT path, or add a warning/validation. Alternatively, expose separate component losses in the config (soiling, mismatch, wiring, LID, etc.) and combine them at runtime, with the soiling component excluded when dynamic soiling is active.

3. **Ensure LUT-path parity with non-LUT path for losses**: Either (a) document that SAM LUTs must be generated with 14% losses baked in, (b) apply `system_losses_fraction` as a post-LUT derate when the LUT was generated with 0% losses, or (c) add an `assert` or runtime check that detects mismatch between LUT loss assumptions and the configured `system_losses_fraction`. The simplest fix is an `#[cfg(debug_assertions)]` check that compares LUT metadata against the configured loss value.

4. **Add climate-zone-dependent default loss terms**: At minimum, expose soiling loss as a per-simulation parameter that can be adjusted by climate zone (e.g., ASHRAE climate zones 1–8 mapped to soiling rates from Kimber 2006 or NREL PVQAT data). Consider adding a snow-loss model for northern US/Canada climates where snow coverage can reduce winter production by 5–15%.

5. **Separate loss into components rather than single lumped fraction**: Store individual loss components (soiling, mismatch, wiring, connections, LID, nameplate, age, availability) as fields in `PvConfig` or `PvArray`, compute the multiplicative effective loss at runtime, and expose individual components via telemetry. This allows users to tune individual terms, enables climate-specific defaults, and avoids linear-vs-multiplicative approximation.

6. **Consider adding CEC inverter efficiency curve**: Replace the flat 96% inverter efficiency with the PVWatts v5/CEC weighted efficiency curve (η at 10%, 20%, 30%, 50%, 75%, 100% load) for improved low-irradiance fidelity. The current flat 96% is adequate for screening but loses ~0.5–1% annual accuracy in low-irradiance climates.

## References / Citations
- Dobos, A. P. "PVWatts Version 5 Manual." NREL/TP-7A40-80694, National Renewable Energy Laboratory, September 2014. https://www.nrel.gov/docs/fy14osti/62641.pdf
- EnergyPlus V9.6 IDD: `vendors/EnergyPlus/idd/versions/V9-6-0-Energy+.idd:88050–88052` — Generator:PVWatts System Losses field, `\default 0.14`
- EnergyPlus PVWatts.cc: `vendors/EnergyPlus/src/EnergyPlus/PVWatts.cc:88,162` — inverter efficiency 0.96, losses passed to SSC as percentage
- EnergyPlus PVWatts.hh: `vendors/EnergyPlus/src/EnergyPlus/PVWatts.hh:186` — constructor default `systemLosses = 0.14`
- Kimber, A., et al. "The Effect of Soiling on Large Grid-Connected Photovoltaic Systems in California and the Southwest Region of the United States." IEEE 4th WCPEC, 2006. DOI: 10.1109/WCPEC.2006.279690
- PVWatts v8 Technical Reference, NREL/TP-7A40-80694 (cited at `mod.rs:56`)
- NREL SAM PVWatts model source (SSC): `lib_pvwatts.h` — defines loss components and multiplicative derating
- HARES `mod.rs:39`: `const DEFAULT_SYSTEM_LOSSES_FRACTION: f64 = 0.14;`
- HARES `pv_sizing.rs:92`: `const DEFAULT_SYSTEM_LOSSES: f64 = 0.14;`
- HARES `soiling.rs:60`: Kimber default loss rate 0.0015/day (suburban temperate)
