# Review and red team: fix-or18c (2f85338d..de561181)

Verdict: CHANGES-REQUIRED

Counts: 0 blockers, 1 major, 3 medium, 5 low. The physics, the registers, the
goldens and the parity triage are largely sound and every previous finding is
fixed at its root. One rebase defect (a pyright failure) and one gable-guard
hole found in the committed corpus keep it from SHIP.

## Checks run (all on de561181, CARGO_TARGET_DIR=target-review-or18c2, incremental off)

- nextest debug 7333/7333, release 7290/7290, `-F observe` 7391/7391, `-F dst`
  7346/7346. This includes the conditioned, free-float and structural oracles,
  BESTEST and parity.
- `frame_golden compare-all` release and `-F dst`: all identical. `frame_golden`
  has no `observe` feature, so there is no third mode there.
- Parity run with output: every residual matches the ledger table to the digit.
- pytest (`not slow and not ochre`) 1327 passed. `maturin develop` clean.
- pyright FAILS: 2 errors (finding A).
- Not run: clippy, the slow/ochre pytest group, a parity reference regeneration
  (it overwrites the fixtures), the consumer repo's tests.
- Scratch: `target/mut` (mutations, a temporary corpus scan test), `target/fold`,
  `target/ref` (EnergyPlus v24.2.0 and OpenStudio-HPXML v1.12.0). All deleted,
  with both target dirs. The worktree is clean.

## Findings

### Major

