---
id: PARITY-020
title: "Output metrics expansion"
kind: implement
depends_on: [PARITY-015, PARITY-016, PARITY-018]
files_to_touch:
  - crates/hares-io/src/output/metrics.rs
  - crates/hares-io/src/output/columns.rs
  - crates/hares-core/src/dwelling/mod.rs
references:
  - docs/equipment/ochre-parity-gaps.md (Gap 12)
  - vendors/OCHRE/ochre/utils/analysis.py (calculate_metrics)
verification:
  - cargo build --workspace
  - cargo test --workspace
  - cargo clippy --workspace -- -D warnings
---

## Background/Context

HARES reports ~10 metrics; OCHRE reports 50+. Key missing metrics include per-equipment COP, duct system efficiency, component loads (infiltration/ventilation/duct losses), islanding capability, and envelope component breakdowns.

**Target**: Add the most impactful metrics for validation and audit workflows. Not all 50+ OCHRE metrics are needed — focus on those required for EnergyPlus comparison and RESNET compliance.

## Work to Do

- [ ] Add to `SimulationMetrics`:
  - **Envelope component loads** (annual kWh): infiltration, ventilation, window solar, opaque conduction, internal gains — available from `EnvelopeComponentGains` in thermal solver
  - **HVAC COP** (heating and cooling): total thermal output / total electrical input, per equipment
  - **Water heater COP**: total delivered energy / total input energy
  - **Duct system efficiency**: delivered / (delivered + duct losses)
  - **Unmet load hours**: heating and cooling separately (already partial)
  - **Peak demand by end-use**: per-EndUse peak kW (not just total)
  - **Renewable fraction**: PV generation / total consumption
  - **Battery round-trip efficiency**: energy_out / energy_in over simulation
  - **EV unmet charging**: hours where SOC < target at departure
- [ ] In `dwelling/mod.rs`, accumulate component gains each timestep (from thermal solver's `EnvelopeComponentGains`)
- [ ] In `metrics.rs`, compute all new metrics from accumulated data
- [ ] Add metric output to Parquet/CSV footer or separate metrics file
- [ ] Add tests: verify infiltration load > 0, COP > 0 when HVAC runs, renewable fraction in [0,1]

## Files to Touch

- `crates/hares-io/src/output/metrics.rs`: New metric structs and computation
- `crates/hares-io/src/output/columns.rs`: New output columns for component gains
- `crates/hares-core/src/dwelling/mod.rs`: Accumulate component gains per timestep

## Measures of Success

- [ ] At least 25 metrics reported (up from ~10)
- [ ] Envelope component loads sum to approximately total HVAC energy (energy balance check)
- [ ] HVAC COP values are physically reasonable (1.0-5.0 for heat pumps, 0.8-0.95 for gas)

## Verification

- [ ] `cargo build --workspace` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
