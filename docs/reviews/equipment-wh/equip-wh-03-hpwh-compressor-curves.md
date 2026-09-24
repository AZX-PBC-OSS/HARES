# HPWH compressor biquadratic curve parameter audit
**Review ID**: equip-wh-03
**Category**: equipment-wh
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-equipment/src/water_heater/heat_pump_wh.rs`
- `crates/hares-equipment/src/water_heater/hpwh_compressor.rs`
- `crates/hares-equipment/src/water_heater/wh_config.rs`
- `crates/hares-physics/src/biquadratic.rs`

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Equipment/WaterHeater.py`

## Findings

### Finding 1: [Severity: high]
**Description**: Low-power HPWH (UEF >= 4.9) coefficient set and detection path are entirely absent from HARES. OCHRE's HeatPumpWaterHeater class (`WaterHeater.py:444-474`) switches to distinct compressor COP and capacity biquadratic coefficients when `low_power_hpwh` is True (typically triggered when UEF >= 4.9 in HPXML), and applies widened ambient lockout bounds (2.778–62.778°C instead of 7.222–43.333°C, `WaterHeater.py:611-616`). HARES has no `low_power_hpwh` flag, no UEF threshold logic, no alternative low-power coefficient set, and no widened ambient bounds. A test named `low_power_hpwh_thermal_capacity_is_1499_4` exists (`heat_pump_wh.rs:1438`) but merely sets a manual `compressor_power_w` value without using any low-power curve coefficients.
**Code Location**: `hpwh_compressor.rs:29-34` — only one coefficient set each for COP and capacity; `wh_config.rs:425-466` — no `low_power_hpwh` config field; `WaterHeater.py:444-474` — OCHRE's three branching coefficient paths.
**Root Cause**: The low-power HPWH path was not implemented during the HARES port from OCHRE. The `HeatPumpWaterHeaterConfig` struct lacks a `low_power_hpwh` field, and `init_typed` has no branching logic to select alternative curve coefficients based on UEF or a boolean flag.
**Impact**: High-efficiency HPWH units (UEF >= 4.9) will be modeled with the standard compressor curves, which overstate compressor power and capacity by design (low-power units use ~470 W electrical vs ~500 W for standard). The missing widened ambient bounds also mean low-power units that could operate down to 2.8°C or up to 62.8°C will be locked out at the standard 7.2–43.3°C range. This affects annual energy use and demand response readiness for any high-UEF HPWH.

### Finding 2: [Severity: medium]
**Description**: COP biquadratic curve scaling differs from OCHRE and may change the effective COP magnitude. OCHRE applies the COP curve as a direct multiplier on `cop_nominal`: `hp_cop = cop_nominal * cop_curve(t_wet, t_lower)` (`WaterHeater.py:637-640`). HARES applies a `cop_scale` derived from the midpoint of the curve domain: `cop_scale = cop_rated / cop_curve(ref_wet, ref_tank)` and then `cop = cop_curve(wet_bulb, tank_avg) * cop_scale` (`heat_pump_wh.rs:417-423, 647`). While the HARES approach anchors the curve to the user-supplied rated COP at the domain midpoint (a defensible design choice for HPXML integration), it is **not** a transparent behavioral equivalent of OCHRE's direct-multiplication approach. When the default coefficients are used with `DEFAULT_RATED_COP = 3.45` at the domain midpoint (wet-bulb 25°C, tank 45°C), OCHRE would produce COP ≈ 3.88 while HARES produces COP = 3.45 — a ~12% difference at the same reference conditions.
**Code Location**: `heat_pump_wh.rs:417-423` (cop_scale computation); `heat_pump_wh.rs:647` (COP evaluation); `WaterHeater.py:637-640` (OCHRE reference).
**Root Cause**: The `cop_scale` design was introduced to align the curve magnitude with a user-provided HPXML/UEF-derived COP (`heat_pump_wh.rs:110-113`). However, the normalization is always applied, even when the user provides custom `cop_biquadratic_coeffs` — the `cop_scale` is recomputed from the midpoint regardless of whether the custom coefficients are already normalized.
**Impact**: COP values deviate from OCHRE benchmark results. Users providing custom COP coefficients from other tools (EnergyPlus, BEopt) may get unexpected results if those coefficients were designed with the OCHRE-style `cop_nominal * curve()` convention. The capacity curve does **not** have this issue — it is used directly as a multiplier without normalization (`heat_pump_wh.rs:650-653`).

### Finding 3: [Severity: medium]
**Description**: Source citation in the coefficient comment is incorrect. `hpwh_compressor.rs:28` claims the default COP coefficients are sourced from `vendors/OCHRE/ochre/Equipment/WaterHeater.py lines 446-448`, but OCHRE lines 446-448 contain the `cop_nominal` assignment and a low-COP warning check, not the coefficient definitions. The actual coefficients are at `WaterHeater.py:469-474` (or specifically line 474 for the standard coefficients).
**Code Location**: `hpwh_compressor.rs:28`.
**Root Cause**: Comment written with incorrect line number reference.
**Impact**: Low — purely documentation. Developers trying to verify coefficient provenance against OCHRE source will look at the wrong lines.

