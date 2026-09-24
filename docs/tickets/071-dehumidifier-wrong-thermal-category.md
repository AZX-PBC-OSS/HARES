# Dehumidifier Written as InternalGain Instead of HVAC Category

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-equipment, hares-types
**Dependency**: Ticket 074 (HPWH category mismatch) depends on this ticket's `HvacDehumidification` variant. Ticket 072 (latent-by-category breakdown) benefits from this ticket landing first so the new category is immediately trackable in the per-category latent array.

## Problem

The dehumidifier writes its thermal port contributions under `ThermalCategory::InternalGain` at `dehumidifier.rs:338`. A standalone dehumidifier is intentional mechanical conditioning equipment; attributing its sensible heat addition and latent moisture removal to `InternalGain` conflates mechanical output with passive gains (lighting, plug loads, occupants) in per-category diagnostics and energy balance reports.

No `HvacDehumidification` (or equivalent) category exists in the `ThermalCategory` enum, so there is no way to separate dehumidifier contributions from passive internal gains in `sensible_by_category` breakdowns.

`sensible_gain_w` in the dehumidifier port at line 186 equals `latent_removal_w + electric_power_w` — the sum of condensation heat released as sensible heat plus motor dissipation. Both are real sensible gains to the zone but not passive gains.

EnergyPlus Engineering Reference, Zone Air Heat Balance §3.1: internal gains (people, lights, equipment) are explicitly a separate term from HVAC equipment contributions in the zone heat balance. ASHRAE Handbook of Fundamentals 2021 Ch. 18 Table 1: internal gains defined as occupants, lighting, and plug loads; conditioning equipment is not included.

## Current Behavior

`hares-equipment/src/hvac/dehumidifier.rs:332–339`:

```rust
ports.accumulate(&PortContribution::Thermal {
    zone: self.zone_id,
    sensible_gain_w: snapshot.sensible_gain_w,
    radiant_gain_w: 0.0,
    latent_gain_w: -snapshot.latent_removal_w,
    category: ThermalCategory::InternalGain,   // wrong
})?;
```

`hares-types/src/ports.rs:17–47`: `ThermalCategory` has five variants — `HvacHeating`, `HvacCooling`, `InternalGain`, `JacketLoss`, `DuctLoss`. `THERMAL_CATEGORY_COUNT == 5`. No dehumidification variant exists.

## Required Behavior

1. Add `ThermalCategory::HvacDehumidification` to the `ThermalCategory` enum in `hares-types/src/ports.rs` with `index()` value 5.
2. Update `THERMAL_CATEGORY_COUNT` from 5 to 6.
3. Update the `index()` match arm for the new variant.
4. In `dehumidifier.rs:338`, change `category: ThermalCategory::InternalGain` to `category: ThermalCategory::HvacDehumidification`.
5. In thermal solver diagnostics (`thermal_solver/mod.rs`), add a read of `sensible_for_category(ThermalCategory::HvacDehumidification)` for reporting.
6. Update all tests that reference `THERMAL_CATEGORY_COUNT` or iterate over the full variant set.

Reference: EnergyPlus Engineering Reference, Zone Air Heat Balance §3.1; OCHRE `dehumidifier.py` — end-use "Dehumidifier" tracked separately from "Internal Gains".

## Approach

1. Add `HvacDehumidification = 5` to the enum; update `index()` with a new match arm; set `THERMAL_CATEGORY_COUNT = 6`.
2. Update `ThermalAccumulator` array sizes — they are `[f64; THERMAL_CATEGORY_COUNT]` so they resize automatically if the constant is updated correctly.
3. Run `cargo test -p hares-types` and `cargo test -p hares-equipment` to catch any tests depending on the count or full-variant exhaustiveness.

## Definition of Done

- [ ] `ThermalCategory::HvacDehumidification` variant exists with `index() == 5`
- [ ] `THERMAL_CATEGORY_COUNT == 6`
- [ ] `dehumidifier.rs:338` writes under `ThermalCategory::HvacDehumidification`
- [ ] Thermal solver diagnostics report the dehumidification category
- [ ] All existing tests pass (array size is derived from the constant; no manual array size literals broken)
- [ ] New test: dehumidifier step writes to `HvacDehumidification` category, `InternalGain` bucket is zero for that step

## Verification

```bash
cargo test -p hares-types
cargo test -p hares-equipment
cargo test -p hares-envelope
```

## References

