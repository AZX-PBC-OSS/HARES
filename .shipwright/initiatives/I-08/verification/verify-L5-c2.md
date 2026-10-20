# Verification L5-c2

Scope: every entry in `ledger/11-other-defects.md`, read line by line (a
previous pass, `verify-L5-c.md`, only sampled OD-1 through OD-21 and read
OD-22 through OD-27 in full; this pass covers OD-1, OD-2 (all nine sites
plus site 6), OD-3, OD-6, OD-7, OD-8, OD-9, OD-10 (both commits), OD-11,
OD-12, OD-13, OD-14, OD-15, OD-17, OD-18, OD-19, OD-20 and OD-21). OD-4,
OD-5 and OD-16 do not exist as headers in this file (OD-16 is explicitly
contingent on OD-15's investigation and correctly absent until that
investigation lands). OD-22 through OD-27 are not re-covered here; the
prior pass's findings on them stand.

Checked against HEAD `7b7a6a7d` in the clean worktree
`.workstrees/audit-7b7a6a7d`, with every citation tagged "at `<sha>`"
checked against that commit's tree with `git show <sha>:<path>`, and
untagged citations checked against current HEAD. Cross-checked against
`plan.md` §3, `00-schedule.md`'s row order and dependency text,
`ledger/pytest-skip-census.txt`, `pyproject.toml`, `.github/workflows/ci.yml`,
and `.gitmodules`. None of OD-1 through OD-21 is committed, so no
acceptance test or verification command could be executed; every Problem
claim was checked by reading the cited code directly, and every
verification command was checked for valid flags, crate names and markers
against the installed toolchain (cargo 1.94.1, cargo-nextest 0.9.137, uv
0.12.8, pytest 9.1.1) and the workspace's actual crate list.

Counts: 17 findings total. 0 blocker. 6 major. 11 minor.

## Findings

### Major: OD-20's Problem narrative misdescribes its own cited evidence (coverage is already clamped, not unbounded)

**Location:** `ledger/11-other-defects.md`, OD-20, Problem, third bullet:
"The coverage fraction is never bounded, so arrays whose collector area
exceeds the roof's give a coverage above 1 and a negative exposed
fraction."

**Evidence:** at the entry's own cited commit `afa9a071`,
`crates/hares-core/src/environment.rs:376-379`
(`set_pv_roof_coverage`) already clamps the fraction to `[0.0, 1.0]`
before storing it, so coverage above 1 and a negative exposed fraction
cannot occur today. The real defect at that site is a silent clamp (still
a violation of "no silent defaults, error loudly"), not an unbounded
value.

**Why it matters:** the Problem section is the thing an implementer reads
to understand what is broken; as written it asserts a reproducible
failure mode (coverage above 1) that cannot happen, which would send an
implementer looking for a bug that is not there. The proposed Change
(error when a roof's summed coverage exceeds 1) is still the correct
long-term fix: it replaces a silent clamp with a loud error, which is
strictly better, so only the narrative needs correcting, not the fix.

**Correction:** rewrite the third bullet to state the actual defect: a
roof's coverage is silently clamped to `[0.0, 1.0]` when the sum of
attached PV collector areas exceeds the roof's area, hiding an
over-subscribed roof instead of reporting it. The Change section's
"roof_coverage_above_one_is_an_assembly_error" acceptance test is still
correct and still fails at HEAD, since HEAD clamps instead of erroring.

### Major: OD-2 site 6 cites the wrong function and lines for `set_typed_setpoint_source`

**Location:** `ledger/11-other-defects.md`, OD-2, Problem, site 6:
"`set_typed_setpoint_source` returns silently when the spec has no typed
object payload or the source cannot be serialized
(`crates/hares-io/src/schedule_resolve.rs:695-713`)."

**Evidence:** at HEAD `7b7a6a7d`, `schedule_resolve.rs:695-713` is not
`set_typed_setpoint_source`; that function is actually at `:741-757`. The
behavior described (returning silently on a missing typed payload or a
serialization failure) is real at that location.

**Correction:** change the citation to `schedule_resolve.rs:741-757`.

### Major: OD-6's "duplicated at" citation points at unrelated code

**Location:** `ledger/11-other-defects.md`, OD-6, Problem: "when
`schedule_path` does not exist, construction generates a schedule from
the HPXML instead (`crates/hares-core/src/dwelling/blueprint.rs:46-56`,
duplicated at `crates/hares-core/src/dwelling/mod.rs:1756-1766`)."

**Evidence:** at HEAD `7b7a6a7d`, `mod.rs:1756-1766` is unrelated
struct-field declarations, not a duplicate of the schedule-generation
branch. Two independent checks place the real duplicate at
approximately `mod.rs:1937-1948` (one check cites `:1937`, another
`:1937-1945`); a third independent check of the same claim in OD-9's
evidence section, which cites the same duplicated branch via
`mod.rs:1882`, is consistent with the duplicate sitting in that
neighborhood (high 1800s to high 1900s), not at 1756.

**Why it matters:** an implementer migrating `schedule_path` to
`Option<PathBuf>` needs to find and fix both call sites; the wrong
citation sends them to struct-field code with nothing to change there.

**Correction:** change the citation to the actual duplicate branch,
confirmed to sit between `mod.rs:1882` and `mod.rs:1948` (re-locate
precisely before the commit; three independent reads converged on
`~1937-1948`, not `1756-1766`).

### Major: OD-17's `load_default_profiles` citation is off by roughly 85 lines

**Location:** `ledger/11-other-defects.md`, OD-17, Evidence:
"`crates/hares-io/src/schedule_resolve.rs:257-362` (`load_default_profiles`:
the empty map at `:261-271`, the skipped rows and values at `:282-316`,
the weekend and monthly substitutions at `:318-330`)."

