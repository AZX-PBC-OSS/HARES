---
id: HARES-044
title: "hares-core — Dwelling from_hpxml Constructor"
kind: implement
depends_on: [HARES-043, HARES-035, HARES-036, HARES-037]
files_to_touch:
  - crates/hares-core/src/dwelling.rs
references:
  - docs/architecture/01-sim-core-and-solver.md
  - docs/architecture/04-data-ingestion-and-fleet.md
verification:
  - cargo check -p hares-core
  - cargo test -p hares-core
  - cargo clippy -p hares-core -- -D warnings
---

## Background/Context
The primary way a `Dwelling` is constructed in production and testing is from a ResStock HPXML bundle (HPXML file, schedule CSV, EPW weather file). This constructor drives the full single-building input chain: parsing, schedule loading, weather loading, override application, envelope RC network construction, state-space discretisation, and equipment instantiation. An OCHRE-compatible constructor wraps this for drop-in compatibility.

## Work to Do
- [ ] Implement `Dwelling::from_hpxml`
  - [ ] Signature: `from_hpxml(hpxml_path: &Path, schedule_path: &Path, weather_path: &Path, start_time: DateTime<Utc>, time_res: Duration, duration: Duration, overrides: Option<serde_json::Value>) -> Result<Dwelling>` — `start_time`, `time_res`, and `duration` cannot be inferred from the HPXML file and must be supplied by the caller. Use `serde_json::Value` for overrides to match HARES-036's `nested_update` signature and HARES-043's `DwellingConfig`.
  - [ ] Step 1: parse HPXML via `hares-io` → `BuildingDescription`
  - [ ] Step 2: parse schedule CSV → `ScheduleTimeSeries`; call `resample(time_res.as_secs() as u32)` on the result. Fail with error if resampling fails.
  - [ ] Step 3: parse EPW → `WeatherTimeSeries`
  - [ ] Step 4: build envelope RC network from boundary properties in `BuildingDescription`
  - [ ] Step 5: discretise the state-space model for `time_res`; validate RC stability — eigenvalue checks must pass before construction completes: continuous Re(λ)<0 for all eigenvalues of A_c, discrete |λ_d|<1.0 for all eigenvalues of A_d (see arch doc `01-sim-core-and-solver.md` §Stability); return `Err` on failure. If any discrete eigenvalue |λ_d| > 0.99, emit a `tracing::warn!` for oscillatory-but-bounded behavior (near-unity eigenvalues can cause visible temperature oscillations even though the system is technically stable).
  - [ ] Step 6: instantiate equipment from HPXML equipment specs via `EquipmentRegistry`
  - [ ] Step 7: apply `overrides` via a recursive `nested_update` merge (override keys shadow parsed values) — overrides are applied AFTER equipment instantiation so that equipment-specific kwargs correctly shadow per-type defaults, matching OCHRE's `nested_update()` semantics (arch doc `04-data-ingestion-and-fleet.md` Step 5)
  - [ ] Step 8: apply ZIP voltage-dependency parameters from defaults library (arch doc `04-data-ingestion-and-fleet.md` Step 6)
  - [ ] Step 9: initialise all equipment with config and initial `EnvironmentState`. Equipment `init()` in this step must use the post-override merged config from Step 7, not the pre-override parsed config.
  - [ ] Step 10: create `StreamingRecorder` with OCHRE-compatible output columns
- [ ] Implement OCHRE compat constructor
  - [ ] `Dwelling::from_ochre_kwargs(kwargs: HashMap<String, serde_json::Value>) -> Result<Dwelling>`
  - [ ] Map `hpxml_file`, `hpxml_schedule_file`, `weather_file`, and `Equipment` overrides dict to `from_hpxml` arguments; overrides must use `serde_json::Value`
- [ ] Implement `Dwelling::from_toml_config(path: &Path) -> Result<Dwelling>` for synthetic building configs (needed by BESTEST cases in HARES-055). TOML config must cover geometry, material properties, zone volumes, and HVAC characteristics.

## Files to Touch
- `crates/hares-core/src/dwelling.rs`: extend with `from_hpxml` and `from_ochre_kwargs` constructors

## Measures of Success
- [ ] Constructing a `Dwelling` from a ResStock HPXML fixture succeeds without error
- [ ] Equipment list from a known fixture HPXML matches expected count and exact name list (e.g., `["Gas Furnace", "Air Conditioner", "Electric Resistance Water Heater"]`)
- [ ] Zone temperature at hour 0 matches steady-state initialization: between outdoor and indoor design temp
- [ ] Output column names at verbosity 0 match OCHRE reference column list exactly
- [ ] The RC model matrix dimensions match the number of thermal zones and boundaries in the fixture
- [ ] A 1-hour simulation run from the constructed `Dwelling` completes without panicking
- [ ] An HPXML with an unsupported equipment type returns `Err` with the unrecognized type name

## Verification
- [ ] `cargo check -p hares-core` passes
- [ ] `cargo test -p hares-core` passes
- [ ] `cargo clippy -p hares-core -- -D warnings` passes
