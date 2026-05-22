# Speed/Startup Internal State Telemetry Gaps

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-equipment/hvac, hares-types

## Problem

Several important internal state values are computed but never exposed to telemetry. These values are essential for validating HVAC performance and debugging speed/staging behavior, but they are invisible to both the output system and external observers:

| Value | Computed In | Written to Telemetry | Used Internally |
|---|---|---|---|
| `speed_frac` | `staging.rs:86-87,117` | ❌ No | Interpolation weight between speed stages |
| `part_load_ratio` (PLR) | `staging.rs:83-87,109,224-226` | ❌ No | Cycling fraction at lowest speed |
| `part_load_factor` (PLF) | `staging.rs:268-312` | ❌ No (stored in `plf_state`) | EIR degradation correction |
| `startup_multiplier` | `speed_control.rs:71-97` | ❌ No | Capacity ramp on compressor restart |
| `duty_cycle` | `hvac_core.rs` (field) | ❌ No | Thermostat on/off fraction |
| `time_at_current_speed_s` | `staging.rs:66,96,131,182` | ❌ No | Minimum time-per-speed guard |
| `mode_duration_s` | computed from `mode_start_at` | ❌ No | Minimum on/off time enforcement |

## Current Behavior

### `speed_frac` — never written

The `SpeedSelection` struct at `speed_control.rs:100-110` computes `speed_frac` (interpolation weight) for every speed selection. It is stored in `hvac.last_speed_frac` at `staging.rs:117` but never written to telemetry. The AC checkpoint state saves it at `air_conditioner.rs:117` (`last_speed_frac`), confirming it's important enough to persist across checkpoints.

### `part_load_ratio` — never written

PLR is the cycling fraction computed in `select_speed_with_zone_temp()` and returned in `SpeedSelection::part_load_ratio`. It determines whether the unit is cycling at the lowest speed (PLR < 1) or running continuously (PLR = 1). The value is used internally in `calculate_performance()` at `air_conditioner.rs:1095-1109` but never exposed.

### `part_load_factor` — stored internally, not exposed

`plf_state` is set at `staging.rs:278,310` inside `part_load_factor_for_stage()`. This value directly affects EIR (EIR is divided by PLF at `air_conditioner.rs:1117-1121`). The staging module does log a `tracing::warn!` at line 301 when PLF is below floor, but the actual PLF value is not available as telemetry.

### `startup_multiplier` — computed and applied, never exposed

`StartupConfig::capacity_multiplier()` at `speed_control.rs:71-97` computes the Winkler exponential ramp multiplier. The result is used to degrade capacity in `apply_startup_capacity_degradation()` at `staging.rs:320-322`. The multiplier value and `time_since_start_min` are stored in `hvac.startup` but never written to telemetry. The AC checkpoint saves them at `air_conditioner.rs:113-114`, confirming they're important.

### `duty_cycle` — HvacEquipment field, never exposed

`hvac.duty_cycle` is set in `update_control()` (e.g., `air_conditioner.rs:716-727`) based on speed selection PLR. It's the thermostat's raw on/off fraction. Not written to telemetry.

### `time_at_current_speed_s` — stored, not exposed

Advanced by `advance_speed_timer()` at `staging.rs:66`, reset at lines 96, 131, 182. Used as a guard in `select_two_speed_setpoint()` and `select_two_speed_time()`. Saved in AC checkpoint state at `air_conditioner.rs:137` but not in telemetry.

### `mode_duration_s` — not even computed, but `mode_start_at` is stored

`hvac.mode_start_at` is set in `set_mode()` at `hvac_core.rs:774`. The duration is computed on-the-fly in `can_transition_mode()` at line 801 but not persisted. To expose `mode_duration_s`, compute `(now - mode_start_at)` during step and write it.

## Required Behavior

