# HPWH Zone Sensible Heat Written as InternalGain Instead of HvacDehumidification/JacketLoss Split

**Severity**: Low
**Priority**: P3
**Status**: Open
**Areas**: hares-equipment
**Depends on**: Ticket 071 (`HvacDehumidification` category must exist before this ticket can be implemented)

## Problem

The heat pump water heater (HPWH) writes compressor-extracted zone heat to the thermal port under `ThermalCategory::InternalGain` at `heat_pump_wh.rs:746`. HP compressor waste heat and latent moisture removal from the zone air are mechanical equipment effects, not passive internal gains. Attributing them to `InternalGain` conflates mechanical conditioning with occupant and appliance heat in per-category diagnostics.

Additionally, two physically distinct sources are both written under `ThermalCategory::JacketLoss`:
- Line 754: compressor waste heat to the wall-facing fraction (`sensible_to_wall_w`)
- Line 768: tank skin conduction loss (`skin_loss_w`)

Merging these into one category makes the `JacketLoss` aggregate ambiguous for diagnosing HP-cycle losses vs. tank losses independently.

EnergyPlus Engineering Reference §50 "Water Heater Heat Pump": heat extracted from zone air by the HP evaporator is a negative zone heat gain; compressor waste heat rejected is a positive zone gain. Both are HP equipment effects, not passive internal gains. Tank skin loss is a conduction term from the hot water body — a genuinely different mechanism.

## Current Behavior

`hares-equipment/src/water_heater/heat_pump_wh.rs:739–757`:

```rust
if sensible_to_zone_w != 0.0 || latent_gain_w != 0.0 {
    ports.accumulate(&PortContribution::Thermal {
        zone,
        sensible_gain_w: sensible_to_zone_w,
        radiant_gain_w: 0.0,
        latent_gain_w,
        category: ThermalCategory::InternalGain,   // wrong: HP waste heat is not a passive gain
    })?;
}
if sensible_to_wall_w != 0.0 {
    ports.accumulate(&PortContribution::Thermal {
        zone,
        sensible_gain_w: sensible_to_wall_w,
        radiant_gain_w: 0.0,
        latent_gain_w: 0.0,
        category: ThermalCategory::JacketLoss,     // conflated with tank skin loss below
    })?;
}
```

`heat_pump_wh.rs:760–769`: tank skin loss written to `ThermalCategory::JacketLoss` — correct for tank skin loss, but now indistinguishable from the wall-fraction compressor heat above.

`heat_pump_wh.rs:3005–3006`: existing test reads `InternalGain` and `JacketLoss` buckets and will need updating.

## Required Behavior

After ticket 071 adds `ThermalCategory::HvacDehumidification`:

1. HPWH compressor zone waste heat (sensible + latent) at `heat_pump_wh.rs:746`: change category to `ThermalCategory::HvacDehumidification`. The HPWH acts as a dehumidifying heat pump on the zone air — the same physics as a standalone dehumidifier.

2. HPWH compressor wall-facing waste heat (`sensible_to_wall_w`) at `heat_pump_wh.rs:754`: keep as `ThermalCategory::JacketLoss` with an inline comment: "compressor waste heat to wall face; shares JacketLoss with tank skin loss pending a dedicated HvacWasteHeat variant".

3. Tank skin loss (`skin_loss_w`) at `heat_pump_wh.rs:768`: keep as `ThermalCategory::JacketLoss` — this is the correct category.

Reference: EnergyPlus Engineering Reference §50 — HPWH zone interactions; OCHRE `WaterHeater.py` — "Zone Gains" and "Tank Losses" tracked as separate line items.

## Approach

1. Confirm ticket 071 has landed and `ThermalCategory::HvacDehumidification` exists.
2. Change `heat_pump_wh.rs:746` from `ThermalCategory::InternalGain` to `ThermalCategory::HvacDehumidification`.
3. Add inline comment at `heat_pump_wh.rs:754` as described above.
4. Update the test at `heat_pump_wh.rs:3005–3006` to read `HvacDehumidification` instead of `InternalGain`.

## Definition of Done

- [ ] `HvacDehumidification` variant exists (ticket 071 merged)
- [ ] HPWH compressor zone sensible/latent at `heat_pump_wh.rs:746` written under `HvacDehumidification`
- [ ] Inline comment at line 754 explains wall-fraction JacketLoss conflation
- [ ] Tank skin loss at line 768 unchanged (`JacketLoss`)
- [ ] Test at `heat_pump_wh.rs:3005–3006` updated: `InternalGain` category is zero when HP is running; `HvacDehumidification` is non-zero
- [ ] No regression in HPWH physics tests

## Verification

