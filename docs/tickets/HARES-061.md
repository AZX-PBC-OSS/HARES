---
id: HARES-061
title: "Cross-Layer Integration Tests (Incremental)"
kind: test
depends_on: [HARES-004]
files_to_touch:
  - crates/hares-types/tests/type_interop.rs
  - crates/hares-envelope/tests/solver_chain.rs
  - crates/hares-equipment/tests/lifecycle.rs
  - crates/hares-core/tests/integration.rs
references:
  - docs/architecture/07-testing-and-verification.md
verification:
  - cargo test --test type_interop -p hares-types
  - cargo test --test solver_chain -p hares-envelope
  - cargo test --test lifecycle -p hares-equipment
  - cargo test --test integration -p hares-core
---

## Background/Context
Individual tickets test their components in isolation. Data flow bugs between layers — wrong field names, unit mismatches, off-by-one indexing, sign convention errors — hide until layers are composed. These integration tests are written **incrementally as each layer completes**, not deferred to the end. Each test file becomes runnable as soon as its dependencies land.

## Work to Do

This ticket grows incrementally — Phase 1 gate runs after HARES-004, Phase 2 gate after HARES-032, Phase 3 gate after HARES-046.

### Phase 1 gate — after types + physics land (HARES-001 through 008)
**File: `crates/hares-types/tests/type_interop.rs`**
- [ ] Construct `PortSlots`, write `PortContribution::Thermal { zone: 0, sensible: 500.0, latent: 50.0 }`, assert accumulation
- [ ] Write `PortContribution::Electrical { active_power_kw: 1.5, reactive_power_kvar: 0.0 }`, assert sum
- [ ] Call `PortSlots::zero()`, assert all accumulators reset
- [ ] Construct `EnvironmentState` with all required fields, assert `custom_domains` is empty Vec
- [ ] Construct `ControlSignal::thermal_setpoint(...)`, verify `can_accept` with matching and non-matching capabilities

### Phase 1 gate — after envelope lands (HARES-012 through 016)
**File: `crates/hares-envelope/tests/solver_chain.rs`**
- [ ] Build 1R1C network → discretize → step 10 times with constant 500W input → verify zone temp approaches analytical steady state
- [ ] Chain: ThermalSolver.resolve() → pass updated zone temp to HumiditySolver.resolve() → verify humidity changes in correct direction
- [ ] ElectricalSolver.resolve() with 2.0 kW load + -1.5 kW PV → verify net = 0.5 kW
- [ ] **Explicit numerical invariant assertions** (not debug_assert):
  - [ ] Thermal: `|ΣQ_gain - ΔE_storage - Q_loss| < max(1.0W, 1e-6·|ΣQ_gain|)`
  - [ ] Electrical: `|P_grid + ΣP_equipment| < 0.001 kW`
  - [ ] Moisture: `|Δm_water - Σ(Q_latent·dt/h_fg)| < 1e-6 kg`

### Phase 2 gate — after each equipment type lands (HARES-019 through 032)
**File: `crates/hares-equipment/tests/lifecycle.rs`**
- [ ] Generic lifecycle harness that tests ANY Equipment implementor:
  ```
  fn test_equipment_lifecycle(eq: &mut dyn Equipment, env: &EnvironmentState, valid_signal: ControlSignal, invalid_signal: ControlSignal)
  ```
- [ ] For each equipment type as it's implemented:
  - [ ] `init(config, env)` → descriptor fields populated (name, stage, capabilities, telemetry_fields all non-empty)
  - [ ] `apply_control(valid_signal)` → Ok
  - [ ] `apply_control(invalid_signal)` → Err (capability rejection)
  - [ ] `step(env)` → at least one PortContribution written
  - [ ] `telemetry()` → field count matches `descriptor().telemetry_fields.len()`
  - [ ] `save_state()` → non-empty bytes
  - [ ] Step again (mutate state)
  - [ ] `load_state(saved_bytes)` → telemetry matches pre-mutation values
- [ ] Run this harness for: ScheduledLoad (Stage 1), Battery (Stage 2), Thermostat+Furnace (Stage 3) — one per execution stage
- [ ] Each Phase 2 equipment ticket (HARES-019 through HARES-032) must add a `test_equipment_lifecycle` invocation to `crates/hares-equipment/tests/lifecycle.rs` as part of its own acceptance criteria

### Phase 2 gate — after equipment + envelope both exist
**File: `crates/hares-equipment/tests/lifecycle.rs` (extend)**
- [ ] Equipment → PortSlots → DomainSolver pipeline:
  - [ ] ScheduledLoad writes 1.5 kW to PortSlots
  - [ ] ElectricalSolver.resolve() produces net_active_kw = 1.5
  - [ ] ScheduledLoad writes sensible + latent to thermal PortSlots
  - [ ] ThermalSolver.resolve() changes zone temp in correct direction

### Phase 3 gate — after core lands (HARES-043, 044)
**File: `crates/hares-core/tests/integration.rs`**
- [ ] **Energy pipeline**: 24h with constant 1.0 kW ScheduledLoad → total_electric_kwh = 24.0 ± 0.001
- [ ] **Stage ordering**: PV (Stage 1) port value is available to Battery (Stage 2) self-consumption controller in same timestep
- [ ] **Control dispatch**: `apply_control("Gas Furnace", ControlSignal::thermal_setpoint(...))` → furnace mode changes; `apply_control("Gas Furnace", ControlSignal::soc_target(...))` → Err (wrong capability)
- [ ] **Checkpoint continuity**: run 100 steps, checkpoint at 50, continue to 100, restore, re-run 50-100, assert identical final state (zone temps, SOC, RNG)
- [ ] **Numerical invariants at Dwelling level**: all three balance equations hold for every timestep in a 24h run — explicit assertions, not debug_assert

## Measures of Success
- [ ] Phase 1 type interop tests pass as soon as HARES-001 through 004 are done
- [ ] Phase 1 solver chain tests pass as soon as HARES-012 through 016 are done
- [ ] Phase 2 lifecycle tests grow incrementally — each new equipment type adds one `test_equipment_lifecycle` call
- [ ] Phase 3 integration tests pass with exact numeric assertions
- [ ] All numerical invariants are explicit `assert!` in tests, never relying on `debug_assert`

## Verification
- [ ] `cargo test --test type_interop -p hares-types` passes (after Phase 1 types)
- [ ] `cargo test --test solver_chain -p hares-envelope` passes (after Phase 1 envelope)
- [ ] `cargo test --test lifecycle -p hares-equipment` passes (grows through Phase 2)
- [ ] `cargo test --test integration -p hares-core` passes (after Phase 3 core)
