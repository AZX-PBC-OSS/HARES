# Review and red team: fix-or18c (5d66dafc..7b409ca5)

Verdict: CHANGES-REQUIRED

Counts: 0 blockers, 0 majors, 7 mediums, 9 lows, infos below. The physics of
the capacitance multiplier, the terrain factor and the gable derivation is
sound against OS-HPXML v1.12.0 and EnergyPlus v24.2.0. The mediums are
register and provenance defects, one untriaged attic bias, and a material
roughness table whose "dataset record" rule picks records inconsistently.

## How this was checked

- Worktree `.workstrees/fix-or18c` at 7b409ca5 (clean). Scratch worktrees
  under its `target/` (`base` at 5d66dafc, `mut` at 7b409ca5 for mutations,
  `fold` at 2f85338d for the cherry-pick), upstream clones in `target/ref`
  (OpenStudio-HPXML v1.12.0, EnergyPlus v24.2.0). `vendors/OCHRE` symlinked
  into the scratch worktrees. All scratch and `target-review-or18c` deleted.
- `cargo nextest run --workspace` (debug): 7236 run, 7236 passed.
- `frame_golden compare-all` (release): consumer_shape_900s, fleet_mixed_offset,
  full_year_hourly, leap_year_february_900s, month_boundary_billing_3600s,
  weather_wrap_900s all identical. full_year_900s_dst needs `-F dst`: not run.
- Not run: pytest (needs a maturin build), clippy, the `-F observe/dst/profiling`
  nextest variants. Python thermostat callers were checked by reading.

## Findings

### Medium

**M1. The "dataset record" roughness rule picks records inconsistently, and
four of its claims are contradicted by the dataset.**
`crates/hares-physics/src/film_coefficients.rs:95-190` and D-007. Against
`target/ref/ep/datasets/ASHRAE_2005_HOF_Materials.idf` (v24.2.0):
- fiber cement and asbestos siding map to "Asbestos-cement board" (Smooth, line
  490/498), but the dataset's own siding record "Siding: Asbestos-cement 6.4mm"
  (line 2022) is VeryRough, as is "Shingles: Asbestos-cement" (2002).
- masonite siding maps to "Hardboard Medium density" (Smooth, 658), but
  "Siding: Hardboard 11mm" (2037) is MediumSmooth.
- wood siding maps to F11 (MediumSmooth, 156), but all four "Siding: Wood ..."
  records (2042-2057) are Rough.
- wood shingles map to F15 (VeryRough, 188), but "Shingles: Wood ..." (2007-2017)
  are MediumRough.
- the doc says stone veneer and vinyl siding have no record. F10 "25mm stone"
  (148) is MediumRough (code gives Rough), and "Siding: Hollow-backed" (2062) is
  Smooth, the vinyl analogue.
Verified correct: asphalt shingles F12 (164) and "Asphalt shingles" (1897)
VeryRough, F13 Rough, F14 VeryRough, F08 Smooth, F07 stucco Smooth and M01 brick
MediumRough (both overridden by the Engineering Reference table, which is
documented), EPS VeryRough. Multipliers 2.17/1.67/1.52/1.13/1.11/1.00 match
`ConvectionCoefficients.cc:114` and the Walton table. Failure scenario: a
fiber-cement or wood-siding home gets Smooth or MediumSmooth walls where the
same dataset's siding records say VeryRough or Rough: lower forced convection
and a biased envelope load, justified in the register by a record choice that
the dataset itself contradicts. Fix: choose per HPXML material the dataset's
siding or roofing record (the "Siding:" and "Shingles:" family), state the
choice and the ones that conflict, and correct the "no record" claims.
Note for the register: OS-HPXML's actual behaviour is Rough for every surface
(`model.rb:49,95`, verified), so every row of this table is a departure.

**M2. A surface with no material takes Rough, but OS-HPXML defaults the material.**
`film_coefficients.rs` (`None | "none" | ...` arm). `defaults.rb:1268` gives a
roof with no RoofType asphalt shingles, `:1338` and `:1379` give a rim joist or
wall with no Siding wood siding. Under HARES's own material policy a roof with
no RoofType should be VeryRough (shingles) and a wall MediumSmooth or Rough, not
1.67. The fallback is a warning-and-default that D-007 describes as "OS-HPXML's
Rough", which is not what OS-HPXML does for a missing element. Also D-007 and
`boundary_outside_roughness` say foundation walls read Siding: HPXML
FoundationWall has no Siding (`hpxml.rb:4173,4401` are RimJoist and Wall only), so
the read is always None and logs a spurious warning for any outdoor-facing one.

