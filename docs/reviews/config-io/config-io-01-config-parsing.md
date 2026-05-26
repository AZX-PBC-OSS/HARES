# Configuration parsing: TOML deserialization, validation, error messages
**Review ID**: config-io-01
**Category**: config-io
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-io/src/config.rs`
- `crates/hares-core/src/dwelling/synthetic.rs` (lines 14-56 — `SyntheticTomlConfig`, `SyntheticSimulationConfig`)
- `tests/parity/mod.rs` (lines 58-68 — `FixtureConfig`)
- `crates/hares-io/tests/config_round_trip.rs` (lines 622-651 — `assert_deny_unknown` / `typed_config_tests!` macro)

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/utils/base.py` (lines 18, 22-29, 44-95 — `OCHREException`, `nested_update`, `convert_hpxml_element`)
- `vendors/OCHRE/ochre/utils/hpxml.py` (lines 98-114, 108, 260, 426, 832, 1053, 1142-1148 — HPXML parsing, assertion-based validation, error messages)
- `vendors/OCHRE/ochre/utils/schedule.py` (lines 89-119, 97, 105, 135 — schedule loading, validation)
- `vendors/OCHRE/ochre/Simulator.py` (lines 60, 110, 181-184 — simulation config validation)

## Findings

### Finding 1: Severity: critical
**Description**: `SimulationConfig` is missing `#[serde(deny_unknown_fields)]`. A misspelled TOML key on any field with a default (the majority of fields) is silently accepted, and the default value is used instead.

**Code Location**: `crates/hares-io/src/config.rs:34` — the `#[derive(…Deserialize)]` attribute on `pub struct SimulationConfig`.

**Root Cause**: The derive macro includes `Deserialize` but does not include `#[serde(deny_unknown_fields)]`. Serde's default behavior is to silently ignore unrecognised keys.

**Impact**: Several scenarios are silently broken:
| Misspelling | Field | Behaviour |
|---|---|---|
| `master_sead = 42` | `master_seed` | Defaults silently to `0` — reproducibility broken |
| `output_verbostiy = 5` | `output_verbosity` | Defaults silently to `0` — user sees no output but gets no error |
| `output_chunck_size = 1000` | `output_chunk_size` | Defaults silently to `10_000` — flush granularity wrong |
| `setpoint_deadband_c = 2.0` | `setpoint_deadband_c` — but a typo like `setpoint_deadband` | Defaults to `None` — deadband ignored |
| `durtion = 3600` | `duration` (required, no default) | Serde ignores `durtion`, then reports "missing field `duration`". User is confused because they *believe* they provided it. |

For required fields the user gets a misleading "missing field" error rather than "unknown field `durtion`". For optional/defaulted fields the user gets no error at all.

**Contrast with HARES conventions**: Every equipment config struct in the codebase (21 structs across `heating_config.rs`, `cooling_config.rs`, `wh_config.rs`, etc.) uses `#[serde(deny_unknown_fields)]`. The `config_round_trip.rs` test suite even has a dedicated `assert_deny_unknown` helper (line 622-629) that verifies this protection on all typed configs. `SimulationConfig` is the outlier.

**Contrast with OCHRE**: OCHRE has no structurally equivalent protection — HPXML keys flow through `xmltodict.parse()` into plain Python dicts, and unknown keys propagate via `nested_update()` into unseen data. HARES already does better than OCHRE for equipment configs; this gap in `SimulationConfig` is an inconsistency within HARES itself.

### Finding 2: Severity: high
**Description**: `SyntheticTomlConfig` and all its sub-structs (`SyntheticSimulationConfig`, `SyntheticGeometryConfig`, `SyntheticMaterialsConfig`, etc.) also lack `#[serde(deny_unknown_fields)]`.

**Code Location**: `crates/hares-core/src/dwelling/synthetic.rs`:
- Line 14: `SyntheticTomlConfig`
- Line 49: `SyntheticSimulationConfig`
- Line 58: `SyntheticGeometryConfig`
- Line 68: `SyntheticMaterialsConfig`
- Line 73: `SyntheticHvacConfig`
- Line 84: `SyntheticWeatherConfig`
- Line 125: `SyntheticScheduleConfig`
- Line 150: `SyntheticOutputConfig`
- Line 183: `SyntheticBoundaryConfig`
- Line 232: `SyntheticWindowConfig`
- Line 244: `SyntheticSetpointConfig`
- Line 255: `SyntheticInfiltrationConfig`

The parity `FixtureConfig` (`tests/parity/mod.rs:58`) has the same issue.

**Root Cause**: Same as Finding 1 — missing `#[serde(deny_unknown_fields)]`.

**Impact**: When users write synthetic-dwelling TOML files (BESTEST fixtures, custom dwelling TOML for `from_toml_config_with_write_output`), misspelled optional section names like `[windos]` instead of `[windows]` or `boundries` instead of `boundaries` are silently skipped. Field-level typos within sub-tables behave identically to Finding 1. Since `SyntheticTomlConfig` is `pub(crate)`, external HARES consumers cannot directly construct it, but users writing `.toml` synthetic dwelling files ARE affected.

**Mitigation**: The heat pump config structs (`HeatPumpCommonConfig`, `HeatPumpHeaterConfig`, `HeatPumpCoolerConfig`, `DefrostConfig`) document the `serde(flatten)` incompatibility with `deny_unknown_fields` and include explicit tests proving unknown keys are accepted (`crates/hares-equipment/src/hvac/heat_pump_config.rs:724-741`). No such documentation or test exists for `SyntheticTomlConfig` or `SimulationConfig`.

### Finding 3: Severity: medium
**Description**: Two validation error messages omit the invalid value, making them less actionable than the other three validators in the same function.

