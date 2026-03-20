# HARES Implementation Backlog

71 implementation tickets spanning 6 phases (~20 weeks). Each ticket maps to one Rust crate (or pure Python package). Tickets within a phase that share no dependency edge can be worked in parallel.

## Ticket Quality Standard

Use these requirements when writing or revising tickets so implementations are clear, reusable, and maintainable:

- Explicit API contract: list exact public functions/types/signatures required.
- Domain naming: require named constants for unit conversions, physical constants, and default coefficients; avoid unexplained numeric literals in implementation code.
- Reusable types: where categories repeat (for example terrain classes, mode families, protocol families), require enums or typed wrappers instead of ad hoc strings/integers.
- Source parity: cite one or more reference implementations/specs and require at least one parity test with known coefficients/cases.
- Behavioral tests: include edge-case expectations (bounds, zero inputs, saturation limits, clamping behavior) and tolerances.
- Scope boundaries: state what belongs in this ticket and what explicitly belongs in other crates/layers.
- Verification commands: include `cargo check`, `cargo test`, and `cargo clippy -D warnings` (or language-equivalent) for touched crates.

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
| [HARES-014](HARES-014.md) | DomainSolver Trait and Thermal Solver | hares-envelope | 012, 013, 003, 007, 008 | 1 |
| [HARES-015](HARES-015.md) | Humidity Solver | hares-envelope | 014, 005 | 1 |
| [HARES-016](HARES-016.md) | Electrical Solver | hares-envelope | 014, 003 | 1 |
| [HARES-017](HARES-017.md) | Fluid Solver (minimal v1) | hares-envelope | 014, 003 | 1 |
| [HARES-018](HARES-018.md) | Equipment Trait, Registry, and Config | hares-equipment | 001, 002, 003, 004 | 2 |
| [HARES-019](HARES-019.md) | Scheduled Load | hares-equipment | 018 | 2 |
| [HARES-020](HARES-020.md) | HVAC Common and Thermostat FSM | hares-equipment | 018 | 2 |
| [HARES-021](HARES-021.md) | Furnace, Baseboard, Boiler | hares-equipment | 020, 017, 006 | 2 |
| [HARES-022](HARES-022.md) | Dynamic HVAC and Biquadratic Performance | hares-equipment | 020, 006 | 2 |
| [HARES-023](HARES-023.md) | Air Conditioner and Room AC | hares-equipment | 022, 005 | 2 |
| [HARES-024](HARES-024.md) | Heat Pump (ASHP + MSHP) | hares-equipment | 022, 023 | 2 |
| [HARES-025](HARES-025.md) | Dehumidifier | hares-equipment | 004, 018, 006 | 2 |
| [HARES-026](HARES-026.md) | Stratified Water Tank | hares-equipment | 018 | 2 |
| [HARES-027](HARES-027.md) | Water Heaters (Resistance, Gas, HPWH, Tankless) | hares-equipment | 026, 020, 006 | 2 |
| [HARES-028](HARES-028.md) | Battery | hares-equipment | 018 | 2 |
| [HARES-029](HARES-029.md) | PV | hares-equipment | 018, 007 | 2 |
| [HARES-030](HARES-030.md) | EV | hares-equipment | 018 | 2 |
| [HARES-031](HARES-031.md) | Generator | hares-equipment | 018 | 2 |

