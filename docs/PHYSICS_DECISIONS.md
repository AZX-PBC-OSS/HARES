# Physics decisions

Recorded decisions where HARES resolves an input the HPXML file does not
state, or refuses to. Each entry names the rule, the source, and why the
stricter behaviour is correct for this codebase.

## ConditionedBuildingVolume stays required

`BuildingSummary/BuildingConstruction/ConditionedBuildingVolume` is
required: a document without it is a parse error, and a code-built
`Building` used with an infiltration input that needs the volume fails
solver construction. No default is substituted.

OpenStudio-HPXML defaults a missing volume to `ConditionedFloorArea ×
AverageCeilingHeight + ConditionedCrawlspaceVolume` with an 8 ft
`AverageCeilingHeight`
(<https://openstudio-hpxml.readthedocs.io/en/latest/workflow_inputs.html>,
"HPXML Building Construction"). HARES derives its ceiling height and zone
air volumes from the declared volume
(`crates/hares-io/src/hpxml/building.rs`), so defaulting the volume would
also silently replace those derived values with an 8 ft constant. Every
HPXML document OpenStudio-HPXML writes after applying its own defaults,
ResStock's included, carries the element, so the strict rule excludes no
document the reference toolchain produces.

## Missing SiteType resolves to suburban

A missing `<Site>/<SiteType>` resolves to the suburban terrain class for
the AIM-2 wind correction, without a warning: OpenStudio-HPXML's
documented default for the element ("HPXML Site",
<https://openstudio-hpxml.readthedocs.io/en/latest/workflow_inputs.html>).
A `<SiteType>` carrying a value outside `rural`, `suburban` or `urban` is
a parse error naming the element, the value and the allowed values; no
value is silently kept as something else.

## Missing ShieldingOfHome

A missing `<ShieldingOfHome>` resolves to the normal shielding class
(today's value). The element's allowed values are `normal`, `exposed`
and `well-shielded`; a value outside that list is a parse error naming
the element, the value and the allowed values.

## xsd:boolean lexical space

Every boolean element the HPXML parser reads is interpreted through
`parse_xsd_boolean`: exactly `true`, `false`, `1` and `0` after
whitespace collapse, anything else a parse error naming the element and
the value. The case variants (`True`) and the words `yes`/`no` are
outside the `xsd:boolean` lexical space and are rejected rather than
silently read as one of the boolean values.

## Deprecated flue element name

`<HasFlueOrChimney>` is the element's older name and is not read. A
document carrying it is a parse error naming the current element
`<HasFlueOrChimneyInConditionedSpace>`, so a declaration can never be
silently dropped.

## The ideal dispatch delivers the HVAC's own share of the zone sensible column

The ideal-capacity solve returns the zone sensible input column's absolute
value: the total convective gain the zone needs to land on the target. The
column mixes the HVAC delivery with every other convective gain (appliances,
plug loads, jacket losses), so the capacity handed to the HVAC equipment is
the column's value minus the non-HVAC share, estimated by the previous step's
split (`solve_ideal_capacity_for_target`,
`crates/hares-envelope/src/thermal_solver/stepping.rs`). Before the fix the
dispatch was short by the previous step's HVAC delivery (the solve subtracted
the whole previous column), which left the zone permanently below its heating
setpoint and the BESTEST 900 case 12 % under its annual heating energy.

Reference: OCHRE's ideal solve returns `h_desired` with the stated contract
"h_desired should be equal to self.delivered_heat"
(`vendors/OCHRE/ochre/Equipment/HVAC.py:424`, the solve at
HVAC.py:411-435): what the solve returns is what the equipment delivers.
The equivalent battery model publishes its energy window every step,
independent of the thermostat's call (`results.update(self.make_equivalent_battery_model())`,
HVAC.py:601-602, the model at HVAC.py:620-641); HARES's port had gated the
window on the thermostat FSM's transient mode, which at coarse resolution is
Deadband every step while the ideal loop delivers. The window now follows the
equipment's served axis (`crates/hares-equipment/src/hvac/equivalent_battery.rs`)
and publishes whenever the zone capacitance gate is open.

