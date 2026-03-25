# ACTOR: IdealHvac Equipment, Actor Model, Solver Decoupling

Extract ideal HVAC from the thermal solver into equipment. Lay down the actor trait + control channel infrastructure for push-based equipment control. Clean three-layer separation: actors (decisions) → equipment (mechanical) → solvers (physics).

Principles: separate concerns, DRY, tested, maintainable, clean and extensible architecture. Zero hot-loop allocations. Config is init-time, ControlSignals are runtime. Actors dispatch signals only — never mutate state directly.

## Execution Order

Run tickets 001→015 sequentially. Each depends only on earlier tickets.

| # | Title | Depends On |
|---|-------|------------|
| 001 | Add IdealCapacity control signal + Equipment::ideal_target() | — |
| 002 | Add solve_ideal_capacity_for_target to ThermalSolver | — |
| 003 | Define Actor trait + wire into Dwelling timestep loop | 001 |
| 004 | Implement IdealHvac equipment (with IdealCapacityMode) | 001 |
| 005 | Remove ideal HVAC internals from thermal solver | 002 |
| 006 | Wire IdealHvac + SolverFeedbackActor in Dwelling | 001,003,004,005 |
| 007 | Update integration tests | 006 |
| 008 | **Review checkpoint** — solver decoupling | 005,006,007 |
| 009 | Generate OCHRE conditioned reference fixtures | — |
| 010 | Conditioned oracle integration test (3 scenarios × 2 modes) | 007,009 |
| 011 | IdealThermostat actor (external setpoint overrides) | 003,007 |
| 012 | Occupant actor (lights, appliances, EV behavior) | 001,003 |
| 013 | DR compliance actor (demand response decisions) | 003,012 |
| 014 | Actor registry + Python bindings | 003 |
| 015 | **Final review** | all |

## Parallelism (when running with multiple agents)

- **Parallel group 1**: 001, 002, 009 (independent file sets)
- **After group 1**: 003 (needs 001), 004 (needs 001), 005 (needs 002)
- **Sequential core**: 006 → 007 → 008 (review)
- **After core**: 010 (needs 007+009), 011-014 (need 003+007)