All 7 internal values must be available as telemetry keys so they can be:
1. Inspected at runtime via the observer
2. Written to output columns at high verbosity
3. Used for post-hoc analysis and validation against OCHRE

## Approach

### Step 1: Add new telemetry key constants in `telemetry_keys.rs`

```rust
// ── HVAC speed/staging ─────────────────────────────────────────────────────
pub const SPEED_FRAC: &str = "speed_frac";
pub const PART_LOAD_RATIO: &str = "part_load_ratio";
pub const PART_LOAD_FACTOR: &str = "part_load_factor";
pub const STARTUP_MULTIPLIER: &str = "startup_multiplier";
pub const DUTY_CYCLE: &str = "duty_cycle";
pub const TIME_AT_CURRENT_SPEED_S: &str = "time_at_current_speed_s";
pub const MODE_DURATION_S: &str = "mode_duration_s";
```

### Step 2: Add `TelemetryField` descriptors

In the AC/furnace telemetry_fields() functions, add descriptors for each new key with appropriate units and descriptions.

### Step 3: Write telemetry in `step()` methods

For each HVAC equipment that uses `HvacEquipment`, add `telemetry.set()` calls at the end of `step()`:

- `speed_frac`: `self.hvac.last_speed_frac` — available from `staging.rs:117`
- `part_load_ratio` (`PART_LOAD_RATIO_W`): `SpeedSelection::part_load_ratio` from speed selection, written immediately after speed selection (start of step)
- `part_load_factor`: `self.hvac.plf_state` — set by `part_load_factor_for_stage()` at `staging.rs:278,310`
- `startup_multiplier`: call `self.hvac.startup.capacity_multiplier()` directly — `capacity_multiplier()` at `speed_control.rs:71` is a simple exponential; calling it twice per step is negligible, so no caching field is needed
- `duty_cycle` (`DUTY_CYCLE`): `self.hvac.duty_cycle` — written after `update_control()` (end of step)
- `time_at_current_speed_s`: `self.hvac.time_at_current_speed_s`
- `mode_duration_s`: `(env.current_time - self.hvac.mode_start_at.unwrap_or(env.current_time)).num_milliseconds() as f64 / 1000.0`

**Timing invariant**: `PART_LOAD_RATIO_W` is written after speed selection (start of step); `DUTY_CYCLE` is written after `update_control()` (end of step). For single-speed equipment these two values must be equal at the end of every step. Any divergence between them indicates a step-ordering bug and must be treated as a defect.

### Step 4: Add keys to default telemetry initialization

In `ac_config.rs:default_telemetry()` and each equipment's `default_telemetry()` / `xxx_default_telemetry()`, add `telemetry.insert()` calls for the new keys with initial value 0.0.

### Step 5: (Removed) No startup multiplier cache needed

The proposed `last_startup_multiplier: f64` field on `HvacEquipment` is dropped. `capacity_multiplier()` at `speed_control.rs:71` is a simple exponential computation — calling it a second time per step for telemetry is negligible cost. Adding a cache field for it would increase struct surface area for no meaningful gain.

## Definition of Done

- [ ] `SPEED_FRAC` telemetry key constant defined
- [ ] `PART_LOAD_RATIO` telemetry key constant defined
- [ ] `PART_LOAD_FACTOR` telemetry key constant defined
- [ ] `STARTUP_MULTIPLIER` telemetry key constant defined
- [ ] `DUTY_CYCLE` telemetry key constant defined
- [ ] `TIME_AT_CURRENT_SPEED_S` telemetry key constant defined
- [ ] `MODE_DURATION_S` telemetry key constant defined
- [ ] All 7 keys written in `CoolingCore::step()`, `ElectricFurnace::step()`, `GasFurnace::step()`, and heat pump equipment steps
- [ ] All 7 keys initialized in default telemetry constructors
- [ ] `TelemetryField` descriptors added for all 7 keys
- [ ] Existing tests pass (new keys default to 0.0; no behavioral change)

