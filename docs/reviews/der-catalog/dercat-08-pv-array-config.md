# PvArrayConfig: multi-array geometry, module count, inverter, irradiance
**Review ID**: dercat-08
**Category**: der-catalog
**Date**: 2026-05-26

## Files Reviewed
crates/hares-equipment/src/pv/array_config.rs
crates/hares-equipment/src/pv/config.rs
crates/hares-equipment/src/pv/mod.rs
crates/hares-equipment/src/pv/shading.rs
crates/hares-physics/src/solar.rs
crates/hares-core/src/environment.rs

## Vendor/Reference Files Consulted
vendors/OCHRE/ochre/Equipment/PV.py
vendors/EnergyPlus/src/EnergyPlus/Photovoltaics.hh
vendors/EnergyPlus/src/EnergyPlus/Photovoltaics.cc

## Findings

### Finding 1: [Severity: critical]
**Description**: Multi-array PV systems cannot be configured. `PvConfig` (the typed configuration path) creates exactly one `PvArray` in `init_typed()` (mod.rs:358-367), assigning the full system `capacity_kw` to that single array. The `PvConfig` doc comment (config.rs:9-10) states "Multi-array systems continue to use the raw config path via `array_count`," but raw configs are explicitly rejected with `"PV requires typed config; raw config is unsupported"` (mod.rs:133-134). There is no `array_count` field in `PvConfig` and no mechanism to define per-array tilt, azimuth, or capacity through either configuration path. The `PV` struct holds `arrays: Vec<PvArray>` and the `step()` method correctly iterates over all arrays, but the infrastructure to populate multiple arrays from configuration is absent.
**Code Location**: `crates/hares-equipment/src/pv/mod.rs:128-137` (new rejects raw config), `mod.rs:358-367` (init_typed creates single array), `crates/hares-equipment/src/pv/config.rs:9-10` (doc comment references non-existent raw multi-array path)
**Root Cause**: The typed config path was implemented for single-array residential systems only. The raw config fallback that was supposed to handle multi-array was removed (or never implemented), but the doc comment and `Vec<PvArray>` storage were not updated to reflect this. The `PvConfig` struct lacks fields for `array_count` or a `Vec<PvArray>` — it only has `tilt_deg`, `azimuth_deg`, and `capacity_kw` as singular values.
**Impact**: Users cannot model residential installations with arrays on multiple roof faces (e.g., south-facing + west-facing). This is a common real-world configuration. The `multi_array_sums_outputs` test (mod.rs:1004-1069) verifies the summation logic works, but only by mutating `pv.arrays` directly after init — there is no end-to-end configuration path for this scenario. EnergyPlus handles this by treating each `Generator:Photovoltaic` as an independent array with its own surface (Photovoltaics.cc:337-340), and OCHRE creates one PV object per orientation (PV.py:74-161). HARES has the multi-array execution engine but no way to configure it.

