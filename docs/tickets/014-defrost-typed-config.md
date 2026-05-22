# Defrost Parameters Not Typed in Config

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-equipment/hvac/heat_pump, hares-equipment/hvac

## Problem

Defrost parameters are configured via raw key-value extraction at runtime, not typed struct fields. There is no compile-time validation, no serde deny_unknown_fields protection, and the defrost configuration is inconsistent with the typed approach used for other HP parameters (lockout temps, ER settings, duct config, etc.).

The `DefrostConfig` struct exists in `defrost.rs` but is never populated from the typed config path — it is initialized with hardcoded defaults in `heater.rs:379`:
```rust
defrost_config: DefrostConfig::on_demand(1.0, 0.0),
```

This means users cannot configure defrost strategy (reverse-cycle vs. resistive), defrost control mode (timed vs. on-demand), defrost time fraction, or the resistive defrost heater capacity through the typed config system.

## Current Behavior

1. **DefrostConfig is initialized with hardcoded defaults** — `heater.rs:379`:
   ```rust
   defrost_config: DefrostConfig::on_demand(1.0, 0.0),
   ```
   This creates an OnDemand, ReverseCycle defrost with `capacity_reduction_factor = 1.0` and `defrost_power_w = 0.0`. These values are never overridden from config.

2. **DefrostConfig struct exists but is not serde-compatible** — `defrost.rs:36-57`:
   ```rust
   #[derive(Clone, Copy, Debug)]
   pub struct DefrostConfig {
       pub capacity_reduction_factor: f64,
       pub defrost_power_w: f64,
       pub control: DefrostControl,
       pub strategy: DefrostStrategy,
       pub defrost_time_fraction: f64,
       pub max_oat_defrost_c: f64,
       pub defrost_eir_coeffs: Option<[f64; 6]>,
       pub resistive_defrost_capacity_w: f64,
   }
   ```
   The struct lacks `Serialize`/`Deserialize` derives and has no serde integration with the typed config system.

3. **HeatPumpHeaterConfig has no defrost fields** — `heat_pump_config.rs:20-139`: the typed config struct has fields for lockout temps, ER settings, biquadratic bounds, and duct config, but zero defrost fields.

4. **Defrost enums are serde-compatible** — `defrost.rs:16-33`: `DefrostControl` and `DefrostStrategy` already derive `Serialize, Deserialize`, so they can be used in a typed config struct.

5. **DuctConfig provides a precedent** — `heat_pump_config.rs:113-114`:
   ```rust
   #[serde(flatten)]
   pub duct: DuctConfig,
   ```
   The `DuctConfig` struct is flattened into the heater config via serde, providing a clean namespace. The same pattern should apply to defrost.

## Required Behavior

1. **`DefrostConfig` must be serde-compatible** and embeddable in `HeatPumpHeaterConfig` via `#[serde(flatten)]`.

2. **All defrost parameters must be configurable** through the typed config:
   - `defrost_control`: "OnDemand" | "Timed" (default: "OnDemand")
   - `defrost_strategy`: "ReverseCycle" | "Resistive" (default: "ReverseCycle")
   - `defrost_time_fraction`: fraction of hour in defrost for Timed mode (default: 0.058)
   - `defrost_max_oat_c`: maximum OAT for defrost activation (default: 4.4445)
   - `defrost_capacity_reduction_factor`: legacy scaling factor [0..1] (default: 1.0)
   - `defrost_power_w`: additional fixed defrost power [W] (default: 0.0)
   - `defrost_eir_coeffs`: optional biquadratic EIR curve for Timed+ReverseCycle
   - `resistive_defrost_capacity_w`: rated defrost heater capacity for Resistive strategy [W] (default: 0.0)

3. **Compile-time validation** — `#[serde(deny_unknown_fields)]` on the defrost struct catches typos like `defrost_startegy` that would silently be ignored in the raw key-value system.

4. **Backward compatibility** — all fields default to the current hardcoded values, so existing configs without defrost keys are unaffected.

## Approach

1. **Add `Serialize, Deserialize` derives to `DefrostConfig`** in `defrost.rs`:
   ```rust
   #[derive(Clone, Copy, Debug, Serialize, Deserialize)]
   pub struct DefrostConfig { ... }
   ```
   Do NOT add `#[serde(deny_unknown_fields)]` to `DefrostConfig` itself. Serde does not support combining `deny_unknown_fields` with `#[serde(flatten)]` on the inner struct — it silently breaks field recognition. Unknown-field rejection is the responsibility of the outer struct (`HeatPumpHeaterConfig` at `heat_pump_config.rs:19`), which already carries `#[serde(deny_unknown_fields)]`.

