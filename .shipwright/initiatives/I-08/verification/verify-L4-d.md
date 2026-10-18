# Verification L4-d: consumer (08) and numeric fixes (09), B-14 in depth

Scope: `ledger/08-consumer.md` and `ledger/09-numeric-fixes.md` against
`plan.md` sections 1 to 3, `decision-log.md` and `ledger/00-schedule.md`.
Engine evidence read with `git show`/`git grep` at `afa9a071` (HEAD),
`21d90588` and `ebdb1a17` as each entry cites; consumer evidence at
`5c651bc` and `c71c542` on `i08-consumer`; EnergyPlus at
`vendors/EnergyPlus` `f7c103167a`. `00-schedule.md`, `08-consumer.md`,
`plan.md` and `decision-log.md` were rewritten while this round ran; every
finding below was re-checked against the text as it stood at 20:00
(schedule with rows 0e, 8a, 9a, 10a, 24h and CN-17 gated on OR-11).
`09-numeric-fixes.md` did not change during the round.

Counts: 0 blockers, 8 majors, 13 minors.

## Findings

### Major

**1. B-14 commit 4 `si_census`: the pathspec matches no file, so the census can never fail.**
Entry: `09-numeric-fixes.md` B-14, acceptance test commit 4, `si_census`.
Evidence: the command is `git grep -nE '<regex>' -- 'crates/*/src' 'python/hares'`.
A git pathspec with a wildcard is matched against the whole path, so
`crates/*/src` matches only a file literally named `src`:
`git grep -c 'capacity_kw\b' -- 'crates/*/src'` prints nothing, while
`-- 'crates/*/src/**'` prints `pv/config.rs:45`, `pv/array_config.rs:19`
and the rest. At `afa9a071` the census regex prints 188 lines with the
written pathspecs (all from `python/`) and 7805 with
`'crates/*/src/**'`.
Correction: write the pathspecs as `':(glob)crates/*/src/**'
':(glob)python/hares/**'`; the verification step records the line count
and the reviewer's per-line classification; add a self-check that the
command prints at least one known boundary line (for example the
`Total Electric Power (kW)` catalogue declaration), so an empty match set
fails.

**2. B-14 census is incomplete: input surfaces, control-layer code and run-state computations are missing.**
Entry: B-14 Evidence (commits 2, 3, 4) and "Boundaries that keep their units".
Evidence (all at `afa9a071` unless stated), each holding or accepting kW,
kWh, kVA, kVAR, miles or mph and named nowhere in B-14:
- Input surfaces that need a rename and a `REMOVED_KEYS` row (plan §2.7
  says every renamed key fails naming its replacement on every surface):
  - `crates/hares-python/src/py_control.rs`: the Python `ControlSignal`
    dict constructor and serialiser read and write `active_power_kw`,
    `reactive_power_kvar`, `max_power_kw`, `power_kw`
    (`:425-449, 550-551, 590-592, 648, 787`); commit 4 names only
    `py_actor.rs`.
  - `crates/hares-python/src/py_gym.rs:114-128` (action keys
    `active_power_kw`, `p_setpoint_kw`, `max_power_kw`,
    `reactive_power_kvar`) and `:496` (observation `net_electric_power_kw`);
    commit 4 names only `python/ochre_next/rl/gym_env.py`.
  - the protocol bridge's JSON signal payloads
    (`crates/hares-equipment/src/protocol_bridge/handler.rs:64-80, 159`:
    `{"PowerSetpoint": {"active_power_kw": ...}}`, deserialised as
    `ControlSignal`): after the rename an old payload fails as a serde
    "missing field", not naming its replacement.
  - `Dwelling.estimate_pv_capacity(target_kw, min_kw=2.0, max_kw=14.0, ...)`
    (`py_dwelling.rs:1792-1799`, `_hares.pyi:2511`); commit 3 names only
    `size_pv_system` and `required_main_panel_ampacity`.
  - `TariffEvaluator.step(net_power_kw, ...)` in Python
    (`py_tariff.rs:249-259`) and the Rust `TariffEvaluator::step` input it
    forwards to (the dwelling calls it with `net_active_kw`).
