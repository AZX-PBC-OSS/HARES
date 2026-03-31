# Cross-Cutting Architectural Issues

These are systemic problems that affect many tickets. Fixing at the architectural level
eliminates entire classes of bugs rather than whack-a-mole per-equipment fixes.

---

## CC-001: Config key contract — typed config structs per equipment

**Fixes:** AR-002, CW-001 through CW-022 (all key mismatches), UC-002, UC-004
**Implemented by:** CFG-007 through CFG-016

**Problem:** `EquipmentSpec.parameters` is `Map<String, Value>` with no compile-time or
init-time validation. The resolver writes keys with one naming convention, equipment reads
different names. Mismatches are silent — the value is just missing, and a default is used.

**Affected equipment (confirmed key mismatches):**
- Gas/Electric Furnace: `heating_efficiency` vs `fuel_efficiency`/`afue`/`efficiency`
- Boiler (gas + electric): same pattern
- All AC/HP cooling: `efficiency_seer`/`cooling_efficiency` vs `seer`/`SEER`
- Tankless WH: `FuelType` vs `fuel_type`, `EnergyFactor` vs `energy_factor`, `heating_capacity_w` vs `max_thermal_power_w`
- Gas WH: `EnergyFactor` vs `energy_factor`, `fuel_type` vs `FuelType`
- HPWH: `heating_capacity_w` vs `backup_element_power_w`
- Ventilation: 3 key mismatches (flow rate, effectiveness, type)
- Battery: double-sqrt from storing pre-processed value
- EV: `battery_capacity_kwh` vs `capacity_kwh`

**Fix:** Replace `Map<String, Value>` with per-equipment typed config structs:
```rust
pub struct FurnaceConfig {
    pub capacity_w: f64,
    pub eir: f64,
    pub fuel_type: FuelType,
    pub fan_power_w: Option<f64>,
    // ...
}
```
With `#[serde(deny_unknown_fields)]` to catch dead keys at parse time.

**Priority:** CRITICAL — this is the single highest-impact architectural change.

---

## CC-002: Telemetry key contract — typed telemetry per equipment category

**Fixes:** AR-001, AR-008, TP-001 through TP-009

**Problem:** `HashMap<String, f64>` with magic string keys. Writer and reader use different
strings (`"mode"` vs `"operating_mode"`, `"electric_output_kw"` vs `"electric_kw"`).
Silent zero values in output.

**Fix:** Per-equipment-category telemetry structs or a shared constants module imported
by both writer and reader. Make `Telemetry::set` a hard error (not debug_assert) for
undeclared keys.

**Priority:** HIGH

---

## CC-003: Init-time config validation — detect unused/missing keys

**Fixes:** All CW-series key mismatches, AR-005
**Implemented by:** CFG-015

**Problem:** Equipment `init()` silently ignores keys it doesn't read, and silently defaults
when keys are missing. No diagnostic that config was actually consumed.

**Fix:** Track which keys are read during init. After init, warn on any unread keys
(dead config) and any expected-but-missing keys. This is a safety net even after CC-001.

**Priority:** HIGH — cheap to implement, catches future regressions.

---

## CC-004: Gain fraction defaults — centralize per-load-type table

**Fixes:** CW-017 (5 wrong gain fractions), CW-020

**Problem:** Sensible/latent gain fractions are scattered across match arms in
`resolve_loads.rs` with incorrect defaults. Each load type needs specific values.

**Fix:** Single `const` table mapping load type → (sensible_frac, latent_frac) with
values from ASHRAE/OCHRE. All resolvers read from this table.

**Priority:** MEDIUM — straightforward data fix.

---

## CC-005: Silent equipment drop on registration failure

**Fixes:** AR-005 (EV, Gas Tankless WH, Generic Heater/Cooler all silently dropped)
**Implemented by:** CFG-008

**Problem:** `registry.create()` failure is caught with `continue` — the simulation
succeeds with missing equipment and no error.

**Fix:** Make registration failure a hard error (or at minimum a visible warning that
lists all dropped equipment at simulation start). Users must know their building model
is incomplete.

**Priority:** HIGH

---

## CC-006: Silent default on missing required input

