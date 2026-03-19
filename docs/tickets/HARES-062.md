---
id: HARES-062
title: "Repository Setup and CI Foundation"
kind: implement
depends_on: []
files_to_touch:
  - Cargo.toml
  - rustfmt.toml
  - docs/PHYSICS_DECISIONS.md
references:
  - docs/architecture/08-operations.md
verification:
  - cargo fmt --check
  - cargo clippy --workspace -- -D warnings
---

## Background/Context
Multiple tickets reference `docs/PHYSICS_DECISIONS.md`, workspace clippy lints, rustfmt enforcement, and tracing subscriber setup — but no ticket establishes the repository-level infrastructure. This ticket creates the shared foundation that all other tickets depend on.

## Work to Do
- [ ] Create `rustfmt.toml` with project formatting rules
- [ ] Add workspace-level Clippy lints to `Cargo.toml` `[workspace.lints.rust]`
- [ ] Document MSRV (minimum supported Rust version) in `Cargo.toml`
- [ ] Create `docs/PHYSICS_DECISIONS.md` with template header — this is the canonical location (not `docs/architecture/` or repo root)
- [ ] Add `postcard` to workspace `[workspace.dependencies]` for checkpoint serialization (used by HARES-018+)
- [ ] Add `tracing-subscriber` to workspace `[workspace.dependencies]` for test/binary logging

## Measures of Success
- [ ] `cargo fmt --check` passes on all workspace crates
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `docs/PHYSICS_DECISIONS.md` exists with a template header
- [ ] `postcard` and `tracing-subscriber` are in workspace dependencies