- Control layer not named in commit 4: `crates/hares-control/src/signal.rs:21-97`
  (`ControlSignalConstructors::power_setpoint(active_power_kw, ...)`,
  `power_limit(max_power_kw, ramp_rate_kw_per_s)`), `dispatch.rs`,
  `compat.rs`; the DR compliance actor (`actors/dr_compliance.rs`:
  `max_kw`, `max_power_kw`, `ramp_rate_kw_per_s`) and the occupant actor
  (`actors/occupant.rs`: `EquipmentBehavior.power_setpoint_kw`,
  `Occupant.active_power_kw`); commit 4 covers only the BMS and EV-driver
  actors. `ActorSeed::Bms { max_charge_kw, max_discharge_kw }` and
  `ActorSeed::Ev { capacity_kwh, max_charge_kw, fuel_economy_kwh_per_mi }`
  (`crates/hares-equipment/src/lib.rs:34-47`) and the hard-coded EV-driver
  arguments the dwelling passes (`dwelling/mod.rs:7967-7983`: 30 mi/day,
  30 mph, 20 mi, 6.6 kW) and the Python builder passes
  (`py_dwelling.rs:1319-1346`: `fuel_economy = spec.capacity_kwh /
  spec.range_miles`, `30.0 // average_speed_mph`, `20.0 //
  range_anxiety_miles`).
- Run state and computation not named in commit 3:
  `crates/hares-types/src/zip.rs` (`p_kw`, `reactive_kvar`, the ZIP load
  power); `crates/hares-physics/src/pump.rs`
  (`compute_ground_loop_pump_power_kw`); `crates/hares-envelope/src/electrical_solver.rs`
  `net_reactive_kvar` (`:87, 118, 206`) and its state payload
  (`:214-215`, pushed into the checkpoint); `invariants.rs`
  (`InvariantChecker.battery_capacity_kwh`); `observer.rs`,
  `observer_capture.rs` and `diagnostics.rs` (`electrical_net_kw`,
  `electrical_net_kvar`, `pv_generation_kw`); `schedule_resolve.rs`
  `DefaultScheduleProfile.max_kw` and `determine_max_kw`; the HPXML
  annual-energy path (`resolve_loads.rs`, `resolve_pool.rs`:
  `annual_electric_kwh`, `annual_gas_therms`, `child_load_kwh`;
  `xml_helpers.rs` `child_energy_kwh`; the plug-load EV's `range_miles`
  and `EV_FUEL_ECONOMY` in miles per kWh, `resolve_loads.rs:868-874` at
  `21d90588`); `defaults.rs` `VehicleMappingEntry { capacity_kwh,
  charger_power_kw }`; `PyTariffTelemetry` (`py_telemetry.rs:212-250`,
  reading the live `BillingState`); the gym's `reactive_power_kvar`
  observation (`gym_env.py:223, 283-284`) and its observation bounds in kW
  (`:221-224` total power `(-100, 100)`, `:235-236` equipment power
  `(0, 100)`), which must scale with a W observation or the declared
  `Box` is wrong by 1000.
- The conversion helpers `power_kw_to_w` and `power_w_to_kw`
  (`crates/hares-physics/src/units.rs:138-151`, a hand-written
  `WATTS_PER_KILOWATT` factor, not `uom`; 83 and 96 call sites across 20
  files) and `energy_therms_to_kwh`, `energy_kwh_to_therms`,
  `power_watt_to_kwh_per_year`, `power_btuh_to_therms_per_year`: B-14
  says hand-written `* 1000.0` conversions are deleted but never names
  these, which is how most of them are written.
Correction: add each item to the commit that owns it (input surfaces with
their `REMOVED_KEYS` rows and scopes; actors and `ActorSeed` to commit 4;
run state, physics helpers and observers to commit 3); say which `units.rs`
helpers are deleted and which are rewritten on `uom`; add the gym
observation bounds to commit 3's change; list every boundary that keeps a
unit (HPXML annual consumption read in kWh/yr and therm/yr, converted at
the read) under "Boundaries that keep their units" with its conversion
site.

