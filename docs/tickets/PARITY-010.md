---
id: PARITY-010
title: "PV near-shading model"
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-equipment/src/pv/shading.rs (new)
  - crates/hares-equipment/src/pv/mod.rs
  - crates/hares-equipment/src/pv/array_config.rs
references:
  - docs/equipment/ochre-parity-gaps.md (Gap 10)
  - PVsyst shading model documentation
  - SAM shade calculator methodology
verification:
  - cargo build --workspace
  - cargo test --workspace
  - cargo clippy --workspace -- -D warnings
---

## Background/Context

HARES has no building-feature shading for PV. Real residential PV systems are commonly partially shaded by dormers, chimneys, trees, and neighboring buildings. The soiling model (Kimber) handles dust accumulation but not geometric shading.

**Target**: Implement a practical shading model — not full ray-tracing (that's SAM's domain), but a configurable shading loss schedule or obstruction-angle model that can represent common residential shading scenarios.

## Work to Do

- [ ] Create `crates/hares-equipment/src/pv/shading.rs`:
  - `ShadingModel` enum:
    - `None` — no shading (default, current behavior)
    - `FixedLoss { annual_fraction: f64 }` — constant shading derating
    - `MonthlyLoss { fractions: [f64; 12] }` — per-month shading schedule
    - `ObstructionAngle { azimuth_deg: f64, elevation_deg: f64, width_deg: f64 }` — single horizon obstruction
    - `HorizonProfile { points: Vec<(f64, f64)> }` — (azimuth, elevation) horizon line
  - `fn shading_factor(model, solar_azimuth, solar_altitude, month) -> f64` — returns 0.0-1.0
- [ ] In `pv/mod.rs`, apply shading factor after soiling and before DC power calculation:
  - `effective_irradiance = poa_irradiance * soiling_ratio * shading_factor`
- [ ] In `array_config.rs`, parse shading config per array:
  - `shading_model` key with sub-keys for the chosen variant
- [ ] Add `shading_loss_fraction` to PV telemetry
- [ ] Add tests: south-facing array with eastern obstruction loses morning production but not afternoon

## Files to Touch

- `crates/hares-equipment/src/pv/shading.rs`: New shading module
- `crates/hares-equipment/src/pv/mod.rs`: Apply shading in DC power pipeline
- `crates/hares-equipment/src/pv/array_config.rs`: Parse shading config

## Measures of Success

- [ ] `FixedLoss(0.1)` reduces annual output by ~10%
- [ ] `HorizonProfile` blocks production when sun is behind obstruction
- [ ] Default (`None`) produces identical results to current implementation

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
