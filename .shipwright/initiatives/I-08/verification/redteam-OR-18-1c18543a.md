# Red team: OR-18 commit 1c18543a

Verdict: ATTACK-SUCCEEDED

Counts: 0 blockers, 3 majors, 5 minors.

Method: scratch worktree at 1c18543a plus a second scratch worktree at 69950a98 for comparison. Reproductions were written as throwaway tests (never committed). Every sample file in `vendors/OCHRE/test/OS-HPXML Sample Files` that carries a WaterHeatingSystem (424 files) was loaded through `Dwelling::from_hpxml` and stepped 24 steps; 343 ran, 81 failed, and none of the 81 failures names a water heater, a zone, a location or the dehumidifier (they are HVAC, enclosure and schema gaps that predate this commit). Nine water-heater location files and nine control files were run for 48 h on both commits through `SimulationEngine` and compared by `energy_by_end_use`.

## Major findings

### M1. Heat pump water heaters in an ambient placement take the dry-bulb as the wet-bulb for the COP and capacity curves

Evidence: `crates/hares-equipment/src/water_heater/heat_pump_wh.rs:373-381` (`zone_wet_bulb_c`), consumed at `:961-994` (COP and capacity curves are wet-bulb driven) and `:1083` (SHR).

The commit returns the ambient dry-bulb as the wet-bulb for every `AmbientLocation`. The comment calls this "the same fallback the zone-not-found branch uses", but for "other housing unit" the water heater used to read the conditioned zone's real wet-bulb, and for "other exterior"/"outside" the environment already carries `weather.outdoor_wet_bulb_c`, which goes unused.

Reproduction (scratch test `rt_scratch` appended to heat_pump_wh.rs): same HPWH config, conditioned zone 22 C with a humidity ratio giving a 14 C wet-bulb.
- zone-resolved ("living space", zone_id 1): COP 2.130
- "other housing unit", zone_id None: COP 2.555 (+20 percent)

The curve sees 22 C instead of 14 C. Whole-dwelling effect: `base-dhw-tank-heat-pump-outside.xml` goes from 30.9 kWh of water heating (69950a98) to 116.8 kWh (1c18543a) over 48 h in January Denver. Part of that is the intended removal of the 21 C zone air, but the COP input is still wrong in the other direction in warm humid weather: outdoor 30 C / 21 C wet-bulb is evaluated at 30 C.

Fix: carry a wet-bulb per placement. For "other housing unit" use the conditioned zone's wet-bulb (`zone_wet_bulb_c` of the conditioned zone). For "other exterior"/"outside" use `weather.outdoor_wet_bulb_c`. For the three blended placements compute a wet-bulb from the blended dry-bulb and the blended humidity ratio (conditioned and outdoor), or state the OS-HPXML rule being followed and cite it. Add these as fields in `AmbientOtherSpaceTemps` computed once per step in `environment.rs` step 4b. Also pin the changed measurement-log delta with a test that fails if wet-bulb is replaced by dry-bulb.

### M2. The claim "no ZoneId(1) fallback anywhere" is false, and the only log of it was deleted

Evidence (non-test code, all at 1c18543a):
- Constructors: `water_heater/resistance.rs:150`, `heat_pump_wh.rs:222`, `gas.rs:157`, `indirect_tank.rs:115`, `tankless.rs:108`, `hvac/ideal_hvac.rs` (`new`), `hvac/furnace.rs:114,525`, `hvac/baseboard.rs:75` and the other HVAC constructors that formerly called `zone_id_from_config_or_default` each do `zone_from_config.unwrap_or(ZoneId(1))`. The commit only inlined the old helper and deleted its warning, so the fallback is now strictly silent.
- `crates/hares-core/src/dwelling/solver_builder.rs:125` and `:899`: `env.zones.first().map(|z| z.id).unwrap_or(ZoneId(1))`, plus `:1572` `unwrap_or(21.0)`.
- The `zone_id_explicit` flag remains, so observers still report the fallback, but nothing fails.

Reproduction (`crates/hares-core/tests/rt_add.rs`, `add_paths`): load `base.xml`, step 3, build a `ResistanceWH` from a typed config with `zone_id: None, zone_type: None`, call `init` with `latest_env()`. Result: `init` Ok, `descriptor().zone == Some(ZoneId(1))`, `add_equipment` Ok. Any Python or manual add of a water heater or HVAC unit that omits `zone_id` lands on the first zone whether or not it is the conditioned one. The commit fixed this for the dehumidifier only.

Fix: follow the dehumidifier. Make the zone `Option<ZoneId>` at construction and return a typed error from `init` when neither `zone_id` nor a classifiable `zone_type` resolves (water heaters), or when `zone_id` is absent (HVAC). Resolve through the conditioned zone where the equipment is by definition conditioned. For the solver builder, error on an empty zone list. Then update the ledger text so it states what was and was not removed.

### M3. A water heater in an "other ..." location cannot be added once stepping has begun

Evidence: placement sets `descriptor.zone = None` and `ports[1].zone = None` (`resistance.rs:333-336`, same pattern in gas, HPWH, indirect, tankless). `Dwelling::validate_candidate_equipment` (`dwelling/mod.rs` around 3720-3740) then requires an accumulator for every declared port in the frozen table.

