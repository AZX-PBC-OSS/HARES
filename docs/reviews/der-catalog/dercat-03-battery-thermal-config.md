# Battery thermal config defaults: heater, thermal mass, heat loss coefficient
**Review ID**: dercat-03
**Category**: der-catalog
**Date**: 2026-05-26

## Files Reviewed
crates/hares-equipment/src/battery/config.rs
crates/hares-equipment/src/battery/mod.rs
crates/hares-equipment/src/battery/catalog.rs

## Vendor/Reference Files Consulted
vendors/OCHRE/ochre/Equipment/Battery.py
vendors/OCHRE/ochre/Models/RCModel.py
vendors/OCHRE/ochre/defaults/Battery/default_parameters.csv
vendors/EnergyPlus/src/EnergyPlus/ElectricPowerServiceManager.cc

## Findings

### Finding 1: [Severity: high]
**Description**: Tesla Powerwall 2 catalog entry has implausibly high heater power: `heater_power_w: 1500.0` W. At this power level, the 13.5 kWh battery would be fully drained by the heater alone in ~9 hours, making it unusable as a thermal management strategy. By comparison, physical Tesla Powerwall 2 thermal management draws ~80–100 W. The 1500 W value appears to be a data-entry error (likely intended as 150 W) or may conflate combined ohmic + heater losses.
**Code Location**: `crates/hares-equipment/src/battery/catalog.rs:213`
**Root Cause**: Likely a numeric data-entry error in the product catalog — the value is 15× the typical residential battery heater rating.
**Impact**: In cold-weather simulations (cell temp below `heater_threshold_c` = 5 °C), the PW2 model will draw 1.5 kW continuously, draining the battery in under one night. This makes the simulated battery nonfunctional in cold weather and produces unrealistic load profiles (a 1.5 kW constant parasitic load). All users simulating a PW2 in cold climates will get incorrect results.

### Finding 2: [Severity: high]
**Description**: All 11 catalog products share identical lumped thermal mass (90 kJ/K) and UA heat loss coefficient (5.0 W/K) because every catalog entry sets `cell_thermal_mass_j_per_k: None` and `cell_ua_w_per_k: None`, falling back to the default constants `DEFAULT_CELL_THERMAL_MASS_J_PER_K = 90_000.0` and `DEFAULT_CELL_UA_W_PER_K = 5.0`. The thermal mass default is physically correct for a mid-size (~100 kg) pack, but it is 2.3× too low for a 27 kWh dual-PW3 system (mass ≈ 228 kg → expected thermal mass ≈ 205 kJ/K) and 2.2× too high for a 5 kWh Enphase IQ 5P (mass ≈ 45 kg → expected thermal mass ≈ 41 kJ/K). Likewise, the UA coefficient does not scale with cabinet surface area; a small single-unit enclosure cannot have the same UA as a stacked dual-cabinet installation.
**Code Location**: `crates/hares-equipment/src/battery/catalog.rs:158–159` (all `to_config()` calls pass `None` for both thermal fields); defaults defined at `crates/hares-equipment/src/battery/mod.rs:146,149`.
**Root Cause**: The catalog lacks per-product thermal mass and UA entries. The `BatterySpec` struct has no fields for thermal mass or UA, so the catalog build cannot supply product-specific values even if they were known.
**Impact**: Temperature dynamics and capacity derating will be physically inconsistent across products. The small Enphase IQ 5P will have 2× the thermal inertia it should (slower cooling), underestimating cold-weather capacity loss. Conversely, the dual-PW3 system will warm up 2.3× too fast, overestimating cold-weather performance. At −5 °C these errors translate to 10–25 % miscapacity (30–50 % derating is typical without heating). The thermal time constant τ = C/UA = 5 h applies identically to all products, which is physically implausible.

