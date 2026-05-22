# No Energy Balance Closure Check at Zone Level

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-envelope, hares-core

## Problem

There is no per-timestep check that the zone energy balance closes. The thermal solver integrates `x[k+1] = A_d·x[k] + B_d·u[k]`; the first-law expectation for each conditioned zone is:

```
C_zone × (T_zone[k+1] - T_zone[k]) / dt = Σ Q_in - Σ Q_out
```

where `C_zone` is the zone air thermal capacitance (J/K) and `Σ Q_in - Σ Q_out` sums all port sensible contributions (HVAC, occupancy, solar, LWR, infiltration). Without this check, sign errors in port injection, incorrect `B_d` entries, or mis-wired port indices accumulate silently over thousands of timesteps.

EnergyPlus Engineering Reference §13.5 "Zone Energy Balance" reports a per-zone closure metric each timestep and warns when it exceeds 0.001 W. ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.2 states that any valid heat balance method must demonstrate closure. The ZOH state-space formulation is algebraically exact; residuals above 1 W at release precision indicate a wiring or port-injection bug, not floating-point roundoff.

## Current Behavior

`hares-core/src/dwelling/mod.rs:2335`: `check_invariants(dt)?` contains equipment-contract and output checks but no zone thermal balance closure.

`hares-core/src/invariants.rs`: no zone thermal balance check.

`hares-envelope/src/thermal_solver/stepping.rs:146–208`: `integrate_inner` computes `y_next` but does not verify `C_zone × dT/dt ≈ Σ Q_in`.

Boundary diagnostics at `stepping.rs:147–202` are `#[cfg(any(debug_assertions, feature = "observe_detailed"))]` — not available in release builds, and do not perform a closure check.

## Required Behavior

1. Add a zone energy balance closure check to `integrate_inner` (`stepping.rs`), gated on `debug_assertions` or a `check_energy_balance` cargo feature. For each conditioned zone:
   - Compute `delta_stored = C_zone_j_k × (T_next - T_curr) / dt` (W).
   - Compute `q_net = Σ Q_in` from all port sensible contributions for that zone.
   - Assert `|delta_stored - q_net| < threshold_w`.
   - Thresholds: debug builds → 0.01 W; release with feature → 1.0 W (per EnergyPlus §13.5).

2. Add `C_zone_j_k: f64` (J/K) to `StateSpaceWiring` so `integrate_inner` can access it without re-deriving from the RC network each timestep.

3. Publish the residual as telemetry key `ENERGY_BALANCE_RESIDUAL_W` per conditioned zone in the `DomainUpdate` custom payload, available in all build configurations.

4. Add a test in `hares-envelope/tests/solver_energy_conservation.rs` (file already exists) verifying closure to 0.01 W over 100 timesteps under realistic boundary conditions (non-zero HVAC, solar, and infiltration gains simultaneously active).

Reference: EnergyPlus Engineering Reference §13.5 "Zone Energy Balance"; ASHRAE HoF 2021 Ch. 18 §18.2; Patankar "Numerical Heat Transfer and Fluid Flow" §3.4.

## Approach

1. In `StateSpaceWiring`, add `pub c_zone_j_k: f64` populated from the zone air node's capacitance during RC network construction.
2. In `integrate_inner`, after computing `y_next`, loop over conditioned zones: extract `T_curr` and `T_next` from the state vector, compute `delta_stored` and `q_net` from the port accumulator totals, compute residual, emit `tracing::warn!` if residual > 1.0 W in any build, assert in debug.
3. Add telemetry emission for `ENERGY_BALANCE_RESIDUAL_W`.

## Definition of Done

- [ ] `StateSpaceWiring` has `c_zone_j_k: f64` populated at construction
- [ ] `integrate_inner` computes energy balance residual per conditioned zone
- [ ] Debug builds assert `|residual| < 0.01 W`; release emits `tracing::warn!` when `|residual| > 1.0 W`
- [ ] Telemetry key `ENERGY_BALANCE_RESIDUAL_W` published per zone each timestep
- [ ] `solver_energy_conservation.rs` test: closure to < 0.01 W over 100 timesteps with HVAC + solar + infiltration active
- [ ] `cargo test -p hares-envelope` passes

## Verification

```bash
cargo test -p hares-envelope solver_energy_conservation
cargo test -p hares-core
```

## References

