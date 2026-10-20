# Review of ffa9051f (OR-19 follow-up), range 7b7a6a7d..ffa9051f

Reviewer: independent, worked in .workstrees/audit-ffa9051f at ffa9051f.

## Verdict: CHANGES-REQUIRED

Counts: blocker 0, major 1, minor 4, nit 2. One further pre-existing defect is reported separately.

The three claimed fixes are real and verified (see "Verified" below). The one major finding is a behavior regression introduced by routing assembly through the shared install step.

## Findings

### M1 (major): construction no longer fails when the output recorder cannot be created

Evidence: the removed assembly code returned
`StreamingRecorder::new(...).map_err(|err| HaresError::Io("output recorder init failed: ..."))?`
(old crates/hares-core/src/dwelling/mod.rs, around line 2783 at 9ef347e0). Assembly now calls
`install_roster_caches`, whose FreshSchema arm (crates/hares-core/src/dwelling/mod.rs:4651-4676) turns the
same failure into `push_warning("output recorder re-init failed: ... subsequent output is dropped")` and
leaves `self.recorder` as `None`. `StreamingRecorder::new` does real I/O (`File::create`,
crates/hares-io/src/output/writer.rs around line 125), so an unwritable or missing output directory is a
reachable input. Before this commit `Dwelling::from_config` returned `Err`; now it returns `Ok` with a
dwelling that silently records nothing. This violates the fail-loudly rule for a configuration error at the
one point where failing is cheap and correct. There is no test covering either behavior (grep for
"recorder init" finds only the new warning string).

The same arm has a second, older defect it now also gives assembly: on `Err` the previous recorder (if any)
stays in `self.recorder` while `output_column_index`, `output_value_count` and `record_scratch` have already
been replaced with the new schema's, so a later runtime re-init failure leaves a recorder whose schema does
not match the row width being written. Present since 9ef347e0, not introduced here.

Required change: make recorder creation part of the plan, not the install. Build the `StreamingRecorder`
(and call `enable_metrics`) inside `plan_roster_caches`'s FreshSchema branch and carry it in
`OutputCachesPlan::FreshSchema`, so a creation failure returns `Err` from the plan and every entrance
(including assembly, which then keeps its hard error) leaves state unchanged. That also restores the
"install is infallible" claim in the doc comment at 4468. Note it creates the output file during planning;
the chunked/rotating writers should be checked for whether a rejected later plan must delete or reuse that
file. Add a test: `Dwelling::from_config` with an output path in a nonexistent directory returns `Err`.

### m1 (minor): assembly still computes what the plan derives, twice

Evidence: crates/hares-core/src/dwelling/mod.rs:2707-2708 build `ports` and `rollback_ports` from the
declarations, 2713-2732 run `validate_fluid_type_consistency`, 2746 builds `FluidSolver::new(...)` from
`fluid_loop_declarations`; then 4311-4337 (`plan_roster_caches`, step 0) rebuild ports, rollback_ports and
the loop-type map from the same declarations via `rebuild_port_slots` and `plan_loop_types`, and install
overwrites all three (4514-4518). The assembly copies exist because the struct needs non-optional fields and
because the early `?` gives a construction-specific error message and the debug-only thermal-accumulator
check (2710-2731 region) reads `ports`. That is a legitimate reason for the early validation, but the
resulting second derivation is unexplained and the `FluidSolver::new` result is discarded work.
Required change: either construct `ports`/`rollback_ports`/`fluid_solver` as empty defaults and let the plan
be the only derivation (moving the debug invariant and the consistency validation to run on the plan's
output), or add a comment saying why the early copies must stay. Cold path, so no performance consequence.

### m2 (minor): placeholder construction

Evidence: crates/hares-core/src/dwelling/mod.rs:2781-2840 build `Dwelling` with about fifteen empty
placeholder fields (`equipment: Vec::new()`, `recorder: None`, empty caches), then 2900-2908 assign
`dwelling.equipment = equipment` and install. For the span between those statements the struct violates its
own invariants (equipment vector disagrees with every cache). Nothing between them uses it, but a future
edit that does would read empty caches silently.
Required change: keep the window to the two statements and add no code between them (it is currently so);
preferably have a `RosterCaches::empty()` or `Default` so the placeholder block is one expression instead of
fifteen fields, which also removes the scattered duplicate field list from the struct literal.

### m3 (minor): generics add four copies of the large planner for no caller need

Evidence: `plan_roster_caches<E, A>` (crates/hares-core/src/dwelling/mod.rs:4290) is instantiated for
(Box, Box) at assembly, (&dyn, Box) at add/replace, (Box, &dyn) at add_actor/auto_register, and (&dyn, &dyn)
at clear/remove. Every entrance except assembly already builds a `Vec<&dyn _>`; assembly could do the same
in two lines. The planner builds schemas and column maps, so it is the largest function in the set. Runtime
effect is nil (cold path), compile time and binary size grow modestly.
Required change: make the planner and the roughly ten helpers take `&[&dyn Equipment]` / `&[&dyn Actor]`
only, and let the two or three entrances that hold boxes collect a ref vector. That deletes both `AsRef`
impls (m4) and every `.as_ref()` hop in the helpers.

### m4 (minor): new public `AsRef` impls on trait-object references

