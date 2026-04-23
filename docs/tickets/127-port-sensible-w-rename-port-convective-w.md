# Rename `port_sensible_w` to `port_convective_w` for Naming Consistency

**Severity**: Nit
**Priority**: P4
**Status**: Open
**Areas**: hares-envelope, hares-core, hares-python

## Problem

The port-input bookkeeping uses `port_sensible_w` for the convective-only portion of equipment heat injection and `internal_gain_w` for the full sensible total (convective + radiant). The names are asymmetric and misleading: a reader reasonably expects `port_sensible_w` to be the *sensible* total, not the *convective* portion of it. This creates a footgun when comparing port telemetry against the dwelling-level sensible balance — any user adding `port_sensible_w` and `port_radiant_w` together expecting "sensible total" will double-count, because `port_sensible_w` is already the convective part only.

## Current Behavior

Across the workspace:
- `port_sensible_w` is the convective-only port input — see `crates/hares-envelope/src/thermal_solver/ports.rs` and `crates/hares-python/src/py_dwelling.rs`.
- `port_radiant_w` is the radiant port input.
- `internal_gain_w` is the full sensible total (convective + radiant) used in zone-level energy bookkeeping.

The `_sensible_` token in `port_sensible_w` overlaps semantically with the `_sensible_` interpretation used in `internal_gain_w` and elsewhere in the codebase.

## Required Behavior

Rename `port_sensible_w` to `port_convective_w` everywhere:

1. Rust struct fields, function names, and local bindings.
2. Python-binding dict keys (`post_solvers["port_sensible_w"]` → `post_solvers["port_convective_w"]`).
3. Diagnostic CSV column names.
4. Test assertions that reference the old name.
5. Documentation comments that reference the old name.

This is a hard rename — no alias, no backward-compat shim (per project memory `feedback_no_backward_compat` — greenfield codebase).

## Approach

1. `grep` workspace for `port_sensible_w` and enumerate every callsite.
2. Run an automated rename using the Edit tool's `replace_all` parameter against each file.
3. Update Python binding tests and CSV-output snapshot tests.
4. Update any markdown documentation that references the old name.
5. Verify the workspace builds and all tests pass after rename.
6. Coordinate with ticket 108 (port_radiant_w Python and Diag exposure) to ensure the new symmetric naming (`port_convective_w` + `port_radiant_w`) appears together in the Python dict and the diagnostic CSV.

## Definition of Done

- [ ] `port_sensible_w` renamed to `port_convective_w` in all Rust source and tests
- [ ] Python binding dict keys updated
- [ ] Diagnostic CSV column header updated
- [ ] No remaining references to `port_sensible_w` in the workspace (except possibly migration notes in CHANGELOG / release notes)
- [ ] All tests pass after rename
- [ ] Doc comments distinguish convective port (`port_convective_w`), radiant port (`port_radiant_w`), and the full sensible total (`internal_gain_w`)

## Verification

```bash
cargo build --workspace
cargo test --workspace
rg port_sensible_w crates/
```

## References

- ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.1 "Internal Heat Gain Components" — defines sensible heat as the sum of convective and radiant components; "convective" and "radiant" are the canonical sub-components, not "sensible" and "radiant".
- EnergyPlus Engineering Reference §3.5 "Inside Surface Heat Balance" — uses `q_conv` and `q_rad` (convection / radiation) as the explicit sub-components, never `q_sensible` for the convective part alone.

## Related Tickets

- 108-port-radiant-w-python-and-diag-exposure (paired exposure work; coordinate naming)
