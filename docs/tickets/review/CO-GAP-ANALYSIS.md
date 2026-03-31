# CO Ticket Series — Gap Analysis

**Date:** 2026-03-30
**Scope:** CO-001 through CO-011 vs. review findings in CROSS-CUTTING.md and FIX-STRATEGY.md

---

## Summary verdict

The CO series solves **one** of the four Phase 0 infrastructure problems (CC-002 /
P0-B: typed telemetry). It does not address CC-001 (P0-A: typed config), CC-005
(P0-C: silent equipment drop), or CC-003 (P0-D: init-time config validation). By
FIX-STRATEGY.md ordering, P0-A is the single highest-impact change and is a
prerequisite for meaningful parity work. The CO series skips it entirely.

The CO tickets also contain a logical staging conflict: the bugs they are fixing
(BUG-1, BUG-2, GAP-1, GAP-2 from AR-001) are already marked **FIXED** in
AR-001's Implementation Status section. The CO series is now proposing to replace
the already-applied string-key fixes with a deeper typed-output layer — which is
correct work, but the "fixes the stated bugs" framing in CO-003/CO-005 is stale.

---

## What the CO series covers well

### CC-002 / P0-B: Typed telemetry (AR-001)

CO-001 through CO-011 constitute a complete, well-sequenced implementation of a
typed `CoreOutput` layer on the `Equipment` trait. The sequencing is sound:

