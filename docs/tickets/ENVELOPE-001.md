---
id: ENVELOPE-001
title: Store and expose EnvelopeDiagnostics from Dwelling
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-core/src/dwelling/mod.rs
  - crates/hares-core/src/dwelling/solver_builder.rs
  - crates/hares-envelope/src/boundary_rc.rs
references:
  - tests/fixtures/parity/ochre_rc_reference.json
  - vendors/OCHRE/ochre/Models/Envelope.py
verification:
  - cargo build --all-targets
  - cargo test -p hares-core
  - cargo clippy --all-targets -- -D warnings
---

## Background/Context

`EnvelopeDiagnostics` (per-boundary UA, R_total, film R, capacitance, node count,
construction path) is already computed inside `build_default_solvers()` but discarded
after `assemble_building_rc()` returns. The binding exists at `solver_builder.rs:395`
but is never included in the `SolverBundle` return tuple. There is no way to inspect
the RC network parameters from outside `solver_builder.rs`.

We need this data exposed on `Dwelling` so that:
- Parity tests can compare per-boundary values against OCHRE reference JSON
- The conditioned oracle can log detailed RC diagnostics
- Future tools can dump the full RC network for debugging

## Work to Do

- [ ] Extend the `SolverBundle` type alias (currently a 5-tuple in `solver_builder.rs`) to include `EnvelopeDiagnostics` — either as a 6-tuple or refactor to a named struct
- [ ] In `build_default_solvers()`, return `EnvelopeDiagnostics` as part of the `SolverBundle`
- [ ] Add `envelope_diagnostics: EnvelopeDiagnostics` field to `Dwelling` struct (private, with accessor)
- [ ] Wire it through `Dwelling::from_config()` / `Dwelling::new()` to store on the struct
- [ ] Add `pub fn envelope_diagnostics(&self) -> &EnvelopeDiagnostics` accessor method
- [ ] Add `Serialize` derive to `EnvelopeDiagnostics`, `BoundaryDiagnostic`, `RCPath`, `ExteriorTarget` in `boundary_rc.rs` for JSON export
- [ ] Add a `pub fn envelope_diagnostics_json(&self) -> String` convenience method that serializes to JSON

## Files to Touch

- `crates/hares-core/src/dwelling/mod.rs`: Add field + accessor to `Dwelling`
- `crates/hares-core/src/dwelling/solver_builder.rs`: Extend `SolverBundle`, return diagnostics from `build_default_solvers`
- `crates/hares-envelope/src/boundary_rc.rs`: Add `Serialize` derives to diagnostic types (`EnvelopeDiagnostics`, `BoundaryDiagnostic`, `RCPath`, `ExteriorTarget`)

## Measures of Success

- [ ] `dwelling.envelope_diagnostics()` returns the full set of per-boundary diagnostics
- [ ] JSON serialization works (field names match `ochre_rc_reference.json` schema)
- [ ] No performance regression — diagnostics are computed at init, not in hot loop
- [ ] All existing tests pass unchanged

## Verification

- [ ] `cargo build --all-targets` passes
- [ ] `cargo test -p hares-core` passes
- [ ] `cargo test -p hares-envelope` passes
- [ ] `cargo clippy --all-targets -- -D warnings` passes
