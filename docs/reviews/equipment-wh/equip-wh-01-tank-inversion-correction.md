# Water heater tank PAV inversion correction effectiveness
**Review ID**: equip-wh-01
**Category**: equipment-wh
**Date**: 2026-05-26

## Files Reviewed
crates/hares-equipment/src/water_heater/tank.rs

## Vendor/Reference Files Consulted
vendors/EnergyPlus/src/EnergyPlus/WaterThermalTanks.cc vendors/OCHRE/ochre/Equipment/WaterHeater.py vendors/OCHRE/ochre/Models/Water.py

## Findings
### Finding 1: [Severity: medium]
**Description**: Inversion mixing (`mix_inversions`) runs **after** conduction/standby loss calculation within the same `step()` call, so the reported `last_skin_loss_w` and zone-coupled heat gains are computed from the **pre-mixing** temperature profile. EnergyPlus explicitly adjusts `Tavg` (the time-averaged temperature) during inversion correction to reflect the adiabatic mixing energy transfer, and all subsequent bookkeeping—skin losses (`Qloss`), use-side energy (`Euse`), source-side energy (`Esource`), and unmet load (`Eunmet`)—uses the post-mixing `Tavg`. HARES does not perform a comparable adjustment.

**Code Location**: `tank.rs:302-330` (`step()`) and `tank.rs:341-421` (`step_tempered()`). The ordering is: (1) `apply_conduction_and_standby` at line 320/390, which computes `last_skin_loss_w`, then (2) heat injection, (3) draw, (4) `mix_inversions` at line 328/395.

**Root Cause**: The explicit Euler integration in HARES separates conduction/standby as a discrete operator step before mixing. In contrast, EnergyPlus uses an analytical ODE solution within sub-timesteps (line 8400–8450 of `WaterThermalTanks.cc`) where inversion mixing runs on `Tfinal` and simultaneously corrects `Tavg[k] += Q_AdiabaticMixing * AvgFactorMixing` so that all loss/heat bookkeeping uses the physically stable mixed profile.

**Impact**: For typical residential water heater simulations where inversions are transient and driven primarily by bottom-element heating (e.g., 4.5 kW in a ~200 L tank), the magnitude of the discrepancy is small. However, for heat-pump water heaters with large source-side heat injections at a single node, inversions can be significant (10–20 K). In these cases, skin loss computed from the unmixed profile may differ from the mixed-profile loss by roughly `ΔT_inversion × UA_node`, potentially 5–50 W for a single step. Over a simulation year this could accumulate to a non-negligible energy balance drift.

### Finding 2: [Severity: low]
**Description**: The PAV (pointer-and-value) algorithm in `mix_inversions` is a **single-pass** implementation (one top-to-bottom scan with an inner cascading-merge while loop). EnergyPlus uses an **iterative do-while loop** with a `HasInversion` flag that restarts scanning from the top after each merge group (line 8401–8450). OCHRE similarly uses an iterative/nodal approach that can re-scan from the top after a correction (Water.py:108–149, called at line 420).

**Code Location**: `tank.rs:464-526` (single pass through `node_temps_c`). E+ iterative restart at `WaterThermalTanks.cc:8446` (`break` out of the for-loop, then `do { } while (HasInversion)` at 8450).

**Root Cause**: The HARES PAV approach uses a stack-based running merge where each time a new node is pushed onto `scratch_inversion_temps`, a `while` loop (line 475–497) cascades the merge upward through all stack layers until monotonicity is restored. This is equivalent in output to the iterative restart for all tested profiles (confirmed by the deterministic fixture test at `tank.rs:1346-1382` and manual trace of complex multi-inversion cascades in this review).

**Impact**: No correctness issue found. Both approaches produce identical post-mixing temperatures for common profiles because the cascading inner while loop handles multi-level inversions in a single forward pass. The iterative restart in E+ gives marginally better protection against numerical corner cases (e.g., near-equal temperatures where floating-point comparison could differ), but the HARES debug assertion at line 514–523 catches any residual inversions that would escape. No change required unless HARES needs to match E+ behavior for formal verification.

### Finding 3: [Severity: low]
**Description**: Volume-weighted temperature averaging during `mix_inversions` does not exactly conserve total internal energy Σ(ρ(T_i)·V_i·Cp·T_i) when water density is temperature-dependent. The merged temperature is computed as `(T1·V1 + T2·V2) / (V1 + V2)`, but the true energy-conserving merge would require solving for T_merge such that ρ(T_merge)·(V1+V2)·T_merge = ρ(T1)·V1·T1 + ρ(T2)·V2·T2. Both EnergyPlus and OCHRE accept the same approximation.