| [HARES-032](HARES-032.md) | Event-Based Loads and Wet Appliance | hares-equipment | 018 | 2 |
| [HARES-033](HARES-033.md) | EPW Weather Parser | hares-io | 002, 005 | 3 |
| [HARES-034](HARES-034.md) | Schedule CSV Parser | hares-io | 002 | 3 |
| [HARES-035](HARES-035.md) | HPXML 4.0 Parser (Building and Envelope) | hares-io | 001, 002 | 3 |
| [HARES-036](HARES-036.md) | HPXML Equipment Resolution and Input Validation | hares-io | 035, 018, 037 | 3 |
| [HARES-037](HARES-037.md) | Equipment Defaults and Simulation Config | hares-io | 001 | 3 |
| [HARES-038](HARES-038.md) | Output Writer (Arrow Streaming) | hares-io | 002, 001, 036 | 3 |
| [HARES-039](HARES-039.md) | Output Metrics | hares-io | 038 | 3 |
| [HARES-040](HARES-040.md) | ResStock Metadata | hares-io | 001, 002 | 3 |
| [HARES-041](HARES-041.md) | Clock and RNG | hares-core | 002 | 3 |
| [HARES-042](HARES-042.md) | Environment Manager | hares-core | 041, 033, 034, 007 | 3 |
| [HARES-043](HARES-043.md) | Dwelling Orchestrator | hares-core | 041, 042, 014, 015, 016, 017, 018, 019, 037, 038, 011 | 3 |
| [HARES-044](HARES-044.md) | Dwelling from_hpxml Constructor | hares-core | 043, 035, 036, 037 | 3 |
| [HARES-045](HARES-045.md) | Telemetry and Checkpoint | hares-core | 043 | 3 |
| [HARES-046](HARES-046.md) | Engine (Batch Runner) | hares-core | 044, 039 | 3 |
| [HARES-047](HARES-047.md) | Fleet Execution | hares-fleet | 046, 040 | 5 |
| [HARES-048](HARES-048.md) | Aggregation and Progress | hares-fleet | 047, 038 | 5 |
| [HARES-049](HARES-049.md) | Core Bindings (Dwelling and Control) | hares-python | 044, 045 | 5 |
| [HARES-050](HARES-050.md) | Fleet Bindings and Conversions | hares-python | 049, 048 | 5 |
| [HARES-051](HARES-051.md) | OCHRE Compat Layer | ochre_next (py) | 049 | 5 |
| [HARES-052](HARES-052.md) | RL Gymnasium Interface | ochre_next (py) | 049, 045 | 5 |
| [HARES-053](HARES-053.md) | External Tool Adapters | ochre_next (py) | 049 | 5 |
| [HARES-054](HARES-054.md) | OCHRE Output Parity Harness | tests | 043, 044 | 4 |
| [HARES-055](HARES-055.md) | BESTEST / ASHRAE Standard 140 Test Suite | tests | 043, 044, 014 | 4 |
| [HARES-056](HARES-056.md) | Benchmark Suite and Performance Gates | tests | 043, 047 | 4 |
| [HARES-057](HARES-057.md) | Regression Corpus and Release Exit Criteria | tests | 054, 055, 056, 062 | 4 |
| [HARES-058](HARES-058.md) | ~~PythonEquipment Adapter~~ *(superseded by HARES-066)* | hares-equipment, hares-python | 018, 049 | 5 |
| [HARES-059](HARES-059.md) | ~~HELICS Co-Simulation Integration~~ *(superseded by HARES-065)* | ochre_next (py) | 049, 043, 047 | 5 |
| [HARES-060](HARES-060.md) | ResStock Python Data Fetchers | ochre_next (py) | 050 | 5 |
| [HARES-061](HARES-061.md) | Cross-Layer Integration Tests (Incremental) | tests | — | 4 |
| [HARES-062](HARES-062.md) | Repository Setup and CI Foundation | workspace | — | 0 |
| [HARES-063](HARES-063.md) | Phase 1 Benchmark Baseline | hares-envelope | 012, 002 | 1 |
| [HARES-064](HARES-064.md) | ControlSignal HumiditySetpoint Variant | hares-types | 004 | 1 |
| [HARES-065](HARES-065.md) | HELICS Co-Simulation Orchestration | ochre_next (py) | 049, 047 | 5 |
| [HARES-066](HARES-066.md) | Python Equipment Adapter | ochre_next (py) | 049, 018 | 5 |
| [HARES-067](HARES-067.md) | ochre_next — nested_update Override Semantics | ochre_next (py) | 051 | 5 |
| [HARES-068](HARES-068.md) | ochre_next — OCHRE Compat simulate/update_model/generate_results | ochre_next (py) | 051 | 5 |
| [HARES-069](HARES-069.md) | Custom Rust Equipment Registration | hares-python | 018, 049 | 5 |
| [HARES-070](HARES-070.md) | Fleet Checkpoint and Restart | hares-fleet | 045, 047 | 5 |
| [HARES-071](HARES-071.md) | Runtime Numerical Invariant Checks | hares-core | 043 | 3 |
| [HARES-072](HARES-072.md) | Schedule Loading Correctness Tests | hares-io | — | 4 |
| [HARES-073](HARES-073.md) | HPXML Config Extraction Correctness Tests | hares-io | — | 4 |
| [HARES-074](HARES-074.md) | Weather/EPW Loading Correctness Tests | hares-io, hares-physics | — | 4 |
| [HARES-075](HARES-075.md) | HVAC Equipment Step Correctness Tests | hares-equipment | — | 4 |
| [HARES-076](HARES-076.md) | Water Heater Equipment Step Correctness Tests | hares-equipment | — | 4 |
| [HARES-077](HARES-077.md) | DER Equipment Step Correctness Tests | hares-equipment | — | 4 |
| [HARES-078](HARES-078.md) | Dwelling Orchestration & Aggregation Tests | hares-core | — | 4 |
| [HARES-079](HARES-079.md) | End-to-End OCHRE Parity Test Harness | tests | 072-078 | 4 |