## Verification

1. Run a multi-speed AC simulation and verify:
   - `speed_frac` varies between 0.0 and 1.0 during inter-speed interpolation
   - `part_load_ratio` < 1.0 when cycling at lowest speed
   - `part_load_factor` = `1 - Cd * (1 - PLR)` for single-speed cycling
2. Run a single-speed AC with `c_d = 0.25` from cold start and verify:
   - `startup_multiplier` ramps from ~0.0 to 1.0 over `t_full` minutes
   - `mode_duration_s` increments while in a mode, resets on mode change
3. Verify `duty_cycle` matches `part_load_ratio` for single-speed equipment (they should be equal).

## References

- `speed_control.rs:100-110`: `SpeedSelection` struct with `speed_frac`, `part_load_ratio`
- `staging.rs:69-119`: `select_speed_with_zone_temp()` — computes `speed_frac` and PLR
- `staging.rs:268-312`: `part_load_factor_for_stage()` — computes PLF, stores in `plf_state`
- `staging.rs:315-323`: `apply_startup_capacity_degradation()` — applies startup multiplier
- `speed_control.rs:71-97`: `StartupConfig::capacity_multiplier()` — Winkler ramp formula
- `hvac_core.rs:770-776`: `set_mode()` — records `mode_start_at`
- `hvac_core.rs:790-809`: `can_transition_mode()` — computes `elapsed_s` from `mode_start_at`
- `telemetry_keys.rs:1-164`: Existing telemetry key constants

## Ordering

**This ticket must ship before ticket 017's RTF column work.** Ticket 017's `{name} Runtime Fraction (-)` column reads from `RUNTIME_FRACTION` (already exists), but the PLR/PLF columns added in 017 read from the `PART_LOAD_RATIO` and `PART_LOAD_FACTOR` telemetry keys introduced here. Implementing 017 before 019 means those column entries would have no backing keys.

## Related Tickets

- #016 — Thermostat FSM decision tracing (tracing complements telemetry — tracing for real-time diagnostics, telemetry for persistent output)
- #017 — Missing output columns v7 (PLR/PLF column work in 017 depends on keys from this ticket; ship 019 first)
- #018 — CoreOutput HVAC promotion (some of these values like speed_index and COP may move to CoreOutput)

## Verification Audit

**Auditor**: claude-sonnet-4-6 (automated)
**Date**: 2026-05-21
**Supersedes**: prior audit dated 2026-05-20

### Code Confirmation

- [x] Referenced line numbers still match — with minor offsets noted below
- [x] Described logic matches current implementation (independently verified by direct file reads)
- [x] OCHRE cross-check result: **diverges intentionally** — OCHRE `Equipment/HVAC.py` `generate_results()` (lines 568–597, read directly from `vendors/OCHRE/`) exposes at verbosity ≥ 7: Delivered (W), Setpoint (C), COP (-), Duct Losses (W), Main Power (kW), Fan Power (kW), Latent Gains (W), SHR (-), **Speed (-)**, Capacity (W), Max Capacity (W). `startup_cap_mult`, `time_in_speed`, `speed_idx` (as PLR proxy), `plf`, `duty_cycle`, and `time_from_start` are all internal-only in OCHRE. HARES exposing all 7 is an intentional improvement.
- [x] EnergyPlus cross-check result: **matches for PLF formula and RTF**. EnergyPlus Engineering Reference (Coils, EnergyPlus 8.1 and 8.3, bigladdersoftware.com) states: "The runtime fraction of the coil is defined as PLR/PLF." The PLF curve example given is `PLF = 0.85 + 0.15(PLR)` (a linear special case). The HARES default `1 − Cd*(1 − PLR)` is the AHRI 210/240-2023 S6.6.3 form (cited verbatim in `staging.rs:17`) and is consistent with EnergyPlus's general PLF-curve framework.

#### Line number corrections (ticket vs. actual)

