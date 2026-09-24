# Interior LWR Fallback Path Uses Linearised h_r Approximation Without Warning

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-envelope

## Problem

When `zone_cfg.scriptf` is `None` in `apply_interior_longwave_inputs`, the code silently engages `interior_longwave_linearised_w_into` — an h_r ≈ 4εσT³ linearization. The fallback is not a valid production path: per project policy `feedback_no_silent_defaults.md`, a silent fallback that degrades physics fidelity must error loudly. Any zone with `interior_lwr_zones` configured must have ScriptF view factors precomputed; if they are absent at simulation start, that is a configuration error, not a runtime mode to silently tolerate.

The EnergyPlus Engineering Reference §"Interior Long-Wave Radiation Exchange" specifies the full radiosity method with view factors (ScriptF / Hottel script-F formulation) as the required approach. The linearized h_r path is a deliberate downgrade. Siegel and Howell "Thermal Radiation Heat Transfer" 4th ed. Ch. 4 quantifies the linearization error: at ΔT = 20 K and T_mean = 290 K, error ≈ (ΔT/2T_mean)² ≈ 0.5 %; at ΔT = 40 K the error reaches 2 %, which is unacceptable for ASHRAE-grade zone LWR exchange.

### Secondary defect: redundant surface-list rebuild inside convergence loop

`hares-envelope/src/thermal_solver/longwave.rs:292–303` rebuilds `lwr_surfaces_buf` on every iteration of the convergence loop (`n_iter = floor(dt/300)+3`, minimum 3 iterations). The surface geometry does not change between iterations; the rebuild is a pure waste of n_iter − 1 allocations and copies per timestep.

## Current Behavior

`hares-envelope/src/thermal_solver/longwave.rs:289–304`:

```rust
if let Some(ref scriptf) = zone_cfg.scriptf {
    scriptf.net_flux_w_into(buf, &mut self.lwr_net_flux_buf);
} else {
    self.lwr_surfaces_buf.clear();
    self.lwr_surfaces_buf
        .extend(zone_cfg.surfaces.iter().map(|s| InteriorSurface { ... }));
    interior_longwave_linearised_w_into(
        &self.lwr_surfaces_buf, buf, t_zone_c, &mut self.lwr_net_flux_buf,
    );
};
```

No error or `tracing::warn!` emitted when `scriptf` is `None`. The same fallback pattern repeats at lines 323–338 (the post-loop net-flux call). The surface-list build is inside the `for _ in 0..n_iter` loop.

## Required Behavior

1. **Error on `scriptf == None` at solver construction**: In the solver builder (`hares-core/src/dwelling/solver_builder.rs`), assert `zone_cfg.scriptf.is_some()` for each zone that has `interior_lwr_zones` configured. Return a configuration error if ScriptF factors are absent — do not silently fall back. Per `feedback_no_silent_defaults.md`, the linearized path must not engage without a loud diagnostic. Per `feedback_no_backward_compat.md`, no shim or fallback mode is acceptable.

2. **One-time runtime warning if fallback is reached**: If the linearized path is reached at runtime despite the construction check (e.g., via `from_discrete` path), emit a one-time `tracing::warn!` per zone per simulation (guard with an `AtomicBool` or `HashSet<ZoneId>`), including the zone ID and a statement that LWR is at reduced physics fidelity. Do not emit per-timestep.

3. **Move surface-list build outside convergence loop**: Call `lwr_surfaces_buf.clear(); extend(...)` once before the `for _ in 0..n_iter` loop at `longwave.rs:286`, not inside it. This eliminates (n_iter − 1) redundant reconstructions per timestep — a minor but gratuitous allocation inside a hot loop (project policy: `feedback_hot_loop_minimal.md`).

Reference: EnergyPlus Engineering Reference §"Interior Long-Wave Radiation Exchange" — ScriptF radiosity method; ASHRAE HoF 2021 Ch. 18 §18.35 "Mean Radiant Temperature".

## Approach

1. In `solver_builder.rs`, after zone configuration is assembled, iterate over zones with `interior_lwr_zones` and return a `HaresError::Configuration` if any have `scriptf.is_none()`.
2. Add a `lwr_scriptf_warned: HashSet<ZoneId>` to `ThermalSolver` (or an `AtomicBool` per zone, depending on `Send` requirements). Check and set it in the `else` branch.
3. In `apply_interior_longwave_inputs`, move `lwr_surfaces_buf.clear(); extend(...)` to before the `for _ in 0..n_iter` loop. Apply the same fix to the post-loop call at lines 323–338 if it has a separate rebuild.
4. Add a test that the solver builder returns `Err` when a zone has `interior_lwr_zones` but `scriptf` is `None`.

