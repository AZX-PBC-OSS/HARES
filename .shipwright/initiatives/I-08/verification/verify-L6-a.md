# Verification L6-a

Scope: `ledger/03-output-reporting.md` (every uncommitted entry, OR-18 and
OR-19 at full depth), `ledger/05-python-surface.md`, `ledger/11-other-defects.md`
(every entry, OD-22 to OD-28 at full depth), `ledger/12-inputs.md`. Code
checked at `7b7a6a7d` in `/home/rich/src/HARES/.workstrees/audit-7b7a6a7d`.

Counts: 1 blocker, 0 major, 2 minor. 23 entries read in full; file:line
evidence spot-checked across all of them; OR-18, OR-19, and OD-22 through
OD-28 checked symbol-by-symbol against the committed tree.

No verification commands were run (`cargo nextest`, `cargo clippy`, the
goldens). All of these entries are unimplemented design text (every OR
entry in scope and every OD/IN/PY entry are "not started" or have no code
yet), so there is nothing to build or test; the relevant check at this
stage is whether the cited code exists as described and whether the
proposed fix is internally consistent. That is what this review did.
Running the verification block against the design text itself is not
possible and is not a gap in this review.

## Blocker

**OD-28 misdiagnoses dead code as a live DST bug, and contradicts OD-2
site 8 on the same lines.**

`ledger/11-other-defects.md` OD-28 names
`crates/hares-core/src/dwelling/blueprint.rs:95-102`:

```rust
let tz_offset = FixedOffset::east_opt((site_location.utc_offset_h * 3600.0).round() as i32)
    .unwrap_or_else(|| FixedOffset::east_opt(0).expect("UTC offset"));
let local_start = config.sim_config.start_time.naive_local()
    .and_local_timezone(tz_offset)
    .single()
    .unwrap_or_else(|| config.sim_config.start_time.with_timezone(&tz_offset));
```

and claims the `.single()` fallback "resolves silently" a civil time that
falls in a DST spring-forward gap or a fall-back ambiguity, proposing an
acceptance test (`a_start_time_in_a_dst_gap_is_a_construction_error`,
`a_start_time_in_a_dst_ambiguity_is_a_construction_error`) that must fail
against this code before the fix.

This cannot happen on this line. `tz_offset` is a `chrono::FixedOffset`,
not a `chrono_tz::Tz`. `FixedOffset`'s `TimeZone` implementation has no
DST transitions by construction: `offset_from_local_datetime` for a fixed
offset always returns `LocalResult::Single`, never `None` or `Ambiguous`.
`and_local_timezone(tz_offset).single()` is therefore always `Some` here;
the `unwrap_or_else` branch is unreachable dead code, not a gap/ambiguity
bug. The real civil-timezone value that could have a gap or an ambiguity
is `parsed_civil_tz: Option<chrono_tz::Tz>` (`blueprint.rs:112-119`,
`#[cfg(feature = "dst")]`), which is a separate field entirely and is
never used to compute `local_start` in this function (confirmed by `grep
-n 'parsed_civil_tz\|local_start' blueprint.rs`: the two names never
appear on the same line, and `local_start` is set once, from `tz_offset`,
before `parsed_civil_tz` is even parsed).

This directly contradicts `ledger/11-other-defects.md` OD-2 site 8, which
names the *same* two lines and reaches the opposite, correct conclusion:
"The `.single().unwrap_or_else(..)` at `:103-104` cannot trigger for a
fixed offset and is replaced by the infallible conversion." OD-2 site 8 is
right; OD-28 is wrong about the same code.

This is not just a correctness error in one entry's prose. `00-schedule.md`
places OD-28 at row 24o and OD-2 at row 31, so OD-28 runs first. If OD-28 is
implemented as written, it adds an unreachable error branch and two tests
that can never fail before the change (violating the acceptance-test rule
that each test "fails against `7b7a6a7d` before the change"), and it will
then conflict with OD-2 site 8's own change to the same lines when OD-2
runs afterward and finds the "infallible conversion" it describes no
longer matches what OD-28 left behind.

