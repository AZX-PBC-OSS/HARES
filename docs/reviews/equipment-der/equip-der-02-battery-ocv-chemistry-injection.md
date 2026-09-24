# Battery OCV chemistry selection and custom table injection
**Review ID**: equip-der-02
**Category**: equipment-der
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-equipment/src/battery/ocv.rs` — OCV and UNeg look-up table definitions (384 lines)
- `crates/hares-equipment/src/battery/mod.rs` — Battery equipment with init, step, and LUT injection (4064 lines)
- `crates/hares-equipment/src/battery/degradation.rs` — Smith 2017 degradation model (1099 lines)
- `crates/hares-types/src/equipment.rs:258` — `BatteryChemistry` enum
- `crates/hares-equipment/src/lib.rs:179-229` — Equipment trait LUT injection interface
- `crates/hares-core/src/dwelling/mod.rs:1540-1609` — Dwelling set_battery_lut / clear_battery_lut

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Equipment/Battery.py` — OCHRE Battery model (480 lines)
- `vendors/OCHRE/ochre/defaults/Battery/degradation_curves.csv` — OCHRE single OCV & U_neg table (11-point)

## Findings

### Finding 1: NMC811 nomenclature discrepancy vs BatteryChemistry enum
**Severity**: low
**Description**: The OCV table `default_li_nmc()` is documented as "NMC811/Graphite OCV curve (PyBaMM Chen2020, LG M50)" but the `BatteryChemistry` enum only exposes `Nmc` — no `Nmc811` variant exists, nor a non-811 NMC variant. Users who select `chemistry = "nmc"` get the NMC811 curve without any indication. The `ocv.rs` module comment lists "NMC: Chen2020 (NMC811/Graphite, LG M50)" as the default, which is technically correct but the enum naming is underspecified.
**Code Location**: `crates/hares-equipment/src/battery/ocv.rs:34`, `crates/hares-types/src/equipment.rs:258-263`
**Root Cause**: The `BatteryChemistry` enum uses generic chemistry names (`Nmc`, `Lfp`, `Nca`, `Lto`) while the OCV curves are generated from specific sub-chemistries (NMC811 from Chen2020, LFP from Prada2013/Afshar2017, NCA from Kim2011, LTO from Colclasure2011). This mapping is documented in the module file comment but not visible in the enum.
**Impact**: Users may assume they're getting a generic NMC (e.g., NMC111 or NMC532) while actually getting NMC811. In practice, OCV differences among NMC variants are <20mV, so the functional impact is negligible for residential energy simulation.

### Finding 2: OCHRE OCV floor differs from HARES NMC default (3.0V vs 2.5V at SOC=0)
**Severity**: low
**Description**: OCHRE's `degradation_curves.csv` defines V_oc at SOC=0 as 3.0V, while HARES' NMC default curve (`default_li_nmc()`) uses 2.5V at SOC=0 — a 0.5V discrepancy. The HARES 2.5V cutoff is consistent with NMC811 deep-discharge characteristics, but OCHRE's higher floor is more conservative. Both curves converge at mid-to-high SOC (at SOC=0.5: OCHRE=3.688V, HARES=~3.75V, ~2% difference). The lower HARES floor may produce different terminal voltage calculations in deeply discharged scenarios.
**Code Location**: `crates/hares-equipment/src/battery/ocv.rs:38` (2.500000 V) vs `vendors/OCHRE/ochre/defaults/Battery/degradation_curves.csv:2` (3.0 V)
**Root Cause**: HARES uses 51-point PyBaMM-generated curves for specific chemistries; OCHRE uses a single 11-point generic curve. The lower SOC floor is physically more accurate for NMC811 but may give different results when migrating OCHRE models.
**Impact**: Minor — SOC is typically clamped above `min_soc` (default 0.15), so the battery rarely operates in the low-SOC region where this discrepancy matters. Only relevant for deeply discharged states.