Measured direction of the delta: annual heating and cooling end uses move by
the double-counted share's energy, about 0.3 % on the yearly goldens and 2 %
on the fleet's gas-boiler home (the boiler had been heating to a setpoint the
zone never reached); the BESTEST 600 heating 3222.6 to 3100.9 kWh, cooling
6134.0 to 5737.7 kWh; the BESTEST 900 heating 948.2 to 1061.2 kWh (toward its
ASHRAE band); the BESTEST 640 heating 2144.3 to 2065.8 kWh.

## Cycling HVAC runs the fraction of the step the zone needs

A cycling unit (an air conditioner, a heat pump heating or cooling, a
furnace, a boiler, a baseboard) no longer runs whole steps at full capacity
or off: its control computes a runtime fraction, the zone's position between
the thermostat's release edge and its full-capacity edge with the span held
to at least 0.5 C (`cycling_load_fraction`,
`crates/hares-equipment/src/hvac/helpers.rs`), and the unit delivers that
fraction of rated capacity within the step. The thermostat FSM stays the
on/off latch and the equivalent battery's window and baseline power follow
the delivery. The electric draw of DX equipment follows the runtime fraction
`RTF = PLR / PLF` with the part-load degradation `PLF = 1 - Cd (1 - PLR)`
(EnergyPlus `DXCoils.cc:9859` and the PLF curve's [0.7, 1] clamp at
`DXCoils.cc:1101-1131`, Cd = 0.25 per AHRI 210/240-2023 S6.6.3, the
vendor's `Coil:DX` default clamp); fuel and resistance coils scale their
delivered load and energy by the part-load ratio (EnergyPlus
`HeatingCoils.cc:1873-1874`). OCHRE runs the same part-load delivery through
duty-cycle control at sub-hourly resolution (`vendors/OCHRE/ochre/Equipment/HVAC.py:315-317,
328-333`). Telemetry reports the runtime fraction on every cycling unit
(`runtime_fraction`, alongside `part_load_ratio` and `part_load_factor`).

The ideal path (the timestep at or above 300 s, or variable-speed
equipment) already delivered a continuous capacity; its dispatch contract
is the preceding section's. Measured: the OCHRE parity corpus at 60 s
improves on every fixture (the conditioned zone MAE 0.39 to 0.25 C on
cz2a_gas_furnace_ac_res_wh, 0.34 to 0.25 on cz6b_resistance_res_wh, 0.60 to
0.24 on the 24-hour ResStock fixture); the six goldens are bitwise
unchanged (every golden runs the ideal path at its resolution); the
single-building benchmark (60 s steps) runs about 10 % faster with the
modulating fraction than with whole-step cycling.

## Wet-appliance hot water follows the OpenStudio-HPXML daily volume

The clothes washer's and dishwasher's daily hot-water volume is the
OpenStudio-HPXML v1.12.0 `calc_clothes_washer_energy_gpd` and
`calc_dishwasher_energy_gpd` result (ERI >= 2019A branch,
`HPXMLtoOpenStudio/resources/hotwater_appliances.rb:824-888` and `:607-675`,
fetched from the v1.12.0 tag): the cycles-per-year rule takes the
operational form (`scy = 123 + 61 n_occ` washer, `scy = 91 + 30 n_occ`
dishwasher, FSEC PF-464-15 Eqs. 1 and 3) when `NumberofResidents` is given
and the RESNET HERS Addendum 81 asset form (Eq. 4.2-34 and Eq. 4.2-36a,
split by apartment unit against every other facility type) otherwise; the
label algebra with the `defaults.rb:5523-5530, 5586-5594` defaults turns the
rated kWh into the water term; `UsageMultiplier` scales the annual energy
and the daily volume alike; a negative result clamps at 0; a home with
`NumberofResidents` 0 has neither. `extension/AdjustedAnnualkWh` is the
adjusted annual energy: it is the resolved energy, never the rated input of
a second adjustment (its water term back-derives the label-cycle energy the
adjustment algebra would have produced).

The volume is drawn the way OpenStudio-HPXML draws it: spread over the
`hot_water_clothes_washer` and `hot_water_dishwasher` schedule columns with
the peak flow the column's daily equivalent full-load hours give
(`resources/schedules.rb:1353-1365`), untempered at the water heater's hot
supply temperature (the appliances' WaterUseEquipment carries no target
temperature schedule, `hotwater_appliances.rb:483-497, 520-534`: the full
hot withdrawal, never mixed with cold mains), through the water heater's
scheduled draw beside the tempered fixture draw. The volume converts to mass
at the drawn water's density (`water_density_kg_m3` at the water heater's
hot supply temperature, its setpoint by default). For a home without a
schedule file the generator produces both columns from the appliance's own
weekday, weekend and monthly fractions, the same
`MonthWeekdayWeekendSchedule` object that shapes the electric cycle
(`hotwater_appliances.rb:63-65, 155-157, 480-482, 517-519`), scaled to the
daily water volume instead of the daily energy. Under an unavailable period
the two columns zero on the same steps as `clothes_washer` and `dishwasher`,
through the paired energy-use column's table row
(`schedules.rb` `set_unavailable_periods`, `:1433-1437`). The appliance's
own draw loop on the water heater's demand channel and its fixed per-cycle
volume (15 L washer, 6 L dishwasher) are gone, so the daily volume no longer
depends on the step length.

