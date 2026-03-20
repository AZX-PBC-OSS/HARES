# FIX-031: Decomposition Summary

## Dependency Graph

```
FIX-031-001  (ThermalCategory enum)
  ├──→ FIX-031-002  (Resistance WH jacket loss)
  ├──→ FIX-031-003  (Boiler jacket loss)         [parallel with 002]
  ├──→ FIX-031-004  (Envelope solver uses categories, remove HVAC subtraction)
  │      └──→ FIX-031-006  (Per-zone output columns)
  └────────→ FIX-031-005  (Observer oracle test) [after 001, 002, 004]
```

## Execution Order

| Step | Ticket | Parallel? | Description |
|------|--------|-----------|-------------|
| 1 | FIX-031-001 | — | Add `ThermalCategory` to ports, update all equipment callers |
| 2 | FIX-031-002 | with 003 | Resistance WH jacket loss to thermal port |
| 2 | FIX-031-003 | with 002 | Boiler jacket loss to thermal port |
| 3 | FIX-031-004 | — | Envelope solver reads categories, remove HVAC subtraction hack |
| 4 | FIX-031-005 | with 006 | Observer-based per-equipment oracle test |
| 4 | FIX-031-006 | with 005 | Per-zone output columns (infiltration, LWR) |

## File Overlap Analysis

| File | Tickets |
|------|---------|
| `crates/hares-types/src/ports.rs` | 001 only |
| `crates/hares-equipment/src/water_heater/resistance.rs` | 002 only |
| `crates/hares-equipment/src/hvac/boiler.rs` | 003 only |
| `crates/hares-equipment/src/scheduled_load.rs` | 001 only |
| `crates/hares-equipment/src/hvac/heat_pump/heater.rs` | 001 only |
| `crates/hares-equipment/src/hvac/air_conditioner.rs` | 001 only |
| `crates/hares-envelope/src/thermal_solver/mod.rs` | 004, 006 (sequential) |
| `crates/hares-envelope/src/thermal_solver/config.rs` | 004, 006 (sequential) |
| `crates/hares-core/src/dwelling/mod.rs` | 004, 006 (sequential) |
| `tests/envelope_oracle.rs` | 005 only |

No conflicting parallel writes — 002 and 003 touch different equipment files.
004 and 006 share envelope/dwelling files but are sequenced by dependency.

## Expected Outcomes

After all tickets:
- Every equipment reports its thermal category (HVAC vs internal vs jacket loss)
- No post-hoc HVAC subtraction — categories are deterministic from the source
- Resistance WH jacket loss appears in zone thermal port (~39 W for BEopt)
- Boiler jacket loss appears in zone thermal port
- Per-zone infiltration, LWR in output columns
- Observer test validates per-equipment breakdown against OCHRE
- Internal gains gap (474 → ~342 W) diagnosed and narrowed