### Finding 3: Independent OCV and UNeg table injection lacks cross-validation
**Severity**: medium
**Description**: `set_ocv_table()` and `set_u_neg_table()` are fully independent operations — calling one does not update the other. The `init_typed()` path links them correctly via `OcvTable::for_chemistry()` + `UNegTable::for_chemistry()` (lines 718-723), but post-init injection of a custom OCV leaves the `u_neg_table` unchanged. While LFP/NMC/NCA all share the same graphite anode curve (`UNegTable::default_li_nmc()`) and are interchangeable, injecting an LTO OCV onto a battery with graphite UNeg (or vice versa) would create a physically inconsistent model where the full-cell OCV uses one chemistry but the degradation Tafel correction (mechanism 1, `update_daily()`) uses a different anode potential.
**Code Location**: `crates/hares-equipment/src/battery/mod.rs:1259-1269` (set_ocv_table, set_u_neg_table), `crates/hares-equipment/src/battery/mod.rs:718-723` (init chemistry linkage), `crates/hares-equipment/src/battery/degradation.rs:286-288` (Tafel correction consumes u_neg_table)
**Root Cause**: The `custom_ocv` and `custom_u_neg` flags (lines 359-360) are independent booleans per table. There is no mechanism to enforce that the OCV chemistry and UNeg chemistry correspond to a physically realizable cell.
**Impact**: Low risk in practice — the main usage pattern is: (a) select a chemistry at init, which sets both tables consistently; or (b) inject a custom high-fidelity PyBaMM LUT for NMC/NCA/LFP, which all use the same graphite UNeg. However, if an EV or exotic chemistry model injects LTO OCV via `set_ocv_table()` without also updating UNeg, the degradation model will silently use wrong anode parameters.

### Finding 4: Checkpoint does not persist custom OCV/UNeg table injection
**Severity**: low
**Description**: The `BatteryCheckpoint` struct used by `save_state()` and `load_state()` omits the `custom_ocv`, `custom_u_neg` flags and the actual table contents. After a `save_state()` → `new()` → `init()` → `load_state()` cycle, the custom tables are not restored — `custom_ocv` and `custom_u_neg` revert to `false` (from `new()` default), and at the next `init()` call, the OCV defaults to the chemistry base default rather than the injected custom curve.
**Code Location**: `crates/hares-equipment/src/battery/mod.rs:255-276` (BatteryCheckpoint struct — missing custom_ocv/custom_u_neg), `crates/hares-equipment/src/battery/mod.rs:451-452` (new() defaults custom_ocv/u_neg to false)
**Root Cause**: The OCV and UNeg tables are treated as configuration rather than state. The checkpoint is designed to persist operational state (SOC, degradation, temperature) but not configuration (LUTs). LUT injection is expected to occur at scenario-setup time and be re-applied on each simulation run.
**Impact**: In a long-running simulation with checkpoint-restart, if the simulation runner re-applies `set_battery_lut` after `init()` but before the first `step()`, there is no issue. If checkpoint restore skips re-injection (e.g., because the restore code assumes `init()` is sufficient), the simulation silently falls back to the chemistry default OCV. This could produce inconsistent results between the original and restarted runs.

### Finding 5: All four default OCV curves are physically correct — verification passed
**Severity**: none (verification)
**Description**: Each of the four chemistry-specific OCV curves was verified for physical plausibility:
- **NMC** (NMC811/Graphite, Chen2020): 2.500 → 4.200 V, strictly monotonic, mid-SOC ~3.75 V. Correct for LG M50.
- **LFP** (LFP/Graphite, Prada2013/Afshar2017): 2.000 → 3.600 V, flat plateau at ~3.27 V spanning most SOC, mid-SOC in [3.20, 3.35]. Matches known LFP chemistry. No path-dependent hysteresis (consistent with OCHRE simplification).
- **NCA** (NCA/Graphite, Kim2011): 2.700 → 4.200 V, strictly monotonic, mid-SOC ~3.60-3.75. Correct for NCA chemistry.
- **LTO** (NMC811/LTO, Chen2020+Colclasure2011): 2.047 → 2.734 V, strictly monotonic, mid-SOC ~2.30-2.40. Lower voltage range correct for LTO anode (~1.556 V plateau) with NMC811 cathode.
All curves are tested for strict monotonicity at `ocv.rs:345-350`. NCA is fully wired in both `OcvTable::for_chemistry()` (line 54) and `UNegTable::for_chemistry()` (line 201). The UNeg dispatch correctly maps LFP/NCA to graphite and LTO to its own flat plateau.
**Code Location**: `crates/hares-equipment/src/battery/ocv.rs:34-165`, tests at lines 284-383.
**Root Cause**: N/A — design is sound.
**Impact**: None.

