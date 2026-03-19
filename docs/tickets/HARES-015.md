---
id: HARES-015
title: "hares-envelope — Humidity Solver"
kind: implement
depends_on: [HARES-014, HARES-005]
# HARES-014 dependency: ThermalSolver produces the infiltration flow rate and updated
# zone temperatures that HumiditySolver consumes. HumiditySolver must run after
# ThermalSolver in the same timestep.
files_to_touch:
  - crates/hares-envelope/src/humidity_solver.rs
  - crates/hares-envelope/src/lib.rs
references:
  - vendors/OCHRE/ochre/Models/Humidity.py
  - docs/architecture/01-sim-core-and-solver.md
verification:
  - cargo check -p hares-envelope
  - cargo test -p hares-envelope
  - cargo clippy -p hares-envelope -- -D warnings
---

## Background/Context

Zone humidity state is advanced in parallel with zone temperature each timestep. Moisture is added by latent gains from HVAC, infiltration/ventilation, and occupancy/appliance loads; it is not removed unless a dehumidifier or cooling equipment operates. This is a Rust reimplementation of OCHRE's `Humidity` model (vendors/OCHRE/ochre/Models/Humidity.py, 65 lines) with improvements: the moisture buffering multiplier is configurable (OCHRE hardcodes 1.0) and the physical clamp is explicit. Depends on HARES-005 for psychrometric functions (`humidity_ratio_to_rh`, `rh_to_wet_bulb`, `saturation_humidity_ratio`).

## Work to Do

- [ ] Define `HumiditySolverConfig` struct with fields: `zone_volume_m3: f64`, `moisture_buffering_multiplier: f64` (default `1.0`), `h_fg_j_kg: f64` (latent heat of vaporisation at ~20°C, default `2_450_000.0`)
  - Air density must be computed from `WeatherState::pressure_kpa` and zone temperature using `moist_air_density_kg_m3` from HARES-005. Do NOT use a hardcoded default of `1.2` — this re-introduces OCHRE's altitude bug.
- [ ] Define `HumiditySolver` struct implementing `DomainSolver` (from HARES-014):
  - Owns `config: HumiditySolverConfig` and current `humidity_ratios: HashMap<ZoneId, f64>` (one entry per zone)
  - `domain_id()` returns `HUMIDITY` (`DomainId(2)`)
  - `resolve()`:
    1. Sum `latent_gain_w` from all `PortSlots::thermal` entries for each zone. The infiltration latent gain is NOT recomputed here: ThermalSolver computes the infiltration flow rate and encodes the resulting per-zone latent gains as `[(zone_id_as_f64, latent_w), ...]` pairs in `DomainUpdate.custom_payload`. The engine populates `env.custom_domains` with prior solver outputs within the same timestep, so HumiditySolver reads these values by finding the `DomainUpdate` with `domain_id == THERMAL` in `env.custom_domains` and extracting its `custom_payload`.
    2. Use the UPDATED zone temperature from ThermalSolver's `DomainUpdate` for the current timestep: find the THERMAL entry in `env.custom_domains` and read `zone_temperatures_c` from it. Do NOT use `env.zones[*].temperature_c`, which holds the previous-step value. HumiditySolver runs after ThermalSolver within the same envelope resolution phase.
    3. Compute `dW = (Q_latent * dt_s) / (h_fg * rho_air * V_zone * moisture_buffering_multiplier)`
    4. Update `humidity_ratios[zone_id] += dW`
    5. Clamp `humidity_ratios[zone_id]` to `[0.0, saturation_humidity_ratio(zone_temp_c, pressure_kpa)]`
    6. Derive `relative_humidity` and `wet_bulb_c` via hares-physics psychrometric functions
    7. Return `DomainUpdate` with updated zone humidity state
- [ ] Expose `HumiditySolver::humidity_ratio(&self, zone_id: ZoneId) -> f64` for telemetry

## Files to Touch

- `crates/hares-envelope/src/humidity_solver.rs`: new file — `HumiditySolver`, `HumiditySolverConfig`
- `crates/hares-envelope/src/lib.rs`: add `pub mod humidity_solver` and re-export public types

## Measures of Success

- [ ] Known latent gain: `Q_latent = 100 W`, `V_zone = 200 m³`, `rho = 1.2 kg/m³`, `h_fg = 2_450_000 J/kg`, `moisture_buffering = 1.0`, `dt = 60 s` — computed `dW` matches `(100 * 60) / (2_450_000 * 1.2 * 200 * 1.0)` to within 1e-12
- [ ] Per-timestep moisture balance assertion passes: `|Δm_water - Σ(Q_latent·dt/h_fg)| < 1e-6 kg` where `Δm_water = ΔW_zone · ρ_air · V_zone` (per arch doc 07-testing invariant)
- [ ] After applying the `dW`, `relative_humidity` computed from the updated `humidity_ratio` and zone temperature is consistent with the psychrometric identity used by hares-physics (round-trip within 0.1% RH)
- [ ] When `dW` would push `humidity_ratio` above saturation, it is clamped to saturation without panic or `NaN`
- [ ] When `dW` is negative and would push `humidity_ratio` below zero, it is clamped to `0.0`
- [ ] `moisture_buffering_multiplier = 2.0` halves the humidity swing relative to `1.0` for the same latent load — verified by running both configs and comparing `dW`

## Performance Notes
- **P2 — HumiditySolver buffers**: `zone_temp_buf` and `latent_buf` are `HashMap` fields on `HumiditySolver`, pre-allocated at construction and `.clear()`ed + reused each step. Avoids per-step `HashMap` allocation for zone temperature lookups and latent gain accumulation.
- Pattern follows the general buffer-reuse strategy documented in HARES-012 Performance Notes.

## Verification

- [ ] `cargo check -p hares-envelope` passes
- [ ] `cargo test -p hares-envelope` passes
- [ ] `cargo clippy -p hares-envelope -- -D warnings` passes
