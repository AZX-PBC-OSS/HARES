# Audit: OR-11, commit b8195c83 ("fix: enforce every physics, conservation, non-finite and wiring check in every build profile")

Blockers: 3. Majors: 1. Minors: 3.

Auditor did not implement, review or red-team this commit. Worktree:
`/home/rich/src/HARES/.workstrees/audit-7b7a6a7d` (checked out at `7b7a6a7d`,
which is `b8195c83` plus the unrelated SL-7 commit touching only
`hares-envelope/src/state_space.rs` and `stepping.rs`). All commands below
ran in that worktree.

## Blocker B1: six "zone unresolved" wiring checks shipped as debug-only warnings, not the promised unconditional typed errors

OR-11's own census table (`ledger/03-output-reporting.md`) dispositions the
following rows as **"unconditional typed error naming the water heater (or
equipment) and its unresolved zone (no silent fallback zone)"**:

- `crates/hares-equipment/src/water_heater/gas.rs:18-19,302-309`
- `crates/hares-equipment/src/water_heater/heat_pump_wh.rs:18-19,414-421`
- `crates/hares-equipment/src/water_heater/indirect_tank.rs:25-26,243-250`
- `crates/hares-equipment/src/water_heater/resistance.rs:304-311`
- `crates/hares-equipment/src/water_heater/tankless.rs:15-16,109-116`
- `crates/hares-equipment/src/hvac/dehumidifier.rs:552-559`

`git diff b8195c83^ b8195c83` for each of these files shows only the cfg
guard narrowed from `#[cfg(any(debug_assertions, feature =
"check_invariants"))]` to `#[cfg(debug_assertions)]`; the body is still a
bare `tracing::warn!`, never an `Err(...)`. Example (`gas.rs`, post-commit):

```
@@ -299,7 +299,7 @@ impl GasWH {
-        #[cfg(any(debug_assertions, feature = "check_invariants"))]
+        #[cfg(debug_assertions)]
         if c.zone_id.is_none() && c.zone_type.is_some() {
             warn!(
```

Confirmed identical pattern in `heat_pump_wh.rs:411-421` (a second,
unrelated check in the same file, the HPWH COP finiteness check, *was*
correctly converted to an unconditional `Err`, proving the author knew how
to do the conversion but did not apply it to this row),
`indirect_tank.rs:26-28`, `resistance.rs:17-19`, `tankless.rs:18-20`, and
`dehumidifier.rs` (where the *other* check in the same file,
`check_invariants`→`check_part_load_bounds`, was correctly made
unconditional, but the zone-kept-on-init warn at line ~545 stayed
`#[cfg(debug_assertions)]`).

Effect: in a release build (the shipped/default profile) a water heater or
dehumidifier whose HPXML zone_type is set but whose zone_id never resolved
is handled exactly as it was before this commit: completely silently,
with no warning, no log, and no error. This is precisely the "silent
substitution on physical/wiring state" the entry's own text and the
commit message ("the census's... wiring checks become unconditional typed
errors... no silent substitution") forbid, and it directly contradicts the
written disposition of OR-11's own census table for these six rows.

**Fix:** change each of the six rows from a `tracing::warn!` under
`cfg(debug_assertions)` to an unconditional `return Err(HaresError::...)`
naming the equipment and the unresolved zone, as the table already says to
do, mirroring the pattern already correctly applied to
`heat_pump_wh.rs`'s COP check and `dehumidifier.rs`'s PLF/RTF check in the
same commit. Add (or extend) the acceptance test for each equipment type
so that a config with `zone_type` set and `zone_id` absent fails
construction in every build profile.

## Blocker B2: the synthetic closed-envelope wiring check was never resolved, and the authoritative ledger entry does not record a stop for it

`crates/hares-core/src/dwelling/synthetic.rs:1368-1431` is classified
**"wiring"** in OR-11's census table with disposition **"unconditional
typed error naming the missing face category."** The shipped code:

```rust
#[cfg(debug_assertions)]
{
    // ...
    // STOPPED ROW: dispositioned as an unconditional typed error, but it
    // fires on existing unit-test fixtures whose minimal one-wall
    // geometry is scaffolding for other behaviors; the row stays a
    // diagnostic log pending the operator's decision on those fixtures.
    if config.geometry.zone_volume_m3 > 0.0 {
        ...
        if face_categories < 2 {
            tracing::error!(..., "Synthetic building envelope is incomplete...");
        }
        ...
    }
}
```

This is not a typed error and not even a `debug_assert!`: it is a
`tracing::error!` that never fails anything, gated out entirely in release
builds. The code's own comment says the row is "pending the operator's
decision." `decision-log.md`'s `or11_resolution` claims "three census
rows fired the stop condition and were re-dispositioned... each now
`debug_assertions`," but (a) this is not what `debug_assertions` means in
the entry's own taxonomy (a `debug_assert!`, which panics on violation;
see `dr_compliance.rs:622-634` for a correct example), and (b) no such
re-disposition, and no operator sign-off, appears anywhere in
`ledger/03-output-reporting.md`, which the entry's own stop-condition rule
requires ("the row, the input and the error are recorded here and the
operator decides"). The census table for this exact row (line ~1259 of
`03-output-reporting.md`) still reads "unconditional typed error," so the
authoritative ledger file and the shipped code disagree, and neither
matches the decision-log's prose.