```bash
cargo test -p hares-equipment water_heater
```

## References

- EnergyPlus Engineering Reference §50 "Water Heater Heat Pump" — zone heat interactions; HP evaporator extraction vs. compressor waste heat
- OCHRE `WaterHeater.py` — "Zone Gains" and "Tank Losses" as separate end-use reporting
- `hares-equipment/src/water_heater/heat_pump_wh.rs:739–769` — current port writes
- Ticket 071 — `HvacDehumidification` category prerequisite

---

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match — `heat_pump_wh.rs:739–769` confirmed; `ThermalCategory::InternalGain` at line 746 and `ThermalCategory::JacketLoss` at lines 755 and 768 all match the ticket description exactly.
- [x] Described logic matches current implementation — the two physically distinct `JacketLoss` writes (compressor wall-fraction waste heat at line 749–757 and tank skin conduction at lines 760–771) are confirmed present and indistinguishable in the category bucket.
- [x] `ThermalCategory::HvacDehumidification` does **not** exist — confirmed from `hares-types/src/ports.rs:17–47`. The enum has five variants (`HvacHeating`, `HvacCooling`, `InternalGain`, `JacketLoss`, `DuctLoss`); `THERMAL_CATEGORY_COUNT == 5`. Ticket 071 (which adds `HvacDehumidification`) is open and unmerged, confirming this ticket's prerequisite.
- [x] Test at `heat_pump_wh.rs:3005–3006` confirmed — `sensible_for_category(ThermalCategory::InternalGain)` at line 3005. The unit test `wall_heat_fraction_preserves_energy_balance_and_tracks_split` currently passes against the buggy code and would need updating after the fix.
- [x] OCHRE cross-check: **Diverges from ticket's characterization**. In `WaterHeater.py`, `HeatPumpWaterHeater.add_gains_to_zone()` (lines 678–685) adds the combined `sensible_gain` (which already includes both HP cycle waste heat and `h_loss` from `finish_sub_update` at line 690) to `zone.internal_sens_gain` via a single undifferentiated attribute. OCHRE does **not** separate "Zone Gains" from "Tank Losses" in its zone accounting — both flow into `internal_sens_gain`. The ticket's claim that OCHRE tracks "Zone Gains" and "Tank Losses" as separate line items refers to OCHRE's results CSV output variables (`"Hot Water Heat Loss (W)"` in `Variable names and units.csv` line 63) and the `Water.py` model's internal `h_loss` field, **not** to separate zone-thermal categories. The architectural concern (HP-cycle gains should be distinguished from passive internal gains) remains valid, but HARES is making a finer-grained distinction than OCHRE actually implements.
- [x] EnergyPlus cross-check: **Partially matches ticket intent; ticket's cited mechanics are incorrect**. See Web-Verified Citations below.

### Web-Verified Citations

**Citation 1**
- **Citation**: "EnergyPlus Engineering Reference §50 'Water Heater Heat Pump': heat extracted from zone air by the HP evaporator is a negative zone heat gain; compressor waste heat rejected is a positive zone gain."
- **Source found**: https://bigladdersoftware.com/epx/docs/9-6/engineering-reference/coils.html (Single-Speed Electric Heat Pump DX Water Heating Coil section) and https://bigladdersoftware.com/epx/docs/9-4/engineering-reference/water-thermal-tanks-includes-water-heaters.html
- **Quoted passage**: From the EnergyPlus 9.6 Engineering Reference (Coils chapter): *"The model assumes that **all compressor power is rejected as heat via the DX heating coil**. Therefore, the evaporator total cooling capacity at the current operating conditions is determined depending on the user input for pump heat: Q̇_Evap = Q̇_heating − P_comp"*. Output variables: *"DX Coil Total Cooling Rate (W) = Q̇_evap(PLR)"*, *"DX Coil Sensible Cooling Rate (W) = Q̇_evap(PLR)(SHR)"*, *"DX Coil Latent Cooling Rate (W) = Q̇_evap(PLR)(1.0 − SHR)"*.
- **Verdict**: **Incorrect on two counts**:
  1. **"§50" does not exist.** The EnergyPlus Engineering Reference uses named section headings only; there are no numbered chapters or sections. The HPWH content lives in the chapter "Water Thermal Tanks (includes Water Heaters)" with a sub-heading "Heat Pump Water Heater", and the DX coil mechanics are in "Coils → Single-Speed Electric Heat Pump DX Water Heating Coil".
  2. **"Compressor waste heat rejected is a positive zone gain" is contradicted by the documentation.** The EnergyPlus model explicitly routes all compressor work to the condenser/water side: *"all compressor power is rejected as heat via the DX heating coil."* The zone's thermal effect from HPWH operation is net cooling (evaporator extracts sensible + latent heat from zone air); there is no EnergyPlus model path that injects compressor waste heat back into zone air as a positive gain. The ticket's correct underlying observation — that HP zone effects are equipment interactions, not passive internal gains — is valid, but the stated EnergyPlus mechanics are wrong.

