---
id: HARES-011
title: "hares-control — OCHRE Compat Mapping, Dispatch Types, Price Signal"
kind: implement
depends_on: [HARES-010]
files_to_touch:
  - crates/hares-control/src/compat.rs
  - crates/hares-control/src/dispatch.rs
  - crates/hares-control/src/types.rs
  - crates/hares-control/src/lib.rs
references:
  - docs/architecture/03-control-interfaces.md
verification:
  - cargo check -p hares-control
  - cargo test -p hares-control
  - cargo clippy -p hares-control -- -D warnings
---

## Background/Context
OCHRE expresses control actions as string key–value pairs tied to equipment type names. The compatibility shim translates these into typed `ControlSignal` values so the rest of HARES never has to parse OCHRE strings. Dispatch types and price signals are also defined here to give callers a single import location for the full control surface.

**Scope note**: The OCHRE compat mapping types (`DispatchTarget`, `DispatchRequest`) and `PriceSignal` are included in Phase 1 to co-locate all control signal type definitions in the types/control crates. The actual dispatch routing logic — reading `DispatchRequest` values and delivering them to the correct equipment instances — remains in Phase 3 (`hares-core`).

## Work to Do
- [ ] Implement in `compat.rs`:
  - `ochre_signal_to_control(equipment_type: &str, signals: &HashMap<String, f64>) -> Vec<ControlSignal>` — accepts all OCHRE key-value pairs for one equipment instance at once and returns the full set of typed `ControlSignal` values they represent
    - Multi-key grouping for SOC keys:
      - When only `Min SOC` and `Max SOC` are present (no bare `SOC` key), they merge into a single `SOCTarget` with `target_soc` set to `min_soc` (conservative default — don't discharge below minimum), `min_soc` and `max_soc` populated from the input values
      - When `SOC` is present alongside `Min SOC` and/or `Max SOC`, they merge into a single `SOCTarget` with `target_soc` from the `SOC` value and `min_soc`/`max_soc` from the respective keys (never two separate signals)
      - When only `SOC` is present, it populates `target_soc` with `min_soc` and `max_soc` both set to the same value
    - Boolean convention: OCHRE encodes boolean flags as `f64` values where `1.0` means `true` and `0.0` means `false`. Apply this convention when mapping `Self Consumption Mode` → `SelfConsumption { enabled, solar_only_charging }`
    - `SelfConsumption::solar_only_charging` defaults to `false` when only the `Self Consumption Mode` OCHRE key is present (there is no separate OCHRE key for it)
    - Unknown keys are silently skipped (the returned `Vec` simply omits them; no panic or error)
    - Document the `0.0`/`1.0` boolean convention explicitly in a module-level or function-level doc comment
- [ ] Implement in `dispatch.rs`:
  - `DispatchTarget` enum with variants: `ByName(Arc<str>)`, `ByEndUse(EndUse)`
  - `DispatchRequest` struct with fields: `target: DispatchTarget`, `signal: ControlSignal`
  - Note: routing logic lives in `hares-core`, not here
- [ ] Implement in `types.rs`:
  - `PriceSignal` struct with fields: `electricity_price: Option<f64>`, `export_price: Option<f64>`, `ghg_intensity: Option<f64>` — add a doc comment on `ghg_intensity` stating its units: `kg CO₂e/kWh`
  - This is intentionally separate from `ControlSignal`
- [ ] Write tests:
  - OCHRE key mapping: for each of the 7 documented OCHRE keys (`Setpoint Temperature (C)`, `P Setpoint`, `Duty Cycle`, `Load Fraction`, `SOC`, `Min SOC`, `Max SOC`, `Self Consumption Mode`), verify the correct `ControlSignal` variant is produced
  - Multi-key grouping — `Min SOC` + `Max SOC` only: pass both together (no `SOC` key) and verify a single `SOCTarget` is returned with `target_soc == min_soc`
  - Multi-key grouping — `SOC` + `Min SOC` + `Max SOC`: pass all three and verify a single `SOCTarget` is returned with `target_soc` from `SOC`, `min_soc` and `max_soc` from their respective keys
  - Multi-key grouping — `SOC` only: pass only `SOC` and verify `target_soc`, `min_soc`, and `max_soc` are all set to the `SOC` value
  - Multi-key grouping — `Max SOC` only: pass only `Max SOC` and verify `target_soc` falls back to `min_soc` (conservative default)
  - Boolean mapping: pass `Self Consumption Mode` with value `1.0` and verify `SelfConsumption { enabled: true, solar_only_charging: false }`; repeat with `0.0` for `enabled: false`
  - `SelfConsumption::solar_only_charging` defaults to `false` when only `Self Consumption Mode` is present
  - Unknown key: pass a map containing only an unrecognised key and verify `ochre_signal_to_control` returns an empty `Vec`
  - `DispatchTarget` construction for both variants
  - `PriceSignal` struct construction and serde round-trip
  - `DispatchRequest` serde round-trip: construct a `DispatchRequest` containing a `ControlSignal` payload, serialise to JSON, deserialise, and assert equality

## Files to Touch
- `crates/hares-control/src/compat.rs`: new file — OCHRE string-to-typed-signal mapping
- `crates/hares-control/src/dispatch.rs`: new file — `DispatchTarget`, `DispatchRequest`
- `crates/hares-control/src/types.rs`: new file — `PriceSignal`
- `crates/hares-control/src/lib.rs`: module declarations and public re-exports

## Measures of Success
- [ ] `ochre_signal_to_control` covers all OCHRE keys documented in `docs/architecture/03-control-interfaces.md`
- [ ] `Min SOC` + `Max SOC` (without `SOC`) are merged into a single `SOCTarget` with `target_soc` set to `min_soc`
- [ ] `SOC` + `Min SOC`/`Max SOC` merge into a single `SOCTarget` (never two separate signals)
- [ ] `SelfConsumption::solar_only_charging` defaults to `false` when only the `Self Consumption Mode` OCHRE key is present
- [ ] The `0.0`/`1.0` boolean convention is documented in code and exercised by tests
- [ ] Unknown OCHRE keys are silently skipped (empty `Vec` returned, no panic or error)
- [ ] `DispatchRequest` carries both target and signal as owned values
- [ ] `DispatchRequest` serde round-trip test passes with a `ControlSignal` payload
- [ ] `PriceSignal` is not a variant of `ControlSignal` (separate struct)
- [ ] `ghg_intensity` field carries a doc comment stating its units (`kg CO₂e/kWh`)
- [ ] All serde derives are present on `DispatchTarget`, `DispatchRequest`, and `PriceSignal`

## Verification
- [ ] `cargo check -p hares-control` passes
- [ ] `cargo test -p hares-control` passes
- [ ] `cargo clippy -p hares-control -- -D warnings` passes
