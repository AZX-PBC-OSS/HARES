# Audit of OR-18, commit 1c18543a

Auditor: independent (did not write the commit). Worktree
`.workstrees/audit-1c18543a` at 1c18543a. A disposable second worktree was
used for the isolation proof and removed with `git worktree remove --force`.
No repository code was edited.

## Counts by severity

| Severity | Count |
|---|---|
| Blocker | 0 |
| Major | 4 |
| Minor | 9 |

Verdict: the numeric delta and the physics are right, and every golden is
bitwise as claimed. The commit is not clean: the "no ZoneId(1) fallback"
requirement is met only by renaming the helper, one numeric change is
unrecorded, the four-commit split was not kept, and a public doc points at a
gitignored planning file.

## Findings

### Major

**M1. The silent ZoneId(1) fallback was moved inline, not removed.**
`zone_id_from_config_or_default` is deleted, but its body was pasted at 18
call sites as `zone_from_config.unwrap_or(ZoneId(1))`:
`hvac/air_conditioner.rs:517`, `baseboard.rs:75`, `boiler.rs:169,553`,
`furnace.rs:114,525`, `heat_pump/cooler.rs:63,364,794`,
`heat_pump/heater.rs:478`, `ideal_hvac.rs:160`, and all five water heaters
(`gas.rs:157`, `heat_pump_wh.rs:223`, `indirect_tank.rs:116`,
`resistance.rs:151`, `tankless.rs:107`). Evidence:
`git grep -n "unwrap_or(ZoneId(1))" -- crates/*/src`. Consequences:
1. The OR-11 debug `warn` for these twelve-plus equipment types was deleted
   with no replacement, so a missing `zone_id` is now silent in every
   profile (an observability regression, and the opposite of OR-11's intent).
2. A water heater with neither `zone_id` nor `zone_type` (manual or TOML
   configs): `resolve_ambient_location` (`water_heater/mod.rs:58-62`)
   returns `Ok(None)` for `zone_type == None`, and the constructor's
   ZoneId(1) stands. The entry required an unconditional typed error when no
   placement resolves; only the "location string present but unclassified"
   case errors.
3. The 3-line idiom is copied 18 times (DRY).
Required fix: one shared helper returning `Option<ZoneId>`; water heaters and
IdealHvac error in `init` when no zone and no ambient placement resolve; the
other HVAC types either error likewise or the entry must record that they are
out of scope. Dispositions for the HVAC types need the operator's call; the
water heater and IdealHvac cases are in scope now.

**M2. Unrecorded numeric change to HPWH `wall_heat_fraction`.**
`heat_pump_wh.rs:705-713`. Old: default 0.5 only when `zone_type ==
"conditioned"`, where the string was `zone_key(parse_zone_label(Location))`.
New: `hares_types::is_conditioned_location(raw Location)`. For a Location of
"basement - conditioned" (parse_zone_label gives Foundation), "garage -
conditioned" (Garage), "kitchen", "indoor", "finished basement" the default
moves 0.0 to 0.5, changing HPWH sensible gain split in those homes. It is
physically defensible, but it is a second numeric change with no recorded
delta, no test, not isolated, and it breaks plan §3. No golden or parity
fixture has an HPWH in such a location (compare-all identical), so nothing
caught it. Required fix: either keep the old classification (test the
parsed zone type, not the raw string) or land it as its own numeric row with
a test and a recorded delta.

**M3. Four specified commits landed as one; the numeric row is not isolated.**
`1c18543a` holds the dehumidifier, water-heater, synthetic-closure and
IdealHvac changes (89 files). Plan §3 requires the numeric change isolated.
Mitigated by the proof below (structural parts are bitwise neutral), but the
acceptance tests were not shown failing against 7b7a6a7d per part, and a
bisect cannot separate the parts. Required: record this as an accepted
deviation, or split. Not remediable by rebase without operator approval.

**M4. Public repo doc points at the gitignored planning tree.**
`docs/python.md:136-147` adds a "Numeric changes" section citing
`.shipwright/initiatives/I-08/ledger/02-measurement.md`. `.shipwright` is
gitignored (`.gitignore:63`), so the link dangles for every reader, and the
section is initiative bookkeeping in a Python API guide (the "no planning
artifacts in public docs" rule). Required fix: delete the section; put any
user-facing note in release notes.

### Minor

**m1. `cargo clippy --workspace --all-targets --release -- -D warnings`
fails.** `hares-envelope/src/thermal_solver/mod.rs:1858`: unused import
`BoundaryDiagnosticInfo` in the test module under the release profile. The
line is not touched by this commit's diff (the commit only adds
`ambient_other_space_c` lines there), so it is pre-existing; I did not build
the parent to confirm. Dev clippy is clean. Fix: remove or gate the import.

**m2. Ledger accuracy.** The entry's premise correction and
`00-schedule.md` row 21b say three fleet homes carry "other ..." locations
(bldg0000003, 0000004, 0000006). `fleet_mixed_offset.toml` holds bldg 2, 4
and 176227; only bldg 4 has an "other" location (see the homes 0/2 section).
bldg0000003 and bldg0000006 are not in any golden (bldg0000006 is read by
`equipment_id_regressions.rs:236`). The recorded delta is right; the premise
text is wrong. Fix the ledger text in place.

**m3. Required per-step measurement is absent.** The entry's Measurement
field (cost of the three scalar computations via the hot-path allocation
counter) has no record in `02-measurement.md`. Code review shows no
allocation in `environment.rs:664-698` (one iterator `find` over the zone
list and four scalar stores), so this is a recording gap, not a defect.

**m4. DRY.** Five water heaters each carry the same `ambient_location`
field, the same `ambient_temp_c` match, and the same 14-line init block
(`gas.rs:286-300,316-333`, `resistance.rs:287-300,322-336`, plus indirect,
HPWH, tankless). Move the placement into one small struct held by each
heater. `ambient_location_from_zone_type` (`wh_config.rs:47-50`) is a pure
forwarder to `AmbientLocation::from_zone_type`, used only by tests.
`resolve_loads.rs:1084-1086` keeps a one-line forwarder to the moved
`is_conditioned_location` and its test block (`:1274-1291`) still tests it
there; call `hares_types` directly and move the test. `tankless.rs:98,229`
stores `ambient_location` that production code never reads.

**m5. Silent substitutions in the new code.** `water_heater/mod.rs:35-42`
(`OtherHousingUnit`) falls back to outdoor temperature when
`conditioned_zone_c` is non-finite, hiding a NaN zone state the invariants
should catch. `environment.rs:679-685` substitutes outdoor temperature as the
indoor share when the building has no conditioned zone. Both contradict the
no-silent-defaults rule; return a typed error (the environment case is
checkable at manager construction).

**m6. End use now lags the FSM at the routing points that read it.** The
assignment moved from `update_control` into `step` as specified, but
`dwelling/mod.rs:1402` (`ByEndUse` dispatch) and `:1473` (zone grouping for
the thermal consistency check) read `descriptor().end_use` before `step`.
The removed comment gave exactly that reason. A unit whose first mode is
cooling is routed as `HVAC_HEATING` (the constructor default,
`ideal_hvac.rs:164`) until its first step completes. Goldens are unaffected,
and the spec mandated the placement, but there is no test for the
`ByEndUse(HVAC_COOLING)` first-step case. Add one or record the limitation.

**m7. `skin_loss` telemetry for ambient-placed heaters.** `gas.rs:~620`
publishes `SKIN_LOSS_W = skin_loss_to_zone_w` (including pilot heat to
ambient) though no thermal contribution is accumulated when `zone` is
`None`. The value is the tank's loss, which is correct, but the name says
"to zone". No test asserts that an ambient-placed heater emits no
`ThermalAccumulator` contribution or that the delta equals the removed skin
loss (see Item 3).

**m8. Dehumidifier port construction** (`dehumidifier.rs:195-204`) rebuilds
`PortDeclaration` by struct-update from `electrical()` to get a zone-less
Thermal/Humidity port instead of a typed constructor. Equivalent today, but
fragile if `PortDeclaration` grows a field.

**m9. Golden provenance.** The re-captured
`fleet_mixed_offset.golden.json` records `git_head` 69950a98 and
`git_dirty: true`. It was captured from a dirty tree on the parent, not the
landing commit. Values verified bitwise (below); only the provenance stamp is
weak.

## Deviation list

1. One commit instead of four (M3).
2. ZoneId(1) fallback retained inline at 18 sites; the "every fallback gone"
   requirement is met in name only (M1).
3. No typed error for a water heater with no zone_id and no zone_type (M1).
4. HPWH `wall_heat_fraction` default reclassified via a function moved to
   `hares_types` (M2); not in the entry.
5. `AmbientOtherSpaceTemps` carries a fourth field, `conditioned_zone_c`,
   beyond the three in the entry (found in review; reasonable, resolves
   "other housing unit" by zone type).
6. `ambient_location_from_zone_type` and `AmbientLocation::from_zone_type`
   both exist (entry specified the free function only).
7. `conditioned_zone_id` made `pub(super)` and `NoConditionedZone` added
   as specified; `PortDeclaration` hand-built in the dehumidifier (m8).
8. Tankless: only `descriptor.zone` is scoped (the unit has no thermal
   port); the stored placement is unused (m4).
9. `docs/python.md` section added (M4); not in the entry.
10. Per-step cost measurement not recorded (m3).
11. 4.44 C floor for "other non-freezing space" is the entry's truncated
    value; OS-HPXML's 40 F is 4.4444 C (0.004 K, immaterial, matches the
    entry's table).
12. Test-fixture edits beyond the six: `equipment_identity_config.rs`,
    `lifecycle.rs`, `water_heater_parity.rs`, `equipment_id_regressions.rs`
    gained `zone_id: 1` for dehumidifier/HPWH configs (required by the new
    init error; acceptable).

Confirmed as implemented per spec: dehumidifier zone through
`conditioned_zone_id` with `NoConditionedZone` and the `init` error
(`resolve_loads.rs:645-657`, `dehumidifier.rs:552-565`); raw HPXML Location
carried in `zone_type` (`resolve_water_heater.rs:80-86`); five-way
`AmbientLocation` with weights and floors matching
`geometry.rb` v1.12.0 `get_temperature_scheduled_space_values` (WebFetched:
other heated 0.5/0.5 floor 68 F; buffer 0.5/0.5 floor 50 F; non-freezing
0/1 floor 40 F; housing unit 1/0 none; `OtherExterior`, `Outside`, `RoofDeck`
return no space or schedule); series computed once per step in
`update_in_place` (`environment.rs:664-698`, no allocation); unclassified
location is a typed `HaresError::Equipment` naming heater and location;
synthetic closure is an unconditional `HaresError::Dwelling` (`synthetic.rs:
1368-1394`) with the six fixtures given a Floor (the one assertion that
inspected area was updated 10 to 20); IdealHvac end use from capacity sign
with deadband preserved and the debug log deleted (`ideal_hvac.rs:674-688`).
No ticket IDs, em dashes or LLM tics in added lines.

## Item 2: golden isolation and bitwise proof

Committed goldens at 1c18543a, run in the audit worktree:
- `cargo run --release -p frame_golden -- compare-all`: 5/5 identical
- `-F dst`: 1/1 identical
- `-F profiling`: 5/5 identical

Golden file diff against 69950a98 (`jq` per-home): fleet home 0 metrics hash
equal, home 2 equal, home 1 differs. Every other golden file is unchanged
between the two commits.

Isolation experiment: disposable worktree at 1c18543a with one edit,
`water_heater/mod.rs:67` `Some(location) => Ok(Some(location))` changed to
`Some(_) => Ok(None)` (water heaters in "other ..." locations back on
descriptor zone ZoneId(1), the old behaviour), and the golden directory
restored to 69950a98's files. `compare-all` (release, default features):
**5/5 identical, including fleet_mixed_offset.** So the dehumidifier change,
the closure check, the IdealHvac end-use move, the raw `zone_type`
carry-through, the new environment field and the equipment plumbing are
bitwise neutral together; the water-heater ambient path is the only source of
the fleet delta. (Not separately run: `-F dst` and `-F profiling` on the
isolated tree; the committed-tree runs of both are identical, and the edit
cannot touch fixtures without an "other" location.)

### Homes 0 and 2 (coordinator's addition)

`tests/fixtures/golden/fleet_mixed_offset.toml` holds bldg 2, 4, 176227.
- Home 0, bldg0000002: one WaterHeatingSystem, natural gas storage,
  Location "conditioned space". Resolves to the conditioned zone id; the
  location string maps to no `AmbientLocation`, so the path is unchanged.
  Its raw `zone_type` is now "conditioned space" instead of "conditioned",
  read only by diagnostics (`dwelling/mod.rs:3092`) and, for HPWH only,
  `wall_heat_fraction` (not an HPWH). Water-heating electricity is 0.0 kWh.
- Home 2, bldg0176227: gas storage, Location "garage". Resolves to the
  garage zone id (an unresolved id would now fail init; it ran), unchanged.
  The same home is the `consumer_shape_900s` golden, also identical.
- Neither home has any "other ..." Location on any equipment (the other
  `<Location>` hits are wall/surface and PV `roof` entries). So no "other"
  location is left on the old behaviour in any golden. The schedule note
  "three fleet homes" is wrong (m2): bldg0000003, bldg0000006 are not in the
  fleet.
- Across all 20 ResStock fixtures and the parity fixtures, "other ..."
  water-heater locations occur in bldg0000003, 0000004, 0000006, 0019694,
  0101439, 0527060 only; only 0000004 is in a golden. The rest run through
  the new path in tests (all pass) with no golden.

## Item 3: the delta is physically right

Home 1 (bldg0000004): electric 30-gal storage, EF 0.92, Location "other
heated space", ResStock week at about -12 C outdoor (G4803030 weather).
Ambient is now max(0.5 Tz + 0.5 Tout, 20 C) = 20 C (the floor binds), not the
zone temperature. Direction: the heater's standby loss rises slightly
(ambient 20 C below a zone near 21 to 22 C): water heating +0.217 kWh over
the week, about +1.3 W average. Tank heat no longer enters the conditioned
zone (zone is `None`, so no `JacketLoss` contribution is accumulated;
`resistance.rs:713-722`), so the heat pump must supply it: HVAC heating
+2.704 kWh, about +16 W electric average, about +40 to 50 W thermal at a
winter COP near 2.5 to 3, against a standing loss of roughly 40 to 75 W
(UA 1.3 to 2.5 W/K times 30 K). Gross +2.92 kWh, peak import down 1.7 W.
Magnitude and sign are consistent (assumptions: UA and COP from typical
values, not read from the run; this is an order-of-magnitude check, not a
reproduction, since ad-hoc scripts were not run). The recorded numbers in
`02-measurement.md` match the golden file exactly (gross 201.989 to 204.911,
water heating 25.109 to 25.326, heating 87.253 to 89.957, peak 5.6056 to
5.6039).

Energy accounting: the tank's losses (skin loss, pilot heat to ambient, HPWH
evaporator draw) now leave through the ambient source, outside every modelled
zone, as OS-HPXML does for these locations. The tank's own balance is
unchanged (ambient is its boundary). The dwelling has no whole-home DHW heat
balance check that this could break, and invariants passed across the runs.
Gap (m7): no test asserts that an ambient-placed heater adds no zone
contribution or that the home-level delta equals the removed skin loss.

