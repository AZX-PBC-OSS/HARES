---
id: HARES-037
title: "hares-io — Equipment Defaults and Simulation Config"
kind: implement
depends_on: [HARES-001]
files_to_touch:
  - crates/hares-io/src/defaults.rs
  - crates/hares-io/src/config.rs
  - crates/hares-io/src/lib.rs
references:
  - docs/architecture/06-input-output.md
  - docs/architecture/08-operations.md
verification:
  - cargo check -p hares-io
  - cargo test -p hares-io
  - cargo clippy -p hares-io -- -D warnings
---

## Background/Context
Equipment defaults and simulation configuration are the two foundational input concerns that every other component in HARES depends on. Defaults must be loadable from the `defaults/` directory at startup so that equipment specs can be fully resolved before simulation begins. The simulation config struct is the single source of truth for temporal settings, output behaviour, and RNG seeding — it must be deserialised from TOML and have sensible field-level defaults so that minimal config files work correctly.

## Work to Do
- [ ] Implement `defaults.rs`: equipment default parameter loader
  - [ ] Load ZIP parameters from `defaults/zip_parameters.toml` into a typed `ZipParameters` struct (impedance Z, current I, power P fractions; power factor)
  - [ ] Load HVAC biquadratic coefficient sets: scan `defaults/hvac_heating/` and `defaults/hvac_cooling/` directories; key each set by filename stem (equipment type name)
  - [ ] Load defaults from all subdirectories: `defaults/battery/`, `defaults/envelope/`, `defaults/ev/`, `defaults/generator/`, `defaults/loads/`, `defaults/pv/`, `defaults/water_heating/` — each keyed by filename stem. Port OCHRE's `defaults/` directory files: biquadratic coefficients, equipment sizing rules, parameter tables. Document which OCHRE defaults files map to which HARES config entries.
  - [ ] Note: this ticket must be updated whenever a new equipment type is added in Phase 2. It cannot be closed until all Phase 2 equipment tickets (HARES-019 through HARES-032) have confirmed their default entries exist in DefaultsStore.
  - [ ] Provide `DefaultsStore` struct with `load(defaults_dir: &Path) -> Result<DefaultsStore>`
  - [ ] `DefaultsStore::zip_params(&self, equipment_type: &str) -> Option<&ZipParameters>`
  - [ ] `DefaultsStore::hvac_coefficients(&self, equipment_type: &str) -> Option<&BiquadraticCoefficients>`
  - [ ] Return typed errors for missing or malformed TOML files
- [ ] Implement `config.rs`: `SimulationConfig` struct
  - [ ] `start_time: DateTime<Utc>` — simulation start
  - [ ] `duration: Duration` — total simulation length
  - [ ] `time_res: Duration` — timestep size; use a custom serde default function `#[serde(default = "default_time_res")]` returning `chrono::Duration::seconds(60)`; reject zero duration with a validation error in `from_toml()`
  - [ ] `output_verbosity: u8` — 0–8 (default 0); validated explicitly in `from_toml()` (not implicit via serde bounds)
  - [ ] `output_path: Option<PathBuf>` — output file path; `None` means write to current directory with auto-generated name
  - [ ] `output_format: OutputFormat` — enum `Csv | Parquet` (default `Csv` for OCHRE compatibility; Parquet recommended for large datasets)
  - [ ] `output_chunk_size: usize` — rows per Arrow RecordBatch flush (default 10 000)
  - [ ] `master_seed: u64` — RNG seed for reproducibility
  - [ ] Derive `serde::Deserialize`; use `#[serde(default)]` on fields with defaults
  - [ ] `SimulationConfig::from_toml(s: &str) -> Result<SimulationConfig>` — run all validation here: zero `time_res`, out-of-range `output_verbosity`, and `duration.num_seconds() % time_res.num_seconds() == 0` (return Err if not aligned)
  - [ ] `SimulationConfig::timestep_count(&self) -> usize` — computed from duration / time_res
- [ ] Re-export `DefaultsStore`, `SimulationConfig`, `OutputFormat` from `lib.rs`

## Files to Touch
- `crates/hares-io/src/defaults.rs`: new file — `DefaultsStore`, TOML loading for ZIP params and HVAC coefficients
- `crates/hares-io/src/config.rs`: new file — `SimulationConfig`, `OutputFormat`, TOML deserialisation
- `crates/hares-io/src/lib.rs`: re-export new public types

## Measures of Success
- [ ] `DefaultsStore::load` successfully reads ZIP parameters from the real `defaults/zip_parameters.toml` fixture
- [ ] `DefaultsStore::load` successfully loads at least one entry from each of: `battery/`, `envelope/`, `ev/`, `generator/`, `loads/`, `pv/`, `water_heating/` subdirectories when the real `defaults/` tree is present
- [ ] `SimulationConfig::from_toml` with a minimal TOML string (only `start_time` and `duration`) applies all expected defaults including `time_res = 60s`
- [ ] `output_verbosity` outside 0–8 returns a validation error from `from_toml()`
- [ ] `time_res = 0` returns a validation error from `from_toml()`
- [ ] `timestep_count` returns `duration_secs / time_res_secs` with no remainder for aligned inputs
- [ ] `from_toml` with misaligned `duration`/`time_res` (e.g. duration=3601s, time_res=60s) returns Err

## Verification
- [ ] `cargo check -p hares-io` passes
- [ ] `cargo test -p hares-io` passes
- [ ] `cargo clippy -p hares-io -- -D warnings` passes
