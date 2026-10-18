# Audit: OD-16, engine fe804b59 (parent 1c18543a)

Auditor: independent (did not implement, review or red-team). Worktree of the
commit with vendors/OCHRE initialized, scratch edits only, removed afterwards
together with its target directory. Logs of every run are under the session
scratchpad (`aud/*.log`).

Counts: 0 blockers, 2 major, 5 minor. One pre-existing block failure
(`[clippy-release*]`) not introduced by this commit.

## Findings

### Major

M1. The release form is not a pure pass-through in the FSM when its
`deadband_c` is positive: it installs a runtime cooling override.
`crates/hares-equipment/src/hvac/thermostat.rs` (the "Deadband constraint"
block, about lines 585-615, running before the hysteresis write at 650-661).
The release form (both setpoints None) with `deadband_c: Some(d)` still takes
`d` as the deadband for the `effective.cooling_c <= effective.heating_c + d`
auto-correct. With base 20/24 and `d = 5.0` (inside the validator's [0, 5]
range) the condition is true: `candidate.cooling_c = Some(26.0)`,
`setpoint_violation_count` increments, a warning fires, and
`runtime_setpoints = Some({None, Some(26.0)})`. Autonomous control does not
resume; the cooling setpoint is moved to 26.
Reproduced with a scratch unit test (deleted): `release installed
RuntimeSetpointOverride { heating_c: None, cooling_c: Some(26.0) };
violations=1`. Reachable for any release carrying a deadband at or above the
base heating-to-cooling gap (4 C here). The commit's own comments claim the
deadband "is simply inert" (control_signal.rs validator comment) and "has
nothing to constrain" (thermostat.rs doc); that is false for the FSM.
The consumer's release uses 0.0, so the gate does not reach it. Fix: for a
signal naming no setpoint, skip the deadband constraint and the auto-correct
entirely (derive `deadband_c` only when a setpoint is named), and add a test
with a positive-deadband release.

M2. The other consumers of `ThermalSetpoint` still apply the release form's
deadband, or reject it, so "pure pass-through in every path" does not hold.
`crates/hares-equipment/src/water_heater/heat_pump_wh.rs:1397-1420`,
`resistance.rs:905-928`, `gas.rs:912-935`: a release (no setpoint named)
with `deadband_c: Some(0.0)` writes `self.deadband_c = 0.0` permanently (the
exact old HVAC defect shape: the release zeroes a hysteresis and nothing
restores it). `indirect_tank.rs:698-712` rejects `db <= 0.0` even for the
release form (a typed error where the HVAC path now accepts it, so the same
signal behaves three ways across equipment). `tankless.rs:653` ignores the
deadband. A consumer that broadcasts the release signal to every equipment
(the dispatch routes `ThermalSetpoint` by capability, `hares-control`
dispatch.rs:68) zeroes the water-heater deadbands. The water heaters have no
EBM window to collapse (the EBM lives only in hvac/), so the failure is
chatter and a lost configured deadband rather than a fatal invariant, but the
entry's own statement of the contract ("the release form is a pure
pass-through") is unmet and no test or ledger text bounds the claim to HVAC.
Fix: apply the same named-setpoint rule in the water-heater arms, or record
that the contract is HVAC only and why.

### Minor

m1. The per-zone thermal consistency skip has no test. Disabling the
`step_succeeded` filter in `verify_per_zone_thermal_consistency`
(`crates/hares-core/src/dwelling/mod.rs:4785-4800`) and running the full
hares-core suite: 1293 passed, 0 failed. The only new dwelling test
(`tolerated_step_failure_reconciles_the_electrical_sum`) uses an
electrical-only equipment. A failed-and-rolled-back HVAC equipment with a
stale `thermal_output_w` has no pin.

