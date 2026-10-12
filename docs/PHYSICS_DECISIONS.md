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
