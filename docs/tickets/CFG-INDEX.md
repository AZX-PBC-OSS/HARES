# CFG — Config Infrastructure

Two distinct sub-series under this prefix.

---

## CFG-001 through CFG-006 — ScheduleSource refactoring

Compact ScheduleSource enum replacing the 525K-key-per-equipment schedule
injection approach. Independent of the config key mismatch work below.

| ID | Title | Depends |
|----|-------|---------|
| CFG-001 | Define unified ScheduleSource enum with value_at() in hares-types | — |
| CFG-002 | Replace schedule_kw_N injection with compact ScheduleSource config keys | CFG-001 |
| CFG-003 | Migrate ScheduledLoad from PowerScheduleSource to unified ScheduleSource | CFG-002 |
| CFG-004 | Migrate EventBasedLoad to unified ScheduleSource | CFG-002 |
| CFG-005 | Consolidate SetpointSource and WaterHeaterScheduleRefs into ScheduleSource | CFG-001 |
| CFG-006 | Cross-crate integration tests for compact ScheduleSource flow | CFG-003, CFG-004, CFG-005 |

---

## CFG-007 through CFG-016 — Typed equipment config structs (P0-A / CC-001)

Eliminates all `Map<String, Value>` magic-string config keys for built-in
equipment. This is the highest-priority architectural change in the codebase,
fixing ~25 confirmed key mismatches (CW series, AR-002, AR-005) where resolvers
write one key name and equipment reads a different name, silently using defaults.

**Parent review findings:** CC-001, CC-003, CC-005, AR-002 F-1 through F-4,
AR-005 F-002 through F-004, CW-001 through CW-022.

**Strategy reference:** `docs/tickets/review/FIX-STRATEGY.md` P0-A, P0-C, P0-D.

**Gap analysis:** `docs/tickets/review/CO-GAP-ANALYSIS.md`

### Dependency graph

```
CFG-007 (foundation: EquipmentTypedConfig trait, ConfigPayload enum, shim)
  ├── CFG-008 (registry name fixes + hard error on registration failure)
  ├── CFG-009 (HVAC heating typed configs: Furnace, Boiler, Baseboard, Ideal)
  ├── CFG-010 (HVAC cooling + heat pumps + Dehumidifier typed configs)
  │     [CFG-009 and CFG-010 can run in parallel]
  │
  └── CFG-011 (resolver migration — HVAC writes typed configs)  ← needs CFG-009, CFG-010
        │
        ├── CFG-012 (water heaters: typed configs + resolver migration)
        │
        └── CFG-013 (DER, loads, Ventilation: typed configs + resolver migration)
              │
              └── CFG-014 (exhaustive registry coverage test + round-trip suite)  ← needs CFG-008
                    │
                    └── CFG-015 (remove Raw path for built-in equipment + key tracking)
                          │
                          └── CFG-016 (CI grep guard: no magic-string config access)
```

Note: CFG-012 and CFG-013 can run in parallel (both depend only on CFG-007,
not on each other or on CFG-011). CFG-011 and CFG-012/CFG-013 can also proceed
in parallel since they each touch independent equipment categories.

### Ticket table

| ID | Title | Kind | Depends | Fixes |
|----|-------|------|---------|-------|
| CFG-007 | EquipmentTypedConfig trait, ConfigPayload enum, migration shim | implement | CFG-006 | CC-001 foundation |
| CFG-008 | Registry name fixes + registration failure → hard error | implement | CFG-007 | AR-005 F-002..F-004, AR-005 F-008..F-009, CC-005 |
| CFG-009 | Typed config structs — HVAC heating (Furnace, Boiler, Baseboard, IdealHvac) | implement | CFG-007 | CW-001, CW-002, CW-004, CW-007 |
| CFG-010 | Typed config structs — HVAC cooling + heat pumps + Dehumidifier | implement | CFG-007 | CW-005, CW-006, CW-008, CW-009, CW-010, UC-002, CW-019 |
| CFG-011 | Resolver migration — HVAC resolvers write typed configs | implement | CFG-009, CFG-010 | AR-002 F-1..F-3 (HVAC), CW-001..CW-010 (activated) |
| CFG-012 | Typed config structs — water heaters + resolver migration | implement | CFG-007 | CW-012, CW-013, CW-014, AR-002 F-1 (WH side) |
| CFG-013 | Typed config structs — DER, loads, Ventilation + resolver migration | implement | CFG-007 | AR-002 F-2, AR-002 F-3, AR-002 F-4, CW-016, CW-018 |
| CFG-014 | Exhaustive registry coverage test + config round-trip test suite | implement | CFG-008, CFG-011, CFG-012, CFG-013 | AR-005 T-001, CC-001 regression guard |
| CFG-015 | Remove Raw path for built-in equipment + init-time key tracking | implement | CFG-014 | CC-003, CC-006 (partial), AR-002 F-6 |
| CFG-016 | CI grep guard — no magic-string config access in equipment init | implement | CFG-015 | CC-001 final, permanent regression prevention |