OCHRE's difference is its older cycle equations: `parse_clothes_washer` and
`parse_dishwasher` (`vendors/OCHRE/ochre/utils/hpxml.py:1243-1278,
1336-1370`) use the pre-Addendum-81 asset equations
(`scy = 164 + 46.5 n_beds`, `dwcpy = 88.4 + 34.9 n_beds`) over the
occupancy-adjusted bedroom count, and the same untempered spread through the
same columns (`ochre/utils/schedule.py:350-370`, `ochre/Models/Water.py:286-290`,
flat 1000 kg/m3 water). The test_ochre_parity suite carries that documented
difference.

Measured direction of the delta (the dwelling goldens, annual appliance hot
water): the parity homes' washer 27.4 to 9.8 L/d and dishwasher 21.1 to
9.7 L/d; bldg0176227's washer 17.1 to 28.2 L/d and dishwasher 31.2 to
14.0 L/d at 900 s; the volumes are identical at 60, 900 and 3600 s where the
old replay drew 39.4 L/d at 3600 s against 17.1 at 900 s. The appliance
electric energies move with the cycles rule; domestic hot water energy moves
with the volumes, and space heating or cooling follows the tank gains.

## Thermostat setpoints resolve by the OpenStudio-HPXML rule

Each HVAC side's thermostat setpoints (heating and cooling separately)
resolve from the first source the OpenStudio-HPXML rule names
(`HPXMLtoOpenStudio/resources/hvac.rb` `apply_setpoints`, master
`57bd1b88`, lines 1641-1741): a schedule file's setpoint column when the
file carries one ("permit mixing detailed schedules with simple
schedules", hvac.rb:1665); otherwise the HPXML's own setpoints
(`get_hvac_setpoints`, hvac.rb:1768-1829), which are the hourly
`WeekdaySetpointTemps*Season` and `WeekendSetpointTemps*Season` extension
arrays when both are declared, and otherwise the constant
`SetpointTemp*Season` element expanded with its setback
(`SetbackTemp*Season` over `TotalSetbackHoursperWeek*` from
`SetbackStartHour*`, hvac.rb:1785-1798); otherwise the default profile,
OS-HPXML's manual-thermostat defaults of 68 F heating and 78 F cooling
(`defaults.rb:2827-2848` `apply_hvac_control`). When a schedule column and
HPXML setpoints are both given, OS-HPXML warns "Both 'heating_setpoint'
schedule file and heating setpoint temperature provided; the latter will
be ignored." (hvac.rb:1669-1675) and the column wins; the warning is an
expected-warning case in OS-HPXML's own validation suite
(`test_validation.rb`, the `schedule-file-and-setpoints` case). HARES
resolves the same precedence and warns for every ignored HPXML input, so
no choice is silent. The schedule-file setpoint columns are degrees
Fahrenheit in OS-HPXML's detailed-schedule convention ("Detailed Schedule
Inputs", <https://openstudio-hpxml.readthedocs.io/en/latest/workflow_inputs.html>;
converted in `resources/schedules.rb:1465-1484`); HARES reads them as
Celsius pending the Fahrenheit reading's own numeric entry.