---

## Fix Tickets (HARES vs OCHRE Audit)

Bug-fix tickets from the HARES vs OCHRE parity audit. Each identifies a specific divergence from the OCHRE reference implementation, cites the relevant source code, and specifies the test that proves the fix works.

### CRITICAL

| ID | Title | Crate |
|----|-------|-------|
| [FIX-001](FIX-001.md) | Schedule fallback produces constant power instead of erroring | hares-io |
| [FIX-002](FIX-002.md) | Missing max_electric_power_w config path | hares-io |
| [FIX-003](FIX-003.md) | HVAC runs with stale zone temperatures | hares-core |
| [FIX-004](FIX-004.md) | HVAC number_of_speeds not derived from HPXML | hares-io |
| [FIX-005](FIX-005.md) | Mains water temperature not wired into dwelling/environment | hares-core, hares-io |
| [FIX-006](FIX-006.md) | DER external control signals partially stubbed (EV missing) | hares-equipment |
| [FIX-007](FIX-007.md) | Startup capacity degradation not extracted from HPXML | hares-io, hares-equipment |
| [FIX-021](FIX-021.md) | PV capacity silently defaults to 0 kW | hares-equipment |
| [FIX-022](FIX-022.md) | AC latent degradation parameters all default to 0.0 | hares-equipment |
| [FIX-023](FIX-023.md) | Scheduled load sensible_gain_fraction defaults to 0.0 | hares-equipment |

### HIGH

| ID | Title | Crate |
|----|-------|-------|
| [FIX-008](FIX-008.md) | PLF bounds missing in biquadratic evaluation | hares-equipment |
| [FIX-009](FIX-009.md) | COP definition mismatch (fan power in denominator) | hares-equipment |
| [FIX-010](FIX-010.md) | Water heater deadband default mismatch | hares-equipment |
| [FIX-011](FIX-011.md) | Generator ramp rate units mismatch | hares-equipment |
| [FIX-012](FIX-012.md) | Water heater tank UA doesn't include jacket R-value | hares-equipment, hares-io |
| [FIX-013](FIX-013.md) | Heat pump backup lockout temperature defaults missing | hares-io, hares-equipment |
| [FIX-014](FIX-014.md) | Ground temperature model amplitude overestimate | hares-io |
| [FIX-015](FIX-015.md) | Multi-speed parameter CSV not loaded | hares-io |
| [FIX-016](FIX-016.md) | Water heater draw profiles are constant average | hares-equipment |
| [FIX-017](FIX-017.md) | Battery degradation formula initial condition divergence | hares-equipment |
| [FIX-018](FIX-018.md) | Auxiliary fan power not scaled by capacity | hares-io |
| [FIX-019](FIX-019.md) | Duct surface area missing for ASHRAE 152 DSE | hares-io |
| [FIX-020](FIX-020.md) | HPWH COP curves and priority logic incomplete | hares-equipment |
| [FIX-024](FIX-024.md) | HPXML window area silently defaults to 0 m² | hares-io |
| [FIX-025](FIX-025.md) | HVAC cutout_ratio and min_cycle_time_s default to 0.0 | hares-equipment |
| [FIX-026](FIX-026.md) | Gas WH pilot power defaults to 0 W | hares-equipment |
| [FIX-027](FIX-027.md) | Heat pump ER hard lockout time defaults to 0 | hares-equipment |
| [FIX-028](FIX-028.md) | HPXML system type parsed as empty string when missing | hares-io |

