---
id: ACTOR-010
title: Conditioned oracle integration test
kind: implement
depends_on:
  - ACTOR-007
  - ACTOR-009
files_to_touch:
  - tests/conditioned_oracle.rs
references:
  - tests/freefloat_oracle.rs
  - docs/tickets/ACTOR-INDEX.md
verification:
  - cargo test --test conditioned_oracle --features observe
---

## Background/Context

With IdealHvac equipment wired and OCHRE conditioned reference CSVs generated, we can create integration tests comparing HARES conditioned behavior against OCHRE. This validates that the equipment-based ideal HVAC produces correct zone temperatures and loads.

## Work to Do

- [ ] Create `tests/conditioned_oracle.rs` modeled on `freefloat_oracle.rs`
- [ ] Build Dwelling from BEopt HPXML — IdealHvac equipment is auto-created from setpoints
- [ ] Strip non-HVAC equipment (appliances, lighting, water heater) to match OCHRE fixture
- [ ] Run simulation with observer enabled
- [ ] Collect per-step: indoor temp, attic temp, hvac_heating_w, hvac_cooling_w from observer
- [ ] Load OCHRE reference CSV from `tests/fixtures/conditioned/{scenario}/`
- [ ] Compare:
  - Indoor temperature MAE (expect < 0.5°C — both track setpoint)
  - HVAC heating load comparison (mean value tolerance)
  - HVAC cooling load comparison (mean value tolerance)
  - Attic temperature MAE
  - All envelope diagnostics (same checks as freefloat)
- [ ] Hourly comparison table (same format as freefloat)
- [ ] Step-0 heat balance dump (same format as freefloat)
- [ ] Three scenarios × two modes = 6 test functions:
  - `conditioned_ideal_spring_72h`, `conditioned_ideal_summer_48h`, `conditioned_ideal_winter_48h` (IdealCapacityMode::On)
  - `conditioned_dynamic_spring_72h`, `conditioned_dynamic_summer_48h`, `conditioned_dynamic_winter_48h` (IdealCapacityMode::Off — on/off thermostat cycling)
- [ ] Use `ResampleOverrides::ochre_compat()` for ZOH weather resampling (parity testing)
- [ ] Ideal mode: tight MAE (< 0.5°C), exact setpoint tracking
- [ ] Dynamic mode: MAE < 2.0°C (thermostat cycling causes oscillation around setpoint), verify temperature stays within deadband bounds

## Files to Touch

- `tests/conditioned_oracle.rs`: **New** — conditioned oracle integration tests

## Measures of Success

- [ ] Indoor temperature MAE < 0.5°C for all seasons
- [ ] HVAC loads are non-zero and have correct sign (heating in winter, cooling in summer)
- [ ] All envelope diagnostic checks pass (same tolerances as freefloat)
- [ ] Tests produce diagnostic output for CI visibility

## Verification

- [ ] `cargo test --test conditioned_oracle --features observe -- --nocapture` passes for all 3 scenarios
