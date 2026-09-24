# Unit consistency across crate boundaries: W, kW, C, kg/s, m3, Pa, kPa
**Review ID**: wiring-08
**Category**: wiring
**Date**: 2026-05-26

## Files Reviewed
All crates

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/utils/units.py`
- `vendors/EnergyPlus/src/EnergyPlus/Data/` (via EnergyPlus reference values embedded in codebase)

## Findings

### Finding 1: Mixed W/kW within PortContribution enum [Severity: medium]
**Description**: The `PortContribution` enum mixes unit scales within its variants. `Thermal` fields use `sensible_gain_w` (W), `Fuel` uses `consumption_w` (W), while `Electrical` uses `active_power_kw` (kW). This is documented with suffix naming convention, but creates mandatory `* 1_000.0` or `/ 1_000.0` conversions at every integration point between equipment and the electrical port accumulator.

**Code Locations**:
- `crates/hares-types/src/ports.rs:69` — `active_power_kw: f64`
- `crates/hares-types/src/ports.rs:64` — `sensible_gain_w: f64`
- `crates/hares-types/src/ports.rs:74` — `consumption_w: f64`
- `crates/hares-equipment/src/event_load.rs:383-384` — `fuel_consumption_w = active_power_kw * 1_000.0`
- `crates/hares-equipment/src/event_load.rs:391` — `gain_source_w = active_power_kw * 1_000.0`
- `crates/hares-equipment/src/event_load.rs:421-424` — telemetry mixes `ACTIVE_POWER_KW` with `SENSIBLE_GAIN_W`

**Root Cause**: The kW scale was chosen for electrical to match utility metering and power system convention (kW), while thermal is conventionally in W (per HPXML and EnergyPlus convention). This split is pragmatic but forces every electrical→thermal bridge point to perform explicit conversion.

**Impact**: Any call site that accidentally passes a kW value to a `_w`-suffixed field (or vice versa) would be off by factor 1000. Currently mitigated by suffix naming convention and the lack of automated dimensional analysis (no `uom` typed wrapper on `PortContribution` — the fields are raw `f64`).

---

### Finding 2: Pressure stored as kPa, converted to Pa inline at every call site [Severity: medium]
**Description**: `WeatherState.pressure_kpa` stores atmospheric pressure in kPa, but all psychrometric functions (`humidity_ratio_from_tdp`, `wet_bulb_from_humidity_ratio`, `dew_point`, `moist_air_density_kg_m3`, `saturation_pressure_pa`) expect pressure in Pa. A convenience method `WeatherState::pressure_pa()` exists at `environment.rs:298`, but many call sites use the raw `env.weather.pressure_kpa * 1000.0` multiplication pattern instead.

**Code Locations**:
- `crates/hares-types/src/environment.rs:132` — `pressure_kpa: f64`
- `crates/hares-types/src/environment.rs:298-300` — `fn pressure_pa() -> f64 { self.pressure_kpa * 1000.0 }` (correct converter)
- `crates/hares-core/src/environment.rs:496` — `pressure_pa = pressure_kpa * 1000.0` (manual conversion, correct)
- `crates/hares-equipment/src/hvac/air_conditioner.rs:1087` — `env.weather.pressure_kpa * 1000.0` (manual, correct)
- `crates/hares-equipment/src/hvac/air_conditioner.rs:1105` — `env.weather.pressure_kpa * 1000.0` (manual, correct)
- `crates/hares-physics/tests/physics_validation_tests.rs:1524` — `w.pressure_pa()` (correct, uses wrapper)
- `crates/hares-envelope/src/thermal_solver/mod.rs:1442` — `p_kpa * 1000.0` (manual, correct)
- `crates/hares-equipment/src/hvac/heat_pump/heater.rs:975` — `env.weather.pressure_pa()` (correct, uses wrapper)
- `crates/hares-equipment/src/hvac/heat_pump/defrost.rs:363` — `pressure_pa: f64` parameter docstring says `[Pa]`, called at `heater.rs:992` with `pressure_pa` from `pressure_pa()`. Consistent.

**Root Cause**: The storage format (kPa) was chosen for human readability in config/JSON (101.325 vs 101325), but all computational APIs require Pa. The inline `* 1000.0` pattern creates a risk that a new call site could pass `pressure_kpa` directly to a function expecting Pa, producing a factor-1000 error in the denominator of `W = 0.622 * p_v / (p - p_v)`.

**Impact**: A missed kPa→Pa conversion would cause approximately 0.3% error in humidity ratio at standard conditions (since `p >> p_v`), but a catastrophic error in moist air density (directly proportional to pressure). The saturated vapor pressure term `p_ws` is pressure-independent, but the denominator `(p_pa - p_ws)` would shrink by factor ~1000, producing absurdly high humidity ratios. Noting that the `pressure_pa()` method is already available on `WeatherState`, the fix is trivial: standardise all call sites to use it.

---

### Finding 3: Implicit Celsius-to-Kelvin conversion with hardcoded 273.15 [Severity: low]
**Description**: The codebase uses `CELSIUS_TO_KELVIN = 273.15` in `constants.rs`, which is the authoritative constant for °C→K conversion. It is consistently applied in the longwave radiation solver and `air_properties.rs`. However, `psychrometrics.rs:48` and `battery/mod.rs:211,1024,1033` use hardcoded `273.15` rather than the named constant.

**Code Locations**:
- `crates/hares-physics/src/psychrometrics.rs:48` — `let t_k = t_c + 273.15;`
- `crates/hares-equipment/src/battery/mod.rs:211` — `let t_k = cell_temp_c + 273.15;`
- `crates/hares-equipment/src/battery/mod.rs:1024` — `let cell_temp_k = self.cell_temp_c + 273.15;`
- `crates/hares-equipment/src/battery/mod.rs:1033` — `let cell_temp_k = self.cell_temp_c + 273.15;`
- `crates/hares-equipment/src/ev/mod.rs:578` — `let cell_temp_k = self.battery_temp_c + 273.15;`

**Root Cause**: Local code style preference; these files were likely written before `CELSIUS_TO_KELVIN` was established as the canonical constant, or the author chose the literal for performance/locality.

**Impact**: The hardcoded value happens to match the constant exactly (273.15 = 273.15), so there is no numerical error. Risk is maintainability: if the constant definition were ever updated (e.g., for a more precise value), these sites would diverge silently.

---

### Finding 4: Legacy DEFROST_CAPACITY_UNIT_FACTOR masks EnergyPlus unit scaling [Severity: low]
**Description**: The constant `DEFROST_CAPACITY_UNIT_FACTOR = 1.01667` at `heat_pump/constants.rs:15` divides rated capacity in the defrost power and Q_defrost equations. The accompanying comment on line 16-19 documents that OCHRE had a legacy "# in kW" comment on this value, but dimensional analysis proved it is in W. The factor 1.01667 has no direct physical meaning — it normalises capacity to the EnergyPlus reference capacity scale.

**Code Locations**:
- `crates/hares-equipment/src/hvac/heat_pump/constants.rs:15` — `pub const DEFROST_CAPACITY_UNIT_FACTOR: f64 = 1.01667;`
- `crates/hares-equipment/src/hvac/heat_pump/constants.rs:16-19` — comment documenting the OCHRE kW confusion
- `crates/hares-equipment/src/hvac/heat_pump/defrost.rs:397` — `(rated_capacity_w / DEFROST_CAPACITY_UNIT_FACTOR)`
- `crates/hares-equipment/src/hvac/heat_pump/defrost.rs:404` — `(post_defrost_cap_w / DEFROST_CAPACITY_UNIT_FACTOR)`
- `crates/hares-equipment/src/hvac/heat_pump/heater.rs:1428` — `(max_capacity_w / DEFROST_CAPACITY_UNIT_FACTOR)`

**Root Cause**: EnergyPlus's defrost model was originally formulated with non-SI capacity units. The factor 1.01667 was introduced to scale capacity into the reference frame expected by the EnergyPlus empirical coefficients (`DEFROST_EIR_TEMP_MODIFIER`, `DEFROST_Q_MULTIPLIER`, etc.).

**Impact**: The factor is applied correctly throughout the heat pump defrost code. The risk is that the unit scale of `DEFROST_CAPACITY_UNIT_FACTOR` is a vestigial normalisation constant with no independent physical meaning — it functions correctly only because the surrounding empirical coefficients (`DEFROST_EIR_TEMP_MODIFIER = 0.1528`, `DEFROST_Q_MULTIPLIER = 0.01`) are calibrated against it. Changing one without the others would silently break defrost physics. The OCHRE "# in kW" comment is a warning about how easily these calibrations can be misinterpreted.

---

### Finding 5: Undocumented magic numbers in annual fan energy computation [Severity: low]
**Description**: `resolve_loads.rs` computes annual ceiling fan energy with the expression `n_fans * 3000.0 / efficiency_cfm_per_w * 10.5 * 365.0 / 1000.0`. The values 3000.0 (operating hours per year?) and 10.5 (hours per day?) are not documented. These are effective annual hour products used as unit conversions from cfm/W to kWh/yr.

**Code Locations**:
- `crates/hares-io/src/hpxml/resolve_loads.rs:498` — `let annual_kwh = n_fans * 3000.0 / efficiency_cfm_per_w * 10.5 * 365.0 / 1000.0;`

**Root Cause**: This appears to be a direct translation from the OCHRE or ResStock reference implementation without explanatory comments. The 3000.0 likely represents rated hours per year for ceiling fans (per ASHRAE 90.2 base schedule), and 10.5 is a usage-hours-per-day multiplier.

**Impact**: The numbers are stitched into an energy computation without documentation. If a future maintainer changes them, they cannot determine whether the change is valid without reverse-engineering the OCHRE reference. The computed annual kWh appear to be used only for peak power sizing in `schedule_resolve.rs:784-813`, so errors affect peak demand estimates rather than physics.

---

### Finding 6: Temperature consistently in Celsius across all crates [Severity: none — verified correct]
**Description**: All temperature fields across crate boundaries use `_c` suffix and represent degrees Celsius. Verified at:
- `ZoneState::temperature_c` (`environment.rs:82`)
- `WeatherState::outdoor_temp_c` (`environment.rs:122`)
- `PortContribution::Fluid::supply_temp_c` (`ports.rs:79`)
- `FluidAccumulator::mean_supply_temp_c` (`ports.rs:326`)
- All psychrometric functions take `t_db_c`, `t_wb_c`, `t_dp_c` in °C (`psychrometrics.rs`)
- Stefan-Boltzmann radiation code correctly applies `+ CELSIUS_TO_KELVIN` before raising to the 4th power (`longwave_radiation.rs:208-210`)

**Comparison with vendor**: OCHRE uses `°C` throughout (via Pint unit registry, `units.py:25` `degC_to_K = convert(0, "degC", "K")`). EnergyPlus also uses `°C` for internal calculations. No Fahrenheit contamination detected.

---

### Finding 7: Latent heat constant consistent across crate boundaries [Severity: none — verified correct]
**Description**: All moisture mass balance computations use the 0°C ASHRAE reference (2,501 kJ/kg = 2,501,000 J/kg). Verified at:
- `thermal_solver/mod.rs:63`: `H_FG_J_PER_KG = LATENT_HEAT_VAPORISATION_0C_KJ_KG * KJ_TO_J` → 2,501,000 J/kg
- `humidity_solver.rs:35`: `h_fg_j_kg = LATENT_HEAT_VAPORISATION_0C_KJ_KG * KJ_TO_J` → 2,501,000 J/kg
- `air_conditioner.rs:811`: `-latent_cooling_w / LATENT_HEAT_VAPORISATION_0C_J_KG` → 2,501,000 J/kg
- `ideal_hvac.rs:580`: `latent_w / LATENT_HEAT_VAPORISATION_0C_J_KG` → 2,501,000 J/kg
- `humidity_solver.rs:709-714`: explicit test asserting `thermal_h_fg == humidity_h_fg` within epsilon
- `psychrometrics.rs:15`: `LATENT_HEAT_VAPORISATION_KJ_KG = LATENT_HEAT_VAPORISATION_0C_KJ_KG` (alias)

The 20°C reference constant `LATENT_HEAT_VAPORISATION_J_KG = 2_450_000.0` is defined at `constants.rs:63` but is **never referenced anywhere in the codebase** (confirmed: only matches are its own definition and the comment warning against its use). Consistent with the explicit policy at `constants.rs:8-11`.

**Comparison with vendor**: OCHRE uses 2501 kJ/kg via psychrolib. EnergyPlus uses 2501 kJ/kg at 0°C reference for enthalpy. No deviation.

---

### Finding 8: Mass flow consistently in kg/s across crate boundaries [Severity: none — verified correct]
**Description**: Fluid port fields use `kg_s` suffix and represent kg/s. Draw water heaters use `draw_rate_kg_s`. No L/s or g/s contamination detected.
- `PortContribution::Fluid::flow_rate_kg_s` (`ports.rs:78`)
- `FluidAccumulator::total_flow_kg_s` (`ports.rs:325`)
- `WetApplianceState::hot_water_draw_rate_kg_s` (`event_load.rs:125`)
- `StratifiedTank` draw rate uses kg/s (`tank.rs`)
- Infiltration flow is in m³/s, explicitly converted to kg/s via `moist_air_density_kg_m3`

---

## Summary
- **Total findings**: 8
- **Critical**: 0
- **High**: 0
- **Medium**: 2 (Finding 1 — mixed W/kW in PortContribution; Finding 2 — kPa stored with inline Pa conversion)
- **Low**: 3 (Finding 3 — hardcoded 273.15; Finding 4 — legacy DEFROST_CAPACITY_UNIT_FACTOR; Finding 5 — undocumented magic numbers)
- **Verified correct (no issue)**: 3 (Findings 6, 7, 8 — temperature, latent heat, mass flow)

## Recommendations

1. **Consider aligning `PortContribution::Electrical` to W**: Change `active_power_kw` to `active_power_w` in the port system, eliminating the mandatory kW↔W conversions at every equipment integration point. This was likely chosen for utility-metering ergonomics, but the cost is pervasive conversion at 20+ call sites. Alternatively, add a `PortContribution::Electrical` unit test that verifies `* 1000.0` is applied consistently at every producer.

2. **Standardise all pressure conversions to use `WeatherState::pressure_pa()`**: Replace the `env.weather.pressure_kpa * 1000.0` pattern with the existing convenience method. This eliminates the risk of future call sites forgetting the conversion. (8 manual `* 1000.0` sites found; 4 sites already use `pressure_pa()`.)

3. **Reconcile Celsius-to-Kelvin usage**: Replace hardcoded `+ 273.15` with `+ CELSIUS_TO_KELVIN` at `psychrometrics.rs:48`, `battery/mod.rs:211`, `battery/mod.rs:1024`, `battery/mod.rs:1033`, and `ev/mod.rs:578`.

4. **Document the ceiling fan energy constants**: Add a comment at `resolve_loads.rs:498` explaining the provenance of 3000.0 (rated hours/year) and 10.5 (usage hours/day) per the ResStock or OCHRE reference.

5. **Add a `DEFROST_CAPACITY_UNIT_FACTOR` provenance comment**: Document which EnergyPlus version produced the 1.01667 value and note that it must remain synchronised with the empirical defrost coefficients.

## References / Citations
- ASHRAE 2017 Handbook of Fundamentals, Chapter 1 (psychrometric constants)
- EnergyPlus Engineering Reference, "Window Heat Transfer Calculations" and "DX Coil" sections
- OCHRE `vendors/OCHRE/ochre/utils/units.py` (Pint-based unit registry, temperature/pressure conventions)
- NIST SP 811 (conversion factors for BTU/h→W)
- Walker & Wilson (1998) "Field Validation of Algebraic Equations for Stack and Wind Driven Air Infiltration Calculations"
