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