- EnergyPlus Engineering Reference, Zone Air Heat Balance §3.1 — HVAC equipment is a separate term from internal gains
- ASHRAE Handbook of Fundamentals 2021 Ch. 18 Table 1 — internal gains: occupants, lighting, plug loads; conditioning equipment excluded
- OCHRE `dehumidifier.py` — end-use "Dehumidifier" separate from "Internal Gains"
- `hares-types/src/ports.rs:17–47` — `ThermalCategory` enum and `THERMAL_CATEGORY_COUNT`

---

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match — `dehumidifier.rs:332–339` confirmed; `ThermalCategory::InternalGain` at line 338 confirmed.
- [x] Described logic matches current implementation — `ports.accumulate` with `category: ThermalCategory::InternalGain` at line 338; `sensible_gain_w = latent_removal_w + electric_power_w` at line 186. Both match the ticket description exactly.
- [x] `ThermalCategory` enum at `ports.rs:17–29`; `THERMAL_CATEGORY_COUNT == 5` at line 47; no `HvacDehumidification` variant — confirmed.
- [x] OCHRE cross-check: **diverges / N/A** — OCHRE does not implement a dehumidifier at all. `vendors/OCHRE/ochre/utils/hpxml.py:1678` contains `# TODO: add dehumidifier` and `vendors/OCHRE/ochre/Models/Humidity.py:44` contains `# FUTURE: Dehumidifier?`. The OCHRE docs (`docs/source/ModelingApproach.rst`) explicitly list "Dehumidifiers" under "Features Not Modeled". The ticket's claim that "OCHRE `dehumidifier.py` — end-use 'Dehumidifier' tracked separately from 'Internal Gains'" is **incorrect**: no such file exists. The broader architectural claim it supports (dehumidifier ≠ internal gain) is still valid from EnergyPlus evidence.
- [x] EnergyPlus cross-check: **matches ticket's intent; diverges from cited section** — EnergyPlus places the `ZoneDehumidifier` in the "Zone Equipment and Zone Forced Air Units" category, explicitly **not** in the "Internal Gains" group. Quoted passage (EnergyPlus Eng. Ref., Zone Equipment and Zone Forced Air Units, multiple versions): *"In EnergyPlus, this object is modeled as a type of zone equipment (ref. ZoneHVAC:EquipmentList and ZoneHVAC:EquipmentConnections). The sensible heat generated by the dehumidifier is carried over to the zone air heat balance for the next HVAC time step."* The EnergyPlus I/O Reference explicitly groups Internal Gains as: People, Lights, ElectricEquipment, GasEquipment, HotWaterEquipment, SteamEquipment, OtherEquipment, ElectricEquipment:ITE:AirCooled, ZoneBaseboard — dehumidifiers are absent from this group.

### Web-Verified Citations

**Citation 1**
- **Citation**: EnergyPlus Engineering Reference, Zone Air Heat Balance §3.1 — HVAC equipment is a separate term from internal gains
- **Source found**: <https://bigladdersoftware.com/epx/docs/8-2/engineering-reference/zone-equipment-and-zone-forced-air-units.html> and <https://bigladdersoftware.com/epx/docs/8-0/engineering-reference/page-108.html> (EnergyPlus Engineering Reference, Zone Equipment and Zone Forced Air Units)
- **Quoted passage**: *"In EnergyPlus, this object is modeled as a type of zone equipment (ref. ZoneHVAC:EquipmentList and ZoneHVAC:EquipmentConnections). The Zone Dehumidifier Sensible Heating Rate (W) is calculated during each HVAC simulation time step, and the results are averaged for the timestep being reported. However, this sensible heating is carried over to the zone air heat balance for the next HVAC time step (i.e., it is reported as an output variable for the current simulation time step but actually impacts the zone air heat balance on the following HVAC time step)."*
- **Verdict**: **Partially correct** — the substantive claim (dehumidifier ≠ internal gain) is correct and well-supported by EnergyPlus. However, the specific section number "§3.1" does not exist in the EnergyPlus Engineering Reference as cited; EnergyPlus does not number sections this way, and the dehumidifier is covered in "Zone Equipment and Zone Forced Air Units", not under a "Zone Air Heat Balance §3.1" heading. The section reference is fabricated or garbled.

