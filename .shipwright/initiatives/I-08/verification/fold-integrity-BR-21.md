# Fold integrity: BR-21 source commits against head 8082592e

Independent audit, 2026-10-05. Source chain (base `1c18543a`): `d41646e9`,
`4872f244`, `4e54d56b`. Head fold: `91815306`, `19ca1069`, `0c0bece9`, plus the
extra commits `ede151da` and `398d21a6`. None of the source commits is an
ancestor of head (merge-base with head is `1c18543a`), so the comparison is by
patch, not by ancestry. Method: `git patch-id --stable`, per-file diff of each
source commit against its fold counterpart, then each source-only hunk checked
against head's file text. Scratch worktree at `8082592e`, removed afterwards.

Result: 0 high, 0 medium, 2 low, 2 info. Every source hunk is present or
superseded; every test the source touched exists and passes.

## Commit mapping

| Source | Fold | Relation |
|---|---|---|
| `4e54d56b` | `0c0bece9` | patch-id identical (`a4ab010a...`); same 12 files |
| `d41646e9` | `91815306` | differ: fold lacks three files' hunks (superseded, below) and rewords one comment |
| `4872f244` | `19ca1069` | differ: fold lacks seven files' hunks (superseded, below) and rewords the same comment |
| (none) | `ede151da` | extra: restructures the fluid-solver violation tests; reworded OCV note |
| (none) | `398d21a6` | extra: golden re-capture |

## d41646e9 against 91815306

Files common to both: `.github/workflows/ci.yml`, `Makefile`,
`scripts/check_ashrae_reference.sh`, `crates/hares-core/src/dwelling/
solver_builder.rs`. All identical except `solver_builder.rs`, where the fold's
comment on `r_total` reads differently (the source rewrote it again in
`4872f244`). The final text in both chains is the same two lines ("Read by the
r_glass identity check in debug builds and by the film-model divergence
telemetry in observe builds"). The `#[cfg(any(debug_assertions, feature =
"observe"))]` gate on the binding is already at base in the fold's chain, so
the source's gate change is not needed.

Source-only hunks, each checked at head:

1. `crates/hares-core/src/dwelling/mod.rs`: gate the `MoistureZoneInvariant`
   import. Head has it, `mod.rs:102-103`:
   `#[cfg(all(feature = "observe", debug_assertions))] use
   crate::observer::MoistureZoneInvariant;` and the grouped import no longer
   lists it. Present (identical text).
2. `crates/hares-envelope/src/thermal_solver/mod.rs`: drop
   `BoundaryDiagnosticInfo` from the test-module imports and import it inside
   the debug-gated test. Head's test module does not import it by name; the
   test uses the fully qualified path
   `crate::thermal_solver::BoundaryDiagnosticInfo::RCNode` (`mod.rs:6503`) and
   the module imports it at `:45` for production code. Superseded, and the
   release lint that motivated the hunk passes (`[clippy-release]` and
   `[clippy-release-observe]` exit 0 on the scratch worktree).
3. `tests/conditioned_oracle.rs`: module-level `cfg(all(observe, any(
   debug_assertions, observe_detailed)))`. Superseded twice: `4872f244`
   reverted it to `cfg(feature = "observe")` with item-level gates, and head
   has exactly that final form (`conditioned_oracle.rs:19` plus gates at
   `:428, :762, :785, :892`, introduced by `89eae0ba`).

## 4872f244 against 19ca1069

Common files identical in added/removed lines except the same `solver_builder.rs`
comment. That includes `fluid_solver.rs` (counter/peak imbalance recorded before
the error return, `fluid_solver.rs:686`, `:714`), `scripts/check_ocv.sh`,
`gen_ocv.py`, `gen_ashrae_reference.py`, `pv/mod.rs`, `battery/ocv.rs`,
`autosize.rs`, `infiltration.rs`, `hvac/heat_pump/defrost.rs`, `building.rs`,
`resolve_hvac.rs`.

Source-only hunks, each checked at head:

1. `scheduled_load.rs`: remove `#[cfg_attr(not(observe), allow(
   unused_variables))]` and `#[allow(clippy::single_match)]` from zone
   resolution. Head's zone resolution is rewritten
   (`scheduled_load.rs:207-225`, `role` then `map.get(role)`, an `Err` when a
   heat-giving load has no zone), by the unresolved-zone commits that precede
   the fold. No suppression at the site; `git grep allow(` finds none there.
   Superseded.
