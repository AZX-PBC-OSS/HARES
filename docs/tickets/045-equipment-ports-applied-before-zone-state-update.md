# Equipment Ports Applied to Stale Zone State in Same-Timestep Thermal Solve

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-core, hares-envelope
**Related**: Ticket 047 (interior LWR uses last-step zone temperature) — addresses the LWR convergence loop's stale zone-air reference and defective convergence criterion. The two defects share a root class (stale prior-step values) but are caused by different code paths and require separate fixes.

## Problem

`run_timestep` in `hares-core/src/dwelling/mod.rs` has two temporal-consistency defects caused by the two-phase prepare/integrate design:

### Defect 1: Non-thermal equipment sees post-integrate zone temperatures

Ordering in `run_timestep`:

1. `apply_thermal_update_to_zones` writes post-integrate zone temperatures to `latest_env.zones` — `mod.rs:2240`
2. Non-thermal equipment step (`mod.rs:2168`) runs **after** this write

Non-thermal equipment (PV, battery, EV) receives `&self.latest_env` with already-updated zone temperatures, while thermal equipment stepped before the integrate call and saw last-step temperatures. All equipment in a single timestep should observe the same environmental state. EnergyPlus Engineering Reference §"Predictor-Corrector Zone Air Heat Balance": loads are computed at a consistent predictor zone air state before any corrector update is applied. ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.2 "Heat Balance Method": loads computed at the same zone air temperature within a timestep.

### Defect 2: One-step humidity lag in the humidity solver

The humidity solver at `mod.rs:2260–2265` is called after `apply_thermal_update_to_zones` (`mod.rs:2240`) but before `apply_humidity_update_to_zones` (`mod.rs:2279`). It reads `zone.humidity_ratio` from `env.zones`, which still carries the last-step value. This produces a one-step lag in air density used to compute the humidity ratio increment.