**Evidence:** at commit `21d90588`, `load_default_profiles` actually
spans `:342-437`, with the empty-map return at `:346-355`, the skipped
rows and values across `:366-368` and `:383-400`, and the weekend/monthly
substitution at `:406-407`. The behavior at each sub-location matches the
entry's description; only the line numbers are wrong, by a consistent
~85-line offset.

**Correction:** change the citation to `schedule_resolve.rs:342-437`
with sub-ranges `:346-355`, `:366-368`/`:383-400`, `:406-407`.

### Major: OD-18's `record_clamped_actions` citation is off by roughly 195 lines

**Location:** `ledger/11-other-defects.md`, OD-18, Evidence:
"`crates/hares-core/src/dwelling/mod.rs:4631-4634`
(`record_clamped_actions`)."

**Evidence:** at commit `21d90588`, `record_clamped_actions` is actually
at `mod.rs:4826`, not `:4631-4634`. Two independent checks confirm this
same offset and both independently confirm the behavioral claim itself
(the function exists, and `git grep -n record_clamped_actions -- crates
python` finds only its own definition, no caller) is accurate.

**Correction:** change the citation to `mod.rs:4826`.

### Major: OD-2 site 8's two `blueprint.rs` citations are both off by 6-7 lines

**Location:** `ledger/11-other-defects.md`, OD-2, Problem, site 8: "An
offset outside ±24 h becomes UTC, and a non-finite offset rounds to 0 and
also becomes UTC (`crates/hares-core/src/dwelling/blueprint.rs:90-91`).
Change: ... The `.single().unwrap_or_else(..)` at `:97-98`."

**Evidence:** at HEAD `7b7a6a7d`, the described out-of-range/non-finite
offset handling and the `.single().unwrap_or_else(..)` conversion sit at
`blueprint.rs:96-97` and `:103-104` respectively, not `:90-91`/`:97-98`.
Confirmed independently three times with the same corrected lines.

**Correction:** change the citations to `blueprint.rs:96-97` and
`:103-104`.

## Minor findings (citation drift; behavior and fix confirmed correct)

1. **OD-1** cites `pyproject.toml:77` for the extension module name
   (`module-name = "ochre_next._hares"`); the actual line is `:81`.
   Confirmed independently four times. The same stale line number is
   independently repeated in `05-python-surface.md` PY-7, so the drift
   should be corrected in both places together.
2. **OD-2 site 1**'s four `heat_pump_config.rs` citations (`:262`,
   `:297`, `:483`, `:700`) are each one line short of the real
   field/type-alias declarations (`:263`, `:298`, `:484`, `:699`).
3. **OD-2 site 3** cites `conversions.rs:937` ("'\*' is ignored when
   'all' is present") and `:940-942` (the dropped non-object override);
   the real lines are approximately `:942` and `:945`, 3-8 lines off, in
   the same function.
4. **OD-8**'s three `battery/mod.rs` citations (`:519`, `:1056-1064`,
   `:1640-1650`) drift by 3 to 24 lines from their actual positions; all
   three symbols and the described defaulting/actor-seeding behavior are
   confirmed real at the drifted locations.