m2. The zero-deadband rejection at two of the three sites is not pinned.
With the validator check (`crates/hares-types/src/control_signal.rs:263-284`)
disabled and with the IdealHvac check (`ideal_hvac.rs:963-976`) disabled,
hares-types, hares-equipment and hares-control all pass (3106/3106 apart from
my scratch test). The hvac_tests named-form test passes through the
validator's twin in the FSM, so it does not prove the validator; there is no
`validate_numeric_bounds` test with a named zero deadband in
`control_signal.rs` tests (existing tests cover -0.1, 6.0, NaN, collision
only). The FSM zero check is pinned (unit test fails when disabled); the FSM
non-finite branch (`!db.is_finite()`) has no test. IdealHvac's check is also
unreachable through `apply_control` and `apply_control_unchecked`
(lib.rs:222-236: both run `validate_numeric_bounds` first), so it can only be
tested by calling `apply_signal` directly, which no test does.

m3. The "typed error at every site" claim is incomplete and the sites are
triplicated. `ideal_hvac.rs:~970` uses `*db <= 0.0`, which is false for NaN,
so a NaN deadband with a named setpoint passes that check and the setpoints
are applied while the NaN is silently dropped by the later `is_finite()`
guard. The FSM site uses `!db.is_finite() || *db <= 0.0`; the entry records
the NaN asymmetry as closed. It is closed only because the validator rejects
NaN first on the trait paths. The same predicate and the same 4-line error
text exist in three places (validator, FSM, IdealHvac), and two of them are
unreachable through the public entry points (see m2). One shared check (for
example a `ControlSignal` method) would remove the copies. The hysteresis
write is also still two sites (FSM and IdealHvac), not one; the AC, heat pump
heater and `apply_heating_control_unchecked` copies are gone as claimed, and
no other `hysteresis_c =` writes from a signal remain (grep over the
workspace).

m4. Recorded outputs still carry the failed equipment's stale power. The
consistency check now excludes the rolled-back equipment, but
`dwelling/mod.rs:4589` (`equipment_power_kw` telemetry), `:7080-7084`
(battery/EV/PV kW that feeds the next step's electrical summary) and
`:7244-7345` (frame rows) still read `core_output()` without the
`step_succeeded` filter that `:6956` applies to `latest_env`. After a
tolerated failure the frame's per-equipment kW reports the last committed
value while the solver total, and therefore billing and net grid, excludes
it. Pre-existing and loud through `health.port_rollbacks` and the warning,
so this is not a masked failure of the check, but the "both sides see the
same set" reconciliation stops at the check. Decide and record whether the
failed step's row should report zero.

m5. Slop and writing-rule drift in added lines (the `[writing]` regex passes:
no em dash, no IDs, nothing flagged; these are the prose rules). History of
earlier behavior in shipped text: thermostat.rs "(writing a zero here
collapsed the EBM's energy window)" and the test comment "a zero written here
degenerated the EBM"; dwelling test doc "Before the reconciliation the
consistency check failed the step ..."; hvac_tests header "used to write ...".
The same rationale is restated five times (ControlSignal doc, validator,
FSM doc, FSM body twice, IdealHvac twice). `apply_heating_control_unchecked`
(`hvac/helpers.rs:197-203`) is now a one-line forwarder with an unused
`_equipment_name` parameter kept at seven call sites; inline it. The
`hvac_tests` EBM assertion hard-codes the default hysteresis as `1.0`
(`expected = CAP * 1.0`) instead of reading it from the config, which breaks
silently if the default moves.

## Not defects (checked)

- Skip exactness of the electrical sum: `step_succeeded[idx]` is set only on
  the success arm in each of the three stage loops (6294, 6370, 6423), every
  equipment steps once per timestep, and a failure restores `self.ports` from
  the pre-step snapshot by a swap (`rollback_failed_equipment_ports`,
  5650), so no partial contribution survives: no real inconsistency from an
  equipment that contributed partially can be hidden by the filter. The
  filter matches the existing convention at 6956 and 6510. An equipment never
  stepped (`false` default) is excluded from the sum, as before it added
  nothing to the solver total.
- The collapsed application sites are one path for the HVAC family: furnace,
  boiler, baseboard, heat pump heater (catch-all arm), AC and heat pump cooler
  all reach `HvacEquipment::apply_control_signal` ->
  `ThermostatFsm::apply_thermal_setpoint_signal`, the only writer besides
  IdealHvac.
- Validator: named zero is rejected; negative, above 5 and NaN are caught by
  the earlier range check; the release form passes the validator.

## Tests: does each fail without the fix

Reverting by scratch edit in the worktree:

| test | revert | result |
|---|---|---|
| `dwelling::tests::tolerated_step_failure_reconciles_the_electrical_sum` | remove electrical `step_succeeded` filter | FAILS: `sum(equipment_power_kw) 0.500000 kW does not match total_power_kw 0.000000 kW`; deterministic (fixed 500 W, fixed bestest 600 fixture) |
| `hvac_tests::actuate_release_keeps_hysteresis_and_the_ebm_window_open` | FSM writes hysteresis for the release form | FAILS |
| `hvac_tests::named_form_zero_deadband_is_rejected_before_state_degenerates` | same revert | FAILS |
| `thermostat::release_form_with_zero_deadband_is_a_pure_pass_through` | same revert | FAILS |
| `thermostat::apply_thermal_setpoint_signal_rejects_zero_deadband_on_named_setpoints` | disable FSM zero check | FAILS |
| per-zone thermal filter | remove | no test fails (m1) |
| validator zero check | remove | no test fails (m2) |
| IdealHvac zero check | remove | no test fails (m2) |

All new tests are deterministic (no RNG, no schedules).

## Numbers

- `frame_golden compare-all` default: consumer_shape_900s, fleet_mixed_offset,
  full_year_hourly, leap_year_february_900s, weather_wrap_900s all identical.
  `-F dst`: full_year_900s_dst identical. `-F profiling`: all five identical.
  No golden moves, matching the measurement record.
- Not reproduced: the consumer-side claims (the four ami2 cases passing, 336
  passed / 0 failed / 7 skips, B0 de-trace identical); they need the consumer
  repository and an engine-under-test install, outside this audit's worktree.
  The record's "no added allocation" claim agrees with the diff (a boolean
  filter per equipment; the `vec![false; n]` at mod.rs:5908 is pre-existing).