2. `water_heater/gas.rs`, `heat_pump_wh.rs`, `indirect_tank.rs`, `tankless.rs`:
   drop the unused `use tracing::warn` with its `allow(unused_imports)`. Head:
   none of the four imports `tracing::warn`. `resistance.rs`: drop only the
   `allow`; head has `use tracing::warn;` at `:17` with no allow. Identical
   final state.
3. `tests/conditioned_oracle.rs`: item-level cfg gating of the surface and
   window-solar diagnostic dumps. Head has them (see above).

## 4e54d56b against 0c0bece9

Identical patch-id. Head's `defaults/ocv_tables/*.toml` headers read
`# PyBaMM version: 26.9.0.0`; `scripts/check_ocv.sh` diffs
`defaults/ocv_tables` against the generator; `PyEv.from_vehicle_with_archetype`
lost the ignored `seed` argument in `py_equipment.rs:769`, the `.pyi`, and
`test_ev_archetypes.py`.

## Extra commits

- `ede151da`: splits the source's single violation test into
  `parallel_loop_splitter_topology_violation_is_a_typed_error`
  (`fluid_solver.rs:1997`) and `..._mixer_...` (`:2004`) over one helper
  (`:1912`). The source's observe assertions (`num_conservation_violations ==
  1`, peak imbalance 0.2) survive in the helper (`:1985-1994`) and are now also
  asserted for the mixer (0.3). The `ocv.rs` note change is comments only.
- `398d21a6`: seven golden manifests change only `defaults_digest`, `git_head`,
  `git_dirty`; nothing else differs (`git show -U0` filtered to other keys is
  empty). The digest moves because `0c0bece9` changed the OCV TOML header line
  from PyBaMM 26.8.0.0 to 26.9.0.0. Consistent.

## Tests the source added or changed

The three source commits add no new `#[test]` or `def test_` function (grep of
their patches for test markers: none). They modify:

| Test | Change | Head status |
|---|---|---|
| `fluid_solver::tests::parallel_loop_splitter_mixer_topology_violation_is_a_typed_error` (source name) | observe-only assertions on counter and peak imbalance | renamed `parallel_loop_splitter_topology_violation_is_a_typed_error` plus a mixer twin; `cargo nextest run -p hares-envelope -F observe fluid_solver`: 30 passed |
| `thermal_solver::tests::rc_node_boundary_diagnostic_uses_tarp_natural_convection` | local import | passes in `[test-debug]` (7237 passed) |
| `tests/conditioned_oracle.rs` tests | cfg gating | compiled and run in the observe lines; `[clippy-observe]`, `[clippy-release-observe]` exit 0; not executed by me under `-F observe` (default nextest compiles it out) |
| `tests/python/test_ev_archetypes.py` | `seed=` removed | passes in `[pytest]` (1396 passed, 4 xfailed, 0 skipped) |

## Verification run on the scratch worktree

`[fmt]` exit 0; nine `[clippy*]` lines exit 0; nextest default 7237 passed;
nextest `--release` 7194 passed; `[golden]` default 6/6 and `-F dst` 1/1
identical; pytest `-m ""` 1396 passed, 4 xfailed, 0 skipped. (Full table in
`audit-OD-13-BR-22-head.md`.)

## Findings

### Low

**L1. `4872f244`'s `conditioned_oracle` change has no test-level guard.**
Under default features the file compiles to nothing (`cfg(feature = "observe")`),
so the two diagnostic dumps and their gates are exercised only by the observe
lines. I ran clippy on those lines but did not run the observe tests. The block's
`[test-observe]` covers it; no action beyond noting that this audit did not run it.

**L2. The fold's chain no longer has a commit for the source's
`MoistureZoneInvariant` and `BoundaryDiagnosticInfo` hunks.** They are present
or superseded at head (above), but the fold's commit messages do not say the
base already carried them (`91815306`'s body drops the paragraph about the
observe build failures that `d41646e9` carried). A reader comparing the chains
by message alone would think the observe release build fix was lost. The head
`[clippy-release-observe]` pass is the evidence. Cosmetic.

### Info

- I1. Chain bases: `git merge-base d41646e9 HEAD` is `1c18543a`, 27 commits
  before head; the fold is a re-application, not a merge.
- I2. `ede151da` and `398d21a6` carry no behaviour change beyond the test
  restructure and the golden metadata.
