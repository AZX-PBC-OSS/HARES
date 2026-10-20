# L5-c2 resolution

Resolves every finding of `verify-L5-c2.md` against `ledger/11-other-defects.md`
(6 majors, 11 minors) and its two verification gaps. Every corrected
citation below was verified directly against `.workstrees/audit-7b7a6a7d`
(HEAD `7b7a6a7d`) by grep and direct line reads, except where a citation's
Evidence paragraph carries its own `(at <sha>)` pin, in which case it was
verified against that commit's tree with `git show <sha>:<path>`.

Counts: 6 majors resolved, 11 minors resolved, 2 gaps closed. 0 rejected.

## Majors

1. **OD-20's Problem narrative (unbounded coverage).** Confirmed
   `environment.rs:376-379`'s `set_pv_roof_coverage` clamps to `[0.0, 1.0]`
   before storing, so coverage above 1 cannot occur. Changed the third
   Problem bullet to: "The coverage fraction is silently clamped to
   `[0.0, 1.0]` (`environment.rs:376-379`, `set_pv_roof_coverage`) when the
   sum of attached PV collector areas exceeds the roof's area, so an
   over-subscribed roof is hidden instead of reported." Kept the Change and
   the `roof_coverage_above_one_is_an_assembly_error` acceptance test
   unchanged: replacing a silent clamp with a loud error is still the
   correct fix, and the test still fails at HEAD.

2. **OD-2 site 6's `set_typed_setpoint_source` citation.** Confirmed the
   function is at `schedule_resolve.rs:741-757` at HEAD, not `:695-713`.
   Changed the citation to `:741-757`.

3. **OD-6's "duplicated at" citation.** Confirmed `mod.rs:1756-1766` is
   unrelated struct-field declarations. The actual duplicate of
   `blueprint.rs:46-56`'s schedule-generation branch is
   `mod.rs:1937-1948` (`from_config`'s own `schedule_raw` if/else,
   byte-for-byte the same shape). Changed the citation to `:1937-1948`.

4. **OD-17's `load_default_profiles` citation.** Confirmed the function
   spans `schedule_resolve.rs:342-437` at HEAD (the `(at 21d90588)` pin had
   drifted ~85 lines after `b8195c83` removed a `check_invariants` block
   earlier in the file). Re-pinned the whole Evidence paragraph to
   `7b7a6a7d` and corrected every citation in it: `:342-437` with
   sub-ranges `:346-355` (empty map), `:366-368`/`:383-400` (skipped rows
   and values), `:406-407` (weekend/monthly substitution); the resolver's
   load at `:554` (was `:475`); the 0.0 kW fallback at `:996-1011` (was
   `:949-963`); the constant-power fallback at `:1028-1034` (was
   `:981-987`); `schedule.rs:286` (was `:262`, in both the Evidence list
   and the Problem bullet that also cited it); the `mod.rs` caller at
   `:1937-1941` (was `:1882`, which is also struct-field code, not a
   caller). The `hpxml_schedule.rs` sub-citations are unchanged between
   `21d90588` and `7b7a6a7d` (no diff to that file), so most needed no
   change; two did (see minor 5 below).