| Ticket reference | Actual location | Notes |
|---|---|---|
| `staging.rs:86-87,117` for `speed_frac` | `staging.rs:86,117` | ✓ Confirmed by direct read |
| `staging.rs:83-87,109,224-226` for PLR | `staging.rs:85,102,108` | Core PLR writes at lines 85, 102, 108; no code at 224-226 |
| `staging.rs:268-312` for PLF | `staging.rs:273-312` | Function body starts at 273; doc comment at 267-272 |
| `staging.rs:278,310` for `plf_state` write | `staging.rs:278,310` | ✓ Exact match |
| `speed_control.rs:71-97` for `capacity_multiplier` | `speed_control.rs:71-97` | ✓ Exact match |
| `speed_control.rs:100-110` for `SpeedSelection` | `speed_control.rs:100-110` | ✓ Exact match |
| `air_conditioner.rs:117` for `last_speed_frac` checkpoint | `air_conditioner.rs:117` | ✓ Exact match |
| `air_conditioner.rs:113-114` for startup checkpoint | `air_conditioner.rs:113-114` | ✓ Exact (`startup_c_d`, `startup_time_since_start_min`) |
| `air_conditioner.rs:137` for `time_at_current_speed_s` | `air_conditioner.rs:137` | ✓ Exact match |
| `air_conditioner.rs:1095-1109` for PLR derivation | `air_conditioner.rs:1095-1109` | ✓ Exact match |
| `air_conditioner.rs:1117-1121` for EIR/PLF division | `air_conditioner.rs:1117-1121` | ✓ Exact match |
| `air_conditioner.rs:716-727` for duty_cycle | `air_conditioner.rs:716-727` | ✓ Exact match |
| `staging.rs:320-322` for startup degradation | `staging.rs:320-322` | ✓ Exact (function at 315-323) |
| `hvac_core.rs:770-776` for `set_mode` | `hvac_core.rs:770-776` | ✓ Exact match (mode_start_at set at line 774) |
| `hvac_core.rs:790-809` for `can_transition_mode` | `hvac_core.rs:790-809` | ✓ Exact match |
| `staging.rs:66` for `advance_speed_timer` | `staging.rs:65-66` | ✓ Function starts at 65, body at 66 |

### Web-Verified Citations

#### Citation 1 — Winkler exponential startup ramp formula

- **Citation**: The ticket names `StartupConfig::capacity_multiplier()` a "Winkler exponential ramp" with formula `(-1.025 * exp(-3.79936 * t/t_full) + 1.025)` and `t_full = 20 * c_d + 0.4`. The prior audit stated OCHRE attributes this to "Jon Winkler's thesis (2013)".

- **Source found (OCHRE)**: `vendors/OCHRE/ochre/Equipment/HVAC.py` lines 971–988, and `vendors/OCHRE/ochre/utils/equipment.py` lines 473–474, read directly from the submodule.

- **Quoted passage (HVAC.py:971–988)**:
  ```python
  def calc_startup_capacity_degredation(self):
      if self.c_d == 0.0:
          return 1.0
      else:
          t_full = 20.0 * self.c_d + 0.4  ## time to full capacity, in minutes
          time_full_cap = dt.timedelta(minutes=t_full)
          if "HP" in self.mode:
              if "HP" not in self.mode_prev:
                  self.time_from_start = 0.5 * self.time_res
              if self.time_from_start > time_full_cap:
                  return 1.0
              else:
                  exp_term = -3.79936 * (self.time_from_start / time_full_cap)
                  capacity_mult = max(0, min(1.0, -1.025 * math.exp(exp_term) + 1.025))
                  self.time_from_start += self.time_res
                  return capacity_mult
          else:
              return 1.0
  ```

