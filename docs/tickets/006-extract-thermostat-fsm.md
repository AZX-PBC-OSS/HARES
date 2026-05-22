# Extract Thermostat FSM from HvacEquipment and IdealHvac

**Severity**: High
**Priority**: P1
**Status**: Open
**Areas**: hares-equipment/hvac, hares-equipment/hvac/thermostat

## Problem

Thermostat FSM logic is fully duplicated between `HvacEquipment` and `IdealHvac`. Five methods (~150 lines total) are copy-pasted across both structs with only minor naming differences. A bug fix in one path may not propagate to the other. This is the single largest DRY violation in the HVAC subsystem.

## Current Behavior

The following methods are duplicated:

| Method | `HvacEquipment` (hvac_core.rs) | `IdealHvac` (ideal_hvac.rs) |
|---|---|---|
| `resolve_profile_setpoints` / `resolve_schedule_setpoints` | lines 629–652 | lines 204–227 |
| `set_mode` | lines 770–776 | lines 240–249 |
| `can_transition_mode` | lines 790–809 | lines 251–266 |
| `update_mode` | lines 654–733 | lines 268–364 |
| `apply_control_signal` (ThermalSetpoint + Delta branches) | lines 572–608 | lines 654–720 |

### Key differences between the two copies

1. **`resolve_*` naming**: `HvacEquipment` calls it `resolve_profile_setpoints` (hvac_core.rs:629); `IdealHvac` calls it `resolve_schedule_setpoints` (ideal_hvac.rs:204). The body is identical.

2. **`set_mode` extra side-effect**: `IdealHvac::set_mode` clears `ideal_capacity_w = 0.0` on Deadband entry (ideal_hvac.rs:245–247). `HvacEquipment::set_mode` has no such branch. This is an `IdealHvac`-specific concern.

3. **`update_mode` target tracking**: `IdealHvac::update_mode` maintains `current_target_c` before and after the mode decision (ideal_hvac.rs:275–281, 355–361). `HvacEquipment::update_mode` does not.

4. **`apply_control_signal`**: `IdealHvac` adds validation via `validate_runtime_override` (ideal_hvac.rs:668, 701), deadband update (ideal_hvac.rs:669–673), `ModeOverride` handling (ideal_hvac.rs:675–683), `IdealCapacityModeOverride` (ideal_hvac.rs:703–704), and `LoadFraction` (ideal_hvac.rs:706–715). `HvacEquipment` handles only `ThermalSetpoint`, `ThermalSetpointDelta`, and `MaxCapacityFraction` (hvac_core.rs:572–608).

5. **Shared state**: Both structs carry `mode`, `mode_start_at`, `last_mode_switch_at`, `thermostat: ThermostatConfig`, `static_setpoints`, `schedule_setpoints`, `runtime_setpoints`, `heating_setpoint_source`, `cooling_setpoint_source`, `min_on_time_s`, `min_off_time_s`.

## Required Behavior

All thermostat FSM logic must live in a single `ThermostatFsm` struct. Both `HvacEquipment` and `IdealHvac` embed `thermostat_fsm: ThermostatFsm` and delegate to it. No behavioral change — all existing tests must pass unchanged.

## Approach

### Step 1: Define `ThermostatFsm` in `thermostat.rs`

Add to `crates/hares-equipment/src/hvac/thermostat.rs`:

```rust
pub struct ThermostatFsm {
    pub mode: ThermostatMode,
    pub mode_start_at: Option<DateTime<FixedOffset>>,
    pub last_mode_switch_at: Option<DateTime<FixedOffset>>,
    pub thermostat: ThermostatConfig,
    pub static_setpoints: ThermalSetpoints,
    pub schedule_setpoints: Option<ScheduleSetpoints>,
    pub runtime_setpoints: Option<RuntimeSetpointOverride>,
    pub heating_setpoint_source: Option<ScheduleSource>,
    pub cooling_setpoint_source: Option<ScheduleSource>,
    pub min_on_time_s: f64,
    pub min_off_time_s: f64,
}
```

