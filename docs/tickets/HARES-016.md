---
id: HARES-016
title: "hares-envelope — Electrical Solver"
kind: implement
depends_on: [HARES-014, HARES-003]
files_to_touch:
  - crates/hares-envelope/src/electrical_solver.rs
  - crates/hares-envelope/src/lib.rs
references:
  - docs/architecture/02-equipment-and-ports.md
verification:
  - cargo check -p hares-envelope
  - cargo test -p hares-envelope
  - cargo clippy -p hares-envelope -- -D warnings
---

## Background/Context

After all equipment has written to `PortSlots`, the electrical domain must sum active and reactive power contributions, apply voltage-dependent load correction via the ZIP model, and compute net grid exchange. OCHRE performs this accumulation in its dwelling step; HARES encapsulates it in `ElectricalSolver` implementing `DomainSolver` so the engine loop has a uniform interface across all physics domains. The ZIP model adjusts consumption based on `GridState::voltage_pu` received from a HELICS co-sim peer (defaults to 1.0 in standalone mode).

## Work to Do

- [ ] Define `ZipCoefficients` struct with fields: `z: f64` (impedance fraction), `i: f64` (current fraction), `p: f64` (constant-power fraction) — validate `z + i + p == 1.0` within 1e-6 at construction
- [ ] Define `ElectricalSolverConfig` struct with fields: `zip: ZipCoefficients`, `nominal_voltage_pu: f64` (default `1.0`)
- [ ] Define `ElectricalSolver` struct implementing `DomainSolver`:
  - Owns `config: ElectricalSolverConfig`
  - `domain_id()` returns `ELECTRICAL` (`DomainId(1)`)
  - `resolve()`:
    1. Read pre-split totals from `PortSlots::electrical`: `P_load = electrical.load_power_kw`, `P_gen = electrical.generation_power_kw`, `Q = electrical.reactive_power_kvar` — the load/generation split is tracked during accumulation by `ElectricalAccumulator` (HARES-003), not recomputed here
    2. Read `v = env.grid.voltage_pu`
    3. Apply ZIP voltage correction to load only: `P_load_adj = P_load * (config.zip.z * v * v + config.zip.i * v + config.zip.p)`
    4. Generation uses identity ZIP (`p=1, z=0, i=0`) — do NOT apply load-oriented ZIP coefficients to generation. `P_gen_adj = P_gen` (unchanged).
    5. Net active power = `P_load_adj + P_gen_adj` (positive = net consume, negative = net export)
    6. Net reactive power = `Q` (no ZIP correction for reactive in v1)
    7. Return `DomainUpdate` with net electrical state (`active_power_kw`, `reactive_power_kvar`)
- [ ] Expose `ElectricalSolver::net_active_kw(&self) -> f64` and `net_reactive_kvar(&self) -> f64` from the most recent resolve for telemetry use. These accessors return stale state after the next `resolve()` call — they are only valid between a `resolve()` call and the next `resolve()` call. `net_active_kw()` and `net_reactive_kvar()` are NOT safe to call concurrently with `resolve()` — document this contract. Callers must only access them after the step completes.
- [ ] Handle the case where `PortSlots::electrical` is empty (zero load building) without panic

## Files to Touch

- `crates/hares-envelope/src/electrical_solver.rs`: new file — `ElectricalSolver`, `ElectricalSolverConfig`, `ZipCoefficients`
- `crates/hares-envelope/src/lib.rs`: add `pub mod electrical_solver` and re-export public types

## Measures of Success

- [ ] Three equipment contributions with `active_power_kw` of `1.0`, `2.0`, `3.0` and no ZIP correction (`z=0, i=0, p=1`) produce a net of `6.0 kW` exactly
- [ ] ZIP model at `V = 0.95`, `z=0.5, i=0.3, p=0.2`: `P_load_adj = P_load * (0.5 * 0.9025 + 0.3 * 0.95 + 0.2)` — verified to within 1e-10
- [ ] PV generation: one contribution with `active_power_kw = -5.0` (negative = export), one load with `active_power_kw = 3.0` — `P_load = 3.0`, `P_gen = -5.0`, ZIP applied only to `P_load`, net is `P_load_adj + (-5.0)`. At `v=1.0` with `p=1` this yields `-2.0 kW` (net export)
- [ ] At `V = 0.95` with `z=0.5, i=0.3, p=0.2`, a `-5.0 kW` PV contribution is unchanged (`-5.0 kW`) while a `3.0 kW` load is adjusted — confirms ZIP is not applied to generation
- [ ] Per-timestep electrical balance assertion passes: `|P_grid + ΣP_equipment| < 0.001 kW` (per arch doc 07-testing invariant; all ports use `+consume / -generate` sign convention)
- [ ] Empty `PortSlots::electrical` returns `DomainUpdate` with zero net power
- [ ] `ZipCoefficients` construction fails with `Err` when `z + i + p` deviates from 1.0 by more than 1e-6
- [ ] `ZipCoefficients` construction fails when any individual component (`z`, `i`, or `p`) is negative

## Verification

- [ ] `cargo check -p hares-envelope` passes
- [ ] `cargo test -p hares-envelope` passes
- [ ] `cargo clippy -p hares-envelope -- -D warnings` passes