A non-physical, open thermal envelope passing silently through release
builds is exactly the class of defect OR-11 exists to close.

**Fix:** either (a) implement the originally-specified unconditional typed
error and fix or quarantine the scaffolding fixtures it legitimately
fires on (moving them to a path that does not go through
`SyntheticBuilding` construction, or giving them a second wall/roof/floor
boundary), or (b) if the operator genuinely accepts non-enforcement, write
an explicit "Resolution (operator, <date>)" block in
`ledger/03-output-reporting.md` for this row (as was done for the
fluid-loop stop) naming the fixtures and the reasoning, update the census
table's disposition column to match, and replace the bare `tracing::error!`
with a real `debug_assert!` so debug builds at least fail loudly as the
"debug" class requires. Either way, remove the "pending the operator's
decision" comment before merge: a landed commit cannot ship with an
unresolved stop marker.

## Blocker B3: the ideal_hvac end-use/capacity-sign check does not implement its own "debug_assert!" disposition

`crates/hares-equipment/src/hvac/ideal_hvac.rs:841-867` is classified
"debug" with disposition **"`debug_assert!`"**. The shipped code is:

```rust
#[cfg(debug_assertions)]
{
    // STOPPED ROW: dispositioned as a plain `debug_assert!`, but it
    // fires on the conditioned-ideal oracle (a thermostat override
    // drives cooling while the FSM-mode-driven end_use still reads
    // hvac_heating); the row stays a diagnostic log pending the
    // operator's decision on that path.
    if capacity_w != 0.0 {
        ...
        if self.descriptor.end_use != expected_end_use {
            tracing::error!(..., "IdealHvac invariant violated...");
        }
    }
}
```

Same pattern as B2: a bare `tracing::error!`, not a `debug_assert!`, so
even in debug builds nothing fails. Lower severity than B1/B2 because the
row's own class ("debug") does not require the check to survive into
release, but it still does not do what the table says, and it ships with
the same unresolved "pending the operator's decision" marker with no
recorded resolution in the ledger.

**Fix:** same as B2's option (b): record the operator's decision in
`ledger/03-output-reporting.md` for this specific row (the entry's text
already names the mechanism, "a thermostat override drives cooling while
the FSM-mode-driven end_use still reads hvac_heating", and this reads like a
genuine, acceptable exception), update the table's disposition column
from "`debug_assert!`" to e.g. "diagnostic log, see resolution," and
remove the "pending" comment. If the operator instead wants the check
live in debug builds, convert the `tracing::error!` to an actual
`debug_assert!`.

### Process point common to B2 and B3

