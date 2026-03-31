# HARES Fix Work Order

**Created:** 2026-03-30
**Branch:** initial-implementation
**Purpose:** Sequenced, independently-testable fix batches. Fix the broken input pipeline before any architectural migration.

---

## Reading this document

- **Complexity:** S = < 30 min, M = 1–2 h, L = half day, XL = full day+
- **Ref:** points to the review ticket that identified the issue
- Each batch is independently testable and mergeable
- Do not start Batch N until Batch N-1 passes its verification tests

---

## Batch 0 — Critical Input Fixes (unblock everything)

These are surgical fixes that do not require architectural changes. They corrupt
physics inputs before any equipment model runs, so no parity test is meaningful
until they are fixed. Implement and verify each item individually.

---

### B0-1 — DST test compile error
**Ref:** DT-011 Finding 1
**Complexity:** S
**File:** `crates/hares-core/src/environment.rs:1663`

`ScheduleTimeSeries` has no `midpoint_offset_secs` field. The field belongs to
`WeatherMeta`. This line is inside the `#[cfg(feature = "dst")]` test helper
`annual_hourly_schedule()`, causing all four DST tests to fail to compile.

**Change:** Remove line 1663:
```
midpoint_offset_secs: 0,
```

**Verify:**
```
cargo test -p hares-core --features dst -- dst_tests
```
All four tests must compile and pass.

---

### B0-2 — AssemblyEffectiveRValue silently dropped
**Ref:** CC-014 / UC-005 F-001
**Complexity:** S
**File:** `crates/hares-io/src/hpxml/building.rs:910`

Standard HPXML nests `AssemblyEffectiveRValue` under `<Insulation>`. The current
call is `node.child("AssemblyEffectiveRValue")` which only checks direct children.
For any real HPXML file where `<AssemblyEffectiveRValue>` is inside `<Insulation>`,
it is silently `None` and the envelope uses only the nominal-layer fallback.

**Change:** Replace both occurrences on lines 910–911:
```rust
// Before
parse_value_with_units(node.child("AssemblyEffectiveRValue"), ValueKind::RValue)
    .or_else(|| parse_value_with_units(node.child("RValue"), ValueKind::RValue));

// After
parse_value_with_units(node.first_descendant("AssemblyEffectiveRValue"), ValueKind::RValue)
    .or_else(|| parse_value_with_units(node.first_descendant("RValue"), ValueKind::RValue));
```

**Verify:**
```
cargo test -p hares-io -- hpxml_parity
```
Assert `assembly_r_value_m2_k_w` is `Some(...)` for all wall/roof/floor surfaces
in the parity corpus fixtures.

---

### B0-3 — Occupancy schedule not scaled by number of occupants
**Ref:** SU-008 F2
**Complexity:** M
**File:** `crates/hares-core/src/dwelling/mod.rs:1721`

`apply_occupancy_gains` reads the raw schedule fraction (0–1) as if it were a
person count. For a 3-person household the peak occupancy fraction is `1.0`,
yielding `n_occupants = 1.0` instead of `3.0`. Internal heat gains are
under-counted by `number_of_occupants` for every household with more than one
person.

**Change:** At dwelling init time, read `number_of_occupants` from the Occupancy
`EquipmentSpec` (stored at `crates/hares-io/src/hpxml/resolve_loads.rs:32` as
`"number_of_occupants"`). Add a `occupancy_scale: f64` field to `Dwelling`
(default `1.0`). In `apply_occupancy_gains`, multiply the schedule value by
`self.occupancy_scale` before using it as `n_occupants`.

**Verify:**
```
cargo test -p hares-core -- occupancy
```
New test: inject an `occupants` column with peak value `0.5` and
`number_of_occupants = 4`; assert `apply_occupancy_gains` uses
`n_occupants = 2.0` at that step.

---

### B0-4 — Water draw schedule fraction not scaled to L/min
**Ref:** CC-015 / SU-008 F3 / WO-002
**Complexity:** M
**File:** `crates/hares-io/src/schedule_resolve.rs:467`

`inject_water_heater_schedule_columns` stores the raw `hot_water_fixtures`
column index. At runtime `resolve_storage_step_inputs` interprets the column
value as L/min directly. A typical fraction of `0.04` produces
`0.04 / 60 ≈ 0.00067 kg/s` instead of the correct `~0.058 kg/s` — roughly
87× underscaled. Water heater tanks never deplete.

