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
