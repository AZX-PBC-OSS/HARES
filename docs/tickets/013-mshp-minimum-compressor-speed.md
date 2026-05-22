# MSHP Minimum Compressor Speed Not Configurable

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-equipment/hvac/heat_pump, hares-equipment/hvac

## Problem

Real mini-split heat pumps have a minimum compressor speed (typically 25–40% of rated capacity). HARES hardcodes MSHP speed stages at 25%/50%/75%/100% with no way to configure the minimum. This matters because:

1. **Overheating at low load** — when the zone needs only 15% of rated capacity, a 25% minimum stage will overshoot, causing thermostat cycling (on/off) instead of continuous low-speed operation. Real MSHPs with 30% minimum would have the same issue, but the behavior should be configurable.

2. **COP at minimum speed differs from rated** — at minimum compressor speed, COP is typically higher than at rated speed (lower pressure ratio). HARES uses the same EIR for all 4 stages (`heater.rs:524`), which is incorrect for units where minimum-speed COP is significantly different.

3. **Different MSHP manufacturers have different minimums** — some inverter-driven units operate down to 20% while others bottom out at 40%. A fixed 25% assumption is not generalizable.

## Current Behavior

1. **MSHP speed generation** in `heater.rs:517-525`:
   ```rust
   if cfg.is_mini_split || matches!(self.variant, HeaterVariant::Minisplit) {
       self.hvac.speed_control_mode = SpeedControlMode::VariableSpeedIdeal;
       if self.hvac.heating_capacities_w.len() == 1 {
           let base_cap = self.hvac.heating_capacities_w[0];
           let base_eir = self.hvac.eir_by_stage[0];
           self.hvac.heating_capacities_w =
               vec![base_cap * 0.25, base_cap * 0.5, base_cap * 0.75, base_cap];
           self.hvac.eir_by_stage = vec![base_eir; 4];
       }
   ```
   The 25% minimum is hardcoded in `base_cap * 0.25`. The 4-stage spacing (25/50/75/100) is also hardcoded.

2. **MSHP cooling path** — the MSHP cooler does NOT generate speed stages itself; it delegates to the inner `AirConditioner` via `typed_hp_to_central_ac_config` in `cooler.rs:104-172`, which then calls `AirConditioner.init`. The cooling-side speed generation happens in `AirConditioner.init` with the same hardcoded fractions.

3. **No config field** — `HeatPumpHeaterConfig` (`heat_pump_config.rs:20-139`) has no `min_compressor_fraction` field. The only speed-related field is `number_of_speeds`, which is forced to 4 for mini-splits.

4. **Same EIR for all stages** — `eir_by_stage = vec![base_eir; 4]` assumes constant EIR across all speeds. Real MSHPs have higher COP (lower EIR) at part-load speeds.

## Required Behavior

1. **Configurable minimum compressor fraction** — a `min_compressor_fraction: f64` field (default: 0.25) in `HeatPumpHeaterConfig` and `HeatPumpCoolerConfig`.

2. **Speed stages derived from min and max** — given `min_fraction` and rated capacity, generate N evenly-spaced stages:
   ```
   stage[i] = rated * (min_fraction + (1.0 - min_fraction) * i / (n_stages - 1))
   ```
   For `min_fraction = 0.25, n_stages = 4`: produces [0.25, 0.50, 0.75, 1.00] (same as current).
   For `min_fraction = 0.30, n_stages = 4`: produces [0.30, 0.533, 0.767, 1.00].

3. **Per-stage EIR option** — when `stage_heating_eirs` is provided, use it; otherwise, derive part-load EIR from a simple model:
   ```
   eir[i] = rated_eir * (1.0 - eir_part_load_benefit * (1.0 - fraction[i]))
   ```
   where `eir_part_load_benefit` defaults to 0.0 (current behavior: constant EIR) and can be set to ~0.1-0.2 for units with documented part-load COP improvement.

## Approach

1. **Add `min_compressor_fraction` field** to `HeatPumpHeaterConfig` (`heat_pump_config.rs`):
   ```rust
   #[serde(default = "default_min_compressor_fraction")]
   pub min_compressor_fraction: f64,
   ```
   with:
   ```rust
   fn default_min_compressor_fraction() -> f64 { 0.25 }
   ```

2. **Add `eir_part_load_benefit` field** (optional, default 0.0):
   ```rust
   #[serde(default, skip_serializing_if = "Option::is_none")]
   pub eir_part_load_benefit: Option<f64>,
   ```

