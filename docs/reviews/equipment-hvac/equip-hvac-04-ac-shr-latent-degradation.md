# AC sensible heat ratio and latent degradation parameters
**Review ID**: equip-hvac-04
**Category**: equipment-hvac
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-equipment/src/hvac/air_conditioner.rs`
- `crates/hares-equipment/src/hvac/latent_degradation.rs`
- `crates/hares-equipment/src/hvac/ac_config.rs`
- `crates/hares-equipment/src/hvac/coil_physics.rs`
- `crates/hares-equipment/src/hvac/cooling_config.rs`

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Equipment/HVAC.py`
- `vendors/OCHRE/ochre/utils/equipment.py`

## Findings

### Finding 1: [Severity: high] Coil Ao computed with single rated SHR for all stages, ignoring per-stage SHR values
**Description**: When per-stage SHR values are provided via `stage_shrs`, the coil Ao factor is computed using only the rated SHR (which becomes `stage_shrs[0]`) for every stage, rather than using each stage's own SHR. This means multi-stage/variable-speed equipment gets incorrect bypass factors and ADP temperatures at non-first stages.
**Code Location**:
- `crates/hares-equipment/src/hvac/latent_degradation.rs:29-48` (`compute_coil_ao_by_stage` loop uses `rated_shr` for all stages)
- `crates/hares-equipment/src/hvac/air_conditioner.rs:601-606` (`rated_shr` set to `stage_shrs[0]` when per-stage SHRs are present)
- `crates/hares-equipment/src/hvac/air_conditioner.rs:1314-1321` (`compute_coil_ao` passes single `rated_shr`)
**Root Cause**: `compute_coil_ao_by_stage` receives a single `rated_shr` parameter (line 20) and applies it to every stage in the loop (line 31). The caller `compute_coil_ao` passes `self.rated_shr` which, when `stage_shrs` is non-empty, is set to `stage_shrs[0]` (line 606) rather than the full-load stage's SHR.
**Impact**: Multi-stage equipment with varying SHR per stage (e.g., low stage SHR=0.80, high stage SHR=0.70) will compute Ao for the high stage using the low stage's SHR, producing incorrect bypass factors, ADP temperatures, and supply air temperatures at higher stages. This affects psychrometric coil state and latent/sensible split.
**Comparison with OCHRE**: OCHRE `HVAC.py:207-211` zips `capacity_list`, `flow_rate_list`, and `shr_list` together, computing Ao per-speed using each speed's own SHR: `ao_data = zip(self.capacity_list[1:], self.flow_rate_list[1:], shr_list[1:])`. HARES should match this pattern.

### Finding 2: [Severity: medium] Room AC never gets latent degradation parameters; model is always disabled
**Description**: Room AC units always use `LatentDegradationParams::default()` (all zeros → `is_active()` returns false → degradation skipped). Central AC units correctly receive the EnergyPlus default parameters (twet=1500s, gamma=1.5, Nmax=3/hr, tau=45s). Since room ACs are single-speed cycling units operating in individual rooms, latent degradation at part load is actually more relevant to them than to continuously-running variable-speed central systems.
**Code Location**:
- `crates/hares-equipment/src/hvac/air_conditioner.rs:474` (constructor sets `LatentDegradationParams::default()`)
- `crates/hares-equipment/src/hvac/air_conditioner.rs:612-617` (central AC branch sets EnergyPlus defaults; room AC branch at lines 516-529 does not set any latent degradation params)
- `crates/hares-equipment/src/hvac/coil_physics.rs:1261` (`self.latent_degradation.is_active()` guard gates the model)
**Root Cause**: Room AC `init_from_typed` branch (lines 516-529) never overrides the zero-valued `LatentDegradationParams` from the constructor. The central AC branch (lines 612-617) does override them, but only for central AC.
**Impact**: Room ACs always report steady-state SHR with no part-load cycling re-evaporation penalties. In humid climates, this over-predicts latent removal by room ACs during cycling operation.
**Comparison with OCHRE**: OCHRE does not implement a Henderson-Rengarajan model at all, so both equipment types lack it there. HARES's central-AC-only application is an internal inconsistency.

### Finding 3: [Severity: medium] rated_shr uses stage_shrs[0] (first stage) rather than full-load stage SHR for latent degradation
**Description**: When per-stage SHRs are provided, `rated_shr` is set to `stage_shrs[0]` (line 606), which is the lowest speed's SHR. But the Henderson-Rengarajan model references the AHRI-rated (full-load) SHR as its baseline for computing rated latent capacity. For equipment where SHR varies with speed (e.g., low speed SHR=0.82, high speed SHR=0.72), using the low-speed SHR understates the rated latent fraction.
**Code Location**:
- `crates/hares-equipment/src/hvac/air_conditioner.rs:604-606` (overwrites `rated_shr` with `stage_shrs[0]`)
- `crates/hares-equipment/src/hvac/air_conditioner.rs:1277` (`rated_latent_w = rated_cap_w * (1.0 - self.rated_shr)` uses the wrong baseline SHR)
**Root Cause**: `stage_shrs[0]` is the per-stage SHR for the first (lowest) speed. The rated SHR for AHRI conditions should be the full-load stage's SHR (the last element of `stage_shrs` or the user-provided `shr` value). Line 606 unconditionally overwrites `rated_shr` with the first stage's value when `stage_shrs` is non-empty.
**Impact**: Multi-speed equipment with different SHR per stage will have their latent degradation model use an incorrect rated latent capacity baseline, skewing the effective SHR at part load.

