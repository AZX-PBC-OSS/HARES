# OR-11 stopped-rows research: zone-unresolved fallbacks, synthetic closed envelope, IdealHvac end-use sign

Scope: the three rows the independent audit (verification/audit-OR-11-b8195c83.md)
rated Blocker B1, Blocker B2 and Blocker B3, plus the related minor m1
(helpers.rs disposition text) and the dehumidifier zone path, which turned
out to share root cause with B1 but needs its own fix. All evidence below
was gathered in the audit worktree (/home/rich/src/HARES/.workstrees/audit-7b7a6a7d,
checked out at 7b7a6a7d) with the vendors/OCHRE submodule initialized
(git submodule update --init vendors/OCHRE; additive, non-destructive).

## Row 1: zone-unresolved fallback to ZoneId(1) (five water heaters, dehumidifier, helpers.rs)

### Root cause and evidence

HPXML Location strings reach HARES zone resolution through
`parse_zone_label` (crates/hares-io/src/hpxml/building.rs:3074-3095), which
buckets any location string containing "other" into `ZoneType::Adjacent`
(building.rs:3088-3091), after the more specific "attic"/"garage"/
"foundation"/"basement"/"crawl"/"condition"/"ground"/"out"/"ambient"
checks. This matches the full HPXML enumeration for water-heater and
appliance Location: "other heated space", "other housing unit", "other
non-freezing space", "other multifamily buffer space" all hit the
"other" branch; "other exterior" also hits it ("exterior" does not
contain "out", so it is not caught by the Outdoor branch); plain
"outside" does hit the Outdoor branch via `contains("out")` and resolves
fine.

`building.zones` (the Vec a Location gets matched against) is built in
`build_zone_map` (building.rs:2258-2340+) and only ever inserts
Conditioned, Outdoor, Attic, Garage and Foundation entries from the
boundary/enclosure data. No `ZoneType::Adjacent` entry is ever
constructed as a modeled zone; `zone_key(&ZoneType::Adjacent)` (building.rs
3147) is `"adjacent"`, which never matches any key `zone_id_for_location`
(crates/hares-io/src/hpxml/resolve_water_heater.rs:53-61) can find. So for
every water heater or appliance whose Location is one of the five "other
..." strings, `zone_id` is structurally guaranteed to be `None` while
`zone_type` is `Some(Adjacent)`: this is not a parsing gap, it is the
HPXML schema's own meaning for these locations: they name space that is
not represented as a separate thermal zone in the model.

OpenStudio-HPXML's own documentation (and OCHRE's hpxml.py, the same
convention HARES already follows for boundary adjacency) treats this
case the same way: "other heated space" and "other housing unit" are
not simulated as distinct thermal zones; equipment and loads located
there read an ambient temperature series rather than a zone's RC state.
OS-HPXML's published rule: Other Heated Space = average(conditioned,
outside) with a 68F floor; Other Multifamily Buffer Space =
average(conditioned, outside) with a 50F floor; Other Non-Freezing Space
and Other Exterior have their own similar average-with-floor rules.
OCHRE's own code confirms the shape of this: `parse_zone_name`
(vendors/OCHRE/ochre/utils/hpxml.py:65-84) maps any Location containing
"other" to the string "Adjacent", and the water heater model reads
`schedule_inputs[f"{self.zone_name} Temperature (C)"]`
(vendors/OCHRE/ochre/Equipment/WaterHeater.py:84), i.e. OCHRE also does
not give these locations a modeled thermal zone; it expects an ambient
temperature time series keyed by the location name, supplied as an input
column, not computed from a zone's envelope state.

A real OS-HPXML sample file exercises exactly this:
`vendors/OCHRE/test/OS-HPXML Sample Files/base-bldgtype-multifamily-adjacent-to-other-heated-space.xml`
has `<WaterHeatingSystem><Location>other heated space</Location>`
(line 315) alongside ClothesWasher/ClothesDryer/Dishwasher/Refrigerator/
CookingRange in the same location (lines 348-384). This is a real,
schema-valid HPXML construction, not a theoretical edge case.

### Census of fixtures that hit the fallback today

Searched the full vendored OS-HPXML sample library
(`vendors/OCHRE/test/OS-HPXML Sample Files/*.xml`) for `WaterHeatingSystem`
blocks whose `Location` contains "other":

