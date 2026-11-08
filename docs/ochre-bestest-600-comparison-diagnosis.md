# OCHRE BESTEST Case 600 comparison: the input identity ledger, the one-factor arms, and the verdict

The independent OCHRE runner (`scripts/ochre_bestest_600.py`) reports an
annual heating load of 4614.424385201 kWh against the ASHRAE 140-2023
Annex B8 Case 600 band [3993, 4504] kWh: 110.42 kWh (2.45%) above the
ceiling. The cooling load, 5746.562098747 kWh, sits inside [5432, 6162].
This report records the comparison's input identity ledger, the one-factor
arms measured over it, and the verdict: **no admissible input correction
closes the excess; the runner is not an acceptance oracle for the Annex
B8 bands**. The failure is retained and the bands are untouched.

Everything here is OCHRE-side evidence. The runner imports the vendored
OCHRE (`vendors/OCHRE` at 58f39d8746520786c26c02ce4c08618d3053c422) and
never the HARES engine, so nothing below supports a claim about HARES
itself.

## The baseline and its determinism

The runner rerun twice in a fresh process each time: the annual totals
agree to 12 significant digits (heating 4614.424385201 kWh, cooling
5746.562098747 kWh). Individual hourly rows carry float noise at the
5e-12 relative level from the linear solves; the noise's contribution to
an annual sum is about 1e-12 kWh. The one-factor arms' deltas are six to
nine orders of magnitude larger, so every attribution below is safe.

## The input identity ledger

Each row: the fixture's value and its source, the script's OCHRE-side
value and its source, and the disposition.

| Item | The fixture (tests/fixtures/bestest/600.toml) | The script's OCHRE construction | Disposition |
| --- | --- | --- | --- |
| Weather file | `[weather] epw_path` → `data/examples/USA_CO_Denver.epw` | `vendors/OCHRE/ochre/defaults/Weather/USA_CO_Denver.Intl.AP.725650_TMY3.epw` | **Identical bytes** (md5 4747d7fae85d1b1dccf7f66154ad8cb4): the same Denver TMY3, 8760 rows |
| Weather timestamps | The engine reads each record by its own month/day/hour stamp, the site's standard offset, never a running index (docs/PHYSICS_DECISIONS.md, the weather section) | `set_annual_index` discards the stamps and re-indexes the 8760 rows sequentially at :30 (the EPW hour-ending convention's midpoint); `resample_and_reindex` bins them to the next hour | **Mismatch, OCHRE's own convention**: schedule step n carries EPW row n-1. Row 0 doubles at steps 0 and 1; row 8759 (Dec 31, -19.4 C) never enters. One-factor arm I: -0.43 kWh heating, -19.15 kWh cooling |
| Duration | `[simulation] duration` = 31536000 s = 365 days | `duration = dt.timedelta(hours=8760)` | **Match** (8760 hourly steps) |
| Calendar year | `[simulation] start_time` = 2024-01-01T00:00:00-07:00 (leap) | `SIM_YEAR = 2023` (non-leap) | **Mismatch, unrepresentable**: OCHRE's `set_annual_index` raises "Cannot parse data for a leap year" for a 2024 start. The label only shifts the solar-position season phase on the same TMY3 rows; arms at 2022/2021 move the heating load +0.34/+0.77 kWh |
| Warmup | The `[simulation]` comment names the default converging warm-up (the EnergyPlus criteria), prepended and replayed until the loads converge | A fixed 24 h warmup consumed from the year's head: the reported window is 8736 h (Jan 2 through Dec 31) | **Mismatch, measured**: the warmup is converged by 24 h (extending it to 168 h and 336 h moves the annual load -0.02 kWh), but the consumed window drops Jan 1's 37.3 kWh of heating. The prepended-warmup arm (the full year reported): +36.88 kWh heating |
| Schedule indexing (gains, setpoints) | Constant 200 W gains, occupancy 0, setpoints 20/27 C, deadband 0 (the `[schedule]`, `[setpoints]`, `[simulation]` tables) | The same constants written onto every schedule row (`_build_schedule`) | **Match** |
| Internal gains representation | `internal_gains_radiant_fraction = 0.6`: 200 W, 100% sensible, 60% radiative / 40% convective (ASHRAE 140-2017 Addendum a Section 5.2.1.7; docs/PHYSICS_DECISIONS.md, "The BESTEST internal-gain radiant fraction is the standard's case input") | 200 W at the zone air node only: OCHRE hardcodes "Radiative Gain Fraction (-)": 0 (`utils/hpxml.py:1567`), folds convective and radiative into one sensible fraction (`Equipment/Equipment.py:80-87`, the FUTURE comment), and the Envelope schedule path injects "Internal Gains (W)" into `zone.internal_sens_gain` (`Models/Envelope.py:1274`) with no surface term | **Unrepresentable**: no OCHRE input carries the split. The runner reports the case at 0% radiant. Kept explicit in the comparison's metadata; it disqualifies the runner as an acceptance oracle |
| Materials | Per-boundary layers with thickness, conductivity, density, specific heat (the `[[boundaries.material_layers]]` tables) | `_MATERIALS_CSV` carries the same layers; the witness test checks each value and derives the R sums (1.789 / 2.994 / 25.254 m2-K/W) | **Match** (the `c_wall` runtime diagnostic checks the wall's capacitance against the fixture every run) |
| Windows | Two 6 m2 south windows, U 3.0 W/m2-K, SHGC 0.769 | `WINDOW_AREAS_M2 = [6.0, 6.0]`, `WINDOW_U_FACTOR = 3.0`, `WINDOW_SHGC = 0.769`, `Shading Fraction (-): 1.0` (no shading) | **Match** |
| Surface optics | solar_absorptance 0.6, emittance 0.9 on the walls and roof (the ground-facing floor carries none) | `SOLAR_ABSORPTANCE = 0.6`, `EMITTANCE = 0.9` on every exterior surface | **Match** |
| Films | The engine's interior/exterior film regime is its own (TARP-family, with the engine's LWR treatment) | OCHRE's TARP interior / DOE-2 exterior convection-only films; the lookup adds film R 0.9 to every boundary (the script's floor comment claimed 1.5; "Floor" is not in OCHRE's 1.5 list - corrected in this row) | **Model semantics, not an input**: documented; no OCHRE input changes it |
| Ground | The engine derives the ground series from the weather file: mean = the annual dry-bulb mean, amplitude = half the monthly-mean range, phase day 35 (`environment.rs`, `hares-physics/src/ground.rs`) | OCHRE's schedule carries the DOE-2 GTEMP correlation: a dry-bulb-derived sinusoid damped to about 55% of the monthly half-range and phase-lagged | **Mismatch, derivations differ**: measured below; one-factor arm G: +15.71 kWh heating, +9.62 kWh cooling |
| Mechanical gains | None declared | None set | **Match** |

