# Core state types: EnvironmentState, WeatherState, ZoneState completeness

**Review ID**: arch-02
**Category**: architecture
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-types/src/environment.rs` — Type definitions (775 lines)
- `crates/hares-core/src/environment.rs` — EnvironmentManager (1273+ lines; weather-to-state mapping)

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Models/Envelope.py` — OCHRE Envelope model: `Zone`, `BoundarySurface`, `Boundary`, `Envelope` classes
- `vendors/OCHRE/ochre/Models/Humidity.py` — OCHRE HumidityModel with moisture state fields
- `vendors/EnergyPlus/src/EnergyPlus/Data/DataEnvironment.hh` — EnergyPlus `EnvironmentData` struct (234 lines)
- `vendors/EnergyPlus/src/EnergyPlus/ZoneTempPredictorCorrector.hh` — `ZoneSpaceHeatBalanceData` with zone temperature/humidity state (100–260)
- `vendors/EnergyPlus/src/EnergyPlus/WeatherManager.hh` — `EnvironmentData` weather-period metadata
- `crates/hares-core/src/checkpoint.rs` — `DwellingCheckpoint` schema
- `crates/hares-core/src/dwelling/conversions.rs` — Zone state update from solver outputs

## Findings

### Finding 1: [Severity: medium] ZoneState is missing Mean Radiant Temperature (MRT)

**Description**: `ZoneState` (`environment.rs:80–87`) carries `temperature_c` (air temperature) but has no `mean_radiant_temperature_c` field. Both EnergyPlus and OCHRE distinguish between zone air temperature and mean radiant temperature. EnergyPlus `ZoneSpaceHeatBalanceData` (`ZoneTempPredictorCorrector.hh:107–108`) explicitly tracks `MAT` (Mean Air Temperature) and `MRT` (Mean Radiant Temperature) as separate fields. OCHRE's `Zone` class (`Envelope.py:413–456`) computes radiative exchange through `calculate_interior_radiation()`, which converges on interior surface temperatures — effectively providing MRT to the HVAC controller.

In HARES, interior LWR surface temperatures are tracked internally by the thermal solver (`thermal_solver/mod.rs` + `longwave.rs`) but never exposed on `ZoneState`. Equipment models that need operative temperature (e.g., radiant heating/cooling, thermal comfort assessment) must either re-derive MRT from unavailable surface-state data or fall back to using air temperature alone.

**Code Location**: `crates/hares-types/src/environment.rs:80–87` (ZoneState struct definition)

**Root Cause**: The thermal solver's interior radiation calculation produces surface temperatures internally, but the current solver-to-state interface (`conversions.rs:486–508`) only maps zone temperature + humidity fields back to `ZoneState`. The surface-temperature → MRT aggregation step is not wired.

**Impact**: Equipment models relying on operative temperature (air + radiant average) will underestimate heating/cooling demand in rooms with cold windows or warm radiant surfaces. Thermal comfort calculations will be biased. Future radiant panel, chilled beam, or thermally-activated building system (TABS) equipment cannot be correctly implemented without MRT.

**Severity Justification**: Medium — no currently implemented equipment model in the HARES codebase reads MRT. However, this is a structural gap that constrains future equipment types and diverges from both EnergyPlus and OCHRE conventions.

---

### Finding 2: [Severity: medium] ZoneState relative_humidity and wet_bulb_c are derived fields that can become stale

**Description**: `ZoneState` (`environment.rs:84–85`) carries both `relative_humidity` and `wet_bulb_c` as explicit fields. These fields are updated only when the humidity solver produces an output chunk that `apply_humidity_update_to_zones` (`conversions.rs:486–508`) writes back into `env.zones`. Both are psychrometrically derived from `humidity_ratio` and `temperature_c`.

If zone temperature changes between humidity solver updates (e.g., during multi-pass equipment iteration within a timestep, or if an equipment model directly mutates `temperature_c`), the stored `relative_humidity` and `wet_bulb_c` no longer reflect the current thermodynamic state. There is no invariant check or on-access re-derivation that ensures these fields stay synchronized with `temperature_c`.

A `debug_assert` at `mod.rs:2262–2276` verifies that `humidity_ratio` matches the solver at step start, but no equivalent assertion covers `relative_humidity` or `wet_bulb_c`.

