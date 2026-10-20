# Audit: OD-13 (d9133308) and BR-22 (8082592e) at head 8082592e

Independent audit, 2026-10-05. Read-only; scratch worktree
`.workstrees/audit-head` at `8082592e` (submodule vendors/OCHRE at
`8681122b`), CARGO_TARGET_DIR `.workstrees/target-audit-head`, `-j 8`.
Worktree and target removed at the end.

## Verification run (all on the scratch worktree)

| Line | Result |
|---|---|
| `[fmt]` `cargo fmt --check --all` | exit 0 |
| nine `[clippy*]` lines (plan.md section 3) | all nine exit 0 |
| `[test-debug]` nextest workspace | 7237 passed, 0 skipped |
| `[test-release]` nextest workspace --release | 7194 passed, 0 skipped |
| `[golden]` default | 6/6 identical |
| `[golden]` `-F dst` | 1/1 identical |
| `-p hares-envelope -F observe fluid_solver` (extra, for BR-21) | 30 passed |
| `[python-build]` maturin develop --release | ok |
| `[pytest]` `-m "" -rs -p no:cacheprovider` | 1396 passed, 4 xfailed, 0 skipped (zero SKIPPED lines, so the empty census is satisfied) |
| parity + thermal trace + conversions, `-v -rxX` | parity 11 PASSED + 1 XFAIL (Lighting); thermal trace 1 PASSED (columns present) + 3 XFAIL; conversions 14 PASSED (6 fleet) |

Findings count: 0 high, 6 medium (M1, M2, M3, M3b, M4, M5), 15 low (L1-L15),
4 info.

## OD-13: findings

### Medium

**M1. The `ochre`-marked tests run in CI only on manual dispatch.**
`.github/workflows/ci.yml:254` gives `slow-python-test` the condition
`if: github.event_name == 'workflow_dispatch'`; the default job at `:233`
runs `-m "not slow and not ochre"`. So the OCHRE parity, the thermal trace
and the six fleet conversion tests never run on a push or pull request. The
ledger's stated harm (a pint release that breaks OCHRE passing unseen, BR-6c)
is closed only for the operator's local `[pytest]` block, not for CI.
Worse, the six fleet conversion tests need no OCHRE at all
(`tests/python/test_py_conversions.py`, the `@pytest.mark.ochre` decorators
at the six fleet tests, about lines 175-270); the marker removes them from the
automatic Python job. Fix: drop `@pytest.mark.ochre` from the fleet tests so
the default job runs them; run the slow/ochre job on push or pull request, or
state in the ledger that CI coverage of OCHRE is BR-10's row.
Confidence: high (read from the files).

**M2. The parity module cites a safeguard that does not exist.**
`tests/python/test_ochre_parity.py:162-166` says a missing-column assertion
inside an xfail case would be swallowed, "pinned loudly by
test_output_columns_present instead". `test_output_columns_present` exists
only in `tests/python/test_thermal_trace.py:269` and checks the thermal
columns. Nothing pins the parity columns, so if HARES drops or renames
"Lighting End Use Electric Power (kW)", `resolve_hares_kwh` returns None, the
`assert h_val is not None` at `test_ochre_parity.py:~208` raises
AssertionError, and the strict xfail (`raises=AssertionError`) reports it as
the declared XFAIL. Fix: add a parity column-presence test (no xfail) over
`PARITY_CHECKS`, or make the xfail match on the message.
Confidence: high (grep: one definition, in the other module).

**M3. Three of the four xfails mark HARES defects with reasons that do not
cite a reference.** `tests/python/test_thermal_trace.py:296-301, 399-403,
451-455` say "needs IAM/SHGC calibration" and "HVAC cycling and solar gain
differences propagate". The OD-11 and OD-13 rule keeps a mark only when its
reason cites the reference showing HARES is correct (EnergyPlus, ASHRAE) or the
documented OCHRE defect; any other divergence is a defect that gets a
`09-numeric-fixes.md` entry. `grep` finds no numeric-block entry for these
three (`ledger/09-numeric-fixes.md` has no `thermal_trace` or `xfail` hit).
A 30 percent window-solar overstatement and a 4.6 C overnight divergence are
open defects with no owner. The overnight reason also quotes "mean abs ~0.87 C"
while my run prints 0.9178 C (`test_thermal_trace.py:400`). Fix: open the
numeric entries and cite them in the reasons; refresh the stale number.
Confidence: medium (I read OD-11/OD-13 text and grepped the numeric ledger; an
owner may exist under a name I did not grep).