### Finding 2: [Severity: critical]
**Description**: `PvArray` fields `tilt_deg` and `azimuth_deg` have no validation. The `PvArray` struct (array_config.rs:42-43) defines them as plain `f64` without any range constraints, no defaults, and no `validate()` method. While `PvConfig::validate()` (config.rs:58-71) checks tilt ∈ [0, 180] and azimuth ∈ [0, 360), this validation only covers arrays created through the typed config path. If arrays are set via test code, programmatic construction, or a future multi-array config path, the geometry fields have zero guardrails. Even in the current single-array path, the per-array `tilt_deg`/`azimuth_deg` values in the `PvArray` struct are set from `c.tilt_deg.unwrap_or(30.0)` and `c.azimuth_deg.unwrap_or(180.0)` — the defaults are applied in `init_typed` (mod.rs:343-344) rather than in `PvConfig::validate()`, so validation of defaults is redundant with the struct-level validation but the per-array storage itself has no invariant enforcement.
**Code Location**: `crates/hares-equipment/src/pv/array_config.rs:41-44` (PvArray without validation or Default impl), `crates/hares-equipment/src/pv/config.rs:58-71` (PvConfig validation that doesn't transfer to PvArray)
**Root Cause**: `PvArray` is a data struct without behavioral methods. Validation logic lives on `PvConfig` but is not re-invoked on the resulting `PvArray`. If future code constructs `PvArray` values directly, invalid geometry (NaN, Inf, negative tilt, azimuth > 360) will propagate into the step function and produce silently wrong cell temperature and power calculations.
**Impact**: Currently low because the only production code path goes through `PvConfig::validate()`. However, this is a latent invariant violation risk for any future multi-array initialization or programmatic construction. If invalid tilt/azimuth reach `surface_id_for_orientation()` (array_config.rs:78-100), they will produce a valid-looking surface_id (due to `clamp` and `normalize_azimuth`) but may silently map to the wrong pre-computed irradiance entry.

### Finding 3: [Severity: high]
**Description**: No module count or module-level power tracking. The `PvArray` struct uses `capacity_kw` (array_config.rs:44) as its DC rating with no `module_count`, `module_rated_power_w`, or module-level granularity. There is no verification that total system capacity Σ(array.capacity_kw) equals the declared `PvConfig.capacity_kw`. Since `init_typed()` assigns the full `c.capacity_kw` to the single array (mod.rs:361), this check would be trivially satisfied for the single-array case, but there's no verification mechanism at all. In contrast, EnergyPlus tracks `NumSeriesNParall` and `NumModNSeries` (Photovoltaics.cc:396-397) — the total number of modules and their series/parallel arrangement — and uses module-level parameters (Isc0, Vmp0, etc.) to scale to array-level behavior. OCHRE PV also uses total capacity but always validates that capacity is positive (PV.py:100,132-133).
**Code Location**: `crates/hares-equipment/src/pv/array_config.rs:41-44` (no module_count field), `crates/hares-equipment/src/pv/mod.rs:361` (capacity assignment without verification)
**Root Cause**: HARES uses a PVWatts-inspired simplified model where only total DC capacity and a temperature coefficient matter. Module count is not needed for the simple linear power model. However, the absence means users cannot cross-reference their module specs (e.g., "20 × 400W modules = 8 kW DC") against the simulation.
**Impact**: Medium. The model is mathematically correct for a single array. Cannot validate that declared system capacity matches module configuration, making input errors harder to catch. A user entering `capacity_kw: 5.0` with no visibility into whether that's 10 × 500W, 12 × 415W, or a typo would not receive a configuration-time sanity check.

### Finding 4: [Severity: high]
**Description**: No per-array inverter association. Each `PvArray` has no inverter field (array_config.rs:41-51). The inverter is a single system-level entity: `inverter_efficiency` and `inverter_capacity_kw` are fields on the `PV` struct (mod.rs:106-108), applied uniformly in `step()` (mod.rs:533-534) via `apply_inverter_limits()`. This means all arrays share one inverter with a single capacity limit, preventing modeling of per-string microinverters, string-level inverters, or any mixed DC/AC ratio scenario where different arrays have different inverter sizes. The DC/AC ratio (`total_dc_kw / inverter_capacity_kw`) is never explicitly computed or validated — if `inverter_capacity_kw` is `None` (the default, mod.rs:108), no clipping ever occurs regardless of array DC size.
**Code Location**: `crates/hares-equipment/src/pv/array_config.rs:41-51` (no inverter field on PvArray), `crates/hares-equipment/src/pv/mod.rs:106-108` (system-level inverter), `crates/hares-equipment/src/pv/mod.rs:269-321` (inverter limits applied to aggregate power)
**Root Cause**: Design assumes single inverter per PV system, which is valid for most residential installations (one string inverter). But the multi-array architecture demands per-array inverter awareness for microinverter setups and for correctly computing the DC-to-AC ratio. OCHRE PV.py (line 122) defaults `inverter_capacity` to `capacity` if not specified. HARES defaults to `None` (no clipping).
**Impact**: For single-array residential systems: low impact if `inverter_capacity_kw` is explicitly set. If it's left `None`, the system never clips — producing unrealistically high AC output during cold, high-irradiance conditions where a real inverter would saturate. For multi-array systems: inability to model per-string or per-array inverter configurations means microinverter architectures (common in residential with multiple roof faces) cannot be accurately simulated.

### Finding 5: [Severity: low]
**Description**: Irradiance calculation is correctly per-array. Each `PvArray` gets a unique `surface_id` derived from its tilt and azimuth via `surface_id_for_orientation()` (mod.rs:394-398). During `step()`, each array independently looks up its `SurfaceIrradiance` by its own `surface_id` (mod.rs:494-504). The `perez_tilted_irradiance()` function in solar.rs (line 393) is called per-surface in the environment pipeline (environment.rs:548-559), using each surface's own tilt and azimuth to compute beam (DNI·cos(aoi)), diffuse (Perez anisotropic), and ground-reflected components independently. This is the correct approach. Beam radiation directionality is preserved because the angle of incidence is computed per-surface using each surface's tilt and azimuth (solar.rs:417-422). The error described in the review brief — computing POA once with an average orientation — does NOT occur in this codebase.
**Code Location**: `crates/hares-equipment/src/pv/mod.rs:488-512` (per-array iteration), `crates/hares-physics/src/solar.rs:393-483` (perez_tilted_irradiance with per-surface geometry), `crates/hares-core/src/environment.rs:526-561` (per-surface irradiance population)
**Root Cause**: N/A (finding confirms correctness)
**Impact**: N/A (no defect)

### Finding 6: [Severity: high]
**Description**: Shading model is not per-array. `ShadingModel` is a single field on the `PV` struct (mod.rs:120), parseable from the equipment config, but `PvArray` has no shading-related fields. In `step()`, a single `shading_factor` is computed from the system-level `shading_model` (mod.rs:476-480), and this same value is passed to every `step_one_array()` call regardless of the array's orientation. For multi-array systems on different roof faces, shading is orientation-dependent: a tree due east will shade east-facing arrays in the morning but not south-facing arrays at noon. The current design applies the same shading reduction to all arrays uniformly, which is physically incorrect for obstruction/horizon-based shading models where the geometric relationship between sun position and obstruction matters.
**Code Location**: `crates/hares-equipment/src/pv/mod.rs:120` (system-level shading_model), `mod.rs:476-480` (single shading_factor applied to all arrays), `crates/hares-equipment/src/pv/array_config.rs:41-51` (no shading field on PvArray)
**Root Cause**: Shading was implemented as a system-level concern transparent to the per-array iteration. The `shading_factor()` method (shading.rs:42-70) takes solar position parameters which are array-independent (sun altitude/azimuth are global), but obstruction models like `ObstructionAngle` (shading.rs:55-65) and `HorizonProfile` (shading.rs:66-68) are inherently direction-dependent — the sun's position relative to an obstruction depends on the array's azimuth. Even `FixedLoss` could reasonably differ between slopes (e.g., south-facing accumulates snow differently).
**Impact**: For single-array systems with FixedLoss shading: correct. For multi-array systems with obstruction or horizon shading: the shading factor is applied to all arrays regardless of whether the array's orientation actually faces the obstruction. An east-facing array would be shaded by a west-facing obstruction factor, or vice versa. In the current state (only single-array configurable), the impact is theoretical but would become real once multi-array configuration is enabled.

## Summary
- Total findings: 6
- Critical: 2 / High: 3 / Medium: 0 / Low: 1

## Recommendations
1. **Enable multi-array configuration**: Either expand `PvConfig` to include a `Vec<PvArray>` or add an `array_count` + per-array geometry fields. Alternatively, allow multiple `PvConfig` entries under the same PV equipment with array-level capacity splitting. Remove the stale doc comment about raw config multi-array support (config.rs:9-10) since raw configs are explicitly rejected.
2. **Add `PvArray::validate()`**: Constrain `tilt_deg ∈ [0, 180]`, `azimuth_deg ∈ [0, 360)`, `capacity_kw > 0`, `noct_c` finite and positive. Call during initialization for every array, not just via `PvConfig::validate()`.
3. **Add module count field**: Add `module_count: u32` and `module_rated_power_w: f64` to `PvArray` (or keep just one derived capacity_kw but validate consistency). Verify `Σ(array.module_count * module_rated_power_w) = system_capacity_kw` within tolerance.
4. **Add per-array inverter or make DC/AC ratio explicit**: Either add `inverter_capacity_kw: Option<f64>` to `PvArray` for per-array microinverter support, or compute and log the system-level DC/AC ratio in `init_typed()`. Default `inverter_capacity_kw` to `capacity_kw` (matching OCHRE) rather than `None` to ensure realistic clipping behavior even when the user omits it.
5. **Make shading per-array**: Add `shading_model: Option<ShadingModel>` to `PvArray`. In `step_one_array()`, compute shading_factor from the array's shading model (if present) or fall back to the system-level model. This is essential for obstruction and horizon-based shading where directionality matters.
6. **No changes needed for irradiance**: The per-surface Perez transposition in the environment pipeline is correctly computing independent POA irradiance for each array's unique orientation. This design is correct and should be preserved.

## References / Citations
- EnergyPlus `Photovoltaics.cc:337-340`: Each `Generator:Photovoltaic` references its own `SurfacePtr` for independent irradiance/temperature computation.
- EnergyPlus `Photovoltaics.cc:396-397`: Tracks `NumSeriesNParall` and `NumModNSeries` — module count and arrangement are explicit inputs.
- OCHRE `PV.py:74-161`: Single PV object per orientation; multi-array requires multiple PV equipment instances.
- OCHRE `PV.py:122`: `self.inverter_capacity = inverter_capacity or self.capacity` — inverter defaults to DC capacity.
- Perez et al. (1990) "An anisotropic hourly diffuse radiation model for sloping surfaces." Solar Energy 44(5):271-289 — the per-surface irradiance model used in `solar.rs:393-483`.
- PVWatts v8 Technical Reference, NREL/TP-7A40-80694 — the basis for the SAM-NOCT cell temperature model and module type gamma values.