**Code Location**:
- Definition: `crates/hares-types/src/environment.rs:84–85`
- Update site: `crates/hares-core/src/dwelling/conversions.rs:498–500`
- Partial invariant check: `crates/hares-core/src/dwelling/mod.rs:2262–2276`

**Root Cause**: Storing psychrometrically-derived values as independent fields creates a denormalization hazard. OCHRE avoids this by computing `rh` and `wet_bulb` from `w` + `temperature` on every access (`Humidity.py:55–57` — `update_humidity` re-derives all three). EnergyPlus stores them independently but updates them in lockstep via `correctHumRat()` (`ZoneTempPredictorCorrector.hh:241`).

**Impact**: Stale RH and wet-bulb values could cause equipment to make incorrect latent-capacity or condensation-risk decisions if a temperature-only solver pass occurs between humidity updates. Current mitigated by the fact that zone temperature and humidity are typically updated together.

**Severity Justification**: Medium — currently unlikely to manifest in the existing single-zone simulation path where humidity updates follow thermal updates in lockstep, but the denormalization creates a footgun for future multi-pass or multi-zone scenarios.

---

### Finding 3: [Severity: medium] opaque_sky_cover parsed from weather files but absent from WeatherState

**Description**: The HARES weather parsing pipeline (`epw.rs`, `weather.rs`, `resstock_csv.rs`, `tmy3.rs`, `psm3.rs`) reads/writes `opaque_sky_cover` from EPW field 23 (Total Sky Cover in tenths). EnergyPlus surfaces this as `OpaqueCloudCover` and `TotalCloudCover` on `EnvironmentData` (`DataEnvironment.hh:167–168`) and uses it for sky temperature calculation (`WaltonUnstableCloudCorrection`, `BerdahlMartin` model).

In HARES, the EPW parser pre-computes `sky_temp_c` using `opaque_sky_cover` + horizontal infrared radiation (`epw.rs:562–571`). The raw `opaque_sky_cover` value is passed through the `WeatherTimeSeries` but is **never written to `WeatherState`**. Only the computed `sky_temp_c` is surfaced.

The `opaque_sky_cover` field at `epw.rs:71` is read into `WeatherTimeSeries.opaque_sky_cover` (`weather.rs:483`) and used internally for resampling, but `EnvironmentManager::update_in_place` never populates a corresponding field on `WeatherState`.

**Code Location**:
- Parsed: `crates/hares-io/src/epw.rs:71,199,834` and `crates/hares-io/src/weather.rs:483`
- Absent from: `crates/hares-types/src/environment.rs:121–192` (WeatherState definition)
- Not populated: `crates/hares-core/src/environment.rs:637–657` (weather scalar writes)

**Root Cause**: Design choice to pre-compute sky temperature rather than expose intermediate meteorological values. EnergyPlus exposes both so alternative sky models can be used post-hoc.

**Impact**: No ability to implement alternative sky temperature models (e.g., Berdahl-Martin, Clark-Allen with cloud correction factor) at the solver or equipment level without changing the weather pipeline. The current Clark-Allen clear-sky model with Walton cloud correction embedded in `epw.rs` is adequate for most residential simulations.

**Severity Justification**: Medium — the current sky temperature computation is physically correct and matches EnergyPlus defaults, but the architectural limitation prevents future model enhancements without breaking the weather pipeline abstraction.

---

### Finding 4: [Severity: low] EnvironmentState contains non-serializable fields that impede direct JSON save/restore

**Description**: `EnvironmentState` (`environment.rs:321–326`) has two fields marked `#[serde(skip_serializing)]`:
- `equipment_telemetry: HashMap<String, Telemetry>` (line 322)
- `equipment_core: HashMap<EquipmentId, CoreOutput>` (line 326)

These are deliberately excluded from serialization because they are ephemeral per-step aggregations repopulated by the Dwelling. However, this means the `EnvironmentState` serde round-trip test (`environment.rs:414–479`) is incomplete — it constructs these as empty HashMaps and never exercises their content.

