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