---

## Phase Assignments

> **Roadmap vs. ticket phasing note:** `docs/architecture/09-roadmap.md` places HPXML/EPW/Schedule parsers and PyO3 bindings in Phase 1 (logical grouping by capability). The ticket backlog defers parsers to Phase 3 (`hares-io`) and PyO3 bindings to Phase 5 (`hares-python`) because these cannot compile without the types from Phase 1 and the equipment/core layers from Phases 2-3 respectively. Both views are valid; the ticket dependency graph is the authoritative schedule. Implementers should use the dependency graph, not the roadmap phases, when sequencing work.

### Phase 0 — Setup: HARES-062

Repository-level infrastructure: `rustfmt.toml`, workspace Clippy lints, MSRV declaration, `docs/PHYSICS_DECISIONS.md` template, and shared workspace dependencies (`postcard`, `tracing-subscriber`). Must land before any crate work begins.

### Phase 1 — Foundation (Weeks 1-4): HARES-001 through HARES-017, HARES-063, HARES-064

Types, physics, control, and envelope layers. Nothing can proceed without the shared types in 001-004, and the envelope solvers (012-017) are on the critical path to the dwelling orchestrator. HARES-063 establishes the RC solver benchmark baseline; HARES-064 adds the `HumiditySetpoint` control signal variant needed by the dehumidifier (HARES-025).

Crates: `hares-types`, `hares-physics`, `hares-control`, `hares-envelope`

### Phase 2 — Equipment (Weeks 5-8): HARES-018 through HARES-032

All concrete equipment implementations. Gated on 001-004 (types) being complete. HARES-018 (trait + registry) must land first; everything else in this phase fans out from it.

Crates: `hares-equipment`

### Phase 3 — I/O and Integration (Weeks 9-12): HARES-033 through HARES-046, HARES-071

Parsers, output writers, and the core dwelling/engine layer. HARES-043 (Dwelling Orchestrator) is the major integration point; it cannot start until Phase 1 envelope work and most of Phase 2 are done. HARES-071 (Runtime Numerical Invariant Checks) is placed in Phase 3 because it wraps around the dwelling orchestrator and enables per-timestep conservation checks as integration work proceeds.

Crates: `hares-io`, `hares-core`

### Phase 4 — Validation (Weeks 13-16): HARES-054 through HARES-057, HARES-061

Cross-validation against OCHRE reference outputs, BESTEST/ASHRAE 140 cases, performance benchmarks, and the regression corpus that gates release. HARES-061 (Cross-Layer Integration Tests) is written incrementally throughout all phases but its Phase 3 gate test (`hares-core/tests/integration.rs`) completes here. HARES-056 (Benchmark Suite) depends on HARES-047 (Fleet) so it runs in late Phase 4 / early Phase 5.

Crates/tests: workspace integration test crates

### Phase 5 — Fleet, Python, and RL (Weeks 17-20): HARES-047 through HARES-053, HARES-058 through HARES-060, HARES-065 through HARES-070