- `base-bldgtype-multifamily-adjacent-to-multifamily-buffer-space.xml`: "other multifamily buffer space"
- `base-bldgtype-multifamily-adjacent-to-non-freezing-space.xml`: "other non-freezing space"
- `base-bldgtype-multifamily-adjacent-to-other-heated-space.xml`: "other heated space"
- `base-bldgtype-multifamily-adjacent-to-other-housing-unit.xml`: "other housing unit"
- `base-dhw-combi-tankless-outside.xml`: "other exterior"
- `base-dhw-indirect-outside.xml`: "other exterior"
- `base-dhw-tank-gas-outside.xml`: "other exterior"
- `base-dhw-tank-heat-pump-outside.xml`: "other exterior"
- `base-dhw-tankless-electric-outside.xml`: "other exterior"

Nine files, one per water heater type HARES models (gas, heat pump,
indirect tank, resistance/electric tank-gas naming aside, tankless), plus
the four multifamily-adjacency files covering the other four "other ..."
strings.

None of these nine files are in HARES's curated or exercised fixture
sets: `tests/fixtures/hpxml/ochre_samples/` (the 14-file curated set
checked by `hpxml_fixture_inventory.rs`) contains no water heater with a
non-"living space"/"garage" Location, and the eight real ResStock golden
fixtures under `tests/fixtures/resstock/` contain no `<Location>other...`
string anywhere. So no currently-green golden, parity or consumer test
exercises this fallback's numeric output today: the fallback is live
code with zero current test coverage, not a path goldens depend on.

### Dehumidifier is a related but distinct bug, not the same legitimate case

`Dehumidifier::new` (crates/hares-equipment/src/hvac/dehumidifier.rs:171-172)
calls the same `zone_id_from_config_or_default` helper. But HARES's HPXML
dehumidifier resolver (`resolve_loads.rs:645-650`) always constructs
`DehumidifierConfig { zone_id: None, .. }`; it never reads a Location at
all (HPXML's `Dehumidifier/Location` enumeration is effectively
"conditioned space" only; OS-HPXML models dehumidifiers exclusively in
the conditioned zone). So *every* HPXML-derived dehumidifier goes through
the `None` branch and lands on `ZoneId(1)` by construction, not only ones
with an unusual Location.

This "works" today only because `building.zones` is sorted by
`zone_sort_key` (building.rs:3615-3624, Conditioned=0, Attic=1, Garage=2,
Foundation=3, Outdoor/Ground/Adjacent=4, Other=5) before being converted
to the `Vec<Zone>` that zone ids index into, so in every real
HPXML-parsed building the conditioned zone is index 0 → `ZoneId(1)`. But
this is coincidence, not resolution: HARES's own test
`conditioned_zone_id_returns_correct_id_when_not_first_zone`
(crates/hares-io/src/hpxml/resolve_hvac.rs:5633-5640) proves the
conditioned zone is not guaranteed to be `ZoneId(1)` in general; only
the real-building sort order happens to make it so today. HVAC equipment
resolvers already have the correct primitive for this:
`conditioned_zone_id(building)` (resolve_hvac.rs:1867-1872), used at
resolve_hvac.rs:2086, 2227, 2522 for exactly this "this equipment belongs
in the one conditioned zone, regardless of index" case. `resolve_loads.rs`
never calls it for dehumidifiers.

This is a genuine (if currently inert) correctness bug independent of
Row 1's Location-semantics question, and it should not be fixed by the
same "ambient source, no zone" treatment as the water-heater "other ..."
case: a dehumidifier always has a real modeled zone (the conditioned
zone always exists when a dehumidifier is present), it is just resolved
by the wrong mechanism.

### Recommended fix

Two distinct changes, both replacing the single silently-defaulting
`zone_id_from_config_or_default` helper:

1. **Water heaters (gas.rs, heat_pump_wh.rs, indirect_tank.rs,
   resistance.rs, tankless.rs).** When `zone_type` resolves to
   `ZoneType::Adjacent` (the "other ..." family), this is not missing
   wiring, it is HPXML's documented way of saying "this equipment has no
   modeled thermal zone; treat it with an ambient temperature source."
   The typed design: introduce an explicit ambient-source config path
   (e.g. `WaterHeaterConfig.ambient_source: Option<AmbientZoneTemperature>`
   capturing the OS-HPXML averaging rule: average of the conditioned
   zone's and outdoor temperature with the location-specific floor, 68F
   for "other heated space", 50F for "other multifamily buffer space",
   etc.) and construct equipment with that source when `zone_id` is
   `None` and `zone_type` is `Some(Adjacent)` or another
   non-zone-backed type. When `zone_type` is `None` entirely (an HPXML
   `Location` the parser could not classify, i.e. truly unresolved
   wiring) or is a type that should always carry a zone and does not
   (programmer/data error), return a typed `HaresError::Equipment`
   naming the water heater and the unresolved zone/location. Never
   `ZoneId(1)`.
