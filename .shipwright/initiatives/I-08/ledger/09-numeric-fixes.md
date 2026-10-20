# Numeric fixes

The defects `decision-log.md` `dl-i08-architecture.numeric_fixes` names,
plus the precedence change (`dl-i08-architecture.precedence`, B-13), the
move of everything HARES stores and computes to SI units
(`dl-i08-architecture.si_units`, B-14), OpenStudio-HPXML unavailable
periods (`dl-i08-architecture.unavailable_periods`, B-15), the
wet-appliance hot-water triage and its contingent fix
(`dl-i08-architecture.wet_appliance_hot_water`, B-16 and B-17) and one
defect fix found while giving B-9 a fixture (B-9a). Rules for every entry
in this file (B-16 is a measurement and takes the recording rules only):

- Its own commit and its own schedule row. No numeric change is folded into
  a structural commit.
  Where a structural entry prepares the ground (OR-1 for B-5, OR-8 for B-7,
  FL-4 for B-12), that structural commit is bug-compatible: bitwise equal on
  every golden fixture and on the consumer-shaped golden fixture. The number
  moves only in the entry here.
- It lands after the consumer migration gate (`08-consumer.md`, preamble
  step 4), in the order `00-schedule.md` gives.
- It follows M-1's numeric entry procedure (`materialize-all` at the parent
  commit, `delta`, `capture`) and records the delta per fixture in
  `02-measurement.md`'s log, including the consumer-shaped golden fixture,
  whose deltas feed B1's attribution (`08-consumer.md` CN-4).
- Its delta has a golden to land on. The fixtures are M-2's six, BR-11's
  warm-up manifests, BR-14's `month_boundary_billing_3600s`,
  `04-fleet.md` FL-1's `fleet_same_offset_300s`, the one B-9a adds and
  B-14's `der_pv_battery_900s` and `der_pv_battery_ev_900s`. A golden an entry needs that does not exist is added before
  the entry, in a commit that changes no engine code and captures the
  golden on the unfixed engine, so `materialize-all` at the entry's parent
  covers it.
- Every acceptance test names its inputs: the files under `tests/fixtures/`
  (or a weather file the entry adds, named with its content), the start,
  the duration, the step and any override. A unit test over values it
  constructs states those values.
- A dependency upgrade split out of `01-baseline-repair.md` BR-6b, BR-6c or
  BR-6d under `08-consumer.md`'s B0-check rule becomes an entry of this
  file, `B-0<package>`, placed first in the numeric block and following
  these rules.
- It adds a section to `docs/PHYSICS_DECISIONS.md` (created by B-1a): the
  reference, what OCHRE does, what changed and the direction of the delta.
  The commit body states the recorded delta in plain words, so the public
  history carries it.
- Each commit that moves a number adds one line to the "Numeric changes"
  list of `docs/python.md` (`plan.md` §2.8), naming what moved, why, and
  the golden fixtures it re-captured; each entry's Verification names
  that line, and `05-python-surface.md` PY-9's
  `numeric_changes_name_existing_goldens` checks it. B-8 and B-9a move no
  number and add none.
- It records, at its parent commit and after its change, its golden delta
  and the physics comparison suites (`plan.md` §3, the rule for numeric
  entries); no metric moves away from its reference unless the entry names
  the reference that justifies it.
- The changed code path logs the inputs of its decision with `tracing` at
  `debug` level (named per entry).
- Dependencies: every entry depends on M-1 and M-2 in addition to those it
  lists.

Pairs that touch the same code are adjacent in the schedule: B-1a, B-1b,
B-1c; B-2 then B-3; B-6, `04-fleet.md` FL-5, then B-11 (B-11 must handle
the nulls B-6 introduces and removes the `Enum` exception FL-5 leaves);
B-9a then B-9. B-5 depends only on OR-1 and shares no code with B-6 or
B-11, so its place before them is an order, not a pairing. B-14's
equipment config commit lands directly before `12-inputs.md` IN-9, which
writes the EV config's SI fields; its run-state and control-layer commits
follow `11-other-defects.md` OD-10's numeric commit and precede
`08-consumer.md` CN-3. B-15, B-16 and B-17 follow B-14 and `11-other-defects.md`
OD-20 and precede CN-3, in that order; B-17 runs only when B-16 confirms
the defect.

## Weather: what EnergyPlus specifies (binding for B-1a, B-1b, B-1c)

Read from the vendored EnergyPlus source and documentation
(`vendors/EnergyPlus`, commit `f7c103167a`) and checked against the
published pages (bigladdersoftware.com, EnergyPlus 24.1 Input Output
Reference "Weather Data Hourly Interpolation" and Engineering Reference
"Climate Calculations").

- **Point values (Input Output Reference, "Weather Data Hourly
  Interpolation", `doc/input-output-reference/src/weather-data/weather-data-hourly-interpolation.tex`).**
  "Last Hour Interpolation": `Value_TimeStep = LastHourValue * Weight_LastHour
  + ThisHourValue * Weight_ThisHour`, `Weight_ThisHour = CurrentTimeStep /
  NumberOfTimeStepsInHour`, `Weight_LastHour = 1 - Weight_ThisHour`. Hour 1
  is 00:00:01 to 01:00:00, so a record's value is reached at the end of its
  hour. Published weights: four steps per hour, ThisHour 0.25, 0.5, 0.75,
  1.0; six steps, 1/6 to 6/6. "For day boundaries (i.e. from hour 24 to hour
  1), the values from the previous day's last hour (23:00:01 to 24:00:00)";
  for the first day of a weather simulation, the values of hour 24 of that
  day. Interpolation runs only when a file has one record per hour and the
  run has more than one timestep per hour (`WeatherManager.cc:3043`); with
  one timestep per hour, each timestep takes its hour's record.
- **Fields (the interpolation loop, `WeatherManager.cc:3093-3121`).**
  Linear with those weights: dry bulb, barometric pressure, dew point,
  relative humidity, wind speed, total and opaque sky cover, horizontal
  infrared. Wind direction: `interpolateWindDirection` (on the circle).
  Liquid precipitation: the weighted value divided by the number of
  timesteps per hour. Sky temperature: recomputed from the interpolated
  inputs (`calcSky`), not interpolated. Snow indicator: the hour's record
  (`:3010, 3121`). Albedo: EnergyPlus does not carry the file's albedo into
  its per-timestep weather; the value is read only into `wvarsMissing`
  (`:2936-2938, 3031`) and the loop assigns no albedo, so holding the
  hour's record is a HARES choice. HARES's weather has no total sky cover
  field (`crates/hares-io/src/weather.rs:451-467`, opaque sky cover only),
  so that EnergyPlus field has no HARES counterpart.
- **Solar (Engineering Reference, "Weather File Solar Interpolation",
  `doc/engineering-reference/src/climate-sky-and-solar-shading-calculations/climate-calculations.tex:304-306`).**
  "The solar values on the weather file are average values over the hour.
  For interpolation of hourly weather data (i.e., when the specified
  timestep is greater than 1), the average value is assumed to be the value
  at the midpoint of the hour." The weights are `SolarInterpolation`
  (`WeatherManager.cc:8343-8375`): for an even number of steps N the step
  ending at the half hour has weight 1.0 on its own hour and the weight
  falls by 1/N per step each side (N = 4: 0.75, 1.0, 0.75, 0.5; N = 6:
  4/6, 5/6, 1.0, 5/6, 4/6, 3/6); for N = 3: 5/6, 5/6, 0.5; for odd N >= 5
  the two middle steps take `1 - 1/(2N)`. Step `ts` of N is before the half
  hour when `2·ts < N`, at it when `2·ts == N` (only for even N, where the
  weight is 1.0) and after it when `2·ts > N` (`ts * TimeStepFraction <
  0.5`, `WeatherManager.cc:3081-3091`). Before the half hour the remainder
  goes to the previous hour's record, after it to the next hour's
  (`:3074-3091, 3113-3116`). This is linear interpolation between hour
  midpoints, evaluated at the end of each timestep. Beam and diffuse are
  interpolated this way. EnergyPlus keeps no global horizontal irradiance
  in its per-timestep weather (`WeatherVars`, `WeatherManager.hh:713-732`);
  HARES applies the same weights to GHI, which is a HARES extension. The
  "next hour" of hour 24 is hour 1 of the same day's records, not of the
  next day: `NextHr = (hour == Constant::iHoursInDay) ? 1 : hour + 1`
  indexes the day's own `wvarsHr` (`:3061-3065`). The "previous hour" of
  hour 1 is the previous day's hour 24 (`wvarsLastHr`, carried across days,
  `:3124`), except on the run's first day: `wvarsLastHr` is the whole
  weather record, set from the first day's hour 24 (or hour 1 under
  `Hour1`, below) before the first hour (`:3050-3056`), so the first hour's
  previous-hour solar value is that record too. The same rule applies on
  every repeated warm-up day ("This same interpolation will be used for
  repeated warmup days", `idd/Energy+.idd.in:1488`; the file is re-read
  from its first record with `LastHourSet` cleared, `WeatherManager.cc:2359`).
- **Timesteps.** "Number of Timesteps per Hour ... Must be evenly divisible
  into 60" (`idd/Energy+.idd.in:925-936`: 1, 2, 3, 4, 5, 6, 10, 12, 15, 20,
  30 or 60).
- **Sub-hourly files.** A file with more than one record per hour must have
  as many records per hour as the run has timesteps, or the run is an error
  (`WeatherManager.cc:715-722`); the records are then used directly.
- **Clock.** Weather records are read by local standard time; daylight
  saving shifts schedules, not weather, and reported time stamps "are
  always in standard time. When daylight saving time is active, scheduled
  loads and controls will shift one hour relative to standard time."
  (`group-location-climate-weather-file-access.tex:830`).
- **First hour (Input Output Reference, RunPeriod, "First Hour
  Interpolation Starting Values", `group-location-climate-weather-file-access.tex:717-721`).**
  `Hour24` (default): the first hour of the run interpolates from hour 24 of
  the first day; `Hour1`: from hour 1. "Note that this option only affects
  the first hour of the first day in the entire run period"
  (`group-location-climate-weather-file-access.tex:721`); the code sets
  `wvarsLastHr` from that record once per environment
  (`WeatherManager.cc:3050-3056`, the assignment at `:3054-3055`), for
  every field, solar included.
- **Typical and actual weather (RunPeriod "Treat Weather as Actual", default
  No, `idd/Energy+.idd.in:1480-1484`; `WeatherManager.cc:866-996`).**
  Non-actual: a file covering a complete year wraps around (`:929-941`); a
  file whose data period is not a complete year runs only when the run's
  start and end dates lie inside that period, and is fatal otherwise
  (`:941-991`). The water-mains correlation from the weather file
  (`Site:WaterMainsTemperature` method `CorrelationFromWeatherFile`) reads
  twelve complete months of dry bulb from the file
  (`CalcAnnualAndMonthlyDryBulbTemp`, `:8742-8800`); without them EnergyPlus
  uses a fixed 10.0 C with a severe error. "When the weather is
  treated as non-actual, the leap day will be skipped if it is not present,
  resulting in incorrect day types for the remainder of the year"
  (`doc/input-output-reference/src/overview/group-location-climate-weather-file-access.tex:597-601`).
  In the source, a leap year whose file does not allow leap years
  (`WFAllowsLeapYears`, read from the EPW header, `WeatherManager.cc:7896`)
  is simulated as a 365-day year (HARES follows this, B-1c, except for the
  day types, below): `IsLeapYear` requires the file to allow
  leap years (`:8456`), `CurrentYearIsLeapYear` takes the environment's
  leap flag (`:872`) and is forced false on a multi-year rollover
  (`:1896-1900`), `LeapYearAdd` is 0 (`:873-876, 1906`), and a Feb 29
  record met in such a year, or met in any non-leap run year, sets
  `SkipThisDay` with the warning "Feb29 data encountered but will not be
  processed." (`:2816-2828`, the condition at `:2817`). The day after
  Feb 28 is Mar 1 and Feb 29 is never simulated; a missing Feb 29 is fatal
  only for actual weather (`:2832-2836`). The day of week advances by one
  per simulated day (`CurDayOfWeek = mod(CurDayOfWeek, 7) + 1` for each day
  read, `:2856-2859`), so after a skipped Feb 29 every later date carries
  the weekday after its predecessor's, one day behind the calendar: the
  "incorrect day types" the documentation names. HARES does not reproduce
  this known defect: its day types follow the calendar
  (`dl-i08-architecture.leap_day_day_types`, B-1c). Actual: the run's start and
  end dates must lie inside the file's dated period (`:904-926`, fatal at
  `:990`), and a leap year in the run with a file that does not support
  leap years is fatal (`:888-899, 993-996`).

HARES labels an output step at its start: the step `[t, t + dt)` takes the
EnergyPlus value of the timestep that ends at `t + dt`.

## B-1a: weather point values follow EnergyPlus last-hour interpolation

**Kind:** numeric fix (`numeric_fixes`: weather-record alignment;
`dl-i08-architecture.weather`). Fixes V4, with B-1b.

**Problem:** which weather value a step reads depends on the step length,
and none of the lengths matches EnergyPlus.
- Every field is shifted by `midpoint_offset_secs` (1800 s for hour-ending
  files) before indexing a resampled array whose row `k * f` is source row
  `k`. The step `[t, t + dt)` therefore reads the value at file-label time
  `t + 30 min` at 900 s and 1800 s, and the record labelled `t` (the hour that
  ended at `t`) at 3600 s. EnergyPlus uses file-label time `t + dt`. At
  3600 s HARES lags one hour (the consumer measured it:
  `irradiance.py:48-67`, `engine_row[i] == file_row[i - 1]`).
- The first steps of a run wrap to the end of the file: at 900 s the first
  30 minutes read a PCHIP blend of the Dec 31 records, at 3600 s the first
  hour reads the Dec 31 hour-24 record. EnergyPlus uses hour 24 or hour 1
  of the first day.
- Point fields are interpolated with PCHIP (cyclic for dry bulb, dew point,
  infrared, ground temperature), not linearly; wind speed is held (zero
  order), not interpolated; precipitation is split evenly, not interpolated.

**Evidence:** `crates/hares-core/src/environment.rs:518-522` (`weather_idx
= (step + weather_start_offset) % weather_len`), `:796-818`
(`compute_annual_offset`: `seconds_into_year + year_secs -
midpoint_offset_secs`, floored to the step); `crates/hares-io/src/weather.rs:568-713`
(`resample_with` defaults: `PchipCyclic` dry bulb, dew point, infrared,
ground temperature; `Pchip` pressure; plain PCHIP relative humidity and sky
cover; `Zoh` wind speed and solar; `CircularLinear` wind direction;
`distribute_accumulated` precipitation at `:708`; surface albedo at
`:709-713`), `:950-1000` (`pchip_resample`:
output row `i` sits at source position `i / factor`); record labels per
format: `epw.rs:429` and `tmy3.rs:254` and `resstock_csv.rs:362`
(`midpoint_offset_secs: 1800`, hour-ending), `psm3.rs:289-292, 309`
(start-of-interval hours 0 to 23, `midpoint_offset_secs: 0`).

**Change:**
- `WeatherMeta.midpoint_offset_secs` and the shift in
  `compute_annual_offset` go. `WeatherMeta` carries `record_label:
  RecordLabel { IntervalEnd, IntervalStart }` per format: EPW, TMY3 and
  ResStock CSV are `IntervalEnd` (a record stamped `h` covers `(h - 1 h,
  h]`); PSM3 is `IntervalStart` (a record stamped `t` covers `[t, t +
  interval)`, so an hourly PSM3 record stamped `t` is the record of the
  hour ending `t + 1 h`).
- `SimulationConfig::validate()` rejects a `time_res` under one hour that
  does not divide 3600 s, naming the value and the rule (EnergyPlus
  allows only timesteps per hour that divide 60, so `N` below is always an
  integer).
- Hourly files at a step shorter than an hour: every point field is linear
  last-hour interpolation with `Weight_ThisHour = ts / N`, `N = 3600 / dt`,
  `ts` the index (1 to N) of the step within the hour, evaluated for the
  step's end. Fields: dry bulb, dew point, relative humidity, pressure, wind
  speed, opaque sky cover, horizontal infrared: linear. Wind direction: the
  existing circular interpolation with the same weight. Precipitation: the
  weighted hourly depth divided by N. Sky temperature: recomputed from the
  interpolated inputs (today's code, `weather.rs:635-648`). Surface albedo:
  the hour's record, a HARES choice (EnergyPlus drops the file's albedo,
  binding section above). Ground temperature is not a weather-file hourly
  field in EnergyPlus (monthly site values); HARES's series, derived from
  monthly values, is held at the value of the hour its step end lies in and
  not re-interpolated.
- Hourly files at a one-hour step: each step takes the record of the hour
  it ends in; no interpolation.
- Sub-hourly files whose interval equals the step: each step takes the
  record covering it. A sub-hourly file (more than one record per hour)
  whose interval differs from the step is a construction error naming both
  intervals, as in EnergyPlus ("Number of intervals per hour on Weather
  file does not match specified number of Time Steps Per Hour",
  `WeatherManager.cc:715-722`; `dl-i08-architecture.weather_edge_cases`).
  Today's resampling of such files (mean when finer, PCHIP or hold when
  coarser) is deleted. The rule covers every format that can be
  sub-hourly: PSM3, and EPW once `12-inputs.md` IN-6 reads sub-hourly EPW
  files (their records are `IntervalEnd`, replacing today's
  `midpoint_offset_secs = 1800 / N`).
- Hourly files at a step longer than one hour keep today's interval mean of
  the records the step covers (EnergyPlus has no timestep longer than an
  hour). A sub-hourly file at such a step is the interval mismatch above.
- First hour: new config field `first_hour_interpolation: FirstHour {
  Hour24, Hour1 }`, default `Hour24` (`plan.md` section 2 weather contract).
  `Hour24`: the last-hour value for the first hour of the run is the hour-24
  record of the run's first day; `Hour1`: the hour-1 record. The rule sets
  the whole previous-hour record, so it governs the solar previous hour of
  the run's first hour too (B-1b). Warm-up days repeat the first day with
  the same rule (B-2; EnergyPlus `idd/Energy+.idd.in:1488`). No step reads a record
  before the run's first day. The field joins `06-runtime-memory.md`
  RT-2's `ForcingKey` in this commit, because it changes which records a
  step reads; test `forcing_key_differs_by_first_hour` (two blueprints over
  one `ParsedWeather` differing only in `first_hour_interpolation` hold
  different tables, `Arc::ptr_eq` false).
- Every day boundary after the first reads the previous day's hour-24
  record as last hour, with days mapped by calendar (B-1c).
- `ResampleOverrides`, `ResampleMethod`, `ochre_compat()` and the Python
  `resample_overrides` kwarg are already gone (`12-inputs.md` IN-6); the
  methods above are the engine's only methods, chosen per field by one
  exhaustive `match`.
- `docs/PHYSICS_DECISIONS.md` is created with the weather section: the
  quotes above, the field table, the record-label table, OCHRE's behaviour
  (pvlib midpoint convention, which this replaces), and the three places
  where HARES's field coverage differs from EnergyPlus: albedo is held at
  the hour's record (EnergyPlus drops it), there is no total sky cover
  field to interpolate, and GHI takes the solar weights (EnergyPlus keeps
  no GHI).
- `tracing` at `debug`: the record indices and weights of the first step of
  each day.

**Fixture added by this entry:** `tests/fixtures/weather/psm3_15min_2019.csv`,
a full-year 15-minute PSM3 file that `psm3.rs` parses as it stands:
- the parser's three-line header (`psm3.rs:4-5, 57-107`): line 1
  `Source,Location ID,City,State,Country,Latitude,Longitude,Time Zone,Elevation`,
  line 2 `NSRDB,0,-,-,-,39.74,-105.18,-7,1829`, line 3 the column names
  `Year,Month,Day,Hour,Minute,Temperature,Dew Point,Relative Humidity,Pressure,GHI,DNI,DHI,Wind Speed,Wind Direction`
  (every column `build_column_map` requires);
- 35040 records stamped 2019-01-01 00:00 to 2019-12-31 23:45 every
  15 minutes, local standard time (the full-year count
  `validate_record_count` accepts at 900 s, `psm3.rs:445-462`);
- record `k` (0-based) has `Temperature` `-20.0 + 0.5·(k mod 96)` C, so
  every day runs from -20.0 to 27.5 C, inside the parser's [-60, 55] C
  bound (`:148-152`); `Dew Point` -40.0 C, never above the dry bulb
  (`:155`); `Relative Humidity` 50; `Pressure` 1013 mbar; `GHI`, `DNI` and
  `DHI` 0; `Wind Speed` 2 m/s; `Wind Direction` 180.

**Acceptance test** (unit tests over the stated series, plus the named
fixtures; the cz4a file is a Denver TMY3 file at UTC-7, so its runs start
at `-07:00`; each fixture test compares the run's outdoor dry-bulb column
with B-1a's interpolation of the named records, computed in the test from
the parsed file):
- `point_weights_match_io_reference_four_steps`: an hourly series with
  the previous day's hour-24 dry bulb 0.0 and hour 1 at 4.0; at 900 s the four
  steps of hour 1 read `0.0 * 0.75 + 4.0 * 0.25`, `... * 0.5`, `... * 0.75`
  and `4.0`, bitwise equal to that formula in f64.
- `point_weights_match_io_reference_six_steps`: the same at 600 s with the
  six published weights.
- `hourly_step_reads_the_record_of_the_hour_it_ends`: the
  `tests/fixtures/parity/cz4a_ashp_hpwh/` inputs from
  2023-01-01T00:00:00-07:00 for 1 d at 3600 s, warm-up disabled: the
  outdoor dry bulb of the row labelled 00:00 equals the record stamped
  Jan 1 01:00 (row 0), and every later row `i` equals row `i`.
- `first_hour_uses_hour24_of_the_first_day_by_default` and
  `first_hour_uses_hour1_when_selected`: the same inputs from
  2023-01-01T00:00:00-07:00 for 1 d at 900 s, warm-up disabled: the row
  labelled 00:00 reads `h24 * 0.75 + h1 * 0.25` (or `h1`) of Jan 1's
  records, never a Dec 31 record.
- `wind_direction_interpolates_on_the_circle`: 350 degrees to 10 degrees over
  four steps reads 355, 0, 5, 10.
- `precipitation_is_interpolated_then_divided_by_steps_per_hour`: hourly
  depths 2.0 mm then 6.0 mm at 900 s read `(2.0 * 0.75 + 6.0 * 0.25) / 4`,
  `(2.0 * 0.5 + 6.0 * 0.5) / 4`, `(2.0 * 0.25 + 6.0 * 0.75) / 4` and
  `6.0 / 4`.
- `psm3_record_covers_the_interval_it_starts`: the cz4a building and
  schedule with the PSM3 fixture from 2019-01-01T00:00:00-07:00 for 1 d at
  900 s, warm-up disabled: step `k` (0 to 95) reads outdoor dry bulb
  `-20.0 + 0.5·k`.
- `subhourly_file_interval_must_equal_the_step`: the same inputs at a
  300 s and at a 1800 s step fail construction naming both intervals.
- `subhour_step_must_divide_the_hour`: `time_res = 420` fails
  `validate()` naming 420 s; 900 and 3600 pass.
- `every_weather_field_has_one_declared_method`: one exhaustive `match` over
  `WeatherField` returns each field's method; the test checks it against the
  field table above.

**Verification:** `cargo nextest run -p hares-io weather::` and `-p
hares-core environment::`; goldens re-captured; the commit's
`docs/python.md` "Numeric changes" line (preamble) names the point-value
interpolation and the re-captured goldens.

**Commit:** `fix: weather point values follow EnergyPlus last-hour interpolation at step end`

**Dependencies:** M-1, M-2; `05-python-surface.md` PY-8 (the one
`SimulationConfig`, which gains `first_hour_interpolation`, `plan.md`
§2.7); `12-inputs.md` IN-4 (its test re-runs here and holds by
construction), IN-6.

**Measurement:** per golden, annual heating and cooling energy, peak
electric power and mean indoor temperature deltas, recorded separately from
B-1b's solar deltas.

**State:** committed `2d3e6721` (branch `row/r79-b1a-b1b`, based on
`e30f70c1`, one commit). The implementation: `WeatherMeta` carries
`record_label: RecordLabel { IntervalEnd, IntervalStart }` (EPW, TMY3 and
ResStock CSV interval-end; PSM3 interval-start) and `midpoint_offset_secs`
is gone with the shift in the annual offset; `WeatherTimeSeries::resample`
takes the step, the first-hour rule and the run's start and resolves by the
EnergyPlus rules: the records used directly at the file's own step, the
interval mismatch a construction error naming both intervals for sub-hourly
files, the interval mean for an hourly file at a step longer than an hour,
and the interpolation expansion (`interpolate_hourly_records`) for an
hourly file at a sub-hourly step: one row per step, the point fields linear
last-hour interpolation evaluated at the step's end, the wind direction on
the circle, the precipitation weighted then divided by N, the sky
temperature recomputed from the row's interpolated inputs, the ground
temperature and surface albedo at the hour's record. The first-hour rule is
`SimulationConfig.first_hour_interpolation` (`hour24` the default), joins
`ForcingKey`, and the resample cache keys on the step, the rule and the
run's start (the rule changes which records the first hour's steps read);
`validate()` rejects a step under an hour that does not divide 3600 s. The
parsed-weather handles share one resolved series per key, the blueprint and
the exposed `resample_weather` ride it, and `docs/PHYSICS_DECISIONS.md`
gains the weather section (the quotes, the field table, the record-label
table, OCHRE's held behaviour, the three field-coverage differences). RED
first: the differential tests in `crates/hares-core/tests/
weather_interpolation.rs` compiled at the parent and failed there (the
1-minute EPW hold read lagged 30 minutes and wrapped to the file's end, and
the 15-minute PSM3 fixture's interval mismatch raised nothing, and pass at
the commit. The acceptance list landed whole (the four- and six-step
published weights bitwise, the circle 350→10 reading 355/0/5/10, the
divided precipitation, the first hour both ways over the cz4a fixture and
over the stated series, the PSM3 covering record, the mismatch refusals,
the divide-the-hour validation, the forcing key differing by the rule, the
exhaustive method match). The census: the goldens all read hourly weather,
so every golden exercises the new rules: all nine moved (declared),
re-captured twice byte-identical, the per-golden deltas in
`02-measurement.md`; the sub-hourly fixtures are the OCHRE comparison's
hold files, read directly at their interval. The PCHIP-era tests are
rewritten to the new methods (the pchip machinery deleted); the drift locks
re-measured with the cause (the BESTEST 640/600FF/900FF locks, the
diagnostics CSV pin re-captured, the held-weather phase test rewritten to
the covering-record phase). Gates at the row: fmt clean, clippy
`--workspace --all-targets --all-features -D warnings` clean, the
`hares-io` and `hares-core` suites green; the final block's gates green at
the tip: nextest `--workspace --all-features -j 2` 8073/8073 (6 skipped),
`pytest -m ''` 1753 passed + 6 xfailed, 0 failed, 0 errors, pyright 0, the
nine goldens bitwise (compare-all 8 + dst 1).

## B-1b: weather solar values follow EnergyPlus midpoint interpolation

**Kind:** numeric fix (`dl-i08-architecture.weather`, interval averages).
Fixes V4, with B-1a.

**Problem:** hourly global, direct and diffuse irradiance are held
constant over each hour (zero order), and through the shared shift the held
value spans from 30 minutes into one hour to 30 minutes into the next.
EnergyPlus treats each hourly average as the value at the hour's midpoint
and interpolates linearly between midpoints.

**Evidence:** `crates/hares-io/src/weather.rs:668-696` (`ghi_w_m2`,
`dni_w_m2`, `dhi_w_m2` default `ResampleMethod::Zoh`, with a comment that
cites the Engineering Reference section but applies a hold);
`environment.rs:796-818` (the shift).

**Change:** for hourly files at a step shorter than an hour, GHI, DNI and
DHI use the `SolarInterpolation` weights above, evaluated for the step's
end: before the half hour, `w * this_hour + (1 - w) * previous_hour`; at the
half hour, `this_hour`; after it, `w * this_hour + (1 - w) * next_hour`. At
a one-hour step each step takes its hour's record. The weights are computed
by one function of `(N, ts)` that reproduces `WeatherManager.cc:8343-8375`
for every N, with step `ts` before the half hour when `2·ts < N`, at it
when `2·ts == N` and after it when `2·ts > N` (an odd N has no step at the
half hour). After the half hour of hour 24, the next-hour value is hour 1
of the same day, as `WeatherManager.cc:3061-3065` does (not the next day's
hour 1); before the half hour of hour 1, the previous-hour value is the
previous day's hour 24, except in the run's first hour and in the first
hour of every warm-up day, where it is the record `first_hour_interpolation`
names (B-1a): hour 24 of the first day under `Hour24`, hour 1 under
`Hour1` (`:3050-3056`; `idd/Energy+.idd.in:1488`). GHI takes the same
weights as DNI and DHI; EnergyPlus keeps no GHI, so this is a HARES
extension, recorded as such. `docs/PHYSICS_DECISIONS.md`'s weather section
states all three. `tracing` at `debug`: the weights table for the run's N.

**Acceptance test** (unit tests over the stated hourly series):
- `solar_weights_match_energyplus_four_steps`: records 400 (previous hour),
  800 (this hour), 0 (next hour); the four 900 s steps read 700, 800, 600,
  400.
- `solar_weights_match_energyplus_six_steps`: the same records at 600 s read
  the six values from the N = 6 weights.
- `solar_weights_match_energyplus_odd_steps`: the weight function returns
  5/6, 5/6, 0.5 for N = 3 and `1 - 1/10` for the middle steps of N = 5.
- `solar_hour24_takes_hour1_of_the_same_day_after_the_half_hour`: one day
  with GHI hour 1 = 100, hour 23 = 300, hour 24 = 500 and the next day's
  hour 1 = 900; the four 900 s steps of hour 24 read `500 * 0.75 + 300 *
  0.25`, `500`, `500 * 0.75 + 100 * 0.25` and `500 * 0.5 + 100 * 0.5`,
  never a value using 900.
- `solar_first_hour_uses_hour24_of_the_first_day`: the run's first day
  with GHI, DNI and DHI hour 1 = 100, hour 2 = 200 and hour 24 = 500, at
  900 s under the default `Hour24`: the four steps of the run's first hour
  read `100 * 0.75 + 500 * 0.25` (200), `100`, `100 * 0.75 + 200 * 0.25`
  (125) and `100 * 0.5 + 200 * 0.5` (150); with `first_hour_interpolation
  = Hour1` the first step reads `100 * 0.75 + 100 * 0.25` (100) and the
  other three are unchanged. (B-2 re-runs it over warm-up days.)

**Verification:** `cargo nextest run -p hares-io weather::`; goldens
re-captured; the commit's "Numeric changes" line names the solar midpoint
interpolation and the re-captured goldens.

**Commit:** `fix: solar irradiance follows EnergyPlus midpoint interpolation`

**Dependencies:** B-1a.

**Measurement:** per golden, annual incident solar on the horizontal, PV
output and solar heat gains deltas.

**State:** committed `487df2e9` (branch `row/r79-b1a-b1b`, on the
point-value commit). The solar fields' interpolation method is
`SolarMidpoint`: the weights one function of (N, ts)
(`solar_interpolation_weight`) reproducing the vendored
`SolarInterpolation` table for every N, the value the three-term sum in
the loop's own form (`last * prev_w + this * wgt + next * next_w`) with
the day-aware next hour (hour 24's next is hour 1 of the same day) and
the first-hour rule's previous record at the run's first hour; GHI, DNI
and DHI all take it, the GHI extension recorded in the physics decisions'
weather section. The acceptance tests landed whole: the four-step and
six-step published weights, the odd-N tables (5/6, 5/6, 0.5 for N = 3;
`1 - 1/10` for the two middle steps of N = 5), the hour-24 same-day wrap
(never the next day's hour 1), and the first hour under both first-hour
rules. One consequence folded into the commit: the OCHRE comparison's
hold file is read by the covering-record rule now, so the adapter's
`hold_weather` places each hour's value in the records stamped from
coverage start plus 30 minutes to the next hour's (the block's second
half carries its own record's values and source flags, the first half the
previous record's, the file's first half hour the first record's value:
OCHRE's own backfill of the first row), the committed hold files
regenerated byte-identical to the adapter's output, and the strict OCHRE
parity is bitwise again (the hold under the new read had carried a
1-minute phase lead). The 48 h thermal trace's records re-measured (the
interpolated values phase against OCHRE's held step values at the solar
ramps: the worst window-solar step error 9.2% inside the widened 10% band,
the mean ratio 0.9885, the 48 h heating 40.136 kWh and the overnight worst
1.2267 °C re-recorded). The census: every golden exercises the solar rule
(hourly weather at sub-hourly or the hour step): all nine moved again
(declared), re-captured twice byte-identical; the annual incident solar on
the horizontal unchanged (the rule preserves the totals: the measured
annual mean delta 0.000000 over the cz4a file at 900 s), the PV deltas and
the HVAC deltas in `02-measurement.md`, the goldens' verbosity-2 frames
carrying no insolation or solar-gain columns so the solar heat gains' own
delta is not derivable from the committed products. Gates at the row and
the final block's gates at the tip: fmt clean, clippy
`--workspace --all-targets --all-features -D warnings` clean, nextest
`--workspace --all-features -j 2` 8073/8073 (6 skipped), `pytest -m ''`
1753 passed + 6 xfailed, 0 failed, 0 errors, pyright 0, the nine goldens
bitwise (compare-all 8 + dst 1).

## B-1c: typical and actual weather years, wrap and leap days

**Kind:** numeric fix (`dl-i08-architecture.weather`: typical vs actual
year, wrap and leap-day handling). Fixes V5.

**Problem:**
- The weather row is `(step + offset) % len` from an offset computed once
  from `start_time.ordinal0()`. Against a 365-day file, a run starting in a
  leap year after Feb 29 reads weather one day late, and a leap-year run
  starting before Feb 29 reads one day late from Feb 29 on.
- Schedules already map leap-year dates after Feb 29 to their non-leap rows
  (`environment.rs:418-461`), so weather and schedules disagree by a day for
  the rest of such a year.
- There is no notion of an actual-year file: any overrun wraps silently, and
  an AMY 2018 file run for 2019 dates reads 2018 weather without error.

**Evidence:** `crates/hares-core/src/environment.rs:518-522` (modulo
wrap), `:806-818` (`ordinal0()` against the file length),
`:418-461`, `:448-450` and `:875` (the schedule and weather leap-year
adjustments), pinned by the environment tests at `:1975-2040` and
`:2090-2130`; `:862-906` (`compute_mains_inputs`: the water-mains annual
mean and monthly range from the file's dry bulb, with fallbacks for data
shorter than a year); `WeatherMeta.wf_allows_leap_years`
(`crates/hares-io/src/weather.rs:294`, set from the EPW header at
`epw.rs:167`); `epw.rs:199-203` (each record's year, month and day must
form a real date), `:318-325` (the record count must be 8760 or 8784,
whatever the DATA PERIODS header says; the period is not kept).

**Change:**
- New config field `weather_year: WeatherYear { Typical, Actual }`,
  default `Typical` (EnergyPlus "Treat Weather as Actual" defaults to No).
  It joins `06-runtime-memory.md` RT-2's `ForcingKey` in this commit,
  because it changes which records a step reads; test
  `forcing_key_differs_by_weather_year` (two blueprints over one
  `ParsedWeather` differing only in `weather_year` hold different tables,
  `Arc::ptr_eq` false).
- Records are addressed by the local standard time date and hour (the
  site's standard offset), never by the civil time of a `civil_timezone`
  with daylight saving, and never by a running index. Daylight saving
  shifts schedules, not weather (EnergyPlus, "Clock" above).
- Every weather file covers a complete year (Jan 1 to Dec 31). A file
  whose data period is not a complete year is a construction error naming
  the file's data period and the inputs that need a complete year: the
  water-mains annual mean and monthly range (`environment.rs:862-906`) and
  the ground temperature the parsers derive from twelve monthly dry-bulb
  means. EnergyPlus runs a partial typical period when the run lies inside
  it (`WeatherManager.cc:941-991`), but its weather-file water-mains
  correlation reads twelve complete months and otherwise falls back to a
  fixed 10.0 C (binding section above); HARES always derives the mains
  from the weather file and substitutes no value it was not given
  (`dl-i08-criteria`), so a partial file has nothing to run on. The EPW
  reader keeps the DATA PERIODS period in `WeatherMeta` and its count
  error (`epw.rs:318-325`) names that period and this reason.
  `docs/PHYSICS_DECISIONS.md` records the rule and the EnergyPlus
  comparison.
- Parsed weather keeps each record's dated stamp as the file states it
  (year, month, day, hour and minute), so `Actual` checks a run against the
  file's own dates. An `IntervalEnd` file's dated range runs from its first
  stamp less one interval to its last stamp.
- `Typical`: month, day and hour address the file whatever its year; a run
  past Dec 31 continues at Jan 1 of the file (wrap by day of year). In a
  non-leap run year a typical file's Feb 29 records are skipped, as
  EnergyPlus skips them (`WeatherManager.cc:2817`). For a leap-year run with a file that
  has no Feb 29, HARES does what EnergyPlus does
  (`dl-i08-architecture.weather_edge_cases`, citations above): Feb 29 is
  not simulated. The run still covers the calendar interval `[start,
  start + duration)`; the clock steps from the last step of Feb 28 to Mar 1
  00:00, no output row is labelled Feb 29, and every later date reads its
  own calendar date's records. Weather and schedules share this one rule
  (the schedule's leap-year row adjustment at `environment.rs:418-461`,
  under which Feb 29 and Mar 1 both read the non-leap Mar 1 row,
  `:429-431`, and the weather adjustments at `:448-450` and `:875` are
  replaced by it, and the environment tests that pin them, `:1975-2040`
  and `:2090-2130`, are replaced by the tests below). An EPW whose header allows no leap day
  already drops Feb 29 rows at parse (`crates/hares-io/src/epw.rs:327-345`).
- Day types (the weekday or weekend the schedules and the HPXML
  weekday/weekend setpoint profiles read) are every simulated date's own
  calendar weekday, including after a skipped Feb 29
  (`dl-i08-architecture.leap_day_day_types`). This is the one weather edge
  case where HARES departs from EnergyPlus: EnergyPlus advances the day of
  week by one per simulated day (`WeatherManager.cc:2856-2859`), so after a
  skipped Feb 29 every later date is one weekday behind its calendar
  weekday, which its own Input Output Reference calls incorrect ("the leap
  day will be skipped if it is not present, resulting in incorrect day
  types for the remainder of the year",
  `group-location-climate-weather-file-access.tex:597-601`).
  `docs/PHYSICS_DECISIONS.md` records the rule, the EnergyPlus behaviour it
  departs from, the citation, and the reason (a schedule's weekday and
  weekend profiles must fall on the days the calendar says). A
  `start_time` on a skipped Feb 29 is a `DwellingBlueprint::validate()`
  error naming the date (the skip depends on the weather file, which
  `SimulationConfig::validate()` does not see). A typical file that has
  Feb 29 records uses them in a leap run year, and then no day is skipped.
- The resolved clock that `DwellingBlueprint::validate()` reports
  (`12-inputs.md` IN-2) carries the run's simulated step count (the
  calendar steps less any skipped Feb 29), so a caller can check a table's
  row count against the engine, not against its own calendar arithmetic
  (`08-consumer.md` CN-14).
- `03-output-reporting.md` OR-7's `frame_starts_at_start_time_with_exact_row_count`
  expects the simulated step count from this commit on, and the "Time"
  section OR-7 adds to `docs/python.md` gains the leap-day rule: on a
  typical file without Feb 29, Feb 29 is not simulated, no row is labelled
  Feb 29, and every later date keeps its calendar weekday.
- `Actual`: the run's first and last steps must lie inside the file's dated
  records, or construction fails naming both ranges; a run that includes a
  leap year with a file that has no Feb 29 fails naming the year.
- `tracing` at `debug`: the calendar mapping and day type at each day
  boundary where a wrap or leap-day rule applies.

**Fixtures added by this entry** (derived from
`tests/fixtures/parity/cz4a_ashp_hpwh/weather.epw`, a typical file: header
"HOLIDAYS/DAYLIGHT SAVINGS,No", data period 1/1 to 12/31, 8760 records):
- `tests/fixtures/weather/cz4a_with_feb29.epw`: the same file with the
  header's leap-year flag "Yes" and 24 Feb 29 records inserted after Feb 28
  hour 24, each a copy of the Feb 28 record of the same hour with year
  1996, month 2, day 29 and dry bulb 50.0 C (the cz4a Feb 28 records are
  year 1994, and `epw.rs:199-203` rejects 1994-02-29; 1996 is a leap year),
  8784 records, data period 1/1 to 12/31 unchanged, so a read of Feb 29 is
  recognisable.
- `tests/fixtures/weather/cz4a_january.epw`: the same header with data
  period 1/1 to 1/31 and the first 744 records.
- `tests/fixtures/weather/G0600770_restamped_2020.csv`: the records of
  `tests/fixtures/resstock/2025.1/weather/G0600770_2018.csv` (hour-ending,
  2018-01-01 01:00 to 2019-01-01 00:00), each restamped by its interval:
  the record's interval start (its stamp less 1 h) is moved to the same
  month, day and hour of 2020, and the record is stamped at that start
  plus 1 h. Feb 28 hour 24 (stamped 2018-03-01 00:00) becomes
  2020-02-29 00:00 and Dec 31 hour 24 becomes 2021-01-01 00:00; the file
  keeps 8760 records, contiguous except that no record covers Feb 29, so
  it is an actual 2020 file without Feb 29.

**Acceptance test** (the building and schedule are
`tests/fixtures/parity/cz4a_ashp_hpwh/`'s unless stated; 900 s unless
stated; warm-up disabled; a test that "reads" records compares the run's
outdoor dry-bulb column with B-1a's interpolation of the named records,
computed in the test from the parsed file):
- `leap_year_start_after_feb29_reads_its_calendar_date`: start
  2020-03-01T00:00:00-07:00 for 1 d on the cz4a typical file reads the
  Mar 1 records (the first hour from Mar 1's hour 24, `Hour24`).
- `leap_year_run_realigns_after_feb29`: a run from 2020-01-01T00:00:00-07:00
  for 70 d at 3600 s reads the Mar 1 records on every row labelled Mar 1.
- `typical_file_without_feb29_skips_the_day`: the `leap_year_february_900s`
  inputs (2024-02-27, 5 days, 900 s) produce 384 rows (480 less the 96 of
  Feb 29), the row after 2024-02-28 23:45 is labelled 2024-03-01 00:00, and
  that row reads the Mar 1 records.
- `day_types_after_a_skipped_feb29_follow_the_calendar`: on the same
  inputs, the day type of 2024-02-28 is Wednesday, of 2024-03-01 Friday
  and of 2024-03-02 Saturday (a weekend day: the schedule and the HPXML
  setpoint profile read their weekend values on it).
- `typical_file_with_feb29_uses_it`: the same run on `cz4a_with_feb29.epw`
  produces 480 rows; the steps ending Feb 29 01:00 to Mar 1 00:00 (hour 24
  of Feb 29) read 50.0 C; the steps ending Feb 29 00:15, 00:30 and 00:45
  read `f28h24 * (1 - w) + 50.0 * w` with `w` 0.25, 0.5 and 0.75
  (`f28h24` the Feb 28 hour-24 dry bulb); 2024-03-01 is a Friday.
- `non_leap_run_skips_the_files_feb29`: `cz4a_with_feb29.epw` from
  2023-02-28T00:00:00-07:00 for 2 d at 3600 s produces 48 rows; the rows
  labelled Mar 1 read the Mar 1 records and no row reads 50.0 C.
- `start_on_skipped_feb29_is_a_validate_error`: start
  2024-02-29T00:00:00-07:00 for 1 d on the cz4a typical file fails
  `DwellingBlueprint::validate()` naming 2024-02-29.
- `weather_and_schedule_agree_on_every_date_of_a_leap_year`: a 2024 run from
  Jan 1 for 366 days at 3600 s reads, on every simulated date, the weather
  and schedule rows of that month and day.
- `typical_year_wraps_by_day_of_year`: the `weather_wrap_900s` inputs
  (2023-12-30, 4 days) read Jan 1 of the file on 2024-01-01.
- `partial_year_file_is_an_error`: `cz4a_january.epw` from
  2023-01-25T00:00:00-07:00 for 5 d at 3600 s, a run inside the file's
  period, fails construction naming the data period 1/1 to 1/31 and the
  water-mains and ground-temperature inputs that need a complete year.
- `weather_is_read_by_standard_time_under_daylight_saving`: the
  `full_year_900s_dst` inputs (`civil_timezone = "America/Denver"`, feature
  `dst`) from 2023-07-01T00:00:00-07:00 for one day at 3600 s read, step
  for step, the same weather records as the same run without
  `civil_timezone`.
- `actual_year_run_outside_the_file_is_an_error`: the
  `tests/fixtures/resstock/2025.1/bldg0176227/` inputs with
  `G0600770_2018.csv` (the building's own weather station) and
  `weather_year = Actual` from 2019-01-01T00:00:00-08:00 for 1 d at 3600 s
  fail construction naming the file's range (2018-01-01 00:00 to
  2019-01-01 00:00) and the run's.
- `actual_year_leap_run_without_feb29_is_an_error`: the same building with
  `G0600770_restamped_2020.csv`, `Actual`, from 2020-02-27T00:00:00-08:00
  for 3 d at 3600 s fails construction naming 2020.

**Verification:** `cargo nextest run -p hares-core environment::`; goldens
re-captured, including the M-2 leap-day fixture; the commit's "Numeric
changes" line names the calendar-date weather rules and the re-captured
goldens.

**Commit:** `fix: address weather by calendar date with EnergyPlus typical and actual year rules`

**Dependencies:** B-1a; `05-python-surface.md` PY-8 (`weather_year` joins
the one `SimulationConfig`, `plan.md` §2.7); `12-inputs.md` IN-2
(`validate()`, which reports the start-on-Feb-29 error and the simulated
step count); `03-output-reporting.md` OR-7 (the row-count test and the
"Time" section this entry updates).

**Measurement:** on the M-2 leap-day golden, the row count (480 to 384) and
the delta on the days after Feb 29; the delta on every golden that starts
after Feb 29 of a leap year.

**State:** committed (2026-10-15, branch `row/r81-b1c`, worktree
`r81-b1c`, cut from `9356e2ec`). The weather is addressed by calendar
date: a new `WeatherYear {Typical, Actual}` config field (serde
`weather_year`, Python `weather_year="typical"|"actual"`, default
`Typical`, EnergyPlus's "Treat Weather as Actual" = No) joins the
forcing key and the resample cache key, and the reads go through a new
`WeatherAddressing` (hares-io): `Typical` maps the run step's local
standard time month, day and hour into the file's own calendar (the
365-day ordinal shares the schedule's `schedule_day_ordinal`; a 366-day
file reads the leap calendar's), wrapping past Dec 31 at the file's Jan
1 by day of year; `Actual` builds the file's dated-stamp index (the
records' `(date, slot-within-day)`, the stamps the parsers now keep as
the files state them: year, month, day, hour, minute), checks at
construction that the run's first and last steps lie inside the file's
dated records (an `IntervalEnd` file's range runs from its first stamp
less one interval to its last stamp) naming both ranges otherwise, and
fails a run that includes a Feb 29 the file has no records for, naming
the year. A file whose data period is not a complete year is a
construction error naming the period and the water-mains and
ground-temperature inputs (the EPW reader keeps the DATA PERIODS
period in `WeatherMeta`, parses the header's month/day fields by
EnergyPlus's both-numeric month/day rule, and accepts the count the
declared period names, so a partial file reaches construction whole);
the count error names the period and the same reason. The Feb 29 rule
the binding section states: the skip is
`weather_year == Typical && the file has no Feb 29 records`; the run's
clock (`SimClock.skip_feb29`) maps the step index to time with every
skipped Feb 29 inserted (the clock steps from the last step of Feb 28
to Mar 1 00:00) and `total_steps` is the run's simulated step count
(the calendar steps less the skipped day's), the forcing table and the
per-step path sharing the one mapping (`StepWeatherEvaluator::time_at`),
the `ResolvedClock` `validate()` reports carrying it, a start on a
skipped Feb 29 a `DwellingBlueprint` construction error naming the
date, and the day types every simulated date's readers see (the
schedule's and the HPXML profiles' weekday/weekend) the date's own
calendar weekday: the one departure from EnergyPlus, which advances the
day of week per simulated day and lands one weekday behind its own
Input Output Reference calls incorrect. The record lookup is by calendar
date never by a running index (`compute_annual_offset` deleted with its
tests, replaced by the addressing's unit tests, the leap-year-over-a-
365-day-file cases among them). The tracing at debug logs the mapping
and day type at each wrap and leap-day boundary. Fixtures:
`cz4a_with_feb29.epw` (the header's leap flag Yes, the 24 Feb 29
records as a block after Feb 28 hour 24, year 1996, dry bulb 50.0 C),
`cz4a_january.epw` (the period 1/1 to 1/31, 744 records),
`G0600770_restamped_2020.csv` (the 2018 records restamped by interval to
2020, no record covering Feb 29). RED first: the acceptance file
(`crates/hares-core/tests/weather_year.rs`, the entry's thirteen cases
plus the leap-crossing wrap differential and the actual-year positive
read) failed to compile at the base on the missing `WeatherYear` API
with only the tests applied, and the differentials failed at the parent
on the behaviour (the one-day-late reads, the 480-row Feb 29 runs, the
unguarded Feb 29 start). The forcing-key test
(`forcing_key_differs_by_weather_year`, in `environment.rs`'s module)
holds `Arc::ptr_eq` false for two managers over one parsed weather
differing only in the rule. PY-8's surface: the field joins the one
`SimulationConfig` (the constructor, the getter and setter, the stub,
`docs/python.md`'s Time section gaining the leap-day rule). OR-7's
`frame_starts_at_start_time_with_exact_row_count` unchanged and green
(its 2023 inputs carry no skipped day; the simulated count is the
engine's own everywhere). The schedule's `schedule_day_ordinal` doc
updated (the Feb 29 to Mar 1 slot applies only to a typical file that
has Feb 29 records). The gates at the final state (canonical env
`uv sync --locked --all-extras --all-groups` + `uv run maturin
develop`): fmt clean, clippy `--workspace --all-targets --all-features
-D warnings` clean, nextest `--workspace --all-features -j 2`
8088/8088 (6 skipped; the one `a_packed_ref_move_reruns_the_build`
failure under the raw invocation is the host's 1Password commit
signing, green with the git config neutralised and green at the base),
`pytest -m ''` 1755 passed + 6 xfailed, 0 failed, 0 errors, pyright 0,
the goldens: seven bitwise identical and the leap fixture's declared
movement re-captured twice byte-identical (the dst set's
`full_year_900s_dst` identical); the movement and the BESTEST drift
locks' re-measure in `02-measurement.md`'s calendar-weather section.
`docs/PHYSICS_DECISIONS.md` records the rule, the EnergyPlus behaviour
and the day-type departure with the citations.

## B-2: warm-up converges by the EnergyPlus criteria

**Kind:** numeric fix (`numeric_fixes`: EnergyPlus-grade warm-up
convergence including NaN). Fixes V22 and V24.

**What EnergyPlus does** (`vendors/EnergyPlus/src/EnergyPlus/HeatBalanceManager.cc:3308-3470`,
`Energy+.idd.in:536-575` `Building` fields N2 to N5,
`DataHeatBalance.hh:96-97`): the first day of the run period is repeated.
After each warm-up day, for every zone:
- `|max zone temperature today - max yesterday| <= TempConvergTol` and the
  same for the daily minimum (the zone's time-averaged mean air
  temperature `ZTAV` per timestep, `HeatBalanceManager.cc:3237-3241`;
  `Temperature Convergence Tolerance Value`, default 0.4 delta C, allowed
  `(0, 0.5]`);
- for the daily peak heating load and the daily peak cooling load (the
  zone's `airSysHeatRate` and `airSysCoolRate`, W, `:3243-3247`): when the
  peak exceeds 1e-4 W, both today's and yesterday's peaks are floored at
  100 W and `|today - yesterday| / today <= LoadsConvergTol` (`Loads
  Convergence Tolerance Value`, default 0.04, allowed `(0, 0.5]`); a peak
  at or below 1e-4 W passes. The IDD gives this field `\units W`
  (`Energy+.idd.in:542`), but the code uses it as a fraction.
- Warm-up ends when every check passes for every zone and at least
  `Minimum Number of Warmup Days` (default 1) have run, or when `Maximum
  Number of Warmup Days` (default 25; the IDD says it "should NOT be set
  less than 25") is reached, in which case the run continues and a
  `ShowSevereError` per unconverged zone names it and its residuals
  (`:3387-3424`). A minimum above the maximum resets the maximum to the
  minimum with a warning (`:686-697`).

**Problem:**
1. HARES compares only the last step's temperatures of conditioned zones
   between consecutive days, with a 0.5 C threshold, and never checks
   loads, daily extremes, unconditioned zones or a minimum day count.
2. The comparison folds with `f64::max`, which returns the other operand
   when one is NaN, so a NaN residual counts as converged.
3. `initialization_duration` is read only for being `Some`; its value is
   ignored, and the maximum is the literal 25.
4. At `ebdb1a17` the result of warm-up is only logged; OR-5 records it in
   `RunHealth.warmup` with the EnergyPlus residuals, but the decision is
   still today's rule.
5. The doc comment calls 0.5 C the EnergyPlus default and cites a section
   that does not say so.

**Evidence:** `crates/hares-core/src/dwelling/mod.rs:2985` (`if let
Some(_init_dur) = config.initialization_duration`), `:3000`
(`run_warmup_converged(0.5, 25)`), `:5360-5541` (`run_warmup_converged`:
last-step conditioned-zone temperatures, `fold(0.0_f64, f64::max)` at
`:5479`, warning only on non-convergence at `:5525-5541`), `:5353`
(the incorrect comment), `:2021` (Rust convenience default `Some(7 days)`),
`:3045-3050` (warning when warm-up is off).

**Change:**
- `initialization_duration` (and the synthetic `initialization_duration_s`,
  a `[simulation]` key until `05-python-surface.md` PY-8 moves it to the
  top level) is replaced by `SimulationConfig.warmup:
  Warmup` (`plan.md` §2.7: `Disabled` or `Converge(WarmupConfig)`, default
  `Converge(WarmupConfig::default())`, defaults 0.4, 0.04, 1, 25); the
  removed keys name `SimulationConfig.warmup` through two `REMOVED_KEYS`
  rows of scope `KeyScope::DwellingTable` (`plan.md` §2.7): the Python
  keyword and `DwellingConfig` field `initialization_duration`, and the
  synthetic TOML top-level key `initialization_duration_s`. `validate()` rejects a
  tolerance outside `(0, 0.5]`, a day count of 0, and `min_days > max_days`
  (EnergyPlus silently resets the maximum; HARES does not substitute
  values). The Rust convenience constructor takes the config's value like
  every other path. With the key gone, `0` can no longer mean one thing in
  TOML (a converging warm-up, `dwelling/mod.rs:2101-2104`)
  and another in Python (no warm-up, `py_dwelling.rs:2474-2480` at
  `872cf289`): every path states `warmup` or takes its one default.
  `frame_golden`'s manifests move in this commit (config-only;
  `02-measurement.md` M-1): the key is per home today
  (`[[home]] initialization_duration_s`, `tools/frame_golden/src/manifest.rs:56`)
  and `warmup` is a `SimulationConfig` field, so it goes to the manifest's
  `[simulation]` table. Every home of a manifest carries the same value
  (checked in the commit; a manifest whose homes differ stops the row). A
  value of `0` becomes `[simulation] warmup = "disabled"`; any positive
  value (BR-11's `604800`) becomes the default `warmup` (key omitted). This
  covers every manifest in `tests/fixtures/golden/`, the fleet manifests
  and the BR-14 and FL-1 manifests included. OR-16's `replay.toml` writes
  `warmup` in its `SimulationConfig` in place of `initialization_duration`,
  and `05-python-surface.md` PY-2's `removed_keyword_names_replacement`
  gains the `initialization_duration` row.
- Each warm-up day records, per zone (every zone, as EnergyPlus does), the
  daily maximum and minimum zone air temperature and the daily peak HVAC
  heating and cooling delivered to the zone (the per-zone values behind
  `HVAC Heating Delivered - {zone} (W)` and `HVAC Cooling Delivered -
  {zone} (W)`, `mod.rs:996-997`). The checks are the four above, with the
  100 W floor and the 1e-4 W threshold. The first warm-up day has no
  predecessor and is not converged (EnergyPlus compares it with a zeroed
  previous day, `HeatBalanceManager.cc:2888-2890`, which fails in practice).
- Stop rule: converged and `days >= min_days`, or `days == max_days`.
- A non-finite residual is not a comparison result: it fails the run with
  `NanDetected` naming the zone and the day. (OR-5 already makes a NaN zone
  temperature during warm-up fatal; this keeps the comparison itself from
  ever treating NaN as converged.)
- The stopping rule uses the `WarmupResiduals` OR-5 already records
  (`plan.md` §2.3); `RunHealth.warmup` is `WarmupOutcome::Ran { days_run,
  converged, residuals }`, or `Disabled` when `warmup` is
  `Warmup::Disabled`. Non-convergence is a health event (escalated under a
  strict policy) and a `RunResult.warnings` entry naming the zones, as
  OR-5 records it.
- The doc comment states the EnergyPlus criteria with the file references
  above. `tracing` at `debug`: the four residuals per zone per day.
- `docs/python.md` gains a "Warm-up" section (requirement R20): the four
  criteria and their defaults (`temperature_tolerance_c` 0.4 C on the
  daily maximum and minimum zone air temperature, `loads_tolerance` 0.04
  on the daily peak heating and cooling delivered with the 100 W floor,
  `min_days` 1, `max_days` 25), the stop rule, and that each warm-up day
  repeats the run's first day; that warm-up steps are never reported (the
  first report row is `start_time`); that `warmup=None` disables warm-up
  and an omitted `warmup` converges with the defaults; and that the
  outcome is read from `RunResult.health.warmup` (`WarmupOutcome`: days
  run, converged, the last day's residuals). Its example builds a dwelling
  with `warmup=WarmupConfig(max_days=2)`, runs it, and reads
  `health.warmup`; `test_docs_warmup_example` (Python) runs it as written.
- `docs/PHYSICS_DECISIONS.md` gains the warm-up section this file's
  preamble requires: the EnergyPlus criteria and their source lines, that
  OCHRE runs a fixed `initialization_time` with no convergence test
  (`vendors/OCHRE/ochre/Simulator.py:136-140`), and the direction of the
  days-run delta.

**Fixture selection (recorded, then pinned).** The candidates are the
inputs of BR-11's warm-up manifests plus one electric-resistance home,
each warmed up on its own first day: (A) `full_year_hourly` (cz2a gas
furnace and AC, 2023-01-01, 3600 s); (B) `full_year_900s_dst` (cz4a heat
pump, 2023-01-01, 900 s); (C) `leap_year_february_900s` (cz4a,
2024-02-27, 900 s); (D) `tests/fixtures/parity/cz6b_resistance_res_wh/`
(2023-01-01T00:00:00-07:00, 3600 s; its weather is the same Denver TMY3
file as cz4a's, so D adds a heating system type, not a colder climate).
When the entry's criteria are
implemented (test first), each candidate runs with `min_days = max_days =
25` so every day's four residuals per zone are recorded, and then with the
defaults and with each tolerance the tests vary; the table (candidate,
day, zone, residuals, `days_run`) goes to `02-measurement.md`'s log. Each
test below pins, by name, the first candidate in the order A to D that
meets its stated property; the test asserts the property, so a later
change that removes it fails the test instead of passing vacuously. If no
candidate meets a property, the row stops for an operator decision.

**Acceptance test:**
- `warmup_tolerance_changes_days_run`: on the first candidate whose
  `days_run` with `temperature_tolerance_c = 0.5` is smaller than with
  0.4 (a day's temperature residual lies in `(0.4, 0.5]`), the two runs
  report those recorded day counts.
- `warmup_load_criterion_extends_warmup`: on the first candidate whose
  temperature residuals are at or below 0.4 C from day 2 while a daily
  peak heating or cooling load still changes by more than 4 %, warm-up
  runs more days with the default `loads_tolerance` than with 0.5.
- `warmup_min_days_is_honoured`: on the first candidate that converges on
  day 2 under the defaults, `min_days = 3` runs exactly three days.
- `warmup_nonconvergence_is_recorded`: on the first candidate whose default
  `days_run` exceeds 2, `max_days = 2` returns `converged == false`,
  `days_run == 2` and a warning naming the unconverged zones.
- `warmup_nan_residual_fails_the_run`: on candidate A, a `#[cfg(test)]`
  hook overwrites the recorded day-1 daily maximum temperature of the
  living zone with NaN while the zone's thermal state stays finite (so
  OR-5's per-step NaN check, which fails a NaN zone temperature on every
  step, is not what fires, and day 1 has no residual of its own); the
  day-2 comparison fails the run with `NanDetected` naming that zone and
  day 2.
- `warmup_days_repeat_the_first_hour_rule` (a unit test of the per-step
  weather evaluation, `12-inputs.md` IN-4's one function): over B-1b's
  `solar_first_hour_uses_hour24_of_the_first_day` series, the first hour of
  warm-up day 1, of warm-up day 2 and of the run's first day each read the
  same four solar values (200, 100, 125, 150 under `Hour24`).
- `min_days_above_max_days_is_a_config_error`: `min_days = 3`, `max_days =
  2` fails `validate()` naming both.
- `warmup_toml_forms_round_trip`: `warmup = "disabled"`, an omitted key and
  a `[warmup]` table with all four fields each parse to the expected
  `Warmup` and serialise back to the same TOML (the `Warmup` serde is
  hand-written, `plan.md` §2.7).
- `03-output-reporting.md` OR-7's
  `frame_starts_at_start_time_with_exact_row_count` is re-run here with
  warm-up enabled: the first output timestamp equals `start_time` and the
  row count equals the simulated step count (B-1c).

**Verification:** `cargo nextest run -p hares-core dwelling::`;
`uv run --no-sync pytest tests/python -k test_docs_warmup_example`; goldens
re-captured; the commit's "Numeric changes" line names the warm-up
convergence criteria and the re-captured goldens; the
`docs/PHYSICS_DECISIONS.md` warm-up section is in the commit.

**Commit:** `fix: warm-up converges by the EnergyPlus temperature and load criteria`

**Dependencies:** `03-output-reporting.md` OR-5 (`RunHealth`,
`WarmupResiduals`, NaN during warm-up), OR-7, OR-16 (`replay.toml`);
`05-python-surface.md` PY-2, PY-8 (the one `SimulationConfig`);
`12-inputs.md` IN-2 (`validate()`); `01-baseline-repair.md` BR-11 (the
warm-up manifests); B-1a (first-hour rule during warm-up), B-1c.

**Measurement:** per golden with warm-up, days run before and after, and the
first-day energy delta on each of the candidates A to D.

**State:** committed `4d54618e` (branch `row/r83-b2-b3`, worktree
`r83-b2-b3`, based on `765968ed`). In brief: `SimulationConfig.warmup:
Warmup` (`Disabled` or `Converge(WarmupConfig)`, the hand-written serde
taking the string `"disabled"` or a `[warmup]` table, the Rust default
`Converge(WarmupConfig::default())` with the EnergyPlus defaults 0.4
delta C, 0.04, 1 day, 25 days; `validate()` rejects a tolerance outside
(0, 0.5], a day count of 0 and `min_days > max_days`, naming both), the
`initialization_duration` Python keyword and `DwellingConfig` settings
field and the synthetic TOML top-level `initialization_duration_s` all
removed (three `REMOVED_KEYS` rows: the two `DwellingTable` rows naming
`SimulationConfig.warmup`, and the `SimulationConfig`-scope row's
message rewritten to the same), the replay manifest's
`initialization_duration_s` gone with the warm-up riding its embedded
config (`REPLAY_SCHEMA_VERSION` 1 to 2), the frame_golden manifest's
per-home key gone (the manifest's `[simulation]` table carries the
`warmup` key; `SIMULATION_KEYS` gains it and its struct pin re-verified),
and every committed manifest and BESTEST fixture migrated (the zero
warm-ups spelled `[simulation] warmup = "disabled"`, the positive
values dropped to the default). The convergence loop is the EnergyPlus
`CheckWarmupConvergence` rule: every zone's (unconditioned included)
daily maximum and minimum temperature against the temperature tolerance
and daily peak HVAC heating and cooling delivered against the loads
tolerance with the 100 W floor and the 1e-4 W pass-through, the
delivery read per zone from the thermal port accumulators (the
per-zone values behind the run's `HVAC Heating/Cooling Delivered -
{zone} (W)` columns), the stop rule converged-and-past-`min_days` or
`max_days`, a non-finite residual failing the run with `NanDetected`
naming the zone and the warm-up day (the outcome recorded before the
failure propagates), the four residuals traced at debug per zone per
day, `WarmupResiduals` now the worst zone's own four residuals (the
worst zone by the daily-max temperature residual; the old aggregate
load attribution's doc comment gone), the build's warm-up driven by the
config (the disabled warm-up warns; the Rust convenience constructor
takes the config's value like every other path), and
`docs/PHYSICS_DECISIONS.md`'s warm-up section with the citations, the
days-run deltas and OCHRE's fixed `initialization_time`
(`vendors/OCHRE/ochre/Simulator.py:136-140`). RED first: the
acceptance tests failed at the base on the missing API; the fixture
selection's census and the two heavyweight candidates the entry's four
could not stand in for (the tolerance band and the load-blocked day
are the BESTEST 900 pair's), the days-run before/after table, the
first-day energy deltas, the drift locks' re-measure (every move inside
the locks' 1 percent, the pins re-captured with the cause) and the
OCHRE comparison's stated cold-both-sides premise are in
`02-measurement.md`. The goldens: `consumer_shape_900s` and
`fleet_same_offset_300s` bitwise identical (their warm-ups disabled);
the seven warmed fixtures' frames moved from row 0 (the initial
condition) and were re-captured twice byte-identical, the days-run
deltas their cause. The acceptance tests: `warmup_tolerance_changes_days_run`
on BESTEST 900FF (8 days at 0.5 against 9 at 0.4),
`warmup_load_criterion_extends_warmup` on BESTEST 900 (4 days at the
default loads tolerance against 3 at the loose 0.5),
`warmup_min_days_is_honoured` on the leap candidate (exactly 3),
`warmup_nonconvergence_is_recorded` on the cz2a candidate (the outcome
recorded and the warning naming the failing zones),
`warmup_nan_residual_fails_the_run` on the cz2a candidate (the test
hook's injected day-1 NaN, the day-2 comparison failing with
`NanDetected` naming the zone and day 2), `warmup_toml_forms_round_trip`
and `min_days_above_max_days_is_a_config_error` in `hares-io`'s config
tests, `warmup_days_repeat_the_first_hour_rule` beside B-1b's first-hour
test, and OR-7's `frame_starts_at_start_time_with_exact_row_count`
running warm-up-enabled in `time_column.rs`. The Python `warmup`
keyword of `SimulationConfig` (B-3's surface) landed in this commit:
the B-2 gate's parity tests state their cold-both-sides premise through
it, and the removal row's replacement keyword cannot be absent from the
surface the same commit strips the old spelling from. Gates at the tip:
fmt clean, clippy `--workspace --all-targets --all-features -D warnings`
clean, nextest `--workspace --all-features -j 2` 8101/8101 (6 skipped),
`pytest -m ''` 1763 passed + 6 xfailed, 0 failed, 0 errors, pyright 0,
the nine goldens bitwise after the declared movement (compare-all 8 +
dst 1).

## B-3: Python warm-up default equals Rust's

**Kind:** numeric fix (`numeric_fixes`: Python warm-up default equal to
Rust). Fixes V23.

**Problem:** the Python constructors default to no warm-up while the Rust
convenience constructor warms up. The calibration system never passes the
kwarg, so every evaluation starts from the initial state.

**Evidence:** `crates/hares-python/src/py_dwelling.rs:2420-2440`
(`Dwelling.from_hpxml` kwargs: `initialization_duration` absent means
`None`, and `0` also means `None`); `crates/hares-python/src/py_config.rs:582-648`
(`DwellingConfig`, `initialization_duration=None`);
`crates/hares-core/src/dwelling/mod.rs:2021` (Rust default);
consumer `calibration/engine.py:320-346` (never passes it).

**Change:** `SimulationConfig(warmup=...)` in Python (the one config type,
`05-python-surface.md` PY-8) takes B-2's `Warmup`: an absent keyword means
`Warmup::default()`, the Rust default; `warmup=None` passed explicitly
means `Warmup::Disabled` and is recorded as `Disabled` in
`RunHealth.warmup`; a `WarmupConfig(...)` (a frozen pyclass over the Rust
type) sets the criteria. The removed `initialization_duration` keyword of
`from_hpxml` and `DwellingConfig` is an error naming `warmup`. The defaults
come from the one Rust table, not a Python copy. `_hares.pyi` states the
three cases.

**Acceptance test:**
- `python_and_rust_warmup_defaults_agree`: a Python dwelling built with no
  `warmup` kwarg and the Rust convenience constructor, both on the
  `tests/fixtures/parity/cz4a_ashp_hpwh/` inputs from
  2023-01-01T00:00:00-07:00 for 2 days at 3600 s, report equal
  `WarmupOutcome`.
- `explicit_none_disables_warmup`: the same inputs with `warmup=None`
  report `Disabled` and no warm-up day runs.
- `initialization_duration_kwarg_names_warmup`: the same cz4a inputs with
  `initialization_duration=0` passed to `Dwelling.from_hpxml` and,
  separately, to `DwellingConfig` each raise the removed-key error naming
  `SimulationConfig.warmup`.

**Verification:** `uv run --no-sync pytest tests/python`; `frame_golden
compare-all`: the manifests run through the Rust adapter, which B-3 does
not change, so no golden changes. The commit's "Numeric changes" line
states that Python dwellings now warm up by default and that no golden was
re-captured.

**Commit:** `fix: Python dwellings warm up by default, as Rust ones do`

**Dependencies:** B-2; `05-python-surface.md` PY-2 (strict kwargs).

**Measurement:** B-3's effect is a default value, so no golden records it.
Recorded instead: after this entry, the per-column maximum absolute
difference between a Python run of the `consumer_shape_900s` inputs with no
`warmup` keyword and the same run with `warmup=None`, and the warm-up days
the first ran. The
calibration consumer's fit moves by this change (it never passes the
keyword); B1 absorbs it (`08-consumer.md` CN-4).

**State:** committed `3935e940` (branch `row/r83-b2-b3`, worktree
`r83-b2-b3`, on B-2's `4d54618e`). In brief: the `WarmupConfig` frozen
pyclass over the Rust type (a partial construction keeps the Rust
defaults for the unset fields; the validation the TOML path runs runs
here too) and the `SimulationConfig.warmup` keyword with the three
cases: an omitted keyword is the Rust default (`Warmup::default()`,
distinguished from an explicit `None` by the keyword's
`PyWarmupArg::Absent` signature default, a custom `FromPyObject`
extractor over `WarmupConfig | None`), an explicit `warmup=None` is
`Warmup::Disabled`, and a `WarmupConfig` sets the criteria; the
`warmup` getter/setter read `WarmupConfig | None`; `warmup` joins
`SIM_CONFIG_FIELD_KEYWORDS`, so a `warmup=` keyword on the dwelling
constructors names `config=SimulationConfig(warmup=...)`; the stub
carries the three cases in `SimulationConfig`'s docstring and the
`WarmupConfig` class; `python/hares/__init__.py` exports
`WarmupConfig`; the dwelling constructors take the config only as
`config=`, so `warmup=` beside it is the config-field TypeError, not a
removed key. `SimulationPlan`'s segments 1+ pass `warmup=None` through
their configs (the segment 0 warm-up's state the
`restore_building_state` transfer discards otherwise). The acceptance
tests: `python_and_rust_warmup_defaults_agree` (the Python surface and
the Rust convenience constructor, both on the cz4a inputs for 2 days
at 3600 s, equal `WarmupOutcome`s; in `py_result.rs`'s tests, where the
crate-private Python surface is reachable),
`explicit_none_disables_warmup` and
`test_warmup_config_sets_the_criteria` (a non-converging home at
`max_days=2` records `Ran { converged: false, days_run: 2 }`),
`initialization_duration_kwarg_names_warmup` (both dwelling
constructors), `test_docs_warmup_example` (the docs' Warm-up section's
example, which builds a dwelling with `WarmupConfig(max_days=2)` and
reads `health.warmup`, runs as written), and the docs/python.md
Warm-up section with the three cases and the defaults. The measurement
is in `02-measurement.md`: the no-keyword run warmed 2 days, converged;
the per-column maximum absolute differences against `warmup=None` over
48 h at 900 s (the zone temperatures up to 1.358 delta C, the gas
power 0.086 therms/hour, the total electric power 0.046 kW). The
calibration consumer never passes the keyword and warms up from this
row on; B1 absorbs its fit (`08-consumer.md` CN-4). No golden
re-captured: the golden manifests run through the Rust adapter, which
B-3 does not change. Gates at the tip: fmt clean, clippy
`--workspace --all-targets --all-features -D warnings` clean, nextest
`--workspace --all-features -j 2` 8101/8101 (6 skipped), `pytest -m ''`
1763 passed + 6 xfailed, 0 failed, 0 errors, pyright 0, the nine
goldens bitwise (compare-all 8 + dst 1).

## B-4: billing month boundary

**Kind:** numeric fix. Fixes V6.

**Note (2026-10-02, from BR-14's implementation):** the tariff
evaluator's `BillingCycle::Monthly` periods are ANNIVERSARY-based
(`compute_period_end`, `billing.rs:115`: a Jan 25 start closes Feb 25),
not civil-month; BR-14's `month_boundary_billing_3600s` fixture
consequently records ONE billing summary (the run's single period),
not the two a civil-month closure would give. When this entry fixes
the one-step-late closure, its acceptance must be stated against the
anniversary semantics (or the civil-month boundary is a separate
change with its own delta); the fixture's golden pins today's
one-period behavior and will move if the semantics change.

**Note (2026-10-04, adversarial check of OR-19 `ba44d9f1`):** the
anniversary anchor drifts: a Dec 31 start closes Jan 31, Feb 29, Mar 29
and never returns to the 31st, because each period's end is computed
from the previous end, not from the anchor day. This entry fixes it:
each period end is the anchor day clamped to the month's length
(Dec 31 start: Jan 31, Feb 29, Mar 31, Apr 30, ...), with a test over a
leap year starting on the 29th, 30th and 31st.

**Problem:** in a dwelling run, the first step of each new billing period
is accumulated into the period that is closing. The tariff evaluator's
contract is that `step()` receives the end time of the step: it folds the
step in, then closes the period when that time has reached the period end.
The dwelling passes the step's start time instead, so the period closes one
step late, after the next period's first step has been folded into it.

**Evidence:** `crates/hares-tariff/src/evaluator.rs:618-634` (update, then
`if current_time >= self.billing_state.period_end` closes the period);
`evaluator.rs:1166-1180` (the evaluator's own tests pass "end-of-step time as
current_time"); `crates/hares-core/src/dwelling/mod.rs:7048-7051` (the
dwelling passes `self.latest_env.current_time`), set from
`clock.current_time()` at `environment.rs:750` before `clock.next()` at
`mod.rs:7134`.

**Change:** `mod.rs:7050` passes the step's end time, `current_time + dt`,
converted to the evaluator's zone. `evaluator.rs:618-634` is unchanged, so
its contract and tests stand. `tracing` at `debug`: the period close time and
the step end that triggered it.

**Acceptance test:** `dwelling_billing_boundary_step_belongs_to_its_own_month`,
on the inputs of `01-baseline-repair.md` BR-14's `month_boundary_billing_3600s`
(`tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh/`, start
2023-01-25T00:00:00-07:00, 12 days at 3600 s, tariff
`tests/fixtures/golden/flat_tariff.json` in `America/Denver`; a `Timestep`
`Memory` report of the net electric power): `BillingPeriodSummary`
records no per-step membership (`crates/hares-tariff/src/billing.rs:409-420`),
so membership is read from the totals. The January summary's
`total_import_kwh` equals, on bits, the step-order sum of each step's
import energy (the positive part of the step's net electric power in kW
times 1 h, computed as the evaluator's update computes it) over the 168
steps labelled Jan 25 00:00 to Jan 31 23:00, and the February summary's
equals the same sum over the 120 steps labelled Feb 1 00:00 to Feb 5
23:00. At the parent commit the January total also holds the Feb 1 00:00
step and the February total lacks it, so the test fails there.

**Verification:** `cargo nextest run -p hares-tariff -p hares-core`;
goldens re-captured (billing summaries change; frame columns do not); the
commit's "Numeric changes" line names the billing month boundary and the
re-captured goldens.

**Commit:** `fix: pass the step end time to the tariff evaluator`

**Dependencies:** M-1, M-2; `01-baseline-repair.md` BR-14 (the
`month_boundary_billing_3600s` golden and the manifest `[tariff]` table).

**Measurement:** per-period billing delta on `month_boundary_billing_3600s`
(January loses, February gains the Feb 1 00:00 step's energy and cost). The
consumer's `calibration/ami2/tariff.py` bills move by this change; recorded
in B1's attribution.

**State:** committed `45cd54e4` (branch `row/r85-b4`, worktree `r85-b4`,
on B-3's `3935e940`). The reconciliation the two notes asked for, read
against the row 32 record (the monthly cycle closes on the start's
day-of-month anniversary, so a 7-day run crosses no boundary): the
anniversary semantics stand, and the civil-month boundary is not taken
here; the acceptance is restated against the anniversary semantics
rather than the entry's original civil-month split, so the entry's
original verification line about re-captured billing goldens does not
apply. The rule as landed:

- The step-end convention: the dwelling's post-solver fold passes the
  step's end time, `current_time + dt`, converted to the evaluator's
  zone; `evaluator.rs`'s contract and tests stand unchanged. The period
  closes on the fold of the step its boundary ends, so a step's energy
  bills to the period whose span holds it; `tracing` at `debug` names
  the close time and the triggering step end.
- The anchored anniversary (the 2026-10-04 note's fix): `BillingState`
  carries the cycle's anchor day, the first period's start day of month,
  unchanged across the closes; each period end is the anchor day clamped
  to that month's length. The containing-period walk follows the same
  anchored chain, and the checkpoint carries the anchor
  (`BillingStateSnapshot.anchor_day`; the tariff snapshot schema version
  2 to 3). A December 31 start closes January 31, February 29, March 31,
  April 30 and keeps returning to the 31st.
- The mid-run convention becomes the from-start run's: a tariff attached
  exactly at a period boundary and a from-start run both book the
  boundary step to the period it opens (the close fires on the fold
  whose end reaches the boundary, before the boundary step's own fold).
  The landed pin
  `a_tariff_attached_at_a_period_boundary_books_the_boundary_step_to_the_new_period`
  was restated from the one-step asymmetry to the agreement, its
  February totals now bitwise equal, with January's total pinned to its
  own steps' sum; the attach-at-boundary full-charge pin stands.

The acceptance test `dwelling_billing_boundary_step_belongs_to_its_own_month`
runs the `month_boundary_billing_3600s` inputs held 35 days at 3600 s
(January 25 to March 1), spanning the February 25 00:00 MST anniversary
boundary: the first period's `total_import_kwh` is, on bits, the
step-order sum of its own 744 steps' import energy (the positive part of
each step's net electric power in kW times one hour, computed as the
evaluator's update computes it), the second period's its own 96, the
bounds January 25 to February 25 and February 25 to the March 1
finalize, and both fixed charges the full monthly charge (the boundary
close at its own end, no proration; the finalize's full monthly charge
plus the daily charge for the four days elapsed, zero here). RED at the
parent on the totals: the first period held the boundary step
(941.51821526152 against the fixed 941.0620277257223, the 0.4562 kWh
step among them) and the second lacked it. The anchor tests: 
`period_ends_keep_the_anchor_day_across_the_leap_year` (December 2023
starts on the 29th, 30th and 31st walked through the leap February by
the close chain; RED at the parent on the 30th's and 31st's chains),
`containing_period_walk_keeps_the_anchor_day` (the walk past a clamped
boundary returns February 29, March 31; RED at the parent), and
`billing_state_snapshot_carries_the_anchor_day` (a state standing on a
clamped boundary restores with its anchor; RED at the parent). The
measurement is in `02-measurement.md` (the boundary-crossing run's
per-period totals; the boundary-attach convention's change). The
`docs/PHYSICS_DECISIONS.md` section "The billing month closes on the
start's day-of-month anniversary" and the `docs/python.md` Numeric
changes line of 2026-10-15 carry the rule and the direction of the
delta: no golden fixture's numbers move (the one tariffed golden closes
no billing boundary inside its 12-day run; all nine bitwise, compare-all
8 + dst 1), a run spanning a cycle boundary moves the boundary step's
energy, cost and peak from the closing period's summary to the opening
one's, the totals across the periods unchanged, every kWh billed exactly
once. Gates at the tip: fmt clean, clippy `--workspace --all-targets
--all-features -D warnings` clean, nextest `--workspace --all-features
-j 2` 8105/8105 (6 skipped), `pytest -m ''` 1764 passed + 6 xfailed,
0 failed, 0 errors, pyright 0, the goldens bitwise (compare-all 8 +
dst 1).

## B-5: end-use rollup by typed kind, not by name string

**Kind:** numeric fix. OR-1 builds the catalogue with end use still taken
from today's name classifiers, so its schema is bitwise today's; this entry
switches end use to the typed descriptor and moves the numbers. Fixes V9
and V10.

**Problem:** end use, EV columns and HVAC columns are chosen by matching
display names.
- `equipment_name_to_end_use` matches exact literal names, so a name that
  carries an instance suffix such as `"PV #1"` would fall to
  `EndUse::OTHER` where the `" #N"` suffix is not stripped first. Today
  this is a latent defect, not an observed one: `assign_instance_names`
  (`crates/hares-io/src/hpxml/equipment.rs:425-446`) writes `"PV #1"` only
  into `instance_name` and the typed config's name and leaves `spec.name`
  `"PV"`, which the cited sites read.
- `is_ev` is a substring test for `"ev"`, so `"Television"` gets EV columns.
- `is_hvac_or_wh` is a substring test for `"heater"`, so `"Pool Heater"` and
  `"Spa Heater"` are treated as HVAC or water heating.
- Five more classifiers decide columns and behaviour from the name:
  `has_soc`, `is_heat_pump_heater`, `is_cooling_equipment`, `is_pv` and
  `is_compressor_equipment`.

**Evidence:** `crates/hares-io/src/output/columns.rs:178-245`
(`equipment_name_to_end_use`), `:1140-1143` (`is_ev`), used at
`columns.rs:870` and `crates/hares-core/src/dwelling/mod.rs:698`;
`columns.rs:1089-1103` (`is_hvac_or_wh`); name-based end use without suffix
stripping at `mod.rs:770` and `crates/hares-core/src/dwelling/blueprint.rs:212`
(`metrics.rs:1252-1258` does strip it). The other name classifiers:
`display_name_to_end_use_key` (`columns.rs:321`),
`parse_end_use_electric_power_column` and `..._key` (`columns.rs:374, 382`),
the `" #"` stripping at `metrics.rs:1254, 1281`, and
`has_soc`, `is_heat_pump_heater`, `is_cooling_equipment`, `is_pv` and
`is_compressor_equipment` (`columns.rs:1105-1147`), called at
`columns.rs:539, 546, 691, 706, 708, 790, 832, 854, 866` and
`dwelling/mod.rs:595-598, 697-698, 7191-7193`.
`extract_unit_from_name` (`columns.rs:27` at `ebdb1a17`) is already
removed by `03-output-reporting.md` OR-1, which reads units from the
catalogue.

**Change:** end use, and whether an equipment has EV, HVAC, SOC,
heat-pump-heater, cooling, PV or compressor columns and behaviour, come
from `EquipmentDescriptor.end_use` and the descriptor's declared variables
through the catalogue. The one function inside the catalogue builder that
still calls the classifiers (`03-output-reporting.md` OR-1 change item 4)
is deleted, and every name classifier listed above is deleted with its
call sites. `tracing` at `debug`: each equipment's end use at catalogue
build.

**Acceptance test** (the HPXML fixtures run with
`tests/fixtures/weather/test_location.epw` from 2023-01-01T00:00:00-07:00
for 2 days at 3600 s, warm-up disabled, with a `Timestep` `Memory` report
of the verbosity-8 selection, the only selection that holds every
name-gated column (they exist at verbosity 3, 7 and 8), and a schedule the
test writes: `data/examples/BEopt_example_schedule.csv` with each column the
fixture's equipment reads and that file lacks added as a constant 0.5,
the added columns listed in the test):
- `instance_qualified_equipment_rolls_up_by_typed_end_use`:
  `tests/fixtures/hpxml/ochre_samples/base-pv.xml` declares two PV systems
  (`PVSystem1`, `PVSystem2`), built as `"PV #1"` and `"PV #2"`; both roll up
  under `EndUse::PV` and the PV end-use column equals their sum. The test
  is written first and run at the parent commit, and the result is
  recorded in the commit body: if it passes there (expected, see Problem),
  it is a regression guard for the latent defect; if it fails, it is the
  defect's test.
- `pool_and_hot_tub_heaters_are_not_hvac`:
  `tests/fixtures/hpxml/ochre_samples/base-misc-loads-large-uncommon.xml`
  (a pool with a gas-fired heater, `Pool1Heater`, and a hot tub with an
  electric resistance heater, `HotTub1Heater`): each heater rolls up under
  its own end use and has no HVAC columns.
- `misleadingly_named_equipment_classifies_by_its_descriptor`: a unit test
  over the catalogue builder with one descriptor named `"Television"`
  (end use plug load, no EV variables) has no EV columns, and one named
  `"EV Charger"` with the EV end use has them; names are never read.
- `no_name_classifier_remains`: `git grep -n -E
  'fn (equipment_name_to_end_use|is_ev|is_hvac_or_wh|has_soc|is_heat_pump_heater|is_cooling_equipment|is_pv|is_compressor_equipment|display_name_to_end_use_key|parse_end_use_electric_power_column|extract_unit_from_name)\b'
  crates` returns nothing (run in the verification step).

**Verification:** `cargo nextest run -p hares-io -p hares-core`; goldens
re-captured (end-use rollups and the columns of the misnamed equipment
change); the commit's "Numeric changes" line names the typed end-use
rollup and the re-captured goldens.

**Commit:** `fix: classify equipment by its typed end use, not its name`

**Dependencies:** `03-output-reporting.md` OR-1.

**Measurement:** the golden deltas (expected none: no golden fixture has a
multi-instance or ambiguously named equipment, recorded as measured), and
the end-use rollup of the two HPXML fixtures above before and after.

**State:** committed `f6f001b8` (branch `row/r86-b5`, worktree
`r86-b5`, based on `b5858446`, one commit; the tree carried the work
recovered from the stashed earlier attempt, judged coherent and
completed here). In brief: the end-use machinery moved to the typed
declarations: `EndUse` gained `is_hvac_or_water_heating`,
`is_cooling_family`, `display_name` and `from_display`
(`hares-types/src/equipment.rs`, the display round trip and the two
family predicates unit-pinned); `hares_equipment` gained
`ClassDeclaredSurface` and `class_declared_surface(class)`, the end
use, core capabilities and declared telemetry the class's own
constructor declares, read once per process from the registry's
construction of the class at its default configuration (the two
tankless classes at the gas configuration their canonical resolvers
write), pinned equal to the constructed descriptors for every
registered class; `CatalogueEquipment` carries `end_use` and
`core_capabilities`, `Catalogue` carries
`equipment_end_uses()` (the instance-qualified names paired with
their typed end uses), `EquipmentColumnFamilies::of_descriptor`/
`of_input` replace the name classifiers (the heat-pump-heater family
is the pan heater power declaration within the HVAC heating end use,
the SOC family the HAS_SOC capability), the metrics plan takes the
catalogue's typed table for the equipment-for-end-use gate and the
per-end-use equipment counts, the blueprint's
`remove_equipment_by_end_use` and the dwelling's aggregate-index
build and first-HVAC position read the declarations, and
`build_schema_for_specs` reads the class's declared surface. Every
name classifier is deleted
(`equipment_name_to_end_use`, `end_use_display_name`,
`display_name_to_end_use_key`, `parse_end_use_electric_power_column`,
`is_hvac_or_wh`, `has_soc`, `is_heat_pump_heater`,
`is_cooling_equipment`, `is_pv`, `is_ev`, `is_compressor_equipment`),
the classifier grep over crates returns nothing, and `tracing` at
debug names each equipment's end use at catalogue build. RED first
(the three acceptance tests run and recorded at the parent; the
results are in the commit body): the instance-qualified PV rollup
FAILED at the parent (no PV aggregate existed; the two systems'
power rolled into Other, so the defect was observed, not latent),
the pool and spa heater test FAILED at the parent (the "heater"
substring put them in the HVAC family), and the catalogue unit test
was a compile RED. The pinned name-string tests migrated or retired
with the reason (the classifier unit tests travel with their
functions, the EV v8 pin reads the typed end use, the all-classes
EV code-column count restated 10 to 6, the mid-run HVAC add pin and
the five stub poses re-posed by declaration). The movement: all nine
goldens re-captured twice byte-identical (the `Cooking Range`'s
power from `cooking` to `other` and the resstock homes' `TV` from
`other` to `plug_loads` re-bucket every fixture's per-end-use
metrics, totals conserved; the full per-golden deltas and the two
HPXML fixtures' before/after rollup sums in the measurement log),
the schema pins re-captured twice byte-identical with every moved
column attributed (the phantom EV columns gone, the pool and spa
heaters out of the HVAC family, the ideal load, the `HPWH` alias
and the indirect tank into it, the ground-source and water-loop
heat pump heaters' defrost and ER columns in). The re-capture
generator's output format now matches the pins' pretty form (a
re-capture that moves nothing diffs to nothing). Gates at
`f6f001b8`: fmt clean, clippy `--workspace --all-targets
--all-features -D warnings` clean, nextest `--workspace
--all-features -j 2` 8108/8108 (7 skipped), `pytest -m ''` 1764
passed + 6 xfailed, 0 failed, 0 errors, pyright 0, the goldens
bitwise after re-capture (compare-all 8 + dst 1), the schema pins
equal, `docs/python.md`'s Numeric changes line carries the rollup
and the re-captured goldens.

## B-6: null for not-applicable values, never 0.0

**Kind:** numeric fix. Fixes V14.

**Problem:** `record_step` zeroes its row and then fills most cells with
`unwrap_or(0.0)` or `map_or(0.0, ...)`, so a quantity that is undefined in
the current state (the sensible heat ratio while not cooling, the COP of
equipment that is not running, the SOC of equipment with no storage, a
setpoint before the first reconcile, an actor telemetry key the actor did not
emit) is written as a real 0.0. The same pattern also covers cells that are
genuinely zero (power of equipment that is off) and cells that are sums, so
a wholesale replacement would be as wrong as today.

**Evidence:** `crates/hares-core/src/dwelling/mod.rs:7166`
(`record_scratch.fill(0.0)`, which is also the reset for the summed cells),
`:7183-7310` (per-equipment cells), `:7308` (`row[idx] += ...DUCT_LOSS_W`,
summed), `:7317-7326` (end-use aggregates, summed), `:7329-7337` (actor
telemetry: an absent key leaves the zeroed cell), `:7256-7259` (power factor
1.0 at zero apparent power), `:7460-7496` (setpoint telemetry "0.0 is the
safe pre-init sentinel").

**Change:** the producers write `None` for a quantity that is undefined
in the current state, and the catalogue decides, per variable, what a
missing value means. Today no producer emits `None` for the quantities
below, so the catalogue rule alone would yield no null:
- COP is always `Some` (`crates/hares-equipment/src/hvac/heat_pump/heater.rs:1583, 2663`;
  `hvac/air_conditioner.rs:1409, 1943`). The heat-pump heater, the
  heat-pump cooler and the air conditioner write COP `None` when the
  step's runtime is 0 or the mode is Off.
- The sensible heat ratio is inserted as 1.0 at init
  (`hvac/ac_config.rs:32`) and set on every step
  (`hvac/air_conditioner.rs:1263`). The init insert is
  removed, and the cooling equipment writes SHR only when the step's
  sensible cooling is above 0, `None` otherwise. An air-source heat pump is
  two pieces of equipment (`ASHP Heater` and `ASHP Cooler`); only the
  cooler has an SHR variable.

Per variable, through `VariableDef.nullable` (`plan.md` section 2.2):
- Not nullable (a quantity that is physically zero when its component is
  inactive): the producer writes 0.0 when inactive; a missing value is a
  wiring defect that fails the run (OR-5). Electric power, gas power,
  energy, capacity (thermal output), reactive power, backup resistance
  power, fan power, main power, runtime fraction, latent gains, and the
  on/off schedule state.
- Nullable (undefined in some states, or not reported by this source): a
  missing value is null. Operating mode, setpoint, SOC, COP, sensible heat
  ratio, speed index, defrost state, power factor at zero apparent power
  (today 1.0), the scheduled and runtime setpoint columns before the first
  reconcile, every actor telemetry field, and each verbosity-8 pass-through
  field as its `TelemetryField` declares.
- Summed cells (duct losses, end-use aggregates): a zeroed accumulator per
  step as today; null only when the dwelling has no contributing
  equipment, which the catalogue knows at assembly.
- Metrics and aggregation: a sum skips nulls; a mean divides by the count of
  non-null cells and the count is reported as the variable's companion
  valid-count variable; a period or fold with no non-null cell is null.
  Declared per `VariableDef` (contract in `plan.md` section 2.2).
- The table above is added to `docs/PHYSICS_DECISIONS.md`. `tracing` at
  `debug`: each variable's nullability at catalogue build.

**Acceptance test** (on the `tests/fixtures/parity/cz4a_ashp_hpwh/` inputs,
an air-source heat pump and a heat-pump water heater, from
2023-01-01T00:00:00-07:00 for 2 days at 900 s, warm-up disabled, unless
stated):
- `undefined_quantities_are_null`: the test first asserts that the run has
  at least one step in which the `ASHP Heater` is Off and at least one in
  which it heats (two January days in Denver); then the `ASHP Cooler`'s
  sensible heat ratio is null in every step in which it is not cooling
  (every step of the run), and the `ASHP Heater`'s COP is null in every
  Off step and non-null in every heating step.
- `inactive_power_is_zero_not_null`: the heat pump's electric power and
  runtime fraction are 0.0 in every off step.
- `missing_non_nullable_value_fails_the_run`: a test equipment type
  registered in the test module, declaring `Electric Power` non-nullable and
  never writing it, added to the same dwelling, fails the run with a
  wiring-defect error naming the variable.
- `absent_actor_key_is_null`: a test actor declaring telemetry key `x` and
  emitting it only on even steps: `x` is null on every odd step.
- `mean_excludes_nulls_and_reports_valid_count`: an hourly mean over four
  steps with one null equals the mean of three and reports a valid count of 3
  (a unit test over the stated four values).

**Verification:** `cargo nextest run -p hares-core -p hares-io`; goldens
re-captured (cells change from 0.0 to null); the commit's "Numeric
changes" line names the null cells and the re-captured goldens.

**Commit:** `fix: undefined output values are null, not 0.0`

**Dependencies:** `03-output-reporting.md` OR-1 (`nullable`), OR-2 (the row
plan's validity bitset), OR-5 (wiring defects fail the run), OR-13 (the
reporting-frequency reduction); `04-fleet.md` FL-1 (the fleet fold's null
rule).

**Measurement:** per golden, the number of cells that change from 0.0 to
null per variable, and any metric that changes.

**State:** committed `81e843f7` (branch `row/r87-b6`, worktree
`r87-b6`, based on `0e897801`). The substitution sites on the record path
(`crates/hares-core/src/dwelling/mod.rs` `record_step`): the state
columns (mode, setpoint, SOC, speed, defrost), the v8 pass-through reads
and the actor keys now write through the row plan's writer
(`RowPlan::writer`/`RowWriter`), which marks every written slot in the
row 37 validity bitset and leaves an unwritten slot invalid, so the cell
is null; the power factor writes null at zero apparent power (the 1.0
sentinel gone); the four setpoint-chain columns write the telemetry
value or nothing (the pre-reconcile 0.0 sentinel gone); the not-nullable
quantities keep their 0.0 substitutions (the physically-zero-when-
inactive reading) and the failed-step branch writes the not-nullable
cells' zeros explicitly, so a rolled-back step records the same zeros as
today with its nullable cells (COP, SHR, power factor, the v8 flows)
null. The summed cells: the bindings' builder computes the slots whose
value is the row's zero every step (the duct-losses slot when the
dwelling has HVAC; every bound end-use aggregate; a not-nullable column a
mid-run roster change orphaned against the frozen schema, which keeps
today's zero rather than inventing a tombstone null) and the step marks
them valid. The producers: the ASHP heater and the air conditioner (the
ASHP/MSHP/GSHP/WSHP coolers are its class) publish COP `None` when the
step's compressor input is 0 (the acceptance's runtime-0/Off rule; the
telemetry channel keeps its historical 0.0, it feeds no output column)
and the air conditioner writes SHR only when the step's sensible cooling
is above 0, the init insert of 1.0 removed; the restore instants publish
COP `None` when the restored mode is idle. The core contract's HAS_COP
presence rule is relaxed to the one-directional form (undeclared never
populated; a COP published when absent stays a defect) with the
plausibility bounds unchanged. The catalogue declares every variable's
nullability per the entry's table (the not-nullable classes flipped from
the all-true placeholder; the telemetry-sourced not-nullable columns and
their equipment-side `TelemetryField` declarations flipped together, the
assembly's contradiction check demands it), each variable's nullability
logged at catalogue build at `debug`; `Telemetry::remove` publishes a
channel's per-step absence (a NaN payload only `remove` can store, the
key set stable, so the snapshot's in-place copy keeps its fast path;
equality reads through the marker). `StreamingRecorder::push_row` takes
the row's validity and the batches carry Arrow nulls; the checkpoint's
`ReportRowsSnapshot` carries the rows' validity (`CHECKPOINT_VERSION` 17
to 18, re-pinned). The reducer (row 39) and the fleet fold (row 50)
needed no change: `push_batch` reads the columns' validity and the fold
skips a home's null cells; the fleet tests' period builder made
null-aware (the reducer's own sum/valid-count rule). The Python surface
is unchanged code: the batches go to polars through pyo3-arrow and the
nulls arrive as None (verified: the unmet column all null, the zone
temperature and the total power never null). The metrics are untouched
(their input slots are all not-nullable produced slots) and B-7's
verbosity independence is not this row's. Acceptance tests (all RED
first: `lacking_equipment_class_columns_are_null`'s probe failed at the
base on `'Scheduled Heating Setpoint (C)' row 0` reading the row's
zero): `undefined_quantities_are_null`,
`inactive_power_is_zero_not_null`,
`missing_non_nullable_value_fails_the_run` (the wiring-defect error is
the core contract's OR-5 failure naming the equipment and the missing
channel), `absent_actor_key_is_null`,
`mean_excludes_nulls_and_reports_valid_count` (in `reduce.rs`), and
`lacking_equipment_class_columns_are_null` (the prompt's RED test). The
goldens re-captured TWICE BYTE-IDENTICAL (compare-all 8 + dst 1): the
movement is exactly two classes, read off the frame diff against the
parent's materialized frames: the schema's nullable flags (the not-
nullable classes' declaration flips) and the `Unmet HVAC Load (C)`
column (no producer claims it in any dwelling: its cells read the
row's zero at the parent and null after; the cells 0.0-to-null per
golden: `consumer_shape_900s` 35040, `full_year_900s_dst` 35040,
`full_year_hourly` 8760, `leap_year_february_900s` 384,
`month_boundary_billing_3600s` 288, `weather_wrap_900s` 384,
`fleet_mixed_offset` aggregate 165, `fleet_same_offset_300s` aggregate
192, `resstock_2025_fleet` aggregate 48), whose covered-weight cells
move to the fold's honest zero. The metrics, the health and the billing
products are unchanged in every golden; the deeper v3+ nulls (COP, SHR,
mode, setpoint, SOC, speed, defrost, the v8 pass-throughs, the
setpoint chain, the duct losses) are pinned by the acceptance tests and
do not reach the v2 fixtures. Gates: fmt clean; clippy `--workspace
--all-targets --all-features -D warnings` clean; nextest `--workspace
--all-features -j 2` 8111/8111 (7 skipped), run once;
`pytest -m ''` 1764 passed, 6 xfailed, 0 failed, 0 errors; pyright 0.

## B-7: metrics independent of verbosity

**Kind:** numeric fix. OR-8 moves metrics onto the catalogue and row plan
while reproducing today's verbosity-dependent values bit for bit; this entry
removes the dependence. Fixes V29, with OR-8.

**Problem:** the envelope-component metrics are computed only when the
recorded schema has the verbosity-4 envelope columns, so the same run
reports different metrics at different verbosities.

**Evidence:** `crates/hares-io/src/output/metrics.rs:593-604`
(`has_envelope_columns` decides from which columns exist).

**Change:** when a run requests metrics, every metric is computed from the
catalogue's sources whatever the selected reports, so `RunResult.metrics`
is identical at every verbosity and with no report at all
(`dl-i08-architecture.output_model`, `metrics_optional`: "When requested,
they are independent of verbosity and report selection"). A run that does
not request metrics is unchanged (`None`, nothing computed). `tracing` at
`debug`: which metric groups were computed.

**Acceptance test:** `metrics_equal_at_every_verbosity_and_with_no_report`:
the `tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh/` inputs from
2023-01-01T00:00:00-07:00 for 7 days at 3600 s, warm-up disabled, run with
`metrics = true` at verbosity 0, 4 and 8 and with an empty report list give
bitwise-equal `RunMetrics`.

**Verification:** `cargo nextest run -p hares-io metrics::`; goldens
re-captured (metrics of runs below verbosity 4 change); the commit's
"Numeric changes" line names the metrics now independent of the selection
and the re-captured goldens.

**Commit:** `fix: compute every run metric whatever the selected reports`

**Dependencies:** `03-output-reporting.md` OR-8 (bug-compatible structural
commit).

**Measurement:** per golden, the metrics delta at verbosity 0 before and
after (after: equal to the verbosity-8 values).

**State:** committed `d7cfb0b9` (branch `row/r89-b7`, worktree of
`62763042`; the row also carries `4f970245`, the base commit's
unformatted re-export list rustfmt'd, its own style commit). The
fix is the union surface: when the run requests metrics, the row plan's
declared surface is the full producer surface (every catalogue variable
the producers can produce, the diagnostics block appended when
requested), so the `MetricsInputPlan`'s optional slots resolve the same
bits at every verbosity and with no report; the availability evaluation
the decoupling entry kept (the union of the reports' variables, the
`Verbosity(0)` preset for no reports) and with it the freeze-time
refusal of a metrics request whose selection lacks the power column die
here (the power column is a verbosity-0 catalogue variable, always on
the producer surface; the refusal's pin became
`narrow_first_report_with_the_metrics_request_returns_full_metrics`).
`tracing` at `debug` names the metric groups computed at finish. The
RED first: `metrics_equal_at_every_verbosity_and_with_no_report` failed
at the parent on the cz2a probe (verbosity 0: per_end_use empty,
`envelope_loads_kwh` None, both HVAC COPs None; verbosity 8: populated;
the totals identical both ways, so the defect was the components'
availability alone); GREEN after (all four runs bitwise equal and
bitwise the parent's verbosity-8 values, measured on the probe's
printed bits). The Python pins that masked the defect updated with it:
`test_envelope_loads_none_at_low_verbosity` became
`test_envelope_loads_equal_at_low_verbosity` (verbosity 0 equals
verbosity 6 field-wise) and `test_metrics_after_simulate`'s COP band
([1.0, 5.0], dead code at the parent: the COP was None at verbosity 1)
became Some/finite/positive, the fixture's heating plant a gas furnace
whose ratio is delivered thermal over the end use's fan electric, not a
compressor COP. The goldens re-captured TWICE BYTE-IDENTICAL
(compare-all 8 + dst 1): the movement is exactly the three metrics
fields per home (`envelope_loads_kwh` and the two HVAC COPs, null to
the verbosity-8 values), every frame product and schema bitwise; the
per-golden values are in `02-measurement.md` and the "Numeric changes"
line in `docs/python.md`. Gates: fmt clean; clippy `--workspace
--all-targets --all-features -D warnings` clean; nextest `--workspace
--all-features -j 2` 8122/8122 (7 skipped), run once;
`pytest -m ''` 1764 passed, 6 xfailed, 0 failed, 0 errors; pyright 0.

## B-8: deterministic fluid-solver summation

**Kind:** dead-code removal with zero delta, in the numeric block because
it closes the `numeric_fixes` item "deterministic fluid-solver summation".
Fixes V8.

**Problem:** the fluid solver carries a hash-order summation,
`resolved.values().sum()` over a `HashMap<FluidNodeId, f64>`, as the
fallback for the total loop flow when no pump node is found. Floating-point
addition is not associative and the map's iteration order is seeded per
process, so if that fallback ever ran, the total could differ bitwise
between processes. It cannot run: resolved flows exist only when a pump
node was found, and the pump's own resolved flow is always in the map. The
dead fallback, and the second search for the pump that feeds it, stand in
the way of reading the code as deterministic.

**Evidence** (at `21d90588`): `crates/hares-envelope/src/fluid_solver.rs:334-351`
(`resolved_node_flows` is `Some` only inside `if let Some(pump) =
pump_node`, the first `Source` node with no incoming edge), `:469`
(`resolved.insert(pump.node_id, pump_resolved)`: the pump's flow is always
resolved), `:746-767` (`pump_node_id`: the same predicate recomputed on the
unchanged topology, so it finds the same pump), `:785-787` (`pump_total =
pump_node_id.and_then(|nid| resolved.get(&nid).copied()).unwrap_or_else(||
resolved.values().sum())`: the `unwrap_or_else` arm is unreachable). The
resolution maps are built internally with `HashMap::new()` (`:315, 370`),
so no test can choose their hasher, and a topology without a pump node
yields `None` and never enters the branch. Every dwelling's fluid solver is
built with an empty topology map today
(`crates/hares-core/src/dwelling/solver_builder.rs:1655`,
`FluidSolverConfig::default()`; `fluid_solver.rs:65`).

**Change:**
- The flow-resolution block returns `Option<(FluidNodeId, f64,
  HashMap<FluidNodeId, f64>)>`: the pump node, its resolved flow and the
  resolved node flows.
- `pump_node_id` (`:746-767`) is deleted; the plumbing-node set takes the
  pump node from the resolution.
- `pump_total` is the pump's resolved flow carried out of the resolution,
  with no lookup and no fallback, so the hash-order sum is gone.
- No other code on this path changes and no number moves. Because no sum
  over a hash map's values remains in the solver, the decision item
  "deterministic fluid-solver summation" (`dl-i08-architecture.numeric_fixes`)
  is met by construction; the commit body says so.

**Acceptance test:** the existing `fluid_solver` tests pass unchanged, and
`git grep -n -F 'resolved.values().sum()' crates` prints nothing (run in the
verification step). No new test: the removed arm had no reachable input.

**Verification:** `cargo nextest run -p hares-envelope fluid_solver::`;
`frame_golden compare-all` passes with every golden unchanged.

**Commit:** `refactor: carry the pump's resolved flow out of the fluid-loop flow resolution`

**Dependencies:** none beyond M-1 and M-2.

**Measurement:** none beyond the golden record (every golden unchanged).

**State:** committed `4a47661e` (branch `row/r90-b8`, worktree
`r90-b8`, based on `247d2a5b`, one commit). In brief: the
flow-resolution block returns the pump node, its resolved flow and
the resolved node flows as one typed triple,
`Option<(FluidNodeId, f64, HashMap<FluidNodeId, f64>)>`; the
plumbing-node set takes the pump node from the resolution, and the
pump's resolved flow carried out of the resolution is the loop's
total flow, with no second search for the pump, no lookup and no
fallback. The evidence's line numbers had drifted from `21d90588`;
at the row's base the sites sat at `fluid_solver.rs:506` (the
pump's flow inserted into the resolved map), `:535` (the block's
only `Some`, returned inside `if let Some(pump) = pump_node`),
`:750-768` (the second pump search) and `:789-791` (the lookup and
its hash-order fallback). Reachability, checked before the edit:
the map the fallback summed is `Some` only when the pump was found,
the pump's own flow is inserted before that return and never
overwritten afterwards (the non-splitter fill skips the pump's
node), and the deleted second search recomputed the same
Source-role zero-parent predicate on the unchanged topology (the
only write between the resolution and the aggregation is a splitter
branch's requested-flow snapshot, which touches neither the node
roles nor the edges), so the lookup always hit and no input, test
or process, could reach the fallback arm. That absence of any
reachable input is why no red test was written and the entry's
stated proof form ran instead: the existing 32 `fluid_solver` tests
pass unchanged, and `git grep -n -F 'resolved.values().sum()'`
over `crates` prints nothing. No number can move: the carried value
is the same f64 the lookup returned, bit for bit, read from the
same map through the same borrow; the aggregation's plumbing set
gains the same pump node the deleted `Option` carried; the
post-resolution invariant and the effective flows read the map
unchanged; and the `None` arm (no topology, no splitters, no pump)
is untouched. Gates at `4a47661e`: fmt clean, clippy
`--workspace --all-targets --all-features -D warnings` clean,
nextest `--workspace --all-features -j 2` 8122/8122 (7 skipped; the
first run's one failure was the fresh worktree's uninitialised
OCHRE submodule, populated with `git submodule update --init` and
the suite re-run whole), `pytest -m ''` 1764 passed + 6 xfailed,
0 failed, 0 errors, pyright 0, the goldens bitwise (compare-all 8
identical + the dst set 1 identical).

## B-9a: `Fleet::from_resstock` builds fleets that can run

**Kind:** defect fix (found while giving B-9 a fixture;
`dl-i08-process.found_defects`). It moves no number: no fleet this
function builds today can run, so there is no number to move. It lands
directly before B-9, its own row and commit, and adds the golden B-9
records its delta on.

**Problem:** `Fleet::from_resstock` cannot build a runnable fleet.
- Every building's HPXML and schedule path is the dataset's zip archive
  path (`building_energy_models/upgrade=0/bldg0000002-up00.zip` for
  2025.1), and nothing in the engine reads HPXML or schedules from a zip.
  The Python data layer (`hares.data.resstock`) extracts each building to
  `<dir>/bldg{id:07}/home.xml` and `in.schedules.csv`, the layout of
  `tests/fixtures/resstock/2025.1/` and of the consumer's cache.
- `weather_fips` is never set, so every building's weather path is empty
  and only a log line reports it.

**Evidence:** `crates/hares-io/src/resstock.rs:204-227` (`build_zip_path`),
`:307-317` (`hpxml_path` and `schedule_path` are the zip path,
`weather_fips: None`); `:424` (`parse_weather_station_fips`, which reads
the HPXML `WeatherStation/Name` and is never called on this path);
`crates/hares-fleet/src/fleet.rs:141-215` (`from_resstock`: the empty
weather path at `:193`), `:836-849` (`resolve_weather_path_for_building`
returns `None` without a FIPS; lines at `ebdb1a17`); `parse_weather_station_fips`
returns `Option`, so a station name it cannot read is silently no FIPS;
`python/ochre_next/data/resstock.py:297-331,
425-460` (extraction to `home.xml`/`in.schedules.csv`; the FIPS from the
HPXML weather station name); the 2025.1 fixtures' `WeatherStation/Name`
(`bldg0019694`, `bldg0162486`, `bldg0352714`: `G5300330`).

**Change:**
- `parse_resstock_metadata(path, version, buildings_dir)` addresses each
  building as `buildings_dir/bldg{id:07}/home.xml` and `in.schedules.csv`,
  the layout the Python data layer writes; `build_zip_path` and the zip
  paths go. A building whose directory or files are missing is an error
  naming the building and the path.
- `weather_fips` comes from the building's HPXML through
  `parse_weather_station_fips`, the rule the Python data layer applies.
  It returns `Result`: an HPXML without a `WeatherStation/Name`, or a name
  that is not a county FIPS, is an error naming the building and the
  value read.
- The Python `Fleet.from_resstock` keyword `hpxml_dir` is renamed
  `buildings_dir` (a `REMOVED_KEYS` row of scope `KeyScope::PythonKeyword`
  naming it, `plan.md` §2.7), and its docstring states the layout.
- `frame_golden` gains `kind = "resstock_fleet"`: keys `metadata`,
  `buildings_dir`, `weather_dir`, `version`, `duration` (seconds),
  `features` and `defaults`, and the `[fleet]` table (a `FleetReport`, as
  `04-fleet.md` FL-1's fleet manifests carry it); weights come from the
  metadata's weight column. Unknown keys are rejected.
- `.gitignore` gains `!tests/fixtures/resstock/**/*.parquet` (next to the
  parity exception at `.gitignore:54`), so the fixture's metadata is
  tracked.
- New fixture `resstock_2025_fleet`: metadata
  `tests/fixtures/resstock/2025.1/metadata_fleet3.parquet`, the rows of
  `bldg_id` 19694, 162486 and 352714 (`in.state` WA, `in.county`
  `G5300330`, weight 253.90367272727272, upgrade 0) with every column of
  the 2025.1 upgrade-0 metadata, extracted from the 2025.1 upgrade-0
  metadata file, which is not in the repository:
  `upgrade0_2025_1.parquet` in the ResStock metadata cache
  (`~/.cache/ochre_next/resstock_meta/` before `08-consumer.md` CN-13's
  cache move, `~/.cache/hares/resstock_meta/` after it), 465,060,991 bytes,
  sha256 `83a288b30b5a83d0b5aded3102acae052c554c587cdb5f7293816325b9a48e06`.
  The commit body records the extraction command, which names that file
  and its hash and writes the three rows in `bldg_id` order; buildings
  `tests/fixtures/resstock/2025.1/`; weather
  `tests/fixtures/resstock/2025.1/weather/` (`G5300330_2018.csv`);
  version 2025.1; duration 172800 s; `[fleet]` with
  `frequency = "hourly"` and the verbosity-2 selection. The golden is
  captured in this commit on the unfixed start rule (2019-01-01 UTC, the
  function's 1-minute step).

**Acceptance test:**
- `resstock_fleet_reads_the_extracted_layout`: the fixture's three
  entries name `.../bldg0019694/home.xml` and `in.schedules.csv` (and the
  same for the other two) and weather `G5300330_2018.csv`.
- `resstock_missing_building_directory_is_an_error`: metadata naming a
  building with no directory under `buildings_dir` fails naming the
  building and the path.
- `resstock_unreadable_weather_station_is_an_error`: a copy of
  `bldg0019694/home.xml` in a temporary `buildings_dir` with its
  `WeatherStation/Name` removed fails naming `bldg0019694`.
- `test_from_resstock_hpxml_dir_names_buildings_dir` (Python):
  `Fleet.from_resstock(..., hpxml_dir=...)` raises the removed-key error
  naming `buildings_dir`; `05-python-surface.md` PY-2's
  `removed_keyword_names_replacement` table gains the row.
- `resstock_2025_fleet` is captured and `compare-all` passes.

**Verification:** `cargo nextest run -p hares-io resstock:: -p hares-fleet
fleet::`; `[pytest]` (the renamed keyword); `frame_golden compare-all`.

**Commit:** `fix: build ResStock fleets from extracted buildings and their weather stations`

**Dependencies:** `01-baseline-repair.md` BR-15 (fleet manifest keys);
`04-fleet.md` FL-1; `05-python-surface.md` PY-2 (`REMOVED_KEYS`).

## B-9: ResStock weather year and offset are declared, not hardcoded

**Kind:** numeric fix. Fixes V42, with B-9a.

**Problem:** ResStock fleets start every home at 2019-01-01 00:00 UTC+0,
whatever the weather year and the site's offset, so a 2025.1 fleet runs
2019 dates on AMY 2018 weather. A building whose weather file cannot be
resolved gets an empty path and only a log line.

**Evidence:** `crates/hares-fleet/src/fleet.rs:779-799`
(`resstock_sim_config`: `FixedOffset::east_opt(0)`, `2019, 1, 1`),
`:168-194` (unresolved weather: `tracing::warn!`/`error!`, then
`PathBuf::from("")` at `:192`), `:814-825` (file name per release: 2024.x
`{fips}.epw` TMY3, 2025.1 `{fips}_2018.csv` AMY 2018, the dataset's own
naming).

**Change:**
- The start is Jan 1 00:00 local standard time of the weather file's own
  year (read from the parsed file's dated records, B-1c) for an
  actual-year release; for a typical-year release the year is a required
  argument of `Fleet::from_resstock` (no built-in year). A file's own year
  is the calendar year whose hours its records cover: for an `IntervalEnd`
  file such as `G5300330_2018.csv` (2018-01-01 01:00 to 2019-01-01 00:00)
  that is 2018, not the year of its last stamp. The offset is the site's
  standard offset from its resolved location.
- `Fleet::from_resstock` declares `weather_year = Actual` for 2025.1 (AMY)
  and `Typical` for 2024.x (TMY3) (B-1c).
- A building whose weather file cannot be resolved or read is a fleet
  construction error. Every building is resolved before the error is
  raised, so the one error names every unresolved building with its
  county.
- The `write_output`/`retain_batches` lines of `resstock_sim_config` are
  removed by OR-3, which removes the fields; this entry does not touch them.
- `tracing` at `debug`: each home's start and offset.

**Acceptance test:**
- `resstock_actual_year_fleet_runs_the_weather_year`: the B-9a fixture
  (`resstock_2025_fleet`'s metadata, buildings and weather) starts every
  home at 2018-01-01T00:00:00-08:00 (Washington, `G5300330`) with
  `weather_year = Actual`.
- `resstock_typical_year_needs_a_year`: `Fleet::from_resstock` with
  version 2024.2 and no year is a construction error naming the argument,
  raised before any file is read.
- `unresolved_resstock_weather_is_an_error`: the same metadata with
  `weather_dir` pointing at an empty directory fails construction with one
  error naming all three buildings (`bldg0019694`, `bldg0162486`,
  `bldg0352714`), each with county `G5300330`.

**Verification:** `cargo nextest run -p hares-fleet fleet::`;
`resstock_2025_fleet` re-captured (dates and weather alignment change);
the commit's "Numeric changes" line names the ResStock weather year and
offset and `resstock_2025_fleet`.

**Commit:** `fix: ResStock fleets run the weather file's year at the site's offset`

**Dependencies:** B-9a, B-1c; `04-fleet.md` FL-1.

**Measurement:** delta on `resstock_2025_fleet` (annual energy per home and
timestamps; the weather the homes read moves from 2019 dates on 2018
records to 2018 dates, and the clock from UTC to UTC-8).

**State:** committed (branch `row/r91-b9`, worktree `r91-b9`, based on
`588b4c51`). The hardcodes, found where the entry names them:
`resstock_sim_config`'s `FixedOffset::east_opt(0)` and
`with_ymd_and_hms(2019, 1, 1, 0, 0, 0)` (the start every home ran) and
its `weather_year: WeatherYear::default()` (the typical rule over AMY
files), and the unresolved-weather path's
`weather_path.unwrap_or_else(|| PathBuf::from(""))` behind the
`tracing::warn!`/`error!`. RED first, at the parent `588b4c51`: the
acceptance tests failed to compile there on the missing `typical_year`
argument (E0061, only the tests applied), and the two differentials,
temporarily written to the parent's signature, failed on the behaviour -
`resstock_actual_year_fleet_runs_the_weather_year` read
`2019-01-01T00:00:00+00:00` against the declared
`2018-01-01T00:00:00-08:00` (the differential: the file's own year 2018
against the hardcoded 2019, the site's -08:00 against the hardcoded
+00:00), and `unresolved_resstock_weather_is_an_error` built the fleet
with all three homes' weather pointing into the empty directory
(`weather_resolved: 0, weather_failed: 3`, the warned empty path). GREEN
at the commit: `resstock_sim_config` takes the resolved start and the
declared year (no year, no offset in it); the release declares the
weather-year rule (`resstock_weather_year`: 2025.1 `Actual`, 2024.x
`Typical`); the actual-year start's year is each file's own year, read
from the parsed file's dated stamps through the new
`WeatherTimeSeries::file_year` (hares-io; the covered interval from the
stamps and record label, the majority year its own: the 2018 file
stamped 2018-01-01 01:00 to 2019-01-01 00:00 reads 2018, not the last
stamp's year), and the typical-year start's year is the new required
`typical_year` argument (no built-in year; the check fires before any
file is read, and a year passed to an actual-year release is a
conflict error naming it); the offset is the site's standard offset
through the new `site_offset_from_inputs` (hares-core, the
`site_offset` resolution over the inputs directly, the same
`resolve_site_location` and checked conversion the built dwelling and
the alignment pre-pass run); every building's weather is resolved
before the error is raised, one `ResStockWeatherUnresolved` naming
every failing building with its county, the site-offset failures
joined; `tracing::debug!` carries each home's start and offset. The
Python surface: the binding gains the `typical_year=None` keyword (the
stub with it). The wrap crossing pinned at the fleet level
(`resstock_typical_year_fleet_wraps_by_day_of_year`): a 2024.2 fleet at
the declared year 2023 runs 367 days at the hour step through the
LocalClock alignment and the last-but-one day's 24 hourly rows read the
file's own Jan 1 records bit for bit. The test buildings'
`write_extracted_building` gained the `BuildingSummary/Site` the
offset resolution reads (the released HPXML documents carry it), and
the Python binding tests' synthetic buildings with it; the
`resstock_2025_fleet` manifest, buildings and weather unchanged. The
delta (in `02-measurement.md`'s ResStock section): the movement is the
`Time` labels' year in `aggregate` and `covered_weight` (48 cells each,
2019 to 2018); `homes`, `fleet_report` and every energy, power and
weather column bitwise (the homes read the same records of the file at
the same step indices; the schedule's month-and-day rows likewise), so
the entry's "annual energy per home" premise does not arise on the
2-day fixture. Gates at the row: fmt clean, clippy `--workspace
--all-targets --all-features -D warnings` clean, nextest `--workspace
--all-features -j 2` 8129/8129 (7 skipped), `pytest -m ''` 1764 passed
+ 6 xfailed, 0 failed, 0 errors, pyright 0, the goldens: compare-all 8
identical, the dst set 1 identical, `resstock_2025_fleet` re-captured
twice byte-identical.

## B-10: missing defaults directory is an error

**Kind:** numeric fix (a silent-default removal that changes which
numbers a run produces, not just an error path: the class-table fallback
uses different HVAC curves and ZIP coefficients than the real defaults
store). Fixes V19.

**Problem:** a missing or unreadable defaults directory degrades to an
empty `DefaultsStore` with a warning, and every equipment silently falls
back to class-table defaults with different curves and coefficients. The
path defaults to `"defaults"` relative to the working directory, so the
numbers a run produces depend on where it was started.

**Evidence:** `crates/hares-core/src/dwelling/blueprint.rs:127-130`
(`unwrap_or_else(|| PathBuf::from("defaults"))`), `:131-146` (malformed TOML
errors; any other load error warns and uses `DefaultsStore::empty()`). A
second relative fallback: `crates/hares-core/src/dwelling/mod.rs:2352-2355`,
`bp.defaults_path.clone().unwrap_or_else(|| PathBuf::from("defaults"))`,
the path `inject_schedule_into_specs` reads. Inside a load that succeeds,
`DefaultsStore::load` (`crates/hares-io/src/defaults.rs:461-520`) still
downgrades failures: an envelope LUT load failure is a warning after which
the lumped `fallback_r` path runs (`:492-498`); the generator curve, the
water-heating CSV and the vehicle mapping are optional sub-tables whose
absence is silent (`:503-513`); and the multispeed-HVAC table is strict
only when the `HARES_STRICT_DEFAULTS` environment variable is set (`:462`).
Production caller that passes no path: `crates/hares-fleet/src/fleet.rs:200`;
`crates/hares-core/src/dwelling/mod.rs:1959, 1986` (the convenience
constructors). Tests: `fleet.rs:1174, 1191`,
`tests/regression/helpers.rs:95, 129, 254`,
`crates/hares-core/tests/engine.rs:158-464`,
`crates/hares-core/tests/wiring_properties.rs:146-464`,
`crates/hares-core/tests/autosize_from_config.rs:181`.

**Change** (`dl-i08-architecture.defaults_path`):
- `DefaultsSource.base` (`plan.md` §2.7) becomes a required `PathBuf`; both
  working-directory defaults go, and `inject_schedule_into_specs` takes
  `DefaultsSource.base`. The production callers above pass their own
  explicit path (the fleet from its configuration, the Rust convenience
  constructors from a new required argument), and the test callers above
  pass the repository's `defaults` directory as an absolute path.
- Any load failure is a construction error naming the path: a missing,
  unreadable or empty defaults directory, a malformed file, an envelope
  LUT that fails to load (the `fallback_r` downgrade goes), and every
  sub-table the loader reads (the generator curve, the water-heating CSV,
  the vehicle mapping and the multispeed-HVAC table become required).
  `HARES_STRICT_DEFAULTS` and `load_with_strict_defaults` are removed: the
  loader has one behaviour, the strict one.
- Python: `defaults=None` (the default) means the defaults shipped with the
  package, resolved from the installed package's own location
  (`hares.defaults_dir()`, documented in `docs/python.md`); an explicit
  path overrides it. Nothing depends on the working directory. Today the
  only Python mechanism is the explicit `defaults_path` keyword
  (`crates/hares-python/src/py_config.rs:557,582`, `py_dwelling.rs:2441-2446`);
  without it Rust falls back to the relative `defaults`. `pyproject.toml:75-81`
  includes `defaults/**` with a glob that resolves relative to the project
  root, outside the `python/` package source, and nothing in Python
  resolves an installed location. The build places the tree inside the
  package, at `hares/defaults/` (138 MB, of which 135 MB is `defaults/ev`),
  and `hares.defaults_dir()` resolves it from the package's own location;
  a source checkout built with `maturin develop` resolves the same function
  to the same packaged path.
- `tracing` at `debug`: the resolved defaults path.

**Acceptance test:**
(Each dwelling-level test builds the `tests/fixtures/parity/cz4a_ashp_hpwh/`
inputs from 2023-01-01T00:00:00-07:00 for 1 d at 3600 s with the stated
defaults path; a partial tree is a temporary directory of symbolic links
to the repository's `defaults/` files, as `02-measurement.md` M-1's
adapter builds one, with the stated file left out or replaced.)
- `missing_defaults_directory_is_a_construction_error`: a path under a
  fresh temporary directory that does not exist fails naming it.
- `unreadable_defaults_directory_is_a_construction_error`: the defaults
  path names a regular file (a temporary file), which no user, root
  included, can list as a directory; construction fails naming it.
- `empty_defaults_directory_is_a_construction_error`: an empty temporary
  directory fails naming it.
- `envelope_lut_load_failure_is_a_construction_error`: a partial tree
  without `envelope/Envelope Boundaries.csv` (one of the three files
  `EnvelopeLookup::load` reads, `envelope_lut.rs:217-220`) fails naming
  that file; the run never reaches the `fallback_r` path.
- `missing_sub_table_is_a_construction_error`: a partial tree without
  `ev/vehicle_mapping.csv` fails naming it, with `HARES_STRICT_DEFAULTS`
  unset.
- `wheel_carries_defaults_inside_the_package`: `maturin build --release`
  into a temporary directory; the wheel's file list holds every file of
  `defaults/` under `hares/defaults/` and no top-level `defaults/` entry.
- `python_default_defaults_are_the_packaged_tree` (Python, run from a
  temporary working directory): a dwelling built with no `defaults`
  keyword reads `hares.defaults_dir()`, and its `Provenance.inputs`
  defaults digest equals a dwelling built with that path given explicitly.

**Verification:** `cargo nextest run --workspace`; `[pytest]`; goldens
re-captured. Each golden records its repository-relative defaults
directory and a digest of every defaults file read (M-1), so a golden
captured against another defaults tree or from another working directory
is detectable. The commit's "Numeric changes" line names the defaults
directory rule and the re-captured goldens.

**Commit:** `fix: require an explicit defaults directory and fail when it cannot be read`

**Dependencies:** `06-runtime-memory.md` RT-4 (`DefaultsSource`),
`12-inputs.md` IN-1 (the defaults digest the Python test compares),
`05-python-surface.md` PY-7 (the package is `ochre_next`, under
`python/ochre_next`, until PY-7 renames it `hares`, which this entry's
`hares.defaults_dir()` and `hares/defaults/` assume).

**Measurement:** the golden record; expected zero delta for every golden
captured with a working defaults directory, recorded as measured.

**State:** committed (2026-10-16, branch `row/r92-b10`, based on
`8e44a61e`, one commit; the schedule row is 92). The strictness sites'
fates. The blueprint-level error existed at the parent (the OD-64/O-49
policy, pinned by `crates/hares-core/tests/defaults_store_policy.rs`):
the entry's missing, unreadable and empty acceptance tests passed at the
parent and pin it, and the store load's `MissingFile` was the error
behind them. What this row adds. (1) The required path everywhere:
`DefaultsSource.base` is `PathBuf`, the `Default` impl is deleted, the
two working-directory fallbacks are gone (`load_with_overrides` takes
`&Path`; `ParsedDefaults::with_file` uses the recorded base), the
`inject_schedule_into_specs` relative fallback did not exist any more at
the parent (RT-4's shared handle: the injection reads the parsed
handle's profiles table), the Rust convenience constructors take the
new required argument (`Dwelling::from_hpxml`'s `defaults: &Path`,
`from_hpxml_with_reports`' `defaults_path` required,
`from_ochre_kwargs` too), `Fleet::from_resstock`'s `defaults_path` is
required (`&Path`; the fleet test that pinned the unset shape is
rewritten to pin the directory reaching every dwelling's config), the
test callers point at the shipped tree through the one helpers
(`shipped_defaults_dir()`, now `python/hares/defaults`), and
`ParsedDefaults::empty`/`from_parts` record the empty base no file is
read from (the synthetic path's typed declaration, untouched).
(2) The loader-internal strictness: `HARES_STRICT_DEFAULTS` and
`load_with_strict_defaults` are removed (one behaviour, the strict one;
the env-var pin asserts the load is identical with the variable set and
unset), the envelope LUT's warn-then-fallback is an error
(`DefaultsError::EnvelopeLut`; the lumped `fallback_r` downgrade is
gone from the load, `envelope_lut()` stays `Option` only for the
synthetic empty store whose boundaries never consult it),
`Envelope Boundaries.csv` is required with the LUT's other two files
(the hardcoded boundary-name fallback remains the resolve-time rule for
a triple the CSV map lacks), the HVAC multispeed CSV is required (no
relaxed mode), and the water-heating CSV is required
(`load_water_heating_csv` returns `Result`); the generator curve and
the EV vehicle mapping were already required at the parent (verified:
`load_generator_curve`'s typed error, OD-24's mapping rule).
(3) The Python packaged defaults: `hares.defaults_dir()` (a pyfunction
resolving `hares._hares`'s own `__file__`'s parent + `defaults`,
returning a `pathlib.Path`), the default defaults source of every
Python surface (`Dwelling.from_hpxml`, `DwellingBlueprint.from_hpxml`,
`DwellingConfig`, `Fleet.from_resstock`, `hares.replay`) is the
packaged tree, an explicit path overrides it, `ParsedDefaults(base,
files)`'s base is required, and nothing depends on the working
directory. (4) The packaging: the tree moved `defaults/` →
`python/hares/defaults/` (git mv, the bytes untouched), the pyproject
`include` glob deleted (the tree rides `python-source`; at the parent
the wheel carried no defaults at all: the glob matched nothing), the
wheel holds 56 defaults files under `hares/defaults/` and no top-level
`defaults/` entry. `tracing` at `debug`: the resolved defaults path at
the load and the packaged resolution. RED first, witnessed at the
parent: the store-level probes (`missing_multispeed_csv_is_a_load_error`,
`water_heating_csv_missing_file_is_a_load_error`,
`envelope_lut_missing_files_are_a_load_error`,
`hares_strict_defaults_env_var_is_ignored`) and the dwelling-level
`envelope_lut_load_failure_is_a_construction_error` failed on the
warn-then-fallback behaviour, and the Python
`python_default_defaults_are_the_packaged_tree` (no `defaults_dir`, the
no-keyword construction reading the working directory) and
`wheel_carries_defaults_inside_the_package` failed; GREEN the same
tests at the row. The wheel acceptance runs explicitly
(`--run-ignored only`; it builds a release wheel). The goldens:
`compare-all` 8 identical and the dst set 1 identical, every product
bitwise, the defaults digest unchanged; re-captured at the row (the movement is
the manifests' `defaults_dir` provenance string, `defaults`
to `python/hares/defaults`, and the capture head; the compare's second
run of every manifest is the bitwise re-verification). Gates: fmt
clean, clippy `--workspace --all-targets --all-features -D warnings`
clean, nextest `--workspace --all-features -j 2` 8135/8135 (8 skipped:
the ignored generators, this row's wheel pin among them), `pytest -m ''`
1766 passed + 6 xfailed, 0 failed, 0 errors, pyright 0.

## B-11: Mode aggregation for multi-valued enum columns

**Kind:** numeric fix (`numeric_fixes`: Mode aggregation;
`dl-i08-architecture.aggregation`). Fixes V49 (its explicit `Mode`
aggregation kind).

**Problem:** operating-mode codes are averaged like physical quantities, so
a period that visits codes 0, 0, 0 and 2 reports 0.5, which is no mode,
and a fleet cell is the weighted mean of such averages. After
`04-fleet.md` FL-5 the fleet folds every other kind by its declaration,
but `Enum` variables still take `Mean` in both stages: over time within
each home and across homes.

**Evidence:** at `ebdb1a17`, `crates/hares-fleet/src/aggregation.rs:46, 122`
(the mean and weighted-mean unit lists, which hold both `"-"` and
`"enum"`), `:63-79, 141-157` (`for_column` and `for_column_by_suffix`),
`:1761-1776` (test `enum_unit_maps_correctly` pins the mean);
`crates/hares-io/src/output/columns.rs:102` (`MODE_SUFFIX = "Mode (-)"`:
the mode column's unit is `-`, so it takes `Mean` through the `"-"` entry,
not the `"enum"` one), `:539` (per-equipment `{name} Mode (-)` columns,
emitted only at verbosity 3 and above, `:532`);
`crates/hares-core/src/dwelling/mod.rs:7230` (the column holds
`OperatingMode::as_code()`, `hares-types/src/equipment.rs:236-238`). The
unit tables and `for_column` are gone by this row: FL-1 carries them as
`unit_keyed_fleet_rules` and FL-5 deletes it, keeping only the `Enum` exception
this entry removes (FL-5's change: "`Enum` stays `Mean` until B-11").

**Change:** this entry changes the fleet fold only. The per-dwelling
reporting-frequency reduction (`03-output-reporting.md` OR-13) is new
output and uses Mode from its first commit.
- Both fleet stages switch `Enum` to Mode: each home's fleet reducer (FL-1)
  takes `Mode` over time, as `VariableDef.aggregation` declares for every
  `Enum`, and the cross-home fold takes the code with the greatest summed
  weight. FL-5's `Enum` exception is deleted.
- Mode applies to multi-valued enum variables only (`VariableKind::Enum`):
  the per-equipment operating mode and the EV charging level. Across
  dwellings, Mode is the code with the greatest summed weight, ties to the
  lowest code. Nulls (B-6) are ignored; a cell with no non-null value is
  null.
- Binary state variables (`VariableKind::State`: EV connection state,
  defrost cycle state, the on/off schedule column) keep the weighted mean,
  which is the duty fraction (`dl-i08-architecture.aggregation`).
- `enum_unit_maps_correctly` is replaced by the tests below, and so is the
  `Enum` case of `04-fleet.md` FL-5's `fleet_fold_follows_declared_kinds`
  (the interim weighted mean), which is deleted. FL-5's
  `fleet_cells_follow_declared_kinds` excludes `Enum` variables and is not
  changed.
- `tracing` at `debug`: each variable's aggregation kind at fold setup.

**Acceptance test** (`crates/hares-fleet`, unit tests over the stated
per-home cells):
- `fleet_mode_uses_summed_weight`: homes with weights 1, 1 and 3 reporting
  modes 0, 0 and 2 in one period fold to 2.
- `fleet_mode_ties_break_to_the_lowest_code`: weights 1 and 1 reporting
  codes 2 and 1 fold to 1.
- `fleet_mode_ignores_null_homes`: a home whose mode cell is null does not
  vote; all homes null gives null.
- `fleet_binary_state_is_weighted_duty_fraction`: connection state 1 and 0
  with weights 1 and 3 folds to 0.25.
- `home_mode_over_time_is_the_most_frequent_code`: one home of the
  `fleet_same_offset_300s` inputs at `Minutes(15)` whose three steps in a
  period report codes 0, 2 and 2 reduces that period to 2 before the fold
  (the reducer is fed the stated codes through a test actor declaring an
  `Enum` variable, as `03-output-reporting.md` OR-13's Mode test does).
- `fleet_enum_cells_follow_mode`: the run of `04-fleet.md` FL-5's
  `fleet_cells_follow_declared_kinds` (the `fleet_same_offset_300s` inputs
  at `Minutes(15)` with `Selection::Pattern("*")`). The test first asserts
  that the selection holds at least one `Enum` (the equipment `enum` modes
  at `21d90588`). For every `Enum` variable and period, the cell equals the
  code with the greatest summed weight over the three homes' period
  values, each the most frequent code of that home's `Timestep` `Memory`
  report in the period (ties to the lowest code, nulls ignored). At the
  parent the cell is the weighted mean, so the test fails there.

**Verification:** `cargo nextest run -p hares-fleet`; `frame_golden
compare-all` with every golden unchanged: the fleet goldens
(`fleet_mixed_offset`, `fleet_same_offset_300s`, `resstock_2025_fleet`)
report the verbosity-2 selection (`04-fleet.md` FL-1), which holds no
`Enum` variable (the mode columns start at verbosity 3), and no dwelling
golden has a fleet fold, so every golden compares bitwise. The tests above
carry the delta. The commit's "Numeric changes" line names the weighted
Mode for fleet `Enum` variables and states that no golden was re-captured.
`docs/PHYSICS_DECISIONS.md` gains the section this file's preamble
requires: the rule (`dl-i08-architecture.aggregation`), that OCHRE has
no fleet fold to compare with, and that a fleet `Enum` cell moves from the
mean of mode codes to the weighted Mode.

**Commit:** `fix: fold fleet operating modes by weighted Mode, not by mean`

**Dependencies:** `03-output-reporting.md` OR-1 (`VariableKind`,
`Aggregation`), OR-13; `04-fleet.md` FL-1 and FL-5 (directly before this
entry); B-6 (nulls).

**Measurement:** the golden record (every golden unchanged, recorded as
measured), the before and after values of the unit tests above, and the
delta of every `Enum` column of `fleet_enum_cells_follow_mode`'s
`Pattern("*")` run, recorded in `02-measurement.md`.

**State:** committed `19332c1f` (branch `row/r88-b11`, worktree
`r88-b11`, based on `1f64ff8b`, one commit). In brief: both fleet
stages fold an `Enum` column by the declared rule, the row 39
reducer's own Mode, never by a mean over codes. The enum's label count
reaches the fold as schema metadata: `variable_field` writes an
`enum_labels` entry beside the unit on an `Enum` variable's field (the
schema builders are the one surface that renders the declared kind
into the report schema; no unit string identifies a mode column, the
mode columns' unit is the dimensionless `-`), and
`hares-fleet`'s `field_fleet_rules` reads it: over time the rule is
`Aggregation::Mode` with `enum_codes` set, the row 45
`Timestamp`-fed reducer counts per code and emits the winner; across
homes the fold's per-column label count switches the cell from the
`v*w, w` sums to per-code summed weights, and the closed cell is the
code with the greatest summed weight. The winner is one shared
function, `mode_winner` (`hares-io` `output::reduce`, generic over
the count type): the reducer's per-period count and the fold's
per-code weighted counts take the same strict maximum, so the tie
breaks to the lowest code in both stages by one rule. A home whose
mode cell is null does not vote (the fold's existing null rule, B-6's
rule where it lands); a cell no home voted on is null; a value that
is no declared code votes nothing, by the reducer's own in-range
rule. Binary state columns keep the weighted mean (the duty
fraction). `tracing` at debug names each column's over-time rule at
the reducer's setup and each column's across-homes rule at the
fold's. The entry's `Pattern("*")` run reconciled to the
verbosity-3 selection: the catalogue's pattern resolution is blind
to the observe gate while the schema builder honours it, so a
pattern selection over the full catalogue panics the roster plan
when the observe feature is off (latent, its own row); verbosity-3
is the tier that holds the per-equipment mode columns either way.
`fleet_binary_state_is_weighted_duty_fraction` and
`fleet_mode_ignores_null_homes` are stated over State-shaped and
null-shaped cells on the landed kinds (the entry's "EV connection
state" example predates the catalogue's declaring it a three-label
`Enum`, which now folds by Mode). RED first (the acceptance tests
run at the parent `1f64ff8b`; the values are in the commit body):
the summed-weight test's cell read 1.2 (the weighted mean of codes
0, 0, 2 at weights 1, 1, 3), the two-home tie test's cell read 1.5
(the mean of codes 2 and 1), the probe's period read
1.3333333333333333 (the mean of codes 0, 2, 2), and the run's `Air
Conditioner Mode (-)` period 39 read 4.5 where the weighted Mode is
0.0, all of them non-codes; the two pins (the duty fraction 0.25,
the ignored nulls) passed at the parent. After: every `Enum`
column's every period cell equals the declared weighted Mode on
bits. The movement: no golden moves (the fleet goldens'
verbosity-2 selection holds no `Enum` column, no dwelling golden
has a fleet fold; compare-all 8 identical, the dst set identical),
recorded in the measurement log with the run's per-column record.
Gates at `19332c1f`: fmt clean, clippy `--workspace --all-targets
--all-features -D warnings` clean, nextest `--workspace
--all-features -j 2` 8115/8115 (7 skipped), `pytest -m ''` 1764
passed + 6 xfailed, 0 failed, 0 errors, pyright 0, the goldens
bitwise, `docs/python.md`'s Numeric changes line carries the
weighted Mode and states that no golden was re-captured.

## B-12: fleet local time labels

**State:** committed (branch `row/r93-b12`, worktree `r93-b12`, based on
`cf3985f3`). The fleet's `Time` column is a naive
`Timestamp(Millisecond, None)` at each period's start, for every
alignment, and the schema metadata records the offset: a single-offset
fleet's labels are the homes' own local wall clock (the undeclared
default's fold rows take the homes' shared site offset, not +00:00, so
`Instant`'s label rendering covers both the declared reference zone and
the undeclared fleet's local clock), a declared `Instant(offset)` fleet
labels in the declared reference zone's wall clock, and `LocalClock`
labels by the local wall clock each folded home's step in the row
starts at (the naive local bucket itself); under `LocalClock` no single
offset applies and the schema records none. The two fold stages' keys,
weights and arithmetic are untouched: the row counts, the period
semantics and every number are the parent's. The `Time` field is
`Timestamp(TimeUnit::Millisecond, None)`, non-nullable, the label
written once per row as an `i64` millisecond (an instant key renders at
its offset's wall clock, the naive local key stamps itself), and
`tracing` at `debug` names the alignment and the offset chosen. FL-4's
three Utf8 label tests (`local_clock_labels_are_shared_naive_local_time`,
`instant_alignment_buckets_on_reference_boundaries`,
`instant_utc_equals_todays_output`) are rewritten to read the
`Timestamp` column, the last from "the declared `Instant(+00:00)`
reproduces the undeclared fleet's output bitwise" to what survives B-12:
the number columns bitwise, the labels the same instants shifted by the
homes' offset, the metadata naming each run's offset; FL-4's
`cross_offset_homes_align_on_the_start_instant` and
`instant_alignment_of_one_start_instant_covers_every_period` pins read
the same instants as naive labels, and the wrap pin's label reads move
to the timestamp column. The regression suite's `aggregation_check`
reads the fleet label back with the schema's `hares:utc_offset_s`
(UTC = Time - offset) instead of parsing an RFC 3339 string. The
three acceptance tests above are written first and failed at the parent
`cf3985f3` with the rest of the label-reading tests (nine failures:
`left: Utf8, right: Timestamp(Millisecond, None)` on the type asserts,
the helper's downcast panic on the label reads); green on the commit.
The Python surface passes the batch through untouched (polars reads
`timestamp[ms]`, the local test's label a naive `datetime`). Python
`test_local_clock_alignment_runs_the_mixed_offset_fleet` asserts the
naive `datetime`.

**Movement** (in `02-measurement.md`'s log): the three fleet goldens'
`Time` labels only, in the `aggregate` and `covered_weight` products:
`fleet_same_offset_300s` and `resstock_2025_fleet` (undeclared,
UTC-8): the UTC `Z` strings to the homes' local wall clock (192 and 48
cells per product, the -8 h shift); `fleet_mixed_offset` (declared
`Instant(+00:00)`): the same instants, the RFC 3339 `Z` string to the
naive timestamp, 165 rows; every other column, the row counts and the
`homes` and `fleet_report` products bitwise. The schema metadata gains
`hares:utc_offset_s` (-28800, -28800, 0). The other goldens identical:
compare-all 8 + the dst set's `full_year_900s_dst`; the three fleet
goldens re-captured twice byte-identical. `docs/PHYSICS_DECISIONS.md`
gains the section (the rule, that OCHRE has no fleet aggregation, and
the labels' move from UTC to local site time), and `docs/python.md`'s
"Numeric changes" list names the naive local fleet labels and the
re-captured fleet goldens.

Gates: fmt clean; clippy `--workspace --all-targets --all-features
-D warnings` clean; nextest `--workspace --all-features -j 2` 8138/8138
(8 skipped; the first full run's one failure was the regression
`aggregation_check`'s RFC 3339 read of the fleet label, rewritten to
the timestamp-plus-metadata read and the suite re-run whole);
`pytest -m ''` 1766 passed + 6 xfailed, 0 failed, 0 errors; pyright 0
(`uv run pyright`); the goldens as above.

**Kind:** numeric fix (`numeric_fixes`: fleet local time labels;
`dl-i08-architecture.fleet`). `04-fleet.md` FL-4 adds the declared
alignment and keeps the fleet `Time` column a Utf8 RFC 3339 string
(bug-compatible); this entry moves every alignment to a naive local
`Timestamp` and owns the local-time label tests.

**Problem:** after FL-4 the fleet `Time` column is a Utf8 string in three
renderings: an undeclared single-offset fleet and `Instant(+00:00)` label
each row's start instant in UTC with a `Z` suffix
(`2018-01-01T05:00:00Z` for a UTC-5 home's local midnight), whatever the
homes' local time; `Instant(offset)` renders the start instant in the
declared offset (`2018-01-01T00:00:00-07:00`); `LocalClock` renders the
naive local period start as text with no offset (`2018-01-01T00:00:00`).
Every per-dwelling output is a naive local `Timestamp` (`03-output-reporting.md`
OR-7), so fleet labels are the one output that is neither naive nor a
timestamp.

**Evidence:** `crates/hares-fleet/src/aggregation.rs:505` (each home's
`Time` string parsed back with `parse_timestamp`, `:444`), `:508` (the
bucket keyed by its epoch second) and `:636-637` (UTC RFC 3339 with `Z`)
at `ebdb1a17`; the Utf8 `Time` field (`aggregation.rs:715` at `21d90588`)
that FL-1 and FL-4 keep; FL-4's three renderings above.

**Change:** fleet rows are labelled in local site time as a naive
`Timestamp(Millisecond, None)` at interval start (`plan.md` section 2.5),
for every alignment. Single-offset fleets use the homes' local time.
Multi-offset fleets use FL-4's declared alignment: `LocalClock` labels by
the shared local wall clock; `Instant` labels in the declared reference
zone. The schema metadata records the offset or the reference zone. FL-4's
Utf8 label tests (`local_clock_labels_are_shared_naive_local_time`,
`instant_alignment_buckets_on_reference_boundaries`,
`instant_utc_equals_todays_output`) are rewritten to read the `Timestamp`
column. `tracing` at `debug`: the alignment and offset chosen.

**Acceptance test:**
- `single_offset_fleet_is_labelled_in_local_time`: a UTC-5 fleet of
  `tests/fixtures/resstock/2025.1/` buildings `bldg0000002` (weather
  `G0900090_2018.csv`), `bldg0000017` (`G0900010_2018.csv`) and
  `bldg0000003` (`G2500170_2018.csv`), weights 1.0, from
  2018-01-01T00:00:00-05:00 for 2 days at 3600 s, hourly: the `Time`
  column is `Timestamp(Millisecond, None)`, the first row is labelled
  2018-01-01 00:00 (naive), not FL-4's `2018-01-01T05:00:00Z`, and the
  schema metadata records the offset -05:00.
- `instant_fleet_is_labelled_in_the_reference_zone`: the
  `fleet_mixed_offset` inputs (UTC-5, UTC-6 and UTC-8 homes; the manifest's
  `start_time` `2018-01-01T00:00:00-06:00` is read by its naive local part
  in each home's own offset, `crates/hares-core/src/dwelling/blueprint.rs:95-103`,
  so each home starts at its local 2018-01-01 00:00) declared `Instant` with
  reference zone UTC-6 at `Hourly`: in reference time the UTC-5 home starts
  at 2017-12-31 23:00, the UTC-6 home at 2018-01-01 00:00 and the UTC-8
  home at 02:00, and the rows are the periods every folded home has a row
  for (`04-fleet.md` FL-1, FL-4), so the first row is labelled 2018-01-01
  02:00 (naive), where FL-4 wrote `2018-01-01T02:00:00-06:00`, and the
  metadata names the reference zone.
- `local_clock_fleet_is_labelled_by_local_wall_clock`: the inputs of
  `04-fleet.md` FL-4's mixed-offset fixture declared `LocalClock`: the
  `Time` column is a `Timestamp`, not FL-4's text `2018-01-01T00:00:00`, and
  every row's label equals the local wall-clock time at which each home's
  step in that row starts.

**Verification:** `cargo nextest run -p hares-fleet`; the fleet goldens
re-captured; the commit's "Numeric changes" line names the naive local
fleet labels and the re-captured fleet goldens. `docs/PHYSICS_DECISIONS.md`
gains the section this file's preamble requires: the rule
(`dl-i08-architecture.time` and `.fleet`), that OCHRE has no fleet
aggregation, and that fleet labels move from UTC to local site time.

**Commit:** `fix: label fleet output in local site time`

**Dependencies:** `04-fleet.md` FL-4 (bug-compatible), `03-output-reporting.md`
OR-7.

**Measurement:** label delta, from FL-4's Utf8 strings to the naive
timestamps, on the `fleet_mixed_offset` golden (M-2, `Z` labels under its
declared `Instant(+00:00)`), on `fleet_same_offset_300s` (FL-1) and on
`resstock_2025_fleet` (B-9a).

## B-13: opaque-R precedence

**Kind:** numeric fix (`decision-log.md` `dl-i08-architecture.precedence`).

**Problem:** for an opaque boundary that matches an envelope LUT row, the
row's precomputed layers are used as they are, and the declared
`AssemblyEffectiveRValue` only selects the row (when the HPXML gives no
insulation description, by the nearest row R). The declared resistance is
therefore a staircase, not the boundary's resistance.

**Evidence:** `crates/hares-core/src/dwelling/conversions.rs:147-166`
(`material_r`: declared assembly R minus the interior and exterior film
resistances, clamped at `:149`, else the layer sum, else 2.5 m2K/W),
`:240-291` (non-slab boundaries without explicit material layers:
`lut.lookup(..., r_value)` returns the row's layers, used unchanged as
`precomputed_rc`); `crates/hares-io/src/envelope_lut.rs:550-590` (row
selection: by insulation details when given, else by the row whose
assembly R plus films is nearest the declared R; a declared R at or above
17.6 m2K/W (R-100 IP) is replaced by 88 m2K/W with the type filters
cleared, which selects the `Minimal` row, `:571-576`, as OCHRE does; this
entry keeps that selection and `k` then scales the `Minimal` row's layers
to the declared R);
`crates/hares-envelope/src/boundary_rc.rs:926-935` (the
lumped `fallback_r` path, used only when there are no precomputed layers);
consumer evidence that the declared R is inert between rows:
`calibration/defaults.py:1-6`, `calibration/coverage.py:133-136, 225-228`.

**Change:** when a boundary has a declared `AssemblyEffectiveRValue` and a
matched LUT row, every layer resistance of the row is multiplied by `k =
material_r / sum(row layer resistances)`, so the layers plus the films equal
the declared assembly R; the row's layer capacitances are kept (they are
mass, not resistance). Row selection is unchanged. Slab boundaries (F-factor
method, `conversions.rs:176-236`), boundaries with explicit material layers
(`:237-240`) and boundaries without a declared R are unchanged.
- A declared `AssemblyEffectiveRValue` at or below the sum of the
  boundary's interior and exterior film resistances leaves no material
  resistance. Today `material_r` clamps it to `1e-6` m2K/W
  (`(r - r_film_int - r_film_ext).max(1e-6)`, `conversions.rs:147-150`), so
  this entry would scale the LUT layers to almost no resistance without a
  word. It is a construction error naming the boundary, its declared R and
  its film sum; the clamp goes from the declared-R path.
- Order with `12-inputs.md` IN-7 (row 66a, earlier): IN-7's
  `opaque_r_scale` multiplies the layer resistances after row selection;
  this entry's normalisation by `k` runs before it, so a layer's resolved
  resistance is the row's times `k` times `opaque_r_scale`, and its
  capacitance the row's times `thermal_mass_scale`. IN-7's text already
  says "after B-13's scaling"; from this commit the code path is row
  selection, then `k`, then the scales.
- `docs/PHYSICS_DECISIONS.md` records the rule, the film-sum error and that
  OCHRE uses the LUT row as is. `tracing` at `debug`: per boundary, the
  row, the declared R and `k`.

**Acceptance test** (on `tests/fixtures/hpxml/ochre_samples/base.xml`, whose
`Wall1` declares `AssemblyEffectiveRValue` 23.0 (IP) with no material
layers and is matched to a LUT row; the expected values are computed in
the test from the fixture, the selected row and the films):
- `assembly_r_distributes_across_lut_layers_uniformly`: every resolved
  layer resistance of `Wall1` is the row's times `k = material_r / sum(row
  layer resistances)`, the layers plus the films equal the declared R in
  SI, and every capacitance equals the row's on bits.
- `declared_r_between_rows_moves_the_resolved_resistance`: two declared R
  values that select the same row as 23.0 (the test edits the fixture text
  to 22.5 and 23.5 after checking the row is unchanged) give different
  resolved resistances and equal capacitances.
- `declared_r_at_or_below_the_films_is_an_error`: the test computes
  `Wall1`'s film sum in SI, converts it to IP, rounds it down to two
  decimals, checks that the SI conversion of that IP value is at or below
  the film sum (an IP value equal to the film sum can convert a few ULP
  above it), and writes it as `Wall1`'s declared R; construction fails
  naming `Wall1`, the declared R and the film sum.
- `assembly_r_and_opaque_r_scale_compose`: `Wall1` with
  `envelope = { opaque_r_scale = 1.25, thermal_mass_scale = 1.0 }`: every
  layer resistance equals the row's times `k` times 1.25 and every
  capacitance equals the row's.
- `slab_and_material_layer_boundaries_are_unchanged`: `base.xml`'s slab
  resolves bitwise as at the parent commit. `base.xml` has no
  material-layer boundary (its layers carry only `NominalRValue`, which
  `crates/hares-io/src/hpxml/building.rs:2239-2244` skips), so the second
  half is a unit test over the boundary conversion with a boundary the test
  builds: a wall with a declared `AssemblyEffectiveRValue` of 23.0 (IP) and
  two explicit material layers (thickness, conductivity, density and
  specific heat stated in the test); its resolved layers equal the
  parent commit's on bits.

**Verification:** `cargo nextest run -p hares-core`; goldens re-captured
(every golden with a declared opaque assembly R changes); the commit's
"Numeric changes" line names the opaque-R precedence and the re-captured
goldens.

**Commit:** `fix: a declared AssemblyEffectiveRValue sets the resistance of LUT layers`

**Dependencies:** `12-inputs.md` IN-7 (the scales this entry composes
with); `08-consumer.md` CN-3 depends on this.

**Measurement:** per golden, annual heating and cooling energy delta and the
distribution of `k` over its boundaries.

**State:** committed (2026-10-16, branch `row/r94-b13`, based on
`cf3985f3`, one commit; the schedule row is 94). The delta and the k
distribution are in `02-measurement.md` (the opaque-R precedence row).

Landed: the k normalisation in `building_to_boundary_inputs`'s LUT path
(`crates/hares-core/src/dwelling/conversions.rs`, restructured from the
`and_then`/`unwrap_or_default` chain into one match so the error below
propagates): row selection unchanged, then every matched row's layer
resistance multiplied by `k = (declared R - convention films) / sum(row
layer resistances)`, the row's capacitances kept, and a `tracing::debug!`
per boundary naming the row, the declared R and k; the envelope scales
apply after it, so a layer's resolved resistance is the row's times k
times `opaque_r_scale` and its capacitance the row's times
`thermal_mass_scale`. The LUT's convention films are now exposed as
`EnvelopeLookup::convention_films(boundary_name)`
(`crates/hares-io/src/envelope_lut.rs`), and the row selection's own
nearest-R match reads them from the same method. The declared-R arm of
`material_r` lost its `.max(1e-6)` clamp (the fallback's own clamp, one
line below, still guards the single-resistor path). The read-back names
the winning source: `RValueSource`
(`crates/hares-core/src/resolved.rs`: `DeclaredAssemblyR`, `LutRowR`,
`MaterialLayers`, `FallbackR`, `SlabFfactor`, with a `label()` for the
Python surface) on `ResolvedBoundary.r_source`, filled where the
read-back derives the layers; `Dwelling.resolved_model()` reports each
boundary's `r_source` label. `docs/PHYSICS_DECISIONS.md` records the
rule, the film-sum error, the OCHRE deviation and the film convention.

One premise correction, with the evidence that forced it: the entry's
film-sum error and its k equation read "the boundary's interior and
exterior film resistances" - the conversion's weather-computed,
convection-only films (the radiative part rides the LWR network). Those
sums exceed legitimate declared assembly R values: on this entry's own
acceptance fixture `base.xml`'s Roof1 declares 2.3 IP (0.405 m²·K/W)
against a convection-only film sum of 0.489 m²·K/W, and the corpus
carries about sixty such roofs (2.19 to 2.30 IP) and floors (2.10 IP
against 2.11 m²·K/W of films), so the entry's literal error would reject
the OCHRE and ResStock sample corpus, the entry's own acceptance
fixture, and its own acceptance tests with it. The films in the k
equation and in the film-sum error are the row-selection convention's
films (`convention_films`, 0.1585 walls and roofs, 0.2642 floor and
ceiling) - the convention the declared R speaks (HPXML's assembly R
includes the standard films) and the same constants the row selection
adds to the row's assembly R. The film-sum error's threshold inherits
the convention, and nothing in the corpus is near it (the minimum
declared R is 0.3698 m²·K/W against 0.2642 m²·K/W). The k distribution
and the deltas are in the measurement log; the k are positive and finite
everywhere, in [0.16, 1.10] over the corpus's homes.

RED first, witnessed at the parent `cf3985f3` with only the tests
applied (`crates/hares-core/tests/envelope_assembly_r.rs`, new):
`assembly_r_distributes_across_lut_layers_uniformly` failed on layer 0's
resistance (the row's raw 0.248 m²·K/W resolved where the declared R
demands the row's times k = 1.3880, the differential the entry names:
the declared 23.0 IP against the row's resolved 3.20 m²·K/W plus films);
`declared_r_between_rows_moves_the_resolved_resistance` failed on 22.5
IP's layer 0 resistance equalling the 23.0 IP one on bits (the
staircase); `declared_r_at_or_below_the_films_is_an_error` failed on
construction succeeding (the clamp, silently);
`assembly_r_and_opaque_r_scale_compose` failed the same distribution
assertion. The read-back's `r_source` was the missing surface (compile
errors at the parent when the assertions first named it). GREEN: the
five acceptance tests pass, the distribution's k on bits, the
capacitances the row's on bits, the layers plus the convention films
equal to the declared R within 1e-12 relative, the between-rows
resistances distinct with equal capacitances, the film-sum error naming
Wall1, the declared R and the film sum, the composition exact on bits,
and the slab's resolved read-back bitwise the parent's.

Movement: every golden moved (all nine carry fixtures with declared
opaque assembly R values); re-captured twice byte-identical, the deltas
in `02-measurement.md`. Two committed value pins moved and were
re-captured with the cause: `diagnostics_cz4a.csv` (the diagnostics
columns' pin; the preamble's provenance lines identical, the zone
temperatures and the opaque LWR column moved) and the structural
envelope oracle's per-boundary UA comparison, whose declared-R groups
now pin the declaration's expectation instead of the OCHRE row's (the
deviation the doc records).

## B-14: HARES stores and computes in SI units

**Kind:** defect fix, numeric (`dl-i08-architecture.si_units`, a found
defect: SI internally). The decision covers everything HARES stores or
computes internally: equipment and actor configuration, control signals
and power setpoints, actor telemetry, billing and metrics running state,
per-step results and gas rates; output columns keep their declared
reporting units, converted once at the reporting boundary. The entry has
four commits. The first changes no engine code and captures the goldens
the entry needs; each of the other three is numeric, with its own row in
the numeric block, its goldens re-captured and its delta recorded
(`00-schedule.md` rows 94c, 94g and 94h):
- commit 2 (row 94c, after commit 1): the equipment config fields;
- commit 3 (row 94g): the run state, meaning equipment state, ports and
  the electrical summary, per-step results, billing and metrics running
  totals, gas rates and the PV-sizing calculation;
- commit 4 (row 94h): the control layer, meaning actor configuration,
  actor state and telemetry, and control signals and power setpoints.

Row 94c starts only after a verification round over this entry records
zero blockers and zero majors in `plan-critique.md`
(`dl-i08-process.readiness`).

**Problem:** equipment configs store power in kW, energy in kWh, apparent
power in kVA and EV fuel economy in kWh/mi, so every equipment converts at
its own call sites with hand-written factors, a caller must know which
fields are not SI, and the read-back (`06-runtime-memory.md` RT-6) has to
convert them to report SI. The run state is in the same units: the
tariff's billing state and the metrics calculator accumulate kWh, Wh and
therms, the electrical summary, the ports and `StepResult` carry kW and
kVAR, and the scheduled load's gas rates are therms per hour. So is the
control layer: actor parameters are kW, kWh, miles and mph, control
signals carry kW, kVAR and kWh, and the BMS and EV-driver actors compute
and publish kW.

**Evidence** (at `21d90588` for commit 2's table, at `afa9a071` for the
run state and the control layer; paths under `crates/hares-equipment/src/`
unless stated).

*Commit 2, equipment config fields.* The census is every field of every
equipment config type, nested payload types included, and every raw
parameter key an untyped equipment reads, whose name or unit is kW, kWh,
kVA or a ratio of them:

| Config type (serde) | Field (unit) | Becomes |
|---|---|---|
| `PvConfig` (`pv/config.rs:36, 50`) | `capacity_kw`, `inverter_capacity_kw` (kW) | `capacity_w`, `inverter_capacity_w` |
| `PvArray`, `PvArraySpec` (`pv/array_config.rs:116, 190`), the elements of `PvConfig.arrays` (`pv/config.rs:77`) | `capacity_kw` (kW) | `capacity_w` |
| `BatteryConfig` (`battery/config.rs:20-22, 79`) | `capacity_kwh` (kWh), `max_charge_kw`, `max_discharge_kw` (kW), `inverter_capacity_kva` (kVA) | `capacity_j`, `max_charge_w`, `max_discharge_w`, `inverter_capacity_va` |
| `BmsMode::SelfConsumption` (`crates/hares-types/src/equipment.rs:879`), the JSON payload of `BatteryConfig.bms_mode`; Python `BmsMode` (`crates/hares-python/src/py_enums.rs:2585`) | `surplus_deadband_kw` (kW) | `surplus_deadband_w` |
| `EvConfig` (`ev/config.rs:381, 385, 443, 449, 453, 461, 472`) | `capacity_kwh` (kWh), `max_charging_power_kw`, `v2l_max_discharge_kw`, `v2g_max_discharge_kw`, `power_limit_kw` (kW), `fuel_economy_kwh_per_mi` (kWh/mi), `charger_capacity_kva` (kVA) | `capacity_j`, `max_charging_power_w`, `v2l_max_discharge_w`, `v2g_max_discharge_w`, `power_limit_w`, `fuel_economy_j_per_m`, `charger_capacity_va` |
| `ChargingStrategy::SolarSurplus`, `ChargingStrategy::V2G` (`crates/hares-types/src/equipment.rs:615, 625`), the JSON payload of `EvConfig.charging_strategy`; Python `ChargingStrategy.v2g`, `.solar_surplus` (`py_enums.rs:1699, 1721`) | `min_charge_rate_kw`, `max_export_kw` (kW) | `min_charge_rate_w`, `max_export_w` |
| `GeneratorConfig` (`generator.rs:52, 79-82`) | `rated_power_kw`, `capacity_min_kw`, `grid_import_limit_kw`, `export_limit_kw` (kW), `delta_kw_per_s` (kW/s) | `rated_power_w`, `capacity_min_w`, `grid_import_limit_w`, `export_limit_w`, `delta_w_per_s` |
| `CentralAirConditionerConfig`, `RoomAcConfig` (`hvac/cooling_config.rs:58, 338`), `HeatPumpCoolerConfig` (`hvac/heat_pump_config.rs:493`) | `crankcase_heater_kw` (kW) | `crankcase_heater_w` |
| `EquipmentConfig` (`config.rs:287`; `#[serde(skip)]`, injected by the dwelling at `crates/hares-core/src/dwelling/mod.rs:2591`, read by `hvac/hvac_core.rs:415` and `hvac/equivalent_battery.rs:89-138`) | `zone_capacitance_kwh_per_k` (kWh/K) | `zone_capacitance_j_per_k` (J/K; no `REMOVED_KEYS` row: no input surface sets it) |
| event-based load raw keys (`event_load.rs:32, 45, 136-141`) and the synthetic TOML `SyntheticEventLoadConfig` (`crates/hares-core/src/dwelling/synthetic.rs:66-70`) | `active_power_kw`, `phase_<n>_power_kw`, `cycle_phase_<n>_power_kw` (kW), `event_power_kw_series` (a kW series, written by `crates/hares-io/src/schedule_resolve.rs:1235, 1264`) | `active_power_w`, `phase_<n>_power_w`, `cycle_phase_<n>_power_w`, `event_power_w_series` |
| scheduled load raw keys (`scheduled_load.rs:32, 36`, written by `schedule_resolve.rs:925, 1281, 1315, 1352, 1758`) | `power_profile_max_kw`, `power_constant_kw` (kW) | `power_profile_max_w`, `power_constant_w` |

Raw-key constants that are deleted, not migrated: the battery's
(`battery/mod.rs:44-48`) and the generator's (`generator.rs:340-360`) are
`#[cfg(test)]`, read by no production path; the EV's (`ev/config.rs:9-47`,
including the HPXML-named `BatteryCapacity` (kWh) and `MaxChargingPower`
(kW) and `range_miles`) feed a raw-payload constructor whose result `init()`
always refuses (`ev/mod.rs:300-306`), so they build only a placeholder.

Input boundaries that write these fields: the HPXML resolvers
(`crates/hares-io/src/hpxml/resolve_der.rs`, which reads PV and battery
values in W and divides by 1000 at `:36-37, 63-64, 114-115`, and the EV's
`MaxChargingPower` in kW at `:197`; `equipment.rs`, `resolve_hvac.rs`;
`resolve_loads.rs:867-918`, which builds `EvConfig` from a `PlugLoad`
of type "electric vehicle charging"); `schedule_resolve.rs` (above);
`crates/hares-io/src/defaults.rs`; the catalogue fills `BatterySpec::to_config`
and `VehicleSpec::to_config` (`battery/catalog.rs:162`, `ev/catalog.rs:126`);
the synthetic TOML path, which writes `active_power_kw` as an HPXML
extension element (`synthetic.rs:936-941`) that the resolver passes through
(`crates/hares-io/src/hpxml/xml_helpers.rs:357`);
`crates/hares-core/src/dwelling/mod.rs`; the BMS actor's fill of
`BmsMode::SelfConsumption` from its own parameter
(`crates/hares-core/src/actor_registry.rs:486`); the Python constructors
(`crates/hares-python/src/py_equipment.rs`: `Battery` `:192-220`, `PV`
`:613, 627`, `EV` `:660-712`, and the catalogue conversions
`PyBattery::from_spec` `:253-260` and `PyEv::from_spec` `:763-841`), the
Python dwelling builders (`py_dwelling.rs:465, 520-528, 539-570, 1267-1341`,
including the EV defaults of 75 kWh and 7.2 or 11.5 kW at `:539-542`) and
`python/` (the `_hares.pyi` stubs of `Battery`, `PV`, `EV`, `BmsMode` and
`ChargingStrategy`, under `python/ochre_next` at `21d90588` and
`python/hares` after `05-python-surface.md` PY-7); `docs/equipment/*.md`;
the tests that construct these configs. The consumer writes
`surplus_deadband_kw` in the `bms_mode` payloads it injects
(`HARES_CALIBRATION_SYSTEM/calibration/der_inject.py:157-180`, `08-consumer.md`
CN-3).

Also written by commit 2's boundaries, at `afa9a071`:
- the plug-load EV the HPXML resolver builds (`resolve_loads.rs:868-902`:
  `range_miles` 100 or 250 by annual kWh, `EV_FUEL_ECONOMY` in miles per
  kWh at `:23-24`, `battery_capacity_kwh = range_miles / EV_FUEL_ECONOMY`,
  `max_charging_power_kw` 7.2 or 11.5, and `fuel_economy_kwh_per_mi:
  None` at `:902`, so this EV takes the default fuel-economy rule);
- `VehicleMappingEntry { capacity_kwh, charger_power_kw }`
  (`crates/hares-io/src/defaults.rs:240-253`, parsed at `:1705-1745` from
  `defaults/ev/vehicle_mapping.csv`), which fills `EvConfig`;
- the Python AC builder's `crankcase_heater_kw: None`
  (`crates/hares-python/src/py_hvac.rs:306`);
- the HPXML energy readers that return kWh (`xml_helpers.rs:96-103`
  `child_energy_kwh`, read for the battery `NominalCapacity` and the EV
  `BatteryCapacity` at `resolve_der.rs:123, 188`).

*Commit 3, run state* (at `afa9a071`):
- the tariff's `BillingState` (`crates/hares-tariff/src/billing.rs:75-99`):
  `cumulative_import_kwh`, `cumulative_export_kwh`, `peak_demand_kw`,
  `period_peak_demand_kw`, `prior_peaks_kw`, `prior_period_peaks` and the
  demand window's samples and running sum (kW), and `cumulative_ev_kwh`;
- the metrics calculator's running totals and peaks
  (`crates/hares-io/src/output/metrics.rs:391-421`, from
  `total_electric_energy_kwh` to `battery_energy_out_kwh`, in kWh, Wh,
  therms and kW);
- `StepResult.net_electric_power_kw` (`crates/hares-core/src/dwelling/mod.rs:1021`,
  filled from `ElectricalSolver::net_active_kw`,
  `crates/hares-envelope/src/electrical_solver.rs:109`, at `mod.rs:7010`),
  the Python step dict key (`crates/hares-python/src/py_dwelling.rs:864`)
  and the `SteppableFleet` step dict keys (`py_fleet.rs:467-472`);
- the electrical summary (`crates/hares-types/src/environment.rs:47-69`:
  `pv_generation_kw`, `base_load_kw`, `net_grid_kw`, `battery_power_kw`,
  `ev_power_kw`, `actual_pv_kw`), which the dwelling checkpoints
  (`crates/hares-core/src/checkpoint.rs:107`) and the actors read; the
  equipment output flows (`crates/hares-types/src/equipment.rs:1172-1209`:
  `CoreFlows.electric_kw`, `reactive_power_kvar`,
  `CorePerformance.main_power_kw`) and the port `reactive_power_kvar`
  (`crates/hares-types/src/ports.rs:309`);
- `DwellingTelemetry` (`crates/hares-core/src/telemetry.rs:20-34`:
  `equipment_power_kw`, `total_power_kw`, `reactive_power_kvar`,
  `island_unserved_kw`, `island_excess_kw`) and the observation names
  `to_observation_vec` resolves (`:92`), the dwelling's
  `island_unserved_kw` and `island_excess_kw` (`mod.rs:6760-6761`) and
  their Python dict (`py_dwelling.rs:1982-1983`), the Python ports dict
  (`py_dwelling.rs:2704-2706`), and the gym's observation keys and reward
  context (`python/ochre_next/rl/gym_env.py:92` `RewardContext.total_power_kw`,
  `:221, 280-281` the `total_power_kw` and `total_electric_kw` observation
  keys, `:309` the equipment `power_kw` observation);
- equipment run state and checkpoint blobs held in kW or kWh
  (`BatteryCheckpoint`, `battery/mod.rs:261`; `EvCheckpoint`,
  `ev/checkpoint.rs:5`; the equipment `*State` structs; the
  `EquivalentBatteryModel` state, `hvac/equivalent_battery.rs:30-52`), and
  the kW working values every equipment computes with after commit 2
  converts its config at construction;
- the scheduled load's gas rates in therms per hour (raw keys
  `gas_profile_max` and `gas_constant`, `scheduled_load.rs:39, 43`, and the
  `GasScheduleUnit` it reads its schedule column with), whose result
  already leaves the equipment in W (`CoreFlows.fuel_w`);
- the premise ZIP aggregate's equipment weights
  (`crates/hares-core/src/dwelling/premise_zip.rs:57-124`,
  `Equipment::expected_mean_power_kw`, `schedule_column_mean_kw`) and their
  Python dict key `mean_power_kw` (`py_dwelling.rs:366-370`);
- the PV-sizing calculation (`crates/hares-physics/src/pv_sizing.rs` and
  its binding `crates/hares-python/src/py_pv_sizing.rs`: `capacity_kw`,
  `max_capacity_kw`, `max_roof_capacity_kw`, `max_ac_kw`, and the
  `target_kw`, `min_kw` and `max_kw` arguments);
- `CHECKPOINT_VERSION` (`checkpoint.rs:22`), 8 at `afa9a071` and 9 after
  `03-output-reporting.md` OR-10;
- the electrical solver (`crates/hares-envelope/src/electrical_solver.rs`:
  `net_active_kw` and `net_reactive_kvar` at `:86-87`, their getters at
  `:109, 118`, computed at `:199-206` through `power_w_to_kw`, and its
  step payload `[net_active_kw, net_reactive_kvar]` at `:213-215`, which
  no reader takes and no checkpoint holds; the checkpoint holds the
  electrical summary, `checkpoint.rs:104-107`) and its readers in
  `dwelling/mod.rs` (`:4704-4733, 6646, 7049`);
- `ElectricalAccumulator` (`crates/hares-types/src/ports.rs:308-311`),
  which holds `reactive_power_kvar` beside `load_power_w` and
  `generation_power_w`;
- the ZIP load model (`crates/hares-types/src/zip.rs`: `ZipLoad::apply(p_kw,
  voltage_pu) -> (real_kw, reactive_kvar)` at `:255-282`,
  `ZipLoad::reactive_kvar` at `:293-299`) and its callers (e.g.
  `hvac/reactive.rs`);
- `hares_physics::pump::compute_ground_loop_pump_power_kw`
  (`crates/hares-physics/src/pump.rs:71`) and its callers
  (`hvac/heat_pump/cooler.rs:651, 1005`, `hvac/heat_pump/heater.rs:2178`);
- the battery's charging-curve rate `charging_lut_c_rate(power_kw,
  soh_adjusted_kwh)` (`pack_electrical.rs:130-132`) and the mode-flow guard
  over `CoreFlows.electric_kw` (`crates/hares-types/src/mode_flow_guard.rs:67-156`);
- the invariant checker (`crates/hares-core/src/invariants.rs`: the
  watt-hour totals `total_heating_wh`, `total_cooling_wh` at `:22-24`, the
  `check_ev_capacity_degraded(battery_capacity_kwh,
  battery_capacity_kwh_rated, ...)` arguments at `:320-356`, and the kW and
  kVAR quantities of `check_electrical` and `check_reactive` at `:101-144`);
- the observer and diagnostics records (`crates/hares-core/src/observer.rs`:
  `EnvironmentCapture` `:84-90`, `EquipmentContribution` `:134-136`,
  `PortsCapture` `:156-158`, `SolverCapture` `:189-194`;
  `observer_capture.rs:46-49, 205-226`; `diagnostics.rs`
  `StepDiagnostics.electrical_net_kw`, `electrical_net_kvar` at `:42-43`,
  `EquipmentDiag.electric_kw`, `reactive_kvar` at `:84-85`, its CSV header
  at `:101-102` and values at `:341-356`). Two of these are wrong today:
  `observer_capture.rs:147-151, 194-196` store the W values
  `load_power_w` and `generation_power_w` in the fields named
  `electrical_load_kw` and `electrical_gen_kw`;
- the schedule resolver's power and energy (`crates/hares-io/src/schedule_resolve.rs`:
  `determine_max_kw` at `:1226-1256` and `determine_constant_kw` at
  `:1258-1269`, with their raw `/ 1000.0` at `:1232, 1262, 1333-1338`) and
  the HPXML annual-energy raw keys they read, `annual_electric_kwh` and
  `annual_gas_therms` (written by `resolve_loads.rs` at `:231-237, 296,
  409-421, 465, 501-519, 559, 571, 816, 833, 934, 962`, by
  `resolve_pool.rs` at `:114-351`, and by `default_annual_kwh_for_equipment`,
  `schedule_resolve.rs:1451-1463`, through `xml_helpers.rs:116-149`
  `child_load_kwh` and `child_load_therms`; read only by the two functions
  above);
- `DwellingTelemetry::verify_consistency` (`telemetry.rs:68-81`), the
  Python observation list `PyDwelling::observation()` (`py_dwelling.rs:2102-2108`),
  the Python telemetry getters (`crates/hares-python/src/py_telemetry.rs:59,
  95-115`), `CoreOutput.electric_kw` and `reactive_power_kvar`
  (`py_equipment.rs:1044-1060`), the observe-build snapshot dict keys
  `electrical_load_kw`, `electrical_gen_kw`, `electrical_reactive_kvar`
  (`py_dwelling.rs:2704-2706`) and `steps_to_polars_df`
  (`crates/hares-python/src/conversions.rs:33-44`);
- the tariff evaluator's input (`crates/hares-tariff/src/evaluator.rs:474-538`:
  `step(net_power_kw, ev_power_kw, dt_seconds, current_time)`, computing
  `ev_import_kwh` and `import_kwh`), called by the dwelling with
  `net_active_kw` (`dwelling/mod.rs:7046-7051`), `BillingState::update`
  (`billing.rs:189-200`), and the Python `TariffEvaluator.step(net_power_kw,
  dt_seconds, current_time)` (`py_tariff.rs:249-261`) and `PyTariffTelemetry`
  (`py_telemetry.rs:212-251`, filled at `py_dwelling.rs:1899-1914` from the
  live `BillingState`: `cumulative_import_kwh`, `cumulative_export_kwh`,
  `peak_demand_kw`);
- the PV-sizing method on the dwelling, `Dwelling.estimate_pv_capacity`
  (`py_dwelling.rs:1792-1840`: `target_kw`, `min_kw`, `max_kw`,
  `inverter_kw_ac`);
- every reader of run state that publishes it: the HELICS federates
  (`python/ochre_next/helics/dwelling.py:384-440, 787-846`: publications
  `total_power_kw`, `reactive_power_kvar`, `equipment_{i}/power_kw` and
  `pv_generation_kw`, registered with `units=kW` and `units=kvar`;
  `helics/fleet.py:193-217, 261-266, 631-687`: `aggregate_power_kw`,
  `aggregate_reactive_kvar` and the per-dwelling and per-equipment
  publications), the gym (`gym_env.py:221-236`, the observation bounds
  `(-100, 100)` for total and reactive power and `(0, 100)` for equipment
  power, sized for kW; `:283-284` the `reactive_power_kvar` observation;
  `:598` and `rl/vec_env.py:277` the reward context), and the Rust gym
  batch step (`crates/hares-python/src/py_gym.rs:494-497`: reward
  `-step.net_electric_power_kw` and the info key `net_electric_power_kw`).

*Commit 4, control layer* (at `afa9a071`):
- actor configuration parameters (`crates/hares-core/src/actor_registry.rs`:
  `:383` `daily_drive_miles_mean`, `:393` `fuel_economy_kwh_per_mi`,
  `:394` `capacity_kwh`, `:395` `average_speed_mph`, `:396`
  `max_charge_kw`, `:411` `range_anxiety_miles`, `:413`
  `away_charge_power_kw`, `:486` `surplus_deadband_kw`, `:540-541`
  `max_charge_kw` and `max_discharge_kw`), set through
  `Dwelling.add_actor_by_name(actor_type, name, params)`
  (`py_dwelling.rs:906-929`), and the demand-response action string
  `PowerLimit:<max_kw>` (`actor_registry.rs:160-170, 191`);
- `ControlSignal` (`crates/hares-types/src/control_signal.rs:38-160`):
  `PowerSetpoint.active_power_kw` and `reactive_power_kvar`,
  `PowerLimit.max_power_kw` and `ramp_rate_kw_per_s`,
  `ReactiveSetpoint.kvar`, `EvDrive.kwh` and `EvAwayCharge.power_kw`; the
  Python constructors that take them (`crates/hares-python/src/py_actor.rs:696,
  706, 814, 824, 950, 1064, 1070, 1128, 1141, 1187`); the HELICS signal
  body key `active_power_kw` (`python/ochre_next/helics/dwelling.py:181`);
  the gym's action-layout keys (`python/ochre_next/rl/gym_env.py:181-183`
  and `:371-376`: `active_power_kw`, `reactive_power_kvar`,
  `p_setpoint_kw`, `kw`, `max_power_kw`, `ramp_rate_kw_per_s`);
- actor state and telemetry: the BMS actor's `pv_kw`, `load_kw`,
  `bms_pv_stale_kw` and `bms_pv_actual_kw` (`crates/hares-core/src/actors/bms.rs:90-93,
  1042-1061`), the EV driver's `charge_kw` (`actors/ev_driver/mod.rs:379,
  534`) and its composer's kW working values (`ev_driver/composer.rs:79`),
  and the actor checkpoint blobs, each versioned by its
  `ActorStateCheckpoint.schema_version` (`checkpoint.rs:61-69`);
- the rest of the EV driver (`actors/ev_driver/mod.rs`: `DayEvent.drive_kwh`
  `:89`, `Driving { remaining_kwh }` `:104`, `build_preferences(..,
  max_charge_kw, ..)` `:131-265`, the fields `daily_drive_miles`,
  `fuel_economy_kwh_per_mi`, `capacity_kwh`, `max_charge_kw`,
  `average_speed_mph`, `range_anxiety_miles`, `away_charge_power_kw`,
  `expected_daily_miles` at `:286-315`, `new()` at `:339-356`, the mile to
  kWh and trip-hour arithmetic at `:639-722, 815-879, 1273-1275`, and the
  signals at `:1315-1389`) and its modules `composer.rs`,
  `preference.rs` (`ChargeContext`, `Vote.power_kw`), `departure.rs`,
  `price.rs`, `soc_gate.rs`, `soc_target.rs`, `time_window.rs`,
  `solar.rs` (`min_charge_rate_kw`), `v2g.rs` (`max_export_kw`) and
  `v2h.rs` (`max_discharge_kw`); the BMS actor's fields, arguments and
  helpers (`bms.rs:29-30, 49-75, 152-206, 252-547, 720-888`);
- the demand-response compliance actor (`actors/dr_compliance.rs`:
  `DrAction::PowerLimit { max_kw }` `:214`, `limit_power(max_kw)`
  `:287-292`, its signal at `:517-519` and the clearing limit at `:555`) and
  the occupant actor (`actors/occupant.rs`: `EquipmentBehavior.power_setpoint_kw`
  `:73`, `with_power_setpoint(kw)` `:101-103`, its signal at `:360-365`);
  the `Actor` trait's `pv_kw` argument (`crates/hares-core/src/actor.rs:120-126`);
- `ActorSeed` (`crates/hares-equipment/src/lib.rs:31-49`:
  `Battery { max_charge_kw, max_discharge_kw }`, `Ev { capacity_kwh,
  max_charge_kw, fuel_economy_kwh_per_mi }`), produced by `ev/mod.rs:2008-2025`
  and `battery/mod.rs:1642-1652` and read by `build_actors_from_seeds`
  (`dwelling/mod.rs:8033-8189`, the TOU fallback's `surplus_deadband_kw:
  0.0` at `:8090`);
- the EV-driver arguments written as literals: the seed path
  (`dwelling/mod.rs:8158-8174`: 30 mi/day, 30 mph, 20 mi, away-charge
  6.6 kW) and the Python builder `add_ev_with_driver`
  (`py_dwelling.rs:1245-1349`: `fuel_economy = spec.capacity_kwh /
  spec.range_miles` at `:1322`, `30.0 // average_speed_mph`, `20.0 //
  range_anxiety_miles`, away-charge 0.0 kW), and the archetype miles
  schedule (`ev/catalog.rs:481-527`, `build_miles_schedule`);
- the control layer's constructors and readers: `hares-control`
  (`signal.rs:21-99`: `ControlSignalConstructors::power_setpoint(active_power_kw,
  reactive_power_kvar)`, `power_limit(max_power_kw, ramp_rate_kw_per_s)`;
  `compat.rs:10, 63-64`: OCHRE's `P Setpoint` in kW passed straight
  through; the test signals of `dispatch.rs` and `capabilities.rs`); the
  Python `ControlSignal` (`crates/hares-python/src/py_control.rs`:
  `power_setpoint(kw, reactive_kvar=None, ...)` `:30`,
  `power_limit(max_power_kw, ramp_rate_kw_per_s=None)` `:227`,
  `reactive_setpoint(kvar)` `:289`, `ev_drive(kwh)` `:361`,
  `ev_away_charge(power_kw)` `:369`, and the dict keys of `from_dict` and
  `to_dict` at `:411-552, 580-787`), `DispatchRequest` and `Signal`
  (`py_actor.rs:695-949, 1063-1187`, with `max_kw` where `ControlSignal`
  says `max_power_kw`, and no ramp rate on `Signal.power_limit`); the gym's
  Rust action reader (`py_gym.rs:87-129`, `active_power_kw` with the
  aliases `p_setpoint_kw` and `kw`, `max_power_kw`, `ramp_rate_kw_per_s`,
  and its NaN-check names at `:345-357`); the protocol bridge's JSON
  commands (`crates/hares-equipment/src/protocol_bridge/handler.rs:66-128`,
  deserialised by the derived serde of `ControlSignal`, so an old key is a
  serde "missing field" error that names no replacement); and the HELICS
  federates' signal readers (`helics/dwelling.py:181-201, 699`,
  `helics/fleet.py:545`, both through `ControlSignal.from_dict`).

Census command (the evidence above is its classification): at
`afa9a071`,
`git grep -niE "$P" afa9a071 -- ':(glob)crates/*/src/**' ':(glob)python/ochre_next/**'`
with `P` the pattern of `si_census` (Acceptance, commit 4) prints 8 857
lines in 132 files, among them
`crates/hares-types/src/telemetry_keys.rs:22:pub const ELECTRIC_KW: &str = "electric_kw";`.
This measurement predates `05-python-surface.md` PY-7 (schedule row 26),
so it uses `python/ochre_next/**`, the package's path at `afa9a071`. B-14's
own acceptance step runs at row 94c or later, after PY-7 has moved the
package to `python/hares/`, so the step below uses `python/hares/**`, the
pathspec that is non-empty when the step actually runs.

Two in-tree readers of a renamed name sit outside `crates/*/src` and
`python/hares` and are not reached by this census, `si_guard`, pytest's
`testpaths` (`tests/python`) or `cargo nextest run --workspace` (which does
not build `[[bench]]` targets): `notebooks/simulation_explorer.py` (`PV`,
`Battery` and `EV` constructor and attribute reads, and a
`BatteryManagement` actor parameter key, all removed by commits 2 and 4)
and `benches/rl_episode.rs` (the observation keys `total_power_kw` and
`reactive_power_kvar`, renamed by commit 3). Both are added to the census
pathspecs and the `si_guard` scope below, and the bench is compiled by
`cargo bench --no-run`, added to commit 3's Verification, so a rename that
reaches either file fails a check instead of only breaking on a manual run.
Every file it lists is named above or in the boundaries below. The
largest after the actors and the configs are the equipment run state
(`battery/mod.rs`, `generator.rs`, `ev/mod.rs`, `pv/mod.rs`, the HVAC and
water-heater modules, `event_load.rs`, `scheduled_load.rs`,
`ventilation.rs`, `pv/lut.rs`), the metrics calculator, the tariff and
`dwelling/mod.rs`, all commit 3. The conversion helpers most of these
call are in `crates/hares-physics/src/units.rs`: `power_kw_to_w` and
`power_w_to_kw` (`:138-151`, a hand-written `WATTS_PER_KILOWATT`; 78 and
91 call sites in 20 and 18 files), `energy_therms_to_kwh` and
`energy_kwh_to_therms` (`:158-165`), `power_watt_to_kwh_per_year` and
`power_btuh_to_therms_per_year` (`:284-296`), and `power_btu_h_to_w`
with `BTU_PER_HOUR_TO_WATT` (`:118-126`). `uom` 0.38 (`Cargo.lock`) has
`kilowatt`, `watt`, `kilowatt_hour`, `joule`, `mile`, `mile_per_hour`,
`meter_per_second` and `newton`, a rounded `btu_it` (1055.056 J), no
plain therm (`therm_us` 1.054804e8 J and `therm_ec` 1.05506e8 J; HARES's
therm is 100 000 of `uom`'s `btu_it`, 105 505 600 J, `units.rs:158`), no
Btu per hour (HARES uses `BTU_PER_HOUR_TO_WATT` = 0.29307107 W), and no
apparent or reactive power quantity; its `unit!` macro
(`uom-0.38.0/src/unit.rs:103`, exported with the default features)
defines a unit of an existing quantity outside the crate.

Boundaries that keep their units, each with its reason and its one
conversion site. Every line `si_census` prints after commit 4 falls in
one of these classes, a `REMOVED_KEYS` row, or the input of a removed-key
test:
- Reporting names. A `VariableDef` name and its declared reporting unit
  (kW, kWh, kVAR, therms/hour) are the report's contract (`plan.md` §2.1
  "Units", §2.2): the value under the name is SI and the row plan
  converts it once (`Unit::from_si`). The names are: the catalogue
  columns (`crates/hares-io/src/output/columns.rs`); the equipment
  telemetry keys of `crates/hares-types/src/telemetry_keys.rs` (the 34
  constants whose string ends in a unit, `ELECTRIC_KW` `:22` to
  `FUEL_CELL_DC_KW` `:895`, among them `COMPRESSOR_KW`, `FAN_KW`,
  `MAIN_POWER_KW`, `CAPACITY_KWH`, `AWAY_CHARGE_POWER_KW`, and the
  observation-bound literals at `:930-931`); and the actor variables
  declared in `Actor::output_variables()` (the BMS actor's `pv_kw`,
  `load_kw`, `bms_pv_stale_kw`, `bms_pv_actual_kw`; the EV driver's
  `charge_kw`). An actor or equipment that reads such a key reads the SI
  value under it, with no conversion.
- Output records, each converted once from SI where it is written:
  `RunMetrics` (by the metrics calculator), `BillingPeriodSummary` and its
  snapshot and Python form `PyBillingPeriodSummary` (by the tariff
  evaluator, `py_telemetry.rs:144-193`), the Python metrics classes over
  them (`crates/hares-python/src/py_metrics.rs`: `TotalEnergyKwh`,
  `RollingPeakKw`, `PeakPowerKw`, `GridInteractionMetrics`,
  `EnvelopeComponentLoadsKwh`, `GasEnergyMetrics.total_kwh_equivalent`,
  `SimulationMetrics` and `dwelling_metrics_to_polars_df`,
  `conversions.rs:114-134`, which read `RunMetrics`), the zeroed metrics
  record `engine.rs:524-546`, and any fleet per-home rollup that remains
  after `04-fleet.md` FL-1 (today `DwellingMetrics.total_energy_kwh` and
  `peak_power_kw`, `crates/hares-fleet/src/aggregation.rs:22-28`). The
  built-in gym reward stays the negative net grid power in kW
  (`py_gym.rs:494`, documented in `docs/python.md`), converted once from
  `net_electric_power_w`, so a trained policy's reward scale does not
  change; a reward a user writes over `RewardContext` sees SI.
- Tariff rate data: external rate schedules in their own units (per kWh,
  per kW of demand, tier thresholds in kWh: `crates/hares-tariff/src/types.rs:9,
  53, 121-123, 233`, the URDB reader `urdb.rs`, the Python
  `add_energy_rate`, `add_demand_rate(rate_per_kw, ...)`,
  `set_tiered_rates(season, thresholds_kwh, rates_per_kwh)` at
  `py_tariff.rs:397-450, 531`, the price telemetry
  `current_rate_usd_per_kwh` and `export_rate_usd_per_kwh`, and the HELICS
  price in currency per kWh). The evaluator converts the SI energy or
  power once where it prices it.
- The compiled-in product catalogues `BatterySpec` (`battery/catalog.rs:97-102`)
  and `VehicleSpec` and the driving archetypes (`ev/catalog.rs:106-120,
  481-527`): datasheet values kept in datasheet units as data, converted
  by the catalogue methods below where each fills a config or builds a
  schedule.
- External data files, converted once by their loader: `defaults/ev/vehicle_mapping.csv`
  (kWh and kW columns, read by `load_vehicle_mapping_csv` into SI
  `VehicleMappingEntry` fields), the ResStock metadata column
  `out.site_electricity_kwh` (`crates/hares-io/src/resstock.rs:471, 515`),
  and the PV look-up table's `ac_power_kw` column written by the PySAM
  adapter (`python/ochre_next/adapters/sam_pv.py:413, 571`, read by
  `crates/hares-equipment/src/pv/lut.rs`). `defaults/battery/default_parameters.csv`
  is in kWh and kW but no code reads it (the battery loader reads only
  `*.toml`, `defaults.rs:500, 764`); it is not a boundary but a dead file,
  handed to `11-other-defects.md`.
- HPXML inputs, read in the unit the document declares and converted once
  where read: the EV `MaxChargingPower` (kW) and `BatteryCapacity` (kWh),
  the battery `NominalCapacity`, the annual consumption of loads in kWh/yr
  and therm/yr and their `units` attribute strings (`xml_helpers.rs:96-149`:
  `child_energy_kwh`, `child_load_kwh` and `child_load_therms` become
  `child_energy_j` and `child_annual_energy_j`, returning J and J per
  year with one `uom` conversion each), and
  `SyntheticHvacConfig.heating_capacity_kbtu_h` (`synthetic.rs:159`), a
  synthetic input written into the HPXML document in HPXML's own unit.
- OCHRE's control key `P Setpoint` (kW) in `hares-control`'s compatibility
  mapping (`compat.rs:10, 63-64`): an OCHRE input format, converted to W
  once where it is mapped to `power_setpoint`.
- External libraries: the PySAM adapters (`python/ochre_next/adapters/sam_battery.py`,
  `sam_pv.py`) pass kWh and kW to PySAM, and the PyBaMM adapter
  (`adapters/pybamm_battery.py:231, 304`, `power_range_kw` and its
  `power_kw` table column) to PyBaMM, in the units those libraries take.
- HELICS publications carry SI values and declare SI units (`units=W`,
  `units=VAR`; commit 3), so no HARES code converts for a federate; a
  federate that subscribes in kW gets HELICS's own unit conversion.

**Change, commit 2 (equipment config fields):**
- Each field in the table is renamed and stored in SI: W, J, VA (an
  apparent power, dimensionally W) and J/m. Every value that enters a
  config in another unit is converted once, with `uom`, at the boundary
  where it is read: `Power` from `kilowatt` to `watt`, `Energy` from
  `kilowatt_hour` to `joule`, and fuel economy as
  `Energy::new::<kilowatt_hour>(x) / Length::new::<mile>(1.0)` read in
  `newton` (J/m). The boundaries are the input boundaries listed under
  Evidence: the HPXML resolvers (an HPXML value in W, such as a PV
  `MaxPowerOutput` or a battery `RatedPowerOutput`, now enters with no
  conversion; the EV `MaxChargingPower` (kW) and `BatteryCapacity` (kWh)
  elements are converted where `resolve_der.rs` and `resolve_loads.rs`
  read them, `child_energy_kwh` becoming `child_energy_j`; the plug-load
  EV of `resolve_loads.rs:868-902` holds its range in m, its capacity in J
  and its charging power in W, and `EV_FUEL_ECONOMY` becomes a J/m
  constant written from its value, 325 Wh per mile (`1000.0 / 325.0`
  miles per kWh at `afa9a071`), with `uom`), the
  schedule resolver, the defaults loaders (`crates/hares-io/src/defaults.rs`,
  by the unit column of each file; `VehicleMappingEntry` holds
  `capacity_j` and `charger_power_w`), the catalogue fills, the BMS
  actor's `BmsMode` fill and the Python builders (`py_hvac.rs:306`
  included). A hand-written `* 1000.0`, `/ 1000.0`, `* 3.6e6` or
  `/ 3600.0` conversion of these fields is deleted; where an equipment or
  actor still computes in kW at this commit it converts the SI config
  value with `uom` once at construction, and commits 3 and 4 delete those
  conversions.
- The catalogue conversions live on the catalogue types in
  `hares-equipment`, which already depends on `uom`: `BatterySpec::capacity_j()`,
  `max_charge_w()` and `max_discharge_w()`, and `VehicleSpec::capacity_j()`
  and `fuel_economy_j_per_m()`. `BatterySpec::to_config`,
  `VehicleSpec::to_config`, `PyBattery::from_spec` and `PyEv::from_spec`
  call them, so the Rust and Python fills share one conversion.
  `hares-core` (the BMS actor's fill at `actor_registry.rs:486`, the
  dwelling's injected zone capacitance) and `hares-python` (the EV builder
  defaults at `py_dwelling.rs:539-542`, datasheet values in kWh and kW)
  gain `uom = { workspace = true }`, one call per crate (`cargo add`
  takes one `-p` and has no `--workspace` flag, measured with cargo
  1.94.1): `cargo add -p hares-core uom` and `cargo add -p hares-python
  uom`, each of which writes `uom.workspace = true` against the workspace
  entry.
- The raw-key constants listed as deleted above go, with every raw-key
  read behind them (the reads in `ev/config.rs:229-300` and
  `ev/mod.rs:300-490`; a helper there that the typed path also uses, such
  as the default charging-power rule, keeps only its typed inputs): an EV
  is built from a typed `EvConfig` only, and a raw EV payload stays the
  construction error it is today. The tests
  that built configs through those keys (`ev/tests.rs:94-163` and its
  callers, the `#[cfg(test)]` battery and generator builders) build the
  typed configs directly, in SI.
- Every renamed key is a `REMOVED_KEYS` row (`plan.md` §2.7, scope
  `EquipmentConfig`) whose key is `<ConfigType>.<key path>` and whose
  replacement names `<ConfigType>.<new key path>` and the unit (for example
  `BatteryConfig.capacity_kwh` names `BatteryConfig.capacity_j (J)`). The
  key format the lookup sees:
  - `<ConfigType>` is the config type, not the class name an override is
    keyed by: the override merge sees the equipment's class (`"PV"`,
    `"Battery"`, `"ASHP Cooler"`, `"Clothes Washer"`) and maps it to its
    config type through the registry entry that builds the class (the
    same class-to-config mapping `12-inputs.md` IN-3's
    `OverrideClassSchema` lists), so a key shared by several configs
    (`capacity_kwh` on `BatteryConfig` and `EvConfig`, `crankcase_heater_kw`
    on three cooling configs) resolves to its own row. Raw equipment rows
    use the reading type's name: `EventBasedLoad`, `ScheduledLoad`.
  - `<key path>` is the dotted path of the key inside the payload. An
    array element is written `[]` (`PvConfig.arrays[].capacity_kw`); a
    JSON-string payload (`bms_mode`, `charging_strategy`) is parsed and its
    keys are pathed under the variant
    (`BatteryConfig.bms_mode.SelfConsumption.surplus_deadband_kw`,
    `EvConfig.charging_strategy.V2G.max_export_kw`).
  - A pattern row writes `<n>` where the key carries a phase number
    (`EventBasedLoad.phase_<n>_power_kw`); `removed_key` matches `<n>`
    against a run of ASCII digits and every other character exactly.
- Every input surface checks every key it receives against `REMOVED_KEYS`
  before deserialising, and an old key fails naming its replacement:
  - the override merge (`merged_equipment_config`,
    `crates/hares-core/src/dwelling/conversions.rs:773-841`), for the
    typed and the raw path alike, walking every key of the override
    payload, the `"all"`/`"*"` payload included, before
    `apply_equipment_overrides` merges it (OD-2 rejects unknown keys on the
    typed path; the raw path accepts any key, so without this walk an old
    raw key would be silently ignored);
  - the synthetic TOML event-load table: `SyntheticEventLoadConfig` gains
    `#[serde(deny_unknown_fields)]` and its keys are checked against
    `REMOVED_KEYS` first, so `active_power_kw` fails naming
    `EventBasedLoad.active_power_w (W)` instead of being ignored;
  - the HPXML extension element the synthetic path writes and the resolver
    passes through (`synthetic.rs:936-941`, `xml_helpers.rs:357`) is renamed
    `active_power_w`; an HPXML document carrying the old
    `active_power_kw` extension element fails parsing naming the element
    and `active_power_w` (with `12-inputs.md` IN-3's `HPXML_ELEMENTS` entry
    renamed and `active_power_kw` added to `HPXML_REJECTED_ELEMENTS` in the
    same commit);
  - the Python equipment constructors and the Python `BmsMode` and
    `ChargingStrategy` constructors (below).
- The Python constructors take the new names, and every renamed
  parameter is keyword-only (a `*` before it in the `#[pyo3(signature)]`),
  because a removed keyword is the only form the removed-key lookup sees:
  under PyO3 0.29 every parameter without a signature, and every one
  before `*`, is positional-or-keyword, so `PV("pv", 5.0, 25.0, 180.0)`
  would otherwise run after this commit and build a 5 W array. The
  signatures: `Battery(name, *, capacity_j, max_charge_w=None,
  max_discharge_w=None, ..., inverter_capacity_va=None, ...)`;
  `PV(name, *, capacity_w, tilt, azimuth, sam_lut_path=None,
  soiling=None)`; `EV(name, *, capacity_j=None, max_charging_power_w=None,
  ..., charger_capacity_va=None, ...)`, where the EV's `max_charging_kw`
  keyword, which is not a field of `EvConfig`, takes the Rust field's name
  and its removed-key row is written out explicitly
  (`EvConfig.max_charging_kw` names `EvConfig.max_charging_power_w (W)`),
  since the `<ConfigType>.<key path>` rows of the table do not produce
  it; `BmsMode.self_consumption(*, min_soc=0.1, max_soc=1.0,
  solar_only_charging=False, surplus_deadband_w=0.0)`;
  `ChargingStrategy.solar_surplus(*, min_charge_rate_w,
  departure_schedule=None)`; `ChargingStrategy.v2g(*, min_soc,
  max_export_w, price_threshold)`. A call that passes a renamed parameter
  positionally raises `TypeError`; a removed keyword raises its
  removed-key error. Their `_hares.pyi` stubs are rewritten to the
  bindings in full (at `afa9a071` they omit `Battery`'s `power_factor` and
  `inverter_capacity_va`, `PV`'s `sam_lut_path`, `EV`'s `power_factor`,
  `charger_capacity_kva` and thermal and cell parameters, and
  `self_consumption`'s `surplus_deadband_kw`), and the `docs/python.md`
  changes section (`05-python-surface.md` PY-9) states the keyword-only
  change.
- `11-other-defects.md` OD-2 site 7's range check becomes 19 200 W.
- `register_pv_roof_shading` reads the PV capacity from the JSON key
  `capacity_kw` (`crates/hares-core/src/dwelling/mod.rs:1570-1600`),
  which this commit renames; it reads `capacity_w` here, and
  its collector-area expression takes the W value in place of its own
  `* 1000.0`, so roof coverage is unchanged, and
  `11-other-defects.md` OD-20 replaces the key reads with the typed config.
- `06-runtime-memory.md` RT-6's kW-to-SI conversions in
  `resolved_parameters()` are deleted. The read-back names are the
  resolved model's (`plan.md` §2.7's required-parameter table) and stay;
  each reads its SI field directly: PV `nameplate_w` from
  `PvConfig.capacity_w`, battery `energy_capacity_j` from
  `BatteryConfig.capacity_j` and `power_capacity_w` from
  `BatteryConfig.max_charge_w`, EV `battery_capacity_j` from
  `EvConfig.capacity_j` and `max_charging_power_w` from
  `EvConfig.max_charging_power_w`.
- `12-inputs.md` IN-3's `DescribeFields` declarations give each renamed
  field its SI `Unit`, so `hares.input_schema()` lists the new names and
  units and its `removed_keys` lists the rows.
- `docs/equipment/*.md` and `docs/PHYSICS_DECISIONS.md` (a section: the
  rule, the boundaries where conversion happens, and that OCHRE stores
  these in kW and kWh) are updated, as is every other current document
  that names a renamed field, method, column or unit: at `7b7a6a7d`,
  `docs/architecture.md`, `docs/invariants-and-observability.md`,
  `docs/outage-behavior.md` and `docs/rust-primer.md`
  (`git grep -lE '_kw\b|_kvar\b|_kwh\b' -- '*.md' ':!docs/findings'
  ':!docs/reviews' ':!docs/tickets' ':!vendors'` re-run when each commit
  starts, every hit read). Dated records (`docs/findings/`,
  `docs/reviews/`, `docs/tickets/`) describe the code as it was and are
  not rewritten.
- `tracing` at `debug`: each converted value at its boundary, with both
  units.

**Change, commit 3 (run state):**
- Every quantity under "Commit 3, run state" is held and computed in W, J,
  VAR (a reactive power, dimensionally W) and J/m, and renamed to match
  (`cumulative_import_j`, `peak_demand_w`, `prior_period_peaks_w`,
  `cumulative_ev_j`, the calculator's `*_j` totals and `*_w` peaks,
  `pv_generation_w`, `net_grid_w`, `CoreFlows.electric_w` (the
  `ElectricPower` variants, `crates/hares-types/src/equipment.rs:1058-1090`,
  carry W), `reactive_power_var`, `main_power_w`, `total_power_w`,
  `equipment_power_w`, `island_unserved_w`, `island_excess_w`,
  `expected_mean_power_w` and the premise dict key `mean_power_w`). Each
  equipment drops the construction-time kW conversion commit 2 left and
  computes in SI.
- Control signals still carry kW, kVAR and kWh at this commit (commit 4
  moves them), so each equipment that applies a signal value converts it
  to SI once with `uom` where it applies the signal (the `apply_control`
  arm that reads the field), and computes in SI from there; commit 4
  deletes those conversions with the field renames. The same holds for
  every actor that reads the electrical summary or equipment telemetry:
  it converts the SI value to its kW working value once at the read until
  commit 4.
- Every equipment publishes SI values to the telemetry variables it
  declares; each declared variable keeps its name and its reporting unit,
  and the row plan converts (`plan.md` §2.2), so no equipment column
  changes name or unit.
- The values leave the run state in a reporting unit only at the output
  records of "Boundaries that keep their units", each one `uom` conversion
  where the record is written: the row plan converts a value to its
  `VariableDef`'s declared unit when it fills the report row (so the
  `Total Electric Power (kW)` column reads `net_electric_power_w`
  converted to kW, and `steps_to_polars_df` reads the same through
  `Unit::from_si`); the tariff evaluator converts energy and demand where
  it prices them and where it writes a `BillingPeriodSummary`; the metrics
  calculator converts where it writes `RunMetrics`; the built-in gym
  reward converts the net power to kW. Every other reader of run state
  takes SI under an SI name, the publishers included: the HELICS
  publications become `total_power_w` (`units=W`), `reactive_power_var`
  (`units=VAR`), `equipment_{i}/power_w` and `pv_generation_w`
  (`units=W`), and the fleet's `aggregate_power_w`,
  `aggregate_reactive_var` and per-dwelling and per-equipment
  publications likewise, each reading the SI telemetry field; the gym's
  observations become `total_power_w`, `reactive_power_var` and the
  equipment `power_w`, with their `Box` bounds in W and VAR, (-100 000,
  100 000) for total and reactive power and (0, 100 000) for equipment
  power, the kW bounds times 1000; `PyTariffTelemetry`'s live billing
  fields become `cumulative_import_j`, `cumulative_export_j` and
  `peak_demand_w` (its rate fields are tariff data and stay). No other
  code converts these values. The row plan's conversion is `Unit::from_si` (`plan.md`
  §2.2), which this commit adds with its inverse `Unit::to_si`:
  `metrics_from_table` (`03-output-reporting.md` OR-15) reads a table in
  reporting units, so it converts each input column to SI with `to_si`
  before the calculator reads it. A value converted to its reporting unit
  and back is not always the same `f64`, so OR-15's two equality tests
  (`metrics_from_table_matches_in_run_metrics` and
  `metrics_from_parquet_file_matches_in_run_metrics`) compare each metric
  against its gross magnitude from this commit on: `|a - b| <= 1e-12 * G`,
  where `G` is the sum over steps of the absolute step contribution for an
  accumulated metric (so a net energy whose imports and exports cancel is
  bounded by the energy that flowed, not by the small net) and the
  magnitude of the value itself for a peak or a count; each test prints
  the largest `|a - b| / G`. The in-run metrics stay bitwise across runs.
  `hares-types` (for `Unit::from_si` and `to_si`) and `hares-tariff` (for
  the evaluator's conversions) gain `uom = { workspace = true }`, one call
  per crate: `cargo add -p hares-types uom` and `cargo add -p hares-tariff
  uom` (`hares-envelope`'s electrical solver holds W and VAR and converts
  nothing, so it needs none).
- The conversion helpers of `crates/hares-physics/src/units.rs`:
  `power_kw_to_w`, `power_w_to_kw` and `WATTS_PER_KILOWATT`,
  `energy_therms_to_kwh`, `energy_kwh_to_therms`,
  `power_watt_to_kwh_per_year` and `power_btuh_to_therms_per_year` are
  deleted. Each call site either holds SI after this commit and needs no
  conversion, or is a boundary of the list above and converts once with
  `uom` there. `power_btu_h_to_w`, `power_w_to_btu_h` and
  `power_btu_h_to_kbtu_h` are rewritten on `uom` with the two units
  `uom` lacks, defined once with `uom`'s `unit!` macro in `hares-types`
  (`crates/hares-types/src/si_units.rs`, new; the lowest crate, because
  `Unit::from_si` for `therms/hour` lives there and `hares-physics`
  depends on it): `therm` (energy, 1.055056e8 J, 100 000 of `uom`'s
  `btu_it`, which is the value `energy_therms_to_kwh` computes today) and
  `btu_it_per_hour` (power, 0.29307107 W, today's `BTU_PER_HOUR_TO_WATT`,
  which the constant then becomes), so the definitions move no value
  beyond its last bit; this commit is bitwise-neutral (its own numeric
  commits are B-1 through B-13 and B-14's run-state commit, none of
  which touch the therm or Btu/h factor), so it keeps today's rounded
  factor rather than adopting OpenStudio-HPXML's exact one. HARES's therm
  (105 505 600 J, 100 000 of `uom`'s rounded `btu_it`, 1.055056e3 J) is
  14.738 J larger than OpenStudio-HPXML v1.12.0's
  (`HPXMLtoOpenStudio/resources/unit_conversions.rb`: 1055.05585262 J per
  Btu, times 100 000), 1.4e-7 relative; `btu_it_per_hour` differs by
  5.9e-10 relative (0.29307107 W against 0.2930710701722 W). Adopting
  OpenStudio-HPXML's exact constant would move every therm- and
  Btu/h-denominated value HARES stores by that fraction, which is a
  numeric change and so its own commit, not folded into this one. A
  rate in therms per hour is `therm` over `uom`'s `hour`. An annual energy
  is an `Energy` over the year: the HPXML annual readers return J per year
  (`child_annual_energy_j`), the raw keys `annual_electric_kwh` and
  `annual_gas_therms` become `annual_electric_j` and `annual_gas_j`, and
  `determine_max_kw` and `determine_constant_kw` become `determine_max_w`
  and `determine_constant_w`, dividing by the year's seconds once. These
  raw keys are written and read only inside `hares-io`, so they get no
  `REMOVED_KEYS` row. The length, area, volume, temperature and
  thermal-property helpers of the file convert HPXML inputs at the read
  and are not part of this rule.
- The ZIP model (`ZipLoad::apply(p_w, voltage_pu) -> (real_w,
  reactive_var)`, `ZipLoad::reactive_var`), the ground-loop pump
  (`compute_ground_loop_pump_power_w`), the battery's charging-curve rate
  (`charging_lut_c_rate(power_w, soh_adjusted_j)`), the mode-flow guard,
  `ElectricalAccumulator.reactive_power_var`, the electrical solver
  (`net_active_w`, `net_reactive_var`, its payload in W and VAR) and its
  readers, `DwellingTelemetry::verify_consistency`, the invariant checker
  (`total_heating_j`, `total_cooling_j`, `check_ev_capacity_degraded` in
  J, `check_electrical` and `check_reactive` in W and VAR), and the
  observer and diagnostics records take SI names and SI values. That
  corrects the two observer fields that hold W under kW names today
  (`electrical_load_kw` and `electrical_gen_kw` become `electrical_load_w`
  and `electrical_gen_w`, holding the W they already hold); the
  diagnostics CSV header names `electrical_net_w` and `electrical_net_var`.
- The tariff evaluator takes SI: `TariffEvaluator::step(net_power_w,
  ev_power_w, dt_seconds, current_time)`, fed by the dwelling with
  `net_active_w`, and `BillingState::update` accumulates J; the Python
  `TariffEvaluator.step(*, net_power_w, dt_seconds, current_time)` names
  its renamed parameter keyword-only, and `net_power_kw` is a
  `REMOVED_KEYS` row of scope `PythonKeyword`.
  `Dwelling.estimate_pv_capacity(*, target_w, min_w=2000.0, max_w=14000.0,
  diffuse_fraction=None, panel_watts=None, ..., inverter_ac_w=None, ...)`
  takes the PV-sizing keywords of the row below, keyword-only, and its
  stub is rewritten to the binding in full (at `afa9a071` it lacks
  `diffuse_fraction`, the panel arguments, `inverter_kw_ac`,
  `max_dc_ac_ratio` and `roof_shape`).
- `BillingSnapshot` and `MetricsAccumulators` take the SI names
  (`plan.md` §2.1, "Snapshot units"); `CHECKPOINT_VERSION` rises from 9 to
  10, and loading a version 9 checkpoint is a typed error naming both
  versions.
- `StepResult.net_electric_power_kw` becomes `net_electric_power_w`
  (`plan.md` §2.1); the Python step dict key, the `SteppableFleet` step
  dict keys, the `telemetry()` fields, the island dict keys and the ports
  dict keys take the SI names, and `to_observation_vec` and the gym
  resolve the SI observation names (`total_power_w`; the gym's alias
  `total_electric_kw` is deleted). An observation name this commit renames
  fails naming its replacement (`plan.md` §2.7, scope `Observation`, which
  this commit adds to `KeyScope`), and `RewardContext.total_power_kw`
  becomes `total_power_w`.
- The scheduled load's gas rates: the raw keys `gas_profile_max` and
  `gas_constant` (therms per hour) become `gas_profile_max_w` and
  `gas_constant_w`, written by the schedule resolver with one `uom`
  conversion from the source's unit; each old key is a `REMOVED_KEYS` row
  of scope `EquipmentConfig` (`ScheduledLoad.gas_constant` names
  `ScheduledLoad.gas_constant_w (W)`), and the gas column the scheduled
  load reads is converted once where the resolver reads it, so
  `GasScheduleUnit` is deleted.
- The PV-sizing calculation computes in W, and its Python functions take
  and return W with every renamed parameter keyword-only
  (`size_pv_system(usable, *, target_w, min_w=2000.0, max_w=14000.0,
  inverter_ac_w=None, ...)`, `required_main_panel_ampacity(*, target_ac_w,
  main_breaker_amps=None)`, and the result fields `capacity_w`,
  `max_capacity_w`, `max_roof_capacity_w`, `max_ac_w` and
  `UsableRoofArea.max_capacity_w`); each old keyword is a `REMOVED_KEYS`
  row of scope `PythonKeyword`, which `estimate_pv_capacity` and
  `TariffEvaluator.step` consult as the PV-sizing functions do.
- The Python step and telemetry surfaces: `PyTelemetry`'s getters
  (`total_power_w`, `reactive_power_var`, `island_unserved_w`,
  `island_excess_w`) and its `equipment()` key `power_w`,
  `CoreOutput.electric_w` and `reactive_power_var`, the observe-build
  snapshot keys `electrical_load_w`, `electrical_gen_w`,
  `electrical_reactive_var`, the step info key `net_electric_power_w` of
  the Rust gym batch step, and the default `PyDwelling::observation()`
  field list (`total_power_w`).
- Python stubs, `docs/python.md` (its changes section and one "Numeric
  changes" line, `plan.md` §2.8), `docs/equipment/*.md` and the
  `docs/PHYSICS_DECISIONS.md` section commit 2 adds follow.

**Change, commit 4 (control layer):**
- Every actor parameter under "Commit 4, control layer" is read in SI
  under a new name: `daily_drive_distance_mean_m`,
  `fuel_economy_j_per_m`, `capacity_j`, `average_speed_m_per_s`,
  `max_charge_w`, `range_anxiety_distance_m`, `away_charge_power_w`,
  `surplus_deadband_w`, `max_discharge_w`. This commit adds the
  `KeyScope` variants `ActorParameter` and `ControlSignal` (`plan.md`
  §2.7). `ActorRegistry::create` checks
  every parameter key against `REMOVED_KEYS` (scope `ActorParameter`, key
  `<actor_type>.<parameter>`, for example `EvDriver.capacity_kwh` names
  `EvDriver.capacity_j (J)`) before the factory runs. The BMS actor's
  `BmsMode` fill (`actor_registry.rs:486`) passes the W value through
  with no conversion, and the conversion commit 2 put there is deleted.
  The demand-response action string takes its limit in W with an explicit
  unit suffix, `PowerLimit:<max>W` (`PowerLimit:5000W`): a bare number
  cannot be told apart between kW and W, so `PowerLimit:5` fails naming
  the suffixed form.
- `ControlSignal`'s fields become `active_power_w`,
  `reactive_power_var`, `max_power_w`, `ramp_rate_w_per_s`,
  `ReactiveSetpoint.var`, `EvDrive.energy_j` and `EvAwayCharge.power_w`.
  Each equipment that applies a signal reads the W or J value with no
  conversion: the conversion commit 3 put at each application is deleted.
  `hares-control`'s constructors take the same names
  (`power_setpoint(active_power_w, reactive_power_var)`,
  `power_limit(max_power_w, ramp_rate_w_per_s)`), and its OCHRE mapping
  converts `P Setpoint` from kW once (`compat.rs:63-64`).
- Every Python surface that builds a signal takes the Rust field names,
  keyword-only: `ControlSignal.power_setpoint(*, active_power_w,
  reactive_power_var=None, min_soc=None, max_soc=None)`,
  `.power_limit(*, max_power_w, ramp_rate_w_per_s=None)`,
  `.reactive_setpoint(*, var)`, `.ev_drive(*, energy_j)`,
  `.ev_away_charge(*, power_w)`; the same fields on `DispatchRequest`
  (after its `target`) and `Signal`, whose `power_limit` gains the
  `ramp_rate_w_per_s` the other two take, so the three surfaces no longer
  differ (`kw`, `reactive_kvar` and `max_kw` at `afa9a071`); and the dict
  form of `ControlSignal.from_dict` and `to_dict`. Each old keyword and
  dict key (the three surfaces' names, `kw`, `reactive_kvar`, `max_kw`
  and `kvar` included), the HELICS signal body keys, the gym's
  action-layout keys (Python and the Rust reader `py_gym.rs:87-129`) and
  the protocol bridge's JSON command keys is a `REMOVED_KEYS` row of scope
  `ControlSignal` and fails naming its replacement (the gym's aliases
  `p_setpoint_kw` and `kw` are deleted and name
  `PowerSetpoint.active_power_w`). The protocol bridge checks each
  command's signal keys against the table before serde deserialises it
  (`handler.rs:128`), so an old payload names its replacement instead of
  failing as a missing field.
- `ActorSeed` (`Battery { max_charge_w, max_discharge_w }`, `Ev {
  capacity_j, max_charge_w, fuel_economy_j_per_m }`) and its producers and
  reader, the demand-response compliance actor (`DrAction::PowerLimit {
  max_w }`, `limit_power(max_w)`), the occupant actor
  (`EquipmentBehavior.power_setpoint_w`, `with_power_setpoint(w)`), the
  `Actor` trait's PV argument (`pv_w`), and every EV-driver module hold
  SI. The EV-driver values written as literals today (daily distance 30
  mi, speed 30 mph, range anxiety 20 mi, away-charge power 6.6 kW on the
  seed path) become named constants in one place, `ev_driver::defaults`,
  each written from its documented value with `uom` (48 280.32 m,
  13.4112 m/s, 32 186.88 m, 6600 W), read wherever the seed path, the
  registry defaults (`actor_registry.rs:383-413`) and the Python builder
  use that value today, so no value moves and none is repeated. (The
  Python builder's different away-charge power, 0.0 W, and its fuel
  economy from capacity over range rather than the catalogue's own
  figure, `py_dwelling.rs:1322, 1349`, are behaviour differences, handed
  to `11-other-defects.md` rather than changed inside this unit move.) The archetype schedules of
  `ev/catalog.rs` are built in metres from their mile parameters with one
  conversion in `build_miles_schedule`, renamed `build_distance_schedule`.
- The BMS and EV-driver actors hold and compute their state in SI and
  publish SI values. A published value is in the SI unit of its declared
  variable's dimension (`plan.md` §2.2), so the variables they declare in
  `Actor::output_variables()` keep their names and their reporting unit
  (`pv_kw`, `load_kw`, `bms_pv_stale_kw`, `bms_pv_actual_kw`, `charge_kw`,
  each kW): the report columns `actor:<name>:<key>` are unchanged, and the
  row plan converts. A declared variable name is a reporting name, which
  the census lists as a boundary. The conversion commit 3 left at their
  read of the electrical summary is deleted. Each actor whose checkpointed
  state changes raises its `schema_version`, and loading a blob of the
  older version is a typed error naming the actor and both versions.
- Python stubs, `docs/python.md` (its changes section and one "Numeric
  changes" line), `docs/equipment/battery.md`, `ev.md` and
  `power-factor.md` where they name signal fields, and the
  `docs/PHYSICS_DECISIONS.md` section follow.

**Acceptance test, commit 2:**
- `equipment_config_fields_are_si` (a `crates/hares-io` test over the
  config types themselves, so it can pass: it reads no source text and
  does not see `BatterySpec` or `VehicleSpec`, which keep their datasheet
  units, or the run state, which commit 3 moves): for every config type
  in the table that lives below `hares-core` (every one but the synthetic
  `SyntheticEventLoadConfig`, a `hares-core` type, which
  `input_schema_override_classes_are_si` below walks the same way), the
  test builds an instance with every optional field set
  and, for `BatteryConfig.bms_mode` and `EvConfig.charging_strategy`, once
  per payload variant, serialises it to JSON, parses the string payloads,
  and walks every key path; no key matches `(^|_)(kw|kwh|kva|mi|miles|mph)(_|$)`.
  It calls nothing in `hares-core`, which `hares-io` does not depend on.
- `input_schema_override_classes_are_si`
  (`crates/hares-core/tests/si_config_schema.rs`, a `hares-core` test
  because `hares_core::input_schema()` is assembled in `hares-core`, the
  lowest crate that sees every described type, `plan.md` §2.7 and
  `12-inputs.md` IN-3): every field of every class of
  `hares_core::input_schema().overrides` has a `Unit` whose `symbol()` is
  none of `kW`, `kWh`, `kVA`, `kVAR`, `kW/s`, `kWh/mi`, `kWh/K`, `mi` and
  `mph`, and no field name matches the pattern above, so a config type added later
  without its own case in the walk above still fails; and a
  `SyntheticEventLoadConfig` with every field set, serialised to JSON, has
  no key that matches it.
- `raw_equipment_keys_are_si`: `git grep -niE
  'const [A-Z0-9_]+: &str = "[A-Za-z0-9_]*(_kw|_kwh|_kva|_miles)[A-Za-z0-9_]*"'
  -- ':(glob)crates/hares-equipment/src/**'` and `git grep -nE
  '"(BatteryCapacity|MaxChargingPower|range_miles)"' --
  ':(glob)crates/hares-equipment/src/**'` print nothing (run in the
  verification step). Every `&str` constant counts, not only `KEY_`
  ones, so the phase-key suffixes `PHASE_POWER_SUFFIX_A` and `_B`
  (`"_power_kw"`, `event_load.rs:136, 141`, joined at `:1832-1834`) are
  covered; at `afa9a071` the first command prints 22 lines, among them
  `crates/hares-equipment/src/event_load.rs:136`.
- `si_guard` (`crates/hares-equipment/tests/si_guard.rs`, run by `make
  check-si-guard` and CI, `ci.yml:46-53`) widens its scan from
  `hares-equipment/src` (`:505-507`) to the `src` of every workspace crate,
  `notebooks/` and `benches/` (the two directories outside any crate's
  `src` that read a renamed name, named above) and gains the unit-suffix
  rule its whole-identifier rules cannot express
  (they use `\b...\b`, so `annual_gas_therms` never matches one): an
  identifier with a segment `kw`, `kwh`, `kva`, `kvar`, `kvarh`, `therms`,
  `mi`, `miles` or `mph`
  (`(?i)(?:^|_)(kw|kwh|kva|kvar|kvarh|therms|mi|miles|mph)(?:_|$)` over
  each identifier `syn` collects) is a violation, with a per-file
  allowlist whose every entry names the boundary of "Boundaries that keep
  their units" it holds (for example `battery/catalog.rs` and
  `ev/catalog.rs`, the catalogues; `telemetry_keys.rs`, reporting names;
  `hares-control/src/compat.rs`, the OCHRE key). Commit 2 adds the rule
  with an allowlist that also lists every file still holding run-state or
  control names, each marked with the commit that removes it; commits 3
  and 4 shrink it, and after commit 4 it lists only boundaries. The
  existing rules keep their scope. A file
  `syn` cannot parse is a test failure naming it, not zero violations
  (`si_guard.rs:418-423` returns none today). Commit 3 removes
  `scheduled_load.rs` from the `therm` rule's allowlist (`:128-132`), as
  its gas rates leave therms.
- `removed_equipment_key_names_its_replacement` (a `crates/hares-core`
  unit test of `merged_equipment_config`), parametrized over every row
  this entry adds: a spec of the row's class (typed or raw) built in the
  test with that type's required fields, merged with an override setting
  the old key path to 1.0 (for a pattern row, `phase_2_power_kw`; for an
  array row, the key inside `arrays[1]`; for a payload row, the key inside
  the `bms_mode` or `charging_strategy` JSON string; each once under the
  class name and once under `"all"`), fails naming the row's replacement.
  The same holds on a raw `EventBasedLoad` spec, whose payload is not
  deserialised into a typed config.
- `synthetic_event_load_removed_key_names_its_replacement`: a synthetic
  TOML whose `[event_load]` table carries `active_power_kw = 1.0` fails
  naming `EventBasedLoad.active_power_w (W)`, and one carrying an unknown
  key `x = 1.0` fails naming `x`.
- `hpxml_active_power_kw_extension_is_rejected`: the synthetic-written
  HPXML of a dwelling with an event load, with its `active_power_w`
  extension element renamed back to `active_power_kw` by the test, fails
  parsing naming the element and `active_power_w`.
- `pv_capacity_kw_override_fails_construction`: end to end,
  `tests/fixtures/hpxml/ochre_samples/base-pv-battery.xml` (the battery
  declaring `{"Battery": {"bms_mode": "\"Manual\""}}`, as
  `11-other-defects.md` OD-8 requires) with the override
  `{"PV": {"capacity_kw": 5.0}}` fails construction naming
  `PvConfig.capacity_w`.
- `test_equipment_constructor_removed_keyword_names_replacement` (Python),
  parametrized over the removed keywords of `Battery`, `PV`, `EV`
  (`max_charging_kw` among them), `BmsMode.self_consumption`,
  `ChargingStrategy.solar_surplus` and `ChargingStrategy.v2g`; and, for
  each, the positional call of the parent's signature
  (`PV("pv", 5.0, 25.0, 180.0)`, `Battery("b", 10.0)`,
  `ChargingStrategy.v2g(0.2, 5.0, 0.3)`, `ChargingStrategy.solar_surplus(1.0)`,
  `BmsMode.self_consumption(0.1, 1.0, False, 0.5)`, `EV("ev", 60.0)`)
  raises `TypeError`.
- `equipment_config_values_are_unit_invariant` (a `crates/hares-core`
  test, one case per row of the commit 2 table and per field in it, the
  plug-load EV, `VehicleMappingEntry` and the Python AC builder's
  `crankcase_heater_w` included): each case builds the config through
  its real input path (the HPXML resolver for PV, battery, generator,
  crankcase heater and the plug-load EV; the override merge for the
  payload fields, `zone_capacitance_j_per_k` through the dwelling's
  injection; the schedule resolver for the event and scheduled load keys;
  the defaults loader for the vehicle mapping; where no fixture carries
  the field, the case writes its input, an HPXML element or an override
  value, and states it) and asserts the SI value equals the value the
  parent commit stored, recorded in the case as a literal in the parent's
  unit and converted once with `uom`, within 2 ULP (the parent's division
  by 1000 and this conversion each round once; a missed or doubled
  conversion is off by a factor of 1000 or more). This covers every
  parameter the resolved model does not report (crankcase heaters,
  inverter and charger apparent power, `max_discharge`, the generator,
  event and scheduled load powers, fuel economy, V2L and V2G).
- `hpxml_pv_capacity_enters_in_watts`: `base-pv.xml`'s two PV systems
  (`MaxPowerOutput` 4000 W and 1500 W) each equal their
  `PvConfig.capacity_w` on bits.
- `battery_catalogue_fill_converts_with_uom`: a `BatteryConfig` filled
  from `BatterySpec` for `LgResu10h` holds `capacity_j`, `max_charge_w`
  and `max_discharge_w` equal, on bits, to the spec's `uom` methods, and a
  Python `Battery` built from the same catalogue entry holds the same
  values.
- `ev_fuel_economy_converts_to_joules_per_metre`: an `EvConfig` filled
  from a `VehicleSpec` holds `fuel_economy_j_per_m` equal, on bits, to
  `VehicleSpec::fuel_economy_j_per_m()`, which equals the `uom`
  expression of its kWh/mi value (0.3 kWh/mi is 671.08 J/m to the
  printed digits).
- The `resolved` golden product (below) of every golden compares bitwise
  across this commit (`[golden]`): every resolved equipment parameter of
  every home, which `06-runtime-memory.md` RT-6 reports in SI on both
  sides of the commit, is unchanged. A missed or doubled conversion (a
  factor of 1000 or 3.6e6) fails it; a stored unit that changed while the
  value did not passes. A parameter that differs in its last bits is
  recorded with the conversion pair that explains it, as the frame guard
  below records columns.
- `numeric_changes_name_existing_goldens` (`05-python-surface.md` PY-9)
  passes with the line this commit adds to `docs/python.md`'s "Numeric
  changes" list (`plan.md` §2.8): the equipment config fields moved to SI,
  and the goldens re-captured, by name.

**Acceptance test, commit 3:**
- `checkpoint_version_9_is_rejected`: a version 9 checkpoint written by
  the test (the `format_version` field set to 9 on a saved checkpoint of
  `der_pv_battery_900s`'s inputs at its step 96) fails `load_checkpoint`
  naming versions 9 and 10.
- `billing_totals_accumulate_in_si`: `month_boundary_billing_3600s`'s
  inputs with its tariff, run to its end: each closed
  `BillingPeriodSummary`'s `total_import_kwh` equals, on bits, the
  period's `cumulative_import_j` at close converted once with `uom`
  (`Energy::new::<joule>(x).get::<kilowatt_hour>()`), and its
  `peak_demand_kw` equals the period's `peak_demand_w` converted once.
- `metrics_totals_accumulate_in_si`: the same run with `metrics = true`:
  `RunMetrics.total_electric_energy_kwh` equals, on bits, the calculator's
  `total_electric_energy_j` converted once.
- `report_column_converts_once_from_si`: `der_pv_battery_900s`'s inputs,
  every step: the `Total Electric Power (kW)` cell equals, on bits,
  `StepResult.net_electric_power_w` converted to kW with `uom`.
- `scheduled_load_gas_rate_enters_in_watts`: a scheduled load built in the
  test with `gas_constant_w = 2930.711` (0.1 therm/h) reports a gas power
  of 2930.711 W in every step, and a spec carrying `gas_constant` fails
  naming `ScheduledLoad.gas_constant_w (W)`.
- `pv_sizing_returns_watts` (Python): `hares.size_pv_system` over the
  usable area `hares.compute_usable_area` returns for `base-pv.xml`'s roof
  planes, with `target_w=6000.0`, returns a `capacity_w` equal to 1000
  times the `capacity_kw` the parent commit returns for `target_kw=6.0`, to
  1e-9 relative (recorded in the test as a constant); a call with
  `target_kw=` fails naming `target_w`; the parent's positional forms
  `size_pv_system(usable, 6.0)` and `required_main_panel_ampacity(6.0)`
  raise `TypeError`; `Dwelling.estimate_pv_capacity(target_w=6000.0)` on
  the same dwelling returns the same `capacity_w`, its positional form
  raises `TypeError`, and `target_kw=` names `target_w`.
- `test_tariff_evaluator_step_takes_watts` (Python,
  `tests/python/test_tariff_evaluator.py`): `TariffEvaluator.step(net_power_w=2000.0,
  dt_seconds=3600.0, current_time=...)` on the file's flat test tariff
  accumulates an import of 7.2e6 J (2 kWh, priced at the tariff's per-kWh
  rate); `step(2.0, 3600.0, ...)` raises `TypeError` and `net_power_kw=`
  names `net_power_w`.
- `observation_name_kw_names_its_replacement`: `to_observation_vec(&["total_power_kw"])`
  fails naming `total_power_w`.
- `test_helics_publications_are_si_with_their_units` (Python,
  `tests/python/test_helics_dwelling.py`, with that file's in-process
  HELICS fake): `der_pv_battery_900s`'s inputs, 4 steps through
  `HELICSDwelling`: each step's `total_power_w`, `reactive_power_var`,
  `equipment_{i}/power_w` and `pv_generation_w` publications equal, on
  bits, the dwelling's telemetry values for that step, and their
  `set_info` is `units=W` or `units=VAR`; the same holds for
  `HELICSFleet`'s aggregate publications (`test_helics_fleet.py`) over two
  homes of `fleet_same_offset_300s`.
- `test_gym_observation_bounds_are_in_watts` (Python,
  `tests/python/test_gym_env.py`): the default observation space's
  `total_power_w` bounds are (-100 000, 100 000), `reactive_power_var`'s
  the same and each equipment `power_w` (0, 100 000), and one episode of
  `der_pv_battery_900s`'s inputs keeps every observation inside its
  `Box`; the built-in reward equals minus the step's net power in kW.
- `observer_fields_hold_their_named_unit` (`observe` feature): on
  `der_pv_battery_900s`'s inputs at step 10, every equipment's
  `electrical_load_w` equals its port's `load_power_w` on bits (at the
  parent the field was named `electrical_load_kw` while holding W).
- `therm_and_btu_per_hour_units_keep_the_parent_factors`:
  `Energy::new::<therm>(1.0).get::<joule>()` equals
  `Energy::new::<btu_it>(100_000.0).get::<joule>()`, the parent's
  definition, within 1 ULP, and
  `Power::new::<btu_it_per_hour>(1.0).get::<watt>()` equals `0.29307107`
  on bits.
- `numeric_changes_name_existing_goldens` passes with this commit's line,
  which also states that HARES's therm and `btu_it_per_hour` are 1.4e-7
  relative larger than OpenStudio-HPXML v1.12.0's (14.738 J per therm),
  kept for this commit's bitwise neutrality, and that adopting the exact
  OpenStudio-HPXML constant is a numeric change of its own commit.

**Acceptance test, commit 4:**
- `removed_actor_parameter_names_its_replacement`, parametrized over every
  `ActorParameter` row: `add_actor_by_name` with the old parameter set to
  1.0 fails naming the row's replacement.
- `removed_control_signal_keyword_names_its_replacement` (Python),
  parametrized over every `ControlSignal` row: each of the three Python
  surfaces (`ControlSignal`, `DispatchRequest`, `Signal`) called with the
  old keyword, and `ControlSignal.from_dict` given the old key, fails
  naming the replacement; each surface's parent positional form
  (`ControlSignal.power_setpoint(5.0)`, `.power_limit(5.0)`,
  `.reactive_setpoint(1.0)`, `.ev_drive(1.0)`, `.ev_away_charge(1.0)`,
  `DispatchRequest.power_limit("ev", 5.0)`, `Signal.power_limit(5.0)`)
  raises `TypeError`; and the HELICS signal body
  `{"type": "PowerSetpoint", "active_power_kw": 1.0}` fails naming
  `active_power_w`.
- `protocol_bridge_old_signal_key_names_its_replacement`
  (`crates/hares-equipment/src/protocol_bridge/handler.rs` tests): the
  command `{"target": "Battery", "signal": {"PowerSetpoint":
  {"active_power_kw": 1.0}}}` fails naming `PowerSetpoint.active_power_w
  (W)`, and the same with `active_power_w` applies 1.0 W.
- `ev_driver_defaults_are_the_documented_values`: the constants of
  `ev_driver::defaults` equal 30 mi, 30 mph, 20 mi and 6.6 kW converted
  with `uom`, and `add_actor_by_name("EvDriver", ...)` with no
  parameters builds a driver holding them.
- `power_limit_action_requires_a_unit`: `parse_dr_action("PowerLimit:5")`
  fails naming `PowerLimit:<max>W`, and `PowerLimit:5000W` limits to
  5000 W.
- `actor_columns_keep_their_reporting_unit`: `der_pv_battery_900s`'s
  inputs: every step's `actor:<bms name>:bms_pv_actual_kw` cell equals, on
  bits, the BMS actor's held SI value converted to kW with `uom`.
- `actor_blob_of_an_older_schema_is_rejected`: a BMS actor blob saved with
  the previous `schema_version` fails `load_checkpoint` naming the actor
  and both versions.
- `si_census` (verification step of commits 2, 3 and 4, run from the
  repository root):

  ```
  P='\b[a-z0-9_]*_(kw|kwh|kva|kvar|kvarh|mi|miles|mph)(_[a-z0-9_]+)?\b|\b[a-z0-9_]*(heating|cooling|electric|ventilation|solar|conduction|mass|gains|lwr|infiltration|loss)_wh\b|\b[a-z0-9_]*therms(_[a-z0-9_]+)?\b'
  git grep -niE "$P" -- ':(glob)crates/*/src/**' ':(glob)python/hares/**' ':(glob)notebooks/**' ':(glob)benches/**' > target/si-census.txt
  grep -qE '^crates/hares-types/src/telemetry_keys\.rs:[0-9]+:pub const ELECTRIC_KW: &str = "electric_kw";$' target/si-census.txt
  grep -qE '^python/hares/.+:[0-9]+:' target/si-census.txt
  grep -qE '^notebooks/simulation_explorer\.py:[0-9]+:' target/si-census.txt
  grep -qE '^benches/output_row_plan\.rs:[0-9]+:' target/si-census.txt
  wc -l < target/si-census.txt
  ```

  The pathspecs are `glob` magic because a plain `crates/*/src` matches
  only a file named `src` (it printed nothing at `afa9a071`). The four
  `grep -q` lines are the self-check, one witness per pathspec, each a
  line known to stay (a reporting name in each tree): `git grep` exits 0
  on a pathspec that matches nothing, so without a witness per pathspec a
  dead pathspec (for example `python/hares/**` before PY-7 renames the
  package, or either new directory if it is ever emptied) passes the step
  silently instead of failing it. The pattern matches the unit as an
  identifier segment, case-insensitively (constants included); watt-hours
  only as the energy totals that carry them
  (`envelope_*_wh`, `hvac_*_wh`, `total_heating_wh` and the like, 82
  lines at `afa9a071` that no other alternative matches), because `_wh`
  alone is also the water-heater abbreviation (`gas_wh`, `heat_pump_wh`,
  `rebuild_wh_typed_config`: 192 lines a bare `wh` alternative matched
  only through it); and therms only in the plural (145 lines), because `e_therm` and
  `burner_w_therm` are thermal. At `afa9a071`, over `python/ochre_next`,
  it prints 8 857 lines in 132 files. The step records the count and the
  file, and the entry's reviewer classifies every line against
  "Boundaries that keep their units", the `REMOVED_KEYS` rows and the
  removed-key tests' inputs; after commit 4 a line in no class is a
  finding. `si_guard` (commit 2) enforces the Rust half permanently.
- `numeric_changes_name_existing_goldens` passes with this commit's line.

**Verification, every commit:** `cargo nextest run --workspace`; `uv run --no-sync pytest
tests/python`; `make check-si-guard` and `si_census` (commits 2 to 4);
`frame_golden compare-all` per M-1's numeric entry
procedure, with the guard below. Commit 3, which renames the observation
keys `benches/rl_episode.rs` reads, also runs `cargo bench --no-run` (no
`[[bench]]` target is built by `cargo nextest run --workspace`), so the
bench fails to compile instead of only failing when someone runs it by
hand. Every golden reaches at least one renamed field:

| Golden | Renamed fields its homes reach |
|---|---|
| `full_year_hourly`, `month_boundary_billing_3600s` (cz2a) | central AC `crankcase_heater_w`; scheduled and event-based loads |
| `full_year_900s_dst`, `leap_year_february_900s`, `weather_wrap_900s` (cz4a) | heat-pump cooler `crankcase_heater_w`; loads |
| `consumer_shape_900s`, `fleet_same_offset_300s` (`bldg0176227`) | PV `capacity_w` (`MaxPowerOutput` 7000 W); central AC; loads |
| `fleet_mixed_offset` | `bldg0176227` as above; room AC (`bldg0000002`); heat pump (`bldg0000004`) |
| `resstock_2025_fleet` | PV (`bldg0352714`, 11000 W); room AC (`bldg0019694`); loads |
| `der_pv_battery_900s` | PV, battery `capacity_j`, `max_charge_w`, `max_discharge_w`; loads |
| `der_pv_battery_ev_900s` | PV, battery, EV `capacity_j`, `max_charging_power_w` (the plug-load EV leaves `fuel_economy_j_per_m` unset, `resolve_loads.rs:902`, so the fuel economy is reached only through `equipment_config_values_are_unit_invariant`); loads |

The guard: the `resolved` product compares bitwise and
`equipment_config_values_are_unit_invariant` passes, so every value is
unchanged in SI. A golden may then move only where a value that
entered in kW or kWh now passes through a conversion pair that is not
exact in f64 (the boundary's `x * 1000.0` followed by an equipment's
`/ 1000.0` back to kW, with `x * 1000.0 / 1000.0 != x`); a value that
enters in W and is divided once, as every PV `MaxPowerOutput` above is
(7000, 11000, 4000 and 1500 W divide exactly), moves nothing. Expected:
every golden compares bitwise. The entry records, per golden, `identical`
or the moved columns with the value and conversion pair that moved them;
a golden that moves without such a value, or by more than that pair
explains, stops the entry.

The guard for commits 3 and 4: every golden reaches the run state (every
home has an electrical summary, reports and billing or metrics);
`der_pv_battery_900s` reaches the BMS actor and its control signals and
`der_pv_battery_ev_900s` reaches the BMS and EV-driver actors and theirs
(`month_boundary_billing_3600s`, whose cz2a building has no PV, battery or
EV, reaches the billing state only). A value now accumulated in J or
computed in W and converted once at the boundary can differ from the kW or
kWh value in its last bits, so a column may move. A difference is measured
against the column's scale, never cell by cell, because a column driven
towards zero (net grid power under `BmsMode::SelfConsumption`) holds
cells near 1e-12 kW whose last-bit change is a relative difference near 1:
for column `c` with parent values `a_t` and values `b_t` after, the
scaled difference is `max_t |a_t - b_t| / max_t |a_t|` (0 when both
columns are all zero), and the scale ratio is `max_t |b_t| / max_t |a_t|`.
Each moved column is recorded with both; a scaled difference above 1e-9,
or a scale ratio within 1e-6 of a factor a missed or doubled conversion
explains (1000, 3600, 3.6e6 or the therm's 105 505 600 J, or their
inverses), stops the entry. Where a last-bit difference flips
a discrete decision (a mode, a dispatch threshold) and a column then moves
by more, the entry records the first step and the comparison that flipped,
and its reviewer accepts it only on that evidence.

**Fixtures added by this entry:** no golden home carries a battery, an EV
or a generator (`consumer_shape_900s`, `fleet_mixed_offset`,
`fleet_same_offset_300s` and `resstock_2025_fleet` carry PV), so the
entry's first commit, which changes no engine code, captures on the
unfixed engine:
- `der_pv_battery_900s`: kind `dwelling`,
  `tests/fixtures/hpxml/ochre_samples/base-pv-battery.xml` with the
  override `{"Battery": {"bms_mode": "{\"SelfConsumption\": {\"min_soc\":
  0.1, \"max_soc\": 0.95, \"solar_only_charging\": false}}"}}` (a declared
  control, `11-other-defects.md` OD-8; self-consumption cycles the battery,
  which `Manual` never does), `data/examples/BEopt_example_schedule.csv`,
  `data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw`, start
  `2023-06-01T00:00:00-07:00`, 7 d at 900 s;
- `der_pv_battery_ev_900s`: kind `dwelling`,
  `tests/fixtures/parity/cz6b_pv_battery_ev/` (`building.xml`, whose EV is
  a `PlugLoad` of type "electric vehicle charging" built into `EvConfig`
  by `resolve_loads.rs:867-918`; `schedule.csv`; `weather.epw`), the same
  battery override, start `2023-06-01T00:00:00-07:00`, 7 d at 900 s;
- both with `master_seed = 0`, the verbosity-2 selection and `warmup`
  omitted (the default);
- a `resolved` product for every golden manifest, captured with the
  others. `frame_golden` (`tools/frame_golden`, whose manifest at
  `afa9a071` captures no resolved model, `manifest.rs:44-75`) gains it in
  this commit, a tool change under `02-measurement.md` M-1's schema change
  rule (every other product of every golden compares unchanged): for each
  home, `DwellingBlueprint::validate()` (`12-inputs.md` IN-2) on the
  `DwellingConfig` the adapter builds for that home, through the one
  adapter function the dwelling and fleet paths both call, so a fleet
  home's product is the model of the config the fleet runs; and, per
  equipment in model order, its name and each `ResolvedParameter`'s name,
  `Unit::symbol()` and value as the 16-digit hex of `f64::to_bits`, written
  as JSON by `frame_golden`'s own writer (`tools/frame_golden/src/resolved.rs`,
  new) over the model's public fields, so `ResolvedModel` gains no serde
  derive. The product is small and stored in full, like the metrics
  (`dl-i08-architecture.golden_storage`). `compare` compares it value by
  value and `delta` names each differing parameter.

From this commit on `[golden]` compares the `resolved` product of every
golden, so every later row that moves a resolved equipment parameter
re-captures it in its own commit through M-1's numeric entry procedure,
which captures every product, and records which parameters moved; a row
that is not numeric may not move it (`plan.md` §3 rule 1). The later
rows known to touch the resolved model: `12-inputs.md` IN-9 (row 94d,
the HPXML vehicles' EV parameters), `11-other-defects.md` OD-2 site 6
(row 94e) and OD-10's numeric commit (row 94f), which change setpoint
sources and infiltration (outside `equipment`, so their product compares
unchanged unless an equipment parameter moves), this entry's commits 3
and 4 (no parameter moves), and B-17 when it runs (it adds
`daily_hot_water_m3` to the wet appliances: a product schema change under
M-1's rule).

**Commits:**
1. `test: capture golden PV, battery and EV dwellings and the resolved models of every golden` (row 94c)
2. `fix: store equipment power, energy and apparent-power config fields in SI units` (row 94c)
3. `fix: hold billing, metrics, equipment state and per-step results in SI units` (row 94g)
4. `fix: carry actor parameters, control signals and actor telemetry in SI units` (row 94h)

**Dependencies:** `12-inputs.md` IN-6 (`REMOVED_KEYS`), IN-3 (the input
schema's units, its `OverrideClassSchema` class-to-config mapping and the
declared HPXML elements) and IN-2 (`validate()`); `11-other-defects.md`
OD-2 (strict overrides, the site 7 check) and OD-8 (the fixtures' declared
battery control); `05-python-surface.md` PY-2 (the Python removed-key
lookup), PY-7 (the package paths), PY-9 (the step dict and `docs/python.md`);
`06-runtime-memory.md` RT-6 (the conversions it deletes and the read-back
the guard compares) and RT-7 (the observation plan whose names commit 3
renames); `03-output-reporting.md` OR-1 (the declared reporting units the
row plan converts to), OR-10 (the checkpoint sections and version 9 that
commit 3 moves to version 10); `02-measurement.md` M-1 (the
schema change rule the `resolved` product follows); `04-fleet.md` FL-3
(the `SteppableFleet` step results). In the numeric block commit 2 follows `12-inputs.md` IN-8
and lands directly before IN-9, which writes the EV config's SI fields;
commits 3 and 4 follow `11-other-defects.md` OD-10's numeric commit and
precede `08-consumer.md` CN-3, which resolves the consumer's contract
drift: its `hares_contract.py` lists `PvConfig.capacity_kw`,
`BatteryConfig`'s and `EvConfig`'s kW fields, `der_inject.py` writes
`surplus_deadband_kw`, `ami2/dr_actuation.py:257` reads the step dict
key `net_electric_power_kw`, and `cosim.py:81` and `coupling.py:393` read
the telemetry field `reactive_power_kvar`.

**Measurement:** for each numeric commit, the per-golden record above,
`der_pv_battery_900s` and `der_pv_battery_ev_900s` included, and the delta
on the EV dwelling of `crates/hares-core/tests/ev_thermal_regressions.rs`'s
`nightly_strategy_keeps_ev_charged_through_a_sustained_cold_snap`
(`bldg0000007` with its injected `ElectricVehicles` element, from
2018-01-01T00:00:00-10:00 for 45 d), before and after, per variable, in
`02-measurement.md`.

**State:** commits 1 and 2 committed on branch `row/r94c-b14` (worktree
`r94c-b14`, based on `ff39b24b`; commit 1 `a776983b`, commit 2 the
branch's next commit). Commits 3 and 4 (rows 94g and 94h) are landed on
branch `row/r94gh` (worktree `r94gh`): commit 3 `51152c52`, commit 4 the
branch's next commit - their records close this entry's State.
Adaptations to this base, recorded: the entry's evidence cites `21d90588`
and `afa9a071` and this base is `ff39b24b`, so the cited line numbers
drifted (PvArray `array_config.rs:100`, PvArraySpec `:155`,
`HeatPumpCoolerConfig`'s crankcase heater `:1399`,
`EquipmentConfig.zone_capacitance` `:338`, the BMS actor's fill
`actor_registry.rs:513`) while every field set matched. (a) Commit 1
adds one read-only accessor, `Fleet::home_configs()` (hares-fleet): the
ResStock constructor builds the homes' configs internally, so the
resolved product cannot otherwise be the model of the config the fleet
runs; it changes no number. (b) The entry's table lists `PvArray` beside
`PvArraySpec`; at this base the config element is `PvArraySpec`
(renamed, SI) and `PvArray` is the runtime struct whose `capacity_kw` is
the construction-time kW working value commit 3 deletes, so it keeps its
name. (c) The shared raw keys' rows are added under `WetAppliance.` as
well as `EventBasedLoad.` (the wet appliances read the same keys; the
entry names two reading types only). (d) The `si_guard` walks Rust files
(`syn`); `notebooks/simulation_explorer.py` is covered by the census
step's own witness grep, not the guard's file walk.

Commit 1 (the capture): the two DER fixtures per the Fixtures section,
captured on the unfixed engine at `ff39b24b` with the resolved product
gained by `frame_golden` (per home, through `DwellingBlueprint::validate`
on the config the home runs: the dwelling path validates the same
blueprint it builds, the fleet paths validate each home's config before
the fleet consumes them); every manifest's committed golden re-captured
with it, twice byte-identical; compare-all identical under both feature
sets. The resolved products captured: the DER dwellings carry the
battery's `energy_capacity_j`/`power_capacity_w`, the PVs' `nameplate_w`,
the EV's `battery_capacity_j`/`max_charging_power_w` (the plug-load EV's
250-mile option at 2.925e8 J and 11500 W), the HVAC's rated capacities,
the loads' `active_power_w` and the ventilation fans' powers.

Commit 2 (the fix): every field in the commit-2 table renamed and stored
in SI, every boundary converting once with `uom` (the HPXML resolvers: a
W value enters with no conversion, a kW or kWh document value converts
where read, `child_energy_j` replacing `child_energy_kwh`; the schedule
resolver's raw-key writes; the vehicle-mapping loader;
`SyntheticEventLoadConfig`'s `active_power_w`; the catalogue fills
through `BatterySpec::capacity_j()`/`max_charge_w()`/`max_discharge_w()`
and `VehicleSpec::capacity_j()`/`charger_power_w()`/
`fuel_economy_j_per_m()`; the BMS actor's payload fill and the EV
driver's and BMS actor's payload reads, one conversion at each read),
the equipment's kW working values converted once at construction, the
read-back reading the retained SI config values directly (the runtime
mirrors `capacity_w`, `capacity_j`/`max_charge_w`,
`battery_capacity_j`/`max_charging_power_w`/`fuel_economy_j_per_m`,
`rated_power_w`, `configured_active_power_w`, so the resolved product
does not pay a conversion pair), the hand-written read-back conversions
(`kwh_to_j`, `kw_to_w`) deleted, the raw EV payload path deleted with
its constants and helpers (a typed config only; the placeholder holds
the typed defaults), the override merge walking every layer's keys
against `REMOVED_KEYS` (the wildcard and own layers, payload strings
parsed and pathed under their variant, array rows under `[]`; the
pattern rows matching `<n>` against a run of ASCII digits), the
synthetic `[event_load]` table checked before its deserialisation, the
HPXML extension element renamed with `active_power_kw` added to the
rejected elements, the Python constructors keyword-only with
`**unexpected` catch-alls raising the removed-key error,
`OD-2` site 7's range check at 19200 W, `register_pv_roof_shading`
reading `capacity_w` and dropping its own factor, `DescribeFields`
giving the renamed fields their SI units (`Unit` gains `VA`, `J/m`,
`J/K`, `W/s`), `si_guard` widened with the unit-suffix rule (123 files
allowlisted, every entry marked with the boundary it holds or the commit
that removes it), and the docs. RED first: the
commit-1 captures are the witness, the fix's movement measured against
them before re-capture (below). GREEN: re-captured twice byte-identical;
compare-all 10 identical (default features) + 1 (dst).

The movement (the RED witness, per golden, every difference last-bit):
`der_pv_battery_900s`: the Clothes Washer's and Other End Use's electric
power, 4 cells each (the event load's kW working value through the
resolver's x1000 and the load's /1000 pair); `der_pv_battery_ev_900s`:
the EV's power and end use (3 cells), the BMS actor's `load_kw` (2), the
EV driver's `charge_kw` (3) and `soc` (120), the totals, and the
metrics' renewable fraction, gross consumption, net energy and EV end
use (last bits); `consumer_shape_900s`: the total electric (6695 cells),
total and water-heater gas power (5), total reactive (10781), the
refrigerator's and refrigeration end use's power, one indoor and 1779
zone-2 temperature cells, four metrics (last bits); `fleet_mixed_offset`
(11 differences) and `fleet_same_offset_300s`: the aggregates' power and
temperature columns and the homes' energies (last bits);
`full_year_hourly` and `full_year_900s_dst`: the total electric and
reactive, the ASHP heater, the loads' powers, the end-use sums, one
indoor temperature cell; `leap_year_february_900s`,
`weather_wrap_900s`, `month_boundary_billing_3600s` and
`resstock_2025_fleet` compare bitwise. The resolved products of every
golden compare bitwise (the guard's expectation: every resolved
equipment parameter unchanged). The pairs that explain the movement: the
schedule resolver's kW-to-W write and the loads' W-to-kW construction
read; the EV's fuel economy (0.325 kWh/mi is not binary-exact in J/m,
and the round trip moves its last bit); each propagated difference is
the one-ulp tail of those through the physics and the folds. The 45-day
EV cold-snap run (the Measurement section's EV dwelling), before and
after, per variable: `EV SOC (-)` and `actor:EvDriver:EV:soc` 304 cells
at one f64 ulp (2.2e-16 absolute); `Total Electric Power (kW)`,
`EV Electric Power (kW)`, `EV End Use Electric Power (kW)`,
`EV Energy (kWh)` and `actor:EvDriver:EV:charge_kw` 4 cells each, max
relative difference 6.6e-15; the first differing rows at 226 to 288
recorded above in `frame_golden`'s diff. No scaled difference above
1e-9 and no scale ratio near a unit factor anywhere: the movement is
the conversion pairs.

Verification (both commits): fmt clean; clippy `--workspace
--all-targets --all-features -D warnings` clean; `cargo nextest run
--workspace --all-features -j 2` all passing; `pytest -m ''` all passing
(0 failed, 0 errors, the 6 xfails as expected); `uv run pyright` 0; the
acceptance tests of this entry's commit-2 list added and passing
(`equipment_config_fields_are_si`,
`input_schema_override_classes_are_si`,
`raw_equipment_keys_are_si` (both greps print nothing), `si_guard`,
`removed_equipment_key_names_its_replacement` (all 33 rows, own key and
wildcard), `si_payload_keys_merge_cleanly`,
`synthetic_event_load_removed_key_names_its_replacement`,
`hpxml_active_power_kw_extension_is_rejected`,
`pv_capacity_kw_override_fails_construction`,
`test_positional_calls_raise_type_error`,
`test_removed_keyword_names_replacement` (Python),
`equipment_config_values_unit_invariant` (the PV, battery, crankcase,
zone-capacitance, plug-load EV and payload cases),
`hpxml_pv_capacity_enters_in_watts`,
`battery_catalogue_fill_converts_with_uom`,
`ev_fuel_economy_converts_to_joules_per_metre`),
`numeric_changes_name_existing_goldens` passing with this commit's
line; `si_census` 8715 lines at this commit with all four witnesses.

Commit 3 (the run-state fix, row 94g): landed as `51152c52` on branch
`row/r94gh` (worktree `r94gh`), the branch that now carries the entry's
four commits rebased under rows 94e and 94f (commit 1 `d12a8c38`, commit
2 `5958ef72`, commit 3 the branch's tip before commit 4).
`CHECKPOINT_VERSION` rose from 19 to 20 (the actual numbers at this
base; the entry's 9 to 10 is the plan's stale numbering) with
`PINNED_SCHEMA_SHA256` re-pinned in the same commit. RED first (the
pre-fix measurement against the parent `6a5d9769`'s frames): the
per-golden movement in `02-measurement.md` M-6, every difference last
bits except the gas columns' one therm-factor relative (1.3969e-7), the
BMS and EV-driver actor columns' first-caught 1000x double conversion
fixed in the commit by converting each actor's declared-kW telemetry to
W once at the row write. GREEN: re-captured twice byte-identical;
compare-all 10 identical (default features) + 1 (dst).

Commit 4 (the control-layer fix, row 94h): landed as the branch's next
commit (worktree `r94gh`). Every actor parameter, control-signal field,
actor state and actor telemetry value is held in SI under its SI name
(the entry's Commit-4 list); the removed-key surfaces are `REMOVED_KEYS`'
`ActorParameter` rows (checked by `ActorRegistry::create` before the
factory runs) and `ControlSignal` rows (checked by the Python
constructors' `**kwargs` catch-alls, `ControlSignal.from_dict`, the
gym's Rust action reader and the protocol bridge's handler before serde
parses); the DR action string takes `PowerLimit:<max>W`; the EV driver's
documented literals are `ev_driver::defaults`' named SI constants; the
`Actor` trait's PV argument is `pv_w`; the demand-response compliance
actor's snapshot version rose to 4 (its dispatched `PowerLimit` payloads
hold W where v3 held kW; the refusal sits at the per-actor
`schema_version` gate) and the EV driver's at 4 (v3's kWh energies would
read 3.6e6x small). `CHECKPOINT_VERSION` stays 20 (the run-state commit
bumped it; the control layer changes no `DwellingCheckpoint` field).
Adaptations recorded: (e) the census step's benches witness moved from
`rl_episode.rs` (whose last kW-named observation key the control-layer
commit renamed to `reactive_power_var`, emptying the bench tree of
census lines) to `benches/output_row_plan.rs`'s declared reporting
column, keeping the dead-pathspec self-check; (f) the gym's observation
resolver accepts the run-state commit's `reactive_power_var` and deletes
the `total_electric_kw` alias (the renamed-key error names
`total_power_w`), the observation bounds table takes the new keys at the
W/VAR scale, and `py_gym.rs`/`gym_env.py` read the renamed action keys.
RED first (the fix's frames against commit 3's goldens, M-6b in
`02-measurement.md`): ten goldens bitwise, `der_pv_battery_ev_900s`'s EV
columns in the last bits (max relative 2.3e-14; the fuel economy's J/m
pair), the 45-day cold-snap run's SOC at 1 to 2 ulps and its power cells
at 6.4e-15, nothing above 1e-9 scaled and no scale ratio near a unit
factor. GREEN: re-captured twice byte-identical (the ten non-dst
goldens, and `full_year_900s_dst` on the dst build); compare-all 10
identical (default features) + 1 (dst). Gates: fmt clean; clippy
`--workspace --all-targets --all-features -D warnings` clean; nextest
`--workspace --all-features` 8220 passed, 8 skipped; `pytest -m ''`
1798 passed + 6 xfailed, 0 failed, 0 errors; pyright 0; the acceptance
tests of the commit-4 list added and passing
(`removed_actor_parameter_names_its_replacement` (all 10 rows),
`removed_control_signal_keyword_names_its_replacement` (Python, all 8
rows on all three surfaces),
`protocol_bridge_old_signal_key_names_its_replacement`,
`ev_driver_defaults_are_the_documented_values`,
`power_limit_action_requires_a_unit`,
`actor_columns_keep_their_reporting_unit`,
`actor_blob_of_an_older_schema_is_rejected`); `si_census` 2987 lines at
this commit with the four witnesses.

## B-15: OpenStudio-HPXML unavailable periods are applied

**Kind:** numeric fix (`dl-i08-architecture.unavailable_periods`: applied
with OpenStudio-HPXML's semantics; HARES accepts nothing it ignores; one
numeric entry after the consumer migration, with golden re-capture and the
physics suites before and after).

**What OpenStudio-HPXML does** (v1.12.0; paths under
`HPXMLtoOpenStudio/` unless stated):
- The input is the HPXML element
  `/HPXML/SoftwareInfo/extension/UnavailablePeriods/UnavailablePeriod`
  (`docs/source/workflow_inputs.rst:282-312`; class `resources/hpxml.rb:1365,
  1387-1448`): `ColumnName` (required), `BeginMonth`, `BeginDayOfMonth`
  (required), `BeginHourOfDay` (0 to 23, default 0), `EndMonth`,
  `EndDayOfMonth` (required), `EndHourOfDay` (1 to 24, default 24),
  `NaturalVentilation` (`regular schedule` (default), `always available`,
  `always unavailable`); defaults at `resources/defaults.rb:229-241`, date
  checks at `hpxml.rb:1410`.
- Four types: `Vacancy`, `Power Outage`, `No Space Heating`,
  `No Space Cooling`. Which end uses each zeroes is the table
  `resources/data/unavailable_periods.csv:1-40` (1 zeroes the row's
  schedule). In summary: `Vacancy` zeroes occupants, lighting, cooking,
  the wet appliances and dryer, ceiling fans, plug and fuel loads, pools
  and spas, hot-water fixtures and recirculation, general water use,
  kitchen and bath fans and EV charging; `Power Outage` zeroes the same
  less occupants and general water use, plus refrigerators and freezers,
  space heating and cooling, the water heater, the dehumidifier,
  mechanical ventilation and battery charging and discharging;
  `No Space Heating` zeroes space heating; `No Space Cooling` zeroes space
  cooling. PV and generators are in no row and keep running.
- A `ColumnName` that is not a column of the table fails the run
  (`resources/schedules.rb:807-838`, `fail` at `:815`).
- A period covers the half-open interval from `BeginMonth/BeginDayOfMonth`
  at `BeginHourOfDay` to `EndMonth/EndDayOfMonth` at `EndHourOfDay`, both
  dates inclusive as days; an end before the begin wraps through the year
  end (`schedules.rb:702-724`, timestep form `:1375-1408`).
- Space heating and cooling: every system's heating (cooling) load
  fraction is 0 during the period; the thermostat setpoints are not
  changed (`resources/hvac.rb:42-71, 4536-4573`; `schedules.rb:1438-1441,
  1455`). The water heater's setpoint drops to 2 C (freeze protection),
  not off, so standby losses continue (`schedules.rb:673-676, 1452-1454`;
  `resources/waterheater.rb:18, 183, 188, 2053`).
- `NaturalVentilation` applies to every period whatever its type
  (`resources/airflow.rb:337, 570-597`).
- Design levels are computed from the schedules before the periods are
  applied (`schedules.rb:1242, 1296`), so a period removes energy rather
  than being renormalised away.
- A detailed schedule CSV is not an input channel for periods
  (`SchedulesFile::Columns`, `schedules.rb:981-1032`, does not list the
  four names). The ResStock `in.schedules.csv` is OpenStudio-HPXML's
  export: `set_unavailable_periods` (`schedules.rb:1421-1462`) appends one
  0 or 1 column per type, set from the HPXML periods, and zeroes every
  affected detailed column, and `export()` (`:1146-1158`) writes it.

OCHRE ignores all four (`vendors/OCHRE/ochre/utils/schedule.py:57-69,
465-467`; `ochre/utils/base.py:110-111` keeps only `BuildingDetails`).

**Problem:** HARES accepts unavailable periods and applies none. The
HPXML element is never read (the parser starts at
`Building/BuildingDetails`, `crates/hares-io/src/hpxml/building.rs:469`
at `afa9a071`), and the four schedule columns are `NotUsed` with the reason
"unavailable periods are not modelled" (`03-output-reporting.md` OR-11,
`crates/hares-io/src/schedule_resolve.rs:204-260` at `b8195c83`). So a home
with `No Space Heating` heats through it, and a vacant home
(`tests/fixtures/resstock/2025.1/bldg0527060/home.xml:297-308`, `Vacancy`
all year, no schedule file) runs every occupant load. Fixture census at
`afa9a071` (35 040 rows at 900 s each):

| Home (`tests/fixtures/resstock/2025.1/`) | Period (HPXML) | Steps flagged in `in.schedules.csv` |
|---|---|---|
| `bldg0000001` | `No Space Heating`, 11/7 0h to 11/13 24h | 672 (168 h) |
| `bldg0000003` | `No Space Heating`, 1/31 0h to 2/2 24h | 288 (72 h) |
| `bldg0000007` | `No Space Heating`, 1/17 | 96 (24 h) |
| `bldg0176227` | `No Space Heating`, 10/4 0h to 10/4 24h | 96 (24 h, rows 26496 to 26591) |
| `bldg0527060` | `Vacancy`, 1/1 0h to 12/31 24h | no schedule file |

Every other 2025.1 file carries the four columns all zero; the 2024.2
files and `data/examples/bldg0112631_schedule.csv` carry `Vacancy` and
`Power Outage`, all zero. Every period uses `NaturalVentilation` `regular
schedule`. `consumer_shape_900s` (`bldg0176227`, full 2018) contains the
Oct 4 period; `fleet_mixed_offset` (7 days from 2018-01-01) contains none;
the parity goldens have no period.

**Change:**
- `hares-io` reads `SoftwareInfo/extension/UnavailablePeriods` into
  `Building.unavailable_periods: Vec<UnavailablePeriod>` (`kind:
  UnavailableKind { Vacancy, PowerOutage, NoSpaceHeating, NoSpaceCooling }`,
  begin and end month, day and hour, `natural_ventilation:
  NaturalVentilationOption`), with OpenStudio-HPXML's defaults and checks:
  an unknown `ColumnName`, an hour outside its range or a date that does
  not exist in the simulated year fails the parse naming the period. The
  elements join `12-inputs.md` IN-3's `HPXML_ELEMENTS`.
- One static table in `hares-io`, `UNAVAILABLE_PERIOD_ROWS`, is
  `unavailable_periods.csv`'s 40 rows with their four flags, cited to the
  file and tag. Each period becomes a per-step flag on the run's local
  standard-time calendar, the calendar schedule rows use
  (`crates/hares-core/src/environment.rs:418-445`), by the half-open rule
  and the year wrap above.
- Schedule-driven end uses: `inject_schedule_into_specs`
  (`schedule_resolve.rs`) multiplies each schedule column whose table row
  a period applies to by 0 on the flagged steps, for a schedule file and a
  generated schedule alike (`hpxml_schedule.rs`), so the vacant
  `bldg0527060` runs no occupant load. The design level of a column
  (`determine_max_kw`, `schedule_resolve.rs`) is computed from its mean
  over the steps no applying period flags, because a ResStock schedule
  file is already zeroed there and its mean over every step would scale
  the remaining hours up, which OpenStudio-HPXML, computing design levels
  before the periods, does not do; `docs/PHYSICS_DECISIONS.md` states that
  the two agree when the file's unflagged values are the unmodified
  schedule, as in an OpenStudio-HPXML export.
- Equipment end uses: every equipment registry entry declares the table
  row it answers to, or none, through one exhaustive match (heating
  equipment `space_heating`, cooling equipment `space_cooling`, a heat
  pump each by mode, water heaters `water_heater`, refrigerators and
  freezers their rows, the dehumidifier, mechanical ventilation
  `house_fan`, kitchen and bath fans, batteries `battery_charging` and
  `battery_discharging`, EVs `electric_vehicle_charging` and
  `electric_vehicle_discharging`, PV and generators none), so a new class
  must choose. The per-step availability reaches each equipment through
  `EnvironmentState` beside `bus_energized`, and the gates that already
  close on an outage close on it the same way: HVAC duty 0 with the
  setpoint unchanged (`outage_forces_off`,
  `crates/hares-equipment/src/hvac/helpers.rs:161-167`, and its callers),
  a water heater's setpoint 2 C for the step, a battery or EV idle. An
  HPXML `Power Outage` period is OpenStudio-HPXML's end-use convention
  (PV runs, occupants stay); it does not de-energise the bus, and the
  physical outage model (`Dwelling::set_grid_voltage`,
  `docs/outage-behavior.md`) is unchanged.
- Natural ventilation: on a flagged step of any period, `always
  available` and `always unavailable` set the natural-ventilation
  availability, and `regular schedule` leaves it.
- The four schedule columns stop being `NotUsed`: each becomes a check
  column. When a schedule file carries one, its value on every row of the
  file's own resolution (before resampling) must equal the flag the HPXML
  periods give that row, 0 or 1; a difference, including a nonzero column
  with no HPXML period, fails construction naming the column, the first
  differing row and the periods. HARES therefore reads the column and
  ignores nothing.
- `docs/PHYSICS_DECISIONS.md` section: the reference, OCHRE's behaviour,
  the design-level rule, and the delta direction (less heating on the
  flagged days). `tracing` at `debug`: each period's first and last
  flagged step and the equipment and columns it gates.

**Acceptance test:**
- `unavailable_periods_parse_from_software_info`: `bldg0000001/home.xml`
  yields one `NoSpaceHeating` period, 11/7 0h to 11/13 24h, `regular
  schedule`; a copy with `ColumnName` `Holiday` fails naming it and the
  four types.
- `period_flags_are_half_open_and_wrap` (unit, 900 s over 2018): 1/31 0h
  to 2/2 24h flags 288 steps; 3/5 6h to 3/5 18h flags 48; 12/31 12h to
  1/1 12h flags the last 48 and first 48 steps.
- `no_space_heating_stops_heating_only`: `bldg0000003` with its
  `in.schedules.csv` and `tests/fixtures/resstock/2025.1/weather/G2500170_2018.csv`,
  start `2018-01-30T00:00:00-05:00`, 5 d at 900 s: on every step from
  2018-01-31 00:00 to 2018-02-03 00:00 every heating equipment's
  delivered heating is 0 and the heating setpoint column equals the run
  of the same inputs with the period removed from the HPXML and the
  `No Space Heating` column zeroed; on 2018-01-30 and 2018-02-03 heating
  is delivered.
- `vacancy_zeroes_the_occupant_loads`: `bldg0527060/home.xml` with its
  generated schedule and `G5300330_2018.csv`, 2 d at 3600 s: occupants,
  lighting, plug-load, cooking and hot-water fixture power and draw are 0
  on every step; refrigerator and HVAC are not.
- `power_outage_keeps_pv_and_holds_the_water_heater_at_2c` (unit, over a
  built `base-pv.xml` dwelling with a test-made one-day `Power Outage`
  period): PV generates, HVAC and lighting are 0, the water heater's
  setpoint is 2 C on the flagged steps.
- `schedule_flag_column_must_match_the_periods`: `bldg0176227`'s
  schedule with one `No Space Heating` row outside Oct 4 set to 1 fails
  construction naming the column and that row.
- `design_level_ignores_flagged_steps` (unit): a column of 1.0 with half
  its steps zeroed under a `Vacancy` period resolves to the design level
  of the unzeroed column.
- `natural_ventilation_option_applies_to_every_period` (unit).

**Verification:** `cargo nextest run -p hares-io`; `cargo nextest run -p
hares-core`; M-1's numeric entry procedure (`consumer_shape_900s`
re-captured, every golden whose homes have a period in their window
listed from the census at the parent); the physics comparison suites of
`plan.md` §3 rule 2 at the parent and after; the commit's "Numeric
changes" line names the periods and the re-captured goldens.

**Commit:** `fix: apply OpenStudio-HPXML unavailable periods to the end uses they name`

**Dependencies:** `03-output-reporting.md` OR-11 (the `NotUsed` entries
this replaces); `12-inputs.md` IN-3 (`HPXML_ELEMENTS`); `11-other-defects.md`
OD-2 (strict inputs); B-1c (the calendar of the simulated year).

**State:** FIXED (2026-10-18, branch `row/r94ij`, worktree `r94ij`,
based on `6a5d9769`, one commit plus the fixes commit (the
dehumidifier gate, the flag-column trace and the file-order sentence
below); the schedule-side slice the entry's
parser, table, calendar flags, column zeroing and pre-period design
levels already carried at that base, re-verified by this entry's tests).

The commit adds what the entry's remaining surface names:
`hares_types::UnavailablePeriodState` on `EnvironmentState` beside
`bus_energized` (the four kind flags and the natural-ventilation
availability the flagged periods set, the last non-regular option
winning), computed by the environment manager once per construction
over the run's calendar schedule rows and set per step beside
`schedule_row`. The last non-regular option is the periods' file
order: the flags apply period by period in the HPXML document order
the parser returns, so two flagged periods that each carry a
non-regular option leave the later period's availability. The
equipment classes' declaration of the table rows they answer to, one
exhaustive match (`hares-equipment`'s
`unavailable_period_rows`, the multi-instance "Class #N" spelling
through its class), pinned against the reference table by the
cross-check test; and the gates that close on it the same way an
outage closes: HVAC duty 0 with the setpoints unchanged
(`unavailable_period_forces_off` beside `outage_forces_off` at every
HVAC caller, the ideal unit's gate beside its bus gate), the water
heater's control setpoint 2 C for the step (the freeze protection,
`step_setpoint_c` in every water-heater type; the unit test fires a
tank the period caught below 2 C, so the protection is not an off), a
battery idle, an EV idle, mechanical ventilation off under its row,
the dehumidifier off under its row (`zeroes_dehumidifier` beside the
unit's outage gate, the unit's power 0 through the period and the
humidistat control resumed after it), and the schedule- and
profile-driven loads zeroed by the row their class declares (the
profile-driven loads are the point: the columns' zeroing cannot reach
them, which the vacant home's exterior lighting witnessed at the
parent). The four schedule flag columns stop being
`NotUsed`: `check_schedule_flag_columns` runs at the blueprint freeze
on the file's own resolution, and a column that disagrees with the
periods fails construction naming the column, the first differing row
and the periods. `tracing` at debug: each period's first and last
flagged step with the equipment classes and columns it gates, each
gated load's step, and each flag column's verdict.

RED first at the parent: `vacancy_zeroes_the_occupant_loads` failed on
the exterior lighting's 3.1 W (the profile-driven load the column
zeroing cannot reach), `no_space_heating_stops_heating_only` and
`power_outage_keeps_pv_and_holds_the_water_heater_at_2c` failed on
their gates (heating delivered through the period; no freeze setpoint
channel), and `schedule_flag_column_must_match_the_periods` failed on
the check's absence. GREEN at the row; `design_level_ignores_flagged_steps`
and `natural_ventilation_option_applies_to_every_period` pin the
pre-period design level and the availability option. The dehumidifier
gate's probe (`power_outage_stops_the_dehumidifier`) ran RED at the
fixes' parent (the unit drew power on the flagged steps, 0.0797 kW at
the first) and GREEN at the fixes commit.

Movement: no golden moves. The only golden whose home carries a period
in its window is `consumer_shape_900s` (bldg0176227's No Space Heating
day, 2018-10-04): the differential run over the window delivers zero
heating with and without the period (the home does not call for heat
on those steps), so every flagged step is bitwise unchanged and
compare-all is identical on the ten default-feature fixtures and the
dst fixture. No golden re-captured; the delta record is in
`02-measurement.md`.

Gates at the commit: fmt clean; clippy `--workspace --all-targets
--all-features -D warnings` clean; `cargo nextest run --workspace
--all-features -j 2` 8222 passed, 8 skipped, 0 failed; `pytest -m ''`
on the rebuilt wheel 1783 passed, 6 xfailed, 0 failed, 0 errors;
pyright 0 errors, 0 warnings; the goldens bitwise (compare-all 10 +
dst 1).

**Measurement:** per golden, before and after; on `consumer_shape_900s` the
heating energy of 2018-10-04 and the year; the consumer-shaped delta feeds
B1's attribution (`08-consumer.md` CN-4), and CN-3 carries the consumer
side (the four columns are read, so its schedule rotation keeps them at
their calendar rows).

## B-16: wet-appliance hot-water volume, measured against both references

**Kind:** measurement (triage, no commit;
`dl-i08-architecture.wet_appliance_hot_water`: a triage entry measures it
against both references; a confirmed defect becomes its own numeric entry,
B-17).

**Problem:** the clothes washer and dishwasher draw a fixed volume per
cycle, 15 L and 6 L (`crates/hares-io/src/hpxml/resolve_loads.rs:258, 432`
at `afa9a071`), spread over the phases that draw water
(`crates/hares-equipment/src/event_load.rs:1380-1395`: 0.0100 kg/s for the
washer, 0.004545 kg/s for the dishwasher, litres taken as kilograms). On
the HPXML path a cycle is a contiguous non-zero run of the schedule's
power series, replayed with `phase_index` held at 0, a water-drawing phase
(`event_load.rs:1474-1499`), so the volume drawn is the rate times the run
length, which depends on the time step. OpenStudio-HPXML derives a daily
volume from the HPXML appliance inputs (`HPXMLtoOpenStudio/resources/hotwater_appliances.rb:607-675, 824-888`
at v1.12.0) and spreads it with the `hot_water_clothes_washer` and
`hot_water_dishwasher` columns, untempered; OCHRE derives a daily volume
by the same formulas with older cycle equations and occupancy-adjusted
bedrooms (`vendors/OCHRE/ochre/utils/hpxml.py:1243-1278, 1336-1370, 1664`)
and spreads it with the same columns (`ochre/utils/schedule.py:350-370`,
`ochre/Models/Water.py:286-290`).

**Change (measurement):** for each golden home with a wet appliance
(every golden: `bldg0176227`, `bldg0000002`, `bldg0000004` and the two
parity buildings), record per appliance:
- the OpenStudio-HPXML daily volume and the OCHRE daily volume, each
  computed from the home's HPXML with the cited formulas (inputs and
  arithmetic recorded);
- HARES's realised daily volume over the golden's run, the sum of the
  appliance's `DHW_DEMAND_LOOP` flow times the step, read by a probe in a
  scratch worktree of the parent (a `tracing` line at the water heater's
  `read_dhw_demand_kg_s`, `water_heater/mod.rs:82` at `7b7a6a7d`; the probe's patch is
  recorded verbatim in the log and never committed), at the golden's
  step and again at 3600 s and 900 s for one parity home.
Planning-time arithmetic (to be re-measured): `bldg0176227` washer
OpenStudio-HPXML 28.19 L/d, HARES 17.11 L/d at 900 s and 39.35 L/d at
3600 s (39% and 130% off); dishwasher 14.05 L/d against 31.17 and 43.35
(122% and 208% off); the parity homes' washer 9.78 L/d against 27.42
(180% off). Every one of these already exceeds the triage rule below by
more than an order of magnitude, including the 900 s-against-3600 s leg,
which depends on nothing upstream. The recorded run is not a trial of
whether B-17 is needed; it is the confirming measurement the triage rule
requires before B-17 lands, and the planning figures are why B-17 is
scheduled as the expected outcome rather than a coin flip.

**Acceptance (the triage rule):** the defect is confirmed, and B-17 runs,
when any appliance's realised daily volume differs from the
OpenStudio-HPXML volume by more than 1%, or differs between 900 s and
3600 s by more than 1%. Otherwise the log records the measurement as the
reason B-17 does not run, and its row is closed with that record.

**Dependencies:** `03-output-reporting.md` OR-11 (the `NotUsed` reason for
the two columns); B-15 (which zeroes the same columns under a period).

## B-17: wet-appliance hot water follows OpenStudio-HPXML (runs when B-16 confirms the defect)

**Kind:** numeric fix (`dl-i08-architecture.wet_appliance_hot_water`).
Contingent on B-16's rule.

**Problem:** as B-16 records. Found with it, in the same functions:
cycles per year use the pre-Addendum-81 equations with the raw bedroom
count (`resolve_loads.rs:290, 463`), where OpenStudio-HPXML uses
`123 + 61 n_occ` and `91 + 30 n_occ` when `NumberofResidents` is given and
the Addendum 81 asset equations otherwise (`hotwater_appliances.rb:627-637,
842-850`); `extension/AdjustedAnnualkWh` is read as if it were the rated
value and adjusted a second time (`resolve_loads.rs:226-229, 263-297`);
`UsageMultiplier` scales energy and not water; and an appliance whose
schedule file lacks its power column falls back to a constant window
that runs cycles back to back (`parse_event_schedule_sources`), a silent
default.

**Change:**
- `hares-io` computes each appliance's daily hot-water volume with
  OpenStudio-HPXML's `calc_clothes_washer_energy_gpd` and
  `calc_dishwasher_energy_gpd` (both branches, `UsageMultiplier` applied,
  clamped at 0, the defaults of `resources/defaults.rb:5523-5530,
  5586-5594`), and the same cycles-per-year rule for the appliance energy;
  `AdjustedAnnualkWh` is used as the adjusted energy without a second
  adjustment.
- The volume is drawn as OpenStudio-HPXML draws it: spread over the
  `hot_water_clothes_washer` and `hot_water_dishwasher` columns (peak flow
  = daily volume / the column's daily equivalent full-load hours,
  `resources/schedules.rb:1353-1365`), untempered at the supply
  temperature, through the water heater's scheduled draw beside
  `hot_water_fixtures` (`schedule_resolve.rs` `inject_water_heater_schedule_columns`),
  with the volume converted to mass at the drawn water's density. The two
  columns stop being `NotUsed`; the schedule generator produces them for a
  home without a schedule file by reusing the weekday, weekend and monthly
  fractions already generated for `clothes_washer` and `dishwasher`
  (`hotwater_appliances.rb:63-65, 155-157, 480-482, 517-519`: one
  `MonthWeekdayWeekendSchedule` per appliance sizes both its electric
  design level from the daily kWh and, through the same object's
  `calc_design_level_from_daily_gpm`, its water draw from the daily
  gallons), scaled to the appliance's daily water volume instead of its
  daily energy; OpenStudio-HPXML has no separate hot-water profile for
  these appliances. The wet appliance's own `DHW_DEMAND_LOOP` draw and
  its `hot_water_draw_volume_l` key are deleted, so the volume no longer
  depends on the step.
- Under an unavailable period (B-15), `hot_water_clothes_washer` and
  `hot_water_dishwasher` zero on the same steps as `clothes_washer` and
  `dishwasher`: the column-zeroing step looks up each hot-water column's
  applicability through its paired energy-use column's name, the same
  two-step lookup `schedules.rb` `set_unavailable_periods` uses
  (`:1433-1437`, confirmed at `v1.12.0`: the `case` statement remaps
  `HotWaterDishwasher` to `Dishwasher` and `HotWaterClothesWasher` to
  `ClothesWasher` before testing `unavailable_period_applies` on the
  remapped name). Without this, B-15's own fixture `bldg0527060`
  (`Vacancy`, all year, no schedule file) would zero `clothes_washer` and
  `dishwasher` but leave `hot_water_clothes_washer` and
  `hot_water_dishwasher` carrying the normal appliance-schedule shape, so
  a vacant home would keep drawing wash and dishwasher hot water for the
  full vacancy.
- A schedule file without an appliance's power column is a construction
  error naming the column; the constant-window fallback is deleted.
- Each appliance reports `daily_hot_water_m3` as a resolved parameter.
- `docs/PHYSICS_DECISIONS.md` section; `tracing` at `debug`: each
  appliance's inputs, cycles per year and daily volume.

**Acceptance test:**
- `wet_appliance_daily_hot_water_matches_openstudio_hpxml`: the resolved
  `daily_hot_water_m3` of `tests/fixtures/parity/cz4a_ashp_hpwh/building.xml`
  is 2.584 gal/d (washer) and 2.563 gal/d (dishwasher), and of
  `bldg0176227/home.xml` (operational branch) 7.448 and 3.711 gal/d, to the
  printed digits.
- `wet_appliance_hot_water_is_independent_of_the_step`: the parity cz4a
  inputs over 7 d draw the same appliance volume at 900 s and 3600 s, to
  1e-9 relative, equal to 7 times the daily volume weighted by the
  columns' daily sums.
- `missing_appliance_power_column_is_an_error`.
- `adjusted_annual_kwh_is_not_adjusted_twice` (unit).
- `vacant_home_draws_no_appliance_hot_water`: `bldg0527060/home.xml` (B-15's
  `Vacancy`, all-year fixture, no schedule file) run a full year: the
  generated `hot_water_clothes_washer` and `hot_water_dishwasher` columns
  are 0 on every step, and the water heater's realised draw from both
  appliances is 0 for the run.

**Verification:** M-1's numeric entry procedure (every golden re-captured:
each has a washer or dishwasher and a water heater); the physics
comparison suites at the parent and after, `test_ochre_parity.py`
included (OCHRE's older cycle equations are the documented difference);
the "Numeric changes" line.

**Commit:** `fix: draw wet-appliance hot water by the OpenStudio-HPXML daily volume`

**Dependencies:** B-16; B-15; `03-output-reporting.md` OR-11.

**Measurement:** per golden, domestic hot water energy and the appliances'
volumes before and after.

**State:** FOLDED as `c0d39990` on the head (2026-10-08, branch
`row/b17-hw-volume`, fast-forwarded unchanged). The branch carries B-17
and its follow-ups: the daily-volume rule with the one density
convention, the indirect tank's wet-appliance and fixture draws wired
like every other water heater, the 365-day unavailable-period calendar
with volume removal, the negative and fractional unavailable hours as
validation errors, the gas water heater's hot supply temperature from
its config, the multi-system water heater fractions following the
schematron's sum rule, the pre-2019 wet appliances reading only the
label fields their algebra consumes, the all-or-nothing appliance label
defaults, the F-002 flake filing, and the dead `draws_dhw_load`
parameter deleted. The goldens re-captured for the changed volumes;
identical at the fold gates (nextest 7537 at the fold, 7575 at the
final head with the later rows).

## Hardening 1: the defrost penalty is applied by one model

**Kind:** numeric fix (the post-initiative hardening phase's first
equipment entry: the defrost consolidation, with the discrete cycle's
trigger defect and the resistive destination defect fixed in the same
path).

**Problem:** the heat pump's defrost penalty was applied twice. The
discrete frost cycle's tracker is unconditionally active
(`heater.rs:664`, advanced at `:1360`), and while the tracker
Accumulates, the continuous model still multiplies capacity by
`0.875*(1-tf)` and adds `extra_power_w` (`heater.rs:1927-2036` at the
parent); while the tracker Defrosts, the FSM's full-intensity override
zeroes capacity and draws the defrost power. The tracker's own trigger
also under-fired its documented duty: frost accumulates at `dt * tf` per
step (`defrost.rs:300`) but the trigger compared the accumulator against
`cycle_duration_s / ewma_tf` (`defrost.rs:300-302`), so the FSM fired
about `tf^2` of its documented duty (the docstring at `defrost.rs:229-233`
claims the same average time-in-defrost as the continuous model, whose
duty is tf). And the resistive-defrost destination disagreed between the
paths: the discrete event delivered 100 percent of the element's heat to
the zone (`heater.rs:2095-2115`), the continuous path 0 percent
(`defrost.rs:560-565`). Measured at a static operating point (48 h at
10 s steps, 1 C / RH 85 percent): the discrete FSM fired at duty 0.01053
against the continuous time fraction 0.09395, the composed defrost
electric ran 2.11x the continuous model's integral, and the composed
heat ran about one point of capacity under the continuous model alone.
The parity test only asserted a positive average output
(`tests/hvac_parity.rs`), which cannot see any of this.

**Change:** one defrost code path selected by `DefrostConfig`. The new
`defrost_discrete_cycle` flag (default true, the status quo) selects the
discrete frost cycle: `evaluate_defrost` then feeds only the frost
accumulator and the cycle's phases own the per-step multipliers, applied
exactly once per step. The Accumulating phase applies the bare frost
derate `0.875` and the melt term without the continuous model's duty
factor (the duty is the cycle's own events; stacking the duty factor
here is the double application), meters no average defrost power, and
keeps the frost-conditions time fraction for the defrost-recovery latent
divergence (D-030). The Defrosting event draws the continuous average
concentrated into the event window (`extra_power_w / tf`), so the
defrost-electric integral equals the continuous model's; the drawn-down
reverse-cycle event draw replaces the E+ full-rated-shaped draw the
discrete branch invented (the OnDemand model's defrost draw is
`0.1528` per unit of post-defrost capacity, not the rated capacity at
unit EIR). The trigger fires at
`cycle_duration_s * (1 - ewma_time_fraction)` frost-seconds: the
accumulation phase then lasts `cycle_duration_s * (1 - tf) / tf` seconds
and one `cycle_duration_s` event per `cycle_duration_s / tf` cycle gives
the documented duty tf exactly (verified over many cycles). The EWMA
persists across the Defrosting-to-Accumulating transition (resetting it
per event re-ramped the threshold and re-under-fired the duty); frost
and elapsed time still reset, and the warm-OAT frost decay still clears
a hiatus. The resistive destination is decided once:
`resistive_defrost_destination(element_w, zone_fraction)` (in
`defrost.rs`, shared by both paths) routes the new
`resistive_defrost_zone_fraction` (default 0, [0,1]-validated) of the
element's heat to the zone and the remainder is frost-melt loss; the
continuous Resistive path carries its zone share as a negative
`q_defrost_w` in the same helper. `defrost_discrete_cycle: false`
selects the continuous model alone, and the tracker stays dormant.

**Acceptance test:** `defrost_accumulated_energy_discrete_matches_continuous`
(`crates/hares-equipment/tests/hvac_parity.rs`): 48 h at 1 C / RH 85
percent (humidity ratio 0.003447), 10 s steps, a 10 s-step warm-up, a
static always-calling operating point with no fan and no backup: the
discrete run's delivered heat equals the continuous-alone run's within 1
percent (measured 1.0003). `defrost_discrete_defrost_electric_matches_continuous_integral`:
the discrete run's defrost-electric integral equals the continuous
model's `extra_power_w` integral within 1 percent (measured 1.0050).
`defrost_fsm_duty_matches_documented_time_fraction`: the FSM's
Defrosting-step fraction equals the continuous time fraction within 5
percent (measured 0.09442 against 0.09395).
`defrost_accumulating_steps_apply_the_frost_derate_once`: Accumulating
steps meter no average defrost power and carry the bare 0.875 multiplier.
Unit tests in `defrost.rs`: the duty over many cycles, the trigger
timing under an oscillating time fraction, the destination split, and
the continuous Resistive zone share as a negative q. RED first: the duty
(0.01053), the stacked average defrost power on Accumulating steps
(209159 W summed), and the boiler-independent heater tests at the parent.

**Verification:** the defrost probe numbers above; `frame_golden
compare-all` twice byte-identical on the default and dst feature sets,
`fleet_mixed_offset`, `leap_year_february_900s`, `weather_wrap_900s` and
`full_year_900s_dst` re-captured deliberately (the ASHP-bearing
fixtures; the movement includes the coupled water-heater columns, the
Water Heating End Use Electric Power and Electric Resistance Water
Heater Electric Power columns, following the changed indoor
temperature; the causes in this entry and in `docs/python.md`'s Numeric
changes list); the diagnostics pin `crates/hares-core/tests/fixtures/diagnostics_cz4a.csv`
re-pinned with the cause; the 48 h OCHRE thermal trace's records
re-captured (heating 31.3747 to 28.9580 kWh, the overnight worst
divergence 1.2309 to 0.7800 C, both toward OCHRE; an interim
contradiction of these numbers was itself measured on a stale extension
and struck: the canonical-env rule now reads: the pytest records
re-verify only after maturin develop); the warm-up
convergence test's leap candidate re-pinned (day 2 to day 4: the
corrected duty's zero-capacity events lengthen the ASHP's warm-up
transient); fmt, clippy `--workspace --all-targets --all-features -D
warnings` clean; nextest `--workspace --all-features -j 2` 8158/8158 (8
skipped); `pytest -m ''` and pyright green on the wave's combined tree.

**Commit:** `fix: the defrost penalty is applied by one model, not stacked`

**Dependencies:** none outside the equipment crate.

**State:** landed on `row/hw1-defrost-boiler` (worktree
`hw1-defrost-boiler`, cut from `7e78923d`). The step-quantization note:
at timesteps coarse against the 210 s event the event occupies whole
steps and the realized duty quantizes upward (the discrete model's
resolution limit; the acceptance tests run at 10 s steps where the event
resolves). The defrost EIR curve coefficients now feature in the discrete
event draw only through the Timed control's continuous model (the
OnDemand model has no curve input, matching OCHRE), where the old
discrete branch read them for both controls.

## Hardening 2: the boiler routes the flue loss outdoors, the jacket loss to the zone

**Kind:** numeric fix (the post-initiative hardening phase's combustion
entry: the boiler's loss destination, with the shared combustion module
extracted).

**Problem:** the gas boiler routed its entire combustion residual (fuel
input plus pump electric minus useful thermal output) into the zone as
jacket loss (`boiler.rs:1009-1023` at the defrost commit's parent): the
non-condensing boiler's `eir_max = 1/AFUE` (AFUE 0.80 default,
`boiler.rs:738`) leaves a residual near 20 percent of the fuel input, so
about a sixth of the burned gas returned to the zone as free heat. The
gas water heater already splits its losses (the flue fraction of the
combustion heat outdoors, `gas.rs:595-596`, default 0.10) and the
furnace keeps its losses out of the zone entirely; the boiler was the
odd one. Measured at the acceptance test's operating point (a
non-condensing AFUE 0.80 boiler at 10 kW, full load, steady state): the
zone's JacketLoss gain was 2179.6 W, the entire residual.

**Change:** the shared module `crates/hares-equipment/src/combustion.rs`:
`combustion_loss_split(fuel_input_w, useful_w, flue_fraction,
skin_fraction) -> (zone_w, outdoor_w)` splits the residual between the
two destinations it has, validating both fractions in [0, 1] and their
sum at 1 (a typed error naming the fractions otherwise). The boiler
gains `flue_loss_fraction` (default 0.9: ASHRAE puts a boiler's jacket
loss at a few percent of the fuel input, so the flue carries the balance
of the residual; the complement is the jacket share) and routes only the
jacket share plus the pump's electric dissipation to the zone; the flue
share leaves outdoors. The gas water heater's own flue-versus-tank
arithmetic is untouched: it is the efficiency model the helper's pattern
extracts, and adopting the residual helper there would reorder its
floating-point operations and move its fixtures by rounding noise. The
condensing boiler's residual floors at zero (the helper's floor), so its
jacket heat stays the pump dissipation only, as before.

**Acceptance test:** `boiler_jacket_gain_is_a_share_of_the_loss_not_the_whole_loss`
and `boiler_jacket_gain_equals_the_configured_share_of_the_loss`
(`crates/hares-equipment/tests/combustion_tests.rs`): a non-condensing
AFUE 0.80 boiler at 10 kW, steady state, 5 measured steps: the zone's
JacketLoss category gain is the configured jacket share of the residual
(0.1 by default, 218.0 W at this operating point) plus the pump's
dissipation (zero here), never the whole loss; the unit tests in
`combustion.rs` pin the split, the zero floor for the condensing
surplus, and the fraction validation. RED first at the defrost commit
(the zone's jacket gain was the entire residual, 2179.6 W).

**Verification:** the jacket numbers above; `frame_golden compare-all`
twice byte-identical on the default and dst feature sets with only
`fleet_mixed_offset` moving (the fleet's gas-boiler homes' gas power and
the coupled indoor temperatures; the Gas Water Heater Gas Power column
moves with them, the water heater's gas draw coupled to the changed
indoor temperature; the flue share is no longer free zone heat),
re-captured deliberately; the gas water heater's split
bit-identical; fmt, clippy `--workspace --all-targets --all-features -D
warnings` clean; nextest `--workspace --all-features -j 2` 8164/8164 (8
skipped); `pytest -m ''` 1766 passed + 6 xfailed, 0 failed, 0 errors;
pyright 0.

**Commit:** `fix: the boiler routes flue loss outdoors, jacket loss to the zone`

**Dependencies:** none outside the equipment and io crates.

**State:** landed on `row/hw1-defrost-boiler` (worktree
`hw1-defrost-boiler`), the defrost consolidation's commit ahead of it.

## Hardening 4: the EV forms an island only when it can deliver power, and islanded charging pays the surplus clamp

**Kind:** numeric fix (the post-initiative hardening phase's grid entry:
the EV's island-source availability and the islanded EV charging clamp).

**Problem:** the EV's `island_source_available()` keyed on a commanded
discharge, not a delivered one: the check was `HomePluggedIn &&
v2l_active && soc > 0.0`, but `v2l_active` latches on the COMMANDED
discharge (`run_charging_physics` returns `is_discharge` true whenever a
negative setpoint resolves to a discharge leg, even when
`compute_discharge` returns `(0, 0)` at the floor or under a DR full
shed), so an EV drained to its V2L/V2G reserve with a latched export
vote held the home bus at 1.0 pu for the whole outage while delivering
zero watts: every load ran at full power on energy from nothing, the
deficit surfacing only as `island_unserved_w`. The stationary battery's
availability check was already rigorous (SOC above its effective floor,
the derate positive, the DR fraction positive); the EV's was not. And
the islanded EV charging leg was gated only on a dead bus: while the
bus was energized (islanded), the charge leg ran at the full EVSE rate
and `apply_soc_and_thermal` credited the pack the full charge energy —
an 11.5 kW EVSE islanded on a 5 kW battery credited the pack 11.5 kW of
charging while the island's sources delivered a fraction of it, the
deficit landing in `island_unserved_w` — the same phantom-energy defect
the battery's own surplus clamp had closed.

**Change:** `island_source_available()` requires delivered export: the
previous step's `v2l_power_w > 0.0` (the dispatch is producing, not
merely latched), the pack above the commanded leg's effective discharge
floor (`max(reserve, commanded min_soc, ready_by_soc)` — the leg
resolved by the same V2G-over-V2L precedence the charge physics
applies, extracted as `commanded_discharge_leg()`), and
`dr_power_fraction() > 0.0` — mirroring the battery's check shape.
Islanding takes effect one step after export begins (as before) and
collapses the first step delivery stops. The islanded charge leg (bus
energized under `grid_outage()`) clamps to the visible Stage-1 surplus
exactly as the battery's target does (the same
`(-net_load_w).max(0.0)` read of the accumulator at the equipment's own
step); away charging is off-site and unaffected; the discharge leg
carries no charge leg and passes through untouched.

**Interaction found by the probe, handled honestly:** the
auto-registered EV driver re-asserts a standing `SOCTarget` every
plugged-in step at the Schedule tier, and the EV's `SOCTarget` arm
clears a latched `PowerSetpoint` (the explicit-hold contract: a fresh
charge target supersedes a zero-power hold). An externally latched
negative setpoint therefore cannot coexist with the auto-registered
driver's own dispatch — same tier, the driver's write lands later and
clears the external one. The tier fight is resolved by configuration,
not by a priority change: a manual-control household supplies an actor
holding the `EvDriver:<equipment>` name (the documented
`add_equipment_with_actors` replacement), and the driver-in-the-loop
scenario runs the driver's own V2G vote, which holds by the composer's
own signal order (the target-of-record is dispatched before the
rate-of-record, so the same-batch rate survives its own target's
clear). The tests exercise both shapes; no dispatch-priority machinery
changed.

**Acceptance tests:** equipment level, `ev/tests.rs`:
`island_availability_requires_delivered_export` (at the reserve the
commanded export delivers zero and the EV is no island source; above
the floor the same command delivers and is — RED at the parent: the
availability read true at the reserve),
`island_availability_ends_at_dr_full_shed` (a GridEmergency shed zeroes
the delivered export and the availability with it — RED at the parent),
`islanded_charging_clamps_to_visible_surplus` (an 11.5 kW EVSE on a
2.5 kW surplus draws the surplus and the SOC credit stays under the
clamped charge energy — RED at the parent: full rate),
`grid_connected_charging_is_not_clamped` (the same bus state on a live
grid draws the full EVSE rate; the clamp is outage-only). Dwelling
level, `tests/grid_outage_islanding.rs`:
`ev_at_discharge_floor_with_latched_export_does_not_hold_bus` and
`dr_shed_ev_does_not_island` (the bus stays dead with a phantom source
— RED at the parent: the bus held at 1.0 pu delivering nothing),
`island_follows_ev_delivery_and_collapses_when_it_stops` (the driver's
own V2G vote discharges the pack to its strategy floor during the
outage; every energized step is preceded by delivered watts and the
island collapses when delivery stops — RED at the parent),
`islanded_pack_energy_is_bounded_by_island_sources` (the conservation
property: every islanded step, neither the EV pack nor the battery
pack gains more energy than the island's sources delivered — RED at
the parent: the first islanded step credited the EV pack the full
11.5 kW rate against the battery's 1.5 kW delivery). The two
pre-existing tests that pinned the old behavior were re-pinned to the
corrected contract: `floor_held_discharge_checkpoint_restores_the_published_v2l_state`'s
availability precondition (the floor-held corner publishes the
dispatch-active state with zero delivered watts and is now
unavailable) and `grid_outage_stops_home_charging_and_islanded_bus_allows_it`'s
islanded arm (charging resumes clamped to the injected surplus, not at
the rating).

**Verification:** RED first (the six behavioral failures above
witnessed at the parent with the tests applied); GREEN after; fmt,
clippy `--workspace --all-targets --all-features -D warnings` clean;
nextest `--workspace --all-features -j 2` 8254/8254 (8 skipped);
`pytest -m ''` after `maturin develop` 1800 passed + 6 xfailed, 0
failed, 0 errors; pyright 0; the goldens compare-all 10 + dst 1
bitwise identical (no golden runs an outage with an EV, and the clamp
and the availability gate key on `grid_outage()`, which no nominal
fixture fires).

**Commit:** `fix: the EV forms an island only when it can deliver power, and islanded charging pays the surplus clamp`

**Dependencies:** none outside the equipment crate.

## Hardening 5: the grid voltage override is validated at the setter

**Kind:** numeric fix (the post-initiative hardening phase's grid
entry: the outage setter's input contract).

**Problem:** `Dwelling::set_grid_voltage` stored its override raw and
`GridState::bus_energized()` keyed on `bus_voltage_pu() != 0.0` — a
NaN override passed `NaN != 0.0`, read as energized, and un-gated every
load: an intended outage signaled with one non-finite value (a gym or
co-sim caller computing voltage from a model that produces one NaN)
ran the house at full load, islands nothing, errors nowhere.

**Change:** the setter accepts exactly `0.0` (the utility-outage
signal) or a finite per-unit voltage in `[0.5, 1.5]` — the band the
HELICS voltage contract and its subscriptions already admit — and
rejects everything else with `HaresError::Dwelling` naming the value
and the band, before any state is written (a rejected override leaves
the stored grid state untouched). `bus_energized()` is NaN-robust as
the second belt: `bus_voltage_pu() > 0.0`, so a non-finite voltage that
reached the state by any other route de-energizes the bus (every load
off) instead of un-gating it. Callers propagate the typed error: the
fleet's `set_grid_voltage` maps it to a new `FleetError::InvalidGridVoltage`
naming the home, `set_grid_voltage_all` returns it (the first home's
rejection, every home running the same validation), the Python
bindings surface it through their existing mappings, and the HELICS
fleet adapter's out-of-band branch (which warned but still applied)
now fails the apply the same way the dwelling adapter's does.

**Acceptance tests:** `tests/grid_voltage_override.rs`:
`in_contract_overrides_are_accepted` (0.0, the band's edges, a sagged
nominal) and `out_of_contract_overrides_are_rejected` (NaN, both
infinities, a negative, 0.2, 1.8 and a volts-vs-per-unit 240.0 each
rejected with the value- and band-naming error and the stored grid
state untouched; RED at the parent as a signature failure — the setter
returned nothing to assert on). Unit:
`environment::tests::bus_energized_is_nan_robust` (a NaN utility
voltage and a NaN island value both de-energize the bus; RED at the
parent: `NaN != 0.0` read as energized).

**Verification:** RED first (the behavioral NaN failure and the
setter's signature failure witnessed at the parent); GREEN after;
every pre-existing `set_grid_voltage` test call site updated to the
checked form; fmt, clippy `--workspace --all-targets --all-features
-D warnings` clean; nextest `--workspace --all-features -j 2` 8254/8254
(8 skipped); `pytest -m ''` after `maturin develop` 1800 passed + 6
xfailed, 0 failed, 0 errors; pyright 0; the goldens compare-all 10 +
dst 1 bitwise identical.

**Commit:** `fix: the grid voltage override is validated at the setter`

**Dependencies:** none outside the core, types, fleet and python
crates.

## Hardening 6: the tariff rejects coverage gaps, and the EV rate reaches the bill

**Kind:** numeric fix (the post-initiative hardening phase's tariff
entry: the silent $0/kWh fallbacks and the dead EV TOU rate).

**Problem:** the tariff evaluator's price precompute fell back to
$0/kWh on coverage gaps — a TOU hour matching no period priced the
step's import at nothing, and a matched period with no energy rate for
the month did the same — so a hand-built tariff with a season or
schedule gap silently billed free electricity, corrupting every bill,
DR vote and RL reward built on it. `ElectricTariff::validate` checked
that rates reference known periods but never the converse (period
coverage) nor full-hour coverage. And the dwelling's only
`evaluator.step` call hardcoded the EV flow to `0.0`: the EV-rate
machinery (the `ev_tou_period_name` validation, the per-step EV price,
the blended import price) was dead code at the integration, and a
configured EV-TOU rider silently billed the EV's kWh at the house rate.

**Change:** `ElectricTariff::validate` gains
`validate_energy_coverage`, run for tariffs that price energy through
TOU periods (no `rtp_schedule`, a non-empty `tou_schedule` — RTP
tariffs price energy from their schedule and a periodless tariff bills
no energy through periods, so neither is this contract's subject). Two
gaps are rejected, both resolved exactly as the evaluator resolves
them (`season_contains_month` with the tariff's split; first matching
window; first matching rate): every month is claimed by some period
and the claiming periods' windows cover every weekday-minute of the
week in every claimed month; and every (period, month) pair the
schedule claims carries an energy rate. The EV sub-tariff's period
must carry a rate in every month (its per-step price resolves by name
and month alone, so a period whose own season claim is narrower than
the year passed the period check while its rate missed months the EV
sub-meter prices). The dwelling sums the per-end-use signed EV flow it
already computed for the electrical summary and passes it into
`evaluator.step`, so a configured `ev_tou_period_name` bills the EV's
import share at its own rate through the evaluator's blended price;
without an EV sub-tariff the flow changes nothing (the evaluator's
`ev_power_w > 0.0` guard and the blended-price identity keep every
unconfigured run's arithmetic untouched).

**Acceptance tests:** `crates/hares-tariff/src/types.rs` unit tests:
`tou_hour_with_no_matching_period_fails_validate`,
`month_no_period_claims_fails_validate`,
`period_without_rate_for_claimed_month_fails_validate`,
`ev_tou_period_without_month_rate_fails_validate`,
`fully_covered_tariff_validates` (the first four RED at the parent:
`validate` accepted the gap tariffs). Dwelling level,
`tests/ev_tariff_rate_wiring.rs`:
`ev_tou_period_name_changes_the_dwelling_bill` (the same run twice on
bits — same total import kWh, identical EV charging — with and without
the EV sub-tariff: the billed energy charge strictly undercuts and the
saving equals the EV energy times the rate difference; RED at the
parent: the bills were identical with the EV flow hardcoded to zero).
The tests that pinned the old fallback were re-pinned to the corrected
contract: `evaluator_no_match_returns_zero` became
`evaluator_no_match_rejected_at_construction` (the gap tariff is a
construction error, not free electricity), the URDB
`detect_summer_no_seasonal_variation_defaults_northern` fixture's
schedule now references the period its rate structure defines (the old
fixture referenced a period with no rate — the parse would silently
price every hour at $0/kWh, which the new validation catches at
parse), and the Python builder suite's gap tariffs were re-pinned to
full coverage: the shared simple-TOU builder's off-peak window covered
minutes 0-960 of 1440 (the defect class itself — evenings, nights and
winter weekends priced free), and the two chaining tests' peak-only
tariffs gained coverage base periods.

**Verification:** RED first (the four validate tests, the construction
test and the dwelling bill test witnessed failing at the parent);
GREEN after; fmt, clippy `--workspace --all-targets --all-features
-D warnings` clean; nextest `--workspace --all-features -j 2` 8254/8254
(8 skipped); `pytest -m ''` after `maturin develop` 1800 passed + 6
xfailed, 0 failed, 0 errors (the suite's 15 pre-fix failures were all
the builder suite's gap tariffs, re-pinned with the fix); pyright 0;
the goldens compare-all 10 + dst 1 bitwise identical (the shipped
`flat_tariff.json` covers 24x7 with its period rated, so no fixture
moves).

**Commit:** `fix: the tariff rejects coverage gaps and the ev rate reaches the bill`

**Dependencies:** none outside the tariff and core crates.
## Hardening 7: generated schedules are addressed by calendar date like the CSV path

**Kind:** numeric fix (the hardening phase's schedule-anchoring
entry).

**Problem:** the generated schedule's rows started at the run's start
date (`generate_default_schedule` anchored its timestamps at
`generation.start`), but the environment addresses schedule rows by
calendar ordinal (`compute_schedule_idx`: the 365-day ordinal
`hares_types::schedule_day_ordinal` times the day length plus the time
of day, modulo the table's length - row 0 is Jan 1), the convention the
CSV path's timestamps and `schedule_day_ordinal`'s leap mapping share.
A run starting Jul 1 therefore read, at its first step, the row the
addressing landed on - for a seven-day table, the Jul 7 23:45 row; for
longer tables, rows generated for the previous December - every
generated column (occupancy, plug loads, lighting, appliances, water)
on the wrong month multiplier and weekday type, silently, for any start
past Jan 1. Jan 1 starts in a non-leap year were the one accidentally
correct case (the anchor and the addressing agree), and a leap start
year's Feb 29 let a February multiplier read at Mar 1.

**Change:** `generate_default_schedule` anchors the table at Jan 1 of
the start's year on the 365-day calendar - the start year's own weekday
pattern, any Feb 29 in the span skipped - and generates the start's
ordinal plus the duration in rows, so the table spans from the
calendar's Jan 1 to the run's end and the environment's calendar
addressing reads every start's own calendar dates. A mid-year start
reads the profile generated for its own dates; the leap-year mapping
(`schedule_day_ordinal`'s after-Feb-29 shift, Feb 29 itself on the Mar 1
slot) lands on the row generated for that date; the Dec 31 to Jan 1 wrap
reads the Jan 1 row. The callers (`DwellingBlueprintBuilder::from_config`,
the fleet's build, the Python constructor's generation span) pass the
run's span unchanged: the anchor is the generator's contract.

**Acceptance test:** `generated_schedule_midyear_start_reads_its_own_calendar_date`
(`crates/hares-core/src/environment.rs`): the generated schedule over
the shipped profiles, a Jul 1 2023 start fed to the environment manager:
the payload's lighting value at the first step is the weekend hour-0
fraction times July's multiplier (0.00574). `generated_schedule_leap_year_start_reads_march_after_feb29`:
a Jan 1 2024 start's Mar 1 step reads the weekday hour-0 fraction times
March's multiplier (0.01224), never the Feb 29 slot's February value
(0.01332). `generated_schedule_dec31_wraps_to_the_jan1_profile`: the
step at Jan 1 reads January's multiplier (0.00833), never the run's own
first row's December (0.00840). Generator-level pins in
`crates/hares-io/src/hpxml_schedule.rs`: the table starts at Jan 1 of
the start's year and spans the start's ordinal plus the duration
(181 + 7 days of 900 s rows for the Jul 1 test), a leap start year's
Feb 29 is skipped (row 59 is Mar 1, row 60 Mar 2), and the Dec 31 run's
table covers into Jan 1. RED first: all three environment tests read a
rotated value (0.00984 / 0.01332 / 0.00840), the generator tests
counted the run start as the anchor.

**Verification:** the numbers above; `frame_golden compare-all` twice,
10/10 identical on the default feature set and the dst fixture identical
- no manifest moves, because every fixture carries an explicit schedule
CSV and a Jan 1 non-leap start's generated table is bitwise the earlier
one; the generated path's own pin
(`schedule_path_contract::GENERATED_SCHEDULE_DIGEST`) re-captured
deliberately (the 24 h Jan 15 start's table now spans the Jan 1 to
Jan 15 rows the calendar addressing reads; the digest's comment names
the move). fmt, clippy `--workspace --all-targets --all-features -D
warnings` clean; nextest `--workspace --all-features -j 2` 8243/8243
(8 skipped); `pytest -m ''` 1800 passed + 6 xfailed, 0 failed, 0
errors; pyright 0.

**Commit:** `fix: generated schedules are addressed by calendar date like the csv path`

**Dependencies:** none outside the io and core crates.

**State:** landed on `row/hw2-anchor` (worktree `hw2-anchor`, cut from
`a0350e07`), commit `37c9095a`. The addressing convention's one known
property, shared with the CSV path: rows past the anchor year carry the
anchor year's weekday pattern (the 365-day calendar is cyclic annual
data), so a run crossing into a second calendar year reads the anchor
year's weekday type for the same ordinal. Follow-up on the convention's
DST edge: the reviewer's probe put a run-spanning generated table's tail
past the span (a five-day March run at 900 s over America/Denver's
2024 spring-forward: after the spring-forward the civil clock runs up to
3600 s past the table's fixed-offset end, and the run's last four steps'
civil addresses wrap to the cycle's opening rows, reading January's
profile). A hold-the-end clamp was assessed and rejected: it breaks the
pinned periodic-read tests and the weather path's documented shared
modular pattern. The cyclic convention and this edge are on the
divergence ledger (docs/alignment/DIVERGENCES.md, D-034) with the probe
pinned at
`environment.rs::dst_run_tail_past_the_generated_tables_span_wraps_per_the_cyclic_convention`.

## Hardening 8: warm-up restores every stochastic stream, not only the dwelling rng

**Kind:** numeric fix (the hardening phase's warm-up neutrality and
checkpoint-completeness entry).

**Problem:** the warm-up ran the full timestep pipeline (actors decide,
event loads draw) and afterwards restored only the dwelling rng
(`build_from_blueprint_inner_with_seed`'s snapshot block), so
production inherited every warm-up draw position: the event loads'
`StartDraws` sat `days_run * steps_per_day` draws into their streams,
every stochastic or shared schedule source sat mid-cursor, the EV
driver's day-roll rng and its four driving sources (miles, departure,
trip duration, arrival - each with its own ChaCha8 generator) sat at the
last warm-up day's positions, and the Occupant's presence read a
rotated step counter. Production telemetry therefore depended on the
warm-up day count - a warm-up converging at day 2 and one at day 4 ran
different production episodes - and the Occupant's rotated counter fell
silently back to `Presence::Home` past the schedule's end (release; the
debug assert fired). Beside that, the checkpoint contract had the same
holes in its own shape: `ScheduleSource::reset()` had no production
caller, the EV driver's snapshot carried only its day-roll rng (episode
two's draws continued episode one's sources, so `reset(seed)` twice
diverged), and the water heaters' draw, mains and appliance schedule
sources rode in no snapshot at all.

**Change:** three moves. The warm-up boundary: the dwelling snapshots
every actor's decision state before warm-up and restores it after (the
actors hold decision state only, so the snapshot is their complete
stochastic state), and rewinds every equipment's stochastic schedule
sources to their construction position through the new
`Equipment::reset_stochastic_state` (default no-op): the event loads'
`StartDraws` to draw zero with the phase back at idle, the schedule
sources of the scheduled loads, ventilation, the thermostats' setpoint
sources and all five water heaters at their seeds and cursors. The
Occupant's presence reads the environment's calendar schedule row
(`env.schedule_row`, the same row addressing the occupancy column's
values) with the private counter kept only as the fallback for states
built without an environment manager - presence follows the clock
whatever decided before production. The checkpoint's shape: the EV
driver's snapshot carries the composer's carried decision state and all
four driving sources' positions (the actor schema at version 5), every
water heater's state carries its sources' positions (gas, tankless,
resistance and the heat pump at their next versions, the indirect tank
gaining its first explicit version), and the checkpoint carries the
environment's actor-facing equipment maps (the telemetry and typed core
outputs the actors read) restored as saved rather than rebuilt from the
restored equipment. `Actor::reseed` re-derives every driving source
from the new dwelling key (the construction derivation applied to the
seed, each position rewound), so a reseeded episode's draws are the
seed's, never the previous episode's; the per-source key derivations
moved next to the archetype builders that own them
(`ArchetypePreset::derive_miles_seed` and siblings). The schedule-source
state type moved to `hares-types` beside `ScheduleSource`
(`ScheduleSourceState` with `capture_state`/`restore_state`/`rederive`):
one home for the capture, the mismatched-restore error and the
re-derivation, with the equipment crate's helpers delegating.

**Acceptance test:** `production_telemetry_is_invariant_to_the_warmup_day_count`
(`crates/hares-core/tests/warmup_stochastic_neutrality.rs`): a synthetic
dwelling whose only thermal state sits at a fixed forcing (constant
weather, solar off, no HVAC, a zero-gain event load), warm-up forced to
two and to four days (`warmup = { min_days = k, max_days = k }`, both
counts pinned through the run health): two simulated days of production
telemetry - net electric, HVAC delivery, gas, every equipment and actor
channel - bit-identical across the two warm-up day counts. (The zone
temperatures are not compared: they carry warm-up's thermal initial
condition, the convergence criteria's to bound, and feed back into
nothing the stochastic state reads here.)
`gym_style_reset_seed_twice_is_bit_identical_on_an_ev_archetype_dwelling`:
the step-0 checkpoint restored, the streams reseeded, an episode run,
then the same restore and reseed again: two episodes bit-identical for
a dwelling with an EV carrying the archetype's stochastic driving
sources beside a stochastic event load. Unit pins: `reseed_rederives_every_driving_source_from_the_new_key`
(each source's key is the new key's derivation, positions zero, two
seeds never share a keystream), `presence_follows_the_environment_schedule_row_not_the_step_counter`
(a rotated counter cannot rotate presence), `checkpoint_restore_continues_the_stochastic_draw_source`
(a gas water heater's noisy draw source continues, not rewinds, across a
restore). RED first: the invariance test's event patterns diverged
(the first event at step 4 against step 1), the gym test's first
episode never charged at step 0 while the restored episode charged 3.75
kW from a rebuilt environment map, the water heaters' draw state rode in
no snapshot.

**Verification:** the tests above; `frame_golden compare-all` on the
default and dst feature sets, every fixture identical against its
manifest after two deliberate re-captures (`der_pv_battery_900s` and
`der_pv_battery_ev_900s`, each twice byte-identical): both moved one
cell, the BatteryManagementActor's `bms_toggled` flag at row 0 (0.0 to
1.0) - the restored pre-warm-up decision state puts the BMS's step-0
hysteresis toggle event in production, where the warm-up-carried latch
had swallowed it; no power, energy or temperature column moves
(`CHECKPOINT_VERSION` 20 to 21, the schema fingerprint re-pinned; the
EV driver's actor schema 4 to 5; the water heater schemas bumped beside
their new fields). fmt, clippy `--workspace --all-targets
--all-features -D warnings` clean; nextest `--workspace --all-features
-j 2` 8248/8248 (8 skipped); `pytest -m ''` after `maturin develop`
1800 passed + 6 xfailed, 0 failed, 0 errors; pyright 0.

**Commit:** `fix: warmup restores every stochastic stream, not only the dwelling rng`

**Dependencies:** none outside the core and equipment crates.

**State:** landed on `row/hw2-anchor` (worktree `hw2-anchor`), the
schedule-anchoring commit ahead of it. The re-derivation scheme's one
scope note: the dwelling reseeds only the actors it recorded
(the auto-registered drivers); a caller-seeded driver's generators are
the caller's, and its checkpoint-carried source positions are what make
`reset(seed)` exact for it.

## Hardening 9: the precomputed same-zone boundary keeps the zone-facing half-layer at the zone

**Kind:** numeric fix (the hardening phase's same-zone precomputed
boundary entry: the resistor order the two advertised-equivalent build
paths disagree on).

**Problem:** the same-zone precomputed path wired every resistor one
index off. After the same-zone split keeps the interior half of the
stack, the pad-and-average (`boundary_rc.rs` Step 2) produces N+1
resistors for N capacitors: the first is the cut-surface half-resistor
(the pad's leading zero averaged with the first kept layer), the last is
the zone-facing half-resistor (the trailing zero averaged with the
innermost kept layer). The code removed the LAST resistor (kept the
stub) and wired with `res_offset = 0`, so the cut stub became the
outermost inter-layer resistor and the zone connected through the
innermost inter-layer average: the same total series resistance with
every capacitor sitting one half-layer off, so the fin's dynamics
diverged from the material path (node-centered, exact) while steady
state still converged. Measured on a 4-layer two-material probe: the
inner-node-to-zone coupling 1.232e-3 (material) against 0.661e-3
(precomputed), -46 percent; the review's identical-layer probe measured
-22.7 percent coupling and -24.3 percent 10-minute response. Masked in
production because the shipped same-zone LUT rows the fixtures resolve
to furniture are single-layer (for n=1 removing the first and the last
resistor are the same r/2), but live for the auto-generated partition
wall's "Interior Wall" row (three layers, two kept) in every
conditioned dwelling, and for any multi-layer furniture-like LUT row or
TOML override. The steady-state-only regression test (600 x 60 s steps,
1e-6) validated convergence, not dynamics: both topologies converge to
zone temperature. The comment at the removal site claimed the pop was
the correction and remove(0) the bug; it had the two orientations
backwards (OCHRE's `res_list[:-1]` drops the last resistor only after
its node and resistor lists are reversed, where the last is the cut
stub; HARES keeps the interior half unreversed, so the stub is the
first).

**Change:** the same-zone branch removes the cut-surface stub
(`res_list.remove(0)`) and keeps `res_offset = 0`, so the zone-facing
half-layer wires the innermost kept node to zone air and the film rides
that edge, matching the material path's node-centered topology resistor
for resistor. The stale comment replaced with the contract in its own
words. The steady-state-only test replaced with the discriminating one:
`material_and_precomputed_same_zone_short_window_step_response_match`
extracts both paths' interior-mass subsystem, steps 5 x 60 s under a
1 K zone step on a 4-layer two-material stack (both materials below the
diurnal-split density threshold) and asserts the responses and the
inner-to-zone couplings agree to 1e-9. A second probe,
`same_zone_fin_chain_matches_the_read_back_rule_on_bits`, walks the
wired edges of both paths' assembled fins (dyadic R, C, film and area,
so every operation rounds exactly) and asserts the chain equals the fin
rule the read-back applies, bit for bit, both paths.

**Read-back agreement:** `resolved.rs`'s `same_zone_fin_chain` mirrored
the old pop rule (the precomputed arm dropped half the LAST kept
layer); after the fix the network drops half the FIRST kept layer on
both paths, so the precomputed arm moved to the material arm's rule
(the arms collapsed, the cut stub's rule stated once) and the
conventions comment, the Rust acceptance sum test and the Python sum
test updated to the same rule. The base.xml partition-wall pin
re-pinned to the fixed network's chain (0.5177775292587776, was
0.5916125292587776); the furniture pin unchanged (single-layer: the
kept half-layer is the same r/2 either way).

**RED first:** the new short-window test at the parent wiring fails at
the coupling assert (mat=0.0012315270935960591, pre=0.0006613756613756613,
-46 percent); GREEN after the fix (1e-9 on both asserts). The
read-back pins and sum tests fail at the old read-back (the partition
wall 0.5916... against the fixed network's 0.5177...).

**Movement:** every golden moves: all 10 default-feature fixtures and
the dst one. Every conditioned dwelling's auto-generated partition wall
resolves the "Interior Wall" LUT row (three layers, two kept), whose
fin now stores the same heat with the capacitors one half-layer toward
the cut surface; the single-layer furniture fins are unchanged. Each
moved fixture re-captured deliberately twice, byte-identical
(compare-all 10 + dst 1 identical after). The moved value pins
re-pinned with the cause: the ASHP parity fixture's timing anchor
35 to 32 (the fin's mass sits closer to the cut surface, the zone warms
earlier in the ramp), the cz4a diagnostics CSV pin, whose movement is
five of its columns on 53 of its 60 rows (zone1_temp_c,
zone1_thermal_gain_w, electrical_net_w, electrical_net_var,
port_convective_w; step 7: zone1_thermal_gain_w 8147.8 to 8082.7), all
in the conditioned zone's thermal path, consistent with the
partition-wall cause, and the 48 h thermal trace's records (the 48 h heating
28.9580 to 29.1539 kWh, the worst-6 h divergence 2.0090 to 2.0590 C,
the 48 h mean 0.3940 to 0.3971 C; OCHRE's own 47.052 kWh unchanged).

**Verification:** the tests above; fmt clean; clippy `--workspace
--all-targets --all-features -D warnings` clean; nextest `--workspace
--all-features -j 2` 8268/8268 passed (8 skipped); `pytest -m ''`
after `maturin develop` 1800 passed + 6 xfailed, 0 failed, 0 errors;
pyright 0 errors, 0 warnings; the goldens compare-all 10 + dst 1
identical after the deliberate re-capture.

**Commit:** `fix: the precomputed same-zone boundary keeps the zone-facing half-layer at the zone`

**Dependencies:** none outside the envelope and core crates.

**State:** landed on `row/hw4-envelope` (worktree `hw4-envelope`, cut
from `44deba5c`).

## Hardening 10: a capacitance-less envelope fails construction and a rollback counts its kept state energy

**Kind:** numeric fix (the hardening phase's conservation entry: the
thermal-balance invariant's vacuous pass and the tolerated rollback's
unaccounted energy).

**Problem:** two conservation holes. (a) The thermal-balance invariant's
gate (`mod.rs`'s `if !q_gains.is_empty()`) is a vacuous pass production
cannot reach: `assemble_building_rc` clamps every zone node's
capacitance up to `MIN_CAPACITANCE_J_K` and `derive_zone_capacitances`
floors its own output, so a production dwelling never assembles a
capacitance-less model - and on the capacitance-less models the gate
would admit, the per-step capture pushes over the empty capacitance map
and the check runs on all-zero terms, catching nothing. The comment at
the site admitted the silent skip. (b) A tolerated equipment-step
failure's rollback restores only the port snapshot
(`record_equipment_step_failure`): the equipment's internal state keeps
the mutation it committed before erroring (the tank's standby and
conduction pass mutates the node temperatures before the per-injection
validation can fail, `tank.rs`'s `step` window; the battery's charge
leg advances the SoC before its own error paths), while the metered
port draw is discarded - energy the step's accounts no longer explain,
invisible to the invariants (they skip the failed equipment by design)
and bounded only by the failure budget.

**Change:** (a) the construction guard, twice over: a declared zone
capacitance that is zero, negative or non-finite fails
`assemble_building_rc` with an error naming the zone (the silent floor
for those values removed; the floor stays for the values above it), and
the assembled model is checked after wiring: a zone air node leaving
the assembly without a positive capacitance entry is a construction
error naming the zone, the insurance against the assembly-defect class
(a node-ID collision draining the zone entry) that would otherwise turn
the vacuous pass on. The invariant site's comment corrected to state
the vacuous pass and the guard. (b) the kept mutation is counted: the
trait gains `Equipment::stored_energy_j` (the energy the internal
state carries, default `None`), read by the dwelling before every
equipment step and again on failure; the delta (kept minus the
pre-step reading, Wh, signed) accumulates into the new
`RunHealth::discarded_state_energy_wh`, and the run's first nonzero
event raises a warning naming the equipment and the energy (later
events only bump the counter; the rollback warning already names the
equipment per event). The reporters: the four tank-backed water heaters
(gas, resistance, heat pump, indirect) report their water's stored
energy, the battery and the EV report their charge (the EV's is a
follow-up to this entry: its SoC moves in `apply_soc_and_thermal` before
`update_degradation` can fail the step, the mutate-then-fail window the
reporter now covers, on the battery reporter's convention: the SoC held
over the degradation-adjusted capacity, the temperature derate and the
pending drive commitment excluded). The classes without an energy-bearing state,
and so without a reporter, each out of scope for a reason: the tankless
water heaters (water heated in flow, nothing held between steps), HVAC
and the ventilation fans (no storage; the delivered or recovered heat is
the port contribution itself), PV (no storage; generation is
port-visible), the scheduled, event and wet loads (stateless schedules),
the generator and fuel cell (fuel burns as drawn, no stored-fuel state),
and the protocol bridge (a proxy for external models, no internal store).
The rollback's keep-the-state behavior is
unchanged (the documented design); the hole is now a counted, warned
event.

**Acceptance test:** `tolerated_step_failure_counts_the_state_energy_the_rollback_cannot_account`
(`crates/hares-core/src/dwelling/mod.rs` tests): the flaky double
carries state energy, a step moves it 3600 J and fails; the counter
reads 1 Wh and the warning names the equipment and the energy; a
second tolerated failure accumulates to 2 Wh with no second warning.
`a_mid_step_failure_keeps_a_state_mutation_the_rolled_back_ports_cannot_account`
(`tank.rs` tests): the real tank's step with a non-finite injection
power errors after the standby pass moved the nodes, and the stored
energy across the failed step is nonzero (the window the counter
measures); `stored_energy_reads_mass_times_specific_heat_times_temperature`
pins the reading's arithmetic on bits.
`tolerated_ev_step_failure_counts_the_soc_energy_the_rollback_cannot_account`
(`crates/hares-core/src/dwelling/mod.rs` tests, the EV follow-up): a
real EV's step fails in `update_degradation` (the pack heater carries it
out of the temperature envelope under a binding power setpoint) with the
SoC still moving; the counter reads the EV's delta in signed Wh,
bit-faithful, and the warning names the EV.
`a_zone_capacitance_without_a_positive_value_fails_construction`
(`boundary_rc.rs` tests): zero, negative and non-finite zone
capacitances fail construction naming the zone; derived capacitances
assemble.

**RED first:** the acceptance tests applied at the parent: the guard
probe fails (the zero zone capacitance assembles under the silent
floor) and the window probe does not compile (`stored_energy_j` did
not exist; the counter, the trait reading and the checkpoint-schema
fingerprint are new). GREEN after the fix on the same probes. The EV
probe (the follow-up) applied at the entry's landed tip fails at the
parent (the EV reports no stored energy, the trait default, so the
counter misses its delta) and greens after the reporter lands.

**Movement:** no physics moves: every fixture's power, energy and
temperature cells are bitwise the prior captures. The run-health
snapshot the goldens record carries the new counter (0.0 everywhere:
no nominal fixture rolls back a step), so seven default-feature
fixtures and the dst one re-captured deliberately, twice
byte-identical (compare-all 10 + dst 1 identical after).
`CHECKPOINT_VERSION` 21 to 22 (the `RunHealth` type rides the
checkpoint's schema; the fingerprint re-pinned; the version-gate
acceptance reads the constant now).

**Verification:** the tests above; fmt clean; clippy `--workspace
--all-targets --all-features -D warnings` clean; nextest `--workspace
--all-features -j 2` 8272/8272 passed (8 skipped); `pytest -m ''`
after `maturin develop` 1800 passed + 6 xfailed, 0 failed, 0 errors;
pyright 0 errors, 0 warnings; the goldens compare-all 10 + dst 1
identical after the deliberate re-capture.

**Commit:** `fix: a capacitance-less envelope fails construction and a rollback counts its kept state energy`

**Dependencies:** none outside the core, envelope and equipment crates.

**State:** landed on `row/hw4-envelope` (worktree `hw4-envelope`),
behind the same-zone boundary row's commit. The EV reporter follow-up
(the disclosure's reporter list extended, the EV probe added) landed
behind it on the same branch: no physics number moves, the goldens are
bitwise the prior captures.

## Hardening 11: a resolution that cannot divide the day fails at construction

**Kind:** numeric fix (the hardening phase's warm-up arithmetic entry:
the silent truncation and the silent zero-warm-up degeneracy).

**Problem:** the warm-up loop's day count divided the 24-hour day by the
resolution's whole seconds (`run_warmup_converged_inner`,
`crates/hares-core/src/dwelling/mod.rs`): `num_seconds()` truncates, so
(a) a sub-second resolution read as zero seconds, `checked_div`'s
zero-denominator `unwrap_or(0)` fell into the
`if steps_per_day == 0 { return Ok(1) }` arm, and warm-up ran zero steps
and reported one day run; and (b) a resolution that is not a whole
number of seconds (1500 ms) truncated to 1 s and computed 86 400 steps
per day where the day at 1.5 s steps holds 57 600, so each warm-up "day"
spanned 129 600 s of simulated time and the day-over-day convergence
comparison compared misaligned windows. The forcing horizon's setup
arithmetic carried the same truncation (`forcing_horizon_steps`,
`as_secs().max(1)`), silently flooring sub-second resolutions to 1 s in
the horizon's warm-up day floor (the same 86 400 instead of 57 600 at
1500 ms; the duration's own step count likewise divided by the floored
second). Both silent. Verified at the parent: a 1500 ms resolution
computed the horizon 86 400 (the truncated count), and
`run_warmup_converged` at 500 ms returned `Ok(1)` with no warm-up step
run. The Python binding's constructor constrains `time_res` to whole
seconds (`WholeSeconds`), so the sub-second case is unreachable from
Python; the hole is the direct Rust `SimulationConfig` construction,
whose own `validate()` also lets 1500 ms through (it truncates to 1 s,
which divides the hour and the duration).

**Change:** `steps_per_day_exact(time_res_ms)` computes the day's step
count in milliseconds and accepts exactly a positive whole number of
seconds that divides 86 400 s; zero, negative, sub-second and
non-integral-second resolutions fail with `HaresError::Dwelling` naming
the resolution in milliseconds (a whole-second resolution longer than
the day is named in seconds). The warm-up entry and the horizon setup
both compute through it, and the horizon call propagates the error with
`?` inside the dwelling build, so the failure is the construction the
config layer's whole-seconds contract already implies. Whole-second
resolutions compute the same counts the truncated arithmetic did (the
millisecond division is the seconds division scaled by 1000 exactly).

**Acceptance tests:** `steps_per_day_at_the_shipped_resolutions` (60,
900 and 3600 s give 1440, 96 and 24; a day-long step gives 1),
`steps_per_day_rejects_a_resolution_that_cannot_divide_the_day` (0,
-1000, 500, 1500 and 86 400 001 ms each rejected with the value named in
milliseconds; 90 000 s rejected named in seconds),
`warmup_rejects_a_sub_second_resolution_naming_it` (a 500 ms resolution
through `run_warmup_converged` fails the run naming "500 ms"),
`forcing_horizon_floors_at_a_day_of_steps_and_validates_the_resolution`
(the floor's 96, 48 and 672 step pins; 500 ms and 1500 ms rejected with
the value named). All in `crates/hares-core/src/dwelling/mod.rs` tests.

**Review corrections (2026-10-20):** two reach notes the entry omitted.
First, the day-long arm of `steps_per_day_at_the_shipped_resolutions`
(86 400 s giving 1) is arithmetic-valid but practically unreachable:
the tank invariant fires first, and every real 86 400 s run dies at
build with `InvariantViolation { tank_temperature_bounds, 234.696 C }`
(the day-long init step overheats the tank). That named-invariant death
is the loud failure, not a silent path. Second, the named 500 ms error
arm is reachable only past `validate()`: `SimulationConfig::validate()`
intercepts 500 ms first with the truncation-blind message "time_res
must be greater than zero" (`num_seconds()` truncates 500 ms to 0), so
an error naming the resolution in milliseconds is met only past
validate (the acceptance test reaches it by mutating the dwelling's
clock directly).

**RED first:** the two witnesses executed at the parent: the horizon at
1500 ms computed 86 400 (the truncated count) and the warm-up entry at
500 ms returned `Ok(1)` with no step run; the acceptance probes could
not run at the parent (the helper did not exist; the warm-up-entry probe
fails behaviorally at the parent, `Ok(1)` where the error is wanted).
GREEN after the change on the same probes.

**Movement:** none. Whole-second resolutions compute identical counts
(the integer identity above); no golden runs a sub-second or
non-integral-second resolution.

**Verification:** fmt clean; clippy `--workspace --all-targets
--all-features -D warnings` clean; nextest `--workspace --all-features
--build-jobs 2` 8282/8282 passed (8 skipped); `pytest -m '' -n 4` after
`maturin develop` 1800 passed + 6 xfailed, 0 failed, 0 errors; pyright 0
errors; the goldens compare-all 10 + dst 1 bitwise identical.

**Commit:** `fix: a resolution that cannot divide the day fails at construction`

**Dependencies:** none outside the core crate.

## Hardening 12: the ASHP debug read leaves the per-step path

**Kind:** hygiene (the hardening phase's equipment hot-path entry: the
per-step environ read in the heat-pump heater's control resolution; per
the measurement ladder in `02-measurement.md` this is the hygiene case,
not a performance fix: 29 ns/step against a 900 s step budget, and no
consumer-measured delta exists or is claimed).

**Problem:** the ASHP control trace's gate read
`std::env::var("HARES_DEBUG_ASHP_CONTROL")` inside
`HeatPumpHeaterCore::resolve_control` (`crates/hares-equipment/src/hvac/
heat_pump/heater.rs`) - every step for every heat-pump heater instance
(the environ check preceded the ASHP-variant check, so the MSHP, GSHP
and WSHP variants paid it too): an environ lock acquisition and a String
allocation per read, release builds included, for a debug aid that is
unset in every real run.

**Change:** `ashp_control_trace_enabled()` reads the variable once per
process into a `OnceLock<bool>` and the per-step gate reads the cache -
one atomic load per step. The trace itself is unchanged: set the
variable before the run and the same control dump prints; the read is
settled by the first heat-pump step and later environment changes have
no effect (documented on the helper). No test or fixture reads or
toggles the variable, so no test-visible reset was needed. (INFO from
the review: the sibling truncating day-division sites,
`crates/hares-io/src/config.rs`'s `timestep_count`,
`crates/hares-core/src/dwelling/roster.rs`'s per-day step count and
`crates/hares-core/src/environment.rs`'s mains samples-per-day, are
unreachable past Hardening 11's construction error and were never in
scope here.)

**RED first (the witness):** the per-call cost measured in release at
the parent: 1 000 000 environ reads 29.4 ms (29 ns/read) against
1 000 000 once-lock reads 0.2 ms (0.2 ns/read), about 145x, with the
read firing per step per heater in `resolve_control`. The change is
behavior-preserving (the variable unset in every test and fixture), so
the gate is the unchanged suite.

**Movement:** none (the trace's gate only; the dump never fires in a
test or fixture).

**Verification:** fmt clean; clippy `--workspace --all-targets
--all-features -D warnings` clean; nextest `--workspace --all-features
--build-jobs 2` 8282/8282 passed (8 skipped); `pytest -m '' -n 4` after
`maturin develop` 1800 passed + 6 xfailed, 0 failed, 0 errors; pyright 0
errors; the goldens compare-all 10 + dst 1 bitwise identical.

**Commit:** `perf: the ashp debug read leaves the per-step path`

**Dependencies:** none outside the equipment crate.

## Hardening 13: envelope index ceilings and capacitance floors fail or count loudly

**Kind:** numeric fix (the hardening phase's envelope entry: the
index-collision guards, the clamp counting, the panicking assert's
construction-time replacement and the initialization fallbacks'
warnings, from review 2's envelope findings).

**Problem:** five silent-corruption sites in the envelope. (a) Zone air
nodes are numbered 1..=n_zones and material-layer nodes start at
`LAYER_NODE_BASE` (1000), and nothing kept the two ranges disjoint: at
1000 zones the last zone's air node collides with the first layer node
and `alloc_node` silently overwrites the zone's declared capacitance
(the probe's 100 999 J/K zone entry became the wall node's 2 000 000
J/K) while the zone's state row aliases the layer chain; some wirings
trip the loud self-loop-edge guard, exterior-edge wirings corrupt
silently, and the assembled-capacitance insurance added with
Hardening 10 cannot catch it (the overwriting entry is positive). (b)
The per-depth ground nodes are `GROUND_NODE_BASE + index`, a table of
99 entries below `OUTDOOR_NODE_ID` (u32::MAX - 1), and the depth-table
build indexed without a bound: at 100 unique foundation depths the
100th node wraps onto the outdoor driving node itself and the assembly
resolves the run's outdoor column against a ground boundary's column,
driving a ground boundary with outdoor temperature (the probe assembled
Ok with the outdoor column resolved on a model with no outdoor
boundary). (c) The `MIN_CAPACITANCE_J_K` floor clamped silently at the
assembler's three application sites (the declared zone caps, the
material-layer path's halved caps, the precomputed path's scaled caps);
the shipped construction tables carry zero-areal-capacitance fictitious
layers (the Foundation Wall, Floor, Foundation Floor and Garage Floor
fictitious-R rows), so the lift engages in production runs and inflates
a thin or small-area layer's thermal mass invisibly. (d) The per-step
solar path's `assert!(shgc >= transmittance - 1e-6)` panicked on
invalid configuration: an SHGC below the transmittance is
self-contradictory (the SHGC decomposes into the transmitted share plus
a glass-absorbed share), so the rejection belongs at construction, not
a panic mid-run. (e) `initialize_steady_state`'s uniform-temperature
fallback was silent in all three arms: the no-pin steady-state solve
failure and the two pinned paths' singular-inverse fallbacks (the
continuous reduced -A_c solve and the discrete reduced I - A_d solve,
where `try_inverse` returns None) each fell back to a uniform
indoor-temperature initial state without a word - a legitimate
fallback that would hide a broken model. The production constructor
`ThermalSolver::new` is the sole non-test caller and always pins the
indoor zone, so the pinned arms are the sites a real run reaches; the
file's own comment records that OCHRE raises on the singular solve
rather than silently substituting uniform temperatures.

**Change:** (a) and (b) are construction ceilings in
`assemble_building_rc`: a zone count that reaches the layer-node base
and a depth table wider than the ground-node table's 99 entries each
fail construction with the function's error naming the count, the
colliding node and the corruption each prevents; the 99-entry capacity
is a named constant (`MAX_UNIQUE_GROUND_DEPTHS`) on the depth table's
own documentation. (c) The floor's three application sites count their
lifts into `EnvelopeDiagnostics::min_capacitance_clamp_count`, shaped
like `default_r_fallback_count` (behind the `observe` feature), the
zone count drained together with the graph's layer and precomputed
counts before the network is consumed; the clamp itself is unchanged
intended behavior, the count makes it visible. The derive-path floor in
`derive_zone_capacitances` stays uncounted (it returns a plain vector
before any diagnostics exist, and its floored output is what the
assembler's zone loop would see). (d) The assert is deleted and the
check moves to construction: `WindowSolarProperties::validate` rejects
an inverted or non-finite pair for either season naming the surface and
the season, wired through `ThermalSolverConfig::validate` into
`ThermalSolver::new`'s typed `ThermalSolverError::Configuration` (the
crate's config-error path); the per-step arithmetic keeps its `.max`
clamp. (e) All three uniform-fallback arms emit a `tracing::warn!`
naming the branch (continuous or discrete for the pinned arms), the
site (`initialize_steady_state`), the state count and the uniform
substitution. The warning is not an error: raising on the singular
pinned solve, as OCHRE does, would be a behavior change with golden
risk and is a separate decision, not taken here.

**Acceptance tests:** the two ceiling errors (1000 zones, 100 unique
depths) and the two just-below cases assembling cleanly (999 zones with
the declared capacitance intact, 99 depths with the outdoor column
unresolved on an outdoor-free model) in `boundary_rc.rs` tests; the
three clamp counts (a sub-floor material layer, a sub-floor precomputed
layer, a sub-floor declared zone capacitance handed straight to the
assembler) asserting the counter and the clamped value;
`window_solar_properties_reject_shgc_below_transmittance` (both
seasons, the equality case, the non-finite case);
`window_shgc_below_transmittance_is_a_construction_error` (the full
solver construction names the surface and the pair);
`steady_state_fallback_warns` (the warning observed through a counting
subscriber); the two pinned arms' warns
(`pinned_discrete_singular_solve_warns`,
`pinned_continuous_singular_solve_warns`: the singular reduced matrix
under the production pin returns the uniform [21, 21] initial state
and exactly one warn naming the branch and the site).

**RED first (the witnesses, executed at the parent 7576ec89):** at 1000
zones the assembly returned Ok with zone 999's declared 100 999 J/K
entry overwritten by the wall node; at 100 unique depths the assembly
returned Ok with the outdoor column resolved on an outdoor-free model;
the sub-floor material and declared zone capacitances assembled with
the lift applied and nothing counting it; the inverted SHGC pair passed
`ThermalSolver::new` and panicked in the per-step solar path
(`prepare_inputs`); the fallback branch emitted nothing (no warn in the
base source); the two pinned `try_inverse`-None arms were also silent
(the review's probe `rev_pinned_path_singular_solve_is_silent`: a
singular reduced matrix under the production pin returned the uniform
[21, 21] state with a warn count of 0). GREEN after the change on the
same inputs.

**Movement:** none - the guards must be and are unreachable for every
shipped fixture: the goldens compare-all 10 + dst 1 bitwise identical,
no re-capture. `EnvelopeDiagnostics` is `observe`-gated and not
serialized into the golden frames, so the new counter moves no recorded
number; the fixture sweep (every capacitance row under the fixtures and
the shipped defaults, 3040 rows) found the sub-floor capacitances
where the count is expected to engage - the fictitious zero-mass
insulation layers - and no boundary small enough to dip a nonzero
layer's node below the floor.

**Verification:** fmt clean; clippy `--workspace --all-targets
--all-features -D warnings` clean; nextest `--workspace --all-features
-j 2` 8294/8294 passed (8 skipped), the touched crates also green on
default features 2351/2351 (5 skipped); `pytest -m '' -n 4` after
`maturin develop` 1800 passed + 6 xfailed, 0 failed, 0 errors; pyright
0 errors; the goldens compare-all 10 + dst 1 bitwise identical.

**Commit:** `fix: envelope index ceilings and capacitance floors fail or count loudly`

**Dependencies:** the envelope crate, plus the observe-gated
`EnvelopeDiagnostics` literal updates in the core crate's tests.
