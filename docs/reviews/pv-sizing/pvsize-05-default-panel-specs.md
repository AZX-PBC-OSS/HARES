# Default panel wattage 420W and area 2.0 m² — modern residential panel representation
**Review ID**: pvsize-05
**Category**: pv-sizing
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/pv_sizing.rs`
- `crates/hares-equipment/src/pv/mod.rs`
- `crates/hares-equipment/src/pv/array_config.rs`
- `crates/hares-io/src/defaults.rs`
- `crates/hares-python/src/py_pv_sizing.rs`

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/PVWatts.hh`
- `vendors/EnergyPlus/src/EnergyPlus/PVWatts.cc`
- `vendors/OCHRE/ochre/Equipment/PV.py`

## Findings

### Finding 1: Default panel wattage is a reasonable 2024 midpoint but already aging for 2030+ projections
**Severity**: low

**Description**: The compile-time constants `DEFAULT_PANEL_WATTS = 420` and `DEFAULT_PANEL_AREA_M2 = 2.0` (`pv_sizing.rs:90-91`) yield 210 W/m² (21% module efficiency). This is a reasonable midpoint for the 2024 mainstream residential market (400-440W, 1.8-2.2 m², 20-22% efficiency). However, in 2026, high-volume residential panels have pushed to 430-470W (REC Alpha Pure-RX: 470W, Qcells Q.TRON: 430-455W, SunPower Maxeon 7: 450W). For a model intended for 2030+ projections, the default is at the low end of current market and will degrade further as panel wattages continue their ~5W/year trend upward. The OCHRE PV model (`PV.py`) avoids this issue entirely by requiring the user to supply `capacity` directly and delegating module physics to SAM PVWatts v8.

**Code Location**: `crates/hares-physics/src/pv_sizing.rs:90-91`

**Root Cause**: The constants were chosen as a 2024-era midpoint. No mechanism exists to periodically update them based on market data.

**Impact**: Users relying on defaults (the only path available from Python — see Finding 2) will undersize PV arrays relative to current market panels by ~2-12% (420W vs. 430-470W range). For a given usable roof area, this reduces `max_capacity_kw` proportionally. The impact is modest for near-term studies but compounds for 2030+ projections.

### Finding 2: Panel defaults are compile-time constants with no path from defaults store or Python override
**Severity**: medium

**Description**: The panel wattage, area, and system losses are defined as `const` values at `pv_sizing.rs:90-92`:

```rust
const DEFAULT_PANEL_WATTS: u32 = 420;
const DEFAULT_PANEL_AREA_M2: f64 = 2.0;
const DEFAULT_SYSTEM_LOSSES: f64 = 0.14;
```

While the Rust API functions (`compute_usable_area`, `size_pv_system`, `enumerate_pv_candidates`) correctly accept `Option<u32>` and `Option<f64>` override parameters (`pv_sizing.rs:204-205, 347-348, 389-390`), the override path is severed in practice:

1. **Python bindings hardcode `None`**: `py_pv_sizing.rs:229` calls `compute_usable_area(..., None, None)` and `py_pv_sizing.rs:231` calls `size_pv_system(..., None, None, None)`. There is no mechanism for Python users to pass panel specs.

2. **No DefaultsStore integration**: The `DefaultsStore` (`defaults.rs`) has a `defaults/pv/` loading path (`defaults.rs:176`) and a `DefaultsCategory::Pv` enum variant (`defaults.rs:264`), but the `defaults/pv/` directory contains only `.gitkeep`. The `pv_sizing` module has no reference to `DefaultsStore` and reads no configuration files.

3. **No TOML/CSV configuration for PV defaults**: Unlike battery (`default_parameters.csv`), generator (`default_parameters.csv`), and water heating (`default_paramters.csv`), there are no PV default parameter files listing panel wattage, area, noct, or module type.

This means every Python-based simulation uses 420W / 2.0 m² / 14% losses with no user-configurable override. Users cannot model specific panel makes (e.g., 450W REC Alpha vs. 420W Silfab) without modifying and recompiling the Rust source.