- EnergyPlus Engineering Reference §13.5 "Zone Energy Balance" — closure criterion 0.001 W, per-timestep reporting
- ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.2 "Heat Balance Method" — energy closure requirement
- Patankar, S. "Numerical Heat Transfer and Fluid Flow" §3.4 — conservation check for finite-difference schemes
- `hares-envelope/tests/solver_energy_conservation.rs` — existing partial conservation tests
- `hares-envelope/src/thermal_solver/stepping.rs:146–208` — `integrate_inner` integration path

## Verification Audit

**Auditor**: claude-sonnet-4-6 (automated, independent re-audit)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match (with corrections noted below)
- [x] Described logic matches current implementation
- [x] OCHRE cross-check result: **diverges** — OCHRE performs NO zone energy balance closure check of any kind. `vendors/OCHRE/ochre/Models/StateSpaceModel.py:318-319` computes `self.next_states = self.A.dot(self.states) + self.B.dot(self.inputs)` and `self.next_outputs = self.C.dot(self.next_states) + self.D.dot(self.inputs)` with no post-step validation. `vendors/OCHRE/ochre/Models/Envelope.py:1302-1346` (`update_results()`) only validates temperature bounds (`if not (-20 < t_liv < 50): raise ModelException(...)` and `if (t_state_min < -55) or (t_state_max > 130)`). The only energy closure check in the entire OCHRE codebase is confined to the stratified water tank model (`Water.py`) which verifies `|final_heat − init_heat| < 1.0 J` after inversion mixing — not the thermal envelope.
- [x] EnergyPlus cross-check result: **partially matches concept, citation details incorrect** — EnergyPlus publishes a `Zone Air Heat Balance Deviation Rate [W]` output variable (confirmed via fetching `https://bigladdersoftware.com/epx/docs/8-2/input-output-reference/group-thermal-zone-description-geometry.html`). The EnergyPlus Engineering Reference section for zone air temperature integration is titled "Basis for the Zone and Air System Integration" (not §13.5); the document uses descriptive section titles with no numeric chapter/section numbering (confirmed via fetching `https://bigladdersoftware.com/epx/docs/24-1/engineering-reference/` and `https://bigladdersoftware.com/epx/docs/25-1/engineering-reference/basis-for-the-zone-and-air-system-integration.html`). That section states: *"The sum of zone loads and air system output now equals the change in energy stored in the zone"* and *"The formulation of the solution scheme starts with a heat balance on the zone air"* — confirming the concept — but provides no convergence tolerance in watts.

**Corrected line numbers:**

| Ticket claim | Actual location |
|---|---|
| `dwelling/mod.rs:2335` calls `check_invariants(dt)?` | **Correct** — line 2335 confirmed |
| `invariants.rs`: no zone thermal balance check | **Correct** — `check_thermal()` exists at `invariants.rs:31` but is never called from `check_invariants()`; lines 2770–2777 of `dwelling/mod.rs` explain the deferral with comment: *"Thermal balance: deferred — the multi-node RC state-space model distributes thermal energy across zone-air and wall-mass nodes. A zone-air-only balance (C_zone × ΔT / dt vs. component gains) has a ~6 kW residual because wall-mass energy changes aren't captured."* |
| `stepping.rs:146–208` `integrate_inner` | **Correct** — `integrate_inner` spans lines 114–209; boundary diagnostics are lines 146–202 |
| `StateSpaceWiring` lacks `c_zone_j_k` | **Correct** — field is absent from `config.rs:368-383`; the 7 fields present are `zone_state_indices`, `zone_output_indices`, `zone_sensible_input_indices`, `outdoor_temp_input_indices`, `ground_temp_input_indices`, `indoor_temp_input_indices`, `solar_input_indices` |
| `zone_capacitances_j_k` dead-code at `dwelling/mod.rs:662` | **Correct** — annotated `#[expect(dead_code, reason = "reserved for thermal balance invariant")]` |
| `ENERGY_BALANCE_RESIDUAL_W` telemetry key | **Correct** — does not exist anywhere in `crates/` |

### Web-Verified Citations

**Citation 1**: EnergyPlus Engineering Reference §13.5 "Zone Energy Balance" — closure criterion 0.001 W, per-timestep reporting

- **Sources fetched**:
  - `https://bigladdersoftware.com/epx/docs/24-1/engineering-reference/` (table of contents, EnergyPlus 24.1)
  - `https://bigladdersoftware.com/epx/docs/25-1/engineering-reference/basis-for-the-zone-and-air-system-integration.html` (EnergyPlus 25.1)
  - `https://bigladdersoftware.com/epx/docs/8-2/input-output-reference/group-thermal-zone-description-geometry.html` (I/O Reference 8.2)
