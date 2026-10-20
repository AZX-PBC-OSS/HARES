# Review of BR-21, commit d41646e9 (parent 1c18543a)

Verdict: APPROVED

Findings: 0 blocker, 0 major, 2 minor, 2 nit.

Reviewed in a fresh worktree of d41646e9, CARGO_TARGET_DIR outside the repo, -j 8. Worktree removed afterwards.

## Verification results

Clippy, every line `cargo clippy --workspace --all-targets <args> -- -D warnings`, all exit 0:
- plan.md lines: (none), `--all-features`, `-F dst`, `-F profiling`, `-F observe`, `-F observe,profiling`, `--release`.
- Extra release combinations I added: `--release -F dst`, `--release -F profiling`, `--release -F observe`, `--release --all-features`, `--release -F observe,profiling`, `--release -F observe_detailed`. All exit 0. Debug `-F observe_detailed` also exit 0.

Block lines:
- `cargo fmt --check --all`: exit 0.
- nextest default: 7151 passed. `-F observe`: 7210 passed. `-F dst`: 7164 passed. `-F profiling`: all passed. `-p hares-core -F observe,profiling`: all passed.
- nextest `--release`: passed. `--release -F observe`: passed (extra).
- `cargo bench --workspace --no-run`: exit 0.
- `frame_golden compare-all` default and `-F dst`: exit 0 (golden unchanged).
- `[test-release-invariants]` (`--release -F check_invariants`): exit 101, "none of the selected packages contains this feature". See M-1.

Environment note: the first debug nextest runs failed 6 hpxml_parsing_tests only because vendors/OCHRE is an uninitialised submodule in a fresh worktree (fixtures not found). After `git submodule update --init vendors/OCHRE` all three debug lines passed. Not a BR-21 defect.

`make check-ashrae-reference` in the fresh worktree with no .venv and no OCHRE submodule: PASS. The generator needs only xmltodict (`import xmltodict`, no vendors/ read), so `uv run --extra ochre` is sufficient. The Makefile comment removal is correct and leaves the preceding comment intact.