### Step 2: Move the five duplicated methods onto `ThermostatFsm`

Implement on `ThermostatFsm`:

- `resolve_profile_setpoints(&mut self, env: &EnvironmentState)` — unified name for the identical logic from hvac_core.rs:629–652 / ideal_hvac.rs:204–227
- `set_mode(&mut self, mode: ThermostatMode, when: DateTime<FixedOffset>)` — the base logic from hvac_core.rs:770–776 (no `ideal_capacity_w` clear; that stays in `IdealHvac` as a post-hook)
- `can_transition_mode(&self, proposed: ThermostatMode, now: DateTime<FixedOffset>) -> bool` — identical in both (hvac_core.rs:790–809 / ideal_hvac.rs:251–266)
- `update_mode(&mut self, env: &EnvironmentState) -> crate::Result<ThermostatMode>` — the core hysteresis + cycle-time logic (hvac_core.rs:654–733 / ideal_hvac.rs:268–364), but **without** the `IdealHvac`-specific `current_target_c` tracking. `IdealHvac` wraps the delegation and adds target tracking around it.
- `effective_setpoints(&self) -> ThermalSetpoints` — already exists in thermostat.rs as `ThermalSetpoints::with_schedule_override` + `with_control_override`; provide a convenience method.
- `apply_thermal_setpoint_signal(&mut self, signal: &ControlSignal)` — the `ThermalSetpoint` and `ThermalSetpointDelta` branches from hvac_core.rs:572–601. The `IdealHvac`-specific validation (`validate_runtime_override`) and deadband update remain in `IdealHvac` as a post-hook.

### Step 3: Embed `ThermostatFsm` in `HvacEquipment`

Replace the 11 scattered fields (mode, mode_start_at, last_mode_switch_at, thermostat, static_setpoints, schedule_setpoints, runtime_setpoints, heating_setpoint_source, cooling_setpoint_source, min_on_time_s, min_off_time_s — confirmed 11 by inspection of hvac_core.rs:122–249) with `pub thermostat_fsm: ThermostatFsm`.

Update all call sites in `hvac_core.rs` and `staging.rs` to go through `self.thermostat_fsm.mode`, `self.thermostat_fsm.update_mode(env)`, etc. The access pattern `self.mode` becomes `self.thermostat_fsm.mode`; a convenience `Deref`/`DerefMut` is NOT recommended because it hides the boundary — explicit delegation keeps the FSM boundary visible.

### Step 4: Embed `ThermostatFsm` in `IdealHvac`

Same field replacement. `IdealHvac` retains its extra fields (`ideal_capacity_w`, `current_target_c`, etc.) and adds wrapper methods:

```rust
fn set_mode(&mut self, mode: ThermostatMode, when: DateTime<FixedOffset>) {
    let prev = self.thermostat_fsm.mode;
    self.thermostat_fsm.set_mode(mode, when);
    if prev != mode && mode == ThermostatMode::Deadband {
        self.ideal_capacity_w = 0.0;
    }
}
```

`update_mode` wrapper updates `current_target_c` before/after delegating.

`apply_control_unchecked` delegates `ThermalSetpoint` / `ThermalSetpointDelta` to the FSM, then applies `validate_runtime_override` and deadband update.

### Step 5: Update `mod.rs` re-exports

Add `ThermostatFsm` to the `pub use thermostat::...` line in `mod.rs:33–35`.

### Step 6: Run full test suite

```
cargo test -p hares-equipment
```

All existing tests must pass with zero behavioral change.

## Definition of Done

