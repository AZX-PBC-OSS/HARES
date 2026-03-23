---
id: PARITY-002
title: "Refactor: Extract hvac_core.rs sub-modules"
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-equipment/src/hvac/hvac_core.rs
  - crates/hares-equipment/src/hvac/staging.rs (new)
  - crates/hares-equipment/src/hvac/duct_distribution.rs (new)
references:
  - docs/equipment/hvac.md
verification:
  - cargo build --workspace
  - cargo test --workspace
  - cargo clippy --workspace -- -D warnings
---

## Background/Context

`hvac_core.rs` is 2749 lines with 121 methods. It contains coil physics, speed staging logic, duct distribution, multi-zone heat fraction routing, and thermostat integration all in one struct. PARITY-013 (ASHP backup ER FSM) will add more complexity to this file.

## Work to Do

- [ ] **`hvac/staging.rs`** (~400 lines): Speed staging and part-load logic
  - Speed selection (single, two-speed, multi-speed, variable)
  - Part-load ratio calculation
  - PLF degradation coefficients
  - Startup capacity degradation (Winkler 2011)
  - Speed disabling (will be extended by future tickets)

- [ ] **`hvac/duct_distribution.rs`** (~200 lines): Duct and zone heat routing
  - DSE (distribution system efficiency) calculation
  - Zone heat fraction distribution
  - Supply/return leakage modeling
  - Multi-zone routing logic

- [ ] **`hvac_core.rs`** (~1500 lines): Core HVAC equipment
  - `HvacEquipment` struct with thermostat
  - Biquadratic curve evaluation
  - Control signal handling
  - Mode determination (heating/cooling/off)
  - Orchestrate staging → duct distribution → port output

### Quality Requirements

- [ ] Clear separation: `staging.rs` has no zone/duct knowledge, `duct_distribution.rs` has no speed/staging knowledge
- [ ] All extracted types use SI units with descriptive names (e.g., `capacity_w: f64`, not `cap: f64`)
- [ ] No allocation in hot-path functions
- [ ] All existing HVAC tests pass unchanged

## Measures of Success

- [ ] No file exceeds 1500 lines
- [ ] `HvacEquipment::step()` is < 50 lines (delegates to staging + distribution)
- [ ] All HVAC tests pass

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
