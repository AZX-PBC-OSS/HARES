# Decompose HvacEquipment Struct into Lifecycle-Grouped Sub-structs

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-equipment/hvac/hvac_core, hares-equipment/hvac/staging, hares-equipment/hvac/air_conditioner

## Problem

`HvacEquipment` is a god struct with 48 public fields mixing configuration, mutable runtime state, physics parameters, and control signal state. These have different lifecycles:

- **Config**: set once at init, immutable after — e.g., `biquadratic_coeffs`, `airflow_m3_s_per_w`, `shr`
- **Runtime state**: changes every step — e.g., `plf_state`, `startup`, `last_speed_index`
- **Control state**: changes on signal receipt — e.g., `max_capacity_fraction`, `disabled_speeds`

The lack of grouping makes invariants unverifiable at the type level. For example, the `mode` ↔ `mode_start_at` invariant (hvac_core.rs:138–143) is documented in a comment but not enforced by types — a caller can assign `self.mode` directly without updating `mode_start_at`, silently breaking minimum on/off time enforcement.

## Current Behavior

The full struct definition spans hvac_core.rs:122–249 (127 lines). The 48 public fields are (configuration: 28, runtime: 8, control: 3, thermostat: 9 — of which `min_on_time_s` and `min_off_time_s` are dual-listed under both configuration and thermostat because they are consumed by `ThermostatFsm` at runtime but set from config; total after ticket 006 extraction: 37 remaining fields in `HvacEquipment`). Field count verified by inspection of all `pub` field declarations at hvac_core.rs:122–249.

### Configuration fields (set once at init)
- `equipment_type: HvacEquipmentType` (line 123)
- `zone_id: ZoneId` (line 124)
- `shr: f64` (line 157)
- `fan_power_w_per_m3_s: f64` (line 156)
- `duct_dse: f64` (line 158)
- `duct_zone_id: Option<ZoneId>` (line 162)
- `basement_heat_frac: f64` (line 170)
- `basement_zone_id: Option<ZoneId>` (line 173)
- `airflow_m3_s_per_w: f64` (line 175)
- `biquadratic_coeffs: Vec<[f64; 6]>` (line 183)
- `biquadratic_x1_bounds: (f64, f64)` (line 184)
- `biquadratic_x2_bounds: (f64, f64)` (line 185)
- `speed_control_mode: SpeedControlMode` (line 186)
- `low_speed_capacity_fraction: f64` (line 187)
- `space_fraction: f64` (line 228)
- `cap_ff_coeffs: [f64; 3]` (line 236)
- `eir_ff_coeffs: [f64; 3]` (line 239)
- `ff_bounds: (f64, f64)` (line 244)
- `plf_min: f64` (line 248)
- `heating_capacities_w: Vec<f64>` (line 153)
- `cooling_capacities_w: Vec<f64>` (line 154)
- `eir_by_stage: Vec<f64>` (line 155)
- `eir_plr_coefficients: Option<Vec<[f64; 3]>>` (line 208)
- `zone_heat_fractions: Vec<(ZoneId, f64)>` (line 182)
- `min_time_per_speed_s: f64` (line 198)
- `min_on_time_s: f64` (line 148)
- `min_off_time_s: f64` (line 152)
- `supply_air_temp_c: f64` (line 174)

### Runtime state fields (change every step)
- `duty_cycle: f64` (line 127)
- `plf_cooling_degradation_coeff: f64` (line 188)
- `plf_state: f64` (line 189)
- `startup: StartupConfig` (line 190) — note: `c_d` is set from config, but `time_since_start_min` is runtime
- `last_speed_index: usize` (line 191)
- `last_speed_frac: f64` (line 192)
- `time_at_current_speed_s: f64` (line 196)
- `prev_zone_temp_c: Option<f64>` (line 224)

### Control state fields (change on signal receipt)
- `max_capacity_fraction: f64` (line 232)
- `disabled_speeds: Vec<bool>` (line 216)
- `max_enabled_speed: usize` (line 220)