### Finding 6: Custom OCV/UNeg propagation to all downstream consumers — verification passed
**Severity**: none (verification)
**Description**: All three downstream consumption paths were traced to confirm they consume `self.ocv_table` and `self.u_neg_table` (which are the targets of `set_ocv_table()` and `set_u_neg_table()`):
- **(a) Terminal voltage computation during power-based dispatch**: `compute_electrical()` reads `self.ocv_table.voltage_at_soc(self.soc)` at `mod.rs:479`. Uses the currently-set OCV table.
- **(b) Degradation Tafel correction (per-timestep)**: `accumulate()` receives `v_oc_before` computed from `self.ocv_table.voltage_at_soc(soc_before)` at `mod.rs:1025`. Uses the currently-set OCV table for mechanism 3 Tafel factor.
- **(b) Degradation Tafel correction (daily)**: `update_daily()` receives `&self.u_neg_table` at `mod.rs:1036` and reads the negative electrode potential via `u_neg_table.potential_at_soc(self.soc_at_max_dod)` at `degradation.rs:286`. Uses the currently-set UNeg table for mechanism 1 Tafel factor.
- **(c) SOC estimation**: Pure coulomb counting (`soc += energy_delta_kwh / capacity_kwh` at `mod.rs:913`), independent of OCV. No Kalman filter or OCV-based SOC correction path exists — this is by design (first-order model).
No propagation breaks detected. Custom tables set via the `Equipment` trait interface flow through to all relevant model paths.
**Code Location**: `crates/hares-equipment/src/battery/mod.rs:479,1025,1036`, `crates/hares-equipment/src/battery/degradation.rs:226-256`
**Root Cause**: N/A — design is sound.
**Impact**: None.

## Summary
- **Total findings**: 6
- **Critical**: 0 / **High**: 0 / **Medium**: 1 / **Low**: 3 / **Verification-only**: 2

## Recommendations

1. **Document the UNeg-OCV coupling constraint** (Finding 3): Add a docstring on `set_ocv_table()` warning that `set_u_neg_table()` must be called separately if the UNeg table for the target chemistry differs from the current one. Alternatively, add an `ocv_table_chemistry: Option<BatteryChemistry>` field and validate consistency during `step()` or `update_daily()`.

2. **Clarify NMC/NMC811 naming** (Finding 1): Either rename `default_li_nmc()` to `default_nmc811()` or add a `BatteryChemistry::Nmc811` variant. The README or config schema should document that `chemistry = "nmc"` maps to NMC811/Graphite (LG M50) specifically.

3. **Document checkpoint LUT-reinjection requirement** (Finding 4): Add a note in `load_state()` documentation that custom OCV/UNeg tables are NOT stored in the checkpoint and must be re-injected after restore.

4. **Consider OCHRE compatibility mode** (Finding 2): If OCHRE model migration is a priority, add a `BatteryChemistry::Generic` variant that uses OCHRE's 11-point curve (or a 51-point resampling of it) for exact behavioral parity with legacy models.

## References / Citations

- Chen, C.-H., et al. (2020). "Development of Experimental Techniques for Parameterization of Multi-scale Lithium-ion Battery Models." *Journal of The Electrochemical Society*, 167(8), 080534. — NMC811/LG M50 OCV source.
- Prada, E., et al. (2013). "A Simplified Electrochemical and Thermal Aging Model of LiFePO4-Graphite Li-ion Batteries: Power and Capacity Fade Simulations." *Journal of The Electrochemical Society*, 160(4), A616. — LFP OCV source.
- Afshar, S., et al. (2017). — LFP OCP parameterisation.
- Kim, G.-H., et al. (2011). "Multi-Domain Modeling of Lithium-Ion Batteries Encompassing Multi-Physics in Varied Length Scales." *Journal of The Electrochemical Society*, 158(8), A955. — NCA OCV source.
- Colclasure, A.M., et al. (2011). "Modeling detailed chemistry and transport for solid-electrolyte-interface in lithium-ion batteries." — LTO negative electrode parameters.
- Smith, K., et al. (2017). "Life prediction model for grid-connected Li-ion battery energy storage system." *IEEE American Control Conference*, 7963578. — Degradation model (Smith 2017).
- OCHRE Battery.py — OCV and degradation model reference implementation (NREL).
- PyBaMM (https://github.com/pybamm-team/PyBaMM) — Source of 51-point OCV LUTs.