**Fixes:** TS-011 (200m³ attic volume), TS-004 (foundation height), CW-019 (dehumidifier),
AR-004 (conductivity without units)

**Problem:** When required physics input is missing, HARES substitutes a guess instead
of erroring. OCHRE raises exceptions in many of these cases.

**Fix:** Required fields must error, not default. Optional fields with documented defaults
are fine. The distinction must be explicit per field.

**Priority:** MEDIUM

---

## CC-007: Local-time-only policy — no UTC conversion anywhere in the stack

**Affects:** DT-001 through DT-010, WO-001, WO-004

**Policy:** HARES uses local time internally throughout. HPXML and EPW weather are always in local time. There is no reason to support UTC or multiple timezones. All internal timestamps are local to the building site. `start_time` from Python is interpreted as local time. EPW `timezone_offset_h` is metadata for solar position calculation only, not for time conversion.

**Consequence for open findings:** Any DT-series finding that frames its fix as "validate that start_time offset matches EPW offset" or "add a TimezoneMismatch error variant" is superseded by this policy. The correct resolution is to remove UTC conversion paths entirely rather than add validation around them. Specifically:

- DT-002 F1 ("add `TimezoneMismatch` error") — instead, document that `start_time` is always local and remove any UTC assumption from `compute_schedule_offset`.
- DT-005 DEFECT 1 (DST FixedOffset) — DST support via `civil_timezone` is opt-in; the base path is local-fixed-offset and is correct by policy.
- DT-009 D1/D2 (naive input assumed UTC, EPW rebase discards offset) — the EPW rebase behavior (keep wall-clock, stamp with EPW offset) is the intended design; the "fix" is to document it clearly, not to preserve arbitrary user-supplied offsets.
- DT-007 Defect 2 (UTC start_time debug assert) — the invariant to enforce is "start_time is local" not "offset matches EPW"; a zero UTC offset on a US location is the bug to reject.

**Priority:** HIGH — resolves ambiguity across the entire DT ticket series.

---

## CC-008: Fan heat not injected into zone thermal port

**Affects:** FP-001, FP-002, FP-003, AR-006, CW-002
**Resolves in:** Phase 2 (P2-A)

**Problem:** Fan motor heat is both an electrical load and a sensible thermal gain to the
conditioned zone (100% of fan shaft energy becomes room heat). OCHRE adds `fan_power` to
`delivered_heat` unconditionally for all HVAC types. HARES:

- `ElectricFurnace`: no `fan_power_w` field at all — neither electrical nor zone heat (FP-001 F1, CRITICAL)
- `HeatPumpHeater`: fan in electrical port, not in thermal port (FP-003 F2, BUG)
- `AirConditioner`: fan adjusts coil entering temp (correct) but not added additively to zone gains (FP-003 F1, BUG)
- `IdealHvac`: no fan concept at all — zero fan power in all modes (FP-002, HIGH)

**Fix:** Pattern is `GasFurnace` which correctly adds `fan_heat_w` to `total_sensible_w`.
Align `ElectricFurnace`, `HeatPumpHeater`, `AirConditioner`, and `IdealHvac` to the same
pattern. For AC: net zone gain = `-(sensible_cooling_w - fan_kw * 1000.0)`.

**Priority:** CRITICAL for ElectricFurnace; HIGH for others.

---

## CC-009: Ventilation double-accounting of sensible/latent load

**Affects:** EA-001 (F1, CRITICAL), DC-004 (F-2, HIGH)
**Resolves in:** Phase 1 (P1-E)

**Problem:** Ventilation thermal load is counted twice every step:
1. `Ventilation::step()` computes `q_sensible_w = m_dot * Cp * (T_supply - T_indoor)` and
   pushes it via `PortContribution::Thermal { category: InternalGain }`.
2. `apply_infiltration_and_ventilation()` in `infiltration.rs` independently computes the
   same forced-ventilation heat load from `config.ventilation_flow_m3_s` and adds it via
   the semi-implicit A-matrix coupling.

Both paths run every timestep for the same fan. Magnitude: `~845 W` duplicated at
`ΔT=20 K, 0.035 m³/s`. Additionally, config key mismatches (EA-001 F2, F3, F4) mean
the `Ventilation` equipment uses hardcoded defaults while the infiltration solver uses
correct HPXML values — they even use different flow rates.

