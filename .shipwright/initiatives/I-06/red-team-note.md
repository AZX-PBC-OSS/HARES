---
red_team_outcome: issues_found
failing_tests:
  - command: cargo test -p hares-core --test add_equipment_empty_name_exemption add_equipment_rejects_an_explicit_id_held_by_an_empty_named_incumbent
    path: crates/hares-core/tests/add_equipment_empty_name_exemption.rs
    name: add_equipment_rejects_an_explicit_id_held_by_an_empty_named_incumbent
---

# Red-team round 5: I-06 — the evictee exemption leaks into the add entrance; pre-pass idempotency pinned; suite verified under memory pressure

## Findings lifecycle across the red-team passes

- **Rounds 1–2** (malformed-id overwrite; `in_schema` prefix false-panic;
  counter `u32::MAX` reissue; "Total" aggregate shadow; misdirecting
  rejection diagnosis): fixed, pinned green.
- **Round 3**: clean; one air-cover test.
- **Round 4** (replace-entrance name collision; WI-4 `max_soc` blind spot;
  telemetry-guard false positive): all three subsequently fixed at the cause
  and green on this tree (verified by execution in the coverage triage and
  re-verified here: the syn-based guard dependency is in `hares-core`'s
  Cargo.toml, the replace-name check and the three-dimension stand-down
  predicate are in the working tree).
- **Round 5 (this pass)**: one new finding — the shared collision check's
  evictee exemption leaks into the add path as an empty-name exemption —
  plus one green air-cover pin (`assign_equipment_ids` idempotency) and one
  operator-requested robustness experiment (full suite under deliberate
  memory pressure; negative result, below).

## Finding (new, this pass): the collision check's evictee exemption exempts an empty-named incumbent on the add path

`Dwelling::add_equipment` and `Dwelling::replace_equipment` share
`assign_equipment_identity`'s explicit-id collision check
(`dwelling/mod.rs:3487-3490`):

```rust
let collides_with = self.equipment.iter().find(|e| {
    e.descriptor().id == requested
        && e.descriptor().name.as_str() != replace_name.unwrap_or("")
});
```

The name-keyed exemption is correct at `replace_equipment` — names are
unique, so it excludes exactly the evictee, whose id leaves the vector with
the swap. But on the add path `replace_name` is `None`, `unwrap_or("")`
turns the exemption into "skip any incumbent named `\"\"`", and there is no
evictee to exempt. An incumbent whose name is the empty string therefore
holds its id **exemption-free**: `add_equipment` with that explicit id
returns `Ok` and a second equipment joins the vector on the same id.

Consequences, from the code that runs next: duplicate ids collapse every
`EquipmentId`-keyed map — `equipment_id_by_name`'s `collect()` keeps both
entries pointing at the shared id, and the end-of-step
`equipment_core.insert(id, …)` writes both equipment's core output under one
key, last-inserted wins. Observation actors read whichever equipment
inserted last — the I-06 root cause re-introduced through a public
API. **No error surfaces in debug or release**: the desync
`debug_assert_eq!` at the snapshot compares the map's id against the live
descriptor, and under the collapse the two *agree* (both wrong), so the
assert that guards this contract cannot fire. Reachability: `add_equipment`
performs no name validation beyond uniqueness, so an empty-named equipment
is addable through the public Rust API (`descriptor().name = ""`), and the
type system puts no constraint on the explicit id of a subsequent add.

Demonstrated by execution (the attack test, left red):

```
add_equipment must reject an explicit id held by an incumbent even when
that incumbent's name is empty — the add path has no evictee, so the
name-keyed exemption must exempt nobody; the add returned Ok and the
dwelling now carries two equipment on id EquipmentId(13), collapsing every
EquipmentId-keyed map (the I-06 observation misbinding)
```

**Fix direction** (for the implementer; no production code was touched):
make the exemption conditional on there being an evictee at all —
`replace_name.is_some_and(|evictee| e.descriptor().name.as_str() != evictee)`
— or key the exemption to the evictee's vector position rather than its
name. One expression; the companion pins that prevent over-rejection
already exist (`replace_equipment_rejects_collision_with_survivors_but_
accepts_the_evictees_id`,
`replace_equipment_with_the_same_name_keeps_the_identity_contract`), and the
attack test's own setup `.expect` rejects a fix that responds by banning
empty-named adds outright.

## Air cover (green): the io pre-pass is idempotent under repeated invocation

`assign_equipment_ids` is public and its preserve-on-valid contract implies
idempotency, but no existing test invoked it twice
(`same_config_builds_identical_name_to_id_mapping` covers two *builds*, not
a double pass over one spec list). New test
`assign_equipment_ids_is_idempotent_on_repeated_invocation`
(`crates/hares-io/tests/equipment_id_assignment.rs`): a spec list holding
one id of each class (absent, explicit valid, malformed) run through the
pass twice must yield identical ids both times — explicit ids preserved,
malformed ids untouched. Passes today; pins the preserve/assign boundary
against a future reordering of the pass's match arms or a retry-shaped
caller.

## Operator-requested experiment: the suite under deliberate memory pressure