**M3b (ledger integrity). Two `## OD-34` headings.**
`ledger/11-other-defects.md:2203` (tariff attach/checkpoint) and `:2619`
(Lighting end-use aggregate); `00-schedule.md:196` names the tariff one at
row 21m while OD-13's State and the Lighting xfail
(`test_ochre_parity.py` PARITY_XFAILS reason, `ochre_names.py:20`) cite "OD-34"
for the Lighting cause. A reader cannot resolve the citation. Fix: renumber
the Lighting entry. Confidence: high. (Rated medium because the xfail reason's
only pointer is ambiguous.)

### Low

**L1. Two of the eleven passing parity cases compare zero with zero.**
HVAC Heating 0.0/0.0 and Water Heating 0.0/0.0 (measured pairs in
`ledger/02-measurement.md`, OD-13 entry). They pass through the
`abs(o_val) < 1e-9` branch (`test_ochre_parity.py:~210`) and can never catch a
regression for the hour chosen (13:00 on 5 May, a one-hour window). The
tolerances (2 to 15 percent) were not weakened, but the measured agreement is
about 1e-15, so they are roughly thirteen orders loose; a regression of a few
percent passes. Suggest a heating-season window or a tolerance tightened to
the measured agreement for the bitwise-equal cases. Confidence: high.

**L2. Fleet fixture deviates from the entry text (documented, acceptable).**
`write_output=True` where the entry said `False`; the State records why (with
`False` the frames do not exist). I did not independently test the `False`
claim; the six tests pass with `True`. No action; the Change text should be
corrected to match.

**L3. Residual runtime skip sites remain** (`test_ev_tariff_integration.py:224,
295, 320`; `test_py_fleet.py:76, 418`; `test_py_steppable_fleet.py:26-48, 71`).
None fired in my run, and the empty census fails any that does, so the gate
holds; but the entry's claim that "no skip site remains that can fire in the
block environment" is about firing, not existence. The two "in this seed" skips
turn a behaviour regression (no charging events) into a skip. Suggest
asserting the events exist. Confidence: high.

**L4. Stale module docstring text in `test_thermal_trace.py`.** Docstring says
`maturin develop ... --features observe --release` is required; the block
builds without `observe` and the tests pass. Fix the docstring or state which
build is meant.