**Code Location**: 
- Constants: `crates/hares-physics/src/pv_sizing.rs:90-92`
- Python bindings with hardcoded None: `crates/hares-python/src/py_pv_sizing.rs:229,231`
- Empty defaults/pv directory: `defaults/pv/.gitkeep`

**Root Cause**: The override API was designed but never wired into the Python layer or the defaults loading pipeline. The `defaults/pv/` directory was created as infrastructure scaffolding but never populated.

**Impact**: Users are locked to a single panel specification. Regional markets or specific panel models cannot be modeled. As panel technology advances, the defaults become stale without a configuration update path.

### Finding 3: Temperature coefficient is correctly applied in the production model, but system_losses in sizing does not distinguish thermal derating from other losses
**Severity**: low

**Description**: The review asks whether the panel temperature coefficient (~-0.3 to -0.4%/°C for silicon) is applied in the production model. It is:

- `DEFAULT_GAMMA_PER_C = -0.0047` (`hares-equipment/src/pv/mod.rs:38`) matches PVWatts v8 Standard module type (-0.47%/°C)
- Applied at `mod.rs:254-255`: `let temp_derate = (1.0 + gamma * (cell_temp_c - DEFAULT_T_REF_C)).max(0.0);`
- Cell temperature uses the SAM-NOCT wind-corrected model (`mod.rs:67-77`)
- The three module types use PVWatts v8 gammas: Standard=-0.47%/°C, Premium=-0.35%/°C, ThinFilm=-0.20%/°C (`array_config.rs:32-37`)

The `DEFAULT_SYSTEM_LOSSES` fraction (0.14) in the sizing module lumps all DC-to-AC losses together (soiling, wiring, mismatch, inverter, etc.) but does not explicitly account for temperature derating. This is acceptable because temperature derating is a dynamic, weather-dependent effect applied hourly in the production model, not a static fraction appropriate for sizing. The 14% system losses default matches EnergyPlus PVWatts (`PVWatts.hh:186` defaults `systemLosses = 0.14`).

One subtlety: PVWatts v8 applies `losses` after temperature derating, so the 14% represents losses exclusive of temperature effects. HARES follows the same convention (`mod.rs:256-258`). The separation is correct.

**Code Location**: 
- Temperature coefficient: `crates/hares-equipment/src/pv/mod.rs:38,254-255`
- Cell temperature model: `crates/hares-equipment/src/pv/mod.rs:62-77`
- Module type gammas: `crates/hares-equipment/src/pv/array_config.rs:32-37`
- System losses: `crates/hares-physics/src/pv_sizing.rs:92,352`

**Root Cause**: N/A — the implementation is correct.

**Impact**: No overestimation in hot climates. The temperature derating is properly applied via the SAM-NOCT model with wind correction.

### Finding 4: Packing efficiency is correctly separated into GCR (flat roofs) and usable_fraction (all roofs), but pitched roofs lack inter-panel gap accounting
**Severity**: low

**Description**: The sizing model separates two distinct spatial deductions:

1. **Perimeter setbacks and obstructions**: Applied via `usable_fraction` (`pv_sizing.rs:98-104`) — 75% for gable, 35% for hip, 70% for flat roofs. The comment documents "fire-code setbacks (~15%) and obstruction deductions (~12%)".

2. **Inter-row spacing (flat roofs)**: Applied via `flat_roof_gcr` (`pv_sizing.rs:154-161`) — the Ground Coverage Ratio adjusts the effective panel footprint: `panel_footprint = panel_area_m2 / gcr` (`pv_sizing.rs:313`). GCR values are latitude-dependent (0.35 for >40°, 0.40 for >30°, 0.50 otherwise), matching typical SAM defaults.