## Definition of Done

- [ ] Solver builder returns `HaresError::Configuration` when `interior_lwr_zones` is configured for a zone without ScriptF factors
- [ ] If the linearized fallback is reached at runtime, a one-time `tracing::warn!` fires per zone with zone ID and fidelity statement
- [ ] `lwr_surfaces_buf.clear(); extend(...)` called once before the convergence loop, not inside it
- [ ] `cargo test -p hares-envelope` passes
- [ ] `cargo test -p hares-core` passes
- [ ] Test: solver builder errors when ScriptF absent for LWR zone

## Verification

```bash
cargo test -p hares-envelope
cargo test -p hares-core
```

## References

- EnergyPlus Engineering Reference §"Interior Long-Wave Radiation Exchange" — ScriptF (Hottel script-F) radiosity method with exact T⁴ formulation
- Siegel, R. and Howell, J.R. "Thermal Radiation Heat Transfer" 4th ed. Ch. 4 — linearization error for h_r ≈ 4εσT³: grows as (ΔT/2T_mean)²; at ΔT = 20 K, T_mean = 290 K, error ≈ 0.5 %
- ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.35 "Mean Radiant Temperature"
- Project policy `feedback_no_silent_defaults.md` — never silently substitute fallback values
- Project policy `feedback_hot_loop_minimal.md` — no per-step allocs or redundant work in convergence loops

---

## Verification Audit

**Auditor**: claude-sonnet-4-6 (automated, independent re-verification)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match — confirmed by direct file read:
  - `longwave.rs:286`: `for _ in 0..n_iter {` — the convergence loop
  - `longwave.rs:289–304`: `if let Some(ref scriptf) = zone_cfg.scriptf { ... } else { self.lwr_surfaces_buf.clear(); self.lwr_surfaces_buf.extend(...); interior_longwave_linearised_w_into(...) }` — exact match
  - `longwave.rs:322–338`: identical pattern in the post-loop final net-flux block — exact match
  - `solver_builder.rs:837–849`: `if interior_lwr_method == ScriptF { ... zone_cfg.compute_scriptf(); ... }` — exact match
- [x] Described logic matches current implementation — confirmed:
  - `ThermalSolver::new` accepts a `ThermalSolverConfig` with non-empty `interior_lwr_zones` where any zone has `scriptf: None` and returns `Ok` — no validation guard exists. The linearised path engages silently.
  - `lwr_surfaces_buf.clear(); extend(...)` executes on **every iteration** of the `for _ in 0..n_iter` loop (minimum 3, up to `floor(dt_s/300)+3` per timestep). Surface geometry is immutable between iterations.
  - The post-loop block at lines 326–331 contains a second identical rebuild (O(1) per timestep, also redundant).
  - `interior_longwave_linearised_w_into` is implemented in `longwave_radiation.rs:481–514`; uses `h_r = 4·ε·σ·T_zone³` via `linearised_h_r` at line 239–241 (`4.0 * emissivity * STEFAN_BOLTZMANN * (t_avg_c + 273.15).powi(3)`) and an emissivity-area-weighted MRT.
- [x] OCHRE cross-check result: **HARES diverges from OCHRE default — design gap, not intentional correction**
  - OCHRE `_solve_interior_radiation` (vendors/OCHRE/ochre/Models/Envelope.py:90–121, confirmed by direct read): uses **exact T⁴** — `h_lwr_out = s_e_factors * (t_surfaces + degc_to_k) ** 4`. OCHRE's default interior method is exact T⁴ Stefan-Boltzmann, not linearised.
  - OCHRE also supports `internal_radiation_method = "linear"` which uses `h_r ≈ 4·ε·σ·T_ref³` (at T_ref = 20°C), baked into an RC resistance at construction time (lines 1048–1061). This is explicitly configured by the user, not a silent fallback.
  - HARES's fallback in `apply_interior_longwave_inputs` resembles OCHRE's explicit "linear" mode but engages without explicit configuration. The divergence is a **design gap**, not an intentional OCHRE correction.