### What each ticket fixes (review finding cross-reference)

| Review finding | Ticket | Description |
|----------------|--------|-------------|
| CW-001 | CFG-009, CFG-011 | Gas furnace AFUE key: `"heating_efficiency"` vs `"fuel_efficiency"` |
| CW-002 | CFG-009, CFG-011 | Electric furnace EIR key: `"heating_efficiency"` vs `"eir"` |
| CW-004 | CFG-009, CFG-011 | Gas boiler AFUE key: same mismatch as CW-001 |
| CW-005 | CFG-010, CFG-011 | Central AC SEER key: `"efficiency_seer"` vs `"seer"`/`"SEER"` |
| CW-006 | CFG-010, CFG-011 | Room AC EER key: `"cooling_efficiency"` vs `"eer"` |
| CW-007 | CFG-009, CFG-011 | Electric boiler EIR key: `"heating_efficiency"` vs `"eir"` |
| CW-008 | CFG-010, CFG-011 | HP per-stage SHR loaded but discarded |
| CW-009 | CFG-010, CFG-011 | Mini-split speed count not inferred as 4 |
| CW-010 | CFG-010, CFG-011 | HP cooling: combines CW-008 + CW-009 |
| CW-012 | CFG-012 | Gas WH fuel type casing: `"FuelType"` vs `"fuel_type"` |
| CW-013 | CFG-012 | HPWH backup element: `"heating_capacity_w"` vs `"backup_element_power_w"` |
| CW-014 | CFG-012 | Tankless WH fuel type casing: same as CW-012 |
| CW-016 | CFG-013 | Battery double-sqrt: resolver writes `rte.sqrt()`, equipment applies sqrt again |
| CW-018 | CFG-013 | Ventilation flow rate: `"ventilation_rate_cfm"` (CFM) vs `"flow_rate_m3_s"` (m³/s) |
| CW-019 | CFG-010 | Dehumidifier config not parsed (CW-019) |
| AR-002 F-1 | CFG-011, CFG-012 | GasWH/TanklessWH read `"FuelType"` (wrong case) |
| AR-002 F-2 | CFG-013 | EV PlugLoad path writes `"battery_capacity_kwh"`, reads `"capacity_kwh"` |
| AR-002 F-3 | CFG-013 | Ventilation: 3 key mismatches + wrong unit (CFM vs m³/s) |
| AR-002 F-4 | CFG-013 | FuelType serialized via `{:?}` Debug format |
| AR-002 F-6 | CFG-015 | Python override keys silently ignored |
| AR-005 F-002 | CFG-008 | `"Gas Tankless Water Heater"` not registered |
| AR-005 F-003 | CFG-008 | `"Generic Heater"` / `"Generic Cooler"` not registered |
| AR-005 F-004 | CFG-008 | `"Water Heating"` fallback not registered |
| AR-005 F-008 | CFG-008 | Equipment init() failure silenced with `continue` |
| AR-005 F-009 | CFG-008 | Registry create() failure warning-only |
| AR-005 T-001 | CFG-014 | Missing test: all resolver names registered |
| UC-002 | CFG-010, CFG-011 | Cooling efficiency routing via SEER |
| CC-001 | CFG-007..CFG-016 | Typed config structs per equipment (full series) |
| CC-003 | CFG-015 | Init-time config key consumption tracking |
| CC-005 | CFG-008 | Silent equipment drop on registration failure |
| CC-006 | CFG-015 | Silent default on missing required input (partial) |