**Citation 2**
- **Citation**: ASHRAE Handbook of Fundamentals 2021 Ch. 18 Table 1 — internal gains: occupants, lighting, plug loads; conditioning equipment excluded
- **Source found**: <https://www.ashrae.org/technical-resources/ashrae-handbook/description-2021-ashrae-handbook-fundamentals> (chapter description only; full text is paywalled). Cross-referenced via: <https://bemcyclopedia.com/wiki/Define_internal_loads_(occupants,_lighting,_equipment)> and EnergyPlus I/O Reference group definition.
- **Quoted passage** (from bemcyclopedia, consistent with ASHRAE methodology): *"Internal loads refer to features in a building that generate heat (or act as heat sinks). These may come from occupants (people), lighting, or equipment ('plug loads')."* and *"internal loads must be 'offset by heating and cooling (HVAC) equipment'"* — meaning HVAC systems are the response to internal loads, not part of them. EnergyPlus I/O Reference corroborates: the "Group – Internal Gains" includes People, Lights, ElectricEquipment, GasEquipment, HotWaterEquipment, SteamEquipment, OtherEquipment, ElectricEquipment:ITE:AirCooled, and ZoneBaseboard — no dehumidifier or HVAC cooling equipment.
- **Verdict**: **Partially correct** — the conceptual claim (ASHRAE defines internal gains as occupants/lighting/plug loads, with conditioning equipment excluded) is accurate and standard across the industry. The specific citation of "Ch. 18 Table 1" cannot be independently verified because the 2021 ASHRAE HoF is paywalled; Chapter 18 covers "Nonresidential Cooling and Heating Load Calculations" but Table 1 contents are not publicly confirmed. The underlying distinction is correct; the precise table number cannot be audited.

**Citation 3**
- **Citation**: OCHRE `dehumidifier.py` — end-use "Dehumidifier" separate from "Internal Gains"
- **Source found**: `/Users/rich/source/HARES/vendors/OCHRE/ochre/` (local submodule, read directly)
- **Quoted passage**: `vendors/OCHRE/ochre/utils/hpxml.py:1678`: `# TODO: add dehumidifier`. `vendors/OCHRE/ochre/Models/Humidity.py:44`: `# FUTURE: Dehumidifier?`. OCHRE docs: *"OCHRE does not currently include a dehumidifier or other models to control indoor humidity."* (ModelingApproach.rst, under "Features Not Modeled").
- **Verdict**: **Incorrect** — no `dehumidifier.py` file exists in OCHRE. OCHRE explicitly does not implement a dehumidifier. The architectural pattern that supports the ticket's intent (OCHRE separates `internal_sens_gain` for appliances from `hvac_sens_gain` for HVAC, at `Envelope.py:427–430`) is real, but the specific citation of a OCHRE dehumidifier end-use is wrong.

### Legitimacy

- **Verdict**: **Legitimate**
- **Rationale**: The core bug is real and confirmed by direct code inspection: `dehumidifier.rs:338` writes `ThermalCategory::InternalGain` for the dehumidifier's thermal output, and no `HvacDehumidification` variant exists in the `ThermalCategory` enum (`ports.rs:17–47`). EnergyPlus explicitly classifies the `ZoneDehumidifier` as zone HVAC equipment (not an internal gain source), and its sensible heat is handled through the HVAC time-step path rather than the internal-gains path. The ASHRAE and EnergyPlus I/O Reference both confirm that internal gains are limited to people, lights, and plug loads — conditioning equipment is excluded. The regression test written for this audit fails with 1630.8 W flowing into the `InternalGain` bucket instead of zero, demonstrating the mismatch is live and observable. The two citation defects (non-existent OCHRE file; non-existent EnergyPlus §3.1 section number) are minor sourcing errors that do not affect the validity of the underlying finding.

### Proposed Fix Summary

1. Add `HvacDehumidification` as a new variant to `ThermalCategory` in `hares-types/src/ports.rs` with `index()` returning 5.
2. Increment `THERMAL_CATEGORY_COUNT` from 5 to 6 (array sizes derive from this constant and resize automatically).
3. Add a match arm for `ThermalCategory::HvacDehumidification => 5` to the `index()` method.
4. In `hares-equipment/src/hvac/dehumidifier.rs:338`, change `category: ThermalCategory::InternalGain` to `category: ThermalCategory::HvacDehumidification`.
5. Optionally add a `sensible_for_category(ThermalCategory::HvacDehumidification)` read to thermal solver diagnostics.
6. Run `cargo test -p hares-types -p hares-equipment -p hares-envelope` to confirm all tests pass.

No production files under `crates/*/src/` were modified by this audit.

### Test Written

- **File**: `crates/hares-equipment/src/hvac/dehumidifier.rs` — `#[cfg(test)] mod tests`, function `dehumidifier_thermal_category_is_hvac_not_internal_gain`
- **What it tests**: When the dehumidifier runs with RH above setpoint (RH=0.60 > target=0.50), the `InternalGain` sensible bucket in the thermal accumulator must be 0.0 after a step. Currently FAILS with 1630.8 W in the `InternalGain` bucket. Will pass once the category is changed to `HvacDehumidification`.
