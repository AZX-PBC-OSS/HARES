---
id: PARITY-003
title: "Quality: Audit unwrap() calls and add proper error handling"
kind: fix
depends_on: []
files_to_touch:
  - (multiple files across all crates)
references:
  - feedback_code_quality.md
verification:
  - cargo build --workspace
  - cargo test --workspace
  - cargo clippy --workspace -- -D warnings
---

## Background/Context

Audit found 1,697 `unwrap()` calls across the codebase. While many are in test code or provably safe contexts (e.g., `HashMap::get` after `contains_key`), production hot-path code should use `?` propagation or explicit error handling. Panics in simulation code corrupt fleet results silently.

## Work to Do

### Phase 1: Triage (no code changes)
- [ ] Categorize all unwrap() calls into:
  - **Test code** (acceptable — skip)
  - **Init/config** (acceptable if input is validated — add comment)
  - **Hot path** (must fix — convert to `?` or `.unwrap_or()`)
  - **Provably safe** (add `// SAFETY: ...` comment explaining why)

### Phase 2: Fix hot-path unwraps
- [ ] In `thermal_solver/mod.rs`: replace unwraps in `resolve_internal()` with `?` propagation
- [ ] In `dwelling/mod.rs`: replace unwraps in `run_timestep()` with `?` propagation
- [ ] In equipment `step()` methods: replace unwraps with `?` or safe defaults
- [ ] In port accumulation: replace unwraps with `?`

### Phase 3: Add clippy lint
- [ ] Add `#![warn(clippy::unwrap_used)]` to each crate's lib.rs
- [ ] Allow unwrap in test modules: `#[cfg_attr(test, allow(clippy::unwrap_used))]`

### Quality Requirements

- [ ] Zero unwrap() calls in any `step()`, `resolve()`, or `run_timestep()` method
- [ ] All remaining unwrap() calls have a `// SAFETY:` comment or are in test code
- [ ] No behavior changes — only error propagation improvements

## Measures of Success

- [ ] `cargo clippy --workspace -- -W clippy::unwrap_used` produces only test/justified warnings
- [ ] Hot-path functions return `Result<>` and propagate errors cleanly
- [ ] All tests pass

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