`decision-log.md`'s `or11_resolution` entry states the commit "landed as
b8195c83 (181 files) after a review pass... and a red-team pass
(ATTACK-FAILED...)" and describes all three stopped rows as resolved
("each now `debug_assertions`... identical behaviour in every buildable
profile"). That is inaccurate for two of the three rows (B2, B3): they are
log statements, not assertions, and the shipped code's own comments say
the stop is still "pending the operator's decision." Per
`.shipwright/initiatives/I-08/plan.md` §3's stop-condition rule and the
"shipwright files authoritative" policy, the row, its input and its
resolution belong in the ledger entry itself, not only (and not
inaccurately) in `decision-log.md`'s prose. This needs correcting
regardless of which fix is chosen for B2/B3.

## Major M1: `cargo fmt --check` fails (pre-existing, not introduced by this commit, but currently red)

```
Diff in crates/hares-envelope/src/state_space.rs:714
Diff in crates/hares-envelope/src/state_space.rs:1787
Diff in crates/hares-envelope/src/state_space.rs:2221
Diff in crates/hares-envelope/src/thermal_solver/mod.rs:7156
```

`git blame` on these lines attributes them to commits from March and
September 2026 (`dc69f823d`, `3f463c34e`), before `b8195c83`, so this is
not a regression introduced by the audited commit. It is not in the
`plan.md` §3 verification block's listed line set (`[fmt]` is not named
among `[clippy]`/`[test-*]`/`[golden]`/etc. in the excerpt reviewed), so it
does not gate this entry. Reported as a residual-risk item since the repo
is not currently `cargo fmt --check`-clean.

## Minor m1: ledger table disposition for `hvac/helpers.rs` zone-fallback row contradicts the shipped (intentional) behaviour