Reproduction (`rt_add.rs`): after 3 steps, `add_equipment` of a `ResistanceWH` with `zone_type` "other heated space", "Other Heated Space " or "OTHER HOUSING UNIT" fails with "declares a Thermal port (zone=None, loop=None, domain=None) with no accumulator in the dwelling's port-slot table, and the table is frozen". The same add before the first step succeeds (`add_before_step`). At 69950a98 every one of these succeeded, because the unit sat on ZoneId(1). The Python add paths (`py_dwelling.rs:1146-1365`) hit this after any `step`.

Fix: an ambient-placement water heater should declare no thermal port at all (its standing loss has no modeled destination), or the frozen table should include a zone-less thermal sink. Add a regression test that adds each placement after step 1.

## Minor findings

### m1. Silent fallback when there is no conditioned zone

`crates/hares-core/src/environment.rs:681-686` uses `.unwrap_or(outdoor_temp_c)` for the conditioned temperature, and `water_heater/mod.rs:42-47` (`OtherHousingUnit`) falls back to outdoor when `conditioned_zone_c` is non-finite. Both contradict the no-silent-defaults rule. The dehumidifier errors in this situation; the water heaters and the ambient series should too (typed error from the manager when a building has no conditioned zone and any ambient placement is configured, or compute the series as `Option`).

### m2. Non-finite temperatures are masked by the floors

`f64::max` returns the non-NaN operand. Reproduction (scratch test in environment.rs, outdoor dry-bulb NaN on step 1): `outdoor_temp_c = NaN`, but `other_heated_space_c = 20`, `other_multifamily_buffer_space_c = 10`, `other_non_freezing_space_c = 4.44`. A water heater in those placements keeps running on a plausible finite ambient while everything else sees NaN. Fix: check `is_finite` on the two inputs in step 4b and return a typed error naming the step, or use a max that propagates NaN.

### m3. Synthetic closure check has false rejects and false accepts

`crates/hares-core/src/dwelling/synthetic.rs:1368-1394`.
- False reject: `parse_boundary_type` (line 465) accepts `Slab`, `FoundationWall`, `RimJoist` and `Door`, but only `Wall`, `Roof`, `Floor` count. Wall + Slab (a physical slab-on-grade box) and FoundationWall + Slab + Roof are rejected as open. Before this commit those only logged.
- False accept: a Wall plus Roof of area 0.0 passes (the category test ignores area), and a Roof whose `interior_zone` is "Attic" satisfies the check for the Conditioned zone, yet the error text says "synthetic zone 'Conditioned'".
Fix: count `Slab` as Floor and `FoundationWall` as Wall; require positive area; filter boundaries by `interior_zone == Conditioned` (or by each zone) before counting.

### m4. HPWH wall-heat default changed for locations other than living space (read from code, not run)

`heat_pump_wh.rs:705-713` now classifies the raw Location with `is_conditioned_location`. Previously `zone_type` was `zone_key(parse_zone_label(..))` and only `"conditioned"` gave 0.5. Now "basement - conditioned" and "crawlspace - conditioned" (parsed to Foundation, previously 0.0) and "other non-freezing space" (contains "non-freezing", so classed conditioned) get 0.5. For the ambient placements the wall share is then discarded because the zone is None, so a non-zero default is dead. The commit message does not list this behavior change. Fix: state it in the ledger measurement delta or restrict the default to locations that resolve to the Conditioned zone.

### m5. Multiple conditioned zones silently resolve to the first

`environment.rs:674-680` and `resolve_hvac.rs:1869-1875` take the first Conditioned zone. A building with two conditioned zones gets one ambient and the dehumidifier on the first. Not a defect for HPXML (one conditioned zone) but synthetic TOML allows zones; error on more than one or document the rule.

## Attacks that failed (held up)

- Location string variants: "Other Heated Space ", "OTHER HOUSING UNIT" classify correctly (`AmbientLocation::from_zone_type` trims and lowercases); "garage" with no garage zone is a typed error naming the water heater and the string; "outside" and "other exterior" map to outdoor.
- Floors: exactly 20.0, 10.0, 4.44 (the unit test and the manager agree; equality is stable).
- Conditioned zone not first: the dehumidifier resolves id 2 in `[foundation, conditioned]`; the manager finds the conditioned zone by type, not list position.
- Checkpoint and restore: the ambient series is recomputed each step from the zone state and weather, and the equipment keeps no ambient state, so nothing needs restoring.
- Energy accounting: for 18 sample files, `sum(per_end_use) == net == gross` on both commits. The 9 control files and the dehumidifier file are bit-identical between 69950a98 and 1c18543a. The 9 location files change only where expected (other heated space +0.04 kWh, buffer +0.15, non-freezing +0.18 on water heating over 48 h; tankless electric outside and indirect/combi outside unchanged).
- IdealHvac end use: follows the sign of the delivered capacity; zero (including negative zero, since neither comparison is true) preserves the previous end use. `ByEndUse` dispatch reads the end use left by the previous step instead of the previous `update_control`; I found no sample that makes this visible.
- Dehumidifier: `Dehumidifier::new` with no zone_id leaves zone `None`, `init` and `step` return typed errors naming the equipment; HPXML resolution errors with `NoConditionedZone`.
- Test suites: `cargo test -p hares-equipment --lib` (2364 passed) and `hares-io` `hpxml_parsing_tests` (56 passed) are green at this commit.

## Residual risk and gaps

- I did not run the full workspace suite or the fleet golden, and did not verify the claimed golden delta numbers against the measurement log.
- The 3.8x HPWH-outside energy change was reproduced but not judged against OS-HPXML output; M1 says the COP input is wrong, not that the final magnitude is.
- m4 is from code reading only.
