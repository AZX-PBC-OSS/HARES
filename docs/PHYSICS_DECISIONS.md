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
