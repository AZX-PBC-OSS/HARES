# Use `InteriorLwrMethod::StarMesh` Explicitly at Callsites

**Severity**: Nit
**Priority**: P4
**Status**: Open
**Areas**: hares-envelope, hares-core

## Problem

Several callsites construct or configure the interior longwave radiation network using `InteriorLwrMethod::default()` rather than naming the variant explicitly. Per the project clarity rule (no implicit defaults where the choice has physics-significant consequences), the variant should be named explicitly at every callsite so that a future reader can see at a glance which network topology is in use.

The `default()` value is `InteriorLwrMethod::StarMesh`, but a reader of the callsite cannot tell this without jumping to the type definition. Worse, if the default is ever changed (e.g. to a future "ScriptF" exact method), every callsite that relied on the implicit default will silently switch behaviour without any callsite change.

## Current Behavior

Callsites use `InteriorLwrMethod::default()` rather than `InteriorLwrMethod::StarMesh`. Locations:
- Anywhere `InteriorLwrMethod::default()` appears in `crates/hares-envelope/src/` and `crates/hares-core/src/dwelling/`.

## Required Behavior

Every callsite that constructs an `InteriorLwrMethod` value must name the variant explicitly:

```rust
// Before:
let lwr = InteriorLwrMethod::default();

// After:
let lwr = InteriorLwrMethod::StarMesh;
```

This applies to struct-literal initialisations, `..Default::default()` spread expressions where `InteriorLwrMethod` is one of the fields, and any other implicit-default usage.

## Approach

1. `grep` the workspace for `InteriorLwrMethod::default()` and `InteriorLwrMethod` in spread expressions.
2. For each callsite, replace the implicit default with the explicit `StarMesh` variant.
3. Where a struct uses `..Default::default()` and `InteriorLwrMethod` is one of the fields, replace the spread with explicit field initialisation OR set the field explicitly before the spread.
4. Optionally remove the `Default` impl on `InteriorLwrMethod` entirely to make the explicit-variant requirement enforced by the compiler.

## Definition of Done

- [ ] No callsite uses `InteriorLwrMethod::default()`
- [ ] Every construction site names the variant explicitly
- [ ] If the `Default` impl is removed, the workspace still builds and all tests pass
- [ ] Comment at the type definition explains why an explicit variant is required

## Verification

```bash
cargo build --workspace
cargo test --workspace
rg 'InteriorLwrMethod::default' crates/
rg 'InteriorLwrMethod' crates/ | rg 'default'
```

## References

- HARES project clarity convention — physics-significant choices must be explicit at the callsite.
- EnergyPlus Engineering Reference §3.5.10 "Network Solution" — distinguishes ScriptF, MRT, and Star/Mesh methods; the choice of method changes results materially.

## Related Tickets

- 044-lwr-fallback-linearised-not-scriptf
- 047-interior-lwr-uses-last-step-zone-temp
- 089-radiation-frac-starmesh-rederivation