**A. The rebase left duplicate stub classes; pyright fails.**
`python/ochre_next/_hares.pyi:1751,1784` (the head's `IndirectTank` and
`TanklessWaterHeater`) and `:1926,1959` (commit 460d89a4 adds the same two with
`zone_id` required). Pyright: "Class declaration IndirectTank is obscured by a
declaration of the same name (reportRedeclaration)", 2 errors, exit 1. The
head's `[pyright]` gate passes, so the branch breaks it. The first pair also
declares `zone_id: int | None`, contradicting the required keyword. Failure
scenario: CI's pyright job is red on the fold. Evidence: `uv run --locked
--all-extras --all-groups pyright`; `git show 2f85338d:...pyi` has the first
pair at 1751/1784, the branch commit adds 1926/1959. Fix: delete the head's
pair, keep the branch's (docstrings and the required keyword), and amend into
460d89a4 or a fix commit.

### Medium

**B. The gable guard accepts a single-outside-gable home with the wrong span.**
`crates/hares-io/src/hpxml/zone_geometry.rs:355` (`end_area_m2 = gable_area_m2 /
2.0`), `:385-395`. The code assumes two equal gable ends. In an attached unit
only one gable wall faces outside (the other is attic to attic).
`vendors/OCHRE/test/OS-HPXML Sample Files/base-bldgtype-attached.xml` (and
`-2stories`, `-infil-compartmentalization-test`) has one 168.7 ft2 outside gable
wall; the half (84.35) is closest to the 24.5 ft side's triangle (75.0), ratio
1.12, inside 0.98..1.98, and the span guard passes. Result: attic 78.02 m3,
rise 1.866 m (6.12 ft). The geometry is the 36.7 ft span (the 293.9 ft2 party
wall over 8 ft is that side; its triangle is 168.2 ft2, ratio 1.003): rise 9.18
ft, 117 m3, so the volume is 33 % low, and the leakage height too. The hip (56.9
m3) is lower still, but OS-HPXML is the reference and the code asserts a span
the HPXML contradicts. D-009 and the `GABLE_END_RATIO_MIN` comment claim "every
gable whose span the walls fix has ends of 0.9993 to 1.0008 times its triangle"
over the sample homes; these three are accepted at 1.12 and are counter-examples.
Evidence: a temporary scan test over all 425 committed HPXML files (29 gable
accepted, 12 hip fallback with warning, 1 no gable walls). All other accepted
gables match 143.34 m3 (base) or the ResStock values; the fallbacks and warnings
are accurate. Fix: count the outside gable walls: with one wall, the end is its
full area; require ends of the same size when there are two; tighten the ratio
window (a 5 ft eave cannot make 1.98 on an eaveless OS-HPXML home), and add the
attached sample as a pinning test.

**C. Parity residuals of 20 to 23 % in a fair comparison are not triaged, and
one comparison is still unfair.**
`tests/parity/mod.rs:fixture_override` and ledger OD-61. Measured at head:
`cz6b_resistance_res_wh` 22.06 % HVAC energy and 20.89 % total site;
`cz4a_ashp_hpwh` 22.56 % and 20.71 %; resstock 13.9 %. Sibling fixtures on the
same weather and pinned start sit at 0.53 %. They pass only because the default
band is 25 %. The triage rule says every deviation is classed with evidence;
OD-61 names only the resstock peak and the mini-split remainder, and the
commit's "every other residual falls inside its default band" treats the band
as the class. `cz4a_ashp_hpwh` is not pinned (OCHRE 18.525 C start against
HARES 18.083 C, README states it): the start difference is the cause the
branch itself found to drive 1 h windows (cz2a_pv_ev went 40.1 % to 0.5 % when
pinned), so its 22.6 % is probably the draw and it is 2.4 points from failing
for unrelated reasons. The old reference is therefore not a fair comparison and
is not marked as one in the harness. The vendored OCHRE is the team's own fork
(`hares-pandas3-sentinel`), where the one-line `hp_cop` ordering at
`WaterHeater.py:456/476` could be fixed so the reference regenerates. Fix: open
the entries in OD-61 for cz6b_resistance (22 %) and cz4a_ashp (22.6 %), fix the
fork and regenerate cz4a, or give it a recorded start-difference xfail.

**D. The mini-split "class (a)" band rests on an orphan duct system in the
fixture.**
`tests/fixtures/parity/cz5a_minisplit_gas_wh/building.xml:788-835`: an
`AirDistributionSystem` with attic ducts that no HVAC unit references (the
heat pump at `:701` has no `DistributionSystem`). Verified: OCHRE
`hpxml.py:971` reads the building-wide `hvac_all["HVACDistribution"]` and
attaches it to every unit (`:970-1006`), and `HVAC.py:166-173` exempts only a
"Room AC", so the 33.6 % and 32.3 % residuals are real. But the cause is an
input that OS-HPXML would treat as ductless and the fixture should not carry:
the comparison differs by input (class d), not an OCHRE physics error. The band
(34.6 / 33.3) is wider than the default by an input defect that removing the
orphan and regenerating would close, and the remainder (5 to 8.5 %) is already
OD-61. Fix: remove the orphan distribution system, regenerate, drop the two
overrides.

### Low

**E. Ledger OD-53 describes behaviour the code does not have.**
`ledger/11-other-defects.md:2845-2865`: "documented mapping to the Engineering
Reference" and "an input with no material takes OS-HPXML's Rough with a
warning". Shipped: EnergyPlus dataset records and OS-HPXML's default material.
The Engineering Reference lists stucco as Very Rough; the dataset record F07 is
Smooth (`ASHRAE_2005_HOF_Materials.idf:124`) and is followed (disclosed in
D-010), which moves the consumer_shape home's cooling +18.6 kWh. A judgement the
register states, not a defect, but worth the ledger's matching words.

**F. Ticket ID and ledger ID in intermediate commits.**
398097d2's DIVERGENCES.md added "triaged under OD-56" (removed by a8396b7e), and
its message says "ratchets". The final tree is clean of IDs in the branch's
added lines; no em or en dashes in any added line or message (the em dashes in
`tests/python/test_actor_param_boundary.py` and `generate_parity_reference.py:103`
are pre-existing). Squash the OD-56 line out of 398097d2 if per-commit
cleanliness matters.

**G. "ratchet" in the BESTEST harness is jargon with a ticket ID.**
`tests/bestest/mod.rs:56-92,106,158,184` use RATCHET-OK, `ratchet_baseline` and
"tracked: T-0075" (7 occurrences of the ticket ID). Pre-existing, untouched by
the branch. "Ratchet" itself is a recognised testing term, but `T-0075` is a
ticket ID in shipped code and should go; rename to `drift_baseline` while there.

**H. Stale comment.** `crates/hares-core/tests/orchestration_parity.rs:223`
still says "7x interior mass multiplier". Pre-existing wording, now the
temperature capacitance multiplier.

**I. Parity reference regeneration is not proven reproducible.**
`generate_parity_reference.py` does not seed numpy; it pins only the starting
temperature. The branch says only start-dependent columns changed and the
`e42d87f3` check pins the first row. I could not regenerate (it overwrites the
fixtures); a testing gap.

## Verification of the requested items

1. **Rebase integrity.** `git range-diff` shows every commit of the branch
   either rebased (`=`/`!`) or deliberately dropped/squashed: the 14 intermediate
   golden commits and 5 later ones became 7 final captures, and the dropped
   docs-only commits (59d73f89, 4a5db8f0, 7b409ca5) are folded into
   a8396b7e/15d1e93a. Files the branch and the base both touched were compared
   by patch lines: identical except deliberate changes (the multiplier naming,
   guard rework, tests, and the mod.rs/conversions.rs rework). Head work kept:
   optional `schedule_path`, `load_schedule(&weather.meta)` (the building
   parameter went with the setpoint columns), `THERMAL_SETPOINT` in
   `required_control_capabilities` (`ideal_thermostat.rs:290`). The
   `schedule_path_contract.rs` diff against the head is renames, the doc text
   and the digest only; the generator diff removes exactly the two setpoint
   columns, which explains the digest. I did not recompute the old digest
   (testing gap). One defect: finding A.
2. **Previous findings.**
   - M1 fixed. All 18 cited records (names, lines, Roughness) match v24.2.0
     `ASHRAE_2005_HOF_Materials.idf`: wood siding 2047 Rough, F07 124 Smooth,
     F06 116 Smooth, Hollow-backed 2062 Smooth, M01 332 MediumRough, F10 148
     MediumRough, Asbestos-cement 2022 VeryRough, Asphalt shingles 1897
     VeryRough, Hardboard 2037 MediumSmooth, Shingles Wood 2007 MediumRough,
     Slate 1907 VeryRough, F14 180 VeryRough, F08 132 Smooth, Built-up 1902
     VeryRough, EPS 810 VeryRough, concrete 1018 MediumRough, M05 364
     MediumRough, plywood siding 2057 Rough. Mutation reproduced: stucco
     Smooth to Rough fails `every_hpxml_material_takes_its_energyplus_roughness`.
     The test restates the table inline (a mirror, but it pins record, line and
     class).
   - M2 fixed. `defaults.rb:1268` (roof asphalt shingles), `:1338` (rim joist
     wood siding), `:1379` (wall wood siding), `:1431-1433` (foundation wall
     solid concrete) confirmed.
   - `ShieldingofHome`: `HPXML.xsd:4479` is the schema spelling (enum
     well-shielded, normal, exposed at 7790-7795). The parser read
     `ShieldingOfHome` (`git show 5d66dafc`), so every real file's shielding was
     ignored, which fell to Normal. All 50 committed files say "normal", so no
     golden moves from it; the facility-type default at `defaults.rb:822-830`
     (apartment or single-family attached well-shielded, else normal) is
     correct and the unknown-value error is new. Parse test
     `shielding_is_read_from_the_schema_element`.
   - M3 now in D-009 with `a_gable_attic_leaks_by_its_geometric_rise`. M5 is
     OD-56 with the candidates. M6 and M7: see 6 and 5.
   - Lows: L1, L2, L6, L7 (name, validation, doc) fixed as described in their
     commits; L8 (`unwrap_or(2.5)`, `unwrap_or(1.0)`) gone from
     `solver_builder.rs`; L3 em dash and ticket ID gone from the added text.
3. **M4 triage.** Both harness defects verified. OCHRE draws its start at
   random and unseeded (`Envelope.py:997-1008`, `np.random.uniform`) unless a
   number is given. The old harness summed unit columns and end-use totals and
   added therms to kW. Mutation reproduced: removing the once-only skip in
   `annual_energy_for_prefixes` fails `hvac_energy_counts_end_use_totals_once`
   (6 kWh against 3 kWh) and the corpus test; the pinned unit test discriminates.
   The pin is applied on both sides: HARES's first zone temperature and the
   reference's first indoor temperature must equal the pin (`mod.rs` start
   checks), and both pass at head. The duct-loss mechanism is verified in the
   vendored OCHRE (`hpxml.py:970-1006`, `HVAC.py:166-173`) and OS-HPXML
   (`hpxml.rb:7078-7087` returns nil without a distribution system;
   `hvac.rb:5606` is_ducted), so the 33.6 % is explained: 0.664 = 1.05 to 1.085 /
   (1/0.64). Bands are the residual plus 1 %: 34.6 against 33.60, 33.3 against
   32.30. Nothing else is loosened (only the resstock peak lock, 25.7 against
   25.39, predates the branch); the earlier overrides were removed and the
   numbers pass at the defaults. The vendored OCHRE pin (8681122b) is the base's.
   See C and D for what the triage leaves open. `WaterHeater.py:456` reads
   `self.hp_cop` before it is set at `:476`: confirmed.
4. **Gable guard.** The span sensitivity guard works (a footprint near a
   square falls to the hip with a warning; it is pinned) and the fallbacks are
   accurate. The corpus attack found B. Synthetic shapes in the tests (garage,
   no floors, wing, near-square, small and oversized ends) are all caught.
5. **7x equivalent battery (D-011).** The EBM energy window is the zone air
   capacitance times the deadband, taken from what the solver integrates, so
   with the multiplier as the lumped mass proxy (OS-HPXML's own, with its
   furniture nodes) it describes the storage the zone shows; physically
   consistent. The consumer (`~/src/HARES_CALIBRATION_SYSTEM`, `i08-consumer`)
   reads no equivalent-battery column or `Min/Max Energy` by name (grep of
   `calibration/`). What changes there is indirect, from the heavier zone: the
   MEASURED shed tables in `calibration/ami2/functions/_flexibility.py`
   (heating and cooling fractions, `UNWINDOWED_FRACTION`); the
   `dr_actuation.py:176-215` measurements; the engine identity pinned in
   `calibration/baseline.py` and the stored `results/matrix` cells (engine
   sha256 changes); `hares_ua_xval.py` UA recovery. None re-run here.
6. **Goldens.** All seven in their own commits, `git_dirty` false, `git_head` the
   previous commit, parent chain 15d1e93a, 9afd978e, 1dcbee99, cf7ec13a,
   10fdcc2c, 1dc3a4a4, 471ed734. Every delta has a cause per commit; the sums
   check (cf7ec13a gas 38.99 against the listed causes 39.0; cooling -37.30 in
   both; consumer_shape cooling -59.99 against -60.05). Identical at head in
   default and dst modes.
7. **DIVERGENCES D-008 to D-011.** Accurate, pinned tests all exist
   (14 of 14 names found); D-009's corpus claim is contradicted by B. The
   numbering gap D-005 to D-007 is deliberate (fix-od30).
8. See the checks list. Failures: pyright (A).
9. **Fold.** Cherry-picking the 38 commits onto db5efe80 conflicts in the first
   five logical commits: 460d89a4 (5 files: `dwelling/mod.rs`,
   `schedule_path_contract.rs`, `hpxml_schedule.rs`, `schedule_resolve.rs`,
   `hpxml_schedule_tests.rs`), bca0651a (3: `solver_builder.rs`,
   `hpxml/building.rs`, `hpxml_parsing_tests.rs`), cb21d63a (12: `autosize.rs`,
   `conversions.rs`, `dwelling/mod.rs`, `synthetic.rs`, `environment.rs`, two
   timezone and weather test files, `climate_zone.rs`, `resolve_hvac.rs`,
   `resolve_loads.rs`, `resolve_water_heater.rs`, `hpxml_defaults_regressions.rs`),
   644a5b67, 8a5564f0 (`equipment_zone_resolution.rs`), 398097d2 (`hpxml/mod.rs`,
   `validation.rs`), then the seven golden captures (data). The head moved 14
   commits since 2f85338d. A real fold needs the re-capture of all goldens at the
   folded code. `fix-od30` (tip 4acf4c4a; 8ea79461 is not its ancestor):
   `git merge-tree` against the folded branch conflicts in `conversions.rs`,
   `dwelling/mod.rs`, `hpxml/building.rs`, `resolve_loads.rs`,
   `resolve_water_heater.rs`, `schedule_resolve.rs`, `hpxml_parsing_tests.rs`,
   `schedule_integration.rs`, `tests/parity/mod.rs`, the seven goldens and
   `DIVERGENCES.md`. The register conflict is textual and trivial: od30 appends
   D-005 to D-007 after D-004 and this branch appends D-008 to D-011 at the same
   place; keep both in order.
10. **Shipped text.** See F, G, H. Nothing else found per commit or in the final
    diff (added lines, excluding goldens and parquet).

## Files

- `/home/rich/src/HARES/.shipwright/initiatives/I-08/verification/review-OR-18c-de561181.md`
- `/home/rich/src/HARES/.workstrees/fix-or18c/python/ochre_next/_hares.pyi`
- `/home/rich/src/HARES/.workstrees/fix-or18c/crates/hares-io/src/hpxml/zone_geometry.rs`
- `/home/rich/src/HARES/.workstrees/fix-or18c/tests/parity/mod.rs`
- `/home/rich/src/HARES/.workstrees/fix-or18c/tests/fixtures/parity/cz5a_minisplit_gas_wh/building.xml`