The active source per side is recorded where the setpoints resolve
(`crates/hares-io/src/setpoint_source.rs`, the equipment spec's
`heating_setpoint_source` / `cooling_setpoint_source`), so the run's
provenance names where each thermostat's setpoints came from: the schedule
column, the HPXML control, or the default profile. OCHRE lets the column
win silently (`vendors/OCHRE/ochre/utils/schedule.py:408-427`); the
warning follows OS-HPXML, not OCHRE.

## Weather reads follow the EnergyPlus interpolation rules

A weather step's values are the EnergyPlus weather rules' values, read
from the vendored EnergyPlus (commit `f7c103167a`) and its published
references. HARES previously resampled weather with PCHIP curves and a
half-hour record shift; both are gone, replaced by the rules below.

### Point values: Last Hour Interpolation

For an hourly file at a step shorter than an hour, every point field is
the linear last-hour interpolation, evaluated for the step's end:
`Value_TimeStep = LastHourValue * Weight_LastHour + ThisHourValue *
Weight_ThisHour`, `Weight_ThisHour = CurrentTimeStep /
NumberOfTimeStepsInHour`, `Weight_LastHour = 1 - Weight_ThisHour`
(EnergyPlus Input Output Reference, "Weather Data Hourly Interpolation",
`doc/input-output-reference/src/weather-data/weather-data-hourly-interpolation.tex`;
the loop at `WeatherManager.cc:3093-3121`, the weights at
`:8343-8347`). Hour 1 is 00:00:01 to 01:00:00, so a record's value is
reached at the end of its hour. The published weights for four steps per
hour are 0.25, 0.5, 0.75, 1.0 on the this-hour value; for six, 1/6 to
6/6. Interpolation runs only when the file has one record per hour and
the run has more than one timestep per hour (`WeatherManager.cc:3043`).

At a one-hour step each step takes the record of the hour it ends in.
The fields that interpolate linearly: dry bulb, dew point, relative
humidity, barometric pressure, wind speed, opaque sky cover and
horizontal infrared (`WeatherManager.cc:3094-3105`). Wind direction
interpolates on the circle with the same weight
(`interpolateWindDirection`, `:3174-3188`). Liquid precipitation is the
weighted hourly depth divided by the steps per hour (`:3119-3120`). Sky
temperature is recomputed from the step's interpolated inputs, never
interpolated itself (`calcSky`, `:3106-3113`). Day boundaries read the
previous day's hour-24 record as last hour; the run's first hour reads
the record the first-hour rule names (below).

### Solar values: the midpoint interpolation

The solar values on the weather file are average values over the hour,
and "For interpolation of hourly weather data (i.e., when the specified
timestep is greater than 1), the average value is assumed to be the
value at the midpoint of the hour" (EnergyPlus Engineering Reference,
"Weather File Solar Interpolation",
`doc/engineering-reference/src/climate-sky-and-solar-shading-calculations/climate-calculations.tex:304-306`).
The weights are EnergyPlus's `SolarInterpolation` table
(`WeatherManager.cc:8343-8375`): for an even number of steps per hour N,
the step ending at the half hour has weight 1.0 on its own hour's
record and the weight falls by 1/N per step each side (N = 4: 0.75,
1.0, 0.75, 0.5); for N = 3 the weights are 5/6, 5/6, 0.5; for odd
N >= 5 the two middle steps take `1 - 1/(2N)`. A step is before the
half hour when `2*ts < N`, at it when `2*ts == N` and after it when
`2*ts > N` (`WeatherManager.cc:3081-3091`). Before the half hour the
remainder goes to the previous hour's record, after it to the next
hour's (`:3074-3091`, `:3113-3116`): linear interpolation between hour
midpoints, evaluated at the end of each timestep. Beam and diffuse are
interpolated this way. After the half hour of hour 24, the next-hour
value is hour 1 of the same day's records, not the next day's hour 1
(`NextHr = (hour == 24) ? 1 : hour + 1`, `:3061-3065`); before the half
hour of hour 1, the previous-hour value is the previous day's hour 24
(`wvarsLastHr`, carried across days, `:3124`), except in the run's
first hour, where the first-hour rule names the record
(`:3050-3056`). The same interpolation is used for repeated warm-up
days (`idd/Energy+.idd.in:1488`).

