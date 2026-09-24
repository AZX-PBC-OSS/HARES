# Warm-Up Period Optional and Not Enforced for HPXML Path

**Severity**: High
**Priority**: P1
**Status**: Open
**Areas**: hares-core

## Problem

`DwellingConfig.initialization_duration` is `Option<StdDuration>` and defaults to `None` at `hares-core/src/dwelling/mod.rs:107`. When `None`, `run_warmup` is never called (`mod.rs:1180–1181`) and the simulation starts from steady-state initial conditions produced by `initialize_steady_state`. All HPXML-path callers via `Dwelling::from_hpxml` (`mod.rs:762–767`) set `initialization_duration: None` by default.

For heavyweight construction (concrete slab, masonry), the thermal time constant τ = RC is measured in days to weeks. The steady-state solve pins the conditioned zone temperature but leaves wall and slab nodes at temperatures derived from a single weather-hour snapshot. EnergyPlus Engineering Reference §"Warmup Convergence" specifies an iterative warm-up: repeat the first simulation day until zone temperatures change by less than 0.5 °C between consecutive iterations, up to 25 iterations. Without warm-up, the first 1–7 days of heat-transfer predictions are systematically biased, distorting peak load, HVAC runtime, and energy balance closure. ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.4 "Transient Conduction" requires initial conditions to represent multi-day thermal history for heavy construction.

OCHRE CLI (`vendors/OCHRE/ochre/cli.py:39`) defaults to `initialization_time = timedelta(days=1)`. HARES produces no warm-up for any HPXML simulation.

## Current Behavior

`hares-core/src/dwelling/mod.rs:765`: `initialization_duration: None`.
`hares-core/src/dwelling/mod.rs:1180–1181`:
```rust
if let Some(init_dur) = config.initialization_duration {
    dwelling.run_warmup(init_dur)?;
```
No warm-up runs when `None`.

`hares-core/src/dwelling/mod.rs:1920–1931`: `run_warmup` executes N forward steps then clears results and resets the clock. This is a fixed-duration forward warm-up, not an iterative convergence check.

BESTEST fixture `tests/fixtures/bestest/600ff.toml` uses `initialization_duration_s = 86400` only because the test author set it explicitly.

## Required Behavior

**Preferred**: Implement an iterative warm-up convergence check. Repeatedly run the first 24-hour weather day until the maximum zone temperature change between consecutive iterations is below 0.5 °C (EnergyPlus Engineering Reference §"Warmup Convergence": 0.5 °C threshold, max 25 iterations). Emit `tracing::info!` reporting the converged iteration count. For lightweight construction this converges in 1–2 iterations; for heavyweight, 4–7.

**Minimum acceptable**: Set a non-`None` default warm-up duration in the HPXML path. 7 days (604 800 s) is conservative: for residential concrete walls (τ ≈ 3 days), 7 days covers 2.3 time constants, decaying initial error below 10 %. This is consistent with EnergyPlus's 25 × 1-day iterative equivalent for standard residential construction.

In either case, `initialization_duration` must not silently remain `None` for HPXML callers. Emit `tracing::warn!` at simulation start when the value is `None` — per project policy `feedback_no_silent_defaults.md`, a missing warm-up is not a safe default for production simulations.

## Approach

1. In `Dwelling::from_hpxml` at `mod.rs:762–767`, replace `initialization_duration: None` with a 7-day default unconditionally (or derive from wall assembly effective capacitance for heavyweight-vs-lightweight detection).
2. Add `run_warmup_converged(threshold_c: f64, max_iter: u32) -> Result<u32, HaresError>` to `Dwelling` implementing the EnergyPlus iterative procedure: run 24 h of weather, check `max |ΔT_zone|` across all conditioned zones, stop if below `threshold_c` or after `max_iter` iterations. Return the iteration count.
3. Emit `tracing::warn!` in the config-construction path if `initialization_duration` is `None`.

## Definition of Done

- [ ] `Dwelling::from_hpxml` sets a non-`None` default warm-up duration (minimum 7 days)
- [ ] `run_warmup_converged(0.5, 25)` iterates until max zone temperature change < 0.5 °C, emits `tracing::info!` with iteration count
- [ ] `tracing::warn!` fires when `initialization_duration` is `None` at simulation start
- [ ] BESTEST fixtures pass with the new default
- [ ] Test: warm-up convergence for a heavyweight slab house reaches < 0.5 °C max zone-temp delta within 25 iterations
- [ ] `cargo test -p hares-core` passes

## Verification

```bash
cargo test -p hares-core
```

## References