2. **Rename fields for serde naming convention** — use snake_case config keys matching the existing HARES convention. Add `#[serde(default)]` and `#[serde(skip_serializing_if = ...)]` as appropriate.

3. **Add `DefrostConfig` to `HeatPumpHeaterConfig`** via `#[serde(flatten)]`:
   ```rust
   #[serde(flatten)]
   pub defrost: DefrostConfig,
   ```
   Follow the same pattern as `DuctConfig` (`heat_pump_config.rs:113-114`). Verify that `DuctConfig` does not carry `deny_unknown_fields` — it does not, confirming that the outer `HeatPumpHeaterConfig` deny_unknown_fields is the correct and sole enforcement point.

4. **Wire `DefrostConfig` in `init_from_typed`** (`heater.rs:460-617`): read `cfg.defrost` and assign to `self.defrost_config` instead of using `DefrostConfig::on_demand(1.0, 0.0)`.

5. **Add validation** to `DefrostConfig`:
   - `capacity_reduction_factor` in [0.0, 1.0]
   - `defrost_time_fraction` in [0.0, 1.0] (only meaningful for Timed mode)
   - `max_oat_defrost_c` in [-30.0, 21.0]
   - `resistive_defrost_capacity_w >= 0.0`
   - If `strategy == Resistive` and `resistive_defrost_capacity_w == 0.0`, emit a warning

6. **Update `Default` impl for `HeatPumpHeaterConfig`** to include `defrost: DefrostConfig::default()`.