**Code Location**: `tank.rs:484-487`. Test documentation at lines 1010-1019 acknowledges the error as ~7,000 J for a 10-node full inversion on a ~42 MJ tank (~0.017%) and sets a 10 kJ tolerance. E+ at `WaterThermalTanks.cc:8418` also uses mass-weighted (volume-weighted with constant density) averaging.

**Impact**: Negligible for residential-scale tanks. The 7 kJ discrepancy is four orders of magnitude below the tank's thermal mass. This is a deliberate simplification shared across all three implementations. No action required.

### Finding 4: [Severity: low]
**Description**: The `mix_inversions` algorithm correctly handles top-most (node 0) and bottom-most (last node) boundaries. The stack-based approach naturally processes the top node first (it is pushed onto the scratch stack first) and the bottom node last. For the `n_nodes = 1` edge case, the function returns 0 merges trivially because the while loop guard (`len() >= 2`) never triggers.

**Code Location**: `tank.rs:470-498` — the for-loop iterates all nodes, each pushed in order. Boundary correctness is validated by a debug assertion at line 512–523 that checks the final profile for monotonicity, plus the dedicated tests `tank_draw_triggers_inversion_mixing` (line 1839, 4-node tank, large draw) and `tank_mix_inversions_identical` (line 1577, 6-node full profile inversion).

**Impact**: No boundary-related energy leaks found. The test `inversion_mixing_conserves_energy` (line 1002, 10 nodes) confirms energy is conserved to within the density-approximation tolerance. No action required.

### Finding 5: [Severity: low]
**Description**: Under positive displacement draw with cold mains water injection at the bottom, the `apply_draw` function (tank.rs:593–672) shifts water content downward, with mains filling the bottommost region. Because the pre-draw profile is monotone non-increasing (corrected by mix_inversions in the previous step), and cold mains is naturally denser than the tank water above it, the resulting post-draw profile is **also monotone non-increasing** — no inversions are created by the draw itself. Element heating (especially bottom-element or heat-pump condenser heating) is the dominant source of inversions, and those are corrected by `mix_inversions`.

**Code Location**: Draw displacement at `tank.rs:634-665`, inversion mixing call at `tank.rs:328`.

**Impact**: The concern about "residual inversions propagating into the next step's conduction calculation" is not applicable to the draw path. The only inversions that could reach `apply_conduction_and_standby` are those created by heating within the current step (which run after conduction) or those from the previous step that `mix_inversions` failed to resolve. The debug assertion at line 512–523 verifies that all inversions are resolved by `mix_inversions`. No residual inversions will propagate. No action required.

## Summary
- Total findings: 5
- Critical: 0 / High: 0 / Medium: 1 / Low: 4

## Recommendations
1. Consider recomputing, or adjusting, `last_skin_loss_w` after `mix_inversions` to match EnergyPlus's approach of using post-mixing average temperatures for all loss bookkeeping. The simplest fix would be to call `apply_conduction_and_standby` a second time after `mix_inversions` (with `dt=0` to record the corrected skin loss) or to store both pre-mixing and post-mixing skin loss values. Evaluate impact for heat-pump water heater configurations where inversions are largest.

2. No changes needed for the PAV algorithm itself — the single-pass approach is equivalent to E+'s iterative method for tested profiles, and the debug assertion provides a safety net for any undiscovered corner cases.

3. No changes needed for volume-weighted averaging — the density nonlinearity approximation is industry-standard (E+/OCHRE accept the same trade-off).

## References / Citations
- EnergyPlus Engineering Reference §14.8 (Stratified Tank Model) — inversion mixing after ODE solution with Tavg adjustment
- `WaterThermalTanks.cc:8400-8450` — EnergyPlus inversion mixing implementation (iterative do-while with HasInversion flag)
- `WaterThermalTanks.cc:8466-8519` — EnergyPlus bookkeeping using Tavg (post-mixing) for Qloss, Euse, Esource, Eunmet
- `Water.py:107-149` — OCHRE `_inversion_mixing` (numba-jitted, iterative with single-node adjustment and thermal check)
- `Water.py:383-393` — OCHRE `run_inversion_mixing_rule` called after model update when inversions detected
- `tank.rs:464-526` — HARES `mix_inversions` (single-pass PAV with cascading inner while loop)
- `tank.rs:302-330` — HARES `step()` operator ordering: conduction → heat injection → draw → mixing
- `tank.rs:550-587` — HARES `apply_conduction_and_standby` (explicit Euler, skin loss from pre-mixing profile)
- `tank.rs:1346-1382` — Deterministic PAV fixture test confirming equivalence to iterative approach
- `tank.rs:1038-1718` — Multi-step roundtrip and energy conservation tests