2. **Dehumidifier and `hvac/helpers.rs`'s `zone_id_from_config_or_default`.**
   Delete the fallback-to-`ZoneId(1)` helper's use for dehumidifiers;
   resolve the dehumidifier's zone via the same `conditioned_zone_id`
   pattern HVAC equipment already uses (either by having
   `resolve_loads.rs` inject `zone_id` into `DehumidifierConfig` from
   `conditioned_zone_id(building)` at parse time, consistent with how
   furnace/AC resolvers do it, or by giving `Dehumidifier::init` the
   same "resolve via conditioned zone, error if none" logic). If no
   conditioned zone exists at all (malformed building), that is a typed
   error, not a `ZoneId(1)` guess.
   `zone_id_from_config_or_default` itself should be deleted once no
   caller depends on the fallback; its doc comment's framing ("the
   fallback stays... legitimately leave the equipment zone unresolved")
   is accurate for the water-heater Adjacent case and wrong for the
   dehumidifier case, which is exactly m1's finding that the census
   table's disposition text needs correcting either way.

### Tests that pin it

- One acceptance test per water heater type: HPXML Location = an
  "other ..." string → construction succeeds with the equipment carrying
  an ambient-source config (not a zone id), using the exact averaging
  rule, checked against a hand-computed value for a known conditioned/
  outdoor temperature pair.
- One test per water heater type: `zone_type: None` (truly unparseable
  Location) → construction returns `Err(HaresError::Equipment(..))`
  naming the equipment and the location.
- `dehumidifier_zone_id_resolves_to_conditioned_zone_not_first_zone`: a
  building whose conditioned zone is not index 0 (mirroring
  `conditioned_zone_id_returns_correct_id_when_not_first_zone`) →
  dehumidifier's resolved zone equals the conditioned zone's id, not 1.
- `dehumidifier_errors_when_no_conditioned_zone_exists`.

### Golden and consumer impact