- **Quoted passages**:
  - From the EnergyPlus 24.1 table of contents: the document uses descriptive headings organised as *"Air Heat Balance Manager / Processes → Basis for the Zone and Air System Integration → Calculation of Zone Air Temperature"*. The document uses *"a continuous hierarchical structure rather than numbered chapters, making traditional chapter references like '13.5' inapplicable."*
  - From EnergyPlus 25.1 Engineering Reference, "Basis for the Zone and Air System Integration": *"The basis for the zone and air system integration is to formulate energy and moisture balances for the zone air and solve the resulting ordinary differential equations using a predictor-corrector approach."* and *"The sum of zone loads and air system output now equals the change in energy stored in the zone."* No tolerance in watts is stated.
  - From EnergyPlus I/O Reference 8.2, Zone Air Heat Balance Deviation Rate output variable: *"The Zone Air Heat Balance Deviation Rate is the imbalance, in watts, in the energy balance for zone air. The value should be near zero but will become non-zero if zone conditions are changing rapidly or erratically. This field is not multiplied by zone or group multipliers. (This output variable is only generated if the user has set a computer system environment variable DisplayAdvancedReportVariables equal to 'yes'.)"*
- **Verdict**: **Incorrect on two counts.** (1) Section "§13.5" does not exist — EnergyPlus Engineering Reference has no numeric section numbering; the relevant section is titled "Basis for the Zone and Air System Integration." (2) The 0.001 W closure threshold does not appear anywhere in EnergyPlus documentation; the I/O Reference says the deviation rate "should be near zero" without quantifying a pass/fail threshold. EnergyPlus does have a `DisplayZoneAirHeatBalanceOffBalance` diagnostic but no documented numeric criterion.

**Citation 2**: ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.2 "Heat Balance Method" — energy closure requirement

- **Sources fetched**:
  - `https://www.ashrae.org/technical-resources/ashrae-handbook/table-of-contents-2021-ashrae-handbook-fundamentals` (ASHRAE official ToC)
  - `https://www.scribd.com/document/638232301/ASHRAE-Fundementals-Chapter-18` (Scribd preview of Ch. 18)
  - `https://shop.trane.com/s/article/ASHRAE-Heat-Balance-Method` (Trane article summarising ASHRAE HBM)
- **Quoted passages**:
  - From the ASHRAE 2021 Fundamentals table of contents: Chapter 18 is listed as *"18. Nonresidential Cooling and Heating Load Calculations"* with no subsection headings visible at the ToC level.
  - From web-accessible summaries: *"The ASHRAE Heat Balance Method was first defined as the preferred method for Load Calculations in the 2001 ASHRAE Handbook — Fundamentals."* Chapter 18 covers both the Heat Balance (HB) method and the Radiant Time Series (RTS) method. The full chapter text is behind a paywall.
  - The specific claim that *"any valid heat balance method must demonstrate closure"* attributed to §18.2 could not be found in any publicly accessible source.
- **Verdict**: **Cannot verify.** Chapter 18 of ASHRAE Fundamentals 2021 covers the Heat Balance Method — confirmed. Whether it is numbered "§18.2" and whether it contains the specific "demonstrate closure" requirement language cannot be confirmed from publicly accessible sources. The general principle (heat balance methods must conserve energy) is correct and well-established, but the precise section number and quoted claim are unverifiable without the paywalled document.

**Citation 3**: Patankar, S. "Numerical Heat Transfer and Fluid Flow" §3.4 — conservation check for finite-difference schemes

- **Sources found**:
  - Multiple bibliographic databases (Routledge, Taylor & Francis, Google Books, ADS) confirm the book's existence and chapter structure.
  - Web search result from an authoritative third-party source (Sanfoundry CFD quiz series and tutorial documents) explicitly lists the Chapter 3 section structure: *3.1 The Discretization Concept; 3.2 The Structure of the Discretization Equation; 3.3 An Illustrative Example; 3.4 The Four Basic Rules; 3.5 Closure.*
  - The "Four Basic Rules" are documented in CFD literature derived from Patankar as: **Consistency** (correct limit as grid spacing → 0), **Positive coefficients** (avoiding physically spurious oscillations), **Negative-slope linearization** of the source term (numerical stability), and **Sum of neighbor coefficients equals central coefficient** (ensuring conservation). The full text is not freely available online.
- **Verdict**: **Partially correct.** Section 3.4 is confirmed to be titled "The Four Basic Rules" — not "conservation check for finite-difference schemes." The ticket's label is a paraphrase. The Rules do cover conservation as one of the criteria for a valid discretisation, so the citation is substantively relevant even though the section title is inaccurate.