3. **Modify speed generation** in `heater.rs:517-525` to use `min_compressor_fraction`:
   ```rust
   let min_frac = cfg.min_compressor_fraction.clamp(0.1, 0.5);
   let n = 4;  // MSHP always 4 stages
   let stages: Vec<f64> = (0..n)
       .map(|i| base_cap * (min_frac + (1.0 - min_frac) * i as f64 / (n - 1) as f64))
       .collect();
   self.hvac.heating_capacities_w = stages;
   ```

4. **Derive per-stage EIR** if `eir_part_load_benefit` is provided:
   ```rust
   let benefit = cfg.eir_part_load_benefit.unwrap_or(0.0);
   self.hvac.eir_by_stage = (0..n)
       .map(|i| {
           let frac = min_frac + (1.0 - min_frac) * i as f64 / (n - 1) as f64;
           base_eir * (1.0 - benefit * (1.0 - frac))
       })
       .collect();
   ```

5. **Apply same changes to cooling side** — add the fields to `HeatPumpCoolerConfig` and update the cooler's speed generation path (which delegates to `AirConditioner`).

6. **Validate** in `HeatPumpHeaterConfig::validate()`: `min_compressor_fraction` must be in `[0.1, 0.5]`. For `eir_part_load_benefit`: no hard ceiling can be cited from a primary source; validate that the value is in `[0.0, 1.0]` and emit a warning when it exceeds 0.5, since measured COP improvement at minimum stage versus rated is typically 30–50% for inverter compressors (NREL/PNNL inverter heat pump field studies, e.g., Cutler et al., NREL/TP-5500-52791, and Daikin/Mitsubishi product engineering data). Do not silently clamp; error loudly outside the validated range.

7. **Cross-ticket interaction with 015 (ER on/off)**: when zone load falls below the minimum-stage HP capacity, the HP cycles on/off. Binary ER staging from ticket 015 may fire during HP cycling and cause zone temperature overshoot. An integration test must run both changes together under a low-load scenario. This test is owned by ticket 015 (see "Verification" in that ticket).

8. **Update existing tests** — the MSHP speed stage tests in `cooler.rs:599-675` assume 4 stages at 0.25/0.5/0.75/1.0. Update to use explicit config or verify the new default produces equivalent results.

## HPXML Wiring

`min_compressor_fraction`: HPXML `MinimumCapacity` / `HeatingCapacity`.
Currently `MinimumCapacity` is not read by `resolve_hvac.rs`. Add extraction:
`min_compressor_fraction = MinimumCapacity / HeatingCapacity` if provided,
otherwise default 0.25.

`eir_part_load_benefit`: No HPXML source. Comes from equipment defaults
or manual config. Default 0.0 (no EIR benefit at part load).

## Definition of Done

- [ ] `min_compressor_fraction` field in `HeatPumpHeaterConfig` and `HeatPumpCoolerConfig`
- [ ] Default value 0.25 preserves existing behavior
- [ ] Speed stages derived from `min_compressor_fraction` instead of hardcoded 0.25
- [ ] Per-stage EIR derived from `eir_part_load_benefit` when provided
- [ ] Validation: `min_compressor_fraction` in [0.1, 0.5] errors loudly; `eir_part_load_benefit` in [0.0, 1.0] errors loudly; warning emitted when `eir_part_load_benefit > 0.5`
- [ ] Existing MSHP tests pass with default `min_compressor_fraction = 0.25`
- [ ] New test: `min_compressor_fraction = 0.30` produces stages [0.30, 0.533, 0.767, 1.00]
- [ ] Cooling-side MSHP also uses configurable minimum
- [ ] `tracing::debug!` when generating speed stages with configurable minimum: `"MSHP generating {n} stages from min_fraction={min:.2} to rated"`
- [ ] Telemetry key `MIN_COMPRESSOR_FRACTION` added

## Verification

1. **Regression test**: MSHP with default `min_compressor_fraction = 0.25` produces identical behavior to current code.
2. **Unit test**: MSHP with `min_compressor_fraction = 0.30`; verify first stage capacity = `0.30 * rated`.
3. **Unit test**: MSHP with `eir_part_load_benefit = 0.15`; verify first-stage EIR < rated EIR.
4. **Unit test**: Validate rejects `min_compressor_fraction < 0.1` and `> 0.5`.
5. **Integration test**: Simulate a low-load condition (10% of rated) with `min_compressor_fraction = 0.25`. The HP should cycle on/off (load < minimum). With `min_compressor_fraction = 0.10`, the HP should run continuously at stage 1.

