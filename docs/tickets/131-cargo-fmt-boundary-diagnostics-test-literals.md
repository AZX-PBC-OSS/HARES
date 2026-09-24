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

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced file `crates/hares-envelope/src/thermal_solver/mod.rs` exists and has the described issue
- [x] Described logic matches current implementation: 25 of 27 `boundary_diagnostics: Vec::new()` lines use 16-space indentation (one extra 4-space level) instead of the correct 12-space level that matching sibling fields like `interior_solar_zones` use. The two correctly-indented instances are at lines 2076 and 4836.
- [x] Bug is present and not already fixed: `cargo fmt --check -p hares-envelope` and `cargo fmt --all --check` both exit 1.
- [x] **Scope is significantly understated**: the ticket says "15+ struct literals" with `boundary_diagnostics` mis-indented in `thermal_solver/mod.rs`. The actual workspace situation is 31 distinct diff hunks in `thermal_solver/mod.rs` alone (not all `boundary_diagnostics` — many are unrelated fmt issues in the same file), and `cargo fmt --all --check` reports failures across **51 files** total in the workspace. The `boundary_rc.rs` file alone has 74 diff hunks. The fix is still `cargo fmt` but the description of scope is incorrect.
- [x] OCHRE cross-check: N/A — this is a Rust toolchain formatting issue with no OCHRE equivalent. OCHRE is Python and uses Black/PEP 8, not rustfmt.
- [x] EnergyPlus cross-check: N/A — pure formatting issue, no algorithmic content to compare.

**Concrete evidence of the bug** (lines 857–864 of `thermal_solver/mod.rs`):
```
            interior_solar_zones: Vec::new(),   // ← 12 spaces (correct)
                boundary_diagnostics: Vec::new(), // ← 16 spaces (wrong, 4 extra)
        };
```
`cargo fmt` corrects this to 12-space indentation consistently.

### Web-Verified Citations

- **Citation**: "rustfmt style guide — canonical Rust formatting rules."
- **Source found**: https://doc.rust-lang.org/style-guide/ (The Rust Style Guide, official)
- **Quoted passage**: *"Use spaces, not tabs. Each level of indentation must be 4 spaces (that is, all indentation outside of string literals and comments must be a multiple of 4)."* and *"Prefer block indent over visual indent"* — struct literal fields are indented one 4-space level deeper than the struct literal opening, not deeper. For a struct literal at 8 spaces (inside a `fn` body in a `mod test`), fields indent to 12 spaces. The 16-space indentation seen in the code adds an extra erroneous level.
- **Verdict**: Confirmed. The rule is clear and the violation is straightforward.

- **Citation** (implicit): `rustfmt.toml` with `edition = "2024"` and `style_edition = "2024"` governs the formatting.
- **Source found**: https://doc.rust-lang.org/edition-guide/rust-2024/rustfmt-style-edition.html and https://doc.rust-lang.org/edition-guide/rust-2024/rustfmt-formatting-fixes.html
- **Quoted passage**: *"In the 2024 Edition, rustfmt now supports the ability for users to control the Style Edition used for formatting. The 2024 style edition introduces several fixes to various formatting scenarios."* The 15 documented 2024 fixes do not change the fundamental 4-space block indent rule for struct fields.
- **Verdict**: Confirmed. The `rustfmt.toml` in the repo correctly specifies `style_edition = "2024"`, and the 2024 style edition's struct-field indentation rule is the same 4-space block indent.

### Legitimacy

- **Verdict**: Partially Legitimate

- **Rationale**: The core issue is real — `cargo fmt --check` fails, and the specific `boundary_diagnostics: Vec::new()` indentation bug in `thermal_solver/mod.rs` is exactly as described (25 instances at 16-space instead of 12-space indentation). The fix (`cargo fmt`) is correct. However, the ticket's scope claim is materially understated in two ways: (1) `thermal_solver/mod.rs` has 31 fmt diff hunks, not just the `boundary_diagnostics` ones — the file has broader formatting drift; (2) `cargo fmt --all --check` fails across **51 files** workspace-wide with the largest single-file failure in `boundary_rc.rs` (74 hunks). The DoD item "`cargo fmt --check --workspace` passes" is therefore more work than the ticket implies, since running `cargo fmt -p hares-envelope` will only fix one crate. The CI note in the DoD is also worth highlighting: there is no `.github/` directory at all in the HARES repo, so no CI currently enforces anything — adding `cargo fmt --check` is a net-new CI setup, not a verification of existing CI.

### Proposed Fix Summary

Run `cargo fmt --all` (not just `-p hares-envelope`) from the workspace root to fix all 51 affected files in one pass. Inspect the diff with `git diff` to confirm no semantic changes. Commit the formatting-only change. Create a `.github/workflows/ci.yml` (or equivalent) that runs `cargo fmt --all --check` and `cargo clippy` on every push/PR. The ticket's DoD checklist is correct but should reference `cargo fmt --all` rather than `-p hares-envelope` to capture the full scope.

### Test Written

- **File**: none needed
- **What it tests**: This is a pure formatting issue verifiable only by the `cargo fmt --check` toolchain. No Rust test (`#[test]`) can assert source-code whitespace; the appropriate guard is CI enforcement. No regression test was written.