- [ ] `ThermostatFsm` struct defined in `thermostat.rs` with all 5 previously-duplicated methods
- [ ] `HvacEquipment` embeds `ThermostatFsm` instead of the 11 individual fields (confirmed count: hvac_core.rs:122–249)
- [ ] `IdealHvac` embeds `ThermostatFsm` instead of the 11 individual fields (same 11 fields, confirmed)
- [ ] No method named `resolve_profile_setpoints` or `resolve_schedule_setpoints` exists on `HvacEquipment` or `IdealHvac` — only `ThermostatFsm::resolve_profile_setpoints`
- [ ] `IdealHvac`-specific side-effects (`ideal_capacity_w` clear, `current_target_c` update, `validate_runtime_override`) remain in `IdealHvac` as wrapper logic around FSM delegation
- [ ] `ThermostatFsm` is re-exported from `mod.rs`
- [ ] `cargo test -p hares-equipment` passes with no behavioral changes
- [ ] `cargo clippy -p hares-equipment` produces no new warnings

## Verification

1. `cargo test -p hares-equipment` — all existing tests pass unchanged
2. `cargo clippy -p hares-equipment` — no new warnings
3. Grep for `resolve_profile_setpoints` and `resolve_schedule_setpoints` — only found on `ThermostatFsm`
4. Grep for direct assignment to `.mode = ` outside of `ThermostatFsm::set_mode` — should only be in init/constructors, never in control flow
5. Confirm the `IdealHvac::set_mode` wrapper still clears `ideal_capacity_w` on Deadband entry

## References

- `hvac_core.rs:629–809` — `HvacEquipment` thermostat FSM methods
- `ideal_hvac.rs:204–364` — `IdealHvac` thermostat FSM methods
- `thermostat.rs:1–184` — existing `ThermostatConfig`, `ThermostatMode`, `is_cycle_change_allowed`, `lookup_zone_temp`
- `mod.rs:33–35` — current re-exports from `thermostat` module

## Related Tickets

- 008-decompose-hvacequipment-struct — depends on this ticket; the struct decomposition is easier once thermostat fields are already grouped into `ThermostatFsm`

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-20

### Code Confirmation