### First hour, record labels, sub-hourly files

The first hour of the run interpolates from hour 24 of the run's first
day (`Hour24`, the default) or hour 1 (`Hour1`), the config field
`first_hour_interpolation` (EnergyPlus RunPeriod "First Hour
Interpolation Starting Values",
`doc/input-output-reference/src/overview/group-location-climate-weather-file-access.tex:717-721`).
"Note that this option only affects the first hour of the first day in
the entire run period" (`:721`); the code sets the whole previous-hour
record from it once per environment (`WeatherManager.cc:3050-3056`), so
it governs the solar previous hour of the run's first hour too. No step
reads a record before the run's first day.

A record's stamp labels its interval's end for EPW, TMY3 and ResStock
CSV (a record stamped `h` covers `(h - 1 h, h]`), and its interval's
start for PSM3 (a record stamped `t` covers `[t, t + interval)`, so an
hourly PSM3 record stamped `t` is the record of the hour ending
`t + 1 h`). The metadata carries this as the record label; a
sub-hourly file's records are the run's timesteps when the file's
interval equals the step, and the run is a construction error naming
both intervals when it does not, as EnergyPlus refuses the mismatch:
"Number of intervals per hour on Weather file does not match specified
number of Time Steps Per Hour" (`WeatherManager.cc:715-722`). A step
shorter than an hour must divide the hour (EnergyPlus allows only
timesteps per hour evenly divisible into 60,
`idd/Energy+.idd.in:925-936`). An hourly file at a step longer than an
hour keeps the interval mean of the records the step covers (EnergyPlus
has no timestep longer than an hour).

OCHRE shifts every record's stamp by a fixed `timedelta(minutes=30)`
whatever the file's interval (`ochre/utils/schedule.py:169`, applied at
`:110`) and its `resample().ffill()` (`:555`) carries the last stamp at
or before each step: a half-period lag and a zero-order hold. That is
the pvlib midpoint convention, which the EnergyPlus rules above
replace; the OCHRE comparison's own hold file (the adapter's
OCHRE-convention construction) is read directly at its interval.

### Where HARES's weather fields differ from EnergyPlus's

Three places, deliberate:

- **Surface albedo.** EnergyPlus does not carry the file's albedo into
  its per-timestep weather: the value is read only into the missing-data
  defaults (`WeatherManager.cc:2936-2938`, `:3031`) and the
  interpolation loop assigns no albedo. HARES holds the hour's record.
- **Total sky cover.** EnergyPlus interpolates total and opaque sky
  cover (`:3098-3100`); HARES's weather has no total sky cover field
  (`crates/hares-io/src/weather.rs`, opaque sky cover only), so that
  field has no counterpart to interpolate.
- **Global horizontal irradiance.** EnergyPlus keeps no global
  horizontal in its per-timestep weather (the `WeatherVars` set,
  `WeatherManager.hh:713-732`); HARES applies the same midpoint weights
  to GHI as to beam and diffuse, a HARES extension recorded as such.

## Weather is addressed by calendar date: typical and actual years, wrap, Feb 29

A run's steps read weather records by the step's **local standard time
calendar date and hour** (the site's standard offset; daylight saving
shifts schedules, not weather), never by a running index. The config
field `weather_year` selects the EnergyPlus rule: `typical` (the
default) or `actual` (RunPeriod "Treat Weather as Actual", which
defaults to No, `idd/Energy+.idd.in:1480-1484`).

