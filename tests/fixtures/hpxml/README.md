# HPXML Fixtures

Curated HPXML fixtures for parser development and tests.

## Source

These files are copied from the vendored OCHRE sample corpus:
`vendors/OCHRE/test/OS-HPXML Sample Files`.

Primary upstream project: <https://github.com/NREL/OpenStudio-HPXML>

## Curated Set

- `ochre_samples/base.xml`: baseline single-family building.
- `ochre_samples/base-enclosure-garage.xml`: garage/enclosure topology.
- `ochre_samples/base-enclosure-windows-physical-properties.xml`: detailed window properties.
- `ochre_samples/base-foundation-basement-garage.xml`: foundation + garage zones.
- `ochre_samples/base-pv.xml`: PV system representation.
- `ochre_samples/base-battery.xml`: battery representation.
- `ochre_samples/base-pv-battery.xml`: combined PV + battery.
- `ochre_samples/base-lighting-mixed.xml`: lighting groups/types.
- `ochre_samples/base-appliances-dehumidifier.xml`: appliances + wet loads + misc loads.
- `ochre_samples/base-misc-loads-large-uncommon.xml`: misc/plug loads including EV charging.

## Intent

Use this curated set to validate:

- HPXML structure ingestion.
- SI unit normalization.
- Supported model mapping for building/zones/surfaces.
- Supported end-use/equipment mapping (PV, Battery, EV, plug/light/wet loads).