Correction: drop OD-28's gap/ambiguity claim and its two acceptance tests
for this code path. OD-28's out-of-range/non-finite UTC offset claim
(the `FixedOffset::east_opt(..).unwrap_or_else(|| ... east_opt(0) ...)`
line) is real and already covered, worded identically, by OD-2 site 8
("An offset outside ±24 h becomes UTC, and a non-finite offset rounds to
0 and also becomes UTC"). OD-28 as a whole duplicates OD-2 site 8 and
should be merged into it or dropped; if a genuine DST gap/ambiguity defect
exists, it is in the `#[cfg(feature = "dst")]` `parsed_civil_tz` path
(wherever that `Tz` is later used to interpret a local time), which OD-28
does not name or cite at all. That is a real gap this entry does not
cover and should, if the `dst` feature's local-time resolution is not
already error-checked elsewhere.

## Minor

1. `ledger/11-other-defects.md` OD-22's acceptance test
   (`equipment_init_failure_fails_assembly`) describes the triggering
   check as `ScheduledLoad`'s `init_from_config` rejecting "a
   `sensible_gain_fraction` above 1." At `crates/hares-equipment/src/scheduled_load.rs:325-329`
   the actual check is `self.sensible_gain_fraction +
   self.latent_gain_fraction > 1.0 + 1e-9`, not a check on
   `sensible_gain_fraction` alone. With the proposed test value (1.5) and
   a default `latent_gain_fraction` of 0.0 this still trips the same
   branch, so the test as designed should still work, but the entry's own
   description of which guard fires is inaccurate and should name the
   actual condition (sum of sensible and latent exceeding 1.0) rather than
   sensible alone, since a reviewer implementing this literally could
   write a test for the wrong invariant.

2. `ledger/03-output-reporting.md` OR-18 and
   `ledger/11-other-defects.md` OD-9/OD-17/IN-5 all cite OpenStudio-HPXML
   source files (`geometry.rb:1399-1428,1675-1678`, `hvac.rb:1641-1741`,
   `schedules.rb:1465-1484`) and documentation pages by URL as the
   physics authority for the proposed change. These citations could not
   be checked against the live upstream source in this review (no network
   access); they are reported as an unverified dependency, not confirmed
   or disputed. Every citation's line numbers were at least checked for
   internal consistency (e.g., the vendored OS-HPXML sample fixture
   `vendors/OCHRE/test/OS-HPXML Sample Files/base-bldgtype-multifamily-adjacent-to-other-heated-space.xml:315`
   that OR-18 cites does contain `<Location>other heated space</Location>`
   at that exact line, which is a good sign for the care taken, but the
   upstream Ruby source itself is out of reach here).

## Entries verified accurate (evidence matched code at `7b7a6a7d`)

- **OR-18** (full depth): every cited line in
  `crates/hares-equipment/src/hvac/helpers.rs`,
  `crates/hares-equipment/src/hvac/dehumidifier.rs`,
  `crates/hares-io/src/hpxml/resolve_loads.rs`,
  `crates/hares-io/src/hpxml/building.rs` (`zone_sort_key`),
  `crates/hares-io/src/hpxml/resolve_hvac.rs` (`conditioned_zone_id`),
  `crates/hares-core/src/dwelling/synthetic.rs:1367-1406`, and
  `crates/hares-equipment/src/hvac/ideal_hvac.rs:569-574,841-866` exists
  exactly as described, including the "STOPPED ROW" comments quoted
  verbatim in the ledger. The `DehumidifierConfig { zone_id: None, .. }`
  construction in `resolve_loads.rs` and the five water-heater files'
  `cfg(debug_assertions)` warn-and-continue pattern were all confirmed.
  The vendored OS-HPXML sample XML line citation was confirmed exact.
- **OR-19** (full depth): the Problem's seven call sites in
  `crates/hares-core/src/dwelling/mod.rs` (`add_equipment:3678-3714`,
  `remove_equipment:3927-3937`, `remove_equipment_by_end_use:3948-3966`,
  `clear_equipment:3911-3919`, `add_actor:3350-3359`,
  `auto_register_actors:3439-3485`, `replace_equipment:3980-4036`) were
  each read and all confirmed to mutate the roster and then call the
  fallible `self.refresh_equipment_caches()?` (`:4155`, itself calling
  `self.fluid_solver.rebuild_loop_types(&fluid_loops)?` at `:4237-4243`)
  with no rollback on `Err`. The Problem is real and precisely evidenced;
  the proposed plan/install split is a sound shape for the fix (no issues
  found in the design itself).
- **OD-17**: the duplicate `"occupants"` column push (constant 1.0 at
  `hpxml_schedule.rs:52-54`, then the `COLUMN_TO_PROFILE` loop's
  `("occupants", "Occupancy")` entry at `:16`) and the resulting
  `column_index` (a `HashMap`, which collapses the duplicate name to the
  second occurrence) versus `columns.len()` (which keeps both) mismatch
  were reproduced by reading the function directly; the ledger's
  description of the 14-vs-13 length mismatch mechanism is exactly
  correct.
- **OD-22, OD-23, OD-24, OD-25, OD-26, OD-27**: file:line evidence checked
  and matched (OD-22's lines are cited at the older `afa9a071` and have
  since shifted to `mod.rs:2604-2617` at `7b7a6a7d`, which the entry's own
  "at afa9a97a07" qualifier accounts for; not a defect).
- **IN-5**: the column-wins-else-HPXML-else-default precedence in
  `crates/hares-io/src/schedule_resolve.rs:701-740`
  (`inject_setpoint_schedules`) and the unread
  `water_heater_setpoint` column mapping (`:196-199`, never consumed by
  `inject_water_heater_schedule_columns`) were confirmed in code.

## Scope note

Given the volume of this scope (four ledger files, roughly 6,500 lines,
most entries carrying five to fifteen file:line citations each), this
pass gave full-depth, line-by-line verification to OR-18, OR-19, and
OD-22 through OD-28 as instructed, spot-checked representative citations
in every other entry in scope (OD-1 through OD-21, IN-1 through IN-9, and
a sample of the OR entries not yet committed), and did not re-derive every
single citation in OR-1 through OR-17 or in `05-python-surface.md`'s
eleven PY entries. No further defect was found in the spot-checked
entries beyond the two reported above. No entry's dependency ordering
against `00-schedule.md` was found wrong except the OD-28/OD-2 conflict.
No numeric-change entry in scope had already landed, so there is nothing
to check against rule 2 of "no regressions, and no worse physics" yet.