`avg_water_draw_l_per_day` is already computed and stored in the water heater
spec at `resolve_water_heater.rs:79–80`. `normalize_draw_profile()` already
exists in `draw_profile.rs` and produces a L/min series from fractions.

**Change:** In `inject_water_heater_schedule_columns`, after finding the draw
column:

1. Extract the column values from `schedule`.
2. Find the matching water heater spec and read `avg_water_draw_l_per_day` from
   its parameters.
3. Call `normalize_draw_profile(&raw_fractions, avg_daily_l)` to produce a
   L/min series.
4. Append the scaled column to `schedule` under a new name (e.g.
   `"hot_water_fixtures_l_min"`) and store that column index in the spec, not
   the raw fraction column index.

The function already has `&mut [EquipmentSpec]` and `&HashMap<String, usize>` but
needs `schedule: &mut ScheduleTimeSeries` added as a parameter (the call site at
`schedule_resolve.rs:434` already has a `&mut ScheduleTimeSeries`).

**Verify:**
```
cargo test -p hares-io -- schedule
cargo test -p hares-equipment -- water_heater
```
New test: inject a `hot_water_fixtures` column with known fractions and an
`avg_water_draw_l_per_day = 200` spec; assert the resolved `draw_rate_kg_s`
matches `fraction × (200/1440/mean_fraction) / 60` within 1e-6.

---

### B0-5 — Registry name mismatches: 4 equipment types silently dropped
**Ref:** AR-005 / CO-GAP-ANALYSIS CC-005 / CFG-008
**Complexity:** M
**Files:**
- `crates/hares-io/src/hpxml/resolve_water_heater.rs:340–341`
- `crates/hares-io/src/hpxml/resolve_hvac.rs:588, 596`
- `crates/hares-equipment/src/water_heater/tankless.rs:443`
- `crates/hares-equipment/src/scheduled_load.rs` (add fallback aliases)

The HPXML resolver emits these names that have no registry entry, so equipment
is silently absent from every simulation:

| Resolver output | Registry entry | Fix |
|---|---|---|
| `"Gas Tankless Water Heater"` | `"Tankless Water Heater"` only | Register alias |
| `"Generic Heater"` | not registered | Register as `IdealHvac` alias or error |
| `"Generic Cooler"` | not registered | Register as `IdealHvac` alias or error |
| `"Water Heating"` | not registered | Register as `"Gas Water Heater"` alias or error |

The EV case (`"Electric Vehicle"`) is already registered in
`crates/hares-equipment/src/ev/mod.rs:1117–1119`. Confirm it works end-to-end.

**Preferred approach:** Register the missing names as aliases pointing to the
correct factory. For `"Generic Heater"` / `"Generic Cooler"`, map to `IdealHvac`
(the only reasonable fallback). For `"Water Heating"`, error with a descriptive
message rather than silently using the wrong type — a gas-vs-electric distinction
is required.

Make `registry.create()` failure return `Err` (currently `continue`) so missing
equipment causes a visible construction error rather than a silent omission.
This change is in `crates/hares-core/src/dwelling/mod.rs` at the equipment
instantiation loop.

**Verify:**
```
cargo test -p hares-io -- hpxml_parity
cargo test -p hares-core -- engine
```
Parse all parity-corpus HPXML fixtures and assert that equipment counts match
expectations (no silently-dropped equipment). Add a negative test: pass an
unknown equipment name to the registry and assert `Err`, not silent success.

---

### B0-6 — Psychrometric wet-bulb bisection inverted bracket on supersaturated input
**Ref:** DV-001 F4
**Complexity:** S
**File:** `crates/hares-physics/src/psychrometrics.rs:126–141`

When `w > w_sat(t_db_c)` (corrupted EPW data or mis-coded caller), `dew_point(w, p)`
returns `t_dp > t_db_c`. The existing bracket `low = t_dp.min(t_db_c)` / `high =
t_db_c.max(t_dp)` evaluates the wet-bulb search *above* the dry-bulb — physically
impossible. The bisection converges but returns a `wet_bulb_c >= t_db_c`, which then
enters HVAC coil biquadratic lookups and can produce negative capacity or negative
COP by extrapolation.