**Citation 2**
- **Citation**: OCHRE `WaterHeater.py` — "Zone Gains" and "Tank Losses" tracked as separate line items.
- **Source found**: `vendors/OCHRE/ochre/Equipment/WaterHeater.py` (local submodule, read directly); `vendors/OCHRE/ochre/Models/Water.py`; `vendors/OCHRE/ochre/defaults/Variable names and units.csv`
- **Quoted passage**: `WaterHeater.py` lines 678–690 (`HeatPumpWaterHeater`): `self.zone.internal_sens_gain += self.sensible_gain * (1 - self.wall_heat_fraction)` and `self.sensible_gain += h_loss` (finish_sub_update). `Variable names and units.csv` line 63: `"Hot Water Heat Loss (W),W,Load: Hot Water: Tank Losses"`.
- **Verdict**: **Partially correct, but misleading.** OCHRE does report `h_loss` as a separate CSV output variable ("Hot Water Heat Loss (W)"), but this is a *reporting* separation only. In the actual zone-thermal accounting, `h_loss` is folded into `self.sensible_gain` (via `finish_sub_update`) before being added to `zone.internal_sens_gain` — there is no separate category tracking in the zone's internal gain structure. OCHRE does not distinguish "HP-cycle zone gains" from "tank skin conduction" in the zone thermal balance; both end up in `internal_sens_gain` as one aggregate. The ticket overstates OCHRE's categorization fidelity.

### Legitimacy

- **Verdict**: **Partially Legitimate**
- **Rationale**: The core diagnostic bug is real and confirmed by code inspection: `heat_pump_wh.rs:746` writes `ThermalCategory::InternalGain` for HPWH compressor-zone waste heat, and the two `JacketLoss` writes (lines 749–757 for compressor wall-fraction heat, and lines 760–771 for tank skin conduction) are indeed indistinguishable in per-category diagnostics. The proposed fix direction — reclassifying the HP-cycle zone interaction to a dedicated HVAC category — is architecturally sound. However, three elements of the ticket require correction: (1) the EnergyPlus section reference "§50" does not exist in any version of the Engineering Reference; (2) the claim that "compressor waste heat rejected is a positive zone gain" is physically incorrect per EnergyPlus — all compressor work goes to the water condenser, not to zone air; the zone effect is net cooling from the evaporator; (3) OCHRE does not actually separate zone gains and tank losses in the thermal zone accounting — both are summed into `internal_sens_gain`. The fix itself remains straightforward and the prerequisite (ticket 071 adding `HvacDehumidification`) is correctly identified. The core mismatch is real and warrants fixing; the citation quality is poor.

### Proposed Fix Summary

1. Wait for ticket 071 to merge, adding `ThermalCategory::HvacDehumidification` (index 5) to `hares-types/src/ports.rs`.
2. In `heat_pump_wh.rs:746`, change `category: ThermalCategory::InternalGain` to `category: ThermalCategory::HvacDehumidification`.
3. Add an inline comment at `heat_pump_wh.rs:754`–755 explaining that `sensible_to_wall_w` (compressor wall-fraction waste heat) shares `JacketLoss` with tank skin conduction pending a future `HvacWasteHeat` variant.
4. Leave `heat_pump_wh.rs:768` (`skin_loss_w`) under `JacketLoss` — this is correct.
5. Update the test at `heat_pump_wh.rs:3005` to read `HvacDehumidification` instead of `InternalGain`; assert `InternalGain` is 0.0 when HP is running.
6. Update the external test `hpwh_wall_heat_fraction_splits_sensible_gain_by_category` in `water_heater_parity.rs` (line 1043) to also check `HvacDehumidification` instead of `InternalGain`.

### Test Written

- **File**: `crates/hares-equipment/tests/water_heater_parity.rs` — function `hpwh_compressor_zone_heat_not_reported_as_internal_gain`
- **What it tests**: Runs one HPWH step with a cold tank (compressor guaranteed active), then asserts `ThermalCategory::InternalGain` sensible gain is 0.0 W. Currently **passes via `#[should_panic]`** — the `assert!` fires because `InternalGain` is non-zero (bug present). After ticket 074 fix, `InternalGain` will be 0.0, the `assert!` will not panic, and `#[should_panic]` will cause the test to fail, driving the implementer to remove `#[should_panic]` and confirm the fix is correct.