**3. B-14 commit 3 "exactly three places" contradicts boundaries that publish run state in kW.**
Entry: B-14 "Change, commit 3", third bullet ("No other code converts
these values") against "Boundaries that keep their units".
Evidence: the HELICS publications register `units=kW`/`units=kvar` and
publish `total_power_kw`, `reactive_power_kvar`, `equipment_{i}/power_kw`
and `pv_generation_kw` read from `telemetry()`
(`python/ochre_next/helics/dwelling.py:384-440`), and
`python/ochre_next/helics/fleet.py` publishes `aggregate_power_kw` and
`aggregate_reactive_kvar`. After commit 3 telemetry is in W and VAR, so
either a fourth conversion exists or the federates receive W labelled kW.
`PyTariffTelemetry` (`py_telemetry.rs:212-250`) and the gym observation
have the same problem.
Correction: name every kept-unit reader of run state with its one `uom`
conversion (HELICS publisher, tariff telemetry, fleet rollup) or move it to
SI with its declared unit changed (`units=W`); add an acceptance test that a
HELICS publication of `der_pv_battery_900s`'s inputs equals the telemetry
W value converted to the published unit.

**4. Renamed Python parameters that can be passed positionally change magnitude silently.**
Entry: B-14 commits 2, 3, 4, Python constructors and functions.
Evidence: `PV(name, capacity_kw, tilt, azimuth, ...)`
(`py_equipment.rs:627`), `Battery(name, capacity_kwh, max_charge_kw=None,
max_discharge_kw=None, ...)` (`:292-296`), `ChargingStrategy.v2g(min_soc,
max_export_kw, price_threshold)` (`py_enums.rs:1699`),
`ChargingStrategy.solar_surplus(min_charge_rate_kw, ...)` (`:1721`),
`ControlSignal.power_limit(target, max_kw, ...)` and
`power_setpoint(target, active_power_kw, ...)` (`py_actor.rs:695-706`),
`ev_drive(target, kwh, ...)` (`:814`), `size_pv_system(usable, target_kw,
*, ...)` and `required_main_panel_ampacity(target_ac_kw, *, ...)`
(`py_pv_sizing.rs:595, 657`), `estimate_pv_capacity(target_kw, ...)`,
`TariffEvaluator.step(net_power_kw, ...)`. The `REMOVED_KEYS` lookup sees
keywords only: `PV("pv", 5.0, 25.0, 180.0)` keeps running after commit 2
and builds a 5 W array.
Correction: in the commit that renames each parameter, make it
keyword-only (`*` before it in the `#[pyo3(signature)]`), so a positional
call raises `TypeError`; extend
`test_equipment_constructor_removed_keyword_names_replacement`,
`pv_sizing_returns_watts` and
`removed_control_signal_keyword_names_its_replacement` with the positional
form; state the change in the `docs/python.md` changes section.

**5. The resolved-model guard of commit 2 is unspecified, partial, and breaks at row 94f.**
Entry: B-14 "Fixtures added by this entry" (`tests/fixtures/golden/resolved/<manifest>.json`),
`resolved_models_are_unit_invariant`, and the commit 2 guard.
Evidence:
- No tool captures it: `frame_golden` has no resolved-model output
  (`tools/frame_golden/src/manifest.rs` fields at `afa9a071`), plan §2.7
  declares `ResolvedModel` with no `Serialize` and no JSON form, and the
  first commit "changes no engine code"; for the fleet manifests the test
  must also build each home's blueprint the way the fleet adapter does.
- The test is a permanent `hares-core` test over frozen JSON of the whole
  `ResolvedModel`, but later numeric rows change that model:
  `11-other-defects.md` OD-10's numeric commit (row 94f) records the
  infiltration height, flue and shielding rules "in `ResolvedModel`", and
  OD-2 site 6 (row 94e) changes setpoint sources. No entry re-captures
  the JSON, so `[test-debug]` turns red at row 94f.
- It covers only the five parameters of plan §2.7's table (`nameplate_w`,
  `energy_capacity_j`, `power_capacity_w` from `max_charge`,
  `battery_capacity_j`, `max_charging_power_w`); crankcase heater,
  inverter capacity, `max_discharge`, generator, event and scheduled load
  powers, fuel economy, charger VA and V2L/V2G are not in it, so "every
  value is unchanged in SI" is not what it proves. The table also claims
  `der_pv_battery_ev_900s` reaches `fuel_economy_j_per_m`, but the
  plug-load EV sets `fuel_economy_kwh_per_mi: None`
  (`resolve_loads.rs:902` at `21d90588`).