**Code Location**: `crates/hares-io/src/config.rs`:
- Lines 92-94: `"duration must be greater than zero"` — does not include the actual value
- Lines 99-101: `"time_res must be greater than zero"` — does not include the actual value

**Root Cause**: These two error messages were written without the `format!(…)` pattern used by the other validators.

**Impact**: If a user provides `duration = 0` or `duration = -500`, the error message says only "must be greater than zero" with no indication of what value was seen. The user must re-read their config file to find the problem. This is especially confusing for `duration = -500`: the message says "must be greater than zero" but doesn't confirm that `-500` was parsed.

**Contrast**: The three other validators in the same method DO include the actual value:
- `output_verbosity` (lines 104-109): `"output_verbosity must be 0-{MAX_VERBOSITY}, got {value}"` — complete
- Duration/time_res divisibility (lines 111-115): `"duration ({d}s) must be evenly divisible by time_res ({t}s)"` — complete
- `setpoint_deadband_c` (lines 117-123): `"setpoint_deadband_c must be finite and >= 0, got {v}"` — complete

**Contrast with OCHRE**: OCHRE's equivalent checks (`Simulator.py:60`) include the values: `"Duration ({duration}) must be longer than time resolution ({time_res})."` HARES's verbosity and alignment messages match this quality; the `duration` and `time_res` ones lag behind.

### Finding 4: Severity: low
**Description**: `deserialize_duration_seconds` only accepts bare integers. String duration formats like `"5min"`, `"2h"`, or `"1h 30m"` produce a serde error from the `i64::deserialize` call, not a configuration-specific error directing users to use integer seconds.

**Code Location**: `crates/hares-io/src/config.rs:152-158`.

**Root Cause**: The custom deserializer is a thin wrapper around `i64::deserialize`. If a user writes `duration = "3600"` (quoted integer) or `duration = "1h"`, serde will emit a generic type-mismatch error ("invalid type: string …, expected i64").

**Impact**: The resulting error is from serde's internals, not from HARES's `ConfigError` wrapper. It says something like `TOML parse error: invalid type: string "1h", expected i64 at line 3 column 13` — technically correct but not as user-friendly as "duration must be an integer number of seconds". Low severity because the serde message is usually understandable, and the TOML spec in the doc examples uses bare integers.

### Finding 5: Severity: low (informational)
**Description**: No environment-specific config layering mechanism exists in HARES.

**Code Location**: N/A — there is no layering to inspect.

**Assessment**: The review criteria asked to verify that debug/release/testing config layers apply in the correct order. HARES has no such layering: `DwellingConfig` (`crates/hares-core/src/dwelling/mod.rs:100-113`) is constructed imperatively from individual components with no tiered file-resolution system. There is no mechanism for lower-priority config files to shadow higher-priority overrides incorrectly because there is no layering mechanism at all. The `cfg(feature = "dst")` mention in the `civil_timezone` doc comment is a feature flag, not a config layer. This is a pass with no defects found.

## Summary
- Total findings: 5
- Critical: 1 (Finding 1 — `SimulationConfig` missing `deny_unknown_fields`)
- High: 1 (Finding 2 — `SyntheticTomlConfig` and sub-structs missing `deny_unknown_fields`)
- Medium: 1 (Finding 3 — inconsistent validation error message quality)
- Low: 2 (Finding 4 — duration deserializer doesn't guide on format; Finding 5 — no environment layering exists, pass)

## Recommendations
1. Add `#[serde(deny_unknown_fields)]` to `SimulationConfig` (`crates/hares-io/src/config.rs:34`). This is a one-line change that brings the struct in line with all 21 equipment config structs. Add a test analogous to `assert_deny_unknown` in `config_round_trip.rs` that verifies a misspelled key like `simulaton` produces a parse error. Note: `duration` and `start_time` lack `#[serde(default)]`, so a missing-field error would still fire for a mistyped required field — but the error message will say "missing field `duration`" instead of "unknown field `simulaton`". To address that UX gap, consider writing a wrapper `from_toml` that catches the serde error and checks whether any unknown keys exist, or rely on the fact that `deny_unknown_fields` will already prevent the silent-default case for optional fields.
2. Add `#[serde(deny_unknown_fields)]` to `SyntheticTomlConfig` and all sub-structs in `crates/hares-core/src/dwelling/synthetic.rs`. Since none of these use `serde(flatten)`, there is no technical barrier.
3. Update the `duration` and `time_res` validation error messages in `config.rs:92,99` to include the actual value using the `format!(…)` pattern already used by the other validators: `"duration must be greater than zero, got {value}"` and `"time_res must be greater than zero, got {value}"`.
4. (Optional) Enhance `deserialize_duration_seconds` to produce a custom error message suggesting integer seconds when a non-integer value is encountered, or support common duration strings like `"1h"`, `"30min"` via a parse library.

## References / Citations
- `crates/hares-io/src/config.rs:34` — `SimulationConfig` struct definition
- `crates/hares-io/src/config.rs:87-126` — `from_toml` validation logic
- `crates/hares-io/src/config.rs:152-158` — `deserialize_duration_seconds`
- `crates/hares-equipment/src/hvac/heat_pump_config.rs:18-22` — documented `deny_unknown_fields` / `flatten` incompatibility
- `crates/hares-equipment/src/hvac/heat_pump_config.rs:724-741` — test proving unknown keys silently accepted on flattened structs
- `crates/hares-io/tests/config_round_trip.rs:622-629` — `assert_deny_unknown` helper used by all typed config tests
- `vendors/OCHRE/ochre/Simulator.py:60` — OCHRE duration validation (includes value in message)
- `vendors/OCHRE/ochre/utils/base.py:22-29` — OCHRE `nested_update` (no unknown-key rejection)