- **Quoted passage (utils/equipment.py:471–474)**:
  ```python
  # Calculate coefficient of degredation (c_d) of equipment based on equipment type and EER/SEER/HSPF
  # Should only affect cases with single speed and two speed compressor driven equipment (ASHP/AC)
  # Capacity losses based on Jon Winkler's thesis and match what's in E+ with "Advanced Research Features for startup losses"
  # https://drum.lib.umd.edu/bitstream/handle/1903/9493/Winkler_umd_0117E_10504.pdf?sequence=1&isAllowed=y page 200
  ```

- **Thesis verification**: The DRUM repository at `https://drum.lib.umd.edu/handle/1903/9493` was fetched directly. The thesis title is **"Development of a Component Based Simulation Tool for the Steady State and Transient Analysis of Vapor Compression Systems"**, author **Jonathan M. Winkler**, year **2009** (not 2013 as stated in the prior audit — the OCHRE comment contains no year; the "2013" cited in the prior audit was an error).

- **HARES implementation** (`speed_control.rs:82,95`):
  ```rust
  let t_full = 20.0 * self.c_d + 0.4;
  (-1.025_f64 * (-3.799_36_f64 * t / t_full).exp() + 1.025).clamp(0.0, 1.0)
  ```

- **Verdict**: **Confirmed** — formula and constants match exactly. **Correction to prior audit**: The Winkler thesis is from **2009**, not 2013. The OCHRE source code contains no year; the DRUM repository confirms the 2009 date. The formula origin is the Winkler (2009) UMD doctoral dissertation, page 200.

#### Citation 2 — PLF formula `1 - Cd * (1 - PLR)` for single-speed cycling

- **Citation**: Ticket §Verification step 1: "verify `part_load_factor` = `1 - Cd * (1 - PLR)` for single-speed cycling."

- **Source found (code)**: `staging.rs:16-18` (module top, read directly):
  ```rust
  /// Default part-load factor degradation coefficient (Cd).
  /// AHRI Standard 210/240-2023, S6.6.3 default when no test data available.
  pub(super) const DEFAULT_PLF_DEGRADATION_COEFF: f64 = 0.25;
  ```
  And `staging.rs:296-297`:
  ```rust
  let cd = self.plf_cooling_degradation_coeff.clamp(0.0, 1.0);
  1.0 - cd * (1.0 - plr)
  ```

- **Source found (EnergyPlus)**: EnergyPlus Engineering Reference, Coils chapter (8.1 at `https://bigladdersoftware.com/epx/docs/8-1/engineering-reference/page-078.html`, 8.3 at `https://bigladdersoftware.com/epx/docs/8-3/engineering-reference/coils.html`), fetched via WebFetch. Both versions confirm:
  - PLF is a curve with PLR as the independent variable (quadratic or cubic general form)
  - "The runtime fraction of the coil is defined as PLR/PLF."
  - Typical example: `PLF = 0.85 + 0.15(PLR)`
  - The specific `PLF = 1 − Cd*(1−PLR)` formula is **not spelled out** in these EnergyPlus pages; they use polynomial curve inputs instead. However, that form is a linear PLF curve with `a = 1−Cd` and `b = Cd`, which is the AHRI 210/240 standard form.

- **Source found (AHRI 210/240)**: Multiple web searches and the AHRI standards index confirm that AHRI 210/240-2023 defines a default Degradation Coefficient `CD = 0.25` for the cycling PLF calculation. A 2021 SI version (AHRI 211/241) was found stating "A default value of 0.25 shall be used for the cooling Degradation Coefficient, CD." The exact `PLF = 1 − CD*(1−PLR)` form appears in the standard's SEER calculation methodology (Section 6.6.3 in the 2023 edition, per the source-code citation in `staging.rs:17`). The PDF of AHRI 210/240-2026 returned HTTP 403; the 2017 edition also returned 403; the 2008 edition timed out — but the formula is independently corroborated by the HARES unit test `plf_ahri_210_240_default_cd` at `staging.rs:445-465`, which cites the standard and spot-checks correct values.

