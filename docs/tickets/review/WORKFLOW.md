# Review Ticket Workflow

## Purpose

These tickets are **investigation-only**. They produce written findings, not code changes.
The goal is to systematically trace HARES behavior against OCHRE, identify divergences,
and classify each as: bug, gap, intentional improvement, or acceptable difference.

## Agent Instructions

When executing a review ticket:

1. **Read the HARES code** referenced in `files_to_touch`. Read it end-to-end — don't
   skim. Note specific line numbers for key logic.

2. **Read the OCHRE reference code** listed in `references`. Same thoroughness.

3. **Compare systematically.** Build tables where the ticket asks for them. Don't
   hand-wave — trace actual values through actual code paths.

4. **Classify divergences** using these categories:
   - **BUG**: HARES behavior is wrong (wrong formula, dropped value, wrong unit).
     Recommend a specific fix with file:line.
   - **GAP**: OCHRE does something HARES doesn't do at all (missing feature, missing
     field). Recommend whether to implement and priority.
   - **IMPROVEMENT**: HARES does something better than OCHRE (better physics, more
     parameters, EnergyPlus-grade model). Document the improvement and why it's
     better — do NOT recommend removing it to match OCHRE.
   - **ACCEPTABLE DIFFERENCE**: HARES does it differently but equivalently or with
     negligible impact. Document why the difference is acceptable.

5. **Write findings directly into the ticket file** under `## Findings`.
   Include:
   - Specific file paths and line numbers
   - Code snippets for key formulas
   - Links to reference documentation (ASHRAE, EnergyPlus, etc.) where relevant
   - Recommended fixes (with priority: critical / high / medium / low)

6. **MANDATORY: Recommend test coverage** under `## Recommended Tests`.
   Every ticket must include specific, actionable test recommendations:
   - **Unit tests** that would catch the specific issue (e.g., "test that °F→°C
     conversion of 212°F yields 100°C, not 100°F × 5/9")
   - **Property-based tests** where applicable (e.g., "round-trip unit conversion
     should be identity within ε")
   - **Oracle tests** comparing HARES output against OCHRE for a reference building
   - **Regression tests** that would prevent the issue from recurring
   - **Invariant tests** (e.g., "total zone heat gains = sum of per-category gains")
   - Name the test, describe what it asserts, and say where it should live.
   - If existing tests already cover the area, note them and whether they're sufficient.

7. **Flag magic strings, premature aggregation, and type safety issues.**
   While investigating, watch for and document:
   - **Magic strings** used as config keys, telemetry keys, port identifiers, or
     equipment names. These are brittle — recommend replacing with typed constants,
     enums, or newtype wrappers.
   - **Premature aggregation** where thermal/energy values are summed before being
     reported, preventing downstream code from distinguishing sources (e.g., HVAC
     heating + internal gains lumped into one number). Recommend where disaggregated
     values should be preserved.
   - **Type safety opportunities** where a `f64` could be a `uom` quantity, where a
     `String` could be an enum, where a `HashMap<String, f64>` could be a typed struct.

8. **Do not make code changes.** Findings-only. Code changes happen in follow-up
   implementation tickets created from findings.

## Parallelism

Tickets within a series are independent unless `depends_on` says otherwise.
An agent can work on any ticket without coordination, as these are read-only
investigations.

**Safe parallel sets (no file overlap within each set):**

| Parallel Set | Tickets |
|-------------|---------|
| Set 0a | AR-001, AR-002, AR-003, AR-004, AR-005 |
| Set 0b | AR-006, AR-007, AR-008, AR-010 |
| Set 0c | TS-001, TS-002, TS-003, TS-004, TS-005, TS-006, TS-007, TS-008, TS-009, TS-010, TS-011 |
| Set 0d | PS-001, PS-002, PS-003, PS-004, PS-005, PS-006 |
| Set A | DT-001, DT-002, DT-003, DT-004, DT-005 |
| Set B | DT-006, DT-007, DT-008, DT-009, DT-010, DT-011, DT-012 |
| Set C | WO-001, WO-002, WO-003, WO-004 |
| Set D | UC-001, UC-002, UC-003, UC-004, UC-005 |
| Set E | UC-006, UC-007, UC-008, UC-009, UC-010 |
| Set F | CW-001 through CW-010 (HVAC per-type) |
| Set G | CW-011 through CW-020 (WH, DER, loads per-type), CW-021, CW-022 |
| Set H | IC-001 through IC-006, IC-007 |
| Set I | TP-001 through TP-009 |
| Set J | DL-001 through DL-004, FP-001 through FP-003 |
| Set K | EG-001 through EG-007 |
| Set L | DC-001 through DC-007, DC-008 |
| Set M | PA-001 through PA-008 |
| Set N | EA-001 through EA-004, EA-005, EA-006, EA-007 |

## Priority Order

Work these series in priority order. AR and TS are front-loaded because they
inform whether structural fixes are needed before the per-equipment investigations.

1. **AR** (Architecture Review) — type safety and structural issues that enable bugs
2. **TS** (Thermal Solver Init) — initial conditions, derived temps, zone physics
3. **UC** (Unit Conversions) — wrong units corrupt everything downstream
4. **CW** (Config Wiring) — dropped config fields mean wrong equipment behavior
5. **IC** (Ideal Capacity) — wrong ideal mode = wrong energy at common timesteps
6. **DT** (Datetime) — wrong timezone = wrong schedules = wrong operation patterns
7. **WO** (Weather/Schedule Offsets) — wrong offsets = systematic bias
8. **DL/FP** (Duct Losses / Fan Power) — missing energy accounting
9. **EG** (Envelope Geometry) — wrong zones = wrong thermal mass
10. **DC** (Duty Cycles) — wrong DR response
11. **TP** (Telemetry) — wrong output, not wrong physics
12. **EA** (Equipment Audits) — deep behavioral comparison
13. **PS** (Python Safety) — replace brittle dict/kwargs with strong types
14. **PA** (Python API) — usability, not correctness