- CO-001 defines the types (`CoreOutput`, `ElectricPower`, `Soc`, `CoreCapabilities`)
- CO-002 adds the trait method with a temporary default stub so the tree stays green
- CO-003/CO-004/CO-006/CO-007 migrate each equipment category in parallel waves
- CO-005 migrates actors (blocked on CO-003's `equipment_core` map)
- CO-008 removes the duplicate telemetry writes that the old fallback chains required
- CO-009 adds the capability contract validator and cross-check tests
- CO-010 exposes `CoreOutput` in Python bindings
- CO-011 adds a CI grep guard

Each ticket is narrow, independently verifiable, and leaves the tree compiling at
each step. This is a well-structured incremental migration.

### AR-001 remaining items (partially)

CO-009 picks up the lifecycle tests for water heater types that AR-001 flagged as
missing. CO-011 removes dead telemetry key constants. Both are legitimate
completions of AR-001's "Remaining" list.

---

## What the CO series does NOT cover

### CRITICAL GAP: CC-001 / P0-A — Typed config structs (the #1 systemic issue)

**No CO ticket touches `EquipmentSpec.parameters` or the resolver → equipment
config key contract.**

CROSS-CUTTING.md ranks CC-001 as the single highest-priority architectural change.
FIX-STRATEGY.md Phase 0 opens with P0-A (typed config) before P0-B (typed
telemetry). CC-001 covers roughly 25 confirmed config key mismatches across the
CW-series tickets:

- `"FuelType"` (PascalCase, what gas.rs/tankless.rs read) vs `"fuel_type"`
  (snake_case, what build_spec writes) — AR-002 Finding 1
- `"battery_capacity_kwh"` (what resolve_loads.rs writes) vs `"capacity_kwh"`
  (what ev/config.rs reads) — AR-002 Finding 2
- Three ventilation key mismatches: flow rate name + unit wrong, sensible
  effectiveness name wrong, latent effectiveness name wrong — AR-002 Finding 3
- `heating_efficiency` vs `fuel_efficiency`/`afue`/`efficiency` for all
  furnaces/boilers — CW-001 through CW-004
- `efficiency_seer`/`cooling_efficiency` vs `seer`/`SEER` for all AC/HP
  cooling — CW-005 through CW-010
- HPWH: `heating_capacity_w` vs `backup_element_power_w` — CW-013
- Battery: double-sqrt from storing pre-processed value — CW-016

Every one of these is a silent zero or wrong-default that corrupts physics output.
The CO series, by migrating the output side of equipment to typed values, does
nothing to fix the input side. Equipment can emit perfectly typed CoreOutput values
that were computed from silently defaulted inputs.

FIX-STRATEGY.md explicitly states: "Fix wiring before physics — no point tuning a
formula if the input never reaches it."

### HIGH GAP: CC-005 / P0-C — Silent equipment drop on registration failure

**No CO ticket addresses the five confirmed registry name mismatches from AR-005.**

AR-005 identified five canonical names produced by HPXML resolvers that have no
corresponding registry entry:

- `"Electric Vehicle"` — both resolve_der.rs and resolve_loads.rs paths; all
  HPXML-parsed EVs silently absent from every simulation (AR-005 F-001, CRITICAL)
- `"Gas Tankless Water Heater"` — resolver produces this, registry has only
  `"Tankless Water Heater"` (AR-005 F-002, HIGH)
- `"Generic Heater"` / `"Generic Cooler"` — fallback names, not registered
  (AR-005 F-003, HIGH)
- `"Water Heating"` — fallback name, not registered (AR-005 F-004, MEDIUM)

FIX-STRATEGY.md P0-C states: "Make `registry.create()` failure return `Err`, not
`continue`. Dwelling construction fails with a list of unresolvable equipment."

The CO series makes no mention of registration validation, `dwelling.warnings`,
the `registry.create()` error path, or any of the AR-005 findings. These bugs
predate the CO series and remain active.

### HIGH GAP: CC-003 / P0-D — Init-time config validation

**No CO ticket adds tracking of which config keys are consumed during `init()`,
warning on unconsumed (dead) keys, or erroring on required-but-missing keys.**

FIX-STRATEGY.md P0-D describes this as the safety net that catches future
regressions after P0-A (typed config) is in place. It is cheap to implement —
a `consumed_keys: HashSet<String>` threaded through init and checked afterward.

Without P0-D, even after CC-001 is fixed, future resolvers or equipment changes
can silently re-introduce key mismatches with no test or compile-time signal.

AR-002 Finding 6 (override keys never validated) is the most user-visible
manifestation of this gap: a Python user passing `{"Gas Furnace": {"eir": 0.8}}`
when the correct key is `"heating_efficiency"` gets no error.

### MEDIUM GAP: CC-004 — Gain fraction defaults table

**No CO ticket centralizes sensible/latent gain fractions for scheduled/event loads.**

CROSS-CUTTING.md CC-004 and FIX-STRATEGY.md P2-F identify wrong gain fractions
for at least five load types (lighting=1.0 should be (0.5, 0), MELs missing latent,
TV/ceiling fan/freezer=0.0). This is a data error, not an architectural change, and
is addressed by CW-017, CW-020 in the review series — but no CO ticket picks it up.

### MEDIUM GAP: AR-001 GAP-3/4/5/6 — Unit suffix normalization and load naming

The CO series retires the fallback chain but does not normalize the underlying key
names. `core_output()` bypasses the need to search by key name in the hot path, so
the inconsistency becomes a documentation/API problem rather than a correctness
problem. However:

- `"compressor_kw"` (HVAC) vs `"compressor_power_w"` (HPWH) — GAP-3
- `"fan_kw"` (furnace, AC) vs `"fan_electric_w"` (gas WH) vs `"fan_power_w"`
  (ventilation) — GAP-4
- `"electric_kw"` (kW) violating SI-unit policy for grid-boundary power — GAP-5
- `"active_power_kw"` (EventLoad) vs `"electric_kw"` (ScheduledLoad) — GAP-6

These gaps are deferred by both AR-001 and the CO series as "normalization pass".
That is an acceptable decision but should be explicitly tracked.

---

## Review findings with no CO ticket coverage at all

The following review findings are confirmed bugs with no corresponding CO ticket
and no FIX ticket covering them (verified by searching the CO-* series):

| Finding | Severity | Description |
|---------|----------|-------------|
| AR-005 F-001 | CRITICAL | `"Electric Vehicle"` never registered; all HPXML EVs silently dropped |
| AR-005 F-002 | HIGH | `"Gas Tankless Water Heater"` never registered |
| AR-005 F-003 | HIGH | `"Generic Heater"` / `"Generic Cooler"` never registered |
| AR-002 F-1 | HIGH | `GasWH` / `TanklessWH` read `"FuelType"` (wrong case) |
| AR-002 F-2 | HIGH | EV PlugLoad path writes `"battery_capacity_kwh"`, reads `"capacity_kwh"` |
| AR-002 F-3 | HIGH | Ventilation: 3 key mismatches + wrong unit (CFM vs m³/s) |
| AR-005 F-008 | HIGH | Equipment `init()` failures are soft-silenced with `continue` |
| AR-005 F-009 | HIGH | Registry `create()` failure is warning-only, not an error |
| CC-003 / AR-002 F-6 | HIGH | Override keys silently ignored with no validation |
| AR-005 F-005 | MEDIUM | `EquipmentRegistry::register` silently overwrites existing entries |
| AR-005 F-006 | MEDIUM | No zone-existence check at equipment init time |
| AR-002 F-4 | MEDIUM | `FuelType` serialized via `{:?}` Debug format — fragile contract |
| CC-004 / CW-017 | MEDIUM | Wrong sensible/latent gain fractions for 5+ load types |
| AR-005 F-004 | MEDIUM | `"Water Heating"` fallback name never registered |

---

## Phase 0 alignment assessment

FIX-STRATEGY.md defines four Phase 0 steps:

| Step | Issue | CO coverage | Assessment |
|------|-------|-------------|------------|
| P0-A | CC-001: Typed config structs | **None** | Missing entirely |
| P0-B | CC-002: Typed telemetry | CO-001 through CO-011 | Complete |
| P0-C | CC-005: Registration failure → hard error | **None** | Missing entirely |
| P0-D | CC-003: Init-time config validation | **None** | Missing entirely |

The CO series implements P0-B only. By FIX-STRATEGY.md ordering, P0-A should
precede or run in parallel with P0-B, and P0-C should follow immediately after
(because typed config enables the build to fail on unknown keys, which makes silent
drops detectable).

Running P0-B in isolation is not wrong — `CoreOutput` is independently useful and
CO-009's capability validator will immediately surface equipment that produces wrong
outputs. But equipment that was never instantiated (due to AR-005 registration gaps)
will still be absent with no signal, because P0-C is not in scope.

---

## Recommended new tickets or scope changes

### New ticket: CFG-001 — Typed config structs per equipment (P0-A)

**Priority: CRITICAL — implement before or alongside CO series**

Scope:
- Define per-equipment `*Config` structs in `hares-equipment/src/*/config.rs`
  with `#[serde(deny_unknown_fields)]`
- Migrate all HPXML resolvers to write the typed struct directly
- Migrate all equipment `init()` to accept the typed struct
- Round-trip test per struct: `serialize → deserialize → fields match`
- Override path: typed struct deserialization from merged JSON map

This eliminates all 25+ CW-series key mismatches in a single compile pass.

### New ticket: CFG-002 — Registration failure → hard error (P0-C)

**Priority: HIGH — implement before any parity tests are run**

Scope:
- Fix all five registry name mismatches from AR-005 (EV, Gas Tankless WH,
  Generic Heater, Generic Cooler, Water Heating)
- Change `dwelling/mod.rs` `registry.create()` failure from `warn + continue`
  to `Err` for all equipment classes except explicitly-optional ones
- Change `eq.init()` failure from `warn + continue` to `Err` for
  HVAC/water heater/DER categories
- Add `all_canonical_resolver_names_are_registered` test (AR-005 T-001)
- Add negative test: unknown equipment name → construction error with message

### New ticket: CFG-003 — Init-time config validation (P0-D)

**Priority: HIGH — implement immediately after CFG-001**

Scope:
- Track which config keys are consumed during each equipment `init()`
- Warn on unconsumed keys (dead config)
- Error on required-but-missing keys (with explicit required/optional distinction
  per field)
- Integration test: inject config with extra key → warning emitted
- Override path: misspelled Python override key → warning emitted

### Scope change: CO-001 should reference CC-002 and FIX-STRATEGY P0-B

CO-001 through CO-011 only reference `AR-001.md` and the plan file. Each should
explicitly acknowledge they implement P0-B and that P0-A (CC-001) must also be
completed as part of Phase 0. Without that link, a developer executing CO tickets
in sequence will complete them believing Phase 0 is done — it will not be.

### Scope change: CO-009 should add the AR-005 T-001 registration test

CO-009 ("capability validation, lifecycle tests, cross-checks") is the right
ticket to also add the exhaustive registry coverage test from AR-005 T-001:
`all_canonical_resolver_names_are_registered`. The test is cheap, belongs in
the same lifecycle test file, and would surface F-001 through F-004 immediately.

---

## CO ticket ordering vs FIX-STRATEGY.md Phase 0

The CO series is internally consistent and correctly sequenced for P0-B. The
ordering problem is external: CO executes P0-B without P0-A having been defined
or scheduled. The consequence is:

1. After CO-011 completes, `CoreOutput` values will be typed and correct — **for
   equipment that was actually instantiated and received correct config.**
2. Equipment silently dropped due to AR-005 name mismatches (EV, Gas Tankless WH)
   will still be absent; the CO series adds no detection for this.
3. Equipment with wrong config inputs (furnace efficiency, ventilation flow,
   WH fuel type) will silently produce `CoreOutput` values computed from wrong
   parameters.

The right sequencing is: CFG-001 (P0-A) + CFG-002 (P0-C) in parallel with or
immediately before CO-001. P0-D (CFG-003) follows after CFG-001 is complete.
CO-001 through CO-011 can proceed in parallel with CFG-001 since they touch
different layers (output side vs input side), but CO-009 should absorb the
CFG-002 registration test.
