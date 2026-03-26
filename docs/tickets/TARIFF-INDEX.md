# TARIFF — Utility Tariff & Battery/EV Management System

Utility rate schedules at the dwelling/simulation level with realistic BMS modes for battery and EV charging strategies that consume tariff information.

## Architecture

- **Tariff** is dwelling-level configuration, not equipment-level
- **Equipment stays physics-only** — `hares-equipment` does NOT depend on `hares-tariff`
- **Environment is immutable input** — `EnvironmentState` carries `PriceSignal` and `ElectricalSummary` as read-only data. Actors never mutate environment — they only emit `DispatchRequest`s
- **Tariff evaluation is env prep, not an actor** — the dwelling owns `TariffEvaluator` and populates `env.price_signal` during Step 1 (environment update), like weather. No TariffActor needed
- **Electrical summary is prior-step observation** — `env.electrical` carries PV generation, base load, net grid from the previous timestep's solver results
- **Billing accumulation is post-step** — dwelling calls `tariff_evaluator.step()` after electrical solver, using actual metered power
- **Actors read env, emit dispatches** — BMS/EV actors read `env.price_signal` and `env.electrical`, emit `ControlSignal` at Schedule tier; Python overrides at UserOverride/Grid tier
- **Hot loop clean** — precomputed price array (O(1) lookup), no allocations per step

## Dependency Graph

```
TARIFF-001 (types: SeasonFilter, BillingCycle, TouPeriod)
├── TARIFF-002 (types: BmsMode, GridExportRule)
│   ├── TARIFF-008 (wire to equipment config) ──┐
│   ├── TARIFF-010 (BatteryManagementActor) ────┤
│   └── TARIFF-014 (Python BmsMode bindings)    │
├── TARIFF-003 (types: ChargingStrategy ext)    │
│   ├── TARIFF-008 ─────────────────────────────┤
│   ├── TARIFF-011 (EVChargingActor) ───────────┤
│   └── TARIFF-014 ────────────────────────────┐│
└── TARIFF-004 (crate: hares-tariff types)     ││
    ├── TARIFF-005 (TariffEvaluator)           ││
    │   └── TARIFF-006 (BillingState)          ││
    │       └── TARIFF-009 (env + dwelling) ───┤│
    │           ├── TARIFF-010 ────────────────┤│
    │           └── TARIFF-011 ────────────────┤│
    └── TARIFF-007 (URDB importer)             ││
        └── TARIFF-013 (Python tariff bindings)││
                                               ││
TARIFF-012 (auto-register BMS/EV actors) ◄─────┘│
TARIFF-015 (Python telemetry bindings) ◄─────────┘
TARIFF-016 (integration test) ◄── all above

Key: TARIFF-009 is NOT an actor — it adds PriceSignal + ElectricalSummary
to EnvironmentState and integrates TariffEvaluator into the dwelling's
env prep phase (Step 1 of run_timestep). No TariffActor exists.
```

## Parallel Execution Opportunities

These ticket groups touch independent file sets and can run in parallel:

| Group A (tariff crate) | Group B (BMS types) | Group C (EV types) |
|---|---|---|
| TARIFF-004 | TARIFF-002 | TARIFF-003 |
| TARIFF-005 | | |
| TARIFF-006 | | |
| TARIFF-007 | | |

After the types layer: TARIFF-008 (equipment config) is sequential.
After actors: TARIFF-013 and TARIFF-014 can run in parallel (different Python files).

## Ticket Summary

| ID | Title | Kind | Depends On |
|---|---|---|---|
| TARIFF-001 | SeasonFilter, BillingCycle, TouPeriod types | implement | — |
| TARIFF-002 | BmsMode, GridExportRule types | implement | 001 |
| TARIFF-003 | ChargingStrategy V2H/V2G/SolarSurplus extension | implement | 001 |
| TARIFF-004 | Create hares-tariff crate with ElectricTariff/GasTariff | implement | 001 |
| TARIFF-005 | TariffEvaluator with precomputed price array | implement | 004 |
| TARIFF-006 | BillingState and demand/energy accumulation | implement | 005 |
| TARIFF-007 | URDB v7 JSON importer | implement | 004 |
| TARIFF-008 | Wire BmsMode/ChargingStrategy to equipment config | implement | 002, 003 |
| TARIFF-009 | PriceSignal/ElectricalSummary in env + TariffEvaluator dwelling integration | implement | 006 |
| TARIFF-010 | BatteryManagementActor (all BMS modes) | implement | 002, 008, 009 |
| TARIFF-011 | EVChargingActor (all charging strategies) | implement | 003, 008, 009 |
| TARIFF-012 | Auto-register built-in BMS/EV actors in dwelling init | implement | 008, 009, 010, 011 |
| TARIFF-013 | ElectricTariff/GasTariff PyO3 bindings + builder | implement | 007 |
| TARIFF-014 | BmsMode/ChargingStrategy PyO3 bindings | implement | 002, 003 |
| TARIFF-015 | BillingPeriodSummary/TariffTelemetry PyO3 bindings | implement | 013, 009 |
| TARIFF-016 | Integration test — full TOU scenario | implement | 012, 013, 014, 015 |