## The measured OCHRE-side quantities

From the runner's own schedule construction (the 8760-row resampled
year):

- Annual dry-bulb average: 10.8755 C (the raw EPW row order: 10.8753 C;
  the resample's row-0 duplicate and final-row drop move the mean by
  0.0002 C).
- Monthly-mean dry-bulb half-range: 11.5776 C (the monthly means run
  from -0.070 C in February to 23.086 C in June on the schedule's
  index).
- The ground series OCHRE actually drives the floor with: mean 10.8202 C,
  ranging 4.6169 C (March) to 17.0999 C (September), a monthly half-range
  of 6.2413 C.
- The engine's derivation over the same EPW: mean 10.875342 C, amplitude
  11.576200 C, phase day 35 (-0.70 C at day 35, +22.45 C at day 217).

## The arms

One factor per arm against the runner's own construction, everything
else byte-equal. The harness drives the Envelope by per-step row dicts
(the script's own loop, verbatim, with `hvac_gain` reset in the deadband
branch exactly as the script's loop has it) and switches off the
Envelope's redundant internal schedule feed; the BASE control reproduces
the script's runner exactly (4614.424385201 kWh heating), proving the
harness equivalent. Each arm's delta is the named factor's annual
leverage.

| Arm | The one changed factor | Heating (kWh) | Cooling (kWh) | d heating | d cooling |
| --- | --- | ---: | ---: | ---: | ---: |
| BASE | the script's own flow (the control) | 4614.424 | 5746.562 | - | - |
| C1 | the calendar year label: 2022 | 4614.764 | 5747.733 | +0.340 | +1.171 |
| C2 | the calendar year label: 2021 | 4615.192 | 5748.696 | +0.768 | +2.134 |
| W0 | the warmup prepended at 0 h, the full 8760 h year reported | 4651.684 | 5755.784 | +37.260 | +9.222 |
| W1 | the warmup prepended at 24 h, the full year reported | 4651.307 | 5755.804 | +36.883 | +9.242 |
| W2 | the warmup prepended at 168 h | 4651.284 | 5755.811 | +36.859 | +9.249 |
| W3 | the warmup prepended at 336 h | 4651.284 | 5755.811 | +36.859 | +9.249 |
| I | the standard hour-beginning weather alignment (row n at step n) | 4613.994 | 5727.413 | -0.431 | -19.149 |
| G | the ground series: the engine's derivation | 4630.137 | 5756.184 | +15.713 | +9.622 |
| W1+I | the window and the alignment together | 4649.043 | 5736.705 | +34.619 | -9.857 |

Reading the table:

- **The calendar is exonerated.** The year label's whole leverage on the
  heating load is under 1 kWh; and the fixture's leap-2024 shape is not
  merely unmeasured but unrepresentable (the raise).
- **The warmup's state initialization is converged.** W0 against W1
  isolates the initial transient at 0.38 kWh; W1 through W3 move by
  0.02 kWh. The 24 h warmup is not the problem.
- **The window is real but the wrong way.** Reporting the full weather
  year (the case's own duration semantics: the warmup is initialization,
  not part of the year) ADDS 36.9 kWh of heating: Jan 1 is a cold day,
  and the consumed-warmup treatment was dropping it from the total.
- **The weather alignment's heating leverage is negligible.** The
  one-hour lag, the doubled row 0, and the dropped final row are worth
  -0.43 kWh of heating; the cooling response (-19.15 kWh) is forty times
  the heating response because the lag misaligns the EPW radiation
  against the sun position, which matters for gains, not for envelope
  losses.
- **The ground derivation's leverage is modest and also the wrong way.**
  Feeding the engine's ground series (a more volatile series than
  OCHRE's damped GTEMP) raises the heating load 15.7 kWh.

## The verdict

No admissible input correction closes the 110 kWh excess:

1. The only input discrepancy whose correction the case specification
   itself supplies is the warmup treatment (the reported window must be
   the weather year; the state initialization must converge). Measured:
   +36.88 kWh heating - the correction moves the runner FURTHER above
   the ceiling, to 4651.3 kWh.
2. The calendar, the weather alignment, and the ground series are
   tool conventions (OCHRE's or the engine's), not case-specified
   values; their leverages are +0.77, -0.43, and +15.71 kWh respectively,
   none of them the excess, and adopting the engine's conventions in an
   OCHRE-only script would be calibrating the comparator to the engine
   rather than to the case.
3. The case's 60/40 radiant split - the one input the standard supplies
   that the runner does not carry - is unrepresentable in OCHRE by
   construction. Its direction, read from the engine's own recorded
   delta when its fixtures adopted the standard's split (Case 600
   heating rose 71.4 kWh at the same weather), is also upward: a
   radiant-capable comparator would report a larger excess, not a
   smaller one.