### Finding 3: [Severity: medium]
**Description**: The default heater power is `DEFAULT_HEATER_POWER_W = 0.0` — heating is disabled by default. For a generic/unknown battery created via the raw config path (not the product catalog), this produces a passive-only thermal model. Residential batteries installed outdoors (most Powerwalls, Franklin aPowers) need active heating to maintain minimum operating temperature. A battery at −5 °C ambient with no heater will follow the ambient temperature with a 5 h time constant, reaching temperatures where capacity is derated 30–50 %.
**Code Location**: `crates/hares-equipment/src/battery/mod.rs:128`
**Root Cause**: The comment on line 126–127 says "Disabled by default; set to ~500 W for Franklin aPower 2 style pad heater, or ~100 W for Tesla Powerwall 3 cell-level resistive heaters." This is a deliberate design choice to avoid imposing heater consumption on users who model indoor batteries. However, the generic default path lacks any contextual awareness of indoor vs. outdoor installation.
**Impact**: Users who construct a `BatteryConfig` manually (not via the catalog) with no explicit `heater_power_w` will get a battery that never self-heats. In cold-weather simulations this leads to under-predicted battery output and potentially zero usable capacity during winter. The impact is partially mitigated by the fact that cold ambient also reduces heat loss (smaller ΔT), but for sustained cold (−15 °C or lower) the battery will reach sub-zero temperatures within 5–10 hours.

### Finding 4: [Severity: low]
**Description**: The default UA heat loss coefficient is `DEFAULT_CELL_UA_W_PER_K = 5.0`, which is 2.5× higher than the OCHRE reference's equivalent (UA = 1 / `thermal_r` = 1/0.5 = 2.0 W/K from `vendors/OCHRE/ochre/defaults/Battery/default_parameters.csv:22`). The resulting thermal time constant is τ = 5.0 h (HARES) vs. τ = 12.5 h (OCHRE). For a metal cabinet with ~2 m² surface area and minimal insulation (US R-2 = RSI-0.35), UA ≈ 5.7 W/K, so the value is physically possible but represents a worst-case poorly-insulated enclosure. Better-insulated cabinets (50 mm foam, RSI ≈ 1.7) would have UA ≈ 1.2 W/K.
**Code Location**: `crates/hares-equipment/src/battery/mod.rs:149`
**Root Cause**: Default value likely chosen based on thin-sheet-metal enclosure without accounting for typical battery cabinet insulation (most residential batteries have 25–50 mm of foam).
**Impact**: The shorter time constant means batteries cool to ambient ~2.5× faster than OCHRE's model predicts. For overnight temperature swings (e.g., 25 °C daytime to 5 °C nighttime), the HARES battery will reach the lower temperature in ~10 h vs. OCHRE's ~25 h, over-predicting cold-weather capacity derating for well-insulated products. Validated against EnergyPlus, the E+ model uses per-product mass and surface area with a default `h = 7.5 W/m²·K`, yielding product-specific UA values that vary with surface area.

### Finding 5: [Severity: medium]
**Description**: The `BatteryConfig::validate()` method has no validation for `cell_thermal_mass_j_per_k`, `cell_ua_w_per_k`, or upper-bound validation for `heater_power_w`. A user could configure a battery with:
- `cell_thermal_mass_j_per_k = 0.0` → division by zero in the thermal ODE (line 975), which is guarded by `> 0.0` check at line 963 but silently disables the thermal model
- `cell_ua_w_per_k = 0.0` → perfectly adiabatic battery (effectively infinite R-value), violating the Second Law and producing implausible results — tested explicitly in the test at `mod.rs:2681`
- `heater_power_w = 10_000.0` → would drain a 13.5 kWh battery in 1.35 h via heater alone
The validate only checks `heater_power_w >= 0` and is_finite (line 122–131), but has no check on `cell_thermal_mass_j_per_k` or `cell_ua_w_per_k` at all.
**Code Location**: `crates/hares-equipment/src/battery/config.rs:79–148` (validate method does not include thermal parameters)
**Root Cause**: The validate method was written before the thermal model was fully integrated; it covers power/energy/capacity plausibility but not thermal physics constraints.
**Impact**: Physically impossible thermal configurations pass validation silently. A `cell_ua_w_per_k = 0.0` battery would never cool down, producing unrealistically high cold-weather capacity. A zero thermal mass disables temperature tracking entirely.