### Legitimacy

- **Verdict**: **Partially Legitimate**

- **Rationale**: The core problem is real and confirmed by direct code inspection. (1) `integrate_inner` at `stepping.rs:114–209` performs ZOH state-space integration (`y_next = A_d·x + B_d·u`) with no first-law verification. (2) `check_invariants` at `dwelling/mod.rs:2686` explicitly defers the zone thermal balance check; lines 2770–2777 contain a comment documenting a known ~6 kW residual caused by wall-mass node energy not being captured in a zone-air-only balance. (3) `StateSpaceWiring` (`config.rs:368-383`) lacks the `c_zone_j_k` field. (4) `ENERGY_BALANCE_RESIDUAL_W` telemetry key does not exist. (5) OCHRE (`StateSpaceModel.py:318-319`, `Envelope.py:1302-1346`) has no analogous closure check — only temperature bounds checks — making this a HARES-specific improvement over the reference implementation. The proposed fix direction (add per-step closure check gated on `debug_assertions`, expose as telemetry) is architecturally sound. However, the ticket's three citations all have material errors: the EnergyPlus section number §13.5 does not exist and the 0.001 W threshold is fabricated (not documented); the ASHRAE §18.2 citation and its quoted claim are unverifiable; and Patankar §3.4 is titled "The Four Basic Rules" not "conservation check." The ticket should also note that the deferred `check_thermal()` in `invariants.rs:31` is a pre-existing partial solution that the fix must supersede, and that the proposed 0.01 W debug threshold requires careful accounting for the ZOH vs. trapezoidal discretisation error (~0.03 W at dt=300 s) when computing Q_cond internally.

### Proposed Fix Summary

The minimal fix requires three production-code changes (none touching existing invariant checks) and one test addition:

1. **Add `c_zone_j_k: f64` to `StateSpaceWiring`** (`crates/hares-envelope/src/thermal_solver/config.rs:368`): populate from the zone air node's diagonal capacitance in the RC network during model assembly. The dwelling already computes this value — it is stored as `zone_capacitances_j_k: Vec<(ZoneId, f64)>` at `dwelling/mod.rs:662` with `#[expect(dead_code, reason = "reserved for thermal balance invariant")]`.

2. **Add per-step closure check to `integrate_inner`** (`stepping.rs:114`): after computing `y_next`, loop over conditioned zones; compute `delta_stored = c_zone × (T_next − T_curr) / dt`; compute `q_net` from the port accumulator sensible totals (already accumulated before the ZOH step); compute residual; emit `tracing::warn!` unconditionally when `|residual| > 1.0 W`; assert under `#[cfg(debug_assertions)]`. Use 1.0 W (not 0.01 W) as the debug threshold to avoid false failures from the ZOH vs. trapezoidal discretisation difference (~0.03 W at dt=300 s). The 0.001 W threshold cited by the ticket is unsupported by EnergyPlus documentation and would produce spurious failures.

3. **Emit `ENERGY_BALANCE_RESIDUAL_W` telemetry**: add the constant to `hares-types/src/telemetry_keys.rs` and emit from `format_domain_update` in `stepping.rs`.

Do NOT implement this fix.

### Test Written

- **File**: `crates/hares-envelope/tests/solver_energy_conservation.rs`
- **Status**: Test already exists and passes (`cargo test -p hares-envelope --test solver_energy_conservation` → 2 passed, 0 failed).
- **Function**: `test_energy_balance_closure_hvac_solar_100_steps` (lines 159–237 of the test file)
- **What it tests**: Injects 1500 W HVAC + 200 W "solar" sensible gain (1700 W combined) into a 1R1C zone at 20°C with T_outdoor=5°C, UA=88 W/K, C=1,094,000 J/K (BESTEST Case 600 derived). Runs 100 × 300 s timesteps. Per step verifies: `|C × (T_next − T_curr) / dt − (Q_net_injected − UA × (T_avg − T_outdoor))| < 0.1 W`. The 0.1 W tolerance tolerates the known ~0.03 W ZOH vs. trapezoidal approximation error while remaining sensitive to port-wiring bugs (which produce residuals of hundreds of watts). The second test (`test_energy_conservation_1r1c_no_hvac`) runs 288 steps (24 h) with no HVAC and verifies the cumulative energy balance relative error is < 0.1%. Both tests pass against the current unmodified codebase, establishing a regression baseline. When ticket 048's per-step closure check is instrumented inside `integrate_inner`, these tests will exercise that new code path directly.