## Item 4: verification (exact)

| Check | Result |
|---|---|
| `uv sync --extra ochre` then `bash scripts/check_ashrae_reference.sh` | PASS (exit 0) |
| `cargo nextest run -p hares-core --release --test bestest --test parity --test envelope_oracle --test structural_envelope_oracle --test conditioned_oracle` | 25 run, 25 passed, 0 skipped |
| `cargo test --workspace --no-fail-fast` (dev) | 135 result lines: 7157 passed, 0 failed, 9 ignored (all doc tests marked ignore) |
| `cargo test --workspace --release --no-fail-fast` | 135 result lines: 7114 passed, 0 failed, 9 ignored (debug-only tests account for the 43 difference) |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo clippy --workspace --all-targets --release -- -D warnings` | 1 error (m1, pre-existing import) |
| `cargo clippy -p hares-equipment -- -D warnings` (lib only) | clean |
| `cargo fmt --all --check` | clean |
| compare-all default / `-F dst` / `-F profiling` | 5/5, 1/1, 5/5 identical |

Not run: the Python OCHRE parity suites (not requested here; recorded as
skipped in the implementer's log).

## Item 5: quality notes

The one-line changes across about 70 files are the new
`ambient_other_space_c: hares_types::AmbientOtherSpaceTemps::default(),`
field in `EnvironmentState` literals (tests and benches, 60-plus sites, plus
`environment.rs:804`). Mechanical and necessary, but it is the 60-plus-site
cost of having no `Default`/builder for `EnvironmentState`; a shared test
constructor would remove it. Dead or orphaned after `helpers.rs` lost 92
lines: nothing orphaned in `helpers.rs`; the orphans are the forwarders and
the unused tankless field listed in m4, and the `zone_id_explicit` doc
comments that still say "fell back to ZoneId(1)" (true only for the retained
fallback, M1). Comments carry why-context, no ticket IDs, no em dashes.
