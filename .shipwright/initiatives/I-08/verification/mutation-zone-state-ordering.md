# Mutation: zone-state ordering test (`fix-headaudit`)

Test: `crates/hares-core/tests/zone_state_ordering.rs`,
`nonthermal_equipment_reads_the_previous_steps_zone_temperatures`.
Worktree `.workstrees/fix-headaudit` at `5f31d549`, its own
`CARGO_TARGET_DIR` (`.workstrees/target-mut-zso`, deleted afterwards).
Operator-approved temporary mutation; nothing committed.

## Mutation

`crates/hares-core/src/dwelling/mod.rs`, immediately before the
"Phase 1: Independent-stage equipment" loop of `Dwelling::step`: run the
step's envelope integration and publish its zone temperatures, so the
Independent-stage equipment sees the zone state of the current solve
instead of the one committed before the step.

```rust
self.thermal_solver.integrate(
    &self.ports,
    &self.latest_env,
    &mut self.thermal_update_buf,
)?;
apply_thermal_update_to_zones(&mut self.latest_env, &self.thermal_update_buf);
```

## Result with the mutation

```
test humidity_solver_produces_valid_output_across_multiple_steps ... ok
test nonthermal_equipment_reads_the_previous_steps_zone_temperatures ... FAILED
thread 'nonthermal_equipment_reads_the_previous_steps_zone_temperatures' panicked at crates/hares-core/tests/zone_state_ordering.rs:218:9:
assertion `left == right` failed: step 0: equipment must see the zone temperatures committed before the step
  left: [20.000250089804304]
 right: [20.0]
test result: FAILED. 1 passed; 1 failed
```

The test catches the broken ordering on the first step: the stub saw the
integrated 20.00025 °C instead of the committed 20.0 °C.

## Result after the revert

```
test nonthermal_equipment_reads_the_previous_steps_zone_temperatures ... ok
test humidity_solver_produces_valid_output_across_multiple_steps ... ok
test result: ok. 2 passed; 0 failed
```

## Restore

The six inserted lines were removed with the same edit, and the inserted
text no longer appears in the file. The file is not byte-identical to the
pre-mutation copy (sha256 `5c909cae...` before), because the agent working in
the worktree edited the same file while the mutation was in place: its only
difference from the pre-mutation copy is that agent's comment change at line
4495 ("Steps 1e–1f" to "Steps 1e to 1f"). That change was left in place.
`git status` lists `crates/hares-core/src/dwelling/mod.rs` as modified for
that edit alone, alongside the other agent's own uncommitted files; nothing
of the mutation remains.