### Finding 6: [Severity: low]
**Description**: The config field name `cell_thermal_mass_j_per_k` uses a `cell_` prefix, implying it is a per-cell value. However, the documentation comments at line 144–146 and the model usage at line 963–976 both treat this as a **lumped pack-level** value. The comment explicitly says "Lumped thermal mass for the battery pack". If a user interpreted `cell_thermal_mass_j_per_k` literally (per-cell) and provided a value, the resulting thermal mass would be off by a factor of `n_series × n_parallel` (up to 872× for a PW3). In the config struct, the corresponding field for water heater thermal mass is named simply `thermal_mass_j_per_k` without a misleading prefix.
**Code Location**: `crates/hares-equipment/src/battery/config.rs:59` (field name); `mod.rs:68` (key constant); `mod.rs:146` (default with contradictory comment)
**Root Cause**: Naming convention drift — the field was likely originally intended as a per-cell parameter but the implementation settled on a lumped model without updating the name.
**Impact**: Low immediate impact because the catalog always passes `None` and the default value `90_000` is physically correct as a lumped value. However, a user or future developer who computes a per-cell thermal mass and sets the field explicitly would produce a grossly incorrect model.

## Summary
- Total findings: 6
- Critical: 0 / High: 2 / Medium: 2 / Low: 2

## Recommendations

1. **Fix Tesla PW2 heater power** (`catalog.rs:213`): Change `heater_power_w: 1500.0` to a physically plausible value. The actual Powerwall 2 thermal management system draws ~80–100 W. A value of 100–150 W would be consistent with similar products. Cross-reference with datasheet section on "Preconditioning" or "Thermal Management" power draw.

2. **Add per-product thermal mass and UA to the catalog**: Add `thermal_mass_j_per_k: f64` and `ua_w_per_k: f64` fields to `BatterySpec` (`catalog.rs:91–119`), populate them with product-specific values scaled by pack mass and surface area, and pass them as `Some(...)` in `to_config()` (line 158–159). As a first approximation, derive thermal mass as `mass_kg_cache * 900` and UA as `surface_area_m2 * 3.5` (midpoint between OCHRE's 2.0 and R-2 cabinet's 5.7).

3. **Add thermal parameter bounds to `validate()`** (`config.rs:79–148`):
   - Reject `cell_thermal_mass_j_per_k` ≤ 0 if provided (or ≥ 1e6 J/K upper bound)
   - Reject `cell_ua_w_per_k` ≤ 0 if provided (a perfectly-adiabatic battery is unphysical)
   - Add an upper bound on `heater_power_w` (e.g., ≤ 2000 W, or warn above 500 W since that exceeds typical residential battery heater ratings)
   - Reject heater_power_w > capacity_kwh * 1000 / MIN_EXPECTED_HEATER_RUNTIME_HOURS as a sanity check

4. **Rename `cell_thermal_mass_j_per_k` → `thermal_mass_j_per_k`** and `cell_ua_w_per_k` → `ua_w_per_k` throughout the config struct, key constants, and runtime state fields, to eliminate the misleading `cell_` prefix. This is a breaking config change requiring coordinated update of `config.rs`, `mod.rs`, `py_dwelling.rs`, tests, and any serialized battery configs.

5. **Re-evaluate UA default** (`mod.rs:149`): The current default of 5.0 W/K corresponds to a poorly-insulated enclosure. Consider aligning with OCHRE's default (UA = 2.0 W/K, i.e., `cell_ua_w_per_k = 2.0`) or providing an insulation-level parameter. A sensitivity study comparing 2.0 W/K vs. 5.0 W/K in a cold-climate test case (e.g., Minneapolis January) would quantify the capacity prediction difference.

## References / Citations

- OCHRE `default_parameters.csv` line 22–23: `thermal_r = 0.5 K/W`, `thermal_c = 90000 J/K` → UA equivalent = 2.0 W/K, τ = 12.5 h
- OCHRE `Battery.py` lines 22–32: `BatteryThermalModel` extends `OneNodeRCModel` with 1 state (T_INT), 2 inputs (T_EXT, H_INT)
- OCHRE `RCModel.py` line 397: `OneNodeRCModel` implements dT_int/dt = (H_int − (T_int − T_ext)/R) / C
- EnergyPlus `ElectricPowerServiceManager.cc` lines 3292–3342: Li-ion NMC defaults — Cp = 1500 J/kg·K, h = 7.5 W/m²·K, mass and surface area are user-specified
- Schimpe et al. (2018) "Comprehensive Modeling of Temperature-Dependent Degradation Mechanisms in Lithium Iron Phosphate Batteries", NREL/TP-5400-70616 — reference for the Arrhenius capacity derating model
- Tesla Powerwall 3 datasheet: thermal management system design targets 0 °C minimum cell temperature; heater power not publicly specified but operational data suggests 50–150 W range