Fleet parallelism, Python bindings (PyO3/maturin), OCHRE compatibility shim, and the Gymnasium RL interface. All depend on HARES-046 (Engine) or HARES-044/045 (Dwelling constructors + telemetry). Also includes: HARES-060 (ResStock Python Data Fetchers), HARES-065 (HELICS Orchestration), HARES-066 (Python Equipment Adapter), HARES-067 (`nested_update` override semantics), HARES-068 (OCHRE compat API methods), HARES-069 (Custom Rust equipment registration via PyO3), and HARES-070 (Fleet checkpoint and restart). Note: HARES-058 and HARES-059 are superseded by HARES-066 and HARES-065 respectively.

Crates: `hares-fleet`, `hares-python`, `ochre_next`

---

## Parallelism Opportunities

Tickets that share no dependency edge within the same phase can be implemented concurrently.

**Phase 0:**

HARES-062 has no dependencies and should land before any crate work. It unblocks all crates that consume workspace dependencies (`postcard`, `tracing-subscriber`).

**Phase 1 — parallel streams:**

```
Stream A (types):    001 -> 002 -> 003
                     001 -> 004 -> 010 -> 011
                     001 -> 004 -> 064 (HumiditySetpoint variant)
Stream B (physics):  005 -> 008 -> 009
                     006
Stream C (envelope): 002 -> 012 -> 013 -> 014 -> {015, 016, 017}
                     002, 012 -> 063 (benchmark baseline)
```

- HARES-002, 003, 004 all depend only on 001 — start them together once 001 merges.
- HARES-005, 006 have no dependencies — can start day one alongside 001.
- HARES-010, 011 (control) and HARES-005-009 (physics) are fully parallel.
- HARES-012-017 (envelope) can proceed in parallel with HARES-010-011 (control).
- HARES-063 (benchmark baseline) and HARES-064 (HumiditySetpoint) are parallel to each other and to envelope/control work.

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
IO parsers:  {033, 034, 035, 037, 040} (all depend only on Phase 1 types)
             035, 037 -> 036 (requires 018 from Phase 2 and 037)
             036 -> 038 -> 039
Core:        002 -> 041 -> 042 -> 043 -> {044, 045} -> 046
             038 -> 039 -> 046 (SimulationMetrics contract)
```

- HARES-033, 034, 035, 037, 040 can all start in parallel at the beginning of Phase 3. HARES-038 depends on 036 (which needs 035 + 037 + 018).
- HARES-041 (Clock/RNG) can also start immediately — it only needs 002.
- HARES-042 (Environment Manager) needs 033, 034, 007, and 041.
- HARES-043 (Dwelling Orchestrator) is the convergence point for all Phase 1 and Phase 2 work.
- HARES-071 (Numerical Invariant Checks) depends only on 043 — start it immediately after 043 merges, in parallel with 044/045.

**Phase 4 — parallel streams:**

```
Validation:  043, 044 -> 054 (OCHRE parity)
             043, 044, 014 -> 055 (BESTEST)
             043, 047 -> 056 (benchmarks — needs Phase 5 fleet, runs late)
             054, 055, 056, 062 -> 057 (regression corpus)
             (no deps)  -> 061 (cross-layer integration tests, incremental)
```

- HARES-054 and HARES-055 can run in parallel — both gate on 043/044 but not each other.
- HARES-061 is written incrementally; its sub-tests become runnable as each layer lands.
- HARES-056 has a dependency on HARES-047 (fleet) so it completes after the fleet crate lands in Phase 5.

**Phase 5 — parallel streams:**

```
Fleet:   046, 040 -> 047 -> 048 -> 050
                     045, 047 -> 070 (fleet checkpoint/restart)
Python:  044, 045 -> 049 -> {050, 051, 052, 053}
                     049, 018 -> {066, 069} (Python equipment adapter + custom registration)
                     049, 047 -> 065 (HELICS orchestration)
         050 -> 060 (ResStock data fetchers)
