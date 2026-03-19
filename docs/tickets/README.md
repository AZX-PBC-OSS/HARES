# HARES Implementation Backlog

53 implementation tickets spanning 5 phases (~20 weeks). Each ticket maps to one Rust crate (or pure Python package). Tickets within a phase that share no dependency edge can be worked in parallel.

---

## Ticket Index

| ID | Title | Crate | Depends On | Phase |
|----|-------|-------|------------|-------|
| [HARES-001](HARES-001.md) | Core IDs, Enums, and Error Types | hares-types | — | 1 |
| [HARES-002](HARES-002.md) | Environment Types | hares-types | 001 | 1 |
| [HARES-003](HARES-003.md) | Port Types | hares-types | 001, 002 | 1 |
| [HARES-004](HARES-004.md) | Control Signal Types | hares-types | 001 | 1 |
| [HARES-005](HARES-005.md) | Psychrometrics and Air Properties | hares-physics | — | 1 |
| [HARES-006](HARES-006.md) | Biquadratic Curves | hares-physics | — | 1 |
| [HARES-007](HARES-007.md) | Solar Geometry | hares-physics | 002 | 1 |
| [HARES-008](HARES-008.md) | Infiltration Functions | hares-physics | 005 | 1 |
| [HARES-009](HARES-009.md) | Units Module | hares-physics | 001, 005, 008 | 1 |
| [HARES-010](HARES-010.md) | Signal Definitions and Capabilities | hares-control | 004 | 1 |
| [HARES-011](HARES-011.md) | OCHRE Compat Mapping, Dispatch Types, Price Signal | hares-control | 010 | 1 |
| [HARES-012](HARES-012.md) | State-Space Solver | hares-envelope | 002 | 1 |
| [HARES-013](HARES-013.md) | RC Network Construction | hares-envelope | 012 | 1 |
| [HARES-014](HARES-014.md) | DomainSolver Trait and Thermal Solver | hares-envelope | 012, 013, 003, 008 | 1 |
| [HARES-015](HARES-015.md) | Humidity Solver | hares-envelope | 014, 005 | 1 |
| [HARES-016](HARES-016.md) | Electrical Solver | hares-envelope | 014, 003 | 1 |
| [HARES-017](HARES-017.md) | Fluid Solver (minimal v1) | hares-envelope | 014, 003 | 1 |
| [HARES-018](HARES-018.md) | Equipment Trait, Registry, and Config | hares-equipment | 001, 002, 003, 004 | 2 |
| [HARES-019](HARES-019.md) | Scheduled Load | hares-equipment | 018 | 2 |
| [HARES-020](HARES-020.md) | HVAC Common and Thermostat FSM | hares-equipment | 018, 006 | 2 |
| [HARES-021](HARES-021.md) | Furnace, Baseboard, Boiler | hares-equipment | 020 | 2 |
| [HARES-022](HARES-022.md) | Dynamic HVAC and Biquadratic Performance | hares-equipment | 020, 006 | 2 |
| [HARES-023](HARES-023.md) | Air Conditioner and Room AC | hares-equipment | 022, 005 | 2 |
| [HARES-024](HARES-024.md) | Heat Pump (ASHP + MSHP) | hares-equipment | 022, 023 | 2 |
| [HARES-025](HARES-025.md) | Dehumidifier | hares-equipment | 018, 006 | 2 |
| [HARES-026](HARES-026.md) | Stratified Water Tank | hares-equipment | 018 | 2 |
| [HARES-027](HARES-027.md) | Water Heaters (Resistance, Gas, HPWH, Tankless) | hares-equipment | 026, 020 | 2 |
| [HARES-028](HARES-028.md) | Battery | hares-equipment | 018 | 2 |
| [HARES-029](HARES-029.md) | PV | hares-equipment | 018, 007 | 2 |
| [HARES-030](HARES-030.md) | EV | hares-equipment | 018 | 2 |
| [HARES-031](HARES-031.md) | Generator | hares-equipment | 018 | 2 |
| [HARES-032](HARES-032.md) | Event-Based Loads and Wet Appliance | hares-equipment | 018 | 2 |
| [HARES-033](HARES-033.md) | EPW Weather Parser | hares-io | 002, 005 | 3 |
| [HARES-034](HARES-034.md) | Schedule CSV Parser | hares-io | 002 | 3 |
| [HARES-035](HARES-035.md) | HPXML 4.0 Parser (Building and Envelope) | hares-io | 001, 002 | 3 |
| [HARES-036](HARES-036.md) | HPXML Equipment Resolution and Input Validation | hares-io | 035, 018 | 3 |
| [HARES-037](HARES-037.md) | Equipment Defaults and Simulation Config | hares-io | 001 | 3 |
| [HARES-038](HARES-038.md) | Output Writer (Arrow Streaming) | hares-io | 001, 002 | 3 |
| [HARES-039](HARES-039.md) | Output Metrics | hares-io | 038 | 3 |
| [HARES-040](HARES-040.md) | ResStock Metadata | hares-io | 001, 002 | 3 |
| [HARES-041](HARES-041.md) | Clock and RNG | hares-core | 002 | 3 |
| [HARES-042](HARES-042.md) | Environment Manager | hares-core | 041, 033, 034, 007 | 3 |
| [HARES-043](HARES-043.md) | Dwelling Orchestrator | hares-core | 041, 042, 014, 015, 016, 017, 018, 019, 038, 011 | 3 |
| [HARES-044](HARES-044.md) | Dwelling from_hpxml Constructor | hares-core | 043, 035, 036, 037 | 3 |
| [HARES-045](HARES-045.md) | Telemetry and Checkpoint | hares-core | 043 | 3 |
| [HARES-046](HARES-046.md) | Engine (Batch Runner) | hares-core | 044 | 3 |
| [HARES-047](HARES-047.md) | Fleet Execution | hares-fleet | 046, 040 | 5 |
| [HARES-048](HARES-048.md) | Aggregation and Progress | hares-fleet | 047 | 5 |
| [HARES-049](HARES-049.md) | Core Bindings (Dwelling and Control) | hares-python | 044, 045 | 5 |
| [HARES-050](HARES-050.md) | Fleet Bindings and Conversions | hares-python | 049, 048 | 5 |
| [HARES-051](HARES-051.md) | OCHRE Compat Layer | ochre_next (py) | 049 | 5 |
| [HARES-052](HARES-052.md) | RL Gymnasium Interface | ochre_next (py) | 049, 045 | 5 |
| [HARES-053](HARES-053.md) | External Tool Adapters | ochre_next (py) | 049 | 5 |