Writing rules (the plan's `writing` check on the commit message and diff): no match for em dashes, ticket IDs or history terms.

## Requirement by requirement

1. Census and per-site cfg fixes: met, no `allow` added.
   - `thermal_solver/mod.rs:1858` and `:6619`: `BoundaryDiagnosticInfo` moved into the one test gated `any(debug_assertions, observe_detailed)`. Used exactly there; the same cfg.
   - `dwelling/mod.rs:105-106`: `MoistureZoneInvariant` import gated `all(observe, debug_assertions)`, matching the only uses (field at 1863, push at 7966, same cfg). Remaining observer imports stay under `observe`.
   - `solver_builder.rs:436-440`: `r_total` bound under `any(debug_assertions, observe)`. Readers: the debug identity check (`debug_assertions`) and the observe film telemetry (`observe`). Exactly the union. Gated code only; no runtime change in any build that compiled before.
   - `tests/conditioned_oracle.rs:18-21`: test module gated `all(observe, any(debug_assertions, observe_detailed))`. Release `-F observe` builds now contain zero oracle tests rather than failing to compile. No test that ran before stops running (it did not compile in that configuration).
2. Block lines: plan.md section 3 carries the `[clippy-*]` lines; all exit 0.
3. Script and Makefile: met (above).
4. CI: the clippy job lints default, dst, profiling, observe, observe+profiling, all-features and release. These cover every feature set the test job and the plan's test lines use. The `uv sync --extra ochre` step is removed from the ashrae job and the script needs none.

## Findings

### minor

M-1. `plan.md` section 3 `[test-release-invariants]` is unrunnable: `cargo nextest run --workspace --release --no-fail-fast -F check_invariants` fails at this commit (hares-core no longer defines `check_invariants`; removed in b8195c83, per `crates/hares-core/Cargo.toml:9-13`). Pre-existing, not introduced by BR-21, but it means the block cannot be green at this head and BR-21's "full block" acceptance cannot literally be met. Fix: delete the line from the block now (it is already marked "deleted by OR-11") or run OR-11's deletion before this entry.

M-2. Remaining lint suppressions that hide exactly the class of defect BR-21 fixes (cfg-only or test-only imports). Requirement 1 forbade adding allows, not removing existing ones, so this does not fail BR-21, but they are broken windows next to the fix:
- `crates/hares-equipment/src/water_heater/heat_pump_wh.rs:18`, `gas.rs:18`, `tankless.rs:15`, `indirect_tank.rs:25`: `#[allow(unused_imports)] use tracing::warn;` with a comment claiming `warn!` is used under debug_assertions. `warn` appears nowhere else in these four files, so the import is dead in every build. Removable: delete the import and the allow.
- `water_heater/resistance.rs:17-18`: `warn!` is used unconditionally at line 399, so the allow is pointless. Removable: delete the allow line.
- `crates/hares-core/src/dwelling/mod.rs:100`: the `#[cfg_attr(not(test), allow(unused_imports))]` covers one use item (there is one such attribute here, not ~100). `Actor`, `ActorInterest`, `EnvironmentManager` and `SimClock` are used in non-test code (lines 788, 1806, 1487, 1668). Only `derive_dwelling_rng` is test-only (used from line 12208). Removable: drop it from the use list and import it in `mod tests`.
- `crates/hares-core/src/dwelling/autosize.rs:45`: same attribute on `use serde_json::{Value, json}`. `json!` is used in non-test code; `Value` only in tests (2944). Removable: import `json` at top level, `Value` in `mod tests`.

### nit

N-1. `solver_builder.rs:436-438` comment says the binding is gated "to keep plain release builds lint-clean". Accurate, but it states the lint motive rather than which builds read it; acceptable.

N-2. CI clippy runs eleven-plus steps serially in one job; the clippy line count equals the plan block, so no action, only a runtime note.

## Other `allow(` attributes in non-vendor Rust (first-pass classification; only the unused_imports group above was verified by reading the code)

Candidates likely removable with small changes:
- `dead_code`: `tests/bestest/material_check.rs:23,34`; `tests/envelope_oracle.rs:88,349`; `hares-equipment/src/pv/lut.rs:482,659`; `pv/array_config.rs:59`; `pv/mod.rs:162`; `hares-python/src/conversions.rs:151,157`; `hares-envelope/.../infiltration.rs:42,58`; `thermal_solver/mod.rs:220`; `hares-io/src/envelope_lut.rs:86`; `hares-io/src/epw.rs:794`; `hares-io/src/hpxml/building.rs:3159`; `hares-equipment/src/battery/mod.rs:424`; `hvac/coil_physics.rs:218`. Unused code should be deleted, or used, or cfg-gated; each needs a per-site check.
- Crate-level `#![allow(dead_code)]` in `benches/common.rs:4` and `tools/frame_golden/tests/common/mod.rs:8`: shared test/bench helper modules; removable only by splitting helpers per consumer.
- `unused_variables` via cfg_attr or allow: `hares-equipment/src/scheduled_load.rs:211`, `infiltration.rs:185`, `hvac/heat_pump/defrost.rs:287` (all `not(observe)` style; removable by cfg-gating the binding as BR-21 does), `hares-python/src/py_equipment.rs:771` (`seed`; rename to `_seed` or use), `hares-io/src/hpxml/building.rs:1604`.
- `clippy::needless_update` in test modules: `water_heater/gas.rs:1140`, `heat_pump_wh.rs:1640,3455,3780,4096`, `resistance.rs:1092,2058`, `hares-io/src/hpxml/resolve_water_heater.rs:1044`, `thermal_solver/mod.rs:1843`, `hares-types/src/equipment.rs:3953`. Removable by dropping `..Default::default()` from the literals, at the cost of brittleness when fields are added.
- Single-site clippy allows, each likely fixable in place: `scheduled_load.rs:227` (`single_match`), `longwave.rs:621` (`unused_enumerate_index`), `hares-types/src/panic_hook.rs:459,487` (`unnecessary_literal_unwrap`, test intent), `hares-tariff/src/evaluator.rs:301` (`manual_is_multiple_of`, use `is_multiple_of`), `hvac/dehumidifier.rs:1889` (`items_after_test_module`, move items), `thermal_solver/mod.rs:2503-2504` (`approx_constant`, test constants), `hares-types/src/fluid.rs:342` (`cast_possible_truncation`, pedantic, not enabled workspace-wide, so the allow is redundant), `hares-equipment/src/lib.rs:543` (`deprecated`).

Structural, removable only by parameter structs or pyo3 signature changes:
- `clippy::too_many_arguments`: `water_heater/tank.rs:381`, `defrost.rs:450`, `coil_physics.rs:95`, `stepping.rs:200,1247,1281`, `hares-physics/src/solar.rs:480,541`, `pv_sizing.rs:487,840,1076`, `hares-tariff/src/billing.rs:184,423`, and the hares-python bindings (`py_config.rs:92,584`; `py_hvac.rs` 9 sites; `py_equipment.rs:323,711`; `py_pv_sizing.rs` 5 sites; `py_water_heater.rs:49,208,368`; `py_dwelling.rs:1798`). The pyo3 sites are keyword-argument constructors, so removal means API redesign, not a lint fix.

## Residual risk

- Debug `-F observe_detailed` and release `-F observe_detailed` lint clean but no test line exercises `observe_detailed`; the release-observe oracle tests are compiled out by design.
- Allow classification beyond the unused_imports group is by reading attribute sites, not by removing each and re-linting.