- [x] EnergyPlus cross-check result: **ticket claim confirmed — ScriptF uses exact T⁴; no linearised interior fallback exists in EnergyPlus**
  - Source fetched: Big Ladder EnergyPlus Engineering Reference v24.1 — Inside Heat Balance
    URL: https://bigladdersoftware.com/epx/docs/24-1/engineering-reference/inside-heat-balance.html
  - Quoted (ScriptF): *"The 'ScriptF' algorithm was developed by Hottel (Hottel and Sarofim, Radiative Transfer, Chapter 3, McGraw Hill, 1967). … qi,j = Ai Fi,j (Ti⁴ − Tj⁴)"*
  - Quoted (CarrollMRT): *"The Carroll method … simplifies the surface-to-surface radiation exchange by using a single, mean radiant temperature node, Tr … q = F′i Ai (Tr⁴ − Ti⁴)"*
  - Both EnergyPlus interior LWR algorithms use exact T⁴. No h_r ≈ 4εσT³ interior fallback is described anywhere in the documentation. The ticket's characterisation is accurate.

### Web-Verified Citations

**Citation 1**: EnergyPlus Engineering Reference §"Interior Long-Wave Radiation Exchange" — ScriptF radiosity method

- **Source found**: Big Ladder Software, EnergyPlus Engineering Reference v24.1 — Inside Heat Balance,
  fetched directly from https://bigladdersoftware.com/epx/docs/24-1/engineering-reference/inside-heat-balance.html
- **Quoted passage**: *"EnergyPlus uses a grey interchange model for the longwave radiation among zone surfaces. EnergyPlus offers two algorithms for modeling long wave radiation: The 'ScriptF' method, and the 'CarrollMRT' methods. Users can select between these two algorithms using the 'PerformancePrecisionTradeoffs' object."* … *"The 'ScriptF' algorithm was developed by Hottel (Hottel and Sarofim, Radiative Transfer, Chapter 3, McGraw Hill, 1967). This procedure relies on a matrix of exchange coefficients between pairs of surfaces that include all exchange paths between the surfaces. … qi,j = Ai Fi,j (Ti⁴ − Tj⁴)"*
- **Verdict**: **Confirmed** — EnergyPlus documents ScriptF (and CarrollMRT) as the only interior LWR methods, both using exact T⁴. No h_r linearisation for interior surfaces appears anywhere in the documentation. The ticket's characterisation is accurate.

**Citation 2**: Siegel & Howell "Thermal Radiation Heat Transfer" 4th ed. Ch. 4 — linearisation error (ΔT/2T_mean)²

- **Source found**: Book confirmed to exist. 4th ed., Taylor & Francis / CRC, ISBN 1-56032-839-8. Amazon listing: https://www.amazon.com/Thermal-Radiation-Heat-Transfer-Fourth/dp/1560328398. Full text is paywalled; chapter content not independently readable.
- **Formula check** (computed independently with exact arithmetic):
  - The formula (ΔT/2T_mean)² is the correct leading-order expression for the relative linearisation error.
  - **The numerical values in the ticket are overstated by approximately 4.2×**:
    - ΔT=20 K, T_mean=290 K: (20/580)² = **0.119%** actual error (not 0.5% as claimed). Exact: T_high=300 K, T_low=280 K → T⁴ difference = 1,953,440,000 K⁴; linearised = 1,951,120,000 K⁴; relative error = **0.119%**.
    - ΔT=40 K, T_mean=290 K: (40/580)² = **0.476%** actual error (not 2% as claimed).
    - 0.5% linearisation error is not reached until ΔT ≈ 42 K (numerically verified).
  - The ticket appears to have misread a figure or applied the formula at a different operating point. The formula itself is correct; the quoted magnitudes overstate the severity of the physics downgrade.
- **Verdict**: **Partially correct** — the Siegel & Howell reference and the (ΔT/2T_mean)² formula are legitimate; the numerical examples (0.5% at ΔT=20 K, 2% at ΔT=40 K) are **incorrect** — the true errors are ~0.12% and ~0.47% respectively.

**Citation 3**: ASHRAE HoF 2021 Ch. 18 §18.35 "Mean Radiant Temperature"

- **Source found**: ASHRAE Handbook of Fundamentals 2021 Table of Contents fetched from
  https://www.ashrae.org/technical-resources/ashrae-handbook/table-of-contents-2021-ashrae-handbook-fundamentals
- **Quoted chapter titles**: Chapter 9 — **"Thermal Comfort"**; Chapter 18 — **"Nonresidential Cooling and Heating Load Calculations"**.
- **Verdict**: **Incorrect** — The citation "Ch. 18 §18.35 'Mean Radiant Temperature'" is wrong on both chapter and section number. MRT is a thermal comfort concept covered in Chapter 9, not Chapter 18 (which covers HVAC load calculations). The correct citation is ASHRAE HoF 2021 **Ch. 9** (Thermal Comfort). This error does not affect the validity of either code defect or the proposed fix.