- **Existing unit test** (`staging.rs:445`):
  ```
  // AHRI 210/240 S6.6.3: PLF = 1 - Cd*(1-PLR), default Cd = 0.25.
  fn plf_ahri_210_240_default_cd() {
      // PLR=0.75 → PLF=0.9375, PLR=0.50 → PLF=0.875
  ```

- **Verdict**: **Confirmed**. The `1 − Cd*(1−PLR)` formula is the AHRI 210/240-2023 S6.6.3 standard form. EnergyPlus uses this implicitly via PLF curve inputs (the linear case matches exactly). The formula is correctly implemented in HARES and is independently tested. **Correction to prior audit**: the "ACEEE 2004 paper" cited in the previous audit as confirming this formula was not independently verified by web fetch; the actual authoritative source is AHRI 210/240-2023 S6.6.3 as cited in the HARES source code itself.

#### Citation 3 — `mode_duration_s` computation from `mode_start_at`

- **Source found**: `hvac_core.rs:801` (read directly):
  ```rust
  let elapsed_s = (now - start).num_milliseconds().max(0) as f64 / 1000.0;
  ```
  The ticket's proposed expression `(env.current_time - self.hvac.mode_start_at.unwrap_or(env.current_time)).num_milliseconds() as f64 / 1000.0` is consistent with this pattern. Note the ticket omits `.max(0)` which the existing code has; implementors should add it to guard against clock skew.

- **Verdict**: **Confirmed**. The expression matches existing internal usage.

#### Citation 4 — OCHRE exposes `speed_idx` as `"{end_use} Speed (-)"` but not PLR/PLF/duty_cycle

- **Source found**: `vendors/OCHRE/ochre/Equipment/HVAC.py` lines 568–600 (read directly from submodule):
  ```python
  def generate_results(self):
      results = super().generate_results()
      if self.verbosity >= 7:
          results[f"{self.end_use} Main Power (kW)"] = main_power
          results[f"{self.end_use} Fan Power (kW)"] = self.fan_power / 1000
          results[f"{self.end_use} Latent Gains (W)"] = self.latent_gain * self.space_fraction
          results[f"{self.end_use} SHR (-)"] = self.shr if on or self.show_eir_shr else 0
          results[f"{self.end_use} Speed (-)"] = self.speed_idx
          results[f"{self.end_use} Capacity (W)"] = self.capacity
          results[f"{self.end_use} Max Capacity (W)"] = self.capacity_max
  ```
  `startup_cap_mult`, `time_in_speed`, `time_from_start`, `plf`/`plr` (as distinct outputs), `duty_cycle`, and `mode_start` are not present in any `generate_results()` override in HVAC.py.

- **Verdict**: **Confirmed**. All 7 ticket items are internal-only in OCHRE. The OCHRE `speed_idx` (which doubles as PLR in single/multi-speed selection) is the only diagnostic exposed at high verbosity.

#### Citation 5 — `duty_cycle` naming collision check

- **Observation**: The ticket states `duty_cycle` will use key `"duty_cycle"` in telemetry. Confirmed: no `DUTY_CYCLE` or `"duty_cycle"` string key exists in `telemetry_keys.rs` (165 lines read in full). The `ControlCapabilities::DUTY_CYCLE` bitflag in `control_signal.rs` is a different type in a different namespace. No collision.
- **Verdict**: **Confirmed**. No issue.

#### Citation 6 — `telemetry_keys.rs:1-164` as reference for existing constants

- **Source found**: `/Users/rich/source/HARES/crates/hares-types/src/telemetry_keys.rs` read in full.
- **Actual line count**: **165 lines** (the `tank_node_key` function closes at line 164 with `}`, and line 165 is the final `}`). The ticket's "1-164" reference is one line short but immaterially so.
- **Confirmed absent keys** (all 7 proposed): `speed_frac`, `part_load_ratio`, `part_load_factor`, `startup_multiplier`, `duty_cycle` (as string constant), `time_at_current_speed_s`, `mode_duration_s` — none present anywhere in the file.
- **Verdict**: **Confirmed**. All 7 proposed keys are genuinely absent.

