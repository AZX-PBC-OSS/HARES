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

## Shielding follows the facility type

When the HPXML declares no `<ShieldingofHome>`, the shielding class is
OpenStudio-HPXML v1.12.0's facility-type default
(`defaults.rb:822-830` `apply_site`, documented at
workflow_inputs.rst:480): well-shielded for an apartment unit or
single-family attached home, normal for single-family detached,
manufactured and anything else. The element's allowed values are
`normal`, `exposed` and `well-shielded`; a value outside that list is a
parse error naming the element, the value and the allowed values. The
building with neither `ShieldingofHome` nor `ResidentialFacilityType` is
an error naming `ResidentialFacilityType`: the element is a required
input of the reference (workflow_inputs.rst:622, "Yes" in the required
column), and OS-HPXML's own code path for the missing facility type (its
`else` branch, normal) is one its input validation prevents. The
direction of the delta: none on the committed runs (the facility-type
rule has run since the phase-1 fold carried it; the error arm moves no
run that built before).

## The infiltration height follows the OpenStudio-HPXML estimate

When the HPXML's air infiltration measurement declares no
`<InfiltrationHeight>`, the AIM-2 model runs at OpenStudio-HPXML v1.12.0's
estimate, ported from `HPXMLtoOpenStudio/resources/defaults.rb:5789-5856`
`get_infiltration_height_and_volume` (applied at `defaults.rb:1171-1174`,
documented at workflow_inputs.rst:1023, "estimated from other inputs
(e.g., ConditionedFloorArea, NumberofConditionedFloorsAboveGrade,
attics/foundations with WithinInfiltrationVolume=true, etc.)"): the
average ceiling height per conditioned floor above grade, plus half a
foot of rim joist and subfloor per added storey above the first
(defaults.rb:5804), plus the tallest attic and the tallest above-grade
foundation height within the infiltration volume, the foundation term
raised by its rim joist and sill plate (9 in, ASHRAE 140,
defaults.rb:5834-5835) and dropped for a walkout basement
(defaults.rb:5845-5848); a foundation with no FoundationWall element
contributes the assumed zone height instead
(`geometry.rb:1352-1359` `calculate_zone_height`'s assumption, since
HPXML's Wall elements carry no height: 8 ft for a basement or a garage,
3 ft for a crawlspace). HARES's previous default was the average ceiling
height times the floors above grade, which drops the foundation and
attic terms. The estimate reads the engine's own ceiling height, so a
building whose terms are all zero keeps its height on bits. The
direction of the delta: up on the fixtures that lack the element
(base.xml 2.4384 to 2.9718 m, the estimate's 9.75 ft, the value OS-HPXML
v1.12.0 defaults for the same file,
`HPXMLtoOpenStudio/tests/test_defaults.rb`
`test_infiltration_height_and_volume`); every golden's HPXML declares
`InfiltrationHeight`, so no golden moved. OCHRE takes the height the
HPXML carries (its `hpxml.py` infiltration section) and adds none of its
own; the estimate is OS-HPXML's defaults rule, which HARES now follows.

## The flue flag follows the OpenStudio-HPXML rule

When the HPXML's air infiltration measurement declares no
`<HasFlueOrChimneyInConditionedSpace>`, the flag is OpenStudio-HPXML
v1.12.0's rule, ported from `defaults.rb:5096-5132`
`get_flue_or_chimney_in_conditioned_space` (applied at
`defaults.rb:3418-3421`, documented at workflow_inputs.rst:1147-1155):
true when a non-electric heating system of the atmospheric types
(Furnace, Boiler, WallFurnace, FloorFurnace, Stove, SpaceHeater) sits in
conditioned space with an AFUE or percent efficiency below 0.89; true
when a non-electric fireplace sits in conditioned space; true when a
non-electric water heater sits in conditioned space whose energy factor,
or the equivalent calculated from its uniform energy factor
(`resources/waterheater.rb:1557-1577` `calc_ef_from_uef`), is below
0.63; false otherwise. HARES's previous default was false always. The
direction of the delta: the flag can only rise, and only on homes whose
atmospheric systems or non-electric water heaters qualify; every
committed golden's HPXML declares the element, so no golden moved.

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

## A declared assembly R-value sets the resistance of the LUT layers

For an opaque boundary matched to an envelope LUT row, the declared
`AssemblyEffectiveRValue` (or the older `RValue`) sets the construction's
resistance: the row selection is unchanged (the construction and finish
filters, then the row whose `Assembly R Value` plus the convention films
is nearest the declared R), and every layer resistance of the matched row
is then multiplied by `k = (declared R - convention films) / sum(row
layer resistances)`, so the layers plus the convention films equal the
declared R exactly. The row's layer capacitances are kept: they are mass,
not resistance. A declared R between two rows now moves the resolved
resistance continuously instead of leaving it at the row's until it
crosses a row boundary - the declared resistance is the boundary's
resistance, not a staircase over it. The envelope scales compose after
it: a layer's resolved resistance is the row's times `k` times
`opaque_r_scale`, its capacitance the row's times `thermal_mass_scale`.

The films in the equation are the row-selection convention's films (the
constants the LUT's row match adds to the row's assembly R: 0.1585
m²·K/W for walls and roofs, 0.2642 m²·K/W for floor/ceiling boundaries),
because that is the convention the declared R speaks: HPXML's assembly
R-value includes the standard films, and the row selection already
measures both the row and the declaration against the same constants.
HARES's own film resistances are convection-only (the radiative part
rides the interior LWR network), and their weather-computed sums exceed
legitimate declared assembly R values - an uninsulated roof at R-2.3 IP
(0.405 m²·K/W) sits below its convection-only film sum on the reference
corpus - so the network's films cannot be the measure of either the
scaling or the error below. The boundary's total conduction resistance is
the declared R in the declaration's convention plus the film-convention
difference.

