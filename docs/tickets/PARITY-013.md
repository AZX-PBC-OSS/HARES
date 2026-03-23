---
id: PARITY-013
title: "Framing factor / parallel-path U-value"
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-envelope/src/boundary_rc.rs
  - crates/hares-io/src/hpxml/building.rs
  - crates/hares-io/src/envelope_lut.rs
references:
  - docs/equipment/ochre-parity-gaps.md (Gap 14)
  - ASHRAE Handbook of Fundamentals Ch. 27.3 (Parallel-Path Method)
  - EnergyPlus Engineering Reference Ch. 3.3.3 (Construction with Internal Source)
verification:
  - cargo build --workspace
  - cargo test --workspace
  - cargo clippy --workspace -- -D warnings
---

## Background/Context

Real wood-frame walls have studs (high conductivity) in parallel with insulation cavities (low conductivity). The parallel-path method computes effective U-value as area-weighted average. HARES currently uses uniform material properties per layer, which overestimates insulation effectiveness.

**Target**: Implement ASHRAE parallel-path method for framed assemblies.

## Work to Do

- [ ] In `hpxml/building.rs`, parse framing factor from HPXML:
  - `FramingFactor` element (fraction of wall area that is framing, typically 0.15-0.25)
  - `StudSpacing` and `StudWidth` as alternative (derive framing factor)
- [ ] Add `framing_factor: Option<f64>` to `Boundary` struct
- [ ] In `boundary_rc.rs`, when framing factor is present:
  - Split insulation layer into two parallel RC paths:
    - Cavity path: (1 - framing_factor) × area, insulation R-value
    - Stud path: framing_factor × area, wood R-value (R = thickness / 0.144 W/m·K for softwood)
  - Combine as parallel resistance: `1/R_eff = ff/R_stud + (1-ff)/R_cavity`
  - Or model as two separate boundary instances sharing the same zone pair
- [ ] Default framing factors by construction type:
  - WoodStud: 0.23 (2x4 @ 16" OC per ASHRAE)
  - WoodStud 2x6: 0.22
  - SteelStud: 0.20 (but with thermal bridging correction factor)
- [ ] Add tests: wall with ff=0.23 has higher U-value than ff=0 (all insulation)

## Files to Touch

- `crates/hares-envelope/src/boundary_rc.rs`: Parallel-path resistance computation
- `crates/hares-io/src/hpxml/building.rs`: Parse framing factor
- `crates/hares-io/src/envelope_lut.rs`: Default framing factors by construction type

## Measures of Success

- [ ] 2x4 R-13 wall with studs: effective R ≈ 10.4 (vs R-13 without studs)
- [ ] Wall heat loss increases 15-25% when framing factor is applied (expected for wood frame)
- [ ] No framing factor specified → current behavior unchanged

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