Run at the operator's instruction (motivated by the flaky
`radiant_gain_weight_distribution_zero_allocations` finding the coverage
triage flagged as the riskiest unaddressed item): a bounded memory hog held
**30.3 GB** (~50% of available RAM) for the duration of a full
`cargo nextest run --workspace --no-fail-fast`, with nextest saturating all
28 cores alongside it. Test binaries were prebuilt first so rustc was not
the process under pressure (a rustc OOM would have been a false
reproduction).

Result: **6943 passed / 0 failed / 12 skipped — byte-identical arithmetic to
the coverage triage's baseline** — `radiant_gain_weight_distribution_
zero_allocations` passed (0.014 s), zero FAIL/ABORT lines in the log, no
OOM kills in the kernel log, memory fully released after the run. The
method matters here: the hog was **bounded** — an unbounded
allocate-until-MemoryError hog is what OOM-kills WSL, so the pressure was
sized to stress without destabilizing the host.

This is a *negative* reproduction: the flake does not reproduce under
amplified synthetic memory/CPU pressure, which argues it is not raw
resource pressure but a narrower interleaving (or a transient of the one
round that observed it). It does not change the tracking recommendation —
deflake-or-document before someone weakens the zero-alloc assertion — but
it narrows the search space and independently re-establishes that the
I-06-era suite is robust under hostile resource conditions.

## Attacked and traced — no defect, no test

Each hunted to an outcome; none is silent:

1. **Composer evaluated-then-truncated on override-fired steps advances
   preference state without a dispatch** (the WI-4 rewrite runs
   `composer.evaluate` before the override decision, then truncates). The
   only mutating preferences are SocGate (hysteresis latch) and
   PriceOptimizer (day-keyed threshold cache). Both mutate from env/SOC
   values — the latch condition was honestly evaluated at the step's SOC
   regardless of which dispatch lands, and the cache is recomputed
   idempotently per day. No dispatch-derived preference state exists, so
   truncation cannot desynchronize anything. The override always emits an
   explicit dispatch on its steps (path exclusivity preserved by the
   `composer_start` truncate), so the receiver never falls back to a
   default.
2. **Stale `last_resolved_{target_soc,power_kw,max_soc}`** standing the
   override down on a dead plan: all three exits of `ChargingComposer::
   evaluate` (constraint-override vote, idle no-preferences, resolved fold)
   update all three fields — no emission path skips the refresh. NaN
   inputs fail every `>=`/`is_none_or` clause toward the fire side
   (fail-safe).
3. **Truncation eating dispatches from earlier in `decide`**:
   `composer_start = out.len()` is computed at entry to
   `evaluate_charging`, after any earlier pushes in the same decide;
   `truncate` removes exactly the composer's own emissions.
4. **Checkpoint restore misbinding under duplicate/extra
   `equipment_states` entries**: the count check is symmetric, and the
   name-keyed lookup's duplicate-collapse always leaves a name missing →
   loud error naming it. Actor restore is name-keyed,
   schema-version-gated, empty-blob-probed, and count-checked; extra actor
   states (checkpoint grown) are unreachable in practice — there is no
   actor-removal API, and no misbinding is possible for states that match
   nothing.
5. **Zoneless-declaration asymmetry**: `ensure_ports_satisfied` (post-step)
   rejects a Thermal/Humidity declaration with `zone: None`, while the
   pre-first-step rebuild (`rebuild_port_slots` → `PortSlots::
   from_declarations`) silently skips it — as assembly has always done.
   No production constructor produces a zoneless thermal/humidity
   declaration (all route through `PortDeclaration::thermal(zone)`-style
   constructors that set the field); the pre-step behavior matches
   assembly's standing semantics, and the post-step guard is strictly
   additive loudness. Pinning a rejection pre-step would invent a contract
   the maintainers have not chosen; noted, not tested.
6. **Parameters-mirror divergence from the typed payload**
   (`explicit_equipment_id` reads `parameters` first): the mirror is built
   from the typed data at parse and `set_spec_equipment_id` writes both
   channels together, so the divergence shape (parameters-null shadowing a
   valid typed id) is not reachable from any construction path; a
   divergence would fail loudly at assembly (sentinel rejection) in any
   case.
7. **`prev_equipment_modes` empty after checkpoint restore** → first
   post-restore step fires mode-change interests once: the same
   first-observation convention the mid-run-add pin documents; one
   redundant actor wakeup, no misbinding.
8. **Counter/exhaustion boundaries at both entrances** (explicit
   `u32::MAX`, exhaustion error, never-reused semantics): pinned by the
   implementer's `equipment_identity_boundaries.rs`; re-traced the
   `checked_add`/skip-loop interplay and found it sound.

## Tests added (no production file touched)

- `crates/hares-core/tests/add_equipment_empty_name_exemption.rs`
  - `add_equipment_rejects_an_explicit_id_held_by_an_empty_named_incumbent`
    — **red, the finding**.
- `crates/hares-io/tests/equipment_id_assignment.rs` (appended)
  - `assign_equipment_ids_is_idempotent_on_repeated_invocation` — green
    air cover.

Verification on the final state (2026-09-23):

```
$ cargo nextest run --workspace --no-fail-fast
Summary [  97.500s] 6945 tests run: 6944 passed (1 slow), 1 failed, 12 skipped
# the single failure is the finding above, left red on purpose
$ cargo clippy -p hares-core -p hares-io --tests -- -D warnings   # clean
$ cargo fmt --check -p hares-core -p hares-io                     # clean
```
