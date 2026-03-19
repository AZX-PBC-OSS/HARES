---
id: HARES-002
title: "hares-types — Environment Types"
kind: implement
depends_on: [HARES-001]
files_to_touch:
  - crates/hares-types/src/environment.rs
  - crates/hares-types/src/lib.rs
references:
  - docs/architecture/02-equipment-and-ports.md
verification:
  - cargo check -p hares-types
  - cargo test -p hares-types
  - cargo clippy -p hares-types -- -D warnings
---

## Background/Context
Equipment models and the envelope solver need a shared, typed representation of the simulation environment at each timestep. This ticket defines the structs that carry zone, weather, grid, and time state into every physics call.

## Work to Do
- [ ] Define `ZoneId(u16)` newtype deriving `Hash, Eq, Copy, Clone, Debug`
- [ ] Define `ZoneState` struct with fields: `id: ZoneId`, `temperature_c: f64`, `humidity_ratio: f64`, `relative_humidity: f64`, `wet_bulb_c: f64`, `volume_m3: f64`
- [ ] Define `SurfaceIrradiance` struct with fields: `surface_id: u32`, `direct_w_m2: f64`, `diffuse_w_m2: f64`, `reflected_w_m2: f64`
- [ ] Define `WeatherState` struct with fields: `outdoor_temp_c: f64`, `outdoor_humidity_ratio: f64`, `wind_speed_m_s: f64`, `wind_dir_deg: f64`, `ground_temp_c: f64`, `sky_temp_c: f64`, `pressure_kpa: f64`, `solar_irradiance: Vec<SurfaceIrradiance>` — do NOT include `ghi_w_m2`, `dni_w_m2`, `dhi_w_m2` (these are EPW parser inputs pre-processed into `solar_irradiance`, not runtime state fields). `wind_dir_deg` is included for completeness and is populated from EPW wind direction data; it is used by directional infiltration models.
- [ ] Define `GridState` struct with fields: `voltage_pu: f64`, `frequency_hz: f64`
- [ ] Define `EnvironmentState` struct with fields: `zones: Vec<ZoneState>`, `weather: WeatherState`, `grid: GridState`, `custom_domains: Vec<DomainUpdate>`, `current_time: DateTime<Utc>`, `time_res: chrono::Duration`
- [ ] Derive `serde::Serialize` and `serde::Deserialize` on all types
- [ ] Re-export from `lib.rs`

## Files to Touch
- `crates/hares-types/src/environment.rs`: new file — all environment state types
- `crates/hares-types/src/lib.rs`: add `pub mod environment` and re-exports

## Measures of Success
- [ ] All environment types compile cleanly
- [ ] `EnvironmentState` can be serialised to and deserialised from JSON
- [ ] `ZoneId` is a distinct newtype, not a raw `u16` alias, and derives `Hash, Eq, Copy, Clone, Debug`
- [ ] `ZoneState` includes `id: ZoneId` field
- [ ] `WeatherState` includes `outdoor_humidity_ratio: f64` and does NOT include `ghi_w_m2`, `dni_w_m2`, or `dhi_w_m2`
- [ ] `EnvironmentState` includes `custom_domains: Vec<DomainUpdate>`
- [ ] `time_res` is typed as `chrono::Duration` (consistent with `DateTime<Utc>`)

## Verification
- [ ] `cargo check -p hares-types` passes
- [ ] `cargo test -p hares-types` passes
- [ ] `cargo clippy -p hares-types -- -D warnings` passes