**M3. The gable attic's height, not just its volume, changes the leakage
coefficients, and D-006 does not say so.**
`zone_geometry.rs:attic_geometry` sets `height_m` to the gable rise;
`solver_builder.rs:1716` feeds it to `attic_ela_coefficients` (stack coefficient
proportional to height, wind height above the walls top). OS-HPXML uses the hip
height for these (`calc_wind_stack_coeffs` with `calculate_zone_height`,
`airflow.rb:2749-2771`, `geometry.rb:1362`). base.xml: 7.5 ft against 8.216 ft,
so the stack coefficient is 9 % lower than OS-HPXML's; commit 5a1edeae claims
OS-HPXML's attic leakage. A departure beyond OS-HPXML with no register line and no
pinning test (the tests pin height only for the gable geometry, not the leakage
coefficient). Fix: either pass the hip height to the leakage, or add it to D-006
with a test.

**M4. Parity bands against OCHRE were loosened without a triaged class.**
`tests/parity/mod.rs` (9cff224b): `cz5a_minisplit_gas_wh` total site 17.2 % to
28.4 % (band 29.5), `cz6b_resistance_res_wh` 19.9 % to 25.4 % (band 26.5),
minisplit peak 92.5 to 94.0. The triage rule says no bound is loosened until
the class is established with evidence; the commit asserts a cause ("already
delivered less heat in their unwarmed first hour") with no measurement. The same
commit reverses the earlier documented finding that a 7x multiplier plus furniture
RC nodes overstates mass by about 39 %, on OS-HPXML's citation alone, with no
EnergyPlus comparison of the overstating. The reversal is right by OS-HPXML
(`constructions.rb:1817-1840` shows InternalMass alongside), but the worsened
windows are classed by assertion.

**M5. The attic bias against OCHRE is not triaged; the roughness change hides a
winter cold bias.**
Reproduced at HEAD (`conditioned_oracle`, release): attic MAE 0.899/0.894 summer,
0.791/0.778 spring, 1.310/1.275 winter, matching D-007. Attic means (HARES, OCHRE):
summer 29.47 vs 28.96, spring 16.38 vs 16.81, winter 3.56 vs 4.82. Experiments in a
scratch copy:
- shingles Rough (1.67) instead of VeryRough: summer 29.97, spring 16.85, winter
  3.61, MAE 1.54/1.27/1.39 (the register's 1.48/1.24/1.37 used Rough everywhere).
  Roughness moves the attic by about 0.5 C in summer and spring and 0.05 C in
  winter.
- rural terrain on every leakage path (OCHRE's wind): attic 29.35 / 16.28 / 3.46,
  more ventilation makes every season colder, so the winter cold bias (-1.26 C) is
  not too little ventilation. Ventilation sensitivity is about 0.1 C.
So roughness corrects the summer and spring warm bias but cannot touch the winter
cold bias, which is the compensation the implementer warns about. Very Rough for
asphalt shingles is right per the dataset (F12, 1897 both VeryRough), but that
is the Engineering Reference's table and the dataset, not evidence about the
attic. Candidates, classed per the triage rule (none has a ledger entry):
- Roof outside roughness: class (a), HARES goes beyond OS-HPXML and OCHRE on
  dataset authority (registered D-007). Not the winter cause.
- Terrain/ventilation: class (a), registered D-005. Not the cause (above).
- Sky and ground long-wave: HARES sky coupling F^1.5 equals EnergyPlus
  (`ConvectionCoefficients.cc:647,659`, `SurfaceGeometry.cc:332`: ViewFactorSky times
  sqrt split), so it matches the reference; matching OCHRE here is incidental.
  Ground at air temperature not checked: untriaged.
- Roof absorptance and emittance: from the same HPXML on both sides; radiant barrier
  false in BEopt (`BEopt_example.xml:246,275`). Not a candidate.
- Interior TARP film on the roof and attic floor (OCHRE holds a frozen film; HARES
  adds a per-step correction only above the frozen value, `stepping.rs:582-640`):
  untriaged, class (c) or (d), the best remaining winter candidate (an
  attic-to-ceiling and deck coupling term).
- Deck mass: same LUT both sides; low likelihood.
Fix: open a ledger entry for the winter -1.3 C and summer residual with the
interior-film and ground long-wave candidates, and state in D-007 that the winter
bias is untouched.

**M6. Golden provenance.**
At 7b409ca5 six of seven goldens carry `"git_dirty": true` (`consumer_shape` is
false) and every one records `git_head` 59d73f89, the docs-register commit.
Commits 5ccbd9c3, 46beca4c, 88cd4944 and 06ef6fcc change code after that head
(site type, outside material reading, hip fallback, unrecognised boundary).
I verified the six goldens that `frame_golden compare-all` reads are identical at
HEAD, so the data are right; the provenance is not. The two intermediate batches
(132ed237..e903016e, 4b7b9d4f..caa47532, 14 commits) have no body at all: no
deltas. The last batch (9998e697..15054847) states deltas but no cause. The
consumer_shape cooling change (1062.3 to 1017.3 kWh, -4.2 %) is on a home with no
attic (`bldg0176227` has none) and the cause is the flat asphalt-shingle roof
going from MediumRough to VeryRough (`home.xml:506,523`, Pitch 0); nothing says so.
Fix: re-capture from a clean committed tree at the final code, one commit each, with
the cause per delta.

**M7. The equivalent-battery energy and capacity change by 7x on every HPXML home
and nothing says so.**
`conversions.rs:building_to_zone_inputs` now gives the conditioned zone 7 where
furniture-bearing HPXML homes had 1; `mod.rs:2596` hands the same capacitance
(`zone_capacitance_kwh_per_k`) to the HVAC equivalent battery, whose energy and
min/max energy telemetry scale with it (`helpers.rs:531`). The consumer reads
those columns. No commit, ledger line or register entry records it, and nothing
decides whether the EBM capacity should be air times 7 at all. Fix: record the
delta, or give the EBM its own capacity definition.

### Low

- **L1. `Actor::health_detail` is implemented for the thermostat only.**
  `EvDriverActor::estimate_error` (`ev_driver/mod.rs:328`) and
  `PyActorWrapper::last_error` (`py_actor.rs:81`) hold Option<String> causes, so
  their step errors still read "no cause reported". The commit says the trait
  carries an unhealthy actor's cause into the error.
- **L2. Thermostat eviction is a silent behaviour change.** With
  `dispatch_target_name` set, removing or swapping the target equipment drops the
  thermostat (info log only). A Python swap that removes "Gas Furnace" loses the
  user's override with no error; the Rust test pins the eviction, the Python
  surface and docs do not mention it. Callers are otherwise migrated: the
  registry, the Python tests, `checkpoint_restart.rs`; `IdealThermostat::new("HVAC")`
  remains only in unit tests that never step a dwelling. `load_state` restores only
  `override_state`; `missing_target` is re-derived by `refresh_equipment_caches`, so
  checkpoint is correct.
- **L3. Shipped text.** One added em dash: `tests/python/test_actor_param_boundary.py`
  ("... schedule {scheduled:.0f}) — the observable"). Commit 4a5db8f0 says
  "Ticket 038". No other em dash, ticket ID, agent reference or Co-Authored line in
  added lines or in any of the 49 commit messages.
- **L4. The gable derivation is ill-conditioned near square footprints.** The
  rectangle comes from a quadratic in area and perimeter. For a 1350 ft2 near-square
  house, a 1 % error in wall area moves the span from 36.7 to 32.0 ft (-13 %) and
  the ratio guard (1.32) still admits it, so the volume is 12.7 % low (still better
  than the hip's 40 % low, so a net gain, but not "robust"). Asymmetric roofs of one
  pitch (saltbox) and roofs with no azimuth (treated as two-way) pass the gate; a
  shed roof is caught by the ratio (2.0).
- **L5. The 0.98-1.5 gable-end window rests on one example.** 0.98 is "rounding",
  1.5 is headroom above BEopt's single 1.28 (eaves on gable walls but not roofs).
  No corpus run backs it. Checked on ResStock `bldg0000017`: roofs 708.8 ft2 at 6:12
  over 634 ft2, walls 840.8 ft2 at 8 ft give 33.8 x 18.8 ft, the 44.0 ft2 gable ends
  equal the 18.8 ft span's triangle exactly, ratio 1.0, so the model is exact for
  eaveless rectangles. `bldg0112631` (four azimuths over a garage) correctly falls
  to the hip with a warning. The hip warnings are accurate; nothing is silently
  defaulted except missing roof azimuths being read as two-way (L4).
- **L6. A site type absent from the HPXML is warned by `tracing::warn!` only**
  (`solver_builder.rs:1361`), not in the dwelling's warning list, so Python
  consumers do not see D-005's "with a warning".
- **L7. Constant and doc names.** `INTERIOR_MASS_MULTIPLIER` is the air temperature
  multiplier now, not interior mass; `docs/architecture.md:473` still says "7x
  interior mass multiplier for thermal storage". A synthetic building's
  `geometry.mass_multiplier` is not validated: zero, negative or NaN clamps
  silently to 1000 J/K (`boundary_rc.rs:546`).
- **L8. Pre-existing silent defaults still there.** `solver_builder.rs:1352,1554`
  `ceiling_height_m.unwrap_or(2.5)` and `floors_above_grade.unwrap_or(1.0)` (`:1393,1404`).
- **L9. The new structural test recomputes the implementation's formula**
  (`gable_attic_volume_m3`), so it pins consistency, not a reference value; it does
  fail under the hip mutation (below).

### Info

- **I1. Capacitance multiplier (item 1): correct.** `ZoneCapacitanceMultiplier:ResearchSpecial`
  with a blank zone applies to every zone (`ZoneTempPredictorCorrector.cc:1725-1830`);
  OS-HPXML sets temperature to the header value (default 7) and humidity to 15
  (`simcontrols.rb:27-31`, `defaults.rb:219-221`, `hpxml.rb:1051`). HARES applies the
  one multiplier once, in `derive_zone_capacitances`, to the zone air node only; the
  same capacitance feeds the B column (`solver_builder.rs:932`), the energy balance
  and the EBM, never multiplied again. Humidity is 15 on every zone
  (`humidity_solver.rs:35`), unchanged. No CO2 model. The temperature multiplier
  does not scale furniture or partition nodes, as in EnergyPlus. Initial
  conditions are not multiplied in either. The legacy element is ignored with a
  warning, non-positive is an error.
- **I2. Terrain (item 2): matches `airflow.rb:200-224, 2766` for rural, suburban,
  urban**, including the `1.0 (32.8/32.8)^0.15` station denominator; absent site
  type is suburban (`defaults.rb:817-818`). OCHRE's rural assumption
  (`envelope.py:648-649`) is correctly described. HARES's TARP 7.238 matches
  EnergyPlus source (`ConvectionCoefficients.hh:433`; the doc's 7.283 is a typo).
- **I3. Mutation reproduced (item 6).** Gable replaced by the hip (`gable_rise_m`
  returns None): the two D-006 parsing tests and `beopt_building_structure`,
  `beopt_rc_network_topology`, `beopt_ua_parity` all fail, as the register says. D-007's
  `only_an_outdoor_surface_reads_its_material`, `only_material_bearing_boundaries_read_their_material`
  and `finish_type_roughness_changes_exterior_film_resistance` fail when every outdoor
  surface takes Rough, and `every_hpxml_material_takes_its_energyplus_roughness`
  fails only for a mapping-level mutation (it calls the mapper directly).
  D-005 and D-006 numbers match the earlier measurement in the attic note; the MAE
  figures were reproduced for D-007 only.
- **I4. Register collision (item 6).** This branch and `fix-od30` (8ded384b) both
  append D-005, D-006 and D-007 after D-004 in `docs/alignment/DIVERGENCES.md`:
  here terrain, gable attic volume, outside roughness; there appliance and plug
  radiant heat, lighting heat split, visible light absorption. A textual add/add
  conflict of the whole tail of the file. `docs/tickets/038-surface-roughness-hardcoded-rough.md`
  here cites "D-007" and would point at the visible-light entry. Nothing else cites
  the IDs. Renumber whichever folds second (D-008 to D-010 here if od30 folds
  first) and fix the ticket reference in the same commit.
- **I5. Fold readiness (item 8).** Cherry-picking the 49 commits onto 2f85338d (24
  commits past the base): textual conflicts in
  - 9ba22b42: `dwelling/blueprint.rs` (head's `config.load_schedule(&building, ...)`
    against the branch's inline schedule block), `dwelling/mod.rs` (three import
    blocks, and `from_config` where head keeps `from_preparsed` and the branch routes
    through `DwellingBlueprint::from_config`);
  - d4d24518 and eb050372: `hares-io/src/hpxml/building.rs` (head edited the garage
    tests the branch deletes along with `compute_garage_volume`);
  - 50f3761a: `dwelling/conversions.rs` (test import list, head added
    `apply_spec_bag_to_typed_config`);
  - all seven goldens at every re-capture commit (21 commits): re-capture at the
    folded tip.
  Semantic conflicts that textual merging hides: head's `DwellingConfig::load_schedule`
  (`dwelling/mod.rs:273`) and the new `crates/hares-core/tests/schedule_path_contract.rs`
  (`:~13941 of the diff`) call `generate_schedule_from_hpxml(building, ...)`, which
  the branch renames to `generate_default_schedule` without the building argument;
  the branch's `schedule_path.exists()` assumes a non-optional path where head made
  it `Option`. Cargo.lock merged cleanly. Not compiled after resolution.
- **I6. Performance (item 10).** `frame_golden run consumer_shape_900s.toml`
  (release, one ResStock year at 900 s, 3 interleaved runs each): simulate():
  base 1.134, 1.127, 1.152 s; head 1.127, 1.160, 1.150 s;
  construct 0.040-0.046 s both. RSS 39.4 MB base, 35.0 MB head. No regression.

## Item 3 and 7 summary

Gable derivation: sound for eaveless rectangles under one pitch, falls back with an
accurate warning where the HPXML does not determine the span (garage, floor
mismatch, smaller storey, no floor counts, gable end disagreement, multi-azimuth
roofs); see L4, L5, M3. Goldens: M6.