**Change:** At the top of `wet_bulb_from_humidity_ratio`, clamp `w` to saturation
before computing the bracket:

```rust
let w_sat = humidity_ratio_from_tdp(t_db_c, p_pa);
if w_eff >= w_sat {
    return t_db_c;   // saturated: Twb = Tdb
}
let t_dp = dew_point(w_eff, p_pa);
bisect(t_dp, t_db_c, ...)
```

This also removes the `.min()/.max()` gymnastics — with the saturation guard the
bracket is always `[t_dp, t_db_c]`.

**Verify:**
```
cargo test -p hares-physics -- psychrometrics
```
New test: `wet_bulb_from_humidity_ratio(20.0, w_sat * 2.0, 101325.0)` must return
`20.0 ± 0.01`. Existing tests must continue to pass.

---

## Batch 0 summary verification

After all five Batch 0 items are applied:

```bash
cargo test -p hares-io
cargo test -p hares-equipment -- water_heater
cargo test -p hares-core --features dst -- dst_tests
cargo test -p hares-core -- engine
```

All must pass before proceeding to Batch 1.

---

## Batch 1 — CFG Series: Typed Equipment Config (P0-A + P0-C + P0-D)

**Ref:** CC-001, CC-003, CC-005, FIX-STRATEGY P0-A/P0-C/P0-D, CFG-INDEX.md
**Complexity:** XL (full series)
**Kills:** All ~25 CW-series key mismatches, AR-002 F1–F4, AR-005 F2–F4 and F8–F9

This is the highest-impact architectural change. Every config key mismatch is a
silent wrong-default that corrupts physics. Typed structs surface all mismatches
as compile errors.

### Execution order (matches CFG-INDEX.md dependency graph)

| Step | Ticket | What it does | Verify |
|------|--------|--------------|--------|
| 1 | CFG-001 | `ScheduleSource` enum (prerequisite for CFG-007) | `cargo test -p hares-types` |
| 2 | CFG-002..006 | Compact ScheduleSource migration | `cargo test -p hares-io -- schedule` |
| 3 | CFG-007 | `EquipmentTypedConfig` trait + `ConfigPayload` enum + migration shim | tree compiles |
| 4a | CFG-008 | Registry name fixes + hard error on registration failure | negative test |
| 4b | CFG-009 | Typed configs: furnace, boiler, baseboard, IdealHvac | round-trip tests |
| 4c | CFG-010 | Typed configs: AC, heat pumps, dehumidifier | round-trip tests |
| 5 | CFG-011 | HVAC resolver writes typed configs | `cargo test -p hares-io -- hpxml_parity` |
| 6a | CFG-012 | Typed configs: water heaters + resolver | `cargo test -p hares-equipment -- water_heater` |
| 6b | CFG-013 | Typed configs: DER, loads, Ventilation + resolver | `cargo test -p hares-equipment` |
| 7 | CFG-014 | Exhaustive registry coverage test + round-trip suite | all tests |
| 8 | CFG-015 | Remove Raw path + init-time key tracking | `cargo test -p hares-core` |
| 9 | CFG-016 | CI grep guard: no magic-string config access in equipment init | CI lint |

Steps 4a, 4b, 4c can run in parallel (independent equipment categories).
Steps 6a and 6b can run in parallel.

**Batch 1 final verification:**
```bash
cargo test -p hares-io
cargo test -p hares-equipment
cargo test -p hares-core
```
Zero `warn!` about unknown config keys. All parity-corpus fixtures build without
construction errors.

---

## Batch 2 — CO Series: Typed CoreOutput (P0-B)

**Ref:** CC-002, FIX-STRATEGY P0-B, CO-INDEX.md
**Complexity:** XL (full series)
**Kills:** AR-001 telemetry key bugs, silent zero outputs

Can run in parallel with Batch 1 since they touch different layers (output side
vs input side). CO-009 must absorb the AR-005 T-001 registration coverage test
(all resolver-canonical names are registered).

### Execution order (matches CO-INDEX.md dependency graph)