The residual is the comparator's own model semantics: air-node-only
internal gains, convection-only films, its own solar transposition, its
weather alignment, its damped ground. **The runner is not an acceptance
oracle for the Annex B8 bands.** It is retained as a reference tool
only; the strict heating assertion keeps failing exactly as it fails
today; the bands [3993, 4504] and [5432, 6162] kWh are untouched; and
no HARES-side conclusion is drawn from this OCHRE-only evidence.

## The preflight witnesses

`tests/python/test_ochre_bestest_600.py` now pins, without annual runs:

- the bands' byte identity with the published Annex B8 values;
- the script's input identity against `600.toml` item by item (gains,
  setpoints, deadband, infiltration, timestep, duration, volume, optics,
  areas, orientations, windows, and every material layer with the
  derived R sums);
- the weather file's byte identity between the runner and the fixture;
- OCHRE's leap-year raise (no factor study can silently run the
  fixture's 2024 start);
- the schedule's one-hour alignment lag (row 0 doubled, the final EPW
  row dropped);
- the internal gains' air-node injection with no radiant surface term,
  with the vendored hardcode and fold drift-locked;
- the comparison's limitation metadata: the declared mismatch set and
  the acceptance-oracle disposition.

The existing slow tests continue to repeat the output-schema,
thermostat, and integration-length checks, and the annual loads alone
determine acceptance.

## Reproducing the arms

The arms run through the script's own construction
(`_build_schedule_for_year`, `_build_envelope`, and the script's
`_run_simulation` stepping loop). An arm builds the year schedule with
its one factor changed (the year label; the weather stamps shifted back
30 minutes without the resample; the ground column replaced; the warmup
rows prepended before Jan 1), constructs the Envelope, switches off the
redundant internal schedule feed (`envelope._has_schedule = False` - the
row dicts already override every registered schedule key each step, and
the BASE control proves equivalence), and steps the script's loop over
the arm's rows, dropping the warmup prefix from the records. Arms are
about 9 s each through the heavy lane.
