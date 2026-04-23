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