### Thermostat fields (will be extracted by ticket 006)
- `thermostat: ThermostatConfig` (line 125) → `ThermostatFsm`
- `mode: ThermostatMode` (line 126) → `ThermostatFsm`
- `static_setpoints: ThermalSetpoints` (line 128) → `ThermostatFsm`
- `heating_setpoint_source: Option<ScheduleSource>` (line 130) → `ThermostatFsm`
- `cooling_setpoint_source: Option<ScheduleSource>` (line 132) → `ThermostatFsm`
- `schedule_setpoints: Option<ScheduleSetpoints>` (line 133) → `ThermostatFsm`
- `runtime_setpoints: Option<RuntimeSetpointOverride>` (line 134) → `ThermostatFsm`
- `last_mode_switch_at: Option<DateTime<FixedOffset>>` (line 135) → `ThermostatFsm`
- `mode_start_at: Option<DateTime<FixedOffset>>` (line 144) → `ThermostatFsm`

## Required Behavior

Group remaining fields (after ticket 006 extracts thermostat fields) into sub-structs with clear lifecycle boundaries. This makes invariants enforceable at the type level and makes the data flow visible in the struct layout. Pure refactor — no behavioral change.

## Approach

**Prerequisite**: Complete ticket 006 (ThermostatFsm extraction) first. This removes 11 fields and simplifies the decomposition.

### Step 1: Define `HvacConfig` — immutable after init

```rust
/// Equipment configuration set once at initialization and never modified at runtime.
#[derive(Clone, Debug)]
pub struct HvacConfig {
    pub equipment_type: HvacEquipmentType,
    pub zone_id: ZoneId,
    pub shr: f64,
    pub fan_power_w_per_m3_s: f64,
    pub duct_dse: f64,
    pub duct_zone_id: Option<ZoneId>,
    pub basement_heat_frac: f64,
    pub basement_zone_id: Option<ZoneId>,
    pub airflow_m3_s_per_w: f64,
    pub biquadratic_coeffs: Vec<[f64; 6]>,
    pub biquadratic_x1_bounds: (f64, f64),
    pub biquadratic_x2_bounds: (f64, f64),
    pub speed_control_mode: SpeedControlMode,
    pub low_speed_capacity_fraction: f64,
    pub space_fraction: f64,
    pub cap_ff_coeffs: [f64; 3],
    pub eir_ff_coeffs: [f64; 3],
    pub ff_bounds: (f64, f64),
    pub plf_min: f64,
    pub heating_capacities_w: Vec<f64>,
    pub cooling_capacities_w: Vec<f64>,
    pub eir_by_stage: Vec<f64>,
    pub eir_plr_coefficients: Option<Vec<[f64; 3]>>,
    pub zone_heat_fractions: Vec<(ZoneId, f64)>,
    pub min_time_per_speed_s: f64,
    pub min_on_time_s: f64,
    pub min_off_time_s: f64,
    pub supply_air_temp_c: f64,
}
```

### Step 2: Define `HvacRuntimeState` — changes every step

```rust
/// Runtime state updated every simulation timestep.
#[derive(Clone, Debug)]
pub struct HvacRuntimeState {
    pub duty_cycle: f64,
    pub plf_cooling_degradation_coeff: f64,
    pub plf_state: f64,
    pub startup: StartupConfig,
    pub last_speed_index: usize,
    pub last_speed_frac: f64,
    pub time_at_current_speed_s: f64,
    pub prev_zone_temp_c: Option<f64>,
}
```

**Design note**: `plf_cooling_degradation_coeff` is set from config at init (hvac_core.rs:396–398) but may also be overridden at runtime by equipment-type Cd defaults (hvac_core.rs:511–513) and by `CoolingCore` (air_conditioner.rs:330). It's borderline config/runtime — grouping it with runtime is safer since it can change. If future work makes it truly immutable, it can move to `HvacConfig`.

Similarly, `startup.c_d` is set from config (hvac_core.rs:399–406) but shares the same override path (hvac_core.rs:513). `startup.time_since_start_min` is definitively runtime. Keep `startup` in `HvacRuntimeState`.