5. **OD-9**/**OD-17**'s shared `hpxml_schedule.rs` citations around the
   duplicate `occupants` column (`:50-52`, `:56-63`) are off by 1-2
   lines from the real pushes; the duplicate-column defect itself is
   confirmed real, and the `Occupancy`/constant-1.0 double-push and the
   mismatched column/index/aggregation lengths are accurately described.
6. **OD-10**'s structural-commit citations in `solver_builder.rs`
   (`:1417-1421` for the zone-id fallback, `:1443` for flue, `:1445` for
   floors) are each off by 2-6 lines; the real lines are approximately
   `:1414-1423`, `:1445`, and `:1447`.
7. **OD-10**'s boolean-parsing census claims "ten sites" using
   `eq_ignore_ascii_case("true")` across `building.rs`,
   `resolve_loads.rs` and `resolve_water_heater.rs`. Running the exact
   cited `git grep` command at `21d90588` returns twelve matches
   (6 in `building.rs`, 5 in `resolve_loads.rs`, 1 in
   `resolve_water_heater.rs`), not ten. This was found independently
   twice. The acceptance test (`xsd_boolean_accepts_one_and_zero`) only
   pins one site, so the miscount does not block the entry, but an
   implementer following the stated count could migrate two fewer call
   sites than the fix actually requires to `parse_xsd_boolean`.
8. **OD-13**'s `.github/workflows/ci.yml:212-234` citation for the
   missing submodule-init step in `slow-python-test` is off by
   approximately one line; the behavioral claim (no `git submodule
   update --init vendors/OCHRE` step in that job) is confirmed accurate.
9. **OD-17** cites `crates/hares-io/src/schedule.rs:262` for the column/
   aggregation zip that a length mismatch would break; the actual line
   is `:286`. The described risk (a downsample dropping the last column
   while the index still points at it) is confirmed real at `:286`.
10. **OD-18**: see the major finding above for the `record_clamped_actions`
    citation; no further minor issues on this entry.
11. **Not a new entry, an observation:** `synthetic.rs:138`'s
    `wall_area_m2` doc comment, cited by OD-2 site 9, carries a ticket ID
    ("T-0226"), which violates the project's rule against ticket
    references in code comments. No separate ledger entry is needed:
    OD-2 site 9 already deletes this exact field and its comment as part
    of its planned fix, so the violation is remediated by the entry that
    already covers this code.

## Verified correct (no finding)

- **OD-1**: the pyo3 `create_exception!` module-mismatch (registering
  under `_hares` while the built extension is `ochre_next._hares`,
  confirmed against the corrected `pyproject.toml:81`) is real; the fix
  is correctly gated on the `05-python-surface.md` package-rename entry;
  schedule ordering (row 28, after the dependency it needs) is
  consistent.
- **OD-2**: all nine Problem sites were checked against code at
  `21d90588` and confirmed real beyond the citation drift noted above:
  the `flatten` + `deny_unknown_fields` conflict in
  `heat_pump_config.rs`, `SimulationConfig`'s missing
  `deny_unknown_fields` and the silent `time_res_secs_u32` 3600 s
  fallback, the three silent override-drop sites in `conversions.rs`,
  the `.ok().flatten()`/`.ok()?` chain of silent `None`s through the HVAC
  typed-config rebuild and its builders, the water-heater UA/efficiency
  shadowing, the thermostat setpoint-source precedence that makes an
  override inert, the unchecked EV charging-power unit, the UTC-offset
  fallback to 0, and the dead `wall_area_m2` field. Site 6 is correctly
  flagged as its own numeric commit with its own schedule row (94e) and
  a recorded delta, matching `09-numeric-fixes.md`'s rule. Dependencies
  (M-2; before PY-8, RT-4, IN-5/IN-3, CN-1) all resolve to real entries,
  and `00-schedule.md` row ordering (23 < 31 < 32 < ... < 94e) is
  internally consistent. OD-8 (row 32) correctly depends on OD-2 (row
  31).
- **OD-3**: every citation checked exact at `21d90588`; the boiler
  `unwrap_or_default` loop-id race (a boiler pending autosizing skipped
  by the pre-autosize wiring pass, then rebuilt with `loop_id: None`) is
  confirmed in `boiler.rs`, `resolve_hvac.rs` and `loop_allocator.rs`. No
  dependency; lands before OD-2 site 4 as stated (row 23 < row 31).
- **OD-6**: the duplicated silent-fallback-to-generated-schedule behavior
  is confirmed real (citation drift aside, see major finding above);
  `Option<PathBuf>` is the correct fix; depends on nothing, lands before
  RT-4 as stated.
- **OD-7**: `_coerce_seconds`'s fallback to 60 s, the untyped mapping
  path, the truthiness tests on `start_time`/`duration_s`/`time_res_s`/
  `master_seed`, and `_sim_config_to_kwargs`'s field-by-field forwarding
  gap are all confirmed in `gym_env.py` at current HEAD. The claim that
  the Python `SimulationConfig` has "17 fields" is slightly off (it
  actually has 18 at HEAD); this does not affect the correctness of the
  fix (deleting `_sim_config_to_kwargs` entirely and forwarding the
  config object unchanged), so it is noted but not raised as a
  standalone finding. Depends on the `05-python-surface.md` package-rename
  entry as stated.
- **OD-8**: all six cited HPXML fixtures confirmed to declare an
  uncontrolled stationary `<Battery>` with no `bms_mode` or
  `BatteryManagement` actor; the Manual-becomes-inert default chain in
  `battery/mod.rs` is confirmed (citation drift aside, see minor finding
  4 above). Depends on OD-2 (misspelled `bms_mode` already an error);
  row 32 after row 31 is consistent.
- **OD-9**: the two literal setpoint defaults (20.0 / 24.0 °C) and the
  single-value flattening of the HPXML weekday profile in
  `hpxml_schedule.rs` are confirmed real. Correctly flagged numeric with
  its own row (94a) and dependencies on IN-5, OD-6, OD-17 and OD-19, all
  of which land before it in the schedule (24, 24f, 24g all precede
  94a).
- **OD-10**: both the structural and numeric commits' Problem claims are
  confirmed real beyond the citation drift noted above: the 400 m³
  volume default, the floors-above-grade 1.0 default, the infiltration
  height ceiling-height×floors estimate, the flue/shielding/site-type
  string-match defaults, and the literal-`"true"`-only boolean parser are
  all present as described. The OpenStudio-HPXML documented-defaults
  quotes could not be independently verified against the upstream
  repository (no vendored OpenStudio-HPXML source tree exists in this
  repo to cross-check the exact wording or the 9.75 ft / 2.9718 m
  reference value); this is reported as a verification gap, not a
  finding, since nothing in the ledger's own text is internally
  contradictory. Two commits in two schedule rows (24a structural, 94f
  numeric) with the numeric commit's dependency on RT-6 and the consumer
  block is consistent with row ordering.
- **OD-11**: `VENDOR_OCHRE` is confirmed referenced but never assigned in
  `test_thermal_trace.py` at `8b09ac4a`, causing a real `NameError`; all
  three `xfail` marks are confirmed `strict=False` with no `raises=`.
  Depends on nothing beyond M-1/M-2; lands before PY-7 as stated (row
  24b before PY-7's row).
- **OD-12**: `generate_pvlib_solar_override.py`'s `main()` is confirmed
  to take no arguments at `8b09ac4a`, write unconditionally to a fixed
  `OUTPUT_DIR`, and import third-party packages and mutate `sys.path`
  before any argument handling exists. `[pytest]` and `[stray]` are
  confirmed defined as verification shorthand in `plan.md`.
- **OD-13**: the three `pytest.importorskip("ochre")` module guards, the
  `ochre` dependency group installing OCHRE's dependencies but not OCHRE
  itself (which is the `vendors/OCHRE` submodule), the five assert-free
  diagnostic tests, and the two never-failing OCHRE-parity tests are all
  confirmed. Depends on OD-11, OD-12; lands before PY-7 and OD-14 (rows
  24b, 24c before 24d before 24e, consistent).
- **OD-14**: the pint cap `>=0.24.4,<0.25` with its comment, and the
  `inch_H2O_39F`/`inch_H2O_4C` unit-rename claim against
  `vendors/OCHRE/ochre/utils/units.py` and `envelope.py`, are confirmed.
  `uv lock --upgrade-package pint` is a valid subcommand. Depends on
  OD-13 and BR-6c; lands before PY-7 (row 24e, consistent).
- **OD-15**: correctly structured as a contingent investigation (not a
  direct fix): `CrankcaseHeaterPowerWatts` 50.0 on the cited fixture and
  the matching `cooling_config.rs` fields are confirmed; the 3.45 kWh /
  72 h = 48 W arithmetic checks out by hand. No premature `OD-16` header
  exists in this file, consistent with the entry's own contingency text
  and with `00-schedule.md`'s contingency note. Depends on BR-12; row
  16h is correctly scheduled early (before the bulk of this file's other
  rows), consistent with it being a prerequisite for the seasonal test
  suite other numeric entries build on.
- **OD-17**: the duplicate `occupants` column push and the genuinely
  missing `month_multipliers` row for the `Ceiling Fan` profile in
  `defaults/Default Schedule Parameters.csv` were both independently
  confirmed by direct file inspection, beyond the citation drift noted
  above. `observe` is confirmed a real cargo feature is not applicable
  here (that check belongs to OD-18); `hares-io`/`hares-core` exist as
  crates. Depends on OD-6; lands before RT-4 and OD-9 as stated.
- **OD-18**: the two process-global statics `ACTIONS_CLIPPED` /
  `ACTIONS_CLIPPED_WARNED`, confirmed zero-caller
  `record_clamped_actions` (via `git grep`), and the separate
  `np.clip`/`_field_bounds` path in `gym_env.py`/`vec_env.py` are all
  confirmed real beyond the citation drift noted above. `observe` is
  confirmed a real feature of `hares-python`
  (`cargo nextest run -p hares-python -F observe` is valid). Depends on
  OR-5 (confirmed already committed as `dc033cee`/`469ea955` in git log)
  and PY-7/PY-5; lands before OR-12 and RT-5 as stated.
- **OD-19**: the token-filtering, 24-value-count, and constant-fallback
  behavior of `parse_setpoint_from_control` in `xml_helpers.rs` is
  confirmed exact at current HEAD; `git grep -n 'Setback\|Setup' --
  crates/hares-io/src` is confirmed empty, matching the claim that no
  setback/setup handling exists anywhere in the parser. No dependency;
  lands before OD-9 and IN-5 as stated.
- **OD-20**: the untyped-JSON tilt/azimuth/area reads with their
  `unwrap_or` defaults in `register_pv_surfaces`/`attach_pv_to_roofs`/
  `register_pv_roof_shading`, and the `base-pv.xml` fixture geometry
  (roof area, PV tilt/azimuth/capacity values) cited in the acceptance
  tests, are all confirmed exact against the fixture file and against
  `afa9a071`. Depends on B-14 and RT-6, M-1, M-2; row 94i correctly
  follows B-14's row 94h in the numeric block.
- **OD-21**: the `tag.attributes().flatten()` attribute-dropping and
  `normalized_value(..).unwrap_or_default()` sites in `building.rs`, and
  the caller/HPXML/weather/lookup/longitude-estimate precedence chain in
  `site_location.rs`, are confirmed exact at `afa9a071`. Depends on BR-6b;
  lands directly after OD-19 at row 24h as stated.
- No em dashes, LLM tics ("Let me", "Note:", etc.), references to agents
  or sessions, or archaeology (references to how the ledger used to be
  structured) were found anywhere in OD-1 through OD-21's prose.
- Verification commands named across all 18 entries (`cargo nextest run`
  with the package names used, `uv run pytest` with the `-k` filters
  named, `frame_golden compare-all`, `uv lock --upgrade-package pint`,
  `uv run --all-groups`/`--all-extras --no-sync`) all use real crate
  names, real cargo features, and real `uv`/pytest flags against the
  installed toolchain (cargo 1.94.1, cargo-nextest 0.9.137, uv 0.12.8,
  pytest 9.1.1) and this workspace's actual crate and marker
  definitions.

## Additional defects in touched code not covered by any entry

None found beyond the `synthetic.rs` ticket-ID observation noted above,
which is already remediated by OD-2 site 9's planned deletion and needs
no new entry.

## Verification gaps

- None of OD-1 through OD-21 is committed, so no acceptance test and no
  verification command listed in any entry could actually be run; every
  check in this report is a citation-against-code read, a schedule
  cross-check, or a tool-flag/crate-name validity check, matching the
  method the ledger itself uses for an uncommitted entry.
- OD-10's and OD-21's external-documentation citations (OpenStudio-HPXML's
  documented infiltration-height, flue and shielding default rules and
  its `test_defaults.rb` reference value of 9.75 ft / 2.9718 m; tzf-rs's
  exact data-release identifiers) could not be independently checked
  against the upstream sources; no vendored OpenStudio-HPXML tree exists
  in this repository to cross-check against, and no network access was
  available. Nothing in the ledger's own text contradicts itself on these
  points, so this is reported as a gap, not a finding.
- OD-2's nine sites, OD-8's six fixtures, and OD-10's 67-file HPXML
  census were spot-checked against their most load-bearing claims rather
  than re-counted exhaustively line by line, except where a specific
  `git grep` census (the boolean-parsing site count) was run in full and
  found to disagree with the ledger's stated count (minor finding 7
  above).
- Six independent passes covered this scope with overlapping entry
  groupings to cross-validate citation checks; where two or more passes
  independently converged on the same corrected line number, that
  citation's correction is reported with higher confidence than
  single-pass findings.