### Legitimacy

- **Verdict**: **Legitimate**

- **Rationale**: All 7 internal values are confirmed computed and persisted across checkpoint save/restore (`AirConditionerState` includes `plf_state`, `last_speed_frac`, `startup_c_d`, `startup_time_since_start_min`, `time_at_current_speed_s`, `ctrl_duty_cycle`) yet none appear in the telemetry map (`telemetry_keys.rs` verified at 165 lines with no matching constants). The presence of these fields in checkpoints proves the maintainers already consider them load-bearing; their absence from telemetry is an oversight. The PLF formula `1 − Cd*(1−PLR)` is the AHRI 210/240-2023 S6.6.3 standard default, cited correctly in both `staging.rs:17` and the unit test at `staging.rs:445`. The startup exponential ramp `−1.025 × exp(−3.79936 × t/t_full)` with `t_full = 20*c_d + 0.4` is confirmed in OCHRE `HVAC.py:971–988` (read from submodule), attributable to the Winkler (2009) UMD doctoral dissertation (not 2013 as the prior audit stated — the OCHRE comment is undated and the DRUM repository gives 2009). EnergyPlus confirms `RTF = PLR/PLF` and the general PLF curve framework, which encompasses the AHRI linear form used here. OCHRE itself does not expose any of the 7 values as outputs (confirmed from HVAC.py source). The `mode_duration_s` expression is consistent with existing `can_transition_mode()` code at `hvac_core.rs:801`.

### Proposed Fix Summary

1. Add 7 string constants to `telemetry_keys.rs`: `SPEED_FRAC`, `PART_LOAD_RATIO`, `PART_LOAD_FACTOR`, `STARTUP_MULTIPLIER`, `DUTY_CYCLE` (telemetry key), `TIME_AT_CURRENT_SPEED_S`, `MODE_DURATION_S`.
2. Add `TelemetryField` descriptors for each key in AC/furnace/heat-pump `telemetry_fields()` functions.
3. In each equipment's `step()` call (CoolingCore, ElectricFurnace, GasFurnace, heat pump heater/cooler), write the 7 values with `telemetry.set(KEY, value)`.
4. Initialize all 7 keys to `0.0` in each equipment's `default_telemetry()` constructor.
5. For `startup_multiplier`: call `self.hvac.startup.capacity_multiplier()` a second time per step (no caching field needed — the function is a simple exponential).
6. For `mode_duration_s`: `(env.current_time - self.hvac.mode_start_at.unwrap_or(env.current_time)).num_milliseconds().max(0) as f64 / 1000.0` (add `.max(0)` which the ticket draft omits but `can_transition_mode()` uses).

Do NOT change production logic; this is additive telemetry exposure only.

### Test Written

- **File**: `crates/hares-equipment/tests/hvac_tests.rs` lines 2687–2853
- **Tests added** (previously inserted, not at end-of-file as the prior audit stated — ticket-020 tests follow at lines 2855–3108):
  - `ticket_019_speed_staging_keys_absent_from_telemetry` — `#[should_panic(expected = "ticket-019")]` test steps a single-speed AC with `startup_cd = 0.25` at hot zone temp and asserts all 7 keys are present in telemetry. Panics today (keys absent).
  - `ticket_019_single_speed_duty_cycle_equals_part_load_ratio` — `#[should_panic(expected = "ticket-019")]` test asserts `duty_cycle == part_load_ratio` for single-speed equipment (the ticket's stated invariant). Panics today (keys absent).
- **Verification**: `cargo test --test hvac_tests ticket_019` → **2 passed** (both panic as expected under `#[should_panic]`). Confirmed 2026-05-21.
