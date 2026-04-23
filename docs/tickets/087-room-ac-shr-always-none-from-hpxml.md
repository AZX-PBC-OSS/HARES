# Room AC SHR Always None From HPXML — Latent Cooling Always Zero

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-io, hares-equipment

## Problem

`try_build_room_ac_config` in `resolve_hvac.rs` hard-codes `shr: None` (line 891),
ignoring any SHR that HPXML may provide. The `CoolingSystem` loop does read
`<SensibleHeatFraction>` into `params["shr"]` (line 1403–1405), but the Room AC
typed-config builder does not pick it up. With `shr: None`, the room AC equipment
model defaults to SHR = 1.0 (all sensible, no latent removal), so the unit never
dehumidifies the zone regardless of how humid the air is.

Room air conditioners are particularly important for latent load because they are
often operated in high-humidity climates (Southeast, Gulf Coast) where latent removal
is a primary function. SHR for room ACs typically ranges from 0.75 to 0.90.

## Evidence

`resolve_hvac.rs:891`:

```
shr: None,
```

`resolve_hvac.rs:1403–1405`:

```
if let Some(shr) = child_f64(cooling, "SensibleHeatFraction") {
    params.insert("shr".to_string(), json!(shr));
}
```

The `params` map may contain `"shr"` from the HPXML element, but `try_build_room_ac_config`
does not call `params.get("shr").and_then(Value::as_f64)`.

## OCHRE Cross-check

OCHRE reads `SensibleHeatFraction` for room AC and passes it to the coil physics
as the steady-state SHR. OCHRE does not hard-code `shr = None` for room ACs.

## Required Behavior

In `try_build_room_ac_config`, read SHR from the params map. The `RoomAcConfig.shr`
field already exists; only the resolver assignment at line 891 is wrong.

## Approach

Change location: `resolve_hvac.rs:891`. Replace:
```
shr: None,
```
with:
```
shr: params.get("shr").and_then(Value::as_f64),
```

No other change is required. The `CoolingSystem` loop already reads
`<SensibleHeatFraction>` into `params["shr"]` at lines 1403–1405 before calling
`try_build_room_ac_config`.

Cross-reference: ticket 022 (Room AC ideal target) addresses the thermostat control
strategy for room AC. That is a separate defect from this SHR wiring gap. Do not merge.

## Citation

- HPXML 4.x schema §CoolingSystem/SensibleHeatFraction (hpxml.nrel.gov)
- AHRI 310/380-2017 §6.2: room AC rated SHR measured at 80°F DB/67°F WB
- EnergyPlus Engineering Reference §16.1.1: SHR partitions total cooling capacity
  into sensible and latent components; SHR = 1.0 implies zero latent removal


## Annual kWh Impact Rank

**Medium.** In humid climates, latent removal accounts for 20–40% of total cooling
load. With SHR = 1.0 (no latent), the room AC never removes moisture, the humidity
rises unchecked, and the thermostat sees no latent load reduction — causing the unit
to run longer than it should to achieve sensible comfort. Annual cooling kWh can be
overstated by 10–20% in humid climates.

## Definition of Done

- [ ] `resolve_hvac.rs:891`: `shr: None` replaced with `shr: params.get("shr").and_then(Value::as_f64)`
- [ ] Test: HPXML room AC with `<SensibleHeatFraction>0.82</SensibleHeatFraction>` → `RoomAcConfig.shr = Some(0.82)`
- [ ] Test: HPXML room AC without `<SensibleHeatFraction>` → `RoomAcConfig.shr = None`

## Verification

```bash
cargo test -p hares-io -- resolve_hvac::tests::room_ac_shr
```

Add a fixture in `crates/hares-io/tests/fixtures/` with a room AC element including `<SensibleHeatFraction>0.82</SensibleHeatFraction>`. Assert the resulting `RoomAcConfig.shr == Some(0.82)`.