The `DwellingCheckpoint` (`checkpoint.rs:15–30`) handles equipment state through separate opaque `equipment_states: Vec<(EquipmentId, Vec<u8>)>` entries using `save_state()`/`load_state()` on each equipment actor. This is architecturally sound for checkpoint/restore but creates a dual-state-encoding pattern: the "published" `EnvironmentState` is not itself sufficient for full simulation state capture.

**Code Location**:
- Skip markers: `crates/hares-types/src/environment.rs:321,326`
- Round-trip test: `crates/hares-types/src/environment.rs:414–479` (empty maps)
- Checkpoint alternative: `crates/hares-core/src/checkpoint.rs:19`
- Restore path: `crates/hares-core/src/dwelling/mod.rs:2036–2044`

**Root Cause**: The state struct serves double duty: as a live per-step "message" between Dwelling phases and as a serde-serializable snapshot. The `skip_serializing` annotation is a pragmatic workaround for varied HashMap value types.

**Impact**: Any tool that serializes `EnvironmentState` directly (e.g., Python bridge, debugging snapshot) will lose equipment telemetry and core outputs. Recovery requires separate equipment state serialization.

**Severity Justification**: Low — the checkpoint system correctly handles separate equipment state serialization, and the `skip_serializing` annotations are well-documented inline.

---

### Finding 5: [Severity: low] No out_door_air_density field in WeatherState

**Description**: EnergyPlus `EnvironmentData` (`DataEnvironment.hh:138`) carries `OutAirDensity` as a computed field. OCHRE's humidity model stores `density` (`Humidity.py:31`) and provides `get_dry_air_density()` as a static method.

HARES `WeatherState` has no `outdoor_air_density_kg_m3` or equivalent field. Consumers that need outdoor air density must recompute it from `outdoor_temp_c`, `outdoor_humidity_ratio`, and pressure themselves. For example, infiltration calculations in `thermal_solver/infiltration.rs:88` call `env.weather.pressure_pa()` and compute density internally.

While density is a derived property (not a weather observation) and can be recomputed consistently, the absence forces every consumer to duplicate the psychrometric call. In OCHRE, density is computed once per timestep in `Envelope.update_infiltration` (`Envelope.py:1212`) and reused by all ventilation paths.

**Code Location**:
- Missing from: `crates/hares-types/src/environment.rs:121–192`
- Recomputations: `crates/hares-envelope/src/thermal_solver/infiltration.rs:88`, heat pump heater uses pressure_pa() but computes density locally

**Root Cause**: Density is treated as a local computation rather than a once-per-step derivation. This is consistent with the existing pattern where `outdoor_wet_bulb_c` and `outdoor_enthalpy_j_kg` ARE pre-computed in `EnvironmentManager::update_in_place` (lines 498–500).

**Impact**: Minor code duplication; negligible performance impact since psychrometric lookups are cheap. Risk of inconsistent density calculations if different components use slightly different psychrometric formulations.

**Severity Justification**: Low — density recomputation is deterministic and cheap. Pre-computing would reduce the risk of formula drift between consumers.

---

### Finding 6: [Severity: low] ZoneState uses f64 for irreducible geometry parameter volume_m3

**Description**: `ZoneState.volume_m3` (`environment.rs:86`) is declared `f64` but represents a fixed building geometry parameter that does not change over the simulation. This is semantically a `const`-like value, not runtime state. In OCHRE, `Zone.volume` (`Envelope.py:443`) is stored similarly but is initialized once and never mutated.

In HARES, `volume_m3` is included in every `ZoneState` instance that is copied into `EnvironmentState.zones` each timestep via `extend_from_slice` (`environment.rs:590`). This causes redundant copying of a constant.

**Code Location**: `crates/hares-types/src/environment.rs:86`

**Root Cause**: Geometry and thermodynamic state are commingled in a single struct. This is pragmatic for serialization but wasteful for memory and serialized size.

**Impact**: Negligible performance cost (one `f64` per zone per timestep). Larger concern for checkpoint files where zone geometry is redundantly included in both `EnvironmentState` and the building definition.

**Severity Justification**: Low — cosmetic overhead with no correctness impact.

---

## Unit Consistency Audit

A systematic check of all state fields against their documented/presumed units was performed:

| Field | Expected Unit | Actual Unit | Consistent? |
|-------|--------------|-------------|-------------|
| `ZoneState.temperature_c` | °C | °C | ✓ |
| `ZoneState.humidity_ratio` | kg/kg | kg/kg | ✓ |
| `ZoneState.relative_humidity` | 0–1 | 0–1 | ✓ |
| `ZoneState.wet_bulb_c` | °C | °C | ✓ |
| `ZoneState.volume_m3` | m³ | m³ | ✓ |
| `WeatherState.outdoor_temp_c` | °C | °C | ✓ |
| `WeatherState.outdoor_humidity_ratio` | kg/kg | kg/kg | ✓ |
| `WeatherState.outdoor_wet_bulb_c` | °C | °C | ✓ |
| `WeatherState.outdoor_enthalpy_j_kg` | J/kg | J/kg | ✓ |
| `WeatherState.wind_speed_m_s` | m/s | m/s | ✓ |
| `WeatherState.wind_dir_deg` | ° | ° | ✓ |
| `WeatherState.ground_temp_c` | °C | °C | ✓ |
| `WeatherState.sky_temp_c` | °C | °C | ✓ |
| `WeatherState.pressure_kpa` | kPa | kPa | ✓ |
| `WeatherState.ghi_w_m2` | W/m² | W/m² | ✓ |
| `WeatherState.dni_w_m2` | W/m² | W/m² | ✓ |
| `WeatherState.dhi_w_m2` | W/m² | W/m² | ✓ |
| `WeatherState.solar_altitude_deg` | ° | ° | ✓ |
| `WeatherState.solar_azimuth_deg` | ° | ° | ✓ |
| `WeatherState.mains_temp_c` | °C | °C | ✓ |
| `WeatherState.rainfall_m` | m | m | ✓ |
| `WeatherState.ground_albedo` | dimensionless | dimensionless | ✓ |
| `WeatherState.ground_t_mean_c` | °C | °C | ✓ |
| `WeatherState.ground_t_amplitude_c` | °C | °C | ✓ |
| `WeatherState.ground_phase_day` | day of year | day of year | ✓ |
| `WeatherState.day_of_year` | day of year | day of year (f64) | ✓ |
| `ElectricalSummary.pv_generation_kw` | kW | kW | ✓ |
| `ElectricalSummary.base_load_kw` | kW | kW | ✓ |
| `ElectricalSummary.net_grid_kw` | kW | kW | ✓ |
| `ElectricalSummary.battery_power_kw` | kW | kW | ✓ |
| `ElectricalSummary.ev_power_kw` | kW | kW | ✓ |
| `GridState.voltage_pu` | per-unit | per-unit | ✓ |
| `GridState.frequency_hz` | Hz | Hz | ✓ |

Note: `WeatherState` stores pressure in kPa but provides the `pressure_pa()` accessor method (`environment.rs:297–301`). This pattern is followed by most consumers, though a few call sites (`coil_physics.rs:238`, `air_conditioner.rs:1087`) convert manually with `* 1000.0`. No kilopascal-to-pascal mismatches were found — all conversion paths are correct.

## Weather Refresh Verification

`EnvironmentManager::update_in_place` (`environment.rs:477–663`) performs a complete refresh of all weather fields on every timestep call. Verified extraction paths:

| Weather Quantity | Source Column | Extraction | Line |
|-----------------|---------------|------------|------|
| Dry-bulb temperature | `WeatherField::DryBulbC` | Direct lookup | 493 |
| Dew-point temperature | `WeatherField::DewPointC` | Direct lookup | 494 |
| Humidity ratio | Derived | `humidity_ratio_from_tdp(dew_point_c, pressure_pa)` | 497 |
| Wet-bulb temperature | Derived | `wet_bulb_from_humidity_ratio(...)` | 498–499 |
| Enthalpy | Derived | `moist_air_enthalpy(...)` | 500 |
| Wind speed | `WeatherField::WindSpeedMS` | Direct lookup | 641 |
| Wind direction | `WeatherField::WindDirDeg` | Direct lookup | 642 |
| Ground temperature | `WeatherField::GroundTempC` | Direct lookup | 643 |
| Sky temperature | `WeatherField::SkyTempC` | Direct lookup | 644 |
| Pressure | `WeatherField::PressureKpa` | Direct lookup | 495 |
| GHI | `WeatherField::GhiWM2` | Direct lookup | 507 |
| DNI | `WeatherField::DniWM2` | Direct lookup | 508 |
| DHI | `WeatherField::DhiWM2` | Direct lookup | 509 |
| Solar position | `solar_position()` | Refreshed each step | 506 |
| Surface irradiance | Perez or solar override | Refreshed each step | 521–561 |
| Mains temperature | `water_mains_temperature_c()` | Refreshed each step | 512–517 |
| Rainfall | `WeatherField::LiquidPrecipM` | Direct lookup | 652 |
| Ground albedo | `WeatherField::SurfaceAlbedo` | Direct lookup | 518 |
| Kusuda-Achenbach params | Pre-computed at init | Copied each step | 654–656 |
| Day of year | `clock.current_time().ordinal()` | Refreshed each step | 657 |

