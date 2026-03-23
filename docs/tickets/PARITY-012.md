---
id: PARITY-012
title: "HPWH HP/ER independent duty cycle control"
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-equipment/src/water_heater/heat_pump_wh.rs
  - crates/hares-types/src/control_signal.rs
references:
  - docs/equipment/ochre-parity-gaps.md (Gap 16)
  - vendors/OCHRE/ochre/models/WaterHeater.py (HP/ER duty cycle, lines 516-526)
verification:
  - cargo build --workspace
  - cargo test --workspace
  - cargo clippy --workspace -- -D warnings
---

## Background/Context

OCHRE allows separate duty cycle control for HPWH compressor vs backup element, enabling fine-grained demand response (e.g., curtail compressor during peak but allow backup for safety). HARES has generic DutyCycle control but cannot independently target HP vs ER.

**Target**: Add `DutyCycleHP` and `DutyCycleER` sub-signals or extend existing DutyCycle to support component targeting.

## Work to Do

- [ ] Extend `ControlSignal::DutyCycle` or add new variant:
  - Option A: Add `component: Option<HpwhComponent>` field to DutyCycle (where HpwhComponent = Compressor | BackupElement)
  - Option B: Add `DutyCycleCompressor` and `DutyCycleBackupElement` variants to ControlSignal
  - Prefer Option A for backward compatibility
- [ ] In `HeatPumpWaterHeater::apply_control_unchecked()`:
  - If DutyCycle with component=Compressor: set `hp_duty_cycle_override`
  - If DutyCycle with component=BackupElement: set `er_duty_cycle_override`
  - If DutyCycle with component=None: apply to both (current behavior)
- [ ] In `step()`, apply component-specific duty cycles:
  - `effective_hp_power = compressor_power * hp_duty_cycle`
  - `effective_er_power = backup_power * er_duty_cycle`
- [ ] Add tests: set HP duty=0.5, ER duty=1.0 → compressor curtailed, backup at full power

## Files to Touch

- `crates/hares-equipment/src/water_heater/heat_pump_wh.rs`: HP/ER split duty cycle handling
- `crates/hares-types/src/control_signal.rs`: Extend DutyCycle if needed

## Measures of Success

- [ ] HP compressor and backup element can be curtailed independently
- [ ] Default behavior unchanged when component not specified
- [ ] DR scenario: curtail compressor during peak while allowing backup for freeze protection

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