Correction: specify the capture (a `frame_golden capture-resolved`
subcommand writing `serde_json` of `ResolvedModel`, with the `Serialize`
derive added by `06-runtime-memory.md` RT-6 or IN-2, not by this numeric
row); compare only `equipment[*].parameters` and delete the fixture and
test in commit 2 after it passes (it guards one commit), or make every
later row that changes `ResolvedModel` re-capture it; add
`equipment_config_values_are_unit_invariant`, one case per row of the
commit 2 table, each building the config through its real input path at
the parent and after and comparing the SI value; correct the
`fuel_economy_j_per_m` claim.

**6. `equipment_config_fields_are_si` cannot compile where it is placed.**
Entry: B-14 acceptance test commit 2.
Evidence: the test is "a `crates/hares-io` test" asserting over
`hares_io::input_schema().overrides`; the function is
`hares_core::input_schema()` (plan §2.7 "Other built-now capabilities";
`12-inputs.md` IN-3 line 192), assembled in `hares-core` because it is
the lowest crate that sees every described type. `hares-io` does not
depend on `hares-core`, and `SyntheticEventLoadConfig` (one row of the
table) lives in `hares-core` too.
Correction: make it a `crates/hares-core` test calling
`hares_core::input_schema()`.

**7. The commit 3 and 4 golden guard's relative difference is undefined near zero.**
Entry: B-14 "The guard for commits 3 and 4".
Evidence: "Each moved column is recorded with its largest relative
difference; a relative difference above 1e-9 ... stops the entry".
`der_pv_battery_900s` runs `BmsMode::SelfConsumption`, which drives net
grid power towards zero, so `Total Electric Power (kW)` and battery power
cells sit at values like 1e-12 kW where a last-bit change of an
accumulation in W versus kW gives a relative difference near 1.
Correction: define the measure against the column's scale,
`|a - b| <= 1e-9 * max_t |a_t|` per column (and the factor check on the
ratio of column maxima), and record both measures.

**8. CN-3 misses the consumer's reads of telemetry fields that B-14 commit 3 renames.**
Entry: `08-consumer.md` CN-3, the B-14 bullet and its census.
Evidence (consumer `5c651bc`): `calibration/cosim.py:81`,
`calibration/coupling.py:393` and `calibration/scale.py:316` read
`dw.telemetry().reactive_power_kvar`, which commit 3 renames
(`DwellingTelemetry.reactive_power_kvar` becomes `reactive_power_var`, in
VAR); text naming it at `cosim.py:25`, `coupling.py:359-361`,
`scale.py:319` and `suite_ami2.py:2016`. CN-3's census regex
(`\["(net_electric_power_kw|mean_power_kw)"\]|step\(\)\["[a-z_]*_kw"\]`)
cannot see attribute reads. `calibration/coverage.py:106` classifies
`^power_limit_kw$`, which commit 2 renames.
Correction: add the three reads (converted to the consumer's kVAR through
the same helper), the text sites and the `coverage.py:106` rule to CN-3,
and extend the census with `telemetry\(\)\.[a-z_]*_(kw|kvar)\b`.

### Minor

**9.** B-14 commit 2 and commit 3 add `uom` with `cargo add uom -p
hares-core -p hares-python` and `cargo add uom -p hares-types -p
hares-tariff`. `cargo add` rejects a repeated `-p` ("the argument
'--package [<SPEC>]' cannot be used multiple times", measured in a
scratch workspace). Correction: one `cargo add uom --workspace -p <crate>`
per crate. (The premise holds: none of the four depends on `uom` at
`afa9a071`; `hares-equipment` and `hares-io` do.)