## References

- AHRI Standard 1230: variable-rate heat pump testing and rating
- Mitsubishi Electric MSHP specifications: minimum capacity typically 30-40% of rated
- Daikin inverter specifications: minimum compressor frequency ~20-25 Hz vs rated 60-80 Hz
- EnergyPlus I/O Reference, `Coil:Heating:DX:MultiSpeed` fields: `Minimum Flow Fraction` and `Gross Rated Heating Capacity`
- OCHRE `HVAC.py:774-797`: multispeed parameters loaded from CSV, not hardcoded

## Related Tickets

- [014-defrost-typed-config.md](014-defrost-typed-config.md) — config struct improvements
- [010-default-biquadratic-performance-curves.md](010-default-biquadratic-performance-curves.md) — curves interact with speed stages

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-20

### Code Confirmation

- [x] Referenced line numbers still match — confirmed at `heater.rs:517-525`; the exact code block cited by the ticket is present and unmodified.
- [x] Described logic matches current implementation — `vec![base_cap * 0.25, base_cap * 0.5, base_cap * 0.75, base_cap]` at line 523; `eir_by_stage = vec![base_eir; 4]` at line 524; both are hardcoded.
- [x] No `min_compressor_fraction` field in `HeatPumpHeaterConfig` (`heat_pump_config.rs:20-139`) or `HeatPumpCoolerConfig` (`heat_pump_config.rs:302-393`). Confirmed by code read and serde round-trip test.
- [x] OCHRE cross-check: **diverges intentionally** — OCHRE `HVAC.py:773-797` loads multispeed capacity ratios from `"HVAC Multispeed Parameters.csv"` (confirmed in vendored file at `vendors/OCHRE/ochre/defaults/HVAC Multispeed Parameters.csv`). OCHRE MSHP Heater Capacity Ratio 1 = **0.40** (40%) not 0.25; OCHRE MSHP Cooler Capacity Ratio 1 = **0.48889** (~49%) not 0.25. HARES diverges from OCHRE by hardcoding 0.25 and using evenly-spaced stages. The ticket is correct that this is a divergence, but mislabels it as a "bug" — it is a design simplification that OCHRE does not use.
- [x] EnergyPlus cross-check: EnergyPlus `Coil:Heating:DX:MultiSpeed` (I/O Reference, EnergyPlus 8.0, bigladdersoftware.com) specifies each speed's capacity **explicitly** via `Speed N Rated Total Heating Capacity` fields — there is no single "minimum fraction" field. The ticket's claim that EnergyPlus has a `Minimum Flow Fraction` field for `Coil:Heating:DX:MultiSpeed` is **incorrect** (see below). EnergyPlus also does not hardcode 0.25; it accepts any explicit capacity at speed 1.

### Web-Verified Citations

**Citation 1**: "AHRI Standard 1230: variable-rate heat pump testing and rating"
- **Source found**: AHRI Standard 1230-2021, https://www.ahrinet.org/system/files/2023-06/AHRI_Standard_1230-2021.pdf (403 on direct fetch; scope confirmed via AHRI website and search results)
- **Quoted passage**: "AHRI Standard 1230 applies to VRF multi-split air-conditioners and multi-split heat pumps using distributed refrigerant technology, including VRF air-source systems with cooling capacity ≥ 65,000 Btu/h. It does not apply to systems below 65,000 Btu/h as defined in AHRI 210/240."
- **Verdict**: **Partially incorrect** — AHRI 1230 covers VRF *multi*-split systems above 65,000 Btu/h. Residential single-zone mini-splits are covered by **AHRI 210/240**, not AHRI 1230. The ticket cites AHRI 1230 for MSHP minimum capacity, but this standard does not directly govern the residential mini-split segment HARES models. The relevant standard would be AHRI 210/240 (2023 edition), which covers variable-speed unitary heat pumps including ductless mini-splits. This is a citation imprecision, not a fatal flaw.