Numeric/golden impact for the water-heater ambient-source fix: none of
HARES's current goldens, parity fixtures or the eight ResStock consumer
fixtures contain an "other ..." Location, so no existing passing test's
output changes. This fix adds new, currently-uncovered behavior rather
than changing covered behavior; it is not bitwise-neutral by definition
(it replaces "silently runs the equipment in zone 1's actual thermal
state" with "runs the equipment against a correct ambient series"), but
nothing today exercises the difference. It should land as its own row
rather than folding into a mechanical audit-fix pass, specifically because
it requires new physics (the averaging formula) rather than just
tightening an existing check, the same reasoning the main entry used to stop
on the fluid-loop row.

The dehumidifier fix is bitwise-neutral for every fixture in the repo
today, because `zone_sort_key` currently makes the conditioned zone
`ZoneId(1)` in every real parsed building; it only changes behavior for
a building where the conditioned zone is not first, which no current
fixture constructs. It is safe to land as part of the same mechanical
pass as Row 2/Row 3's typed-error conversions.

## Row 2: synthetic closed-envelope check (dwelling/synthetic.rs:1368-1431)

### Is this a production path?

Yes. `build_synthetic_building` (crates/hares-core/src/dwelling/synthetic.rs:479)
is called from `Dwelling::from_toml_config` /
`Dwelling::from_toml_config_with_write_output`
(crates/hares-core/src/dwelling/mod.rs:2049-2087), a `pub fn` constructor
documented as "Constructor for synthetic TOML dwelling definitions
(BESTEST-style inputs)." This is the production entry point for every
BESTEST and synthetic-TOML dwelling, including the real BESTEST
benchmark fixtures (`tests/fixtures/bestest/600.toml`, `640.toml`,
`900.toml`, `600ff.toml`, `900ff.toml`) that back the thermal-solver
parity goldens. It is not a test-only or Python-only path: it is the
one and only way a synthetic-TOML dwelling gets built, used by both the
production CLI/benchmark harness and by the crate's own unit tests.

### Which fixtures trip the check, and what they test

The check only fires when the TOML supplies an explicit `[[boundaries]]`
override list (the branch at synthetic.rs:1080 that skips the default
six-face auto-generation at synthetic.rs ~3318-3439) and
`zone_volume_m3 > 0.0`. Walking every `[[boundaries]]` block in
`dwelling/synthetic.rs` and `tests/`:

- The four real BESTEST goldens (`600.toml`, etc.) supply a full
  Wall×4 + Roof + Floor set, already closed, already pass.
- `benchmark_style_default_produces_physically_closed_envelope` and
  `explicit_boundaries_not_affected_by_auto_generation` (synthetic.rs)
  supply Wall+Roof or more, already pass.
- `bestest600_south_wall_area_clamped_to_zero` supplies the full 7-boundary
  BESTEST 600 set, already passes.
- Five tests trip it, all single-`Wall`-only overrides with
  `zone_volume_m3 = 120.0`: `window_area_deducted_from_host_wall`,
  `zero_conductivity_with_positive_thickness_returned_as_error`,
  `zero_density_with_positive_thickness_succeeds`,
  `zero_specific_heat_with_positive_thickness_succeeds`,
  `negative_density_returned_as_error`. Each is testing a narrow,
  unrelated behavior (window-area deduction arithmetic on a single wall;
  material-layer validation edge cases for conductivity/density/specific
  heat), not envelope physics, and none of them assert on boundary count
  or envelope closure elsewhere in the test body.
- One more lives outside this file:
  `crates/hares-core/tests/thermal_invariant_wiring.rs`'s
  `SYNTHETIC_RC_DWELLING_TOML` (single Wall, `zone_volume_m3 = 129.6`),
  used to test that the thermal-balance invariant fires correctly when
  balance terms are deliberately broken, again unrelated to envelope
  closure.

### Recommended fix

Option (a) from the audit: implement the originally-specified
unconditional typed error (naming the missing face categories and the
zone), and give each of the six scaffolding fixtures a second face
(e.g. add a `Floor` boundary with the same area and a minimal material
layer) so they construct a legitimately closed envelope. This is the
right choice over a test-only constructor because:

- The check is reachable through the only production constructor for
  synthetic dwellings; a parallel non-validating constructor would be
  exactly the kind of silent-default escape hatch the project forbids,
  and it would mean the real BESTEST path is validated while a second,
  unvalidated path quietly exists alongside it for anyone who later
  reuses it.
- The six fixtures are using the boundary-override mechanism purely as
  scaffolding to reach an unrelated code path; each needs one trivial
  additional boundary, not a redesign. None of their assertions depend
  on boundary count, so adding a `Floor` does not touch what they
  verify.
- A non-physical one-wall "zone" with positive volume is the literal
  defect class (open thermal envelope) OR-11 exists to close; leaving a
  production constructor able to build one silently is inconsistent with
  every other wiring row in this entry.

### Tests that pin it

- `synthetic_building_rejects_open_envelope`: a one-wall,
  `zone_volume_m3 > 0` TOML (the exact scaffolding shape the five
  existing tests used) → `Err(HaresError::Dwelling(..))` naming the
  zone and which face categories are present/missing.
- `synthetic_building_accepts_two_of_three_face_categories`: Wall+Floor
  only (no Roof) → succeeds, proving the check is "at least 2 of 3," not
  "all 3."
- The six existing fixtures gain a `Floor` boundary; their existing
  assertions are otherwise unchanged (this is the regression test that
  the unrelated behavior still holds with a closed envelope).

### Golden and consumer impact

Bitwise-neutral for every current golden and the real BESTEST fixtures
(they already have closed envelopes). The six scaffolding fixtures gain
one more boundary surface each; this can in principle add a small
additional RC branch to those specific unit tests' internal state, but
none of their assertions inspect total boundary/RC-node count, so their
pass/fail outcome is unaffected. No consumer-facing numeric change.

## Row 3: IdealHvac end_use / capacity-sign invariant (hvac/ideal_hvac.rs:841-866)

### Mechanism

`end_use` is set once per `update_control()` call from the discrete FSM
mode (ideal_hvac.rs:569-578): `Heating -> HVAC_HEATING`,
`Cooling -> HVAC_COOLING`, `Deadband -> carries over the previous
end_use unchanged`. Delivered `capacity_w` is computed independently in
`step()` (ideal_hvac.rs:640-654). In `use_ideal_cached` mode (the ideal
solver-driven capacity path used by the "ideal" HVAC oracle), the
Deadband branch does **not** zero capacity:

```
ThermostatMode::Deadband => self.ideal_capacity_w * self.load_fraction,
```

whereas the FSM-mode `Heating`/`Cooling` branches clamp capacity to the
matching sign. `cached_ideal_target` (and therefore `ideal_capacity_w`'s
sign) is computed from `self.thermostat_fsm.effective_setpoints()`
(ideal_hvac.rs:550-562), which incorporates any active
`RuntimeSetpointOverride` (`thermostat_fsm.runtime_setpoints`, set via
`validate_runtime_override`, ideal_hvac.rs:261-282, 973-1034), a
genuine consumer-facing control path (demand-response / thermostat
override actors, not a test-only hook). The FSM's own `mode` (what
`end_use` is derived from) uses separate hysteresis logic
(`update_mode`, called at ideal_hvac.rs:544) that can still read
`Deadband` for one or more steps after a runtime override has already
pushed the ideal target, and therefore `ideal_capacity_w`'s sign, to the
opposite thermal direction. In that window, `capacity_w` is nonzero with
one sign while `descriptor.end_use` still carries whatever heating/
cooling value it had from before the override, i.e. exactly the
documented case: "a thermostat override drives cooling while the
FSM-mode-driven end_use still reads hvac_heating."