7. **Add `DefrostConfig` to `HeatPumpCoolerConfig`** (cooling side typically doesn't defrost, but the struct should accept and ignore the fields for config round-trip compatibility, or use `#[serde(skip)]`).

8. **Update tests** — add typed config test with explicit defrost fields, verify they propagate to `defrost_config`.

## Dependencies

**014 must be merged before 011.** Ticket 011's `DefrostCycleTracker` adds fields (`cycle_duration_s`, `max_defrost_duration_s`) that must flow into the typed config namespace established here. To avoid namespace collisions, this ticket must either: (a) declare those fields now with documented defaults, or (b) explicitly list them as deferred to 011 in a comment on the flattened struct.

## HPXML Wiring

| DefrostConfig Field | HPXML Source | Default |
|---|---|---|
| `defrost_control` | `DefrostControl` (Timed/OnDemand) | OnDemand |
| `defrost_strategy` | `DefrostType` (ReverseCycle/Resistive) | ReverseCycle |
| `defrost_time_fraction` | `DefrostTimePeriodFraction` | 0.058 |
| `defrost_eir_coeffs` | Not in HPXML; from defaults | None |
| `resistive_defrost_capacity_w` | Not in HPXML; config-only | 0.0 |
| `defrost_max_oat_c` | Not in HPXML; config-only | 4.4445 |

Currently `resolve_hvac.rs` does NOT read `DefrostType` or `DefrostControl`
from HPXML — they are hardcoded as OnDemand/ReverseCycle. This ticket must
add extraction of these HPXML elements in `resolve_hvac.rs` when the typed
config is built.

## Definition of Done

- [ ] `DefrostConfig` derives `Serialize, Deserialize` (no `deny_unknown_fields` on the inner struct)
- [ ] `DefrostConfig` embedded in `HeatPumpHeaterConfig` via `#[serde(flatten)]`
- [ ] `init_from_typed` populates `defrost_config` from typed config
- [ ] All defrost parameters configurable through typed config JSON
- [ ] Default values match current hardcoded defaults
- [ ] Validation rejects invalid ranges (e.g., `capacity_reduction_factor > 1.0`) with a loud error (no silent substitution)
- [ ] If `defrost_strategy` is set to a mode that requires additional fields (e.g., Resistive requires `resistive_defrost_capacity_w > 0.0`) and those fields are absent or zero, return a validation error — not a warning, not a silent default
- [ ] Unknown defrost keys in config JSON cause serde error (enforced by `HeatPumpHeaterConfig`'s existing `deny_unknown_fields`)
- [ ] Existing heater tests pass without modification
- [ ] New test: typed config with `defrost_strategy: "Resistive"` propagates correctly

## Verification

1. **Unit test**: Create `HeatPumpHeaterConfig` with explicit defrost fields; verify `defrost_config` matches after init.
2. **Unit test**: Create config with unknown key `defrost_startegy` (typo); verify serde error.
3. **Unit test**: Config with `defrost_control: "Timed"` and `defrost_time_fraction: 0.05`; verify `evaluate_defrost` uses Timed mode.
4. **Regression test**: Existing heater test suite passes without any config changes.
5. **Round-trip test**: Serialize and deserialize `HeatPumpHeaterConfig` with defrost fields; verify round-trip fidelity.

## References

- HARES `defrost.rs:36-74`: current `DefrostConfig` struct
- HARES `heat_pump_config.rs:113-114`: `DuctConfig` flatten precedent
- HARES `heater.rs:379`: hardcoded `DefrostConfig::on_demand(1.0, 0.0)`
- EnergyPlus I/O Reference, `Coil:Heating:DX` fields: `Defrost Strategy`,
`Defrost Control`, `Defrost Energy Input Ratio Function of Temperature
Curve Name`, `Defrost Time Period Fraction`, `Maximum Defrost Cycle Time`.

## Related Tickets

- [011-discrete-defrost-cycle.md](011-discrete-defrost-cycle.md) — discrete defrost will need additional config fields
- [013-mshp-minimum-compressor-speed.md](013-mshp-minimum-compressor-speed.md) — config struct improvements

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-20

### Code Confirmation

- [x] **heater.rs:379 still matches** — `defrost_config: DefrostConfig::on_demand(1.0, 0.0)` is present at line 379 in `HeatPumpHeaterCore::new()`. Line number confirmed exact.
- [x] **init_from_typed (heater.rs:460-617) has no defrost wiring** — the function populates every other typed config field (backup capacity, lockout temps, ER settings, duct DSE, biquadratic bounds) but contains zero references to `defrost_config`. Confirmed by reading lines 460–618 in full.
- [x] **DefrostConfig struct (defrost.rs:36-57) is missing Serialize/Deserialize** — line 36 reads `#[derive(Clone, Copy, Debug)]` with no serde derives. The enums `DefrostControl` (line 16) and `DefrostStrategy` (line 27) correctly derive `Serialize, Deserialize`.
- [x] **HeatPumpHeaterConfig (heat_pump_config.rs:20-139) has zero defrost fields** — confirmed by reading the entire struct. No `defrost_strategy`, `defrost_control`, `defrost_time_fraction`, `defrost_max_oat_c`, `defrost_capacity_reduction_factor`, `defrost_power_w`, `defrost_eir_coeffs`, or `resistive_defrost_capacity_w` fields exist.
- [x] **DuctConfig flatten precedent (heat_pump_config.rs:113-114)** — lines 112-114 show `#[serde(flatten)] pub duct: DuctConfig,`. Confirmed. `DuctConfig` does NOT carry `#[serde(deny_unknown_fields)]`, consistent with the ticket's explanation.
- [x] **`HeatPumpHeaterConfig` carries `#[serde(deny_unknown_fields)]` at line 19** — confirmed; the outer struct is the correct enforcement point for unknown-field rejection.
- [x] **OCHRE cross-check result: HARES OnDemand mode matches OCHRE** — OCHRE `HVAC.py:1139-1165` (URL cited in OCHRE source: `bigladdersoftware.com/epx/docs/8-9/engineering-reference/...#defrost-operation`) implements exactly: `defrost = t_ext_db < 4.4445`, `defrost_time_frac = 1/(1 + 0.01446/delta_omega)`, `defrost_capacity_mult = 0.875*(1-defrost_time_frac)`, `defrost_power_mult = 0.954/0.875`, `q_defrost = 0.01 * dtf * (7.222 - t_ext_db) * (cap_max/1.01667)`, `power_defrost = 0.1528 * (cap/1.01667) * dtf`. HARES constants `DEFROST_CAPACITY_MULTIPLIER_BASE=0.875`, `DEFROST_POWER_MULTIPLIER_NUMERATOR=0.954`, `DEFROST_TIME_FRACTION_NUMERATOR=0.01446`, `DEFROST_Q_MULTIPLIER=0.01`, `DEFROST_REFERENCE_TEMP_C=7.222`, `DEFROST_CAPACITY_UNIT_FACTOR=1.01667`, `DEFROST_EIR_TEMP_MODIFIER=0.1528` match OCHRE exactly. OCHRE implements only OnDemand/ReverseCycle — it has no typed Timed or Resistive config.
- [x] **EnergyPlus cross-check result: Defrost field names and defaults confirmed** — See web-verified citations below.

### Web-Verified Citations

**Citation 1**: EnergyPlus I/O Reference `Coil:Heating:DX` — `Defrost Strategy`, `Defrost Control`, `Defrost Energy Input Ratio Function of Temperature Curve Name`, `Defrost Time Period Fraction`, `Maximum Defrost Cycle Time`.

- **Source found**: EnergyPlus 8.0 I/O Reference (bigladdersoftware.com/epx/docs/8-0/input-output-reference/page-034.html, VRF section) + DesignBuilder DX Heating Coil documentation (designbuilder.co.uk/helpv7.2/Content/HeatingCoilDX.htm)
- **Quoted passage** (from EnergyPlus 8.0 I/O Reference, VRF section, via WebFetch):
  > "This numeric field defines the fraction of compressor runtime when the defrost cycle is active. For example, if the defrost cycle is active for 3.5 minutes for every 60 minutes of compressor runtime, then the user should enter 3.5/60 = **0.058333**."
  
  From DesignBuilder (which documents EnergyPlus model):
  > **Defrost Strategy** — Allowed choices: "1-Reverse-cycle or 2-Resistive". Default: "The default defrost strategy is reverse-cycle."
  > **Defrost Control** — Allowed choices: "1-Timed or 2-On-demand."
  > **Defrost Time Period Fraction** — Default value: "The default value is **0.058333**."
  
  From EnergyPlus I/O Reference (bigladdersoftware.com/epx/docs/8-0/input-output-reference/page-034.html):
  > **Defrost Strategy**: "This alpha field has two choices: reverse-cycle or resistive. If this input field is left blank, the default defrost strategy is reverse-cycle."
  > **Defrost Control**: "This alpha field has two choices: timed or on-demand. If this input field is left blank, the default defrost control is **timed**."
  > **Maximum Outdoor Dry-Bulb Temperature for Defrost Operation**: "If this input field is left blank, the default value is **5 C**."

- **Verdict**: **Partially correct** — the five field names cited by the ticket exist. However, two discrepancies with HARES defaults:
  1. EnergyPlus default for `Defrost Control` is **Timed**, not OnDemand. HARES (following OCHRE) uses OnDemand as the default, which is a deliberate divergence to match OCHRE's physics-based approach.
  2. EnergyPlus default for `Maximum Outdoor Dry-Bulb Temperature for Defrost Operation` is **5°C**, while OCHRE and HARES use **4.4445°C** (≈ 40°F). This is a known discrepancy; HARES follows OCHRE.
  3. EnergyPlus field name is "Maximum Outdoor Dry-Bulb Temperature for Defrost Operation" — not "Maximum Defrost Cycle Time" as written in the ticket's References section. The ticket's reference line contains a field name error.
  4. The default defrost time fraction of **0.058333** matches HARES `DEFAULT_DEFROST_TIME_FRACTION = 0.058` (truncated but equivalent).

**Citation 2**: HPXML `DefrostControl`, `DefrostType`, `DefrostTimePeriodFraction` elements.

- **Source found**: OpenStudio-HPXML documentation (openstudio-hpxml.readthedocs.io), HPXML schema documentation (hpxmlwg.github.io/hpxml), HPXML Toolbox (hpxml.nlr.gov — searched exhaustively).
- **Quoted passage**: No mention of `DefrostControl`, `DefrostType`, or `DefrostTimePeriodFraction` was found in any HPXML documentation page accessed. The OpenStudio-HPXML workflow_inputs.rst does not list these as heat pump inputs. The HPXML schema explorer at hpxmlwg.github.io/hpxml/schemadoc lists high-level HPXML elements but defrost-specific sub-elements were not found.
- **Verdict**: **Cannot fully verify** — the HPXML table in the ticket (`DefrostControl`, `DefrostType`, `DefrostTimePeriodFraction` as HPXML elements) could not be independently confirmed through web-accessible HPXML documentation. These element names are plausible HPXML schema names (consistent with HPXML naming conventions like `HeatPumpType`, `BackupSystemFuel`) but their presence in the HPXML XSD was not confirmed. The HPXML wiring described in the ticket (reading `DefrostType`/`DefrostControl` in `resolve_hvac.rs`) is aspirational — `resolve_hvac.rs` currently has zero defrost-related code (confirmed by `grep`).

**Citation 3**: "Defrost EIR biquadratic curve evaluated at (wb, db) with 15.555°C floor."

- **Source found**: EnergyPlus Engineering Reference VRF section (bigladdersoftware.com/epx/docs/8-9/engineering-reference/variable-refrigerant-flow-heat-pumps.html), OCHRE source HVAC.py line 1140 URL reference.
- **Quoted passage**: The 15.555°C floor is referenced indirectly from `DEFROST_EIR_CURVE_TEMP_MIN_C` in `constants.rs`; the OCHRE source cites the EnergyPlus VRF Engineering Reference for this model. Direct fetch of the Engineering Reference section was truncated, but the URL in OCHRE source (`#defrost-operation-201605050925`) confirms this is the authoritative upstream source.
- **Verdict**: **Confirmed by code cross-reference** — the HARES `defrost.rs:196-197` applies `inlet_wb_c.max(DEFROST_EIR_CURVE_TEMP_MIN_C)` and `outdoor_db_c.max(DEFROST_EIR_CURVE_TEMP_MIN_C)`, matching the EnergyPlus VRF model structure. Tests 8 (EIR curve floor clipping) and 12 (plausible reference values) pass, confirming correct implementation.

### Legitimacy

- **Verdict**: **Legitimate**
- **Rationale**: Every core claim in the ticket is confirmed by direct code inspection. (1) `DefrostConfig::on_demand(1.0, 0.0)` is hardcoded at `heater.rs:379` — verified exact line. (2) `init_from_typed` (lines 460–617) contains no defrost wiring — verified by reading every line. (3) `DefrostConfig` has no `Serialize`/`Deserialize` derives — confirmed at `defrost.rs:36`. (4) `HeatPumpHeaterConfig` has no defrost fields — confirmed by reading the entire struct. (5) The `DuctConfig` flatten precedent at `heat_pump_config.rs:113-114` is exactly as described. (6) EnergyPlus I/O Reference confirms the five defrost field names and the 0.058333 default for Defrost Time Period Fraction. One minor error in the References section: the ticket writes "Maximum Defrost Cycle Time" but the correct EnergyPlus field name is "Maximum Outdoor Dry-Bulb Temperature for Defrost Operation." The ticket's proposed approach (serde flatten, no `deny_unknown_fields` on inner struct) is architecturally sound and consistent with the `DuctConfig` precedent and the serde limitation described.

### Proposed Fix Summary

1. Add `#[derive(Clone, Copy, Debug, Serialize, Deserialize)]` to `DefrostConfig` in `defrost.rs:36`. Add `#[serde(default)]` on each field and appropriate `#[serde(rename = "defrost_...")]` attributes to match snake_case config keys.
2. Add `pub defrost: DefrostConfig` to `HeatPumpHeaterConfig` with `#[serde(flatten)]`, following the `DuctConfig` pattern at `heat_pump_config.rs:113-114`. Do NOT add `deny_unknown_fields` to `DefrostConfig`.
3. Add `DefrostConfig::default()` to `HeatPumpHeaterConfig::default()`.
4. In `init_from_typed` (`heater.rs:460-617`), add `self.defrost_config = cfg.defrost;` after the existing config assignments. Remove the `DefrostConfig::on_demand(1.0, 0.0)` initialization from `new()` (line 379) or keep it and let `init_from_typed` override it.
5. Add `DefrostConfig` validation (range checks on `capacity_reduction_factor`, `defrost_time_fraction`, `max_oat_defrost_c`, `resistive_defrost_capacity_w`; error if `strategy == Resistive && resistive_defrost_capacity_w == 0.0`).
6. Optionally add `#[serde(skip)]` on the defrost field in `HeatPumpCoolerConfig` or add the same flattened struct for round-trip compatibility.
7. Wire `DefrostType` and `DefrostControl` extraction in `resolve_hvac.rs` when building typed config from HPXML, once HPXML element names are confirmed against the actual XSD.

### Test Written

- **File**: `crates/hares-equipment/tests/hvac_tests.rs` (appended at end of file)
- **Tests added** (4):
  1. `ticket_014_defrost_strategy_field_not_in_typed_config` — verifies that `defrost_strategy` is not yet in `HeatPumpHeaterConfig` (serde rejects it as unknown field); will fail once ticket-014 adds the field.
  2. `ticket_014_defrost_control_field_not_in_typed_config` — same for `defrost_control`.
  3. `ticket_014_resistive_defrost_capacity_field_not_in_typed_config` — same for `resistive_defrost_capacity_w`.
  4. `ticket_014_defrost_config_not_wired_from_typed_path` — initialises an ASHP via typed config, runs a defrost-condition step (OAT = -5°C), confirms defrost activates via the hardcoded OnDemand path; asserts `defrost_time_fraction ≠ 0.058` (proving OnDemand formula is running, not a hypothetical Timed default). Will need updating once the wiring is implemented.
- All 4 tests pass under the current (unfixed) codebase, confirming they correctly document the bug.