**L5. The skip-census history is not auditable.** `ledger/pytest-skip-census.txt`
is gitignored (`.gitignore:63`), so I could not diff the old census (19 lines
per `02-measurement.md`; OD-13's Problem lists nine) against what now runs.
My `-rs` run shows zero skips, which is the property the empty file asserts;
the justification for the ten non-OD-13 lines is not in the repository.

### Verified (no finding)

- `pyproject.toml` `pythonpath = ["vendors/OCHRE"]`; no test module inserts
  VENDOR_OCHRE into `sys.path` any more (remaining hits are scripts and
  `bench_4month_der.py`, outside pytest).
- Deleted: `test_observer_diagnostics.py` (347 lines, five skip-marked tests),
  `test_print_comparison`, `test_7day_benchmark`, `_run_ochre_7d`,
  `_run_hares_7d`, the unused step-based `_run_hares`. Each had no assertion.
- Missing-column skips became assertions at four sites
  (`test_ochre_parity.py` two, `test_thermal_trace.py` solar peak and the 48 h
  HVAC heating check).
- Original nine tolerances unchanged; three new cases added (Water Heating,
  Lighting, Other) with stated tiers; `Other` is the fifteen-member
  per-equipment sum (`ochre_names.py`), a definitionally exact mapping.
- Fleet fixture exists (`tests/fixtures/resstock/2025.1/bldg0000002/home.xml`,
  `in.schedules.csv`, `weather/G0900090_2018.csv`); six tests pin shapes and the
  1e-12 aggregation math.
- CI slow job gained the submodule step (`ci.yml:260-261`).
- strict xfails use `raises=AssertionError` and none XPASSed.

## BR-22: findings

### Medium

**M4. A kept `allow(deprecated)` shelters a dead deprecated shim.**
`crates/hares-equipment/src/lib.rs:549-552` re-exports `save_versioned`
"for backward compatibility"; `crates/hares-equipment/src/serial.rs:57-61`
marks it `#[deprecated]`. `git grep` finds no caller anywhere
(`save_versioned(` appears only at its definition and the re-export). The
codebase is greenfield (no backward-compat shims), so the lint is not wrong for
the site: the item is dead. Fix: delete the function and drop it from the
re-export list; the `allow` goes with it. Confidence: high.

**M5. `ordering_claim_is_factually_incorrect` now asserts nothing.**
`crates/hares-core/tests/zone_state_ordering.rs:254-271`. The commit removed
the tautology `assert!(2168 < 2240, ...)` and its `#[allow(clippy::
assertions_on_constants)]` and left an empty body whose comment says "This test
passes unconditionally". That is the assert-nothing class OD-13 and BR-8
deleted. Fix: delete the test (it also hard-codes stale line numbers of
`mod.rs`). Confidence: high.

### Low

**L6. Two kept suppressions carry no reason at the site.**
`crates/hares-types/src/schedule.rs:302` (`ScheduleSource`) and `:382`
(`ScheduleSourceConfig`): `#[allow(clippy::large_enum_variant)]` with no comment
(the doc text above `:382` is about `deny_unknown_fields`). The ledger gives the
reason (boxing changes the public shape) but `.shipwright/constitution.md:68`
requires the comment at the site, and BR-22 says "a one-line reason". Fix: add
the line. Confidence: high.

**L7. Several kept reasons are not one line.**
`benches/common.rs:1-4` (3 lines), `tools/frame_golden/tests/common/mod.rs:3-8`,
`crates/hares-envelope/src/thermal_solver/mod.rs:2328-2330` (2 lines),
`crates/hares-types/src/panic_hook.rs:459-462, 487-490` (3 lines each),
`crates/hares-python/src/py_pv_sizing.rs:478-481` (3), `py_dwelling.rs:1800-1801`
(2). All are true and the lint is wrong for the site in each (per-binary shared
modules; 0.318 is a U-value, not 1/pi; intentional panics under catch_unwind;
pyo3 keyword signature is the API). Style only.

**L8. The kept census is 32, not 31, and the acceptance regex cannot see two of
them.** The ledger and commit say 31 kept. `git grep -E '\ballow\(' -- '*.rs'
':!vendors'` finds 32 sites: 22 pyo3 `too_many_arguments`, 2 panic_hook, 2
`large_enum_variant`, 1 `deprecated`, 1 `approx_constant`, 2 shared test-helper
modules, `ashrae152.rs:251` (cfg_attr, test-only field) and
`crates/hares-envelope/src/thermal_solver/longwave.rs:626-629`
(`cfg_attr(not(any(...)), allow(clippy::unused_enumerate_index))`), which the
ledger's list does not name. The acceptance regex
`#!?\[(cfg_attr\([^)]*, *)?allow\(` stops at the first `)` and the second
site spans lines, so the regex lists 30 and the ledger says 31. The red team's
"kept-31 census exact" is therefore not exact. The commit body also does not
list the kept sites (the ledger says it does; the body says "listed in the
ledger"). Fix: correct the count, name `longwave.rs:626`, and use `\ballow\(`
as the acceptance grep. `longwave.rs:626` is also fixable by BR-22's own method
(gate the `.enumerate()` under the cfg).

**L9. `#[expect(` suppressions are outside BR-22's census, and one is the thing
BR-22 targets.** Five sites:
`crates/hares-core/src/dwelling/mod.rs:1777` (`expect(dead_code, reason =
"reserved for potential future use")` on `zone_capacitances_j_k`, written at
`:2938`, never read from the `Dwelling` field: dead), `diagnostics.rs:758` and
`hares-envelope/src/state_space.rs:601, 640` (`too_many_arguments`, "bundling
would obscure", the rationale BR-22 rejected elsewhere), `mod.rs:5651`
(`expect(unused_must_use)` on `self.scheduler.build()`, which returns a
`#[must_use]` slice; `let _ =` or using the plan removes it). Pre-existing, not
introduced by the commit. Fix: extend the census regex to `(allow|expect)`.

**L10. New production `expect` in the warmup path.**
`crates/hares-core/src/dwelling/mod.rs:3034`:
`let iterations = warmup_result.expect("warmup just returned Ok");` under
`observe`. Unreachable (the Err arm returned at `:3022-3030`), value-identical,
but it replaces a `match` with a panic site to dodge a lint. A `match` that
returns on Err and yields the count under `observe` needs no `expect`.

**L11. A test lost an assertion with its field.**
`crates/hares-io/src/hpxml/building.rs:~5922`: `assert!((gg.wall_height_m -
wall_height).abs() < 1e-6, "wall_height")` was deleted with the dead
`GarageGeometry.wall_height_m` field, replaced by a comment claiming the
protruded-area check pins the height. It does indirectly
(`expected_protruded` uses the same height), but the intermediate is no longer
asserted. Acceptable; mention only.

**L12. Broken intra-doc link.**
`crates/hares-equipment/src/water_heater/tank.rs:80` doc says
``[`Tank::step_tempered`]``; there is no `Tank` type (it is `StratifiedTank`).
Not run through `cargo doc`, so unverified as a warning, but the target does not
exist.

**L13. Stale comments after the counter deletions.**
`crates/hares-core/src/actors/ideal_thermostat.rs:323-326` still says the
rejection is "observable counter/telemetry flag" after the counter was deleted;
`crates/hares-core/src/actors/ev_driver/mod.rs` (`EvDriverActor::new`, the
`// The parameters are independent behavioral inputs ...` block) is a
rambling leftover of the old suppression reason.

**L14. API footgun in the PV sizing parameter structs.**
`crates/hares-python/src/py_pv_sizing.rs` `size_pv_from_dwelling` takes a
`PvRoofTuning` whose `panel_watts`/`panel_area_m2` are silently overridden by
`system_tuning`'s resolved values (documented in the doc comment; the sole
caller, `py_dwelling.rs:~1845`, passes `None` for them). Value-identical to the
old code but invites misuse. Consider a roof tuning type without the panel
fields, or one struct.

**L15. MSRV (pre-existing, unverified).** `Cargo.toml:30` says
`rust-version = "1.89"`; the new `u64::is_multiple_of` use
(`crates/hares-tariff/src/evaluator.rs:301`) joins many existing ones
(`billing.rs:45`, `hvac_core.rs:967`, ...). I believe the unsigned method was
stabilized in 1.90; I did not verify against the release notes. BR-20 pins a
newer toolchain, so impact is nominal.

### Spot-check of 15 removals (all value-faithful)

1. `ideal_thermostat.rs` `setpoint_inversion_rejected_count`: write-only
   under `observe` (the `allow(dead_code)` proves no reader); the
   `telemetry.set("setpoint_inversion_rejected", 1.0)` and violation path
   are untouched.
2. `battery/mod.rs` `setpoint_violation_count`: same; the `tracing::warn!` stays.
3. `dwelling/mod.rs` `StageSnapshot` and the `stage_snapshot` field/assignment
   (debug-only, never read, cloned `PortSlots` per step in debug).
4. `dwelling/synthetic.rs` `wall_area_m2` plus `default_wall_area_m2`: the
   struct derives `Deserialize` without `deny_unknown_fields`
   (`grep` in `synthetic.rs` finds none), so leftover keys were inert; no
   `wall_area_m2` TOML key remains in `git grep` outside docs; the 24 literal
   removals are keys only.
5. `rng_word_pos_before` now `#[cfg(debug_assertions)]` (`mod.rs:5447`); clippy
   release and release-observe pass, so no un-gated reader exists.
6. warmup `iterations` regated under `observe` (see L10); behaviour identical.
7. `tariff/evaluator.rs:300-301`: `% != 0` to `!is_multiple_of`; differs only
   at divisor 0, and `interval_seconds == 0` returns Err at `:88-89` first.
8. `actors/bms.rs` `BmsParams`, `SelfConsumptionPolicy`: fields map by name,
   `with_name`/`new` bodies destructure identically.
9. `actors/ev_driver/mod.rs` `EvDriverParams`: all 14 fields map in order at
   `actor_registry.rs`, `dwelling/mod.rs`, `py_dwelling.rs` and the test helpers
   in the diff (read literal by literal; the 150-site total was not re-diffed).
10. `hares-physics/src/solar.rs` `SkyIrradiance`/`SunPosition`/
    `SurfaceOrientation`: zenith/azimuth and tilt/azimuth pairs keep their
    roles in `perez_tilted_irradiance`, `omni_directional_irradiance` and the
    `environment.rs` callers.
11. `observer_capture.rs` `PortBalance`: four f64 fields, named at the single
    caller (`mod.rs:6387`).
12. `heat_pump/defrost.rs` `DefrostInputs`, `water_heater/tank.rs`
    `TemperedDrawInputs`, `coil_physics.rs` `CoilOperatingState`: named fields
    match the old positional order at the callers I read (defrost tests,
    `gas.rs`, tank tests, `air_conditioner.rs`); the remaining `coil_physics.rs`
    and `stepping.rs` hunks were not read.
13. `py_pv_sizing.rs` / `py_dwelling.rs`: `PvRoofTuning`, `PvSystemTuning`,
    `PvSizingTargets` (see L14); Python signatures untouched.
14. Dead-code deletions: `conversions.rs` `obs_to_numpy`,
    `record_batch_arc_to_polars_df` (no callers), `pv/array_config.rs`
    `to_sam_index`, `pv/lut.rs` `sam_array_type`, `infiltration.rs`
    `q_latent_w`/`latent_flow_m3_s` fields (compiler-proven dead; `q_latent`
    local is still used for `latent_out`).
15. `envelope_lut.rs` `boundary_label`, `material_check.rs` `boundary_type`,
    `conductivity_w_m_k`: serde ignores unknown columns/keys; the only
    behaviour change is that a CSV without a "Boundary Label" column is now
    accepted (a relaxed check, immaterial).
Also read: `equipment.rs` test accumulator split by `observe` (compiled and run
in observe lines); `epw.rs` `clark_allen_sky_temp_c` to `#[cfg(test)]`;
`hpxml/resolve_water_heater.rs` and `gas.rs` `..Default::default()` removals.

### Orphaned TOML keys

Truly unread: `SyntheticGeometryConfig` no longer has the field; serde would
have ignored it before. `git grep wall_area_m2 -- ':!docs' ':!vendors'` leaves
only the unrelated `attic_gable_wall_area_m2` in a script and a local variable
and a comment in `synthetic.rs`. `tests/fixtures/bestest/{610,620,ce100,ce200,
s54_heat_pump}.toml` lines removed are the same key. No finding.

### Info

- I1. `[golden]` default 6/6 and `-F dst` 1/1 identical: no runtime change.
- I2. Counts: default 7237 and release 7194 match the ledger's record exactly.
- I3. `git grep 'allow('` for `.rs` code matches only the kept list plus the two
  sites in L8.
- I4. The deleted write-only counters were `observe`-gated; no observe CSV
  column was ever fed from them.

## Residual risk and gaps

- Observe, observe+profiling, dst and profiling nextest lines were not run
  (the brief named default and release); the observe `fluid_solver` subset was.
- The HELICS flake family (OD-32) did not appear in my single full pytest run.
- `cargo doc` and `cargo deny` were not run.