### Finding 4: [Severity: low]
**Description**: HARES uses volume-weighted average tank temperature (`tank_avg_temp_c`) as the second biquadratic input (`x2`), while OCHRE uses the condenser-weighted average of the lower tank node temperatures: `t_lower = np.dot(self.hp_nodes, self.model.states)` (`WaterHeater.py:632`). The two temperature inputs differ systematically — OCHRE's approach samples only the condenser-related nodes (lower half of tank), which better represents the water temperature actually seen by the condenser coil, while HARES averages across all nodes including the hot upper region. This means the same HPWH state produces different curve evaluations in HARES vs OCHRE even with identical coefficients.
**Code Location**: `heat_pump_wh.rs:643-647` (volume-weighted average used as x2); `WaterHeater.py:632` (condenser-weighted average used as t_lower).
**Root Cause**: HARES uses `weighted_average_tank_temp()` across all nodes rather than the condenser-weighted temperature. The tank model has separate condenser injection logic (`build_heat_injections`) but the temperature input to the curve is decoupled from where the heat actually goes.
**Impact**: Moderate — shifts curve evaluations slightly. Because the volume-weighted average includes hotter upper-region water, the curve sees a slightly higher x2 than OCHRE at the same tank state. For the COP curve (which has a negative d-coefficient of −0.01113), this depresses the computed COP. The magnitude depends on tank stratification.

### Finding 5: [Severity: low]
**Description**: No step-discontinuity guard around UEF = 4.9 exists — because no low-power HPWH transition exists at all. Since HARES uses a single coefficient set regardless of UEF, there is technically no discontinuity at UEF = 4.9, but this is only because the low-power path is absent rather than because the curves interpolate smoothly. If the low-power path were added in the future, a cross-fade or blending region would be required to avoid an instantaneous jump in COP and capacity when UEF crosses the threshold.
**Code Location**: `hpwh_compressor.rs:29-34` (single coefficient set); `wh_config.rs:425-466` (no UEF field); `WaterHeater.py:444-474` (branching at `low_power_hpwh`).
**Root Cause**: Low-power HPWH not implemented.
**Impact**: Not currently an observable issue (no transition exists to be discontinuous) but represents a design risk for future implementation.

## Summary
- Total findings: 5
- Critical: 0
- High: 1
- Medium: 2
- Low: 2

## Recommendations
1. **Implement low-power HPWH coefficient switching** (Finding 1): Add a `low_power_hpwh: Option<bool>` field to `HeatPumpWaterHeaterConfig`, with automatic detection when `uniform_energy_factor >= 4.9`. When enabled, use the two OCHRE low-power coefficient sets (`WaterHeater.py:466-470`) for both COP and capacity, and widen ambient bounds to 2.778–62.778°C. The existing test at `heat_pump_wh.rs:1438` should be updated to validate curve selection rather than only pass-through of a manual wattage value.
2. **Consider aligning COP scaling with OCHRE** (Finding 2): Either (a) adopt OCHRE's direct `cop_nominal * cop_curve()` convention and deprecate/bury `cop_scale`, or (b) add a configuration flag `cop_curve_is_normalized: bool` that controls whether `cop_scale` is computed. If the curve coefficients come from OCHRE/EnergyPlus, option (a) is preferred; `cop_scale` should only activate when the curve is known to need anchoring.
3. **Fix source citation** (Finding 3): Update `hpwh_compressor.rs:28` to reference `WaterHeater.py:469-474` for the default COP coefficients and `WaterHeater.py:473` for the default capacity coefficients.
4. **Evaluate condenser-weighted tank temperature** (Finding 4): Replace `weighted_average_tank_temp()` with a condenser-node-weighted average using `self.condenser_node_weights` for biquadratic x2 input, matching OCHRE's `np.dot(self.hp_nodes, self.model.states)` convention (`WaterHeater.py:632`). This requires exposing the condenser-weighted temperature from the tank model or computing it inline from `node_temps()` and `condenser_node_weights`.
5. **Plan smooth transition for future low-power path** (Finding 5): When implementing Finding 1, use a blending approach (e.g., linear cross-fade of curve outputs in the UEF range 4.8–5.0) or introduce a separate `low_power_coefficients` override to avoid a hard step.

## References / Citations
- `vendors/OCHRE/ochre/Equipment/WaterHeater.py:444-474` — OCHRE low-power HPWH branching with three coefficient sets.
- `vendors/OCHRE/ochre/Equipment/WaterHeater.py:611-616` — OCHRE widened ambient bounds for low-power HPWH.
- `vendors/OCHRE/ochre/Equipment/WaterHeater.py:632-640` — OCHRE COP and capacity biquadratic evaluation.
- `crates/hares-equipment/src/water_heater/hpwh_compressor.rs:17-34` — HARES default curve constants and coefficient comments.
- `crates/hares-equipment/src/water_heater/heat_pump_wh.rs:417-423` — HARES cop_scale computation.
- `crates/hares-equipment/src/water_heater/heat_pump_wh.rs:643-653` — HARES curve evaluation with volume-weighted average.
- `crates/hares-physics/src/biquadratic.rs:9-16` — HARES biquadratic polynomial form `a + b*x1 + c*x1² + d*x2 + e*x2² + f*x1*x2`.