**10.** `raw_equipment_keys_are_si` greps `const KEY_...`, so it misses
`PHASE_POWER_SUFFIX_A` and `PHASE_POWER_SUFFIX_B` (`"_power_kw"`,
`event_load.rs:136, 141` at `21d90588`, joined at `:1838-1840`), which build the phase keys.
Correction: grep `const [A-Z0-9_]+: &str = "[A-Za-z0-9_]*(_kw|...)"`.

**11.** The `si_census` regex alternative `wh` matches water-heater
identifiers (`gas_wh`, `resistance_wh`, `tankless_wh`, `heat_pump_wh`,
`elec_res_wh_spec_from_py`; 171 lines at `afa9a071`), none a unit.
Correction: list the water-heater prefix as a boundary class or drop `wh`
and rely on `kwh`.

**12.** An SI guard already exists: `make check-si-guard` runs
`crates/hares-equipment/tests/si_guard.rs`, a `syn`-based identifier scan
with per-file allowlists for `therm` (`scheduled_load.rs` among them) and
`mph`. B-14 adds parallel `git grep` checks and leaves those allowlists
stale once therms and mph leave the code. Correction: extend `si_guard`
with the kW, kWh, kVA and kVAR rules and shrink its allowlists in the
commits that remove the units.

**13.** `uom` 0.38 has `energy::btu_it` but no therm and no BTU-per-hour
power unit (`si/power.rs`), so `Unit::from_si` for `therms/hour` is not
"one uom conversion" as written. Correction: define `therm` with uom's
`unit!` macro (or state the composed form) in commit 3.

**14.** The commit 3 and 4 guard says `month_boundary_billing_3600s`
reaches the BMS and EV-driver actors; its building
(`tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh/building.xml`) has no
PV, battery or EV. Correction: name only the two DER goldens for the
actors and `month_boundary_billing_3600s` for billing.

**15.** Commit 3 changes `metrics_from_table`'s two equality tests to a
relative tolerance of 1e-12 per metric; a net metric with cancellation
(net energy of a PV home) can exceed it from per-step last-bit
differences. Correction: bound by the gross magnitude (sum of absolute
step values) of the metric.

**16.** Commit 3 moves equipment run state to SI while control signals stay
kW until commit 4; it does not state that each equipment converts an
incoming signal value once at application until commit 4, which commit 4
then deletes (commit 4 names only the actors' electrical-summary
conversion). Correction: state the transitional conversion in commit 3.

**17.** The Python EV keyword `max_charging_kw` is not a field of
`EvConfig`, so the `<ConfigType>.<key path>` row form does not name it.
Correction: list the row explicitly (`EvConfig.max_charging_kw` naming
`EvConfig.max_charging_power_w (W)`).

**18.** Commit 4 says an actor's declared variable name "is a reporting
name, which the census lists as a boundary", but the boundary list names
neither actor variables (`pv_kw`, `charge_kw`) nor equipment telemetry
keys (`crates/hares-types/src/telemetry_keys.rs`: `fan_kw`,
`compressor_kw`, `total_power_kw`). Correction: add both to the list.

**19.** `08-consumer.md` CN-4's Dependencies omit OD-2 site 6, OD-10's
numeric commit, OD-20 and FL-5 (the schedule order is right).
Correction: list them.

**20.** No consumer row owns the contract drift of engine rows after CN-3
(M-4a, M-4b and any CI fix entry before M-4b); CN-15 checks
`[hares-contract] PASS` but its change does not update the contract, and
a CI fix that moves numbers after B1 has no rule. Correction: assign the
drift to CN-15 and state that a CI fix entry that moves numbers stops the
closing sequence for an operator decision on a second baseline.

**21.** Citation order: B-14's table gives `ChargingStrategy.solar_surplus`,
`.v2g` at `py_enums.rs:1699, 1721`; 1699 is `v2g`, 1721 is
`solar_surplus`.