---

## Phase Assignments

### Phase 1 — Foundation (Weeks 1-4): HARES-001 through HARES-017

Types, physics, control, and envelope layers. Nothing can proceed without the shared types in 001-004, and the envelope solvers (012-017) are on the critical path to the dwelling orchestrator.

Crates: `hares-types`, `hares-physics`, `hares-control`, `hares-envelope`

### Phase 2 — Equipment (Weeks 5-8): HARES-018 through HARES-032

All concrete equipment implementations. Gated on 001-004 (types) being complete. HARES-018 (trait + registry) must land first; everything else in this phase fans out from it.

Crates: `hares-equipment`

### Phase 3 — I/O and Integration (Weeks 9-12): HARES-033 through HARES-046

Parsers, output writers, and the core dwelling/engine layer. HARES-043 (Dwelling Orchestrator) is the major integration point; it cannot start until Phase 1 envelope work and most of Phase 2 are done.

Crates: `hares-io`, `hares-core`

### Phase 4 — Validation (Weeks 13-16): no tickets

Dedicated phase for cross-validation against OCHRE reference outputs, performance profiling, and fixing regressions found during integration. No new feature tickets are scheduled here by design.

### Phase 5 — Fleet, Python, and RL (Weeks 17-20): HARES-047 through HARES-053

