# `cargo fmt` Required for `boundary_diagnostics` Test Struct Literals

**Severity**: Nit
**Priority**: P4
**Status**: Open
**Areas**: hares-envelope

## Problem

`crates/hares-envelope/src/thermal_solver/mod.rs` contains 15+ test struct literals that initialise `boundary_diagnostics: Vec::new()` with incorrect indentation. The misindentation is consistent (suggesting a manual edit that did not run `cargo fmt` afterwards) and harmless to compilation, but it fails `cargo fmt --check` and degrades readability.

This is a pure formatting issue — fixed by running `cargo fmt`.

## Current Behavior

15+ struct literals in `crates/hares-envelope/src/thermal_solver/mod.rs` have `boundary_diagnostics: Vec::new()` lines at the wrong indent level relative to the surrounding fields.

## Required Behavior

All struct literals in the file must satisfy `cargo fmt --check`.

## Approach

1. Run `cargo fmt -p hares-envelope` from the workspace root.
2. Inspect the diff to confirm only formatting changes (no semantic changes).
3. Commit only the formatting changes.
4. As a guard, ensure CI runs `cargo fmt --check` so the issue does not recur.

## Definition of Done

- [ ] `cargo fmt --check -p hares-envelope` passes
- [ ] Workspace-level `cargo fmt --check` passes
- [ ] CI enforces `cargo fmt --check` (verify or add)

## Verification

```bash
cargo fmt --check --workspace
cargo build -p hares-envelope
cargo test -p hares-envelope thermal_solver
```

## References

- `rustfmt` style guide — canonical Rust formatting rules.

## Related Tickets

(none — pure formatting)
