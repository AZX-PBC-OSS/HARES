---
red_team_outcome: clean
failing_tests: []
---

# Red-team (this run): I-07 — the actor-param guard fix

## State since the last red-team pass

The actor-param finding (the last run's: `py_to_config_value` accepted
NaN/±inf, the thermostat seed armed `Some(NaN)` as an active override,
and the failure surfaced mid-simulation — a telemetry panic in debug
builds, a silent never-heating controller behind a log line in
release) is fixed and gated. The guard landed at the boundary the
value crosses — `py_to_config_value` rejects non-finite scalars with a
typed message naming the mechanism (NaN comparisons are always false;
the actor's conditionals silently stop firing) — and the fix extended
the class one level deeper than the attack reached: the converter's
**list arm** now checks elements too (`FloatArray` via the Occupant's
real `occupancy_column` param), with the fix's own gate cases appended
to my boundary file (ids preserved — 13 cases now). Baseline verified
this run: the boundary file **13/13** green; hares-equipment lib
**2337 passed / 0 failed**; `ev_thermal_regressions` 8/8;
`equipment_id_regressions` 39/39; clippy clean; `cargo fmt --check
--all` clean.

## Why this run's outcome is `clean`

The fix's surface is the guarded converter and its sweep. Attacked:

- **A second unguarded `ConfigValue::Float` producer** — swept by grep
  across the workspace: every other construction site is a pattern
  match (a read) or a test fixture. The Python converter is the only
  producer, and it now guards both arms.
- **A non-Python entry to actor params** (a TOML/config-file path —
  TOML natively encodes `nan`/`inf`) — refuted: no production Rust
  path builds `ActorConfig` with params from a file; `with_param`'s
  only non-test caller is the guarded Python converter. The parity
  fixture TOMLs carry equipment overrides, not actor params.
- **The sweep's other boundaries** (`py_hvac.rs`, `py_water_heater.rs`
  — their local converters deleted in favor of the shared
  `python_to_json_value`) — the shared function is one implementation
  already exercised end-to-end from three surfaces (tariff, EV,
  actor); re-testing per call site would re-exercise the same guard
  with different data.
- **Nested list-of-lists or dict-valued params** — no converter arm
  matches → the loud "config value must be bool, float, string, or
  list of floats" error; a NaN can only ride a `Vec<f64>`, which the
  element guard covers.
- **The carried dismissals** from the earlier runs (drive-I²R chunking,
  the threshold dead zone, the LUT c-rate divisor divergence,
  `power_limit_kw` on the discharge path) — untouched code, dismissals
  stand.

No hypothesis survived. Forcing another test would re-exercise the
shared guard through a fourth call site — coverage rounding, not an
attack. `failing_tests` is the explicit empty list.

## Verification commands (run, not recalled)

```
$ uv run pytest tests/python/test_actor_param_boundary.py -q
13 passed in 2.60s      # the last run's 8 findings fixed; the fix's own
                        # list-element gates + controls appended with ids intact

$ uv run pytest tests/python/test_ev_pack_thermal_boundary.py -q
20 passed in 3.01s      # the earlier Python-boundary finding: still gated

$ cargo test -p hares-equipment --lib
test result: ok. 2337 passed; 0 failed

$ cargo test -p hares-core --test ev_thermal_regressions --test equipment_id_regressions
test result: ok. 39 passed; 0 failed
test result: ok. 8 passed; 0 failed

$ cargo clippy -p hares-equipment --tests   # clean
$ cargo fmt --check --all                   # clean (FMT_OK)
```

The cumulative red-team record across the initiative: four findings
(the GridEmergency discharge label, the classifier threshold window,
the EV config boundary, the actor-param boundary), all fixed, all
gated — 4 red-team Rust tests plus the two Python boundary files
(8 test functions, 33 cases: 20 EV + 13 actor) — no open findings.