- **Typical.** The file's month, day and hour address the records
  whatever the stamps' years: a run past Dec 31 continues at Jan 1 of
  the file (wrap by day of year, `WeatherManager.cc:929-941`). A file
  whose data period is not a complete year (Jan 1 to Dec 31) is a
  construction error naming the period and the inputs that need a
  complete year: the water-mains annual mean and monthly range and the
  ground temperature the parsers derive from twelve monthly dry-bulb
  means. EnergyPlus runs a partial typical period when the run lies
  inside it (`WeatherManager.cc:941-991`), but its weather-file
  water-mains correlation needs twelve complete months and otherwise
  falls back to a fixed 10.0 C; HARES always derives the mains from the
  weather file and substitutes no value it was not given, so a partial
  file has nothing to run on.
- **Feb 29.** In a non-leap run year a typical file's Feb 29 records
  are skipped (`WeatherManager.cc:2816-2828`, the condition at
  `:2817`). For a leap-year run over a file that has no Feb 29, HARES
  does what EnergyPlus does: Feb 29 is **not simulated** (EnergyPlus
  simulates such a year as a 365-day year: `IsLeapYear` requires the
  file to allow leap years (`:8456`), `LeapYearAdd` is 0 (`:873-876,
  1906`), and the missing Feb 29 is fatal only for actual weather
  (`:2832-2836`)). The run still covers the calendar interval
  `[start, start + duration)`: the clock steps from the last step of
  Feb 28 to Mar 1 00:00, no output row is labelled Feb 29, and every
  later date reads its own calendar date's records. Weather and
  schedules share the one rule (the schedule's own 365-day row
  mapping). A typical file that has Feb 29 records is read by a
  leap-year run, and then no day is skipped. A `start_time` on a
  skipped Feb 29 is a construction error naming the date.
- **Day types.** The day type every simulated date's readers see (the
  schedule's weekday and weekend profiles, the HPXML weekday/weekend
  setpoint profiles) is the date's own **calendar weekday**, including
  after a skipped Feb 29. This is the one weather edge case where HARES
  departs from EnergyPlus: EnergyPlus advances the day of week by one
  per simulated day (`WeatherManager.cc:2856-2859`), so after a skipped
  Feb 29 every later date is one weekday behind its calendar weekday,
  which its own Input Output Reference calls incorrect: "when the
  weather is treated as non-actual, the leap day will be skipped if it
  is not present, resulting in incorrect day types for the remainder of
  the year"
  (`doc/input-output-reference/src/overview/group-location-climate-weather-file-access.tex:597-601`).
  HARES does not reproduce that known defect: a schedule's weekday and
  weekend profiles must fall on the days the calendar says.
- **Actual.** The run's first and last steps must lie inside the
  file's own dated records (the stamps the parser keeps as the file
  states them; an `IntervalEnd` file's dated range runs from its first
  stamp less one interval to its last stamp), or construction fails
  naming both ranges (`WeatherManager.cc:904-926`, fatal at `:990`).
  A run that includes a Feb 29 the file has no records for fails
  construction naming the year (`:888-899`, `:993-996`).

The resolved clock a validate() reports carries the run's simulated
step count (the calendar steps less any skipped Feb 29), so a caller
checks a table's row count against the engine, not against its own
calendar arithmetic. At `tracing` debug, the calendar mapping and day
type are logged at each day boundary where a wrap or leap-day rule
applies.

## Warm-up converges by the EnergyPlus criteria