**Root fix:** One owner only. OCHRE places ventilation entirely in the envelope solver.
Remove `PortContribution::Thermal` from `Ventilation::step()`. Equipment reports fan
electrical power and telemetry only. Envelope solver owns all ventilation heat exchange.

**Priority:** CRITICAL — energy balance error every step for every building with ventilation.

---

## CC-010: Checkpoint/restore gaps in equipment telemetry

**Affects:** TP-003 (HIGH-1, MEDIUM-1, MEDIUM-2), TP-007 (F1), DC-007 (GAP: checkpoint mid-event), EA-006 (F4 timing)

**Problem:** Several equipment types do not persist all telemetry fields across
`save_state` / `load_state` round-trips. After restore, callers that read telemetry before
the first `step()` (HELICS handshake, fleet aggregators, warm-start supervisors) receive
stale zero values:

- HPWH: `ELECTRIC_KW` not in `HpwhState` — zeroed on restore (TP-003 HIGH-1)
- HPWH: `WALL_SENSIBLE_GAIN_W`, `UNMET_LOAD_W` not restored (TP-003 MEDIUM-2)
- `ResistanceWH`, `GasWH`: `OUTLET_TEMP_C`, `UNMET_LOAD_W` absent entirely (TP-003 MEDIUM-3)
- `ScheduledLoad`: `REACTIVE_POWER_KVAR` not restored (TP-007 F1)
- `EventBasedLoad`: `active_power_kw` not persisted; one-step gap after restore mid-event (DC-007 GAP)
- Battery: `OCV`/`SOC` passed to degradation accumulate at end-of-step, not start-of-step (EA-006 F4)
- All tank-backed WH: `SKIN_LOSS_W` not checkpointed (TP-003 LOW-1)

**Fix:** Audit each equipment's `*State` struct against its `default_telemetry()` and
`step()` writes. Every field written in `step()` that is needed before the next step
must be saved and restored.

**Priority:** HIGH — affects HELICS federation correctness and fleet simulation warm-starts.

---

## CC-011: Battery degradation behaviourally inert

**Affects:** TP-004 (CRITICAL), EA-006 (F1, F3)
**Resolves in:** Phase 2

**Problem:** Three independent defects make the battery degradation subsystem functionally
disconnected from actual battery behaviour:

1. **Degradation does not reduce `capacity_kwh_nominal`** (TP-004 CRITICAL): `capacity_fade_pct()` is computed and written to telemetry but never applied to `capacity_kwh_nominal`. After simulated years of cycling, the battery behaves identically to a new cell.

2. **Mechanism 3 (BOL transient) is dead code** (EA-006 F3, HIGH): `B3_REF = -2.805e-2` (negative). The expression `(b3_accum - q_li3).max(0.0)` with `b3_accum < 0` always returns `0.0`. `dq_li3` is permanently zero. `q_li3` never updates.

3. **Double ohmic loss on discharge** (EA-006 F1, CRITICAL): `dc_power_kw = power_kw / discharge_efficiency` already embeds losses; `effective_cell_power_kw` then subtracts `ohmic_loss_w` again. Ohmic losses are applied twice on every discharge step, causing systematic SOC drift.

**Fix (TP-004):** Store `capacity_kwh_rated` immutably; apply `soh = 1 - fade_pct` to `capacity_kwh_nominal` at the daily boundary. **Fix (EA-006 F3):** Implement sign-correct `q3` update or remove the dead mechanism with clear documentation. **Fix (EA-006 F1):** Remove the redundant `ohmic_loss_w` subtraction in `effective_cell_power_kw`.

**Priority:** CRITICAL for F1 (energy balance error per step) and TP-004 (degradation model inert); HIGH for F3.

---

## CC-012: IdealHvac missing features

**Affects:** IC-001, IC-002, IC-003, FP-002

**Problem:** `IdealHvac` is missing several features that OCHRE implements:

- **No capacity upper-bound clip** (IC-003 F2, HIGH): ideal path is unbounded — a 10 kW unit silently delivers 50 kW if the solver requests it.
- **No fan power** (FP-002, HIGH): zero fan electrical consumption and no fan zone heat in all ideal modes.
- **No latent cooling** (IC-002 D1, MEDIUM): `latent_gain_w` is always `0.0`; humidity solver starved of dehumidification in cooling mode.
- **Wrong EndUse hardcoded** (IC-002 D4, LOW): `EndUse::HVAC_HEATING` regardless of operating mode.
- **`HvacEquipment` auto-select inconsistent** (IC-001, MEDIUM): `IdealHvac` auto-selects via `n_speeds >= 4 || time_res >= 300s`; `HvacEquipment` (used by `AirConditioner`, etc.) requires explicit config flag only.
- **No unmet-load capacity column** (IC-003 F6, HIGH): `IdealHvac` emits no `HVAC Heating/Cooling Capacity (W)` column; `unmet_load_hours` metric is silently zeroed for all ideal-HVAC dwellings.

**Priority:** HIGH for unbounded capacity and missing unmet-load detection.

---

## CC-013: Python safety boundary

**Affects:** PS-001 through PS-006, PA-001 through PA-008

**Problem:** Multiple safety holes exist at the Python/Rust FFI boundary:

- **No `catch_unwind` in `step()` or `simulate()`** (PS-006 CRITICAL): Rust panics abort the Python process with no traceback. `batch_step_py` Rayon closures have the same issue.
- **`from_hpxml` silently drops `overrides` and `resample_overrides`** (PS-001 F1, CRITICAL): `build_config` hard-codes both to `None` regardless of caller input.
- **No unknown-key detection** (PS-001 F2, HIGH): misspelled kwargs silently ignored.
- **Two parallel construction paths** (PS-001 F3, HIGH): `DwellingConfig` and `build_config` diverge; features added to one are not in the other.
- **All `HaresError` variants → `PyValueError`** (PS-006 HIGH): callers cannot distinguish physics errors from IO errors from invariant violations.
- **`simulate()` holds Mutex for entire run** (PS-006 HIGH): blocks any concurrent access for multi-step simulations.

**Fix priority order:** `catch_unwind` first (process stability), then `overrides` wiring, then unknown-key detection, then typed Python exceptions.

**Priority:** CRITICAL for `catch_unwind` and `overrides` drop; HIGH for the rest.

---

## CC-014: HPXML XML path bugs

**Affects:** UC-003 (CRITICAL), UC-005 (HIGH)

**Problem:** Two XML navigation bugs cause critical parse failures on real HPXML files:

- **Duct leakage always 0** (UC-003 F1/F2, CRITICAL): HPXML 4.x places `<DuctLeakageMeasurement>` as a sibling of `<Ducts>` under `<AirDistribution>`, not a child. `first_descendant("DuctLeakage")` always returns `None` for production fixtures. Even if found, `text_as_f64()` is called on the `<DuctLeakage>` node whose text is empty — the value is in a `<Value>` child. All parity-corpus buildings silently use `leakage_fraction = 0.0`.

- **`AssemblyEffectiveRValue` silently dropped** (UC-005 F-001, HIGH): standard HPXML nests this value under `<Insulation>/<AssemblyEffectiveRValue>`. `node.child("AssemblyEffectiveRValue")` only searches direct children, finding nothing. Fix: `node.first_descendant("AssemblyEffectiveRValue")`.

**Priority:** CRITICAL for duct leakage (affects all duct-loss calculations); HIGH for R-value.

---

## CC-015: Hot water draw 950× underscaled

**Affects:** WO-002 (CRITICAL)

**Problem:** `inject_water_heater_schedule_columns` stores only the raw column index for
`hot_water_fixtures`. At runtime `resolve_storage_step_inputs` treats the schedule
fraction (0–1) as if it were already in L/min. A typical fraction of 0.1 produces
`0.1 / 60 = 0.00167 kg/s` instead of the correct ~0.1 L/min. The underscale factor is
roughly `950×` for a 3-bedroom home. Water heater tanks never deplete and thermal energy
balance is wrong across all storage WH types.

**Fix:** In `inject_water_heater_schedule_columns`, apply `normalize_draw_profile()` (which
already exists in `draw_profile.rs`) to convert fractions to L/min before storing the
column. `avg_water_draw_l_per_day` is already available in spec parameters.