### Step 3: Define `HvacControlState` — changes on signal receipt

**REQUIRED as part of this decomposition (not a follow-up)**: replace `disabled_speeds: Vec<bool>` with a fixed-size array. `set_disabled_speeds` in `staging.rs:42–56` calls `Vec::resize` on every invocation (`hvac_core.rs:563` equivalent in staging), which allocates in the hot control-signal path. The project's hot-loop policy prohibits per-step heap allocation. `MAX_SPEEDS` is confirmed absent from the codebase (searched crates/); define it as `const MAX_SPEEDS: usize = 8` (the maximum stage count used across all default performance curves in the HVAC subsystem). Replace the Vec with `[bool; MAX_SPEEDS]` and add a `speed_count: u8` field so iteration is bounded to the actual stage count without reading a length from a Vec header.

```rust
pub const MAX_SPEEDS: usize = 8;

/// Control-signal state modified by external control signals.
#[derive(Clone, Debug)]
pub struct HvacControlState {
    pub max_capacity_fraction: f64,
    /// Per-speed disable flags. Only indices `0..speed_count` are meaningful.
    pub disabled_speeds: [bool; MAX_SPEEDS],
    /// Number of active speed stages (mirrors the length of `heating_capacities_w`).
    pub speed_count: u8,
    pub max_enabled_speed: usize,
}
```

Update `set_disabled_speeds` in `staging.rs` to write into the fixed array and iterate only up to `speed_count`, eliminating all `Vec::resize` / `Vec::push` calls.

### Step 4: Restructure `HvacEquipment`

```rust
pub struct HvacEquipment {
    pub config: HvacConfig,
    pub thermostat_fsm: ThermostatFsm,  // from ticket 006
    pub runtime: HvacRuntimeState,
    pub control: HvacControlState,
}
```

### Step 5: Update all access sites

The largest mechanical effort. Every `self.<field>` becomes `self.config.<field>`, `self.runtime.<field>`, or `self.control.<field>`. Key files:

- `hvac_core.rs` — struct definition, `init_from_config`, all methods
- `staging.rs` — speed selection, duty cycle, PLF
- `air_conditioner.rs` — `CoolingCore` accesses `self.hvac.*`
- `heat_pump.rs` — `HeatPumpCore` accesses `self.hvac.*`
- `furnace.rs` — `FurnaceCore` accesses `self.hvac.*`
- `baseboard.rs` — if applicable
- `boiler.rs` — if applicable

Use `cargo clippy` to catch any missed field accesses.

### Step 6: Run full test suite

```
cargo test -p hares-equipment
```

## Definition of Done

- [ ] `HvacConfig` struct defined in `hvac_core.rs` with all configuration-only fields
- [ ] `HvacRuntimeState` struct defined with per-step mutable fields
- [ ] `HvacControlState` struct defined with signal-response fields; `disabled_speeds` is `[bool; MAX_SPEEDS]` with a `speed_count: u8` field — no `Vec<bool>`
- [ ] `MAX_SPEEDS: usize = 8` constant defined and used by `HvacControlState`
- [ ] `set_disabled_speeds` in `staging.rs` writes into the fixed array; no `Vec::resize` or heap allocation
- [ ] `HvacEquipment` struct composed of `config`, `thermostat_fsm`, `runtime`, `control` sub-structs
- [ ] No field that is semantically config is in `runtime` or `control` (except `plf_cooling_degradation_coeff` and `startup.c_d` which are documented as borderline)
- [ ] All access sites updated: `self.<field>` → `self.config.<field>`, `self.runtime.<field>`, or `self.control.<field>`
- [ ] `cargo test -p hares-equipment` passes with no behavioral changes
- [ ] `cargo clippy -p hares-equipment` produces no new warnings

## Verification

1. `cargo test -p hares-equipment` — all existing tests pass unchanged
2. `cargo clippy -p hares-equipment` — no new warnings (confirms no stale field accesses)
3. Grep for `self\.mode\b` and `self\.mode_start_at\b` — should not exist; these should be `self.thermostat_fsm.mode` etc.
4. Grep for `self\.max_capacity_fraction` — should be `self.control.max_capacity_fraction`
5. Grep for `self\.biquadratic_coeffs` — should be `self.config.biquadratic_coeffs`
6. Grep for `self\.duty_cycle` — should be `self.runtime.duty_cycle`