- EnergyPlus Engineering Reference §"Warmup Convergence" — iterative first-day procedure, 0.5 °C threshold, max 25 iterations
- ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.4 "Transient Conduction" — initial conditions must represent multi-day thermal history for heavy construction
- OCHRE `vendors/OCHRE/ochre/cli.py:39` — `initialization_time = timedelta(days=1)` default
- Project policy `feedback_no_silent_defaults.md` — never silently substitute fallback values

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match
  - `mod.rs:107` — `pub initialization_duration: Option<StdDuration>` ✓ exact match
  - `mod.rs:762–767` — `from_hpxml` DwellingConfig literal with `initialization_duration: None` at line 765 ✓ exact match
  - `mod.rs:1180–1181` — `if let Some(init_dur) = config.initialization_duration { dwelling.run_warmup(init_dur)?;` ✓ exact match
  - `mod.rs:1920–1931` — `run_warmup` implementation (runs to 1932 including closing brace) ✓ substantive match
- [x] Described logic matches current implementation
  - `run_warmup` is a fixed-duration forward simulation, NOT an iterative convergence loop. Confirmed: no convergence check exists anywhere in the codebase.
  - All other `DwellingConfig` construction sites in test code also set `initialization_duration: None`.
  - `initialize_steady_state` is a real function in `crates/hares-envelope/src/thermal_solver/initialization.rs` — it solves `x = A_d x + B_d u` for the first weather snapshot, not a multi-day history.
- [x] OCHRE cross-check: **matches** the ticket's claim, with one nuance
  - `vendors/OCHRE/ochre/cli.py:39` — `initialization_time=1` (integer days) ✓ confirmed
  - Line 68: converted to `timedelta(days=initialization_time)` before passing to `Dwelling`.
  - OCHRE's warm-up is **forward-time only**, not iterative convergence (`Simulator.py:133–153`). OCHRE does not implement EnergyPlus's iterative first-day procedure.
  - HARES's `run_warmup` (when called) matches OCHRE's approach: forward simulation then `reset_time()` / `clear()`. The divergence is only that `from_hpxml` sets `None` while OCHRE defaults to 1 day.
- [x] Bug confirmed as **not already fixed**: all HPXML-path callers produce `initialization_duration: None`.
- [x] EnergyPlus cross-check: **partially matches** — see below.

### Web-Verified Citations

**Citation 1**: EnergyPlus Engineering Reference §"Warmup Convergence" — iterative first-day procedure, 0.5 °C threshold, max 25 iterations