5. **OD-18's `record_clamped_actions` citation.** Confirmed the function
   is at `mod.rs:4826` at HEAD (`:4631` at the entry's pinned `21d90588`;
   `mod.rs` moved between the two commits). Since the rest of this
   Evidence paragraph's citations (`py_gym.rs`, `gym_env.py`,
   `vec_env.py`) drift by at most a few lines between the two commits and
   none of them was flagged, left the paragraph's `(at 21d90588)` pin in
   place and annotated this one citation with both: `mod.rs:4826` at
   `7b7a6a7d` (`4631` at the entry's pinned `21d90588`).

6. **OD-2 site 8's `blueprint.rs` citations.** Confirmed the out-of-range/
   non-finite offset handling is at `:96-97` and the
   `.single().unwrap_or_else(..)` conversion at `:103-104` at HEAD (both
   untagged, so checked against HEAD per the entry's own convention).
   Changed `:90-91` to `:96-97` and `:97-98` to `:103-104`.

## Minors

1. **OD-1's `pyproject.toml:77`.** Confirmed `module-name =
   "ochre_next._hares"` is at `:81`. Fixed in `11-other-defects.md`. The
   same stale number in `05-python-surface.md` PY-7 belongs to another
   resolver (see Scope note below); not edited here.

2. **OD-2 site 1's `heat_pump_config.rs` citations.** Confirmed the field/
   type-alias declarations are at `:263` (`common`), `:298` (`defrost`),
   `:484` (cooler's `common`), `:699` (the `HeatPumpConfig` alias, moved
   from `:700` after `b8195c83` deleted one `#[cfg(...)]` line earlier in
   the file). Changed `:262,297`/`:483`/`:700` to `:263,298`/`:484`/`:699`.

3. **OD-2 site 3's `conversions.rs` citations.** Confirmed in
   `apply_equipment_overrides`: the `"*"`-ignored-when-`"all"`-present
   branch (`root.get("all").or_else(|| root.get("*"))`) is at `:942`, and
   the dropped-non-object-override branch
   (`if let Some(Value::Object(eq)) = root.get(name)`) is at `:945`.
   Changed `:937` to `:942` and `:940-942` to `:945`.

4. **OD-8's `battery/mod.rs` citations.** Confirmed the default-Manual
   assignment is at `:522`, the declared-`bms_mode`-replaces-it block at
   `:1049-1056`, and `actor_seed()`'s Manual-returns-`None` at `:1626-1628`.
   Changed `:519`/`:1056-1064`/`:1640-1650` to `:522`/`:1049-1056`/
   `:1626-1628`.

5. **OD-9/OD-17's `hpxml_schedule.rs` citations.** `hpxml_schedule.rs` is
   byte-identical between `21d90588` and `7b7a6a7d`. Confirmed OD-17's
   constant-`occupants`-push citation is at `:52-54` (was `:50-52`) and the
   1.0-fallback for-loop at `:57-65` (was `:56-63`); `:31-35`, `:43`,
   `:16-17` and `:92-99` were already exact and left unchanged. OD-9's own
   `:66-73`/`:75-83` setpoint citations were not flagged and left as is.

6. **OD-10's `solver_builder.rs` structural citations.** Confirmed at this
   entry's pinned `21d90588`: the zone-id fallback (`.unwrap_or(hares_types
   ::ZoneId(zone_idx as u16))`) is at `:1423`, the flue default at `:1445`,
   and the floors default at `:1447`. Changed `:1417-1421`/`:1443`/`:1445`
   to `:1414-1423`/`:1445`/`:1447`.

7. **OD-10's boolean-parsing census.** Ran
   `git grep -n 'eq_ignore_ascii_case("true")' -- crates/hares-io/src` at
   the entry's pinned `21d90588`: twelve matches, not ten:
   `building.rs:596, 623, 1623, 2140, 2356, 2411` (six),
   `resolve_loads.rs:322, 490, 623, 989, 991` (five), and
   `resolve_water_heater.rs:478` (one). Changed the Evidence citation from
   "ten sites" to the exact twelve with their lines, changed the Change
   section's "the ten sites of the census use it" to "all twelve sites",
   and added a new acceptance test,
   `every_xsd_boolean_site_uses_parse_xsd_boolean`, a table test with one
   case per one of the twelve sites parsing `1`/`0` through its own parse
   path, so a site the fix misses fails this test instead of only the
   single flue site `xsd_boolean_accepts_one_and_zero` already pinned.

8. **OD-13's `ci.yml` citation.** Confirmed the `slow-python-test` job
   spans `:211-234` (the job's own `slow-python-test:` key line is `211`,
   not `212`). Changed `:212-234` to `:211-234`.

9. **OD-17's `schedule.rs:262`.** Confirmed the column/aggregation zip is
   at `:286` at HEAD (`schedule.rs` gained a 24-line function,
   `resolve_occupancy_column`, between `21d90588` and `7b7a6a7d`, shifting
   everything below it). Changed both occurrences (the Problem bullet and
   the Evidence list) from `:262` to `:286`.

10. **OD-18.** Resolved by major 5 above; no separate change.

11. **`synthetic.rs:138`'s ticket ID.** Confirmed `wall_area_m2`'s doc
    comment carries "T-0226". No new ledger entry: OD-2 site 9 already
    deletes this exact field and its comment, so the violation is
    remediated by the entry that already covers this code. No action
    taken, matching the verification's own conclusion.

## Gaps

- **OD-10's OpenStudio-HPXML infiltration-height reference value.**
  Fetched NREL's `OpenStudio-HPXML` `HPXMLtoOpenStudio/tests/test_defaults.rb`
  (`v1.12.0`): `test_infiltration_height_and_volume` asserts
  `assert_in_epsilon(9.75, ...infiltration_height, 0.01)` for the `base.xml`
  case with the height removed. Matches the ledger's "9.75 ft, 2.9718 m"
  exactly (9.75 ft × 0.3048 = 2.9718 m). Also fetched OpenStudio-HPXML's
  "Workflow Inputs" docs and confirmed the ledger's quoted default rules
  verbatim: the `InfiltrationHeight` "estimated from other inputs"
  sentence, the `HasFlueOrChimneyInConditionedSpace` true/false conditions,
  the `ShieldingofHome` normal/well-shielded-by-facility-type default and
  its three allowed values, and `SiteType`'s suburban default and three
  allowed values. Gap closed: no finding, the ledger's citations and
  quotes are accurate against the upstream source.

- **OD-21's tzf-rs data-release identifiers.** Searched and fetched
  `ringsaturn/tzf-dist`'s GitHub releases: `v0.0.2026-b-fix1` (published
  2026-05-03) and `v0.0.2026-d-fix1` (published 2026-09-23, latest) both
  exist, in that chronological order. Matches the ledger's claim that
  `01-baseline-repair.md` BR-6b moved the data release "from 2026-b-fix1
  to 2026-d-fix1". Gap closed: no finding.

## Scope note

PY-7's `pyproject.toml:77` citation in `05-python-surface.md` (the same
stale line number minor finding 1 flags) was left untouched; it belongs to
another resolver.
