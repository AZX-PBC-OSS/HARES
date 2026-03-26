# DER Capability Gaps + EV Actor Refactor

Closes the gap between HARES and the DER_Detection simulation_explorer.
Refactors EV from OCHRE-legacy monolith to clean equipment + actor separation.

## Dependency Graph

```
DER-001 (domain types)
  ├── DER-002 (EV key fix)
  │     └── DER-006 (EV strip-down) ──┬── DER-007 (EV driver actor) ──┐
  │                                    └── DER-009 (EV vehicle catalog) ┤
  │                                                                     └── DER-010 (archetype presets)
  ├── DER-004 (chemistry OCV) ──┬── DER-003 (battery params)
  │                              └── DER-008 (battery catalog)
DER-005 (outdoor temp) — standalone
DER-011 (noisy TimeWindows) — standalone, used by DER-007/DER-010
```

## Parallel Groups

| Phase | Tickets | Notes |
|-------|---------|-------|
| 1 | DER-001, DER-005, DER-011 | Foundation types, outdoor temp, noisy schedules (independent) |
| 2 | DER-002, DER-004 | EV key fix, chemistry OCV |
| 3 | DER-003, DER-006 | Battery params (needs 004), EV strip-down (needs 002) |
| 4 | DER-007, DER-008, DER-009 | Actor (needs 006+011), battery catalog (needs 003+004), vehicle catalog (needs 006) |
| 5 | DER-010 | Archetype presets (needs 007+009+011) |

## Tickets

| ID | Title | Kind | Depends |
|----|-------|------|---------|
| [DER-001](DER-001.md) | Add domain types to hares-types | implement | — |
| [DER-002](DER-002.md) | Fix EV max_charging_kw config key mismatch | fix | DER-001 |
| | | | |
| [DER-003](DER-003.md) | Wire Battery config params through Python bindings | implement | DER-001, DER-004 |
| [DER-004](DER-004.md) | Chemistry-aware OCV table selection | implement | DER-001 |
| [DER-005](DER-005.md) | Add outdoor temperature to simulation output | implement | — |
| [DER-006](DER-006.md) | Refactor EV equipment to dumb battery-on-wheels | implement | DER-001, DER-002 |
| [DER-007](DER-007.md) | Implement EV Driver Actor | implement | DER-006, DER-011 |
| [DER-008](DER-008.md) | Battery product catalog (Rust + Python) | implement | DER-003, DER-004 |
| [DER-009](DER-009.md) | EV vehicle catalog (Rust + Python) | implement | DER-006 |
| [DER-010](DER-010.md) | EV archetype presets (Rust + Python) | implement | DER-007, DER-009, DER-011 |
| [DER-011](DER-011.md) | Add optional noise to TimeWindows | implement | — |