**Priority:** CRITICAL — affects energy balance for every simulation with a storage water heater and a ResStock schedule.

---

## CC-016: Gas appliance fuel consumption is zero

**Affects:** EA-004 (F1, CRITICAL)

**Problem:** `schedule_resolve.rs::determine_max_kw()` reads only `annual_electric_kwh`;
`annual_gas_therms` is never consulted. For gas dryers and gas cooking ranges, the combustion
energy (~93% of total for a gas dryer) is entirely absent from the event power series.
`EventBasedLoad` routes `active_power_kw` (only the ~7% electric parasitic) to the fuel port.

**Fix:** When `fuel_type == Gas`, read `annual_gas_therms`, convert to average watts
(`1 therm = 105,480,400 J`), and add to the electric parasitic in `determine_max_kw()`.

**Priority:** CRITICAL — complete omission of dominant energy component for gas appliances.

---

## CC-017: Defrost extra power uses pre-defrost capacity (2× overestimate)

**Affects:** EA-007 (F1, CRITICAL)

**Problem:** HARES computes `extra_power_w` using the pre-defrost `current_capacity_w`.
OCHRE uses the post-defrost capacity (`current_cap * cap_mult − q_defrost`). At typical
defrost conditions (OAT ≈ 0 °C, `cap_mult` ≈ 0.55–0.66) post-defrost capacity is 45–55%
lower than pre-defrost, so HARES overestimates defrost electrical consumption by roughly
2×. Also, `DEFAULT_DEFROST_CAPACITY_REDUCTION_FACTOR = 0.75` has no EnergyPlus basis
(EA-007 F2, HIGH); OCHRE has no equivalent secondary scaling. HARES applies an extra
25% capacity penalty above the physically-derived `cap_mult` in all defrost-active steps.

**Priority:** CRITICAL for the pre/post defrost capacity bug; HIGH for the 0.75 fallback.

---

## Related ticket clusters (same root cause, fix together)

### Cluster A: SEER/EER key mismatch (fix with CC-001 / CFG-010, CFG-011)
- UC-002, CW-005, CW-006, CW-008, CW-010

### Cluster B: Heating efficiency key mismatch (fix with CC-001 / CFG-009, CFG-011)
- CW-001, CW-002, CW-004, CW-007

### Cluster C: Water heater key casing (fix with CC-001 / CFG-012)
- CW-012, CW-013, CW-014

### Cluster D: Ventilation key mismatch (fix with CC-001 / CFG-013)
- AR-002 Finding 3, CW-018, EA-001 F2/F3/F4, DC-004 F-4

### Cluster E: Gain fraction defaults (fix with CC-004)
- CW-017 (lighting, MELs, TV, ceiling fan, freezer)
- CW-020 (appliance defaults)

### Cluster F: Mini-split speed inference (single fix / CFG-010)
- CW-009, CW-010 (both need unconditional 4-speed for MSHP)

### Cluster G: Multispeed SHR not propagated (single fix / CFG-010)
- CW-008, CW-010 (per-stage SHR loaded but discarded)

### Cluster H: Fan heat zone injection (fix together in P2-A)
- FP-001 F1 (ElectricFurnace — CRITICAL), FP-001 F3 (HP heater + AC), FP-003 F1 (AC), FP-003 F2 (HP heater), FP-002 (IdealHvac), AR-006, CW-002

### Cluster I: Ventilation double-accounting and key mismatches (fix together in P1-E)
- EA-001 F1/F2/F3/F4, DC-004 F-2/F-4, CW-018

### Cluster J: Battery degradation inert (fix together in Phase 2)
- TP-004 CRITICAL, EA-006 F1, EA-006 F3

### Cluster K: Checkpoint telemetry gaps (fix in Phase 5 or before HELICS work)
- TP-003 HIGH-1/MEDIUM-2, TP-007 F1, DC-007 GAP, EA-006 F4

### Cluster L: IdealHvac missing features (fix together in P2-A)
- IC-001, IC-002 D1/D2/D3/D4, IC-003 F2/F4/F6, FP-002