The run's warm-up repeats the run's first day until the EnergyPlus
warm-up convergence criteria are met
(`vendors/EnergyPlus/src/EnergyPlus/HeatBalanceManager.cc:3309-3430`,
`CheckWarmupConvergence`; the tolerances and day counts are the
`Building` object's fields N2 to N5, `idd/Energy+.idd.in:536-575`), or
until the maximum day count is reached, whichever comes first. After
each warm-up day, for every zone (EnergyPlus compares every zone,
unconditioned included):

- the daily maximum zone air temperature changed from the previous
  warm-up day by at most the temperature tolerance, and the daily
  minimum the same (`:3330-3336`, `:3342-3348`);
- the daily peak HVAC heating delivered to the zone: when today's peak
  exceeds 1e-4 W, both days' peaks are floored at 100 W (`MinLoad`,
  `:3320`) and the relative change must be at most the loads tolerance
  (`:3350-3364`); a peak at or below 1e-4 W passes;
- the daily peak HVAC cooling delivered to the zone, the same rule
  (`:3366-3380`).

The defaults are EnergyPlus's: the temperature tolerance 0.4 delta C,
the loads tolerance 0.04 (a fraction; the IDD gives the field
`\units W` at `Energy+.idd.in:542` but the code uses it as a
fraction), at least 1 and at most 25 warm-up days. A minimum above the
maximum is a configuration error in HARES; EnergyPlus silently resets
the maximum to the minimum with a warning
(`HeatBalanceManager.cc:686-697`). At the maximum without convergence
the run continues: the outcome is recorded on the run health, a
warning names the zones whose checks failed, and a health policy may
escalate the non-convergence to a run failure. EnergyPlus reports its
`ShowSevereError` per unconverged zone and continues
(`:3382-3424`). A non-finite residual is never a comparison result:
it fails the run with `NanDetected` naming the zone and the day.

HARES previously compared only the last step's temperatures of the
conditioned zones between consecutive warm-up days at 0.5 delta C,
never the loads, the daily extremes, the unconditioned zones or a
minimum day count; the doc comment called 0.5 the EnergyPlus default,
which the EnergyPlus sources do not say (the IDD default is 0.4). The
days-run delta of the new criteria against the old rule, measured on
the warm-up fixtures: the conditioned homes that stopped at 2 days now
run 3 (their first day-over-day daily-extreme and load residuals sit
above the 0.4 and 0.04 tolerances the old rule never read), the leap
fixture now converges on day 2 where the old rule took 3, and the
heavyweight BESTEST 900 pair runs 4 days where the old rule stopped at
2 with a 5.2 delta C daily-min residual and a 47 percent load residual
still open. The per-day per-zone residual tables are in the
measurement log.

OCHRE runs a fixed `initialization_time` with no convergence test
(`vendors/OCHRE/ochre/Simulator.py:136-140`): the reference
initializes by simulating forward from the start for the fixed period
and resetting the clock, whatever the residuals. The warm-up's initial
condition feeds every record; a HARES-OCHRE comparison's first
simulated window inherits whatever each side's initialization
produced, so the days-run delta above is a difference between the two
initializations, not a defect of either.

## The billing month closes on the start's day-of-month anniversary

The monthly billing cycle's periods are anniversary-based: the run's
first period opens at the run's start and closes at the start's day of
month in the following month, midnight in the tariff's zone
(`compute_period_end` in `crates/hares-tariff/src/billing.rs`), so a
January 25 start bills January 25 through February 24 into one period.
The anchor day is the first period's start day, carried unchanged across
the closes: each period end is the anchor day clamped to that month's
length, so the chain never drifts. A December 31 start closes January
31, February 29, March 31, April 30 and keeps returning to the 31st;
computing each end from the previous end instead would close February 28
and stay on the 28th forever. A mid-run tariff attach joins the period
the anchored chain holds open at the attach instant.

A step's energy bills to the period whose span holds it: the dwelling
hands the tariff evaluator the step's end time, and the evaluator folds
the step in, then closes its period when that time has reached the
period end. The step ending at a period boundary is the closing period's
last step; the step starting there books to the period it opens. A
tariff attached exactly at a boundary opens the new period, and the
boundary step books to it on a mid-run attach and on a from-start run
alike: both routes agree, and every kWh is billed exactly once. A period
closed before its own end bills its fixed and demand charges prorated to
the span the tariff served, per the replacement and finalize rules.