3. **Inter-panel gaps (pitched roofs)**: For gable/hip roofs, `panel_footprint = panel_area_m2` (`pv_sizing.rs:315`) — there is no inter-panel gap. This is a reasonable simplification because pitched-roof modules are typically mounted flush with 10-20mm gaps for thermal expansion (negligible relative to the ~2.0 m² module area). However, in practice, some pitched installations also require 36" walkways (fire-code ridge/valley/eave access paths) that may not be fully captured by a single 75%/35% usable fraction for all roof geometries.

The EnergyPlus reference (`PVWatts.hh:191`) also defaults GCR to 0.40, matching HARES. Neither EnergyPlus nor OCHRE model pitched-roof inter-panel gaps explicitly; both delegate to SAM which applies GCR only for flat roof array types.

**Code Location**: 
- Usable fractions: `crates/hares-physics/src/pv_sizing.rs:98-104`
- GCR: `crates/hares-physics/src/pv_sizing.rs:154-161`
- Flat roof panel footprint: `crates/hares-physics/src/pv_sizing.rs:312-313`
- Pitched roof panel footprint: `crates/hares-physics/src/pv_sizing.rs:314-315`

**Root Cause**: The design correctly separates perimeter setbacks from row-spacing, following standard practice. The simplification for pitched roof inter-panel gaps is intentional and defensible.

**Impact**: For pitched roofs, panel count may be marginally overestimated (by 1-2% due to unmodeled thermal expansion gaps). The 75%/35% usable fractions already embed generous conservatism that likely dominates over unmodeled inter-panel gaps.

## Summary
- Total findings: 4
- Critical: 0 / High: 0 / Medium: 1 / Low: 3

## Recommendations

1. **Populate `defaults/pv/` with panel specification files** and add a loading path from `DefaultsStore` to the PV sizing module. A TOML file could define `panel_watts`, `panel_area_m2`, `noct_c`, `module_type`, and `system_losses_fraction` per panel model (e.g., `standard_420.toml`, `premium_450.toml`). The `compute_usable_area` function (or a new builder) should accept a `&DefaultsStore` reference to resolve panel specs.

2. **Expose `panel_watts`, `panel_area_m2`, and `system_losses` in the Python bindings** for `size_pv_from_dwelling` (`py_pv_sizing.rs:219-234`) and `pv_candidates_from_dwelling` (`py_pv_sizing.rs:207-217`). Currently all three functions hardcode `None` for the override parameters despite the Rust API supporting them.

3. **Bump `DEFAULT_PANEL_WATTS` to 440W** to reflect 2026 mainstream panels, and consider documenting the value with its approximate model-year correspondence so maintainers know when to re-evaluate.

4. **Document the separation of concerns** between sizing losses (static 14%, applied as design margin) and production losses (dynamic temperature derating + static 14%, applied hourly). The current implementation is correct but the relationship between `DEFAULT_SYSTEM_LOSSES` in `pv_sizing.rs:92` and `DEFAULT_SYSTEM_LOSSES_FRACTION` in `pv/mod.rs:39` (both 0.14) should be made explicit in comments to avoid future drift.

## References / Citations

- PVWatts v8 Technical Reference: NREL/TP-7A40-80694 — module type temperature coefficients (§2.2), system losses default 14% (§2.5), NOCT cell temperature model (§2.4), GCR defaults (§2.6)
- EnergyPlus PVWatts implementation: `PVWatts.hh:186` defaults `systemLosses = 0.14`; `PVWatts.cc:158-159` delegates module_type to SSC which applies internal temperature coefficients
- OCHRE PV model: `PV.py:53` delegates all module physics to SAM `pvwatts.default("PVWattsNone")`; `PV.py:100-101` requires user-provided `capacity`, `tilt`, `azimuth` — no default panel wattage/area assumptions
- SAM PVWatts v8 SSC defaults: Standard gamma = -0.0047, Premium gamma = -0.0035, ThinFilm gamma = -0.0020
- Lawrence Berkeley National Lab "Tracking the Sun" 2024: median residential panel efficiency ~21.5%, median nameplate ~420-430W
- NREL SAM default module database: 2024 module entries cluster around 400-440W with 1.8-2.2 m² area