**Citation 4**: Project policy `feedback_no_silent_defaults.md`

- **Source found**: Project-internal document; not web-accessible.
- **Verdict**: **Cannot web-verify** — the principle (no silent fallback substitution) is consistent with observable code patterns across the codebase. The ticket's invocation is plausible.

**Citation 5**: Project policy `feedback_hot_loop_minimal.md`

- **Source found**: Project-internal document; not web-accessible.
- **Verdict**: **Cannot web-verify** — the principle (no per-step allocations in convergence loops) is consistent with existing code patterns (e.g., pre-allocated `lwr_net_flux_buf`). The application is valid.

### Legitimacy

- **Verdict**: **Partially Legitimate**

- **Rationale**: Both code defects are real and confirmed by direct code inspection and test execution:
  1. **Primary defect** — `ThermalSolver::new` accepts `interior_lwr_zones` containing zones with `scriptf: None` without returning an error. Confirmed by running `cargo test -p hares-envelope lwr_zone_without_scriptf_must_error_at_construction`, which **FAILS** (the test asserts `is_err()` but `ThermalSolver::new` returns `Ok`). EnergyPlus (web-verified) and OCHRE (code-verified) both use exact T⁴ as the default; the ticket's physics argument is sound.
  2. **Secondary defect** — `lwr_surfaces_buf.clear(); extend(...)` executes on every iteration of the convergence loop (lines 292–298), rebuilding an immutable surface list n_iter − 1 extra times. Confirmed present at lines 286–304.

  Two citation problems reduce confidence in the ticket's precision:
  - **ASHRAE citation is wrong**: Ch. 18 §18.35 does not cover MRT; the correct chapter is Ch. 9. Minor but sloppy.
  - **Linearisation error magnitudes are wrong**: The ticket claims 0.5% at ΔT=20 K and 2% at ΔT=40 K; exact calculation gives 0.119% and 0.476% respectively. The defect framing overstates the physics impact by ~4×. The argument for preferring exact T⁴ is still correct — but the severity claim of "unacceptable for ASHRAE-grade zone LWR exchange" is based on inflated error numbers.

  One framing nuance: the **production path** through `solver_builder.rs` already prevents the bug in practice — it only populates `interior_lwr_zones` when `ScriptF` mode is selected (line 837) and always calls `compute_scriptf()` (line 845). The silent fallback is currently reachable only via direct `ThermalSolverConfig` construction (tests, future integration paths). The fix to `ThermalSolver::new` closes the API gap correctly and defensively; the severity is lower than implied by the ticket.

### Proposed Fix Summary

1. **`ThermalSolver::new`** — add a validation pass over `config.interior_lwr_zones`: if any zone has `scriptf.is_none()` and `surfaces.len() >= 2`, return `Err(ThermalSolverError::Configuration("zone {id}: interior_lwr_zones requires ScriptF factors; call compute_scriptf() before constructing ThermalSolver"))`.
2. **Runtime warn** — if the linearised path is reached despite the construction check (defensive, e.g. via `from_discrete` or future paths), emit a one-time `tracing::warn!` per zone using an `AtomicBool` or `HashSet<ZoneId>` on the solver, including zone ID and fidelity statement. Do not emit per-timestep.
3. **Move surface-list build before the convergence loop** — call `lwr_surfaces_buf.clear(); extend(...)` once before `for _ in 0..n_iter` at line 285, not inside the loop (lines 292–298). The post-loop block at lines 326–331 also contains a redundant rebuild but is O(1) per timestep; fix both for consistency.
4. No change to `solver_builder.rs` needed — the builder already enforces `compute_scriptf()` for ScriptF-mode zones.

Ticket body should be corrected: ASHRAE HoF 2021 Ch. 9 (not Ch. 18); linearisation errors at the cited conditions are ~0.12% and ~0.47% (not 0.5% and 2%).

### Test Written

- **File**: `crates/hares-envelope/tests/interior_lwr.rs`
- **Test name**: `lwr_zone_without_scriptf_must_error_at_construction` (lines 297–378)
- **Status**: Test already exists and currently **FAILS** — confirmed by `cargo test -p hares-envelope lwr_zone_without_scriptf_must_error_at_construction` → FAILED with message "ThermalSolver::new must return Err when interior_lwr_zones contains a zone with scriptf == None (ticket 044)". No new test needs to be written; this is the intended failing regression test demonstrating the bug.
