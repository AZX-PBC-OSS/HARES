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