## Verified correct

Consumer (`08-consumer.md`):
- Census of engine imports (CN-13): every `ochre_next` import site at
  `5c651bc` (`paths.py:83`, `stock.py:331`, `cvr_actuation.py:126`,
  `dr_actuation.py:77, 174`, `tariff.py:71, 82`, `forward_model.py:260`,
  the three DER tools, `suite_ami2.py:799, 850, 1737, 1956`,
  `provision_data.py:65-66`) and text sites match `git grep`; indirect use
  goes through `paths.engine()`.
- Census of construction sites (CN-1): every `from_hpxml`,
  `DwellingBlueprint.from_hpxml`, `DwellingConfig`, `SteppableFleet`,
  `initialize` and `simulate` site at `c71c542` is listed; engine classes
  used (`ControlSignal`, `Priority`, `ElectricTariff`, `parse_epw`,
  `VehicleId`, `EvArchetypeId`, `PV`, `DRLevel`) need no B-14 change
  except `PV`, which CN-3 covers; the step-dict reads CN-3 lists
  (`dr_actuation.py:257`, `cosim.py:94, 107`, `coupling.py:412`,
  `suite_ami2.py:822, 879, 1767`) and `cvr_actuation.py:137`
  (`mean_power_kw`) are exact; `der_inject.py:169` writes
  `surplus_deadband_kw` and the HPXML it writes keeps W for PV and battery.
- Engine-under-test mechanism: the maturin pin extraction prints
  `maturin==1.15.0` from `uv.lock` at `afa9a071`; the engine's
  `build-system.requires` is `maturin>=1.15.0`, so the constraint is
  needed; `RESAMPLE_OVERRIDES` is `None` in `grid.py`, so the CN-13
  consumer passes no key IN-6 removes before PY-8.
- Gate: `run_all` exits non-zero only on failures, so the gate reads
  `gate.txt`; `hares_contract_check.py` prints `SKIP` and returns 0
  (`:52-58`) and `PASS` at `:142-144`; the gate case skips on
  `not paths.have_repo()` (`suite_calibration.py:264-278`); CN-17 makes
  `contract.txt` require `PASS` and the skip set exact, so the gate cannot
  pass on skips, a `SKIP` line or a new failure.
- Drift assignment matches the current schedule: CN-17 rows 1 to 19 on
  OR-11's commit, CN-13 rows 16b to 16h and 20 to 26, CN-1 rows 28 to 71a,
  CN-14 rows 79 to 81, CN-3 rows 83 to 94i.