The vendored OCHRE computes no utility bills, so this number has no
OCHRE comparison. Direction of the delta: no golden fixture's numbers
move (the one tariffed golden closes no billing boundary inside its run);
a run that spans a cycle boundary moves the boundary step's energy, cost
and peak from the closing period's summary to the opening one's, with
the totals across the periods unchanged.

## A not-applicable output value is null, never 0.0

An output column's cell reads null wherever the underlying quantity does
not apply to the current state, so the consumers' statistics (means,
sums) are not poisoned by zeros-as-data. The catalogue decides, per
variable, what a missing value means (`VariableDef.nullable`,
`crates/hares-io/src/output/catalogue.rs`), and every variable's
nullability is logged at catalogue build (`tracing` at `debug`):

| Variable class | Nullability | What a missing value means |
|---|---|---|
| Electric power, gas power, energy, capacity (thermal output), reactive power, backup resistance power (ER), fan power, main power, runtime fraction, latent gains, the on/off schedule state, the end-use aggregates | not nullable | physically zero when the component is inactive: the producer writes 0.0; a declared channel's absence is a wiring defect that fails the run (OR-5) |
| Operating mode, setpoint, SOC, COP, sensible heat ratio, speed index, defrost state, power factor at zero apparent power (was the 1.0 sentinel), the scheduled and runtime setpoint columns before the first reconcile, every actor telemetry field, every verbosity-8 pass-through field as its `TelemetryField` declares | nullable | a missing value is null in the report |
| Summed cells (duct losses, end-use aggregates) | nullable | a zeroed accumulator per step; null only when the dwelling has no contributing equipment, which the catalogue knows at assembly |

The producers that publish `None`: the heat-pump heater, the heat-pump
cooler and the air conditioner write COP `None` when the step's runtime
is 0 or the mode is Off (the ratio is undefined without compressor
input); the cooling equipment writes the sensible heat ratio only when
the step's sensible cooling is above 0 (the init insert of 1.0 is
removed). Metrics and aggregation: a sum skips nulls; a mean divides by
the count of non-null cells and the count is reported as the variable's
companion valid-count variable; a period or fold with no non-null cell
is null.
## A fleet's Enum column folds to the weighted operating Mode

A fleet's `Enum`-kind columns (the per-equipment operating modes, the
compressor speed, the EV connection state and charging level, an actor's
declared enum channel) fold by the weighted Mode in both fleet stages:
each home's over-time reduction takes the period's most frequent code,
ties to the lowest code, exactly the reporting reducer's Mode rule
(`crates/hares-io/src/output/reduce.rs`), and the across-homes fold takes
the code with the greatest summed home weight, ties to the lowest code.
A mean over mode codes is never taken: the codes are labels of an
enumeration, not quantities, and their mean is no code. A period whose
steps visit codes 0, 0, 0 and 2 reduces to 0, not to the mean 0.5; a
two-home fleet whose modes disagree folds to the code with the greater
summed weight, never to the average of the two codes. A home whose mode
cell is null does not vote, and a cell no home voted on is null.

Before this rule the fleet's unit-keyed rules averaged the mode columns
over time and across homes: the mode columns' unit is the dimensionless
`-`, so they took the mean through the dimensionless entry, and the EV
code columns fell through their name suffix to the mean as well. The
`Enum` column's field now carries its label count in the schema metadata
(`enum_labels`, written beside the unit by the schema builders), which is
what feeds the Mode rule's per-code counting in both stages.

Binary state columns (`VariableKind::State`: the on/off schedule column,
the defrost cycle state) keep the weighted mean, which is the duty
fraction, not the Mode.

OCHRE has no fleet fold to compare with: the fleet aggregation is HARES's
own surface, and this rule's only reference is the declared aggregation
of the catalogue itself. Direction of the delta: no golden moves (the
fleet goldens' verbosity-2 selection holds no `Enum` column, so every
golden compares bitwise); a fleet run at a selection that holds mode
columns moves those columns' cells from the mean of the mode codes to
the weighted Mode, carried by the acceptance tests.
