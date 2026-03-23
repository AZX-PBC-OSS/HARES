---
id: PARITY-006
title: "Battery temperature-dependent capacity derating"
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-equipment/src/battery/mod.rs
references:
  - docs/equipment/ochre-parity-gaps.md (Gap 5)
  - vendors/OCHRE/ochre/models/Battery.py (d0 Arrhenius model, lines 321-330)
  - SAM Battery model documentation (Perez et al.)
verification:
  - cargo build --workspace
  - cargo test --workspace
  - cargo clippy --workspace -- -D warnings
---

## Background/Context

HARES has a lumped thermal model for battery cell temperature but does not feed temperature back into available charge/discharge power. OCHRE uses a 2-term Arrhenius model to derate capacity at extreme temperatures. Cold batteries deliver significantly less power; hot batteries should be power-limited to prevent thermal runaway.

**Target**: Better than OCHRE — implement full temperature-dependent power derating with configurable curves, not just the OCHRE d0 model. Support both Arrhenius (electrochemistry-based) and piecewise-linear (manufacturer datasheet) approaches.

## Work to Do

- [ ] Add `CapacityDerateModel` enum:
  - `Arrhenius { d0_ref: f64, e_ad1: f64, e_ad2: f64, t_ref_k: f64 }` — OCHRE-compatible
  - `PiecewiseLinear { points: Vec<(f64, f64)> }` — (temp_c, derate_factor) pairs
  - `None` — no derating (current behavior, for backward compat during transition)
- [ ] In `Battery::step()`, compute `capacity_derate = model.evaluate(cell_temp_c)`:
  - Arrhenius: `d0 = d0_ref * exp(-e_ad1/R * (1/T - 1/T_ref) + -e_ad2/R * (1/T - 1/T_ref)^2)`
  - Piecewise: linear interpolation between bracketing points
- [ ] Apply derate to `max_charge_kw` and `max_discharge_kw` before power clamping:
  - `effective_max_charge = max_charge_kw * capacity_derate`
  - `effective_max_discharge = max_discharge_kw * capacity_derate`
- [ ] Parse config: `capacity_derate_model` key with `"arrhenius"` or `"piecewise"` sub-keys
- [ ] Default to Arrhenius with OCHRE reference values (d0_ref=1.0, e_ad1, e_ad2 from SAM)
- [ ] Add `capacity_derate` to telemetry output
- [ ] Add tests: verify derate < 1.0 at 0°C, derate ≈ 1.0 at 25°C, derate < 1.0 at 45°C

## Files to Touch

- `crates/hares-equipment/src/battery/mod.rs`: Derating model, apply in step(), config parsing, telemetry

## Measures of Success

- [ ] At -10°C, capacity derate is 50-70% of rated (matches Li-ion datasheet behavior)
- [ ] At 25°C, derate is 1.0 (reference temperature)
- [ ] At 45°C, derate is 0.9-0.95 (mild hot derating)
- [ ] Power limits are actually enforced (charge/discharge clamped to derated values)

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