A declared R at or below the convention film sum leaves no material
resistance: it is a construction error naming the boundary, the declared
R and the film sum. The clamp the old code applied
(`(r - r_film_int - r_film_ext).max(1e-6)`) would otherwise scale the LUT
layers to almost no resistance without a word. Slab boundaries (the
F-factor perimeter method), boundaries with explicit material layers and
boundaries without a declared R are unchanged.

OCHRE uses the LUT row as is: the declared `AssemblyEffectiveRValue`
selects the row (`get_boundary_rc_values`, the nearest-row match HARES
implements) and the row's precomputed layer resistances are used
unchanged, so the declared resistance is inert between rows. HARES's
scaling is a deliberate deviation, recorded here; the calibration
consumer scales the declared R (a continuous knob) rather than the
LUT's `Resistance (m^2-K/W)` column, which the `k` normalisation makes
inert.

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
converted in `resources/schedules.rb:1465-1484`), and HARES reads them the
same way: the parser converts every value of a `heating_setpoint` or
`cooling_setpoint` column from Fahrenheit to Celsius at the parse, once
per value, with the faithful formula `(t - 32.0) * 5.0 / 9.0` (the
`uom` conversion reached Celsius through the kelvin base with rounded
offset constants and landed ulps off the formula's exact value; the
Fahrenheit setpoint test's independent bit pin found it), so every
reader downstream sees one column of one
unit. The conversion is unit-for-unit: OS-HPXML's `convert_setpoints`
applies the on-off thermostat's deadband offset after its own conversion,
and HARES's deadband is the thermostat's own configuration, not a
schedule-column property; OCHRE converts the same columns from
Fahrenheit at its schedule load
(`vendors/OCHRE/ochre/utils/schedule.py:408-427`) with no offset. The
schedule-file setpoint columns are not pair-reconciled: OS-HPXML's own
semantics separate the sides by the on-off `offset_db` (the deadband
offset folded into the simple-setpoint profiles,
`hvac.rb:1809-1810`, and the thermostat's cutout difference,
`hvac.rb:1746`; its setpoint-adjustment loop runs only when both sides
resolve from HPXML setpoints, `hvac.rb:1690`), while OCHRE reconciles
the schedule columns at load, clipping every step whose cooling and
heating setpoints sit within 1 C of each other to half a degree around
their mean (`vendors/OCHRE/ochre/utils/schedule.py:620-629`); HARES
keeps the columns as written and records the divergence from OCHRE. The
direction of the change: a schedule file written for the HPXML ecosystem
(previously read as Celsius) drives cooler: a 70 F heating column reads
21.11 C, not 70 C, so heating energy and mean zone temperature fall on
any home whose schedule file carries setpoint columns; no committed
golden carries one, and no golden moved.

The active source per side is recorded where the setpoints resolve
(`crates/hares-io/src/setpoint_source.rs`, the equipment spec's
`heating_setpoint_source` / `cooling_setpoint_source`), so the run's
provenance names where each thermostat's setpoints came from: the schedule
column, the HPXML control, or the default profile. OCHRE lets the column
win silently (`vendors/OCHRE/ochre/utils/schedule.py:408-427`); the
warning follows OS-HPXML, not OCHRE.

## A constant setpoint override replaces the resolved setpoint sources

A dwelling-level equipment override that sets a constant setpoint
(`setpoint.heating_setpoint_c` or `setpoint.cooling_setpoint_c`) replaces
the setpoint source the resolution stamped for that side (the schedule
column, the HPXML control's setpoints, or the default profile): the merge
removes the matching `*_setpoint_source`, so the constant is the tier the
thermostat reads. OpenStudio-HPXML has no equipment-override channel; the
override is HARES's configuration surface, and "accepted means applied"
(the engine's own rule) is what makes it a setpoint the thermostat
follows rather than a value the sources outrank. The equipment init's
deadband reconciliation never moves a side the config names a constant
for: the unnamed axis opens the gap, the same named-axis rule the
thermostat's control signals follow, and a violation with both sides
named is a construction error, since named constants never move and
there is no unnamed axis to open the gap on. OCHRE's thermostat resolves its
heating and cooling setpoints from its schedule and control signals the
same way every step (`vendors/OCHRE/ochre/Equipment/HVAC.py`), with no
constant-override tier; the record of the active source names the
override when it applies. The direction of the change: a run that sets a
constant setpoint override moves from the resolved tier to the constant
(the acceptance run's heating setpoint 20 C to 25 C, the delivered
heating and gas power up with it); no committed golden sets one, and no
golden moved.

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

## A fleet's `Time` labels are the local site time

The fleet's aggregate and covered-weight rows are labelled at each
period's start in **local site time**, as a naive Arrow
`Timestamp(Millisecond, None)`: the same convention every per-dwelling
output already follows (`03-output-reporting.md` OR-7), so the fleet's
`Time` is no longer the one output that is neither naive nor a
timestamp. The rule (`decision-log.md` `dl-i08-architecture.time` and
`.fleet`): a single-offset fleet's labels are the homes' own local wall
clock; a multi-offset fleet declares its alignment, and `LocalClock`
labels by the local wall clock each folded home's step in the row
starts at while `Instant(offset)` labels by the declared reference
zone's wall clock. The schema metadata records the offset, the homes'
shared site offset or the declared reference zone, as
`hares:utc_offset_s` (adding it to a label gives the UTC instant);
under `LocalClock` no single offset applies and none is recorded.

Before this rule the fleet's `Time` was a Utf8 RFC 3339 string: an
undeclared fleet and `Instant(+00:00)` rendered each row's start
instant in UTC with a `Z` suffix (a UTC-5 home's local midnight read
`2018-01-01T05:00:00Z`), `Instant(offset)` rendered the instant at the
declared offset, and `LocalClock` rendered the naive local period start
as text: labels, not timestamps, and UTC-shaped whatever the homes'
local time.

OCHRE has no fleet aggregation to compare with: the fleet's output
surface is HARES's own, and the per-dwelling convention it now follows
is the naive local standard time the engine reports everywhere else.
Direction of the delta: the three fleet goldens' `Time` labels move
from the RFC 3339 strings to the naive local timestamps in the
`aggregate` and `covered_weight` products (the `homes` and
`fleet_report` products carry no `Time` column and are unchanged); no
energy, power or weather number moves.

## The defrost penalty is applied by one model

A heat pump's defrost penalty exists in the engine as two models of the
same physics: the continuous EnergyPlus ERM 26.1 on-demand model (the
capacity multiplier `0.875 * (1 - tf)`, the melt term, the averaged
defrost power, OCHRE's implementation, `HVAC.py:1141-1166`) and a
HARES-specific discrete frost cycle that resolves the same penalty into
explicit events (a zero-capacity defrost of `cycle_duration_s` firing
once per `cycle_duration_s / tf` seconds). Both were applied at once: the
continuous multipliers on every frost-condition step and the
full-intensity events on top, so a cold-climate ASHP carried the penalty
twice (the composed run delivered about one point of capacity less than
the continuous model alone at near-freezing conditions and metered about
2.1x the continuous model's defrost electric, because the discrete
trigger also under-fired its own documented duty).

The rule now: the discrete frost cycle owns the penalty and the
continuous model feeds only the frost accumulator. The cycle's
Accumulating phase carries the bare frost derate (0.875) and the melt
term without the continuous model's duty factor - the duty is the cycle's
own events - and its Defrosting phase draws the continuous average
concentrated into the event window (the draw is `extra_power_w / tf`), so
the cycle's averages reproduce the continuous model's: the discrete and
continuous-alone runs deliver the same heat within 1 percent and meter
the same defrost-electric integral. The cycle trigger honors its
documented duty: the trigger fires at
`cycle_duration_s * (1 - ewma_time_fraction)` frost-seconds, which gives
one `cycle_duration_s` event per `cycle_duration_s / tf` seconds (duty
tf), and the EWMA persists across events so no cycle re-ramps the
threshold. Disabling the discrete cycle (`defrost_discrete_cycle: false`)
selects the continuous model alone.

OCHRE runs the continuous model only. The discrete cycle remains a HARES
extension (the docs call the resolution out); its averaged behavior is
now the continuous model's within its step resolution, and at timesteps
coarse against the 210 s event the event occupies whole steps, which
quantizes the realized duty upward. Direction of the delta: the
ASHP-bearing goldens re-captured (`fleet_mixed_offset`,
`leap_year_february_900s`, `weather_wrap_900s`, `full_year_900s_dst`);
the defrost electric integral fell to the continuous model's own, and the
48 h OCHRE thermal trace's heating fell 31.37 to 28.96 kWh with the
overnight divergence against OCHRE tightening 1.23 to 0.78 C.

## A gas boiler's flue loss leaves outdoors, the jacket share heats the zone

The boiler's combustion residual (fuel input minus useful output) is
split by the shared `combustion_loss_split` (the gas water heater's
flue-versus-delivered pattern, generalised to the two destinations the
residual has): the flue share (`flue_loss_fraction`, default 0.9) leaves
outdoors and the jacket share (the complement) heats the boiler's
surrounding space. Before, the entire residual was routed to the zone as
jacket loss, which manufactured free heat: a non-condensing boiler at
AFUE 0.80 has a residual near 20 percent of its fuel input, so about a
sixth of the burned gas came back as zone heat. ASHRAE puts a boiler's
jacket loss at a few percent of the fuel input; the 0.9/0.1 split of the
residual puts the jacket there.

OCHRE's gas boiler is a stub and EnergyPlus's `Boilers.cc` loses the
stack to its air node; the gas water heater's split
(`flue_loss_fraction`, default 0.10 of the combustion heat, outdoors) is
the pattern the helper extracts. Direction of the delta: the heating gas
in boiler-bearing fixtures rises (the flue share is no longer free zone
heat) and the zone's free heat falls; `fleet_mixed_offset` re-captured.
## Equipment configuration fields are stored in SI

Every equipment config field that carries a power, an energy or an
apparent power is stored in SI and named for its unit: `capacity_w`,
`inverter_capacity_w`, `max_charge_w`, `max_discharge_w`,
`inverter_capacity_va` (an apparent power, dimensionally W), `capacity_j`,
`max_charging_power_w`, `v2l_max_discharge_w`, `v2g_max_discharge_w`,
`power_limit_w`, `fuel_economy_j_per_m`, `charger_capacity_va`,
`rated_power_w`, `capacity_min_w`, `grid_import_limit_w`,
`export_limit_w`, `delta_w_per_s`, `crankcase_heater_w` and
`zone_capacitance_j_per_k`; the loads' raw keys carry the same rule
(`active_power_w`, `phase_<n>_power_w`, `power_profile_max_w`,
`power_constant_w`, `event_power_w_series`). Every value that enters a
config in another unit is converted once, with `uom`, at the boundary
where it is read: the HPXML resolvers (a document value in W enters with
no conversion; a value in kW or kWh converts once), the schedule
resolver, the defaults loaders (the external data files keep their own
unit columns), the catalogue fills (the compiled-in catalogues keep
datasheet values as data and convert in their fill methods), the
override merge and the Python constructors. An equipment that still
computes in kW converts the SI config value once at construction. OCHRE
stores these fields in kW and kWh; the boundary to it converts once.

The rule removes the per-equipment hand-written conversion factors and
the caller's need to know which fields are not SI: every config a caller
builds or reads is SI, and the read-back reports the resolved parameters
directly from the SI fields.
## PV roof attachment and coverage follow the envelope's own orientation

The roof a PV array shades is decided once at assembly, from the
orientation the envelope already registered for that roof's solar gains
(the environment's surface geometry: tilt, azimuth and the
omnidirectional flag), never from the PV code's own defaults. The
matching rule: a roof with an azimuth matches a PV within 15° of azimuth
and 10° of tilt, the closest by summed difference; an omnidirectional
roof (the envelope registers an exterior roof with no azimuth as
omnidirectional, azimuth-averaged Perez irradiance) has no azimuth to
compare, so it matches by tilt alone, every PV within 10° of its tilt
attaching whatever the PV's azimuth, and the arrays' coverages sum on
that roof. An equidistant match picks the first roof in boundary
order: the candidates scan in the building's boundary order and a
running best is replaced only on a strictly smaller sum. A PV without
a tilt or an azimuth is an assembly error naming
the PV and the HPXML element (`ArrayTilt`, `ArrayAzimuth`): OpenStudio-
HPXML requires both (`EPvalidator.sch`, through
`defaults.rb` `apply_pv_systems`). A PV that matches no roof attaches to
none, and the run's warning log names it and the nearest roof's azimuth
(when it has one) and tilt differences.

The coverage an attached array registers on the roof is its collector
area over the roof's area, and the collector area per watt of DC
nameplate is a named constant, `COLLECTOR_AREA_PER_W` = 2/420 m²/W: a
420 W panel presents 2 m² of collector: a 21 %-efficient module at
standard test conditions (1000 W/m²; 420 W over 0.21 × 1000 W/m² is
2.0 m²). The efficiency is PVWatts v8's Premium
module assumption, not an assumption of 20 % and not PVWatts's
default: the PVWatts v8 area-free capacity form divides the DC
nameplate by the module's STC efficiency (SSC `cmod_pvwattsv8.cpp`,
`module_m2 = dc_nameplate / stc_eff / 1000`), and its `module_type`
defaults to 0, Standard, at 19 %, under which a 420 W module presents
2.21 m²; the three assumptions in the source are Standard 0.19,
Premium 0.21 and thin film 0.18, and no 20 % is among them. The
constant is HARES's own pairing of a 420 W module with the 2 m² its
premium efficiency needs. A roof whose summed coverage exceeds 1 is
an assembly error naming the roof, its area and the PVs on it;
nothing clamps it silently. The boundary is on bits: the check
compares the sum against 1 with no tolerance, so a summed coverage of
exactly 1 assembles and a sum a single ulp above 1 fails, the
fail-fast error naming the roof, its area, the PVs on it and their
summed coverage.

OCHRE does not couple its PV to the roof's solar gains at all: the
envelope model carries no PV shading, and a PV without a tilt or an
azimuth takes the roof closest to south with preference to west
(`PV.py` `__init__`, `(roof_data["Az"] - 185).abs().idxmax()`), or the
site latitude and south when the model has no roof. HARES shades the
matched roof by `(1 - coverage)` because its envelope solves the roof's
heat balance explicitly; the attachment rule above keeps the array and
the shaded surface on the same orientation. Direction of the delta: a
covered roof's solar gains fall on the sunlit steps; at this commit no
golden fixture's numbers move (the attachment and coverage values are
bitwise those the previous bag-key plumbing computed; the entry that
introduced the typed-config read recorded the proof in the measurement
log).
## Unavailable periods apply to the end uses their table rows name

OpenStudio-HPXML's unavailable periods
(`/HPXML/SoftwareInfo/extension/UnavailablePeriods/UnavailablePeriod`,
v1.12.0) are applied with the reference's semantics: the four period
types (`Vacancy`, `Power Outage`, `No Space Heating`, `No Space
Cooling`) become per-step flags on the run's calendar schedule rows, by
the half-open rule (begin month/day at the begin hour to end month/day
at the end hour, both dates inclusive as days, an end before the begin
wrapping through the year end), and every schedule column whose row in
`resources/data/unavailable_periods.csv` the period applies to zeroes
on the flagged steps, for a schedule file and a generated schedule
alike (`resources/schedules.rb` `set_unavailable_periods`, the two-step
column lookup that remaps the hot-water columns to their energy-use
rows). A schedule file's own four flag columns must match the periods
on the file's own resolution: HARES reads the columns and ignores
nothing.

The design level of a column is computed from its mean over the steps
no applying period flags (the pre-period column), because
OpenStudio-HPXML computes every design level and peak from the
schedules before the periods are applied (`schedules.rb` `:1242,
:1296`); the two agree when the file's unflagged values are the
unmodified schedule, as in an OpenStudio-HPXML export.

The setpoint-driven end uses gate at the equipment through the step's
flags, beside `bus_energized`: space heating and cooling duty forced to
0 with the thermostat setpoints unchanged (`resources/hvac.rb:42-71`,
`schedules.rb:1438-1441, 1455`); the water heater's control setpoint
held at 2 C for the step, freeze protection and not off, so standby
losses continue (`schedules.rb:673-676, 1452-1454`,
`resources/waterheater.rb:18, 183-188, 2053`); a battery or an EV idle
through the step; mechanical ventilation off under its row; the
dehumidifier off under its row (`dehumidifier`, a `Power Outage` only:
the zeroed availability schedule stops the unit, the humidistat
setpoints untouched and its control resumed after the step). The loads
that follow a schedule column or a profile are gated by the table row
their class declares, so a profile the columns cannot reach stops with
them. PV and generators are in no row and keep running: an HPXML
`Power Outage` period is the reference's end-use convention (the
occupants stay too), and it does not de-energize the bus; the physical
outage model (`Dwelling::set_grid_voltage`,
`docs/outage-behavior.md`) is unchanged.

The natural-ventilation availability rides every period whatever its
type (`resources/airflow.rb:337, 570-597`): `always unavailable`
closes the step's natural ventilation, `always available` and `regular
schedule` leave it. Direction of the delta: less heating, cooling and
occupant energy on the flagged days, never a renormalized remainder.

## The run state is held in SI units

The run state — billing accumulators, metrics totals and peaks, equipment
state and checkpoints, the electrical ports and summary, the per-step
results and the telemetry under every reporting key — is held and
computed in SI: watts, joules, VAR and joules per metre. Every quantity
leaves the engine's objects in SI, and each reporting surface converts
once at its own boundary: the output columns convert once in the row
plan (`Unit::from_si`, the kW/kWh/therms-per-hour cells the reports
name), the billing summaries convert once at their close, the metrics
records convert once at `finish`, and the HELICS/gym surfaces publish SI
with their own units. OCHRE holds the same quantities in kW and kWh; the
boundary converts once.

Two units `uom` 0.38 lacks are defined in `hares-types::si_units` with
`uom`'s exported `unit!` macro, at the factors HARES's conversion
constants already carried (so the move is bitwise neutral):

- the `therm` (energy) = 100 000 × `uom`'s `btu_it` = 105 505 600 J.
  `uom`'s `btu_it` is the rounded 1055.056 J, so this therm is 14.738 J
  (1.3969e-7 relative) larger than OpenStudio-HPXML v1.12.0's
  (1055.05585262 J per Btu); the gas therms/hour columns carry that
  relative move. Adopting the exact OpenStudio-HPXML constant is a
  numeric change of its own commit.
- the `btu_it_per_hour` (power) = 0.293 071 07 W (NIST), the value
  `BTU_PER_HOUR_TO_WATT` held before the move.

The scheduled load's gas schedule enters in W (`gas_constant_w`,
`gas_profile_max_w`); the parent's therms-per-hour raw keys are removed
and name their replacement.

## The control layer is held in SI units

The control layer - actor parameters, actor state and telemetry, and the
control signals and power setpoints - is held and computed in SI, under
the same rule as the run state: every quantity is stored under a name
that ends in its SI unit (`capacity_j`, `max_charge_w`,
`fuel_economy_j_per_m`, `average_speed_m_per_s`,
`daily_drive_distance_mean_m`, `range_anxiety_distance_m`,
`away_charge_power_w`, `surplus_deadband_w`; the signal fields
`active_power_w`, `reactive_power_var`, `max_power_w`,
`ramp_rate_w_per_s`, `var`, `energy_j`, `power_w`), and every value that
enters in another unit is converted once, with `uom`, at the boundary
where it is read: the actor registry's parameter checks and fills, the
Python builders (the catalogue's datasheet figures convert once), the
OCHRE `P Setpoint` mapping, and the HELICS/gym/protocol-bridge signal
bodies. The demand-response action string carries an explicit unit
(`PowerLimit:<max>W`); a bare number is rejected. The published actor
variables keep their kW reporting spellings (`pv_kw`, `load_kw`,
`charge_kw`, `bms_pv_actual_kw`): a declared variable name is a
reporting name, the value under it is SI, and the row plan converts
once. The EV driver's behavioural defaults are named SI constants
(`ev_driver::defaults`) written from the documented values (30 mi,
30 mph, 20 mi, 6.6 kW); no value moved.

## Natural ventilation keeps the fixed geometry discharge coefficient

The natural-ventilation model in
`crates/hares-physics/src/infiltration.rs`
(`natural_ventilation_flow_m3_s`) is a stated hybrid of three sources,
and the discharge coefficient stays the ASHRAE fixed geometry values
(single-sided 0.20, cross 0.60), not EnergyPlus's AutoCalculate
correlation.

The three sources:

- EnergyPlus `ZoneVentilation:WindandStackOpenArea` supplies the
  structure: the wind and stack flow components computed separately and
  combined in quadrature (`VVF = sqrt(Qw² + Qst²)`,
  `ZoneEquipmentManager.cc`'s WindAndStack block, v24.2.0 lines
  5716-5806), and the auto opening effectiveness (Cw: 0.55 for
  perpendicular wind, linear to 0.30 at 45°, zero leeward). HARES's
  `compute_natural_ventilation_cw` implements the same auto-Cw exactly.
- ASHRAE HoF 2009 Ch. 16.14 supplies the discharge coefficient, fixed by
  opening geometry: single-sided openings 0.15-0.25 (turbulent buoyancy,
  no well-defined pressure head; HARES holds the midpoint 0.20),
  cross-ventilation 0.60-0.65 (HARES holds 0.60).
- OCHRE supplies the comfort gating: the three flow conditions and the
  adjustment factor `adj = (T_zone - T_base)/(T_zone - T_outdoor)`
  clamped to [0, 1], and the 20-ACH cap
  (`Envelope.py`'s `_natural_ventilation`, lines 28-55, itself ResStock's
  `apply_natural_ventilation_and_whole_house_fan`).

The decision - why not E+'s auto Cd
(`Cd = 0.40 + 0.0045|T_ext - T_mat|`, the `DiscCoef` AutoCalculate arm
of the same E+ block):

1. E+'s auto correlation is geometry-blind. The `WindandStackOpenArea`
   object takes the discharge coefficient as an input and offers the
   correlation as the default for objects that specify nothing; the
   ASHRAE chapter the model cites gives the coefficient BY GEOMETRY
   (single-sided versus cross), which is the physics the opening type
   carries. Adopting the auto value erases the single-sided/cross
   distinction the module models: at a 6 K delta-T the auto Cd (0.427)
   more than doubles the single-sided stack flow (0.20 -> 0.427) and cuts
   the cross flow by about 30% (0.60 -> 0.427).
2. OCHRE - the parity reference whose gating this module adopted - uses
   a fixed coefficient itself: the ResStock convention applies a flat
   0.6 multiplier to the window area (`Envelope.py:44`,
   `area = open_window_area * 0.6`), with no auto correlation anywhere.
   The cross leg's 0.60 is exactly that value; the single-sided leg
   refines it with the ASHRAE single-sided physics. Adopting E+'s auto
   Cd would move the cross leg off the OCHRE-aligned value the gating
   was adopted alongside.
3. E+'s own literal for the gravitational acceleration in the same block
   is 9.81; HARES holds the standard 9.80665. The divergence pin holds
   both literals' effect.

The docstring's former claim ("Implements the EnergyPlus
`ZoneVentilation:WindandStackOpenArea` model") was wrong by omission -
the gating and the discharge coefficient are not E+'s - and now names
the hybrid. The magnitude is pinned:
`infiltration::tests::natural_ventilation_pin_matches_the_recorded_divergence_vector`
ports the E+ AutoCalculate arms verbatim (from the upstream source,
including the 9.81 literal), pins HARES's reference-case vector at 1e-9,
pins the port's own vector, and holds the stack terms' ratio - the
recorded divergence's magnitude. The pin ran at the parent in its
agreement form (HARES == the E+ port at 1e-9) and failed: HARES
0.5779 m³/s against the port's 0.8465 m³/s on the reference case
(a 0.5 m² south-facing cross-ventilated opening, perpendicular 3 m/s
wind, 26 °C zone over 20 °C outdoor, 22 °C comfort base) - the
disagreement the decision records.