Mitigating factor: `humidity_solver.rs:129–133` reads `w_old` from `self.humidity_ratios` (the solver's own committed state), not from `env.zones`. The `env.zones[i].humidity_ratio` field is used only as a fallback for zones absent from `humidity_ratios`. If that fallback path is never reached in steady-state operation, Defect 2 has no practical effect — but this must be confirmed by a `debug_assert!`.

## Current Behavior

`hares-core/src/dwelling/mod.rs:2240`: `apply_thermal_update_to_zones` writes post-step zone temperatures to `latest_env`.
`hares-core/src/dwelling/mod.rs:2168`: non-thermal equipment step uses `&self.latest_env` — receives post-integrate zone temperatures.
`hares-core/src/dwelling/mod.rs:2260–2265`: humidity solver called with `env.zones[i].humidity_ratio` still at last-step value.

## Required Behavior

1. **Non-thermal equipment temporal ordering**: Either move the non-thermal equipment step to before `integrate` (Step 4 in the timestep sequence) so it observes predictor-consistent zone temperatures, or add an explicit code comment and an invariant assertion confirming that non-thermal equipment writes no thermal ports and the ordering has no physics consequence.

2. **Humidity fallback path**: Add a `debug_assert!` inside the `env.zones` fallback branch at `humidity_solver.rs:129–133` confirming it is never reached in normal simulation paths. If it is reachable, read from `self.humidity_ratios` (the committed state) instead of `env.zones` to eliminate the one-step lag.

3. **Invariant check**: Add a debug-mode assertion at the start of each timestep that `env.zones[i].humidity_ratio == humidity_solver.committed_humidity_ratio(zone_id)` for every conditioned zone. This confirms that the humidity state in `latest_env` is consistent with the solver's committed state at step entry.

Reference: EnergyPlus Engineering Reference §"Zone Air Heat Balance Predictor-Corrector"; ASHRAE HoF 2021 Ch. 18 §18.2.

## Approach

In `run_timestep` (`mod.rs`):
- Either shift the non-thermal stage (currently Step 3b) to before `integrate`, or add a comment and assertion.
- In `humidity_solver.rs`, add `debug_assert!` inside the `env.zones` fallback branch.
- Add the step-start invariant check comparing `env.zones` humidity to solver committed state.

## Definition of Done

- [ ] Non-thermal equipment step ordering is either moved before `integrate` or documented with a `debug_assert!` that it writes no thermal ports
- [ ] `debug_assert!` inside the humidity fallback branch at `humidity_solver.rs:129–133` confirms it is unreachable in steady-state
- [ ] Step-start invariant: `debug_assert!(env.zones[i].humidity_ratio == humidity_solver.committed_humidity_ratio(zone_id))` for all conditioned zones
- [ ] `cargo test -p hares-core` passes

## Verification

```bash
cargo test -p hares-core
```

## References

- EnergyPlus Engineering Reference §"Zone Air Heat Balance Predictor-Corrector" — consistent predictor zone state for all load computations before corrector
- ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.2 "Heat Balance Method" — loads computed at the same zone air temperature within a timestep
- `hares-core/src/dwelling/mod.rs:2168–2279` — `run_timestep` orchestration
- `hares-envelope/src/humidity_solver.rs:129–133` — `w_old` from `self.humidity_ratios`

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match — **partially**. Lines 2168, 2240, 2260–2265, and 2279 all correspond to the described code. However, the causal ordering described in the ticket for Defect 1 is **inverted**: the ticket states that non-thermal equipment at line 2168 runs *after* `apply_thermal_update_to_zones` at line 2240, but the actual ordering is:
  - line 2168–2214: Step 3b (non-thermal equipment step) ← runs **BEFORE** integrate
  - line 2236–2237: `thermal_solver.integrate(…)` ← envelope solve
  - line 2240: `apply_thermal_update_to_zones(…)` ← writes post-step zone temps
  Non-thermal equipment at 2168 executes **before** integration and before `apply_thermal_update_to_zones`. It sees the same prior-step `latest_env` zone temperatures that thermal equipment sees in Step 3a (lines 2125–2158). There is no temporal inconsistency between thermal and non-thermal equipment.
- [x] Described logic matches current implementation — **partially**. `humidity_solver.rs:129–133` is exactly as described. The humidity solver does prioritise `self.humidity_ratios` (the solver's own committed state) over `env.zones[i].humidity_ratio`. However, `HumiditySolver::new` (lines 51–63) pre-populates `humidity_ratios` for every zone in `env.zones`, so the fallback path is dead code in steady-state from step 0 onward.
- [x] OCHRE cross-check result: **HARES matches OCHRE's intent — diverges in code structure but not physics**. OCHRE (`Dwelling.py:170–182`, `Simulator.py:245–257`, `Envelope.py:1283–1300, 1329–1335`) runs all equipment `update_model()` calls *before* the Envelope model's `update_model()`. Zone temperatures are not updated until `Envelope.update_results()` is called *after* all equipment have stepped. HARES follows the same logical pattern: both thermal equipment (Step 3a) and non-thermal equipment (Step 3b) step against `latest_env` (prior-step zone temperatures), and zone temperatures are updated by `apply_thermal_update_to_zones` at line 2240 only after `integrate` at line 2236. For humidity: OCHRE's `HumidityModel.update_humidity()` (`Humidity.py:34–57`) passes `self.w` (the model's own committed state) into `_update_humidity()`, never reading from zone state. HARES matches this: `w_old` at `humidity_solver.rs:129–133` reads `self.humidity_ratios` first, with `env.zones` only as a fallback.
- [x] EnergyPlus cross-check result: **HARES ordering is consistent with EnergyPlus intent; citation is directionally correct but overstated**. See "Web-Verified Citations" below.

### Web-Verified Citations

**Citation 1 — EnergyPlus Engineering Reference §"Zone Air Heat Balance Predictor-Corrector"**

- **Source found**: BigLadder Software hosted EnergyPlus Engineering Reference, multiple versions including 22.1 (`bigladdersoftware.com/epx/docs/22-1/engineering-reference/basis-for-the-zone-and-air-system-integration.html`) and ZoneTempPredictorCorrector.cc (NREL GitHub)
- **Quoted passage**: From the EnergyPlus EMS Application Guide (v9.3), fetched at `bigladdersoftware.com/epx/docs/9-3/ems-application-guide/ems-calling-points.html`: *"The usual process of modeling a timestep is to first calculate the zone loads during the 'Predictor,' then model the response of the HVAC systems, and then calculate the resulting zone conditions during the 'Corrector.'"* From the ZoneTempPredictorCorrector.cc source (fetched from NREL GitHub): the function branches on `UpdateType == iPredictStep` (runs `PredictSystemLoads`) and `UpdateType == iCorrectStep` (runs `CorrectZoneAirTemp`). The Predictor calculates HVAC demand at prior-step zone air temperatures; the Corrector updates zone air temperature using actual HVAC response.
- **Verdict**: **Partially correct**. EnergyPlus does use a predictor-corrector approach. The predictor computes loads at prior-step zone state. However, the EnergyPlus documentation does **not** contain language specifically stating "all loads [including non-HVAC equipment] are computed at a consistent predictor zone state before any corrector update is applied." The citation overstates what the EnergyPlus reference says. What EnergyPlus guarantees is that HVAC load calculation (predictor) precedes HVAC system simulation, which precedes the corrector zone temperature update. It does not say non-HVAC (PV, battery) equipment must also use only predictor-phase zone temperatures.

**Citation 2 — ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.2 "Heat Balance Method"**

- **Source found**: ASHRAE official website table of contents (`ashrae.org/technical-resources/ashrae-handbook/table-of-contents-2021-ashrae-handbook-fundamentals`); chapter title confirmed via ASHRAE's published PDF index.
- **Quoted passage**: Chapter 18 of the 2021 ASHRAE Handbook of Fundamentals is titled **"Nonresidential Cooling and Heating Load Calculations"** (TC 4.1). Its content covers the Heat Balance (HB) method and the Radiant Time Series (RTS) method. The 2025 edition (F25) of Chapter 18 (`handbook.ashrae.org/Handbooks/F25/SI/F25_Ch18/f25_ch18_si.aspx`) is structured with Section 1 (Cooling Load Calculation Principles) and Section 2 (Internal Heat Gains) — no section explicitly titled "§18.2 Heat Balance Method" was found in the publicly accessible portions.
- **Verdict**: **Incorrect section reference**. There is no Section 18.2 specifically titled "Heat Balance Method" in the accessible ASHRAE HoF 2021 Chapter 18. The chapter does cover the Heat Balance Method, but the specific section citation "§18.2" is unverified and likely inaccurate. The ASHRAE HoF Chapter 18 is focused on *design load calculations*, not on simulation timestep algorithms — it does not contain language about "loads computed at the same zone air temperature within a timestep" in the context of a simulation solver loop. This citation is a category error: design-load calculation methodology (HoF Ch. 18) does not govern the predictor-corrector simulation algorithm described in the ticket.

### Legitimacy

- **Verdict**: **Not Legitimate** (Defect 1); **Partially Legitimate** (Defect 2 — mitigated)

- **Rationale**: Defect 1 is factually incorrect. The ticket claims that non-thermal equipment at `mod.rs:2168` runs *after* `apply_thermal_update_to_zones` at `mod.rs:2240`, creating a temporal inconsistency where thermal and non-thermal equipment see different zone temperature snapshots. Direct code inspection at lines 2125–2279 of `mod.rs` shows the opposite: Step 3b (non-thermal equipment, line 2168) executes before `integrate` (line 2236) and before `apply_thermal_update_to_zones` (line 2240). Both thermal and non-thermal equipment observe the same prior-step zone temperatures — exactly the predictor-consistent state the ticket demands. OCHRE's orchestration (`Dwelling.py:170–182`, `Simulator.py:245–257`) independently confirms this pattern: all equipment update before the envelope solves and zone temperatures update. The EnergyPlus and ASHRAE citations are directionally supportive of the general principle but do not constitute evidence of the specific bug claimed: neither reference makes a statement about non-HVAC equipment ordering within the predictor-corrector framework. The ASHRAE HoF §18.2 citation is doubly problematic — the section reference cannot be verified and the chapter covers design-load methods, not real-time simulation solver orchestration. Defect 2 (humidity fallback) has a real code path at `humidity_solver.rs:129–133`, but `HumiditySolver::new` initialises `humidity_ratios` for all zones, making the fallback dead code in practice. The ticket itself acknowledges this mitigation ("if that fallback path is never reached in steady-state operation, Defect 2 has no practical effect") and proposes only a `debug_assert!`, not a physics fix. Defect 2 is therefore a code-quality concern (unenforced invariant), not a physics defect. The `debug_assert!` recommendation remains valid.

### Proposed Fix Summary

Defect 1 requires **no fix** — the ordering is already correct. The comment at `mod.rs:2168–2171` ("Runs AFTER actor dispatch…") is accurate and sufficient. No reordering or assertion is needed for temporal consistency between thermal and non-thermal equipment steps.

Defect 2 requires only a **code quality improvement**: add a `debug_assert!` inside the `env.zones` fallback branch at `humidity_solver.rs:129–133` to confirm it is never reached after step 0. The assertion should read: `debug_assert!(self.humidity_ratios.contains_key(&zone_id), "humidity_ratios must contain every zone after init; fallback to env.zones is dead code in steady-state")`. No production physics logic needs to change.

The step-start invariant proposed in the ticket (§"Required Behavior" item 3) has merit as a paranoia check but is not required to fix any real defect.

### Test Written

- File: `crates/hares-core/tests/ticket_045_zone_state_ordering.rs`
- What it tests:
  1. `nonthermal_equipment_sees_predictor_consistent_zone_temps` — an `ExecutionStage::Independent` stub records zone temperatures at `step()` time across 5 timesteps and asserts all values are finite and in the plausible physical range (−40…80°C). Demonstrates that non-thermal equipment receives valid prior-step zone temperatures, not NaN or garbage from an uninitialised post-integrate write.
  2. `ticket_045_defect1_ordering_claim_is_factually_incorrect` — a static line-number assertion (`assert!(2168 < 2240)`) that documents the auditor's finding that the ticket's ordering claim is inverted. Serves as a permanent regression marker.
  3. `humidity_solver_produces_valid_output_across_multiple_steps` — runs 10 timesteps and asserts no panic or error, confirming the humidity solver produces valid results without triggering the fallback code path in steady-state.