- One re-baseline (CN-4, B1, after every numeric row) and one pin advance
  (CN-15, after the operator's push, `Engine:` trailers until then).
- B0 re-check: `8b09ac4a` is row 12's engine plus a tool-only change, so
  its capture is cumulative for rows 1 to 12, with bisection over
  `1d60f478..75a182a0`.

Numeric (`09-numeric-fixes.md`):
- EnergyPlus weather: `WeatherManager.cc:3043-3125` (point weights
  `Interpolation(ts) = ts/N`, `wvarsLastHr` from hour 24 or hour 1 when
  `LastHourSet` is clear, `NextHr` wraps to hour 1 of the same day,
  precipitation divided by N, `calcSky` on interpolated inputs, snow from
  the hour), `:2359` (`LastHourSet` cleared on rewind), `:8343-8375`
  (`SolarInterpolation`: N=4 0.75, 1, 0.75, 0.5; N=6 4/6..3/6; N=3 5/6,
  5/6, 0.5; odd N middle `1 - 1/(2N)`), `:715-722` (sub-hourly interval
  mismatch is an error), `:2816-2836, 2856-2859` (Feb 29 skip, actual-year
  fatal, weekday advance), `:7896`, `:8456`, `:872-876`; the B-1a and B-1b
  worked values (700, 800, 600, 400; 200, 100, 125, 150) follow from
  them.
- Day types after a skipped Feb 29 follow the calendar per
  `leap_day_day_types` (2024-02-28 Wednesday, 03-01 Friday, 03-02
  Saturday; 480 to 384 rows; 96 rows in CN-14's 3600 s case).
- Warm-up: `HeatBalanceManager.cc:3330-3470` (four per-zone checks, the
  1e-4 W threshold and 100 W floor, min and max days), `:2886-2889` (zeroed
  previous day), `:684-697` (min above max resets the max);
  `Energy+.idd.in` N2 to N5 defaults 0.04, 0.4, 25, 1. HARES defects at
  `ebdb1a17` (`mod.rs:2725, 2740, 5039-5043, 1812`) and the 0-means-two-
  things evidence at `872cf289` are real.
- B-4 (`evaluator.rs:618-634`, `mod.rs:6520-6526`, `environment.rs:777`,
  `mod.rs:6581`), B-7 (`metrics.rs:593-604`), B-8 (`fluid_solver.rs` at
  `21d90588`), B-13 (`conversions.rs:147-166`, `envelope_lut.rs:571-576`)
  are real at the cited code.
- B-14 commit 2 table: every field and line at `21d90588` matches; the
  raw-key constant claims (battery and generator `#[cfg(test)]`, EV raw
  path refused by `init()`) hold; `register_pv_roof_shading`'s
  `cap * 1000.0 / 420.0 * 2.0` and key read are at the cited lines.
- B-14 commit 3 and 4 citations at `afa9a071` (`billing.rs:75-99`,
  `metrics.rs:391-421`, `StepResult` and `net_active_kw`,
  `environment.rs:47-69`, `CHECKPOINT_VERSION = 8`, `telemetry.rs`,
  island and ports dicts, gym keys, `actor_registry.rs:383-541`,
  `control_signal.rs` fields including `ramp_rate_kw_per_s`) match; the
  numbers in its tests hold (0.1 therm/h is 2930.711 W; 0.3 kWh/mi is
  671.08 J/m; the therm is 1.055e8 J; SAE J1772 L2 is 19 200 W).
- `uom` dependency premise and conversion forms (`Energy / Length` read in
  `newton`) are valid in `uom` 0.38; the `REMOVED_KEYS` scopes B-14 adds
  match plan §2.7 (IN-6 creates four scopes, B-14 adds `Observation`,
  `ActorParameter` and `ControlSignal`).
- Checkpoint: equipment blobs carry no per-equipment version, so commit 3's
  `CHECKPOINT_VERSION` rise covers them; actor blobs carry
  `schema_version`; no `ControlSignal` is stored in a checkpoint, so
  commit 4 needs no format rise.
- Report columns keep their declared units through `Unit::from_si` at the
  row plan, and plan §2.1 and §2.2 state the same final form.
- Each numeric commit has its own row (94c, 94g, 94h) in the numeric
  block; commit 2 lands after IN-8 and before IN-9; commits 3 and 4 follow
  OD-10's numeric commit and precede CN-3; every dependency
  (IN-2, IN-3, IN-6, OD-2, OD-8, PY-2, PY-7, PY-9, RT-6, RT-7, OR-1, OR-10,
  OR-15, FL-3) is scheduled earlier; the file preamble requires the
  physics suites of plan §3 rule 2 before and after each.
- Fixtures exist at `afa9a071` or are created by a scheduled entry:
  `base-pv-battery.xml`, `base-pv.xml` (4000 and 1500 W),
  `cz6b_pv_battery_ev/` (PV 4000 W, battery 6000 W, plug-load EV),
  `BEopt_example_schedule.csv`, the Denver TMY3 EPW,
  `ev_thermal_regressions.rs` with `bldg0000007`, the ResStock weather
  files B-1c, B-9, B-12 and IN-9 name, the weather stations
  (`G0600770`, `G5300330`, `G0900090`, `G0900010`, `G2500170`), the 2025.1
  metadata parquet in the cache with the stated size; `month_boundary_billing_3600s`
  (BR-14, row 16e), `fleet_same_offset_300s` (FL-1, row 50) and
  `resstock_2025_fleet` (B-9a, row 90a) precede their users; the physics
  suite targets of plan §3 rule 2 all exist.