### Finding 4: [Severity: low] Uniform SHR default of 0.75 for all AC types and climates
**Description**: The default SHR of 0.75 (lines 527 and 601) is applied identically to both central AC and room AC, and does not vary with climate or equipment type. This default is a reasonable industry average (AHRI 210/240 rated conditions at 80°F DB / 67°F WB), but single-speed fixed-orifice systems in dry climates typically have SHR ≈ 0.80-0.85, while variable-speed or TXV-equipped systems in humid climates may operate at SHR ≈ 0.65-0.70.
**Code Location**:
- `crates/hares-equipment/src/hvac/air_conditioner.rs:472` (constructor default: `rated_shr: 0.75`)
- `crates/hares-equipment/src/hvac/air_conditioner.rs:527` (room AC: `cfg.shr.unwrap_or(0.75)`)
- `crates/hares-equipment/src/hvac/air_conditioner.rs:601` (central AC: `cfg.shr.unwrap_or(0.75)`)
**Root Cause**: No climate-zone or equipment-type-based differentiation of the default SHR. HPXML inputs can override it, so this only affects simulations where SHR is omitted from the HPXML.
**Impact**: In dry-climate simulations without explicit HPXML SHR, latent removal is slightly over-predicted. In humid-climate simulations, it may be slightly under-predicted. The magnitude is small since SHR can always be set explicitly.
**Comparison with OCHRE**: OCHRE defaults SHR to 1.0 when not provided (`HVAC.py:118-119`: `if shr is None: shr = 1`), which is a much worse default. HARES's 0.75 is significantly better.

### Finding 5: [Severity: low] Henderson-Rengarajan parameters are hard-coded with no config override
**Description**: The four latent degradation model parameters (`twet_rated_s=1500.0, gamma_rated=1.5, max_cycling_rate=3.0, latent_time_constant_s=45.0`) are hard-coded at `air_conditioner.rs:612-617` and not exposed through the typed configuration (`CentralAirConditionerConfig` or `RoomAcConfig`). There is no way for users to tune these parameters without modifying the source code.
**Code Location**:
- `crates/hares-equipment/src/hvac/air_conditioner.rs:612-617`
- `crates/hares-equipment/src/hvac/cooling_config.rs:15-109` (no latent degradation fields in `CentralAirConditionerConfig`)
**Root Cause**: The parameters were added as direct constants rather than configurable fields. EnergyPlus exposes these as user-adjustable curve coefficients.
**Impact**: Users cannot calibrate the latent degradation model to specific equipment or validate against measured data. The current defaults (EnergyPlus residential DX coil defaults) are reasonable but not universally appropriate.
**Comparison with OCHRE**: OCHRE does not implement latent degradation at all, so this is an implementation beyond the reference. The lack of configurability limits its calibration potential.

## Summary
- Total findings: 5
- Critical: 0
- High: 1
- Medium: 2
- Low: 2

## Recommendations
1. **Fix Ao per-stage computation** (Finding 1): Modify `compute_coil_ao_by_stage` to accept per-stage SHR values when available, matching OCHRE's approach of zipping capacity, flow, and SHR by stage. When `stage_shrs` is empty, fall back to the single `rated_shr` for all stages.
2. **Enable latent degradation for room AC** (Finding 2): Apply the same EnergyPlus default parameters to room AC units in `init_from_typed`, or expose via config so users can enable it per equipment type.
3. **Use full-load stage SHR as rated_shr** (Finding 3): When per-stage SHRs are provided, set `rated_shr` to the last element of `stage_shrs` (full-load stage) rather than the first, since AHRI-rated conditions correspond to full-load operation.
4. **Consider configurable latent degradation parameters** (Finding 5): Add optional latent degradation fields to `CentralAirConditionerConfig` and `RoomAcConfig` so users can override the EnergyPlus defaults for specific equipment models.

## References / Citations
- Henderson, H.I. and K. Rengarajan. "A Model to Predict the Latent Capacity of Air Conditioners and Heat Pumps at Part-Load Conditions with Constant Fan Operation." *ASHRAE Transactions*, 1996, Vol. 102, Part 1.
- EnergyPlus Engineering Reference, §16.5 "Residential DX Coil Model" — latent degradation parameters and coil bypass factor methodology.
- EnergyPlus source: `DXCoils.cc` — reference implementation of `CalcEffectiveSHR` and Henderson-Rengarajan model.
- OCHRE `HVAC.py:116-124` — SHR default behavior (sets to 1.0 when absent from HPXML; HARES's 0.75 is an improvement).
- OCHRE `HVAC.py:199-215` — Ao computation per-speed using per-stage SHR values (the pattern HARES should follow for Finding 1).
- OCHRE `equipment.py:795-875` — `coil_bypass_factor` reference implementation using EnergyPlus methodology.
- OCHRE `equipment.py:658-680` — `calculate_mass_flow_rate` uses `GetMoistAirDensity` (moist-air density); HARES intentionally deviates to use dry-air density for ASHRAE HOF Ch.1 consistency (`coil_physics.rs:424-441`).
- ASHRAE 2017 Handbook of Fundamentals, Ch.18 Eq.63 — enthalpy-based coil bypass factor.
- AHRI 210/240 — rated cooling test conditions (80°F DB / 67°F WB indoor; 95°F DB outdoor).
