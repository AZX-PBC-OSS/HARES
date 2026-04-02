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

### Deviation Gate (Required)

For every implementation detail that differs from OCHRE or prior HARES behavior:

- If the current behavior is **worse than OCHRE** or **physically weak**, **fix it**.
- If the current behavior is **more physical / better than OCHRE**, **keep it** and
  document the intentional deviation with explicit tests (and brief ticket notes)
  so the improvement is preserved.

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

After all six Batch 0 items are applied:

```bash
cargo test -p hares-physics -- psychrometrics
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
| P1-F | BESTEST-900 | RC layer auto-splitting: split dense layers exceeding Fourier criterion into sub-nodes; fixes Case 900 heating 12% over band and Case 900FF peak 25% over band | M | BESTEST case 900 and 900FF within ASHRAE 140 bands |

**Batch 3 final verification:**
```bash
cargo test -p hares-envelope
cargo test -p hares-core -- envelope_oracle
cargo test -p hares-core -- freefloat_oracle
```
Envelope oracle MAE should tighten relative to pre-fix baseline.

---

### P1-F — RC layer auto-splitting for dense/thick layers (BESTEST-900)

**Ref:** BESTEST investigation 2026-03-30
**Complexity:** M
**File:** `crates/hares-envelope/src/boundary_rc.rs:678–691` (`build_layered_boundary`)

`build_layered_boundary` assigns exactly one RC node per `LayerInput` regardless of
layer thickness or thermal diffusivity. A single node for the 100mm concrete layer in
BESTEST Case 900 (`k=0.510, ρ=1400, cp=1000 J/kg·K, α=3.64×10⁻⁷ m²/s`) produces a
discretization error that transmits 2.1× more thermal amplitude at the 24-hour
diurnal cycle than the analytical solution:

- Theoretical attenuation: `exp(-L√(π/(α·P))) = exp(-3.18) = 0.042` (96% damped)
- 1-node RC approximation: amplitude ratio ≈ `1 / (1 + (ωRC/2)²)^0.5 = 0.192` (81% damped)
- Net over-transmission: 4.6× (amplitude); phase shift error: 1.2 h premature

This is the **primary cause** of:
- Case 900 annual heating 12.4% over the ASHRAE 140 upper band (cold conducted in
  too readily in winter)
- Case 900FF peak zone temperature 25% over the ASHRAE 140 upper band (heat conducted
  in too readily and delivered prematurely in summer)

EnergyPlus uses `max(1, floor(thickness_m / 0.025))` nodes for each layer, giving
4 nodes for 100mm concrete. The correct criterion is Fourier number: for a node of
thickness `Δx`, the explicit stability criterion `α·Δt/Δx² < 0.5` sets the minimum
node count. For BESTEST 3600 s timestep, Fourier-stable `Δx_max ≈ √(2·α·Δt) ≈ 51mm`.
The 100mm concrete layer therefore requires at minimum 2 nodes; 4 is the EnergyPlus
convention.

**Change:** In `build_layered_boundary`, before the node-allocation loop, split each
`LayerInput` that exceeds the Fourier criterion into N sub-nodes where
`N = max(1, ceil(thickness_m / dx_max))`. The sub-nodes share the layer's thermal
properties; each sub-node gets `thickness_m / N` thickness and thus `cap / N`
capacitance. Resistance between sub-nodes is `(thickness_m/N) / (2k·A)` (half-node
each side), same as the existing inter-layer formula.

The split should happen only for solid materials (`density_kg_m3 > 100` and
`conductivity_w_m_k > 0.1`) to avoid splitting lightweight insulation where 1 node is
already an over-estimate of mass.

```rust
const FOURIER_DX_MAX_FACTOR: f64 = 0.5; // α·Δt/Δx² ≤ this
// dt_s comes from BuilderParams; add dt_s: f64 to BoundaryParams