## Verification block (worktree of fe804b59, `-j 8`)

- fmt: pass.
- clippy (all-features), default, dst, profiling, observe, observe+profiling:
  pass with `-D warnings`.
- clippy `--release`, `--release -F dst`, `--release -F observe`: FAIL.
  `crates/hares-envelope/src/thermal_solver/mod.rs:1858`: unused import
  `BoundaryDiagnosticInfo` in the `cfg(test)` module (its only use is at 6679,
  evidently compiled out of release test builds). Pre-existing: this commit
  does not touch hares-envelope, and the lines blame to 2026-05. Per plan.md
  section 3 the block must be green at every entry, so a fix entry is owed
  (not attributable to OD-16).
- nextest: default 7155 passed, `-F observe` 7214, `-F dst` 7168,
  `-F profiling` 7161, hares-core `-F observe,profiling` 1348, `--release`
  7112: all passed, 0 failed, 0 skipped.
- `cargo bench --workspace --no-run`: pass.
- Physics: `make check-ashrae-reference` fails here only because
  `xmltodict` is not installed (environment); with
  `uv run --extra ochre scripts/check_ashrae_reference.sh` it PASSES.
  hares-core bestest/parity/conditioned_oracle/freefloat_oracle/
  structural_envelope_oracle/envelope_oracle: 25 passed. hares-physics
  physics_validation_tests and solar_parity: 53 passed.
- Not run: `[advisories]`, python-build, pytest, the Python OCHRE parity
  tests (not requested).
- `[writing]` on the commit message and added lines (non-fixture, non-lock):
  clean, no non-ASCII characters added.

## Verdict

The defect is fixed as the entry states for the HVAC thermostat path with a
named or zero deadband, goldens are bitwise, and the headline tests are real.
The commit should not go final until M1 (release not a pass-through when its
deadband exceeds the setpoint gap) and M2 (water-heater consumers) are fixed
or the contract narrowed, the three unpinned behaviors (m1, m2) have tests,
and the pre-existing release clippy failure is addressed.