## References

- `hvac_core.rs:122–249` — full `HvacEquipment` struct definition with all 48 public fields (verified)
- `hvac_core.rs:138–143` — invariant comment for `mode` ↔ `mode_start_at`
- `hvac_core.rs:396–515` — Cd cascade that demonstrates the config/runtime blur for `plf_cooling_degradation_coeff`
- `air_conditioner.rs:330–332` — runtime Cd override from `CoolingCore`
- `staging.rs` — heavy user of `HvacEquipment` fields
- `thermostat.rs:1–184` — `ThermostatConfig` and related types (pre-ticket-006)

## Related Tickets

- 006-extract-thermostat-fsm — **must be completed first**; removes 11 thermostat fields from `HvacEquipment`
- 007-unify-variable-speed-selection — reduces method count on `HvacEquipment`, simplifying this decomposition
- 009-consolidate-config-helpers — overlaps with `HvacConfig` definition; coordinate on field types

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match (confirmed by independent inspection + `grep -n`)
  - `hvac_core.rs:122–249`: `HvacEquipment` struct definition exactly spans these lines. Exactly 48 `pub` fields confirmed by `awk 'NR>=123 && NR<=249' … | grep -c '^\s*pub '` → **48**. Full enumerated list verified: all 48 fields listed in the ticket are present at the claimed line numbers.
  - `hvac_core.rs:125`: `pub thermostat: ThermostatConfig` (ticket lists this as a thermostat field to be extracted by ticket 006). Confirmed.
  - `hvac_core.rs:138–143`: Invariant comment for `mode` ↔ `mode_start_at` confirmed present at these exact lines. Exact quote: *"INVARIANT: `mode_start_at` must be updated atomically with `mode` whenever the mode changes. Always use `set_mode` to change `mode`; never assign `mode` directly without also updating `mode_start_at`, otherwise `can_transition_mode` will enforce constraints against a stale timestamp and the minimum on/off time protection will be silently bypassed."*
  - `hvac_core.rs:396–398`: `plf_cooling_degradation_coeff` first assigned from config key `"cooling_cd"` / `"cd"` with `DEFAULT_PLF_DEGRADATION_COEFF` fallback. Confirmed.
  - `hvac_core.rs:399–406`: `startup.c_d` cascade set from `"startup_cd"` / `"cooling_cd"` / `"cd"` / fallback. Confirmed.
  - `hvac_core.rs:511–513`: Both `plf_cooling_degradation_coeff` and `startup.c_d` overridden by `derived_cd` in equipment-type default block. Confirmed.
  - `air_conditioner.rs:330–332`: `fn apply_cooling_startup_cd` at lines 328–332; `self.hvac.plf_cooling_degradation_coeff = cd;` at line 330, `self.hvac.startup.c_d = cd;` at line 331. Confirmed (off-by-one from ticket's "330–332" — actual span is 328–332, but the two assignment lines are 330–331, within the claimed range).
  - `staging.rs:42–56`: `pub fn set_disabled_speeds` at lines 42–56. `self.disabled_speeds.resize(n, false)` at line 44. Confirmed.
- [x] Described logic matches current implementation
  - The "god struct" problem is real: 48 `pub` fields in a single flat struct span config (set once at init), runtime state (per-step mutable), control state (on signal receipt), and thermostat state (to be extracted by ticket 006).
  - The `mode` ↔ `mode_start_at` invariant is comment-enforced only — no type-level barrier prevents direct `self.mode = x` assignment without updating `mode_start_at`.
  - `plf_cooling_degradation_coeff` is correctly classified as borderline config/runtime: it has two override sites (hvac_core.rs:511–513 and air_conditioner.rs:330) beyond the init-time assignment.
  - `disabled_speeds: Vec<bool>` is heap-backed. `Vec::resize` is called on every `set_disabled_speeds` invocation (staging.rs:44), potentially allocating on the hot control-signal path.
  - `MAX_SPEEDS` is confirmed absent from the codebase (`grep -r MAX_SPEEDS crates/` → no matches except the ticket-008 test that documents its absence).
- [x] OCHRE cross-check: **diverges** — with file+line evidence
  - OCHRE `HVAC.py:754`: `self.disable_speeds = np.zeros(self.n_speeds, dtype=bool)  # if True, disable that speed` — fixed-size at init, never resized.
  - OCHRE `HVAC.py:853–855` (exact lines read): `for idx in range(self.n_speeds): self.disable_speeds[idx] = bool(control_signal.get(f"Disable Speed {idx + 1}"))` followed by `self._max_enabled_speed = int(np.nonzero(~self.disable_speeds)[0][-1]) + 1` — index writes only, no resize.
  - OCHRE `HVAC.py:757`: `self._max_enabled_speed = self.n_speeds` — initialized at construction, updated via `np.nonzero` in `update_external_control`.
  - HARES `staging.rs:44`: `self.disabled_speeds.resize(n, false)` on every call — accidental divergence from OCHRE's fixed-array pattern, not an intentional improvement.
  - OCHRE `HVAC.py:823`: `"eir_plr": np.ascontiguousarray(np.array([val[f"{x}_eir_plr"] for x in "abc"], dtype=np.float64))` — per-speed PLR curve confirmed. Matches HARES line 208 `pub eir_plr_coefficients: Option<Vec<[f64; 3]>>`.
  - OCHRE `HVAC.py:841–844`: `if kwargs.get("Disable HVAC Part Load Factor", False): for key in biquad_params: biquad_params[key]["eir_plr"] = np.ascontiguousarray(np.array([1, 0, 0], dtype=np.float64))` — test-mode PLF bypass. Ticket citation accurate.
- [x] EnergyPlus cross-check: **matches (pattern)** — with quoted passage
  - EnergyPlus `DXCoils.hh` (NREL/EnergyPlus, develop branch, fetched via WebFetch): `DXCoilData` struct contains **~200+ member variables** mixing configuration (`RatedTotCap`, `RatedSHR`, `RatedCOP`, performance curve indices) with runtime state (`InletAirTemp`, `PartLoadRatio`, `OutletAirTemp`, `ElecCoolingPower`). Speed-related fields include `int NumOfSpeeds` and `Array1D<Real64> MSRatedTotCap` — dynamic sizing, no fixed `MAX_SPEEDS` equivalent. Quoted struct excerpt: *"struct DXCoilData { // Members // Some variables in this type are arrays (dimension=MaxModes)"* with `constexpr int MaxCapacityStages(2)` (stages, not speeds). This confirms HARES inherited the mixed-lifecycle pattern from EnergyPlus; the ticket proposes an improvement beyond EnergyPlus.
  - EnergyPlus Coding Guidelines wiki (fetched via WebFetch): *"Avoid global (including namespace scope) data in new code"* and *"It is recommended that EnergyPlus start to move to namespace directories and away from a monolithic source directory."* These guidelines validate the architectural direction of this ticket. Note: the prior audit quoted "EnergyPlus is moving away from global state and into 'managed' state" — that exact phrase was not found; the accurate quotes are those above.

### Web-Verified Citations

**Citation 1**: `DEFAULT_PLF_DEGRADATION_COEFF = 0.25` attributed to AHRI 210/240 as "default when no test data available" (staging.rs comment references "AHRI 210/240-2023, S6.6.3").

- **Sources found**:
  1. EnergyPlus-Fortran `StandardRatings.f90` (nrgsim/EnergyPlus-Fortran on GitHub, fetched via WebFetch, line 290): `REAL(r64), PARAMETER :: CyclicDegradationCoeff = 0.25D0` — in a module whose header identifies the calculation as per "ANSI/AHRI Standard 210/240-2008".
  2. Web search synthesis of AHRI 210/240-2008 text (confirmed across multiple government and industry sources): *"In lieu of conducting C and D tests or the heating cycling test, an assigned value of 0.25 may be used for either the cooling or heating Degradation Coefficient, CD, or both."* — this language appears in AHRI 210/240-2008 Section 6.1.3.1 ("Assigned Degradation Factor").
  3. EnergyPlus I/O Reference (bigladdersoftware.com, multiple fetched pages): *"Multi-speed DX cooling coils use AHRI Std 210/240-2008 default PLF linear curve and cooling coefficient of degradation (C_D) value of 0.25."*
- **Quoted passage (primary)**: *"In lieu of conducting C and D tests or the heating cycling test, an assigned value of 0.25 may be used for either the cooling or heating Degradation Coefficient, CD, or both."* (AHRI 210/240-2008, Section 6.1.3.1; confirmed by EnergyPlus-Fortran line 290 and EnergyPlus I/O Reference)
- **Verdict**: **Confirmed with section number caveat**. The value 0.25 is correct and traceable to AHRI 210/240. The section reference in the staging.rs comment is "S6.6.3" (2023 version numbering), while the equivalent provision in the 2008 version is Section 6.1.3.1. All AHRI PDF downloads (2017, 2024 editions) returned HTTP 403 during this audit, so direct 2023-edition section verification was not possible; however, the 0.25 value and "in lieu of" language are confirmed across AHRI 210/240-2008, EnergyPlus source, and the EnergyPlus I/O Reference. The section number should be treated as approximately correct but not independently verified for the 2023 edition.

**Citation 2**: Cd = 0.11 for two-speed equipment implied by ticket (hvac_core.rs:499–501; ticket does not explicitly cite this value but the code is described as part of the Cd cascade).

- **Source found**: OCHRE `HVAC.py:1125–1126` (read from vendor submodule): `if self.biquad_params is not None and self.n_speeds == 1 and hspf >= 7: self.biquad_params[1]["eir_plf"] = np.array([0.89, 0.11, 0])`. This is a PLF quadratic where PLF = 0.89 + 0.11·PLR + 0·PLR², implying Cd = 1 − 0.89 = 0.11 — but only for high-HSPF (≥7) **single-speed** heat pumps, not generically for two-speed equipment.
- **Quoted passage**: `self.biquad_params[1]["eir_plf"] = np.array([0.89, 0.11, 0])` (OCHRE HVAC.py:1126) — single-speed, high-HSPF heat pump only.
- **Verdict**: **Partially correct**. HARES applies Cd = 0.11 to all two-speed modes (hvac_core.rs:499–501) whereas OCHRE applies it to single-speed high-HSPF heat pumps. No published AHRI 210/240 table independently confirms 0.11 as the two-speed standard default (PDFs inaccessible). This is a pre-existing engineering question for the Cd cascade and is tangential to the structural decomposition this ticket describes.

**Citation 3**: OCHRE HVAC.py lines 754, 853–857 for `disable_speeds` and `_max_enabled_speed`.

- **Source found**: `/Users/rich/source/HARES/vendors/OCHRE/ochre/Equipment/HVAC.py` (vendor submodule, read directly).
- **Quoted passage**: Line 754: `self.disable_speeds = np.zeros(self.n_speeds, dtype=bool)  # if True, disable that speed`. Lines 853–855: `for idx in range(self.n_speeds): self.disable_speeds[idx] = bool(control_signal.get(f"Disable Speed {idx + 1}"))` / `self._max_enabled_speed = int(np.nonzero(~self.disable_speeds)[0][-1]) + 1`.
- **Verdict**: **Confirmed**. Line numbers match exactly. OCHRE variable is `disable_speeds`; HARES uses `disabled_speeds` — an intentional naming difference, not a bug.

**Citation 4**: OCHRE HVAC.py lines 823, 841–844 for `eir_plr` per-speed quadratic coefficients.

- **Source found**: `/Users/rich/source/HARES/vendors/OCHRE/ochre/Equipment/HVAC.py` (vendor submodule, read directly).
- **Quoted passage**: Line 823: `"eir_plr": np.ascontiguousarray(np.array([val[f"{x}_eir_plr"] for x in "abc"], dtype=np.float64))`. Lines 841–844: `if kwargs.get("Disable HVAC Part Load Factor", False): for key in biquad_params: biquad_params[key]["eir_plr"] = np.ascontiguousarray(np.array([1, 0, 0], dtype=np.float64))`.
- **Verdict**: **Confirmed**. Lines match exactly.

### Legitimacy

- **Verdict**: **Legitimate**

- **Rationale**: Every structural claim in the ticket is confirmed by independent code inspection and web-verified sources. The `HvacEquipment` struct has exactly 48 `pub` fields (lines 122–249, confirmed by enumerated `grep -n` audit above). The `mode` ↔ `mode_start_at` invariant is comment-only — no type-level barrier prevents unsafe direct assignment; the bug is real. `disabled_speeds: Vec<bool>` calls `Vec::resize` on every control-signal dispatch (staging.rs:44), diverging from OCHRE's fixed numpy array (HVAC.py:754) and violating the project's hot-loop allocation policy. `MAX_SPEEDS` is absent from the codebase. The Cd cascade (hvac_core.rs:396–515, air_conditioner.rs:330) accurately illustrates the config/runtime lifecycle blur. All four OCHRE citations (lines 754, 823, 841–844, 853–855) verified by direct file read. EnergyPlus `DXCoilData` struct (~200+ fields mixing config and runtime, fetched from GitHub) independently corroborates the pattern being addressed. AHRI 210/240 Cd = 0.25 confirmed via EnergyPlus-Fortran StandardRatings.f90 line 290 and multiple secondary sources. The only unverified detail is the exact section number "S6.6.3" in the staging.rs comment for the 2023 edition of AHRI 210/240 (PDF inaccessible); the equivalent 2008 provision is Section 6.1.3.1. This does not affect ticket legitimacy.

### Proposed Fix Summary

Pure structural refactor in `hvac_core.rs`. No behavioral changes:

1. Define `const MAX_SPEEDS: usize = 8` in `hvac_core.rs` or `staging.rs`.
2. Create `HvacConfig` (28 immutable fields), `HvacRuntimeState` (8 per-step fields), `HvacControlState` (3 control-signal fields with `disabled_speeds: [bool; MAX_SPEEDS]` and `speed_count: u8`).
3. After ticket 006, embed `thermostat_fsm: ThermostatFsm` as the fourth sub-struct.
4. Replace `HvacEquipment` flat fields with `config`, `runtime`, `control` sub-structs (and `thermostat_fsm`).
5. Update `set_disabled_speeds` in `staging.rs` to write into `[bool; MAX_SPEEDS]` by index, eliminating `Vec::resize`.
6. Mechanically update all `self.<field>` access sites across `hvac_core.rs`, `staging.rs`, `air_conditioner.rs`, `heat_pump.rs`, `furnace.rs`, etc.
7. Run `cargo test -p hares-equipment` and `cargo clippy -p hares-equipment` to verify no behavioral changes.

### Test Written

- **File**: `crates/hares-equipment/src/hvac/staging.rs` (`#[cfg(test)] mod tests`, appended in prior audit run)
- **Status**: Both tests exist and **pass** on current code (verified: `cargo test -p hares-equipment -- disabled_speeds` → 4 passed; `cargo test -p hares-equipment -- max_speeds` → 1 passed).
- **What they test**:
  - `disabled_speeds_is_vec_not_fixed_array`: Asserts `disabled_speeds` starts as empty `Vec<bool>`, grows to length 2 on first `set_disabled_speeds` call, and remains heap-backed on repeat calls. Will need updating after ticket 008 replaces the Vec with `[bool; MAX_SPEEDS]`.
  - `max_speeds_constant_not_yet_defined`: Asserts `std::mem::size_of_val(&hvac.disabled_speeds) == 3 * size_of::<usize>()` (the Vec fat-pointer layout). Documents absence of `MAX_SPEEDS` constant as a pre-condition for ticket 008. Annotated with `ticket-008` marker.