- **Source found**: [EnergyPlus 9.4 Engineering Reference — Warmup Convergence](https://bigladdersoftware.com/epx/docs/9-4/engineering-reference/warmup-convergence.html); [EnergyPlus 9.4 Input/Output Reference — Group Simulation Parameters](https://bigladdersoftware.com/epx/docs/9-4/input-output-reference/group-simulation-parameters.html)
- **Quoted passage** (from I/O Reference, `Building` object):
  - `Temperature Convergence Tolerance Value`: *"Default: 0.50 [delta C]. This value represents the number at which the zone temperatures must agree (from previous iteration) before 'convergence' is reached."*
  - `Maximum Number of Warmup Days`: *"Default: 25 days. This field specifies the number of 'warmup' days that might be used in the simulation before 'convergence' is achieved."*
  - `Minimum Number of Warmup Days`: *"Default: 1 day."*
  - From Engineering Reference: *"The first day of the environment is repeated until the loads/temperature convergence tolerance values specified in the Building object are satisfied or until it reaches 'maximum number of warmup days'."*
  - Initial conditions: *"temperatures are initialized to 23°C and zone humidity ratios are initialized to the outdoor humidity ratio."*
- **Verdict**: **Confirmed** for the 0.5 °C threshold and 25-iteration max. **Partially incorrect** on procedure: EnergyPlus repeats the **first day** (not first hour) until convergence, checking four criteria (max/min zone temperature, max heating/cooling load). The ticket says "iterative first-day procedure" which is correct, but also says "repeat the first simulation day until zone temperatures change by less than 0.5 °C between consecutive iterations" — the actual criterion is day-to-day change in *max/min zone temperature per day*, not per-step change.

**Citation 2**: ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.4 "Transient Conduction"

- **Source found**: [2021 ASHRAE Handbook—Fundamentals Table of Contents](https://www.ashrae.org/technical-resources/ashrae-handbook/table-of-contents-2021-ashrae-handbook-fundamentals); [ASHRAE Chapter 4 Heat Transfer (F21)](https://handbook.ashrae.org/Handbooks/F21/IP/F21_Ch04/F21_Ch04_ip.aspx)
- **Quoted passage**: From the official ASHRAE ToC: *"Chapter 18: Nonresidential Cooling and Heating Load Calculations"*. From Chapter 4 inspection: transient conduction is covered under Section 2 ("Thermal Conduction"), not in Chapter 18.
- **Verdict**: **Incorrect**. The ticket's citation is wrong in two ways:
  1. Chapter 18 of the 2021 ASHRAE Handbook of Fundamentals is "Nonresidential Cooling and Heating Load Calculations", not "Transient Conduction". Transient conduction is covered in **Chapter 4** (Heat Transfer).
  2. There is no section numbered "§18.4" in this context. Chapter 4 does not carry section numbers of the form "4.4".
  - The substance of the claim — that initial conditions matter for heavyweight construction — is physically correct and well-established, but the citation is fabricated or confused. The correct authoritative source for EnergyPlus warm-up convergence criteria is the EnergyPlus Engineering Reference itself (cited above), not ASHRAE HoF Ch. 18.

**Citation 3**: OCHRE `vendors/OCHRE/ochre/cli.py:39` — `initialization_time = timedelta(days=1)` default

- **Source found**: Read directly from `vendors/OCHRE/ochre/cli.py` (local submodule)
- **Quoted passage**: Line 38–39 of the `create_dwelling` function:
  ```python
  def create_dwelling(
      ...
      initialization_time=1,  # integer days
  ```
  Line 68: `initialization_time=dt.timedelta(days=initialization_time)`
- **Verdict**: **Confirmed** (value is integer `1`, converted to `timedelta(days=1)` before use). The line number is off by one (it is line 38–39 in context, not precisely "line 39"), but the substance is correct.

**Citation 4**: Project policy `feedback_no_silent_defaults.md` — never silently substitute fallback values

- **Source found**: Referenced as local policy file; not web-verifiable. Exists as project-internal policy referenced in the ticket for completeness.
- **Verdict**: Not independently verifiable via web search; accepted as project-internal reference.

### Regression Test Results

The test at `tests/warmup_regression.rs` was executed:

- `heavyweight_freefloat_warmup_changes_initial_zone_temperature` — **PASSES**: confirms the precondition that warmup shifts zone temperature by **42.957 °C** at step 1 for BESTEST 900FF heavyweight concrete free-float building. This is dramatically above the 0.5 °C EnergyPlus threshold, showing that `initialize_steady_state` (single weather-snapshot solve) is severely inadequate for heavy concrete construction.
- `hpxml_path_applies_default_warmup` — **FAILS** (marked `#[ignore]` pending fix): asserts the post-fix correct behaviour. Currently fails because `from_hpxml` sets `initialization_duration: None`.

### Legitimacy

- **Verdict**: **Partially Legitimate**

- **Rationale**: The core bug is confirmed and severe. `Dwelling::from_hpxml` hard-codes `initialization_duration: None`; `run_warmup` is never called; `initialize_steady_state` leaves RC nodes at a weather-snapshot steady-state that is 42.957 °C wrong for a heavyweight free-float concrete building. The EnergyPlus citation for the 0.5 °C threshold and 25-day maximum is verified correct. The OCHRE default of 1 day is confirmed. However, two technical claims in the ticket need correction: (a) the ASHRAE Handbook citation (Ch. 18 §18.4 "Transient Conduction") is wrong — the correct chapter is Chapter 4, not 18, and no §18.4 exists; (b) the description of OCHRE's warm-up as forward-time is accurate but the ticket's framing of the fix as "iterative convergence" goes beyond OCHRE's approach — OCHRE uses a fixed-duration forward warm-up, not an EnergyPlus-style iterative check. The "preferred" fix (iterative convergence) is more sophisticated than OCHRE's own implementation.

### Proposed Fix Summary

**Minimum fix** (line 765 of `crates/hares-core/src/dwelling/mod.rs`):
```rust
// Change:
initialization_duration: None,
// To:
initialization_duration: Some(StdDuration::from_secs(7 * 24 * 3600)),
```
This sets a 7-day default for all HPXML-path simulations (7 days covers 2.3× the concrete slab's τ ≈ 3 days). No other production code changes are required for the minimum fix.

**Preferred fix**: Implement `run_warmup_converged(threshold_c: f64, max_iter: u32)` in `Dwelling` following the EnergyPlus first-day repetition algorithm: run 24 h of weather, check `max |ΔT_zone|` across all conditioned zones, repeat up to `max_iter` times or until delta < `threshold_c`. Add `tracing::warn!` in `from_config` when `config.initialization_duration.is_none()`.

### Test Written

- **File**: `tests/warmup_regression.rs` (entry point: `crates/hares-core/tests/warmup_regression.rs`)
- **What it tests**:
  1. `heavyweight_freefloat_warmup_changes_initial_zone_temperature` — Documents the precondition: BESTEST 900FF heavyweight concrete building has a 42.957 °C zone-temperature bias at step 1 when warm-up is skipped vs. applied (21-day warmup). Passes now, should continue to pass after the fix.
  2. `hpxml_path_applies_default_warmup` — The failing regression test (marked `#[ignore]`). Asserts that without explicit warmup config the zone temperature at step 1 agrees with the warmup run within 0.5 °C. Fails until the fix (7-day default or iterative convergence) is applied to `from_hpxml`.