Fleet parallelism, Python bindings (PyO3/maturin), OCHRE compatibility shim, and the Gymnasium RL interface. All depend on HARES-046 (Engine) or HARES-044/045 (Dwelling constructors + telemetry).

Crates: `hares-fleet`, `hares-python`, `ochre_next`

---

## Parallelism Opportunities

Tickets that share no dependency edge within the same phase can be implemented concurrently.

**Phase 1 — parallel streams:**

```
Stream A (types):    001 -> 002 -> 003
                     001 -> 004 -> 010 -> 011
Stream B (physics):  005 -> 008 -> 009
                     006
Stream C (envelope): 002 -> 012 -> 013 -> 014 -> {015, 016, 017}
```

- HARES-002, 003, 004 all depend only on 001 — start them together once 001 merges.
- HARES-005, 006 have no dependencies — can start day one alongside 001.
- HARES-010, 011 (control) and HARES-005-009 (physics) are fully parallel.
- HARES-012-017 (envelope) can proceed in parallel with HARES-010-011 (control).

**Phase 2 — parallel streams within equipment:**

Once HARES-018 lands, three independent streams open:

```
HVAC stream:   018 -> 020 -> {021, 022} -> 023 -> 024
                      020 -> 025 (dehumidifier, parallel to 021/022)
WH stream:     018 -> 026 -> 027
DER stream:    018 -> {028, 030, 031, 032}
               018, 007 -> 029
```

- HARES-019 (Scheduled Load) is independent of the HVAC, WH, and DER streams — it only needs 018.
- HARES-021 and HARES-022 both depend on 020 but not on each other.
- HARES-028, 030, 031, 032 all depend only on 018 — fully parallel with each other and with the HVAC/WH streams.

**Phase 3 — parallel streams within I/O and core:**

```
IO parsers:  {033, 034, 035, 037, 038, 040} (all depend only on Phase 1 types)
             035 -> 036 (requires 018 from Phase 2)
             038 -> 039
Core:        002 -> 041 -> 042 -> 043 -> {044, 045} -> 046
```

- HARES-033, 034, 035, 037, 038, 040 can all start in parallel at the beginning of Phase 3.
- HARES-041 (Clock/RNG) can also start immediately — it only needs 002.
- HARES-042 (Environment Manager) needs 033, 034, 007, and 041.
- HARES-043 (Dwelling Orchestrator) is the convergence point for all Phase 1 and Phase 2 work.

**Phase 5 — parallel streams:**

```
Fleet:   046, 040 -> 047 -> 048 -> 050
Python:  044, 045 -> 049 -> {050, 051, 052, 053}
```

- HARES-051, 052, 053 all depend only on 049 — fully parallel once 049 merges.
- HARES-050 needs both 049 and 048 — it is the last ticket to complete.

---

## Critical Path

The longest dependency chain from project start to a running single-building simulation:

```
001 -> 002 -> 012 -> 013 -> 014 -> 043 -> 044 -> 046
```

Expanded:

| Step | Ticket | Milestone |
|------|--------|-----------|
| 001 | Core IDs, Enums, Error Types | Shared vocabulary available |
| 002 | Environment Types | Weather/zone state types available |
| 012 | State-Space Solver | ZOH discretisation working |
| 013 | RC Network Construction | RC matrices can be built from topology |
| 014 | DomainSolver Trait and Thermal Solver | First domain solver integrated |
| 043 | Dwelling Orchestrator | End-to-end single-building timestep loop |
| 044 | Dwelling from_hpxml Constructor | Buildings can be constructed from ResStock inputs |
| 046 | Engine (Batch Runner) | CLI and batch runs possible |

Note: HARES-043 has ten direct dependencies (014, 015, 016, 017, 018, 019, 038, 011, 041, 042). The chain above is the longest serial sub-path; the full gate for 043 requires all ten to be ready.