fn split_layer_count(layer: &LayerInput, dt_s: f64) -> usize {
    if layer.density_kg_m3 <= 100.0 || layer.conductivity_w_m_k <= 0.1 {
        return 1;
    }
    let alpha = layer.conductivity_w_m_k / (layer.density_kg_m3 * layer.specific_heat_j_kg_k);
    let dx_max = (2.0 * FOURIER_DX_MAX_FACTOR * alpha * dt_s).sqrt();
    let n = (layer.thickness_m / dx_max).ceil() as usize;
    n.max(1)
}
```

The `BoundaryParams` struct must gain a `dt_s: f64` field; callers in
`build_all_boundaries` must pass the simulation timestep. OCHRE pre-computed RC layers
(`build_ochre_rc_boundary`) are unaffected because OCHRE already handles splitting
during LUT generation.

**Verify:**
```bash
cargo test -p hares-envelope -- boundary_rc
cargo test --test bestest -- bestest_case_900
cargo test --test bestest -- bestest_case_900ff
```
New unit test: a 100mm concrete layer at 3600 s timestep must produce `N ≥ 2`
sub-nodes. BESTEST Case 900 annual heating must fall within [1170, 2041] kWh.
BESTEST Case 900FF peak zone temperature must fall within [41.6, 44.8] °C.

---

## Batch 4 — Equipment Physics Fixes (Phase 2)

**Ref:** FIX-STRATEGY Phase 2 (P2-A through P2-J) + SU/DV review series
**Complexity:** XL
**Prerequisite:** Batch 3 complete (envelope must be correct before HVAC tuning)

Items within this batch can parallelize by equipment category.

### Original P2 items

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

### P2-K — HVAC speed selection and interpolation (SU-002)

**Ref:** SU-002 F-001 through F-005
**Complexity:** L
**Files:** `crates/hares-equipment/src/hvac/staging.rs`, `crates/hares-equipment/src/hvac/air_conditioner.rs`

Four independent bugs compound to make multi-speed and variable-speed HVAC behaviour
incorrect at every operating point.

| Sub-item | File:Line | Defect | Fix |
|----------|-----------|--------|-----|
| K-1 (CRITICAL) | `staging.rs:339` | `capacity_fractions` picks the longer Vec regardless of active mode; heat pump with unequal heating/cooling stages picks wrong normalization | Pass `ThermostatMode` (or the correct capacities slice) into `select_multi_speed`; make `capacity_fractions` a free function taking `&[f64]` |
| K-2 (HIGH) | `air_conditioner.rs:811–816` | Multi-speed biquadratic correction evaluates stage 0 for all intermediate speeds | Evaluate biquadratic at both `speed_index` and `speed_index+1`, then linear-blend correction ratios by `speed_frac` |
| K-3 (HIGH) | `staging.rs:91–101` | `TwoSpeedAlternating` hardcodes `speed_frac=1.0` and uses raw `load_fraction` as PLR without normalising to stage capacity | Mirror `TwoSpeedSetpoint`'s per-stage PLR normalisation |
| K-4 (HIGH) | `air_conditioner.rs:559–568` | `VariableSpeedIdeal` maps any non-zero `speed_frac` to `duty_cycle=1.0` — partial delivery at fine timesteps is broken | Set `duty_cycle = selection.speed_frac` |

**Verify:**
```
cargo test -p hares-equipment -- hvac
```
New tests: `TwoSpeedAlternating` sub-unity load → PLR is stage-normalised;
`VariableSpeedIdeal` at `load_fraction=0.5` → `duty_cycle=0.5`; heat pump with
unequal-stage count → correct vector selected per mode.

---

### P2-L — Fan shaft heat DB correction before SHR calculation (SU-001 F-1)

**Ref:** SU-001 F-1
**Complexity:** M
**File:** `crates/hares-equipment/src/hvac/air_conditioner.rs:781–877`

Fan shaft heat ΔT is added to `zone.wet_bulb_c` directly for the biquadratic, but
`calculate_shr` receives the uncorrected `zone.temperature_c`. Two compounding errors:

1. Dry-bulb passed to `calculate_shr` is too low — ADP and SHR are biased.
2. Adding ΔT to wet-bulb without psychrometric recalculation is physically wrong;
   the correct procedure is `DB_corrected = DB + ΔT`, then re-derive WB from
   `(DB_corrected, W_in, P)`.

**Change:**
```rust
let coil_entering_db_c = zone.temperature_c + fan_shaft_heat_correction_c;
let coil_entering_wb_c = if fan_shaft_heat_correction_c.abs() > f64::EPSILON {
    wet_bulb_from_humidity_ratio(coil_entering_db_c, zone.humidity_ratio,
                                 env.weather.pressure_kpa * 1000.0)
} else {
    zone.wet_bulb_c
};
// pass coil_entering_db_c to calculate_shr (not zone.temperature_c)
```

**Verify:**
```
cargo test -p hares-equipment -- hvac_parity
```
New test: at AHRI rated conditions with a non-zero fan shaft correction, assert that
SHR changes relative to zero-correction baseline; assert `coil_entering_wb_c` is
derived from the corrected DB, not `zone.wet_bulb_c + ΔT`.

---

### P2-M — Natural ventilation uses wrong ELA coefficients and wrong zone scope (SU-004)

**Ref:** SU-004 F1, F5
**Complexity:** M
**Files:**
- `crates/hares-core/src/dwelling/solver_builder.rs:959` (F1 — wrong coefficients)
- `crates/hares-envelope/src/thermal_solver/infiltration.rs:130` (F5 — wrong zone scope)

**F1 — Wrong ELA coefficients (HIGH):**
`attic_ela_coefficients(1.5, bldg_h)` uses `hor_lk_frac=0.75` (ceiling-dominated,
Walker-Wilson 1998 Table 2). Natural ventilation through windows uses the conditioned
zone's own infiltration ELA coefficients. Error magnitude can be ~2× depending on
geometry.

Fix: look up the conditioned zone's ELA `stack_coeff` and `wind_coeff` from the
already-computed infiltration config and use them for `NaturalVentilationConfig`.
Do not call `attic_ela_coefficients` for this purpose.

**F5 — Applied to all zones, not indoor-only (MEDIUM):**
`apply_infiltration_and_ventilation()` iterates all zones. OCHRE (`Envelope.py:603`)
explicitly restricts nat-vent to the Indoor zone. For multi-zone buildings, nat-vent
is applied to attic zones using conditioned-zone parameters — physically incorrect.

Fix: gate the nat-vent call on `zone.id == config.indoor_zone_id`.

**Verify:**
```
cargo test -p hares-envelope
cargo test -p hares-core -- freefloat_oracle
```
New test: two-zone solver with conditioned + attic — assert nat-vent flow is zero for
the attic zone.

---

### P2-N — Tariff billing: tiered block rates never applied; net-metering minimum charge applied to wrong base (SU-005)

**Ref:** SU-005 H1, H2
**Complexity:** M
**File:** `crates/hares-tariff/src/billing.rs`, `crates/hares-tariff/src/evaluator.rs`

**H1 — Tiered block rates never applied (HIGH):**
`billing.rs:175` accumulates energy cost using only the flat TOU `import_price`.
`tier_multiplier` at `evaluator.rs:268` is a point-in-time rate lookup that is never
called from the billing path. The URDB parser correctly builds `TieredBlock` entries
which are then silently ignored. US residential tariffs with inclining blocks (PG&E
E-1, E-TOU-C baseline, etc.) will always compute too-low energy bills.

Fix: at billing period close in `BillingState::step()`, recompute the tiered energy
charge by walking the `TieredBlock` with the final `cumulative_import_kwh` for the
period. Rename `tier_multiplier` to `tier_rate_at` to clarify its limited role.

**H2 — Minimum charge applied after export credit (HIGH):**
`billing.rs:301–305` applies the floor to `(energy + demand + fixed) - export_credit`.
Regulatory majority (CA CPUC NEM 3.0 and most state tariffs) apply the minimum to
metered charges before netting export:

```rust
let metered = energy_charge_usd + demand_charge_usd + fixed_charge_usd;
let floored = minimum_charge.map_or(metered, |min| metered.max(min));
let net_bill_usd = floored - export_credit_usd;
```

Add a `minimum_charge_excludes_export: bool` field to `ElectricTariff` (default
`true`) to control the convention explicitly.

**Verify:**
```
cargo test -p hares-tariff
```
New tests: (a) full billing period crossing a tier boundary — assert blended cost
matches hand-computed reference; (b) net-exporting customer with minimum charge —
assert correct bill under both `minimum_charge_excludes_export` conventions.

---

### P2-O — Water heater TMV dead code and pre-step outlet temperature snapshot (SU-009)

**Ref:** SU-009 F1, F2
**Complexity:** M
**Files:**
- `crates/hares-equipment/src/water_heater/resistance.rs:506`
- `crates/hares-equipment/src/water_heater/gas.rs:455`
- `crates/hares-equipment/src/water_heater/tank.rs:320, 330, 361, 400`

**F1 — `step_tempered` never called (MEDIUM):**
`ResistanceWH`, `GasWH`, and `HeatPumpWH` all call `tank.step()` only;
`step_tempered` is never invoked. The TMV volume-reduction ratio
(`(tank_temp - mains_temp) / (fixture_setpoint - mains_temp)`) is never applied,
overestimating volumetric draw by ~1.46× for a 52°C tank / 40.6°C fixture setpoint /
15°C mains. `unmet_load_w` is always zero for ResistanceWH and GasWH.

Fix: each storage WH `step()` must call `step_tempered()` with a `TemperedDrawConfig`
when the draw rate is non-zero. `HeatPumpWH` similarly needs to use `step_tempered()`
once F1 is resolved (see `heat_pump_wh.rs:809–811`).

**F2 — Outlet temperature uses pre-step top-node snapshot (MEDIUM):**
`DrawResult.outlet_temp_c` is overwritten at `tank.rs:330` and `tank.rs:400` with the
pre-step top-node temperature. OCHRE computes a volume-weighted average across the
drawn segment (`Water.py:54–61`). For draws exceeding the top-node volume (~1.6 L per
node in a 12-node 19 L tank), HARES reports too-warm outlet and misreports
unmet-load.

Fix: in `apply_draw`, after the volume-displacement shift, compute the
volume-weighted average of the drawn nodes and return it as `outlet_temp_c`.

**Verify:**
```
cargo test -p hares-equipment -- water_heater
```
New tests: (a) TMV volume reduction — 55°C tank / 15°C mains / 40.6°C fixture target
→ draw volume reduced by 0.64×; (b) outlet blending — 12-node tank with 55°C upper /
20°C lower, draw > top-node volume → `outlet_temp_c` is blended, not pre-draw top;
(c) ResistanceWH uses `step_tempered` — assert `unmet_load_w > 0` when draw temp
below fixture setpoint.

---

### P2-P — Checkpoint: missing state fields cause post-restore physics drift (SU-003)

**Ref:** SU-003 F1, F2
**Complexity:** M
**Files:**
- `crates/hares-equipment/src/hvac/air_conditioner.rs:1048` and state struct at `:107`
- `crates/hares-equipment/src/hvac/heat_pump/heater.rs:1098` and state struct at `:136`
- `crates/hares-equipment/src/hvac/ideal_hvac.rs:496` and state struct
- `crates/hares-equipment/src/hvac/hvac_core.rs:197`

**F1 — `thermostat.hysteresis_c` not checkpointed (DEFECT):**
`apply_control_unchecked` for `ThermalSetpoint` mutates `hvac.thermostat.hysteresis_c`
(AC, HP heater) and `thermostat.hysteresis_c` (IdealHvac). None of the corresponding
state structs save this field. After restore the thermostat reverts to init-configured
deadband, permanently altering mode-switching thresholds for all simulations that use
DR setpoint control.

Fix: add `thermostat_hysteresis_c: f64` to `AirConditionerState`, `HeaterState`, and
`IdealHvacState`; save and restore the field.

**F2 — `time_at_current_speed_s` not checkpointed (DEFECT):**
`HvacEquipment.time_at_current_speed_s` accumulates each step and gates speed changes
against `min_time_per_speed_s` (default 300 s). After restore the field resets to 0,
immediately allowing a speed-stage change even if the unit had been locked for 299 s.

Fix: add `time_at_current_speed_s: f64` to `AirConditionerState` and `HeaterState`;
save and restore.

**Verify:**
```
cargo test -p hares-equipment -- checkpoint
```
New deterministic tests for each DEFECT: apply a control signal that mutates the
missing field; `save_state` + `load_state`; assert restored value matches saved;
run one step and assert mode/stage behavior matches pre-checkpoint baseline.

---

### P2-Q — Biquadratic input bounds and PLF clamps not wired from CSV to runtime (DV-003)

**Ref:** DV-003 F3, F4
**Complexity:** M
**Files:**
- `crates/hares-io/src/hpxml/resolve_hvac.rs:922–928`
- `crates/hares-io/src/defaults.rs:54–61, 498+`
- `crates/hares-equipment/src/hvac/hvac_core.rs:390–400, 755–763`

**F3 — Input bounds dropped in resolver (HIGH):**
`select_primary_curve_pair` at `resolve_hvac.rs:927–928` returns only coefficients;
`cap_t.x1_bounds` / `.x2_bounds` / `eir_t.x1_bounds` / `.x2_bounds` are discarded.
The equipment falls back to `DEFAULT_BIQUADRATIC_X1_BOUNDS=(-100, 100)` /
`DEFAULT_BIQUADRATIC_X2_BOUNDS=(-100, 100)`. OCHRE cooling equipment uses
`min_Twb=13.88, max_Twb=23.88, min_Tdb=18.33, max_Tdb=51.66`. At extreme conditions
(very dry air, heat waves) HARES extrapolates where OCHRE clamps, potentially
producing negative capacity or negative COP.

Fix: return bounds alongside coefficients from `select_primary_curve_pair`; write
`biquadratic_x1_min`, `biquadratic_x1_max`, `biquadratic_x2_min`, `biquadratic_x2_max`
into the equipment config at the call site.

Also fix the MSHP missing-row default: `load_hvac_csv_file`'s `get_row()` must default
missing `min_Twb`/`min_Tdb` rows to `-100.0` and `max_Twb`/`max_Tdb` rows to `100.0`
(not `0.0`).

**F4 — `min_ff`/`max_ff`/`min_plf`/`max_plf` never parsed (MEDIUM):**
`HvacCurveVariant` has no fields for these. The hardcoded PLF floor of 0.7
(`hvac_core.rs:1625`) over-clamps MSHP at low part-load (OCHRE `min_plf=0.2195`
for MSHP Single_1). Flow-fraction clamping is absent entirely.

Fix: add `ff_bounds: (f64, f64)` and `plf_bounds: (f64, f64)` to `HvacCurveVariant`;
parse from CSV; propagate through resolver; apply in `evaluate_biquadratic_with_flow`.

**Verify:**
```
cargo test -p hares-io -- hpxml_parity
cargo test -p hares-equipment -- hvac
```
New tests: (a) load AC HPXML fixture → assert `hvac.biquadratic_x1_bounds == (13.88,
23.88)` after resolver; (b) MSHP PLF floor matches CSV `min_plf`, not 0.7; (c)
`evaluate_biquadratic_with_flow` with `ff < min_ff` clamps to `min_ff`.

---

### P2-R — Battery pack topology hardcoded 350 V target; default resistance 49× too high (DV-004)

**Ref:** DV-004 F-002, F-003
**Complexity:** M
**File:** `crates/hares-equipment/src/battery/mod.rs:87–91, 664–676`

**F-002 — Pack topology hardcodes 350 V target (HIGH):**
`n_series = round(350.0 / v_cell)` at `mod.rs:664`. OCHRE derives `n_series` from
an `initial_voltage` config key (default 50.4 V). Passing OCHRE-style `ah_cell=70,
v_cell=3.6` to HARES produces `n_series=97` vs OCHRE's `n_series=14` — completely
different pack resistance and ohmic losses. Any adapter that passes OCHRE-compatible
`ah_cell`/`v_cell` parameters gets silently wrong round-trip efficiency.

Fix: replace the hardcoded `350.0` with an `initial_voltage` config key (default
350.0 for modern packs); expose it in `BatterySpec::to_config()` and in Python
`py_equipment.rs`.

**F-003 — Default pack resistance 49× too high (HIGH):**
`DEFAULT_N_SERIES=96, DEFAULT_N_PARALLEL=1, DEFAULT_CELL_RESISTANCE_OHM=0.005 Ω` →
pack resistance = 0.480 Ω. OCHRE equivalent (14S, 2.83P, 0.002 Ω/cell) = 0.0099 Ω.
At 5 kW / 352 V, HARES ohmic loss = 96.8 W vs OCHRE ~2 W. The efficiency model
overstates losses for every catalog product that does not explicitly set topology.
Catalog products in `catalog.rs` do not write `n_series`/`n_parallel`/
`cell_resistance_ohm` and all inherit these defaults.

Fix: for each catalog product in `catalog.rs`, derive and write explicit `n_series`,
`n_parallel`, and `cell_resistance_ohm` values consistent with the product's rated
voltage and documented RTE. Add a validation test: at rated power, ohmic efficiency
must be within ±2% of `spec.round_trip_efficiency`.

**Verify:**
```
cargo test -p hares-equipment -- battery
```
New tests: (a) `ah_cell=70, v_cell=3.6, initial_voltage=50.4` → `n_series=14 ± 1`;
(b) each catalog product → computed RTE at rated power within ±2% of spec value.

---

### P2-S — Envelope LUT minimal clamping doesn't clear filters (DV-005 F3)

**Ref:** DV-005 F3
**Complexity:** S
**File:** `crates/hares-io/src/envelope_lut.rs:192–194`

When `r_val >= 17.6` SI (100 IP), HARES clamps to 88.0 but retains any
construction/finish/insulation filters. OCHRE clears all filters before the Minimal
lookup. For a wall with `construction_type=Some("WoodStud")` and high R-value, the
`Minimal` row (empty construction type) is excluded after WoodStud filtering, causing
a wrong assembly selection.

**Change:**
```rust
if r_val >= 17.6 {
    filtered = candidates.iter().collect();  // clear all type filters
    r_val = 88.0;
}
```

**Verify:**
```
cargo test -p hares-io -- envelope_lut
```
New test: `lut.lookup("Exterior Wall", Some("WoodStud"), None, None, Some(20.0))`
must match the `Minimal` variant, not a WoodStud variant.

---

### P2-T — Synthetic fixture zone mass multiplier: bare-box BESTEST uses residential furniture mass (BESTEST-600)

**Ref:** BESTEST investigation 2026-03-30
**Complexity:** S
**Files:**
- `crates/hares-core/src/dwelling/conversions.rs:24–31` (`mass_multiplier_for_zone`)
- `crates/hares-core/src/dwelling/synthetic.rs` (`SyntheticTomlConfig`)

`mass_multiplier_for_zone` returns 7.0 for `ZoneType::Conditioned`. This multiplier
is applied as a scale factor on the zone air capacitance to represent furniture and
interior partition mass in a furnished residential dwelling. BESTEST Cases 600 and 900
are bare boxes with no furniture; ASHRAE 140 Section 5.2.1 explicitly specifies "no
interior mass beyond the building envelope surfaces."

Applying 7.0× to the BESTEST zone air capacitance adds:
- Zone volume: 129.6 m³; air density ≈ 1.2 kg/m³; cp ≈ 1006 J/kg·K
- Air capacitance: 129.6 × 1.2 × 1006 = 156,455 J/K
- Multiplier adds: 6 × 156,455 = 938,730 J/K of phantom thermal mass

This phantom mass buffers diurnal temperature swings, reducing both heating and cooling
loads relative to the true bare-box response. It is the **primary cause** of:
- Case 600 annual heating 2.1% under the ASHRAE 140 lower band
- Case 600 annual cooling 10.5% under the ASHRAE 140 lower band
- Case 600FF peak zone temperature 1.9°C over band (phantom mass delays but doesn't
  eliminate the over-temperature)

**Change:** Add a `mass_multiplier: Option<f64>` field to `SyntheticTomlConfig` and
`SyntheticGeometryConfig`. When set, it is written into `ZoneInput::mass_multiplier`
instead of calling `mass_multiplier_for_zone`. In `build_synthetic_building`, pass
the override through to the `Zone` struct so that `building_to_zone_inputs` picks it
up via the existing `zone.mass_multiplier` path (if that field does not exist on `Zone`,
add it).

Update all BESTEST fixture TOMLs to set `mass_multiplier = 1.0`:

```toml
[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 129.6
mass_multiplier = 1.0
```

No production (HPXML) code path is changed; the override is synthetic-fixture-only.

**Verify:**
```bash
cargo test --test bestest -- bestest_case_600
cargo test --test bestest -- bestest_case_600ff
cargo test --test bestest -- bestest_case_900
```
BESTEST Case 600 annual heating must fall within [4296, 5709] kWh.
BESTEST Case 600 annual cooling must fall within [6137, 7964] kWh.

---

### P2-U — Synthetic fixture internal gains: MELs schedule and sensible fraction not BESTEST-compliant (BESTEST-600)

**Ref:** BESTEST investigation 2026-03-30
**Complexity:** S
**Files:**
- `crates/hares-core/src/dwelling/synthetic.rs:369–411` (`build_synthetic_building`)
- `crates/hares-io/src/schedule_resolve.rs:702–726` (`inject_power_schedule`)
- `crates/hares-io/src/hpxml/resolve_loads.rs` (`default_gain_fractions`)

`build_synthetic_building` converts `internal_gains_w` to a `PlugLoadType=other`
MELs item. Two independent errors cause the delivered zone sensible heat to differ
from the ASHRAE 140 spec of constant 200 W sensible:

**Issue U-1 — MELs sensible_gain_fraction = 0.855 (HIGH):**
`default_gain_fractions` returns 0.855 sensible / 0.045 latent for `"MELs"`. ASHRAE
140 Section 5.4.3 specifies 200 W as 100% sensible internal gains. With
`sensible_gain_fraction = 0.855`, only 171 W reaches the zone as sensible heat,
leaving 29 W lost to a latent path. Over a year this is 254 kWh less sensible heating
of the zone, explaining ~39% of the 647 kWh cooling shortfall in Case 600.

**Issue U-2 — ANSI/RESNET 301 time-varying schedule instead of constant (MEDIUM):**
When no CSV column exists for `"plug_loads_other"` (true for all BESTEST synthetic
fixtures), `inject_power_schedule` falls through to `profiles.get("MELs")` which loads
the ANSI/RESNET 301 Table C.3(1) weekday/weekend fractions from
`Default Schedule Parameters.csv`. This gives a diurnal profile with mean ≈ 0.0417
scaled to `max_kw ≈ 0.2/0.0417 ≈ 4.8 kW`, producing instantaneous power ranging from
173 W to 245 W. Annual total is preserved but the time-of-use pattern diverges from
the ASHRAE 140 constant-200W spec.

**Change:** In `build_synthetic_building`, when emitting the `PlugLoad` XML node for
`internal_gains_w`, additionally write:
1. A `SensibleFraction` child node with value `"1.0"` inside `PlugLoad`, so that
   `resolve_loads.rs` writes `sensible_gain_fraction = 1.0` into the equipment spec.
2. A `WeekdayScheduleFractions` child node with 24 values all `"1.0"` and a
   `WeekendScheduleFractions` child node with 24 values all `"1.0"`, which causes
   `inject_power_schedule` to prefer the HPXML-parsed schedule (constant fraction) over
   the MELs default profile.

If the HPXML schema does not provide a flat path for `SensibleFraction` on `PlugLoad`,
an alternative is to add a `internal_gains_sensible_fraction: Option<f64>` field to
`SyntheticTomlConfig` and inject it as a raw equipment parameter override, bypassing
`default_gain_fractions`.

The minimal surgical fix — with lowest risk of touching production code paths — is the
second approach: add `internal_gains_sensible_fraction: Option<f64>` defaulting to
`None` (production behavior unchanged), and when `Some(f)`, write
`"sensible_gain_fraction"` directly into the `PlugLoad` config map before it is
resolved. Set all BESTEST fixture TOMLs to `internal_gains_sensible_fraction = 1.0`.

For the schedule: add `internal_gains_constant: bool` field (default `false`) to
`SyntheticTomlConfig`. When `true`, write `power_constant_kw = internal_gains_w / 1000`
directly into the equipment spec rather than relying on `inject_power_schedule`'s
profile lookup. This short-circuits the MELs schedule profile entirely.

```toml
[geometry]  # or top-level
internal_gains_w = 200.0
internal_gains_sensible_fraction = 1.0
internal_gains_constant = true
```

**Verify:**
```bash
cargo test --test bestest -- bestest_case_600
cargo test --test bestest -- bestest_case_640
```
BESTEST Case 600 annual cooling must fall within [6137, 7964] kWh.
After P2-T is also applied: heating must fall within [4296, 5709] kWh.

---

**Batch 4 final verification:**
```bash
cargo test -p hares-equipment
cargo test -p hares-tariff
cargo test -p hares-core -- conditioned_oracle
uv run pytest tests/python/test_ochre_parity.py
```
Per-equipment energy balance: electrical_in = thermal_out + losses within ε.
At-rated-conditions biquadratic = 1.0 within ε.

---

## Batch 4.5 — Hot-Path Allocation Fixes

**Ref:** SU-007
**Complexity:** M (total; items parallelize)
**Prerequisite:** Batch 4 complete (do not introduce new allocations while fixing physics)

Five genuine violations of the `feedback_hot_loop_minimal.md` policy. Each can be
implemented independently and verified in isolation.

---

### HP-1 — `check_invariants` three `Vec::new()` + N `format!` per step
**Ref:** SU-007 Hotspot 1
**Severity:** HIGH (fires every step in debug builds and all CI runs)
**File:** `crates/hares-core/src/dwelling/mod.rs:2378–2468`

Three fresh Vecs (`conditioned_temps`, `unconditioned_temps`, `tank_temps_c`,
`infiltration_latent_by_zone`) allocated from scratch every timestep under
`cfg(any(debug_assertions, feature = "check_invariants"))`. Additionally,
`format!("tank_node_{i}_c")` fires N times per step (N ≈ 12 for HPWH), allocating a
new `String` per call solely for a HashMap lookup.

**Fix:** Move the four Vecs to pre-allocated scratch fields on `Dwelling` (cleared and
refilled each step). Replace the `format!` key with a pre-built `Vec<String>` of node
keys sized at init from the water heater descriptor's node count.

---

### HP-2 — `StepResult::zone_temperatures_c` double-clone every step
**Ref:** SU-007 Hotspot 2
**Severity:** HIGH (two `Vec<(ZoneId, f64)>` clones per step)
**File:** `crates/hares-core/src/dwelling/mod.rs:2131, 2152`

`zone_temp_scratch` is cloned once to build `StepResult` (line 2131), then the entire
`StepResult` is cloned again to push into `simulation_results.steps` (line 2152). For
an 8760-step annual simulation with 3 zones this is ~17,520 small Vec allocations for
temperatures alone.

**Fix:** Either eliminate `simulation_results.steps` entirely (fleet runner already
accumulates externally) or construct `StepResult` once and move it into
`simulation_results.steps`, returning only a reference or index to the caller. The
secondary clone at line 2152 also doubles allocation cost for every other `StepResult`
field; eliminating it is the highest-leverage single change.

---

### HP-3 — `equipment_telemetry` fill: `HashMap::clear()` makes fast path unreachable
**Ref:** SU-007 Hotspot 3
**Severity:** MEDIUM (N `String` key + N `Telemetry` clone per step when actors active)
**File:** `crates/hares-core/src/environment.rs:561`, `crates/hares-core/src/dwelling/mod.rs:1830–1841`

`EnvironmentManager::update_in_place` calls `state.equipment_telemetry.clear()` before
the fill loop. `HashMap::clear()` drops all owned `String` keys. On the next step every
entry is re-inserted via `desc.name.clone()`, making the `clone_from` fast path at
`mod.rs:1826` unreachable every step.

**Fix:** Remove the `state.equipment_telemetry.clear()` call. The equipment set is
fixed; stale keys cannot accumulate. `clone_from` then handles all updates in-place
with no allocations. If a clean-slate guarantee is required, clear only on
`add_equipment` / `remove_equipment`.

---

### HP-4 — Solver `DomainUpdate` returns: four `Vec` allocations per step
**Ref:** SU-007 Hotspot 4
**Severity:** MEDIUM (four fresh `Vec`-bearing `DomainUpdate` structs per step)
**File:** `crates/hares-core/src/dwelling/mod.rs:2032–2084`, `crates/hares-types/src/domain_solver.rs:15`

Each `resolve()` call constructs a new `DomainUpdate` with a freshly allocated
`zone_temperatures_c` Vec. `upsert_domain` drops the old Vec and takes ownership of
the new one; no capacity recovery occurs (unlike the swap-based pattern already used
for schedule/mains payloads at `environment.rs:507–528`).

**Fix:** Extend the swap pattern to solver domains: have `upsert_domain` return the
replaced `DomainUpdate` so its inner Vecs can be handed back to solvers as scratch
buffers. Alternatively, change `DomainSolver::resolve` signature to accept
`&mut DomainUpdate` to fill in-place.

---

### HP-5 — `per_actor_timing` `to_string()` per actor per step under `actor_profiling`
**Ref:** SU-007 Hotspot 5a
**Severity:** LOW (profiling feature only)
**File:** `crates/hares-core/src/dwelling/mod.rs:1859–1860`

`actor.name().to_string()` allocates a `String` per actor per step when the
`actor_profiling` feature is enabled. For a 3-actor dwelling at 5-minute steps over a
year: 105,120 String allocations solely for actor names.

**Fix:** Pre-allocate actor name strings into a `Vec<String>` on `Dwelling` at init;
`per_actor_timing` stores `(usize, StdDuration)` and resolves the name from the
pre-built slice at reporting time only.

---

**Batch 4.5 verification:**
```bash
cargo test --workspace
cargo test --workspace --features check_invariants
```
HP-1 through HP-4 must reduce the allocation count in `check_invariants`-enabled
builds to zero per step for the affected paths. No test regressions.

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
| P4-C | P1-F, P2-T, P2-U | BESTEST cases 600, 640, 900, 600FF, 900FF within ASHRAE 140 bands; requires P1-F (RC splitting), P2-T (mass multiplier), P2-U (MELs gains) | M |

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
                                    Batch 4.5 (hot-path allocations)
                                        │
                                    Batch 5 (HPXML, output, Python API)
```

---

## Appendix: Complete Ticket Resolution Map

147 review tickets: 132 resolved by work order batches, 14 no action needed (correct/acceptable), 1 deferred.

### AR series (10 tickets)

| Ticket | Resolution |
|--------|------------|
| AR-001 | Batch 2 (CO typed telemetry) |
| AR-002 | Batch 1 (CFG typed config) |
| AR-003 | Batch 4 P2-E (duct/port accounting) |
| AR-004 | Batch 1 (typed config eliminates raw f64 boundary issues) |
| AR-005 | B0-5 (registry name fixes) |
| AR-006 | Batch 4 P2-A (fan heat) + P2-E (duct) |
| AR-007 | Batch 2 (CO EndUse) |
| AR-008 | Batch 5 P5-A (output columns) |
| AR-009 | B0-1 (DST test compile) + deferred (test audit) |
| AR-010 | Batch 4 (dispatch layer) |

### TS series (11 tickets)

| Ticket | Resolution |
|--------|------------|
| TS-001 | Batch 3 P1-A (foundation init) |
| TS-002 | No action (correct/low) |
| TS-003 | Batch 3 (ground temp wiring) |
| TS-004 | Batch 3 P1-A (foundation geometry) |
| TS-005 | No action (correct) |
| TS-006 | Batch 3 P1-E (infiltration h_limit) |
| TS-007 | Batch 3 P1-C (b_coeff fix) |
| TS-008 | Batch 3 P1-D (solar conservation) |
| TS-009 | Batch 3 P1-C (LWR iteration) |
| TS-010 | Batch 3 P1-E (humidity latent constant) |
| TS-011 | Batch 3 P1-B (attic zone) |

### UC series (10 tickets)

| Ticket | Resolution |
|--------|------------|
| UC-001 | No action (correct) |
| UC-002 | Batch 1 (typed config fixes key mismatch) |
| UC-003 | Batch 5 P3-A (XML duct leakage path) |
| UC-004 | Batch 1 (typed config) |
| UC-005 | B0-2 (AssemblyEffectiveRValue descendant lookup) |
| UC-006 | No action (correct) |
| UC-007 | Batch 5 P3-B (exterior shading parsing) |
| UC-008 | No action (correct) |
| UC-009 | Batch 5 P3-B (ModuleType case-insensitive) |
| UC-010 | No action (correct, low-risk passthrough) |

### CW series (22 tickets)

| Tickets | Resolution |
|---------|------------|
| CW-001 through CW-022 | Batch 1 (CFG-007→016: typed config migration eliminates all key mismatches) |

### IC series (7 tickets)

| Ticket | Resolution |
|--------|------------|
| IC-001 | Batch 4 P2-A (HvacEquipment auto-select consistency) |
| IC-002 | Batch 4 P2-A (latent cooling, EndUse) |
| IC-003 | Batch 4 P2-A (capacity clip, unmet-load column) |
| IC-004 | Batch 4 P2-F (HPWH backup element capacity key) |
| IC-005 | Batch 4 P2-F (simultaneous element mode) |
| IC-006 | Batch 4 P2-F (water heater fixes) |
| IC-007 | No action (low severity) |

### DT series (12 tickets)

| Ticket | Resolution |
|--------|------------|
| DT-001 | CC-007 local-time policy (UTC paths removed) |
| DT-002 | CC-007 local-time policy (TimezoneMismatch variant superseded) |
| DT-003 | CC-007 local-time policy |
| DT-004 | CC-007 local-time policy |
| DT-005 | CC-007 local-time policy (DST FixedOffset base path is correct by policy) |
| DT-006 | CC-007 local-time policy |
| DT-007 | CC-007 local-time policy (invariant: start_time is local) |
| DT-008 | CC-007 local-time policy |
| DT-009 | CC-007 local-time policy (EPW rebase behavior is intended design) |
| DT-010 | Batch 3 (solar override offset) |
| DT-011 | B0-1 (DST test compile) + Batch 3 |
| DT-012 | No action (correct) |

### WO series (4 tickets)

| Ticket | Resolution |
|--------|------------|
| WO-001 | No action (medium, documented by CC-007 policy) |
| WO-002 | B0-4 (water draw fraction → L/min scaling) |
| WO-003 | Batch 4 P2-H (capacity curves) + P2-Q (biquadratic bounds) |
| WO-004 | No action (correct, HARES is better than OCHRE here) |

### DL/FP series (7 tickets)

| Ticket | Resolution |
|--------|------------|
| DL-001 | Batch 4 P2-E (duct loss accounting) |
| DL-002 | Batch 4 P2-E (DSE=1 in conditioned zone) |
| DL-003 | Batch 4 P2-E (duct zone contributions) |
| DL-004 | Batch 4 P2-E (basement airflow ratio) |
| FP-001 | Batch 4 P2-A (ElectricFurnace fan heat — CRITICAL) |
| FP-002 | Batch 4 P2-A (IdealHvac fan power) |
| FP-003 | Batch 4 P2-A (HeatPumpHeater + AirConditioner fan heat) |

### EG series (7 tickets)

| Ticket | Resolution |
|--------|------------|
| EG-001 | Batch 3 P1-A (foundation zone geometry) |
| EG-002 | Batch 3 P1-B (attic LWR, gable selection) |
| EG-003 | Batch 3 P1-A (foundation wall height) |
| EG-004 | Batch 3 P1-A (conditioned floor area) |
| EG-005 | Batch 3 P1-A (foundation area subtraction) |
| EG-006 | Batch 3 P1-B (attic volume formula) |
| EG-007 | No action (correct) |

### DC series (8 tickets)

| Ticket | Resolution |
|--------|------------|
| DC-001 | Batch 4 P2-K (HVAC speed selection) |
| DC-002 | Batch 4 P2-K (multi-speed interpolation) |
| DC-003 | Batch 4 P2-F (simultaneous element mode) |
| DC-004 | Batch 3 P1-E (ventilation double-accounting) |
| DC-005 | Batch 4 (dehumidifier typed config via CFG-010) |
| DC-006 | Batch 4 P2-I (gain fraction defaults) |
| DC-007 | Batch 4 P2-G (EventBasedLoad checkpoint mid-event) |
| DC-008 | No action (correct, comment fix only) |

### TP series (9 tickets)

| Ticket | Resolution |
|--------|------------|
| TP-001 | Batch 2 (CO typed telemetry) |
| TP-002 | Batch 2 (CO typed telemetry) |
| TP-003 | Batch 2 (CO telemetry) + Batch 5 P5-D (checkpoint completeness) |
| TP-004 | Batch 4 P2-B (battery SOH applied to nominal capacity) |
| TP-005 | Batch 2 (CO typed telemetry) |
| TP-006 | Batch 2 (CO typed telemetry) |
| TP-007 | Batch 2 (CO telemetry) + Batch 5 P5-D (checkpoint completeness) |
| TP-008 | Batch 5 P5-E (remaining telemetry field coverage) |
| TP-009 | Batch 4 P2-E (DuctLoss telemetry category) |

### EA series (7 tickets)

| Ticket | Resolution |
|--------|------------|
| EA-001 | Batch 3 P1-E (ventilation double-accounting) |
| EA-002 | Batch 1 (fuel type via typed config) |
| EA-003 | No action (correct) |
| EA-004 | Batch 4 P2-G (gas appliance combustion watts) |
| EA-005 | Batch 4 (EV fixes) |
| EA-006 | Batch 4 P2-B (battery: double ohmic loss, BOL transient, SOC/OCV timing) |
| EA-007 | Batch 4 P2-C (defrost: post-defrost capacity, remove 0.75 factor) |

### PS series (6 tickets)

| Ticket | Resolution |
|--------|------------|
| PS-001 | Batch 5 P5-B (overrides wiring + unknown-key detection + construction path unification) |
| PS-002 | Batch 5 P5-B (ControlSignal.event_delay() constructor) |
| PS-003 | Batch 5 P5-B (Actor.decide() typed EnvironmentView) |
| PS-004 | Batch 5 P5-B (complete _hares.pyi type stubs) |
| PS-005 | Batch 5 P5-B (equipment construction parameter validation) |
| PS-006 | Batch 5 P5-B (catch_unwind + typed Python exceptions + Mutex scope) |

### PA series (8 tickets)

| Ticket | Resolution |
|--------|------------|
| PA-001 | Batch 5 P5-C (telemetry timestep_index/current_time) |
| PA-002 | Batch 5 P5-C (Battery/EV LUT getters) |
| PA-003 | Batch 5 P5-C (Dwelling.envelope_diagnostics() exposure) |
| PA-004 | Batch 5 P5-C (Dwelling.profiling_summary() exposure) |
| PA-005 | No action (correct, Arrow output format already selectable) |
| PA-006 | Batch 5 P5-C (from_hpxml() kwarg equipment config audit) |
| PA-007 | Batch 5 P5-C (step() return dict keys vs OCHRE) |
| PA-008 | Batch 5 P5-C (DispatchRequest EV signal constructors) |

### SU series (9 tickets)

| Ticket | Resolution |
|--------|------------|
| SU-001 | Batch 4 P2-L (fan shaft heat DB correction before SHR calculation) |
| SU-002 | Batch 4 P2-K (HVAC speed selection and interpolation) |
| SU-003 | Batch 4 P2-P (checkpoint: thermostat hysteresis and speed timer) |
| SU-004 | Batch 4 P2-M (natural ventilation ELA coefficients and zone scope) |
| SU-005 | Batch 4 P2-N (tariff: tiered block rates + minimum charge base) |
| SU-006 | Batch 5 (fleet aggregation) |
| SU-007 | Batch 4.5 (hot-path allocation fixes HP-1 through HP-5) |
| SU-008 | B0-3 (occupancy scale) + B0-4 (water draw scale) |
| SU-009 | Batch 4 P2-O (TMV dead code + outlet temperature blending) |

### DV series (5 tickets)

| Ticket | Resolution |
|--------|------------|
| DV-001 | B0-6 (psychrometric wet-bulb supersaturated input clamp) |
| DV-002 | No action (HARES correct, OCHRE has the bug) |
| DV-003 | Batch 4 P2-Q (biquadratic input bounds + PLF clamps wired from CSV) |
| DV-004 | Batch 4 P2-R (battery pack topology + default resistance) |
| DV-005 | Batch 4 P2-S (envelope LUT filter clear on minimal clamping) |

### BESTEST investigation findings (3 tickets, 2026-03-30)

| Finding | Resolution |
|---------|------------|
| BESTEST-900: 1 RC node per 100mm concrete layer → 2.1× amplitude over-transmission | Batch 3 P1-F (RC layer auto-splitting) |
| BESTEST-600: INTERIOR_MASS_MULTIPLIER=7.0 applied to bare-box (no furniture) | Batch 4 P2-T (synthetic fixture mass multiplier override) |
| BESTEST-600: MELs sensible_gain_fraction=0.855 + ANSI/RESNET time-varying schedule | Batch 4 P2-U (synthetic fixture internal gains compliance) |