| Step | Ticket | What it does | Verify |
|------|--------|--------------|--------|
| 1 | CO-001 | `CoreOutput`, `ElectricPower`, `Soc`, `CoreCapabilities` types | `cargo test -p hares-types` |
| 2 | CO-002 | `core_output()` on `Equipment` trait with default stub | tree compiles |
| 3a | CO-003 | Migrate dwelling to read `CoreOutput` | `cargo test -p hares-core` |
| 3b | CO-004 | Implement `core_output()` for HVAC | `cargo test -p hares-equipment -- hvac` |
| 3c | CO-006 | Implement `core_output()` for DER + loads | `cargo test -p hares-equipment` |
| 3d | CO-007 | Implement `core_output()` for water heaters | `cargo test -p hares-equipment -- water_heater` |
| 4 | CO-005 | Migrate actors to read `CoreOutput` from `EnvironmentState` | `cargo test -p hares-core -- actors` |
| 5 | CO-008 | Remove duplicate `electric_kw` writes + default stub | tree compiles |
| 6 | CO-009 | Capability validation + lifecycle tests + AR-005 T-001 | all tests |
| 7 | CO-010 | Expose `CoreOutput` in Python bindings | `pytest tests/python/` |
| 8 | CO-011 | CI grep guard + cleanup | CI |

Steps 3a–3d can run in parallel (independent equipment categories). CO-005
depends on CO-003.

**Batch 2 final verification:**
```bash
cargo test -p hares-equipment
cargo test -p hares-core
uv run pytest tests/python/test_py_dwelling.py
```

---

## Batch 3 — Envelope & Thermal Fixes (Phase 1)

**Ref:** FIX-STRATEGY Phase 1 (P1-A through P1-E)
**Complexity:** L–XL
**Prerequisite:** Batches 1 and 2 complete (envelope models need correct config wiring)

These fixes are independent of each other and can parallelize within the batch.

| Item | Ref | Change | Complexity | Verify |
|------|-----|--------|-----------|--------|
| P1-A | TS-004, EG-003..006 | Foundation geometry: use actual wall height; subtract foundation area from conditioned floor | M | Unit tests with known geometries |
| P1-B | TS-011, EG-002 | Attic zone: fix radiant barrier surface (interior not exterior); add interior LWR; fix gable selection; remove 200 m³ fallback → error | M | Synthetic box with attic → known temperature response |
| P1-C | TS-007, TS-009 | Thermal solver: fix b_coeff (continuous B_c); fix exterior LWR iteration off-by-one; add iterative interior LWR surface-temp update | L | Envelope oracle tests tighten |
| P1-D | TS-008 | Solar distribution: redistribute beam to walls when floor_area=0; never drop energy | S | Invariant test: absorbed solar = transmitted solar within ε |
| P1-E | CC-009, EA-001 F1, DC-004 F2/F4 | Ventilation double-accounting: remove `PortContribution::Thermal` from `Ventilation::step()`; envelope solver is sole owner of ventilation heat exchange | L | Ventilation on/off → zone temp difference matches analytical; energy balance closes |

**Batch 3 final verification:**
```bash
cargo test -p hares-envelope
cargo test -p hares-core -- envelope_oracle
cargo test -p hares-core -- freefloat_oracle
```
Envelope oracle MAE should tighten relative to pre-fix baseline.

---

## Batch 4 — Equipment Physics Fixes (Phase 2)

**Ref:** FIX-STRATEGY Phase 2 (P2-A through P2-J)
**Complexity:** XL
**Prerequisite:** Batch 3 complete (envelope must be correct before HVAC tuning)

Items within this batch can parallelize by equipment category.

| Item | Ref | Kills | Complexity |
|------|-----|-------|-----------|
| P2-A | CC-008, CC-012, FP-001..003, IC-001..003 | Fan heat injection into zone thermal port; IdealHvac capacity clip, latent cooling, EndUse, unmet-load capacity column | L |
| P2-B | CC-011, TP-004, EA-006 F1/F3 | Battery: remove double ohmic loss; apply SOH to nominal capacity; fix BOL transient dead code | M |
| P2-C | CC-017, EA-007 | Defrost: use post-defrost capacity for extra_power_w; remove 0.75 secondary reduction factor | M |
| P2-D | UC-002, CW-005..008 | HVAC efficiency end-to-end wiring (activated by Batch 1 CFG work) | S (verify only) |
| P2-E | DL-002, DL-004, AR-003 | Duct loss: DSE=1 in conditioned zone; correct zone gain accounting | M |
| P2-F | CC-015, CW-012..014, IC-004 | Water heater: gas WH conversion_efficiency; HPWH backup element; tankless casing (activated by Batch 1); simultaneous element mode | M |
| P2-G | CC-016, EA-004 | Gas appliances: add combustion watts from `annual_gas_therms` to event power series | M |
| P2-H | WO-003, CW-008, CW-010 | Capacity curves: flow-fraction quadratic; per-stage curves; MaxCapacityFraction control | L |
| P2-I | CW-017, CW-020, DC-006 | Gain fraction defaults: centralize table; fix 5+ wrong values | S |
| P2-J | CW-009, CW-010 | Mini-split: force 4 speeds; propagate per-stage SHR | S |