Compat:  051 -> {067, 068} (nested_update and compat API — both depend only on 051)
```

- HARES-051, 052, 053 all depend only on 049 — fully parallel once 049 merges.
- HARES-050 needs both 049 and 048 — it is the last core ticket to complete.
- HARES-058 is superseded by HARES-066 (Python equipment adapter); implement HARES-066 only.
- HARES-059 is superseded by HARES-065 (HELICS orchestration); implement HARES-065 only.
- HARES-060 depends on HARES-050 (fleet bindings) and has no other Phase 5 dependencies — can start as soon as 050 merges.
- HARES-067 and HARES-068 both depend only on HARES-051 — fully parallel with each other and with 052/053.
- HARES-069 (custom equipment registration) depends on 018 and 049 — parallel with 058/066.
- HARES-070 (fleet checkpoint/restart) depends on 045 and 047 — can start as soon as both land.

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

Note: HARES-043 has eleven direct dependencies (014, 015, 016, 017, 018, 019, 037, 038, 011, 041, 042). The chain above is the longest serial sub-path; the full gate for 043 requires all eleven to be ready.

---

## OCHRE Alignment Test Tickets (HARES-072 through HARES-079)

These tickets create correctness and parity tests that validate HARES against the OCHRE reference implementation. They are test-only tickets — they do NOT fix underlying code; fixes come from findings surfaced by these tests.

All tickets except HARES-079 have no dependencies and can be worked in parallel. HARES-079 (end-to-end harness) depends on all seven preceding tickets.

| ID | Title | Crate(s) | Depends On |
|----|-------|----------|------------|
| [HARES-072](HARES-072.md) | Schedule loading correctness tests against OCHRE | hares-io | — |
| [HARES-073](HARES-073.md) | HPXML config extraction correctness tests against OCHRE | hares-io | — |
| [HARES-074](HARES-074.md) | Weather/EPW loading correctness tests against OCHRE | hares-io, hares-physics | — |
| [HARES-075](HARES-075.md) | HVAC equipment step correctness tests against OCHRE | hares-equipment | — |
| [HARES-076](HARES-076.md) | Water heater equipment step correctness tests against OCHRE | hares-equipment | — |
| [HARES-077](HARES-077.md) | DER equipment step correctness tests against OCHRE | hares-equipment | — |
| [HARES-078](HARES-078.md) | Dwelling orchestration and aggregation correctness tests | hares-core | — |
| [HARES-079](HARES-079.md) | End-to-end OCHRE parity test harness | tests | 072-078 |

```
Parallelism:  {072, 073, 074, 075, 076, 077, 078} (all independent)
                          ↓ all complete ↓
                              079 (e2e harness)
```

---

## Config/Schedule Refactor Tickets (CFG-*)

Unified `ScheduleSource` enum replacing materialized 525K-element schedule arrays with lazy, stateful, zero-copy value sources. Eliminates ~690MB per dwelling of schedule data duplication.

| ID | Title | Crate(s) | Depends On |
|----|-------|----------|------------|
| [CFG-001](CFG-001.md) | Define unified ScheduleSource enum in hares-types | hares-types | — |
| [CFG-002](CFG-002.md) | Replace schedule_kw_N injection with compact config keys | hares-io | CFG-001 |
| [CFG-003](CFG-003.md) | Migrate ScheduledLoad to ScheduleSource | hares-equipment | CFG-002 |
| [CFG-004](CFG-004.md) | Migrate EventBasedLoad to ScheduleSource | hares-equipment | CFG-002 |
| [CFG-005](CFG-005.md) | Consolidate SetpointSource + WH refs to ScheduleSource | hares-equipment, hares-core | CFG-001 |
| [CFG-006](CFG-006.md) | Delete legacy N-key injection and parsing | hares-io, hares-equipment | CFG-003, CFG-004 |

```
Dependency graph:
CFG-001 ──┬── CFG-002 ──┬── CFG-003 ──┬── CFG-006
          │             └── CFG-004 ──┘
          └── CFG-005
```

**Parallel groups:** CFG-003 + CFG-004 + CFG-005 can run in parallel once their deps are met.