All weather fields are recomputed at step boundaries. No weather field persists across timesteps without refresh. The `solar_irradiance` vector is swap-cycled (`environment.rs:578–581`) so old capacity is reused without reallocation but old values never leak into the next step.

## Checkpoint/Restore Assessment

The `DwellingCheckpoint` struct (`checkpoint.rs:15–30`) captures:

| State Category | Captured? | How |
|---------------|-----------|-----|
| Thermal node temperatures | ✓ | `envelope_state: Vec<f64>` |
| Per-zone humidity ratios | ✓ | `humidity_states: Vec<(ZoneId, f64)>` |
| Fluid solver state | ✓ | `fluid_states: Vec<f64>` |
| Equipment states | ✓ | `equipment_states: Vec<(EquipmentId, Vec<u8>)>` (opaque per-equipment) |
| RNG state (seed + stream + position) | ✓ | `rng_state`, `rng_stream`, `rng_word_pos` |
| Timestep index | ✓ | `timestep_index` |
| Building ID | ✓ | `bldg_id` |
| Last thermal control vector | ✓ | `thermal_last_u` (for corrector step) |
| Exterior LWR surface temps | ✓ | `lwr_t_prev_c: Vec<f64>` |
| Zone air temperature | ✓ | Encoded in `envelope_state` (ODE state vector includes zone air node) |
| Interior LWR surface temperatures | ✓ | Encoded in `envelope_state` (ODE state vector) |
| Weather state | ✗ | NOT checkpointed — deterministically replayed from weather file at each `timestep_index` |
| Clock time (DateTime) | ✗ | NOT checkpointed — derivable from `timestep_index` + start time + step duration |
| Price signal, electrical summary | ✗ | NOT checkpointed — repopulated fresh each step from tariff evaluator and prior solver results |

**Assessment**: Checkpoint/restore is **sufficient** for exact simulation resume. Weather is deterministically replayed from immutable input files, so no weather state needs to be persisted. The solver state vectors capture all continuous-time ODE states. Equipment states are captured through opaque `save_state()`/`load_state()` contracts. Exterior LWR surface temperatures are preserved to maintain corrector-step convergence. The `EnvironmentState` struct itself could not serve as a standalone checkpoint format (due to `skip_serializing` fields and missing internal solver state), but the purpose-built `DwellingCheckpoint` is comprehensive.

## OCHRE Field Coverage Comparison

| OCHRE Concept | HARES Equivalent | Status |
|--------------|-----------------|--------|
| `Zone.temperature` | `ZoneState.temperature_c` | ✓ Present |
| `Zone.capacitance` | Implicit from `volume_m3` × air properties | △ Computable |
| `Zone.volume` | `ZoneState.volume_m3` | ✓ Present |
| `HumidityModel.w` | `ZoneState.humidity_ratio` | ✓ Present |
| `HumidityModel.rh` | `ZoneState.relative_humidity` | ✓ Present |
| `HumidityModel.wet_bulb` | `ZoneState.wet_bulb_c` | ✓ Present |
| `HumidityModel.density` | Not present | ✗ Missing (Finding 5) |
| `HumidityModel.pressure` | `WeatherState.pressure_kpa` | ✓ Present (different struct) |
| `BoundarySurface.temperature` | Internal to thermal solver | △ Not exposed |
| `BoundarySurface.solar_gain` | Internal to thermal solver | △ Not exposed |
| `BoundarySurface.lwr_gain` | Internal to thermal solver | △ Not exposed |
| `Zone.inf_flow` / `inf_heat` | Internal to infiltration module | △ Not exposed |
| `Zone.nat_vent_flow` | Internal to infiltration module | △ Not exposed |
| `Zone.radiation_heat` | Internal to thermal solver | △ Not exposed |
| `Envelope.unmet_hvac_load` | Internal to HVAC equipment | △ Not exposed |
| `Zone.internal_sens_gain` | Passed via equipment control ports | △ Different mechanism |
| `Zone.internal_latent_gain` | Passed via equipment control ports | △ Different mechanism |

