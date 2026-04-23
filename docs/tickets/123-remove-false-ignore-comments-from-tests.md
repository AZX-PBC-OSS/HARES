# Remove False `#[ignore]` Comments From Three Tests

**Severity**: Low
**Priority**: P3
**Status**: Open
**Areas**: hares-io/tests, hares-equipment/tests

## Problem

Three tests carry comments claiming `#[ignore]` status but actually run as part of the standard test suite. The comments are stale and misleading — a contributor reading the test will assume it is skipped and ignore failures or remove the test thinking it is dead.

The tests:
- `ac_has_startup_capacity_degradation_default` in `crates/hares-io/tests/hpxml_parity.rs`
- `ashp_backup_lockout_temperature_extracted` in `crates/hares-io/tests/hpxml_parity.rs`
- `hpwh_cop_at_multiple_ambient_temps` in `crates/hares-equipment/tests/water_heater_parity.rs`

## Current Behavior

Each test function carries an inline comment (or doc comment) claiming `#[ignore]` status. No `#[ignore]` attribute is actually present, so the tests run normally.

## Required Behavior

Remove the false claims. If the comment was intended to document why the test was at one point ignored, replace with a brief note explaining what was fixed and when it was un-ignored. Otherwise delete the comment outright.

## Approach

1. Open each of the three test files.
2. Locate the comment claiming `#[ignore]` status above (or inside) each test function.
3. Verify by inspection that no `#[ignore]` attribute is actually present.
4. Delete the comment, or replace with a useful explanation if there is contextual value.
5. Run the tests to confirm they still pass.

## Definition of Done

- [ ] False `#[ignore]` comment removed from `ac_has_startup_capacity_degradation_default`
- [ ] False `#[ignore]` comment removed from `ashp_backup_lockout_temperature_extracted`
- [ ] False `#[ignore]` comment removed from `hpwh_cop_at_multiple_ambient_temps`
- [ ] All three tests pass under the standard `cargo test` invocation
- [ ] No other test in the workspace carries a similarly false `#[ignore]` claim (audit via grep)

## Verification

```bash
cargo test -p hares-io --test hpxml_parity ac_has_startup_capacity_degradation_default
cargo test -p hares-io --test hpxml_parity ashp_backup_lockout_temperature_extracted
cargo test -p hares-equipment --test water_heater_parity hpwh_cop_at_multiple_ambient_temps
rg "ignore" crates/hares-io/tests/ crates/hares-equipment/tests/ | grep -v '#\[ignore\]'
```

## References

- Project policy `feedback_no_useless_comments.md` — comments must accurately describe what the code does.
- Project policy `feedback_no_ignore_tests.md` — never `#[ignore]` failing tests; debug and fix.

## Related Tickets

- 094-bestest-tests-still-ignored (related true `#[ignore]` removal)