**Citation 2**: "Mitsubishi Electric MSHP specifications: minimum capacity typically 30-40% of rated"
- **Source found**: Mitsubishi MSZ-FH series guide specs (https://www.mitsubishitechinfo.ca/sites/default/files/GuideSpecs-MSZ-FH-NAH-Series_201905.pdf) and Green Building Advisor analysis of Mitsubishi performance data
- **Quoted passage**: Search results confirm "For Mitsubishi mini-split heat pumps, minimum capacity for heating is typically 15% to 30% of rated capacity; for cooling, 20% to 40%."
- **Verdict**: **Partially confirmed** — the 30–40% range is a plausible upper bound for some models, but some Mitsubishi units can go as low as 15% minimum in heating. The ticket's claim of "30–40%" is a conservative estimate, not universally wrong, but understates the lower bound. More importantly, OCHRE uses 40% for its MSHP Heater model (Capacity Ratio 1 = 0.40), which is at the high end of the range.

**Citation 3**: "Daikin inverter specifications: minimum compressor frequency ~20-25 Hz vs rated 60-80 Hz"
- **Source found**: Daikin submittal data sheets (multiple models at backend.daikincomfort.com). PDF binary content could not be parsed by WebFetch; specific Hz values not confirmed from source.
- **Quoted passage**: Could not retrieve verbatim — PDF binary encoding prevented extraction.
- **Verdict**: **Cannot Verify from source** — the Hz range (20–25 Hz minimum vs 60–80 Hz rated) corresponds roughly to 25–40% of rated frequency, which would support the ticket's claim that minimum capacity is 25–40% of rated (since compressor capacity scales roughly with frequency). The claim is physically plausible but the specific Hz numbers were not confirmed from the Daikin datasheet.

**Citation 4**: "EnergyPlus I/O Reference, `Coil:Heating:DX:MultiSpeed` fields: `Minimum Flow Fraction` and `Gross Rated Heating Capacity`"
- **Source found**: EnergyPlus 8.0 I/O Reference, Group – Heating and Cooling Coils, bigladdersoftware.com/epx/docs/8-0/input-output-reference/page-042.html
- **Quoted passage**: `Coil:Heating:DX:MultiSpeed` input fields include: `Speed Rated Total Heating Capacity`, `Speed Rated COP`, `Speed Rated Air Flow Rate`, `Number of Speeds`. There is **no `Minimum Flow Fraction` field** in this object. The `Minimum Flow Fraction` field exists in the parent object `AirLoopHVAC:UnitaryHeatPump:AirToAir:MultiSpeed` and controls the *air-side* no-load fan flow fraction, not the *compressor* minimum capacity.
- **Verdict**: **Incorrect** — the ticket conflates two different fields. `Minimum Flow Fraction` in EnergyPlus is an air-handler fan field (fraction of air flow at no-load), not a compressor minimum capacity field. The *compressor* minimum capacity in EnergyPlus `Coil:Heating:DX:MultiSpeed` is set by explicitly providing `Speed 1 Rated Total Heating Capacity` with whatever absolute value the user wants. There is no single "minimum fraction" parameter; the minimum is implied by the ratio of Speed 1 capacity to Speed N capacity. The ticket's citation of `Minimum Flow Fraction` as an analog for `min_compressor_fraction` is inaccurate.

**Citation 5**: "OCHRE `HVAC.py:774-797`: multispeed parameters loaded from CSV, not hardcoded"
- **Source found**: `/Users/rich/source/HARES/vendors/OCHRE/ochre/Equipment/HVAC.py:773-797` (vendored copy) and GitHub NREL/OCHRE main branch
- **Quoted passage** (lines 773-797):
  ```python
  # Load multispeed parameters from file
  if self.n_speeds > 1:
      rated_efficiency = kwargs.get("Rated Efficiency", "(Unknown Efficiency)")
      multispeed_file = kwargs.get("multispeed_file", "HVAC Multispeed Parameters.csv")
      df_speed = load_csv(multispeed_file)
      speed_params = df_speed.loc[
          (df_speed["HVAC Name"] == self.name)
          & (df_speed["HVAC Efficiency"] == rated_efficiency)
          & (df_speed["Number of Speeds"] == self.n_speeds)
      ]
      kwargs["Capacity (W)"] = [
          kwargs["Capacity (W)"] * speed_params[f"Capacity Ratio {i + 1}"] for i in range(self.n_speeds)
      ]
      kwargs["EIR (-)"] = [1 / speed_params[f"COP {i + 1}"] for i in range(self.n_speeds)]
  ```
  And from the CSV: `MSHP Heater` with 4 speeds has `Capacity Ratio 1 = 0.40`, `COP 1 ≠ COP 4`.
- **Verdict**: **Confirmed** — OCHRE loads per-stage capacity ratios and COPs from CSV. The MSHP Heater minimum stage is 40% of rated, not 25%. The ticket's citation is accurate and the line numbers are exact.

**Citation 6**: "NREL/TP-5500-52791, Cutler et al." (eir_part_load_benefit reference)
- **Source found**: Could not find TP-5500-52791 via WebSearch. The closest NREL report found is TP-5500-56354 (docs.nrel.gov/docs/fy13osti/56354.pdf) which may be a different report by Cutler. PDF binary content could not be decoded.
- **Quoted passage**: Not available — report number TP-5500-52791 not found in NREL publications database via search.
- **Verdict**: **Cannot Verify** — the specific report number does not appear in any indexed source. It may be an internal NREL preprint or an incorrect report number. The physical claim (inverter heat pump COP improves at part-load by 30–50%) is widely corroborated by general HVAC literature, but the specific citation cannot be verified.

### Legitimacy

- **Verdict**: **Partially Legitimate**
- **Rationale**: The core bug is real and confirmed: `heater.rs:517-525` hardcodes MSHP speed stages at 0.25/0.50/0.75/1.00 with no configurable minimum, and `eir_by_stage` is a flat vec of the base EIR. The OCHRE cross-reference is legitimate and the line numbers are exact. However, several supporting details are incorrect: (1) the EnergyPlus `Minimum Flow Fraction` citation is wrong — that field controls air-handler fan flow, not compressor minimum capacity; EnergyPlus sets minimum compressor capacity via the absolute `Speed 1 Rated Heating Capacity` field; (2) AHRI 1230 does not apply to residential ductless mini-splits (AHRI 210/240 applies); (3) the NREL report number TP-5500-52791 cannot be verified. The proposed fix (adding `min_compressor_fraction` defaulting to 0.25 for backward compatibility) is sound. The description of OCHRE's approach is accurate. The default of 0.25 in the proposed fix is conservative — OCHRE uses 0.40 for MSHP Heater, so the HARES default of 0.25 causes HARES to allow lower-than-OCHRE minimum speeds; this may or may not be desirable depending on the target equipment being modeled.

### Proposed Fix Summary

1. Add `min_compressor_fraction: f64` (default 0.25) to `HeatPumpHeaterConfig` and `HeatPumpCoolerConfig` in `heat_pump_config.rs`.
2. Add `eir_part_load_benefit: Option<f64>` (default `None` = 0.0) to both config structs.
3. Replace the hardcoded `vec![base_cap * 0.25, ...]` in `heater.rs:522-523` with the evenly-spaced formula using `min_compressor_fraction`.
4. Replace `vec![base_eir; 4]` with the per-stage EIR formula when `eir_part_load_benefit` is provided.
5. Apply the same changes to the cooling path (the cooler delegates stage generation differently, requiring a separate audit of `cooler.rs` and `AirConditioner.init`).
6. Add validation: `min_compressor_fraction` in [0.1, 0.5]; `eir_part_load_benefit` in [0.0, 1.0] with warning > 0.5.
7. Do NOT cite `Minimum Flow Fraction` as the EnergyPlus analog — cite `Speed 1 Gross Rated Heating Capacity / Speed N Gross Rated Heating Capacity` instead.

### Test Written

- **File 1**: `crates/hares-equipment/src/hvac/heat_pump/heater.rs` (internal `mod tests`, lines after `add_identity_biquadratic_curves`)
  - `ticket_013_mshp_speed_stages_hardcoded_at_25_50_75_100pct`: directly reads `eq.core.hvac.heating_capacities_w` after init, verifies 4 stages at exactly [0.25, 0.50, 0.75, 1.00] × rated; documents OCHRE divergence (0.40 at stage 1).
  - `ticket_013_mshp_eir_identical_across_all_stages`: reads `eq.core.hvac.eir_by_stage`, verifies all 4 EIRs equal `base_eir` (no part-load benefit).

- **File 2**: `crates/hares-equipment/tests/hvac_tests.rs` (external integration tests)
  - `ticket_013_mshp_load_above_stage1_runs_continuously`: behavioral test, verifies HP delivers > 20% rated capacity when zone is cold.
  - `ticket_013_mshp_has_no_min_compressor_fraction_field`: serde deny_unknown_fields test; verifies `min_compressor_fraction` does not exist in config yet — must be updated/removed after ticket is implemented.

All 4 tests pass with `cargo test -p hares-equipment ticket_013`.
