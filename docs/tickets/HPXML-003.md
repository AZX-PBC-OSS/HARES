---
id: HPXML-003
title: Parse gas furnace/boiler pilot light from HPXML
kind: implement
depends_on:
  - HPXML-000
files_to_touch:
  - crates/hares-io/src/hpxml/equipment.rs
  - crates/hares-equipment/src/hvac/furnace.rs
  - crates/hares-equipment/src/hvac/boiler.rs
references:
  - "HPXML spec: HeatingSystemType/*/PilotLight (boolean), extension/PilotLightBtuh"
verification:
  - cargo build -p hares-io
  - cargo test -p hares-io
  - cargo clippy -p hares-io
---

## Background/Context

Gas furnaces and boilers with standing pilot lights have continuous gas consumption that should be modeled. HPXML provides `PilotLight` (boolean) on the heating system type child element, and `extension/PilotLightBtuh` with the actual Btu/hr rate. The gas water heater model already has a `pilot_light_w` parameter. Gas furnace/boiler should support the same.

**Wiring status:** ❌ `pilot_light_w` is NOT consumed by `furnace.rs` or `boiler.rs`. This ticket requires both HPXML extraction AND a small equipment-side change to add idle-mode gas draw to gas furnace/boiler. The change is small: add a `pilot_light_w` field, read it from config, add it to gas consumption when burner is off.

## Work to Do

- [ ] In the `resolve_hvac` heating system loop, check for `PilotLight` boolean in the `HeatingSystemType` child (e.g., `Furnace/PilotLight`)
- [ ] If present and true, extract `extension/PilotLightBtuh` (default to 500 Btu/hr per RESNET if absent)
- [ ] Convert Btu/hr to W (× 0.293_071_07) and insert as `"pilot_light_w"` param
- [ ] Add unit test for a gas furnace with pilot light

## Files to Touch

- `crates/hares-io/src/hpxml/equipment.rs`: Extend heating system resolution

## Measures of Success

- [ ] Gas furnace with `<PilotLight>true</PilotLight>` and `<PilotLightBtuh>600</PilotLightBtuh>` → `pilot_light_w ≈ 175.8`
- [ ] Gas furnace with `<PilotLight>true</PilotLight>` but no Btuh → `pilot_light_w ≈ 146.5` (500 Btu/hr default)
- [ ] No PilotLight element → no pilot_light_w inserted

## Verification

- [ ] `cargo build -p hares-io` passes
- [ ] `cargo test -p hares-io` passes
- [ ] `cargo clippy -p hares-io` passes