### Does energy get booked to the wrong end use today?

Yes, for the affected steps, in the output end-use breakdown. `end_use`
is the key every per-end-use electric power output column is keyed by
(`crates/hares-io/src/output/metrics.rs`: `energy_by_end_use` /
`peak_by_end_use`, built from `end_use_columns` derived from equipment
descriptors). During the mismatch window, this equipment's electric
power for those steps is summed into the end-use bucket matching the
*stale* FSM mode, not the bucket matching the capacity actually
delivered. This only affects the per-end-use split in the output
summary (e.g. "Heating Electric Power" vs "Cooling Electric Power"
columns); it does not affect total dwelling electric consumption, which
sums across all end uses regardless of label. It is reachable only
through `use_ideal_cached` combined with an active runtime thermostat
override, the "ideal" HVAC capacity-mode oracle used by calibration/
solver-driven runs, not the default bang-bang equipment model, and only
HARES's own conditioned-oracle test infrastructure
(`crates/hares-core/tests/conditioned_oracle.rs`) and any consumer code
that issues `RuntimeSetpointOverride`s against an ideal-capacity HVAC
unit can hit it. No current golden or ResStock fixture issues such
overrides, so no existing fixture's output numbers are affected today.

### Recommended fix

Derive `end_use` from the sign of the *delivered* `capacity_w` each step,
not from the FSM's discrete mode, so the invariant holds by
construction instead of being asserted after the fact. Concretely: move
the `self.descriptor.end_use = ...` assignment from `update_control()`
(which only sees the FSM mode) into `step()`, after `capacity_w` is
finalized (including the minimum-capacity clamp), with
`capacity_w > 0.0 -> HVAC_HEATING`, `capacity_w < 0.0 -> HVAC_COOLING`,
`capacity_w == 0.0 -> carry over the previous end_use` (preserving the
existing, correct deadband-preservation behavior for the only case where
capacity is legitimately zero). This removes the possibility of
divergence entirely rather than detecting it; the `#[cfg(debug_assertions)]`
block and its `tracing::error!` are then deleted, not converted to a
`debug_assert!`, because the invariant is no longer reachable as a
violation, exactly the "right fix" pattern used elsewhere in OR-11
(e.g. the HPWH COP check converted to an unconditional `Err` rather than
an assertion).

### Tests that pin it

- `ideal_hvac_end_use_tracks_capacity_sign_through_runtime_override`: set
  an ideal-capacity HVAC unit to `Heating` mode, let it settle, then
  issue a `RuntimeSetpointOverride` that drives the ideal target to
  cooling while the FSM mode has not yet transitioned; assert
  `descriptor.end_use == HVAC_COOLING` on the very next `step()`, i.e.
  before the FSM mode itself changes.
- Existing deadband-preservation tests (`ideal_capacity_mode_override_switches_at_runtime`
  and neighbors) continue to assert end_use carries over when capacity is
  exactly zero.

### Numeric impact

Bitwise-neutral for every fixture and golden in the repo today (none
currently drive this equipment with a runtime override that crosses
deadband while the FSM mode lags). It is a real fix for a real
output-attribution bug in the ideal/calibration HVAC path, not a
cosmetic one, and should be verified against the conditioned-oracle test
suite (`crates/hares-core/tests/conditioned_oracle.rs`) before landing
since that is the one test file that exercises `use_ideal_cached` with
mode transitions.

## Cross-row process note

`decision-log.md`'s `or11_resolution` entry already states, correctly,
that none of these three rows landed as the census table's specified
disposition and that this research is the prerequisite for their own
ledger entry before row 16a. The ledger's census table
(`ledger/03-output-reporting.md`) disposition text for
`hvac/helpers.rs:59-90` (minor m1 in the audit) also needs correcting
regardless of which fix lands for the dehumidifier: once the
dehumidifier goes through `conditioned_zone_id`-style resolution instead
of the shared helper, the table should describe the water-heater
"ambient source for Adjacent locations" behavior and the dehumidifier's
"conditioned-zone resolution, typed error if absent" behavior as two
separate dispositions rather than one shared "unconditional typed error
naming the equipment and the HPXML location" line, which was never
accurate for the water-heater Adjacent case to begin with.
