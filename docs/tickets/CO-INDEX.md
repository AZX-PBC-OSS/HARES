# CO — Typed CoreOutput Architecture

Tickets for replacing string-based telemetry reads in core simulation paths with
typed `CoreOutput` on the `Equipment` trait.

Plan: `~/.claude/plans/linked-herding-island.md`
Parent audit: `docs/tickets/review/AR-001.md`

## Dependency graph

```
CO-001 (types)
  ├── CO-002 (trait + default stub)
  │     ├── CO-003 (dwelling migration)
  │     │     └── CO-005 (actor migration)
  │     └── CO-004 (equipment impls — batch 1: HVAC)
  │     └── CO-006 (equipment impls — batch 2: DER/loads)
  │     └── CO-007 (equipment impls — batch 3: water heaters)
  │           └── CO-008 (revert duplicate electric_kw writes)
  │                 └── CO-009 (validation + lifecycle tests)
  │                       └── CO-010 (Python bindings)
  │                             └── CO-011 (CI grep guard + cleanup)
```

## Key design notes

- Gas WH/Tankless(gas) have ELECTRIC capability (fan/parasitic draw)
- ScheduledLoad/EventLoad have optional FUEL capability (gas cooking, etc.)
- `equipment_states` REMOVED from DwellingTelemetry (resolved in CO-003)
- `prior_electrical_summary` telemetry reads migrated in CO-003
- `equipment_core` population is CO-003's responsibility; CO-005 only consumes
- `core_output` is NOT checkpointed — recomputed on next step after restore
- `equipment_core` populated end-of-timestep, actors read next-timestep
- CO series assumes CFG typed-config series lands in parallel for end-to-end safety

## Tickets

| ID | Title | Kind | Depends |
|----|-------|------|---------|
| CO-001 | Add CoreOutput types to hares-types | implement | — |
| CO-002 | Add core_output() to Equipment trait with default stub | implement | CO-001 |
| CO-003 | Migrate dwelling to read CoreOutput instead of telemetry strings | implement | CO-002 |
| CO-004 | Implement core_output() for HVAC equipment | implement | CO-002 |
| CO-005 | Migrate actors to read CoreOutput from EnvironmentState | implement | CO-003 |
| CO-006 | Implement core_output() for DER + loads | implement | CO-002 |
| CO-007 | Implement core_output() for water heaters | implement | CO-002 |
| CO-008 | Revert duplicate electric_kw writes + remove default stub | implement | CO-004, CO-006, CO-007 |
| CO-009 | Capability validation, lifecycle tests, cross-checks | implement | CO-008 |
| CO-010 | Expose CoreOutput in Python bindings | implement | CO-009 |
| CO-011 | CI grep guard + final cleanup | implement | CO-010 |