Evidence: crates/hares-core/src/actor.rs:180-189 and crates/hares-equipment/src/lib.rs:541-550 add
`impl AsRef<dyn Actor> for &dyn Actor` and `impl<'a> AsRef<dyn Equipment + 'a> for &'a dyn Equipment`.
They compile and are lifetime-correct (clippy clean, all tests pass). But they are public, permanent trait
impls on the crates' extension traits added only to serve a private helper bound, and the two are written
differently (explicit `'a` for Equipment, none for the `'static` Actor). If the generics stay, a private
bound avoids the public surface: `E: Deref<Target = dyn Equipment + 'e>` is satisfied by both `Box<dyn
Equipment>` and `&dyn Equipment` without any new impl (not compiled by me; verify before adopting).
Required change: resolve through m3 (preferred), or switch to a `Deref` bound and delete the impls.

### n1 (nit): test comments narrate history

Evidence: crates/hares-core/tests/roster_plan_schema_parity.rs:116-123 ("the deeper break this fix closes",
"before this fix, rebuilt the schema through a path that never enriched it"). The comment describes the
defect that was fixed rather than what the assertion guarantees.
Required change: reword to state the invariant (the assembled column keeps its unit provenance through the
post-construction schema rebuild).

### n2 (nit): redundant clone and scan in `auto_register_actors`

Evidence: crates/hares-core/src/dwelling/mod.rs:3462-3482 clones `auto_registered_actor_names`, filters the
roster once to borrow user actors, then `mem::take` plus `retain` to commit. Correct and panic-safe, but
clones a `HashSet<String>` on each call and filters twice.
Required change: optional; commit with `self.actors.retain` after `new_actors` is built from the borrowed
names without cloning, or partition by index.

## Testing gaps

- No test exercises `write_output = false` together with zone temperatures in `StepResult` after the zone
  caches moved to unconditional install. Code reading shows per-step use goes through `zone_env_indices`
  (dwelling/mod.rs around line 7196), not the output column index, and frame_golden is identical, so I
  believe it is correct, but nothing pins it.
- No test for recorder creation failure at construction (M1).
- The parity test covers only the battery on `base-battery.xml`; no case with duplicate-named equipment
  (instance-qualified names in `enrich_schema_with_telemetry_units`) or a multi-actor roster.
- The id tests cover add and replace rejection through a fluid-type conflict only; a rejection from zone
  validation or `ensure_ports_satisfied` happens before the counter write by construction, so it is not at
  risk, but is unasserted.

## Pre-existing, not introduced by this range

- `cargo clippy --workspace --all-targets --release -- -D warnings` fails:
  `unused imports: BoundaryCategory and BoundaryDiagnosticInfo` at
  crates/hares-envelope/src/thermal_solver/mod.rs:1678 (test module; the two names are only used under
  `debug_assertions`-gated tests at 6492 and 6497, so release test builds see them unused). The file was last
  touched by 7b7a6a7d, outside this range. The dev-profile clippy passes. This contradicts the stated goal
  of b8195c83 ("every build profile") and should be fixed under the no-broken-windows rule.

## Verified

- Entrances (add_equipment, replace_equipment, add_actor, auto_register_actors, clear_equipment,
  remove_equipment, remove_equipment_by_end_use): each computes against a borrowed `Vec<&dyn _>`, calls the
  plan with `?` or matches on it, and only then mutates and installs. No `mem::take` of the live equipment
  or actor vector remains before planning; the one `mem::take(&mut self.actors)` (auto_register, line 3474)
  is after a successful plan and followed only by an infallible `retain`. Restore helpers and
  `partition_*` are gone with no dangling references.
- `next_equipment_id`: `assign_equipment_identity` and both validators now take `&self` and return the
  planned counter; add and replace write it only in the `Ok` arm, so a plan rejection leaves it unchanged.
  I added the two new id tests to a disposable worktree at 9ef347e0: both fail there
  (`rejected_add_...` and `rejected_replace_...`), and pass at ffa9051f.
- Parity test: at ffa9051f it passes. At 9ef347e0 (with `StreamingRecorder::schema()` copied in so it
  compiles) it fails at the `unit_source == telemetry_field` assertion on "Battery Electric Power (kW)", so
  it asserts the assembled column itself and not only equality with the runtime-added one. It reads fixed
  fixtures, no RNG or clock; deterministic. The disposable worktree was removed.
- Zone caches moved into `RosterCaches` unconditionally: correct for `write_output = false` (they use the
  empty or frozen column index and the env zone indices, same values assembly produced before).
- `StreamingRecorder::schema()` (hares-io writer.rs): small `#[must_use]` borrowing accessor, `Arc`
  inner kept private; appropriate surface.
- No per-step change: the diff touches no step code; frame_golden compare-all is identical for the default
  build, `-F dst` and `-F profiling` (release). Zero-allocation tests pass:
  synthetic_box_zero_alloc, radiant_gain_weights_zero_alloc, and hares-core `profiling_alloc`
  (with `--features profiling`).
- `cargo clippy --workspace --all-targets -- -D warnings`: clean. `cargo test -p hares-core`: all pass
  (1007 lib tests plus integration tests, 0 failed).
- Writing: no ticket or ledger IDs, em dashes or agent references in added lines or in the two commit
  messages.