The HARES architecture separates concerns differently from OCHRE: OCHRE's `Zone` class is a thick object that carries infiltration, ventilation, radiation, and HVAC state alongside temperature. HARES decomposes these into separate solver modules (`thermal_solver`, `humidity_solver`, equipment models) that communicate through the `EnvironmentState` + domain updates + control signals pattern. This is architecturally cleaner but means `ZoneState` is intentionally leaner than OCHRE's equivalent.

## Summary

- **Total findings**: 6
- **Critical**: 0
- **High**: 0
- **Medium**: 3 (Findings 1, 2, 3)
- **Low**: 3 (Findings 4, 5, 6)

## Recommendations

1. **Add `mean_radiant_temperature_c` to `ZoneState`** — Expose the thermal solver's computed interior surface-temperature average so equipment models can make operative-temperature decisions. This requires the thermal solver to aggregate interior surface temperatures and write them to the zone state output at each timestep.

2. **Make `relative_humidity` and `wet_bulb_c` computed accessors or re-derive on write** — Either remove them as independent fields and compute on access, or add a `#[cfg(debug_assertions)]` invariant check that verifies they are consistent with `temperature_c` and `humidity_ratio` whenever zone state is read. This prevents the denormalization hazard described in Finding 2.

3. **Expose `opaque_sky_cover` on `WeatherState`** — Add an `opaque_sky_cover: f64` field (default 0.0) to `WeatherState` and populate it from `self.weather.get(WeatherField::OpaqueSkyCover, weather_idx)` in `update_in_place`. This enables future alternative sky temperature models without architectural changes.

4. **Pre-compute `outdoor_air_density_kg_m3` in `WeatherState`** — Compute once in `update_in_place` using `psychrometrics::moist_air_density()` and store as a field. This follows the existing pattern of pre-computing `outdoor_wet_bulb_c` and `outdoor_enthalpy_j_kg` and eliminates duplicate psychrometric lookups in infiltration, ventilation, and HVAC consumers.

5. **Document the pressure unit convention in `WeatherState` doc comments** — The field comment at `environment.rs:132` should explicitly note that `pressure_kpa` stores kPa (not Pa), and that consumers should prefer `pressure_pa()` for SI pressure. This mitigates the risk of misreading by new contributors.

6. **Consider separating geometry from runtime state** — Move `volume_m3` out of `ZoneState` into a separate `ZoneGeometry` or building-descriptor struct. This would reduce per-timestep copy overhead and make checkpoint files smaller by eliminating redundant geometry data. Low priority.

## References / Citations

- EnergyPlus Engineering Reference §"External Longwave Radiation" — sky temperature model with cloud cover correction
- Kusuda, T. & Achenbach, P.R. (1965). "Earth Temperature and Thermal Diffusivity at Selected Stations in the United States." *ASHRAE Transactions* 71(1):61–74 — ground temperature model
- Burch, J. & Christensen, C. (2007). "Towards Development of an Algorithm for Mains Water Temperature." *ASHRAE Transactions* 113(1) — water mains temperature model
- ASHRAE Handbook of Fundamentals (2021), Chapter 1: Psychrometrics — humidity ratio, wet-bulb, and enthalpy derivations
- EnergyPlus I/O Reference: EPW Weather Data Format — fields 7 (dry-bulb), 8 (dew-point), 9 (rel humidity), 15 (GHI), 16 (DNI), 17 (DHI), 20 (wind speed), 21 (wind direction), 23 (opaque sky cover), 33 (liquid precipitation)