**Batch 4 final verification:**
```bash
cargo test -p hares-equipment
cargo test -p hares-core -- conditioned_oracle
uv run pytest tests/python/test_ochre_parity.py
```
Per-equipment energy balance: electrical_in = thermal_out + losses within ε.
At-rated-conditions biquadratic = 1.0 within ε.

---

## Batch 5 — HPXML Parsing, Output, Python API (Phases 3–5)

**Ref:** FIX-STRATEGY Phase 3–5
**Complexity:** XL
**Prerequisite:** Batch 4 complete (no point formatting wrong values)

### P3 — HPXML parsing hardening

| Item | Ref | Change | Complexity |
|------|-----|--------|-----------|
| P3-A | UC-003 | Duct leakage path: verify the HPXML 4.x sibling lookup is exercised by parity corpus; add assertion test | S |
| P3-B | UC-007, CW-019, UC-009 | Exterior shading parsing; dehumidifier allowlist; PV ModuleType case-insensitive | M |
| P3-C | AR-004, UC-010 | Unit conversion: error on unrecognized unit strings instead of silent passthrough | M |

### P4 — Integration & parity testing

| Item | Ref | Change | Complexity |
|------|-----|--------|-----------|
| P4-A | — | Per-equipment oracle tests: 24 h with known inputs vs analytical reference | L |
| P4-B | — | Per-fixture parity tests vs OCHRE reference parquet | L |
| P4-C | — | BESTEST cases 600, 640, 900, 600FF, 900FF within ASHRAE 140 bands | M |

### P5 — Output, Python API, checkpoint telemetry

Execute in this priority order within P5:

1. **P5-B** (Python safety): `catch_unwind` in `step_core`/`simulate`/Rayon closures (PS-006 CRITICAL, process stability); then `overrides` wiring through `from_hpxml` (PS-001 F1 CRITICAL); then unknown-key detection (PS-001 F2); then typed Python exceptions (PS-006 HIGH).
2. **P5-D** (checkpoint telemetry): HPWH missing `ELECTRIC_KW`, `WALL_SENSIBLE_GAIN_W`, `UNMET_LOAD_W`; ResistanceWH/GasWH missing `OUTLET_TEMP_C`, `UNMET_LOAD_W`; battery SOC/OCV timing fix (EA-006 F4).
3. **P5-A** (output column population): AR-008.
4. **P5-C** (Python API gaps): PA-001 through PA-008 in priority order.
5. **P5-E** (remaining telemetry field coverage).

**Batch 5 final verification:**
```bash
cargo test
uv run pytest tests/python/
cargo test --test bestest
```
BESTEST 600 zone temp must fall within ASHRAE 140 acceptance bands.

---

## Anti-patterns

- Do not run BESTEST until Batch 4 is complete.
- Do not fix individual key mismatches one-by-one; CFG-007 through CFG-016 eliminates them at compile time.
- Do not add integration tests for equipment with broken config wiring; fix the wiring first.
- Do not chase parity percentages; fix the physics and parity follows.
- Do not implement P5-B `catch_unwind` last; it belongs at the start of P5-B to prevent process aborts during all subsequent Python testing.

---

## Dependency summary

```
Batch 0 (critical input fixes)
  └── Batch 1 (typed config) ──────────┐
  └── Batch 2 (typed CoreOutput) ──────┤ (can run in parallel)
                                        │
                                    Batch 3 (envelope physics)
                                        │
                                    Batch 4 (equipment physics)
                                        │
                                    Batch 5 (HPXML, output, Python API)
```