- [x] Referenced line numbers still match (verified against current source)
  - `hvac_core.rs`: `resolve_profile_setpoints` at lines 629–652 ✓; `update_mode` at lines 654–733 ✓; `set_mode` at lines 770–776 ✓; `can_transition_mode` at lines 790–809 ✓; `apply_control_signal` at lines 572–608 ✓
  - `ideal_hvac.rs`: `resolve_schedule_setpoints` at lines 204–227 ✓; `set_mode` at lines 240–249 ✓; `can_transition_mode` at lines 251–266 ✓; `update_mode` at lines 268–364 ✓; `apply_control_unchecked` at lines 654–720 ✓
  - `thermostat.rs`: file is 184 lines, contains all cited types ✓
  - `mod.rs:33–35`: re-exports from `thermostat` confirmed — does NOT yet include `ThermostatFsm` (as expected, it doesn't exist yet) ✓
  - `HvacEquipment` struct fields lines 122–249: confirmed 11 thermostat-related fields (`mode`, `mode_start_at`, `last_mode_switch_at`, `thermostat`, `static_setpoints`, `schedule_setpoints`, `runtime_setpoints`, `heating_setpoint_source`, `cooling_setpoint_source`, `min_on_time_s`, `min_off_time_s`) ✓
  - `IdealHvac` struct (lines 30–68): same 11 fields confirmed ✓

- [x] Described logic matches current implementation
  - `resolve_profile_setpoints` (hvac_core.rs:629) and `resolve_schedule_setpoints` (ideal_hvac.rs:204): bodies are byte-for-byte identical ✓
  - `can_transition_mode`: both implementations are identical — same `elapsed_s >= min_s` logic, same `mode_start_at` guard ✓
  - `set_mode`: `HvacEquipment::set_mode` (hvac_core.rs:770) does NOT clear `ideal_capacity_w`; `IdealHvac::set_mode` (ideal_hvac.rs:240) does clear it on Deadband entry ✓
  - `update_mode`: both contain identical hysteresis branches; `IdealHvac::update_mode` additionally maintains `current_target_c` before and after the mode decision (lines 275–281 and 355–361) ✓
  - `apply_control_signal` vs `apply_control_unchecked`: correctly described divergence — `IdealHvac` adds `validate_runtime_override`, deadband update, `ModeOverride`, `IdealCapacityModeOverride`, and `LoadFraction` handling ✓

- [x] OCHRE cross-check result: **partially matches, with one unverified claim**
  - OCHRE `Equipment.py` (lines 95–99 of vendored copy): `min_time_in_mode` is read from `"{end_use} Minimum On Time"` and `"{end_use} Minimum Off Time"` kwargs, defaulting to **0 minutes** for HVAC. OCHRE's two-speed HVAC (`DynamicHVAC`, HVAC.py line 759–761) uses a `min_time_in_speed` of **5 minutes** per speed stage — not 120 s / 180 s.
  - **The HARES comments "OCHRE reference: 120 s for heat pump heating/cooling" and "180 s for heat pump off-cycle"** (hvac_core.rs:147,151 and core_config.rs:458) are **not confirmed by the vendored OCHRE source**. The OCHRE base class defaults are 0 minutes; the two-speed variant uses 5-minute speed locks (not on/off cycle times). These specific values do not appear anywhere in the vendored `ochre/` directory or in any HARES config or test fixtures. The 120 s / 180 s values may originate from ecobee/ASHRAE field guidance (web search confirmed they appear in commercial thermostat specs, e.g., Trane Symbio uses 5 min on / 3 min off, and ecobee defaults to 5 min) rather than OCHRE. **This comment is misleading but does not affect correctness** — `min_on_time_s` and `min_off_time_s` both default to 0.0 in HARES, meaning the feature is off unless explicitly configured, which matches OCHRE's default behaviour.
  - OCHRE `run_thermostat_control` (HVAC.py lines 392–410): uses `deadband_offset=0.2` (confirmed line 222), and the asymmetric threshold formula `temp_turn_on = setpoint − hvac_mult × deadband × (1 − offset)` matches HARES exactly ✓.
  - OCHRE mode management is simpler: uses string modes (`'On'`, `'Off'`) and `time_in_mode` dictionary in `Equipment.py:227–242`. HARES extracted the same concept into a typed `ThermostatMode` enum and added `mode_start_at` for timestamp-precise compressor protection. This is an intentional improvement over OCHRE, not a regression.

- [x] EnergyPlus cross-check result: **N/A for this ticket**
  - This ticket is a pure code-structure refactoring (DRY violation). The thermostat FSM logic itself (hysteresis, deadband, min-cycle debounce) is not derived from EnergyPlus. EnergyPlus uses a load-based zone predictor-corrector approach (zone temperature load calculation → zone demand), not an explicit heating/cooling/deadband state machine of this form. The state-machine pattern with `deadband_offset` originates from OCHRE (confirmed above). No EnergyPlus citations are made in this ticket, and none are needed.
  - Source consulted: [EnergyPlus 9.2 Engineering Reference — Zone Controls](https://bigladdersoftware.com/epx/docs/9-2/engineering-reference/zone-controls.html): *"Predictor-Corrector evaluates the active heating and/or cooling setpoints, determines if the zone requires heating or cooling or is in the deadband, and then passes this single load to the equipment."* This confirms EnergyPlus uses load-based mode determination, not the deadband-offset hysteresis FSM that HARES/OCHRE implement.

### Web-Verified Citations

This ticket contains **no explicit standards citations** (no ASHRAE, NFRC, DOE, ISO, or EnergyPlus section numbers are cited). The only references are to code locations and OCHRE, which were cross-checked against the vendored OCHRE source above.

Implicit claim audited:

- **Citation**: hvac_core.rs:147 comment "OCHRE reference: 120 s for heat pump heating/cooling" / "180 s for heat pump off-cycle"
- **Source found**: Vendored `vendors/OCHRE/ochre/Equipment/Equipment.py:96–99` and `HVAC.py:759–761`; supplementary web search for OCHRE GitHub (github.com/NREL/OCHRE)
- **Quoted passage**:
  - `Equipment.py:96–99`: `on_time = kwargs.get(self.end_use + " Minimum On Time", 0)` / `off_time = kwargs.get(self.end_use + " Minimum Off Time", 0)` — defaults are **0 minutes** for HVAC
  - `HVAC.py:759–761` (DynamicHVAC): `min_time_in_low = kwargs.get("Minimum Low Time (minutes)", 5)` — this is a **speed-lock** (5 min per speed), not an on/off cycle time
- **Verdict**: **Incorrect** — the 120 s / 180 s values do not appear in OCHRE. The comment is a documentation error. Functionally harmless since `min_on_time_s` defaults to 0.0 (disabled) in HARES, matching OCHRE's actual default.

### Legitimacy

- **Verdict**: **Legitimate**
- **Rationale**: The DRY violation described in the ticket is real and confirmed by direct code inspection. The five methods are present in both `HvacEquipment` (hvac_core.rs) and `IdealHvac` (ideal_hvac.rs) with the exact line ranges cited, and the bodies of `resolve_*_setpoints` and `can_transition_mode` are identical character-for-character. The three documented differences (IdealHvac-specific `ideal_capacity_w` clear, `current_target_c` tracking, and extra `apply_control_unchecked` signals) are accurately described and understood. The proposed extraction into `ThermostatFsm` is structurally sound: the common logic moves to the new struct, and `IdealHvac` retains its extra behaviour as wrapper hooks. The one inaccurate detail is the "OCHRE reference: 120 s / 180 s" comment, which does not reflect the vendored OCHRE source — but this is a documentation issue in an existing code comment (not introduced by this ticket), and it does not affect the correctness or legitimacy of the refactoring plan.

### Proposed Fix Summary

1. Define `ThermostatFsm` in `thermostat.rs` holding the 11 shared fields and implementing the five unified methods.
2. In `HvacEquipment`: replace the 11 individual fields with `pub thermostat_fsm: ThermostatFsm`; update all call sites in `hvac_core.rs` and `staging.rs` to use `self.thermostat_fsm.*`.
3. In `IdealHvac`: same field replacement; add thin wrapper methods for `set_mode` (adds `ideal_capacity_w` clear on Deadband) and `update_mode` (adds `current_target_c` tracking before/after delegation). `apply_control_unchecked` delegates the two shared signal branches to the FSM.
4. Re-export `ThermostatFsm` from `mod.rs:33–35`.
5. Fix the misleading OCHRE comment at hvac_core.rs:147,151 — change to "configurable; default 0.0 (disabled)" without citing a specific OCHRE reference value that does not exist.

**Do NOT implement the fix.** See Definition of Done in ticket body.

### Test Written

- **File**: `crates/hares-equipment/tests/hvac_tests.rs` (appended at end of file)
- **Tests added** (all passing; `cargo test -p hares-equipment fsm_`):
  - `fsm_both_paths_heat_on_below_turn_on_threshold`: verifies both `HvacEquipment` and `IdealHvac` enter Heating at 19.0°C given a 20°C setpoint with default `deadband_offset=0.2` and `hysteresis=1.0°C` (turn-on threshold = 19.2°C).
  - `fsm_both_paths_stay_off_in_deadband`: verifies both paths report Off at 22°C, squarely inside the heating/cooling deadband.
  - `fsm_both_paths_respect_min_cycle_time_lockout`: documents the baseline no-lockout behaviour (min_cycle_time_s=0.0 default) for both paths, establishing a reference that would detect divergence if one path's default changed without the other.