`ledger/03-output-reporting.md`'s census table still reads, for
`crates/hares-equipment/src/hvac/helpers.rs:59-90`: **"unconditional typed
error naming the equipment and the HPXML location that mapped to no zone
(no silent ZoneId(1))."** The shipped code keeps the silent
`ZoneId(1)` fallback in every profile (a `tracing::warn!` only under
`cfg(debug_assertions)`, nothing at all in release), with a doc comment
giving a reasoned justification ("HPXML locations that map to no modeled
zone... legitimately leave the equipment zone unresolved"), consistent
with `decision-log.md`'s explanation ("the fallback is documented
legitimate behaviour for 'other heated space' equipment"). Unlike B2/B3,
this one reads as a deliberate, justified decision, not an unresolved
stop, but the authoritative ledger table was never updated to match,
so a reader of `03-output-reporting.md` alone would conclude (wrongly)
that this row is a typed error. **Fix:** update the table's disposition
cell to match the code and the decision-log's rationale.

## Minor m2: test environment gap, `vendors/OCHRE` submodule not initialized by default in the audit worktree

`cargo test --workspace` initially failed one test,
`pool_and_hot_tub_loads_parsed_from_ochre_fixture`
(`crates/hares-io/tests/hpxml_parsing_tests.rs:1695`), with "No such file
or directory" for `vendors/OCHRE/test/OS-HPXML Sample
Files/base-misc-usage-multiplier.xml`. `git submodule status` showed
`vendors/OCHRE` uninitialized (`-ffc8b56e...`). This predates `b8195c83`
(the test and its fixture dependency are unchanged by this commit) and is
a worktree/environment setup gap, not a code defect. Ran `git submodule
update --init vendors/OCHRE` (additive, non-destructive) and re-ran; the
test then passed. Reporting per the audit's "report testing gaps" rule:
anyone auditing a fresh worktree of this initiative should initialize
submodules before running the full suite, or the gate will show a false
failure unrelated to the entry under review.

## Minor m3: a spurious test failure from running concurrent `cargo test` invocations against one target directory (methodology note, not a code defect)

An initial run of `cargo test --workspace` and `cargo test --workspace
--release` launched concurrently in the same worktree produced one
failure, `thermal_solver_step_allocation_free_after_first_step` in
`-p hares-envelope --test radiant_gain_weights_zero_alloc`, panicking at
a line number (337) past the end of the current 275-line source file,
proof the binary that ran did not match the checked-out source, almost
certainly a build-artifact race from the two concurrent invocations
sharing `target/`. Re-running `cargo test --release -p hares-envelope
--test radiant_gain_weights_zero_alloc` alone, and then a full clean
sequential `cargo test --workspace --release`, both passed with 0
failures. Noted so this false signal is not mistaken for a hot-path
allocation regression in a future audit; always run debug and release
suites sequentially, not concurrently, in a shared target directory.

## Verified correct (no finding)

- `check_invariants` feature: absent from every `*.rs`/`*.toml` in the
  workspace (`git grep -n check_invariants -- '*.rs' '*.toml'` → no
  output), and from the full census path (`git grep -n check_invariants --
  . ':!.shipwright' ':!docs/reviews' ':!docs/tickets' ':!docs/findings'
  ':!docs/superpowers' ':!scripts/reviews' ':!scripts/triage.py'` → no
  output). `.github/workflows/ci.yml`'s `-F check_invariants` release step
  is deleted, not edited, exactly as the entry specifies.
- `extract_fluid_loops_from_specs` and its name matching: fully deleted
  (`git grep` finds zero references).
- No `#[ignore]` attributes anywhere in `crates/`.
- Fluid-loop declarations: `FluidSolver::new`/`rebuild_loop_types` builds
  the loop-type map from `(LoopId, FluidType)` pairs
  (`crates/hares-envelope/src/fluid_solver.rs:100-114,209-216`), rejecting
  conflicting declarations as `HaresError::Envelope`
  (`fluid_solver.rs:105-111`). The caller builds this list from the live
  port-slot table's fluid accumulators at assembly and in
  `refresh_equipment_caches` (`crates/hares-core/src/dwelling/mod.rs:4220-4243`),
  which `add_equipment`, `remove_equipment` and `replace_equipment` all
  call (`mod.rs:3712,3917,4034`), gated to `current_step == 0` as
  documented. An accumulator on an undeclared loop raises
  `HaresError::InvariantViolation` (`fluid_solver.rs:327-333`), never a
  substituted value. `validate_fluid_type_consistency`
  (`crates/hares-types/src/ports.rs:840`) runs at construction
  (`dwelling/mod.rs:2722`).
- `FluidSolverConfig.fluid_specific_heats` and its `Default` table: gone.
  `unknown_fluid_type_falls_back_to_water_cp`: gone. The solver takes `cp`
  from `hares_physics::constants::cp_j_kg_k`
  (`fluid_solver.rs:347`), an exhaustive `match` over `FluidType`
  (`crates/hares-physics/src/constants.rs:135`).
- Boiler port fix: both `GasBoiler::init` and `ElectricBoiler::init` set
  `port.fluid_type = Some(self.fluid_type)` from the configured value
  (`crates/hares-equipment/src/hvac/boiler.rs:279,701`), confirmed by the
  new test `boiler_port_declares_configured_fluid_type`
  (`boiler.rs:2577`).
- Warning channel: `hares_types::Warning`
  (`crates/hares-types/src/warning.rs:17`), `Equipment::drain_warnings`
  implemented by battery, EV and PV equipment plus the dwelling's test
  equipment (`dwelling/mod.rs:9030`, `battery/mod.rs:1144`,
  `ev/mod.rs:1740`, `pv/mod.rs:1013`, `hares-equipment/src/lib.rs:282`
  default), `Building.parse_warnings` populated in
  `hpxml/mod.rs:188-193`, `ElectricTariff.parse_warnings`
  (`hares-tariff/src/types.rs:311`). `Telemetry::set`'s unknown-key latch
  (`crates/hares-types/src/telemetry.rs:74-91`) is surfaced in
  `run_timestep` (`dwelling/mod.rs:6460,6479`), matching the decision
  log's claim that this gap was found and fixed in review.
- Schedule columns: `ScheduleCategory::NotUsed { reason }`
  (`crates/hares-io/src/schedule_resolve.rs:51,204-226`); the named
  acceptance tests (`fixture_schedule_columns_are_all_known`,
  `unknown_schedule_column_warns_naming_it`,
  `resstock_homes_carry_no_schedule_warning`, plus the warning-channel
  tests `equipment_warnings_reach_the_dwelling_log`,
  `battery_heater_above_700_w_warns_at_init`,
  `ev_setpoint_above_rated_warns_once_per_run`,
  `multispeed_without_csv_entry_warns`,
  `hpxml_schema_warnings_reach_building`,
  `contradicting_hemisphere_is_a_tariff_parse_warning`,
  `set_tariff_reports_tariff_parse_warnings`) all exist in the tree.
- `crates/hares-core/tests/fluid_loop_declarations.rs` contains
  `fluid_loops_come_from_port_declarations`,
  `runtime_added_equipment_declares_its_loops` and
  `conflicting_port_fluid_types_are_a_construction_error`, each exercising
  behaviour (runtime-added loop declaration, conflicting-type
  construction rejection) that the pre-fix `extract_fluid_loops_from_specs`
  name-matching code could not have passed: these tests would fail
  without the change, by inspection (the old code never read runtime
  equipment's `ports()`, and had no conflict check at all).
- `crates/hares-core/tests/schedule_columns.rs` compiles and runs; the
  rust-analyzer-flagged issues (undeclared `DateTime`, `?` on a toml
  error, `as_u64` on `toml::Value`, an i64/u32 mismatch) are not present
  in the checked-out file: `chrono::DateTime` is imported, the toml
  parse error is mapped via `std::io::Error::other`, and `cargo check
  --workspace --tests` compiles clean. This claim appears to be stale or
  mistaken; the test target compiles and passes.
- Hot-path restructuring: `build_hvac_thermal_consistency`
  (`dwelling/mod.rs:1459-1480`) builds per-zone equipment index lists once
  at assembly/refresh; the per-step
  `verify_per_zone_thermal_consistency` (`dwelling/mod.rs:4768+`) only
  iterates those pre-resolved indices; no `HashMap`, `HashSet` or sort in
  the step path, matching the entry's claim.
- No em dashes or other LLM tics introduced in the diff's prose
  (`CONTRIBUTING.md`, `README.md`, code comments), checked via pattern
  search for U+2014 in the added lines.
- `#[should_panic]` tests in `fluid_solver.rs`: none remain (converted to
  `Err`-asserting tests as the table specifies).

## Verification run (all in the audit worktree)

- `cargo check --workspace --tests`: compiles clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: 0
  warnings/errors.
- `cargo fmt --check`: 3 pre-existing diffs (see Major M1), none in files
  this commit touches in a way that would be new.
- `cargo test --workspace` (debug, clean sequential run, after
  initializing the `vendors/OCHRE` submodule): 7133 passed, 0 failed
  (summed across all test binaries and doctests).
- `cargo test --workspace --release` (clean sequential run): 7090 passed,
  0 failed. Close to decision-log's claimed "release suite... passes
  7086" (the small difference is consistent with the OCHRE-fixture test
  now running).
- `cargo run --release -p frame_golden -- compare-all`: all 5 fixtures
  (`consumer_shape_900s`, `fleet_mixed_offset`, `full_year_hourly`,
  `leap_year_february_900s`, `weather_wrap_900s`) reported `identical`.
- `cargo run --release -p frame_golden -F dst -- compare-all`: not run
  (time-boxed; the default-feature golden run above and the identical
  debug/release test results are the strongest signal available here).
  **Gap:** this `[golden]` line from `plan.md` §3 was not executed; a
  follow-up should run it before sign-off.

## Confidence and scope note

High confidence on B1 (six-file pattern, directly diffed against the
parent commit) and on the golden/clippy/test results (directly run).
Medium-high confidence on B2/B3 (code and its own comments read directly;
the only uncertainty is whether some other part of the tree the search
missed records an operator decision for these two rows; none was found
in `ledger/03-output-reporting.md` or `decision-log.md` beyond the
mischaracterizing prose already quoted). The `-F dst` golden line is an
acknowledged gap, not a pass/fail claim.
