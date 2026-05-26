# Equipment telemetry: verify all reported values physically correct
**Review ID**: wiring-05
**Category**: wiring
**Date**: 2026-05-26

## Files Reviewed
crates/hares-types/src/telemetry.rs crates/hares-types/src/telemetry_keys.rs crates/hares-core/src/telemetry.rs

## Vendor/Reference Files Consulted
vendors/OCHRE/ochre/Dwelling.py vendors/EnergyPlus/src/EnergyPlus/OutputProcessor.cc

## Findings

### Finding 1: COP reported without physical bounds validation [Severity: high]
**Description**: Heat pump heater, air conditioner, and heat pump water heater all report COP telemetry but never validate the computed COP against physically possible ranges. COP can exceed physically plausible values (e.g., >10 for air-source, >6 for ground-source) with degenerate curve inputs or report exactly `0.0` when equipment is off, which is physically meaningless and may contaminate RL training data.

**Code Location**:
- `crates/hares-equipment/src/hvac/heat_pump/heater.rs:1093-1102` (ASHP heating COP)
- `crates/hares-equipment/src/hvac/air_conditioner.rs:891-899` (AC cooling COP)
- `crates/hares-equipment/src/water_heater/heat_pump_wh.rs:647` (HPWH COP, `.max(0.1)` lower bound only)

**Root Cause**: COP division is guarded against NaN/Inf (`> 1e-6` check), but the resulting value is not clamped to an equipment-type-appropriate range before writing to telemetry. The BiquadraticCurve clamps *inputs* but not *outputs*, so a poorly-conditioned curve within bounds can still produce extreme COP.

**Impact**: Unbounded COP values in telemetry output can poison downstream machine-learning training data. RL controllers trained on COP telemetry from an ASHP reporting COP=100 will learn physically impossible behaviors. COPs of `0.0` during off states are also ambiguous (0 could mean "off" or "zero-efficiency operation").

**Comparison**: OCHRE (HVAC.py:575-581) also lacks COP bounds but uses `replace_nans()` at the DataFrame level (Analysis.py:220-221) to squash NaN/Inf to 0 at analysis time. EnergyPlus does not handle COP within the OutputProcessor — bounds are an equipment-model concern.

### Finding 2: Battery capacity_kwh division without zero-denominator guard [Severity: high]
**Description**: In the battery SOC update step, `energy_delta_kwh / self.capacity_kwh` at `battery/mod.rs:913` has no guard against `self.capacity_kwh == 0.0`. When the battery is fully degraded (SOH reaches 0.0), `capacity_kwh_nominal` and therefore `capacity_kwh` both become zero, producing `Inf` in telemetry SOC.

**Code Location**: `crates/hares-equipment/src/battery/mod.rs:890,913`

```
Line 890: self.capacity_kwh = self.capacity_kwh_nominal * capacity_derate;
Line 913: self.soc += energy_delta_kwh / self.capacity_kwh;
```

**Root Cause**: `capacity_kwh_nominal` becomes 0.0 when `soh = 0.0` (fully degraded), computed at `mod.rs:1037-1038`. With `capacity_derate` never exactly zero under the default Arrhenius model at realistic temperatures, `capacity_kwh` would also be zero. There is no check that `capacity_kwh > 0.0` before division, nor is the step aborted when the battery has zero effective capacity.

**Impact**: A fully degraded battery (theoretical extreme) would produce `Inf` in SOC telemetry and subsequent calculations, potentially crashing downstream Python consumers or corrupting RL training buffers. While SOH=0 is an edge case requiring decades of degradation, the code path is reachable and unguarded.

**Comparison**: OCHRE (Battery.py:341) asserts `soc_max + 0.001 >= next_soc >= soc_min - 0.001` with tolerance, catching the Inf case at the assertion level. EnergyPlus does not guard numeric values within the OutputProcessor.

### Finding 3: capacity_fade_pct telemetry not monotonically increasing during BOL transient [Severity: medium]
**Description**: `capacity_fade_pct` telemetry can decrease during the beginning-of-life (BOL) transient of the Smith 2017 degradation model. This violates the review requirement that "capacity degradation must be monotonically increasing."

**Code Location**:
- `crates/hares-equipment/src/battery/mod.rs:1065-1068` (telemetry write)
- `crates/hares-equipment/src/battery/degradation.rs:324` (q_li3 decreasing into negative values)
- `crates/hares-equipment/src/battery/degradation.rs:336-338` (capacity_fade = 1.0 - remaining; remaining increases when q_li3 goes negative)

**Root Cause**: Mechanism 3 (B3 sealed SEI growth) of the Smith 2017 model uses a negative `B3_REF = -2.805e-2`. During the first ~27 days, `q_li3` decays from 0.0 toward `B3_REF` (negative), which *reduces* total lithium loss and temporarily *increases* `remaining`, causing `capacity_fade` and `capacity_fade_pct` to decrease. This is a genuine physical transient (SEI growth partially passivates the anode) but violates the monotonicity expectation.

**Impact**: Downstream consumers that assume monotonic degradation (e.g., health-monitoring dashboards, SOH forecasting models) may incorrectly treat the BOL capacity "boost" as a data error. RL agents may learn to cycle aggressively in early life to maintain the apparent capacity gain, accelerating long-term degradation.

### Finding 4: Air conditioner reports dimensionless COP (W/W), no EER (Btu/Wh) telemetry [Severity: medium]
**Description**: The air conditioner writes `COP` telemetry as `(sensible_cooling_w + latent_cooling_w) / compressor_only_w` — a dimensionless W/W ratio. The review requires EER in "typically 8-14 (Btu/Wh) or 2.3-4.1 (W/W)". No EER (Btu/Wh) conversion is performed or reported. Consumers expecting EER in Btu/Wh will misinterpret the dimensionless COP values (e.g., COP=3.5 W/W appears as EER=3.5 instead of the conventional ~11.9 Btu/Wh).

**Code Location**: `crates/hares-equipment/src/hvac/air_conditioner.rs:891-899`

**Root Cause**: The `COP` telemetry key is defined as dimensionless (`tk::COP` in `telemetry_keys.rs:102`). No `EER_BTU_WH` key exists. The conventional EER = COP * 3.412_141_633 conversion is present in `cooling_config.rs:134` for SEER derivation but is never applied in telemetry reporting.

**Impact**: Analysis code and external consumers must know the unit convention. If a consumer assumes EER (Btu/Wh) format, an ASHP with COP=3.5 appears to have EER=3.5 instead of ~11.9 — a ~70% under-estimation of efficiency. The fix is either to document the convention in the key name (e.g., `COP_W_PER_W`) or to add a separate `EER_BTU_PER_WH` telemetry key.

**Comparison**: OCHRE labels its COP column explicitly as `"HVAC Cooling COP (-)"` (Analysis.py:239) with the dash convention for dimensionless. Both OCHRE and HARES follow this convention — the ambiguity exists in both but is clearer in OCHRE's column naming.

### Finding 5: No NaN/Inf guard at the Telemetry::set layer [Severity: low]
**Description**: `Telemetry::set()` accepts any `f64` without NaN or infinity checks. While individual equipment models guard their own division operations, there is no defense-in-depth at the telemetry infrastructure layer to catch a future equipment model that produces NaN/Inf.

**Code Location**: `crates/hares-types/src/telemetry.rs:31-39`

```rust
pub fn set(&mut self, key: &str, value: f64) {
    if let Some(v) = self.0.get_mut(key) {
        *v = value;    // no is_finite() check
    } else {
        panic!("...");
    }
}
```

**Root Cause**: Design choice — the `Telemetry` struct is a thin wrapper around `HashMap<String, f64>` with no value validation. This is consistent with performance-sensitive design but provides no safety net.

**Impact**: Any future equipment model that bypasses division guards could produce NaN or Inf in telemetry, which would corrupt downstream data without warning. Adding `debug_assert!(value.is_finite())` would catch these during development without runtime cost in release builds.

**Comparison**: OCHRE uses `replace_nans()` at the DataFrame level (Analysis.py:220-221) to squash NaN/Inf to 0 during post-processing. EnergyPlus passes raw values through the OutputProcessor without NaN/Inf guards. Both are reactive rather than proactive.

### Finding 6: HPWH COP curve uses biquadratic without output clamping [Severity: low]
**Description**: The heat pump water heater COP is computed as `(self.cop_curve.evaluate(wet_bulb_c, tank_avg_temp_c) * self.cop_scale).max(0.1)`. The `BiquadraticCurve` clamps *inputs* to bounds but does not clamp the polynomial *output*. A poorly-conditioned curve (e.g., from user-supplied coefficients) within valid input bounds could produce extreme COP values.

**Code Location**:
- `crates/hares-equipment/src/water_heater/heat_pump_wh.rs:647`
- `crates/hares-physics/src/biquadratic.rs:38-55` (curve evaluate — clamps inputs, not output)

**Root Cause**: The biquadratic polynomial evaluation is unbounded — `BiquadraticCurve::evaluate()` clamps inputs then passes the clamped values to `biquadratic()` which computes the raw polynomial. There is no output clamping.

**Impact**: With standard industry curves (OCHRE/EnergyPlus defaults) this is not a practical issue — the curves are calibrated within their operating domain. However, user-supplied curves or very wide input bounds could produce COP values of hundreds, which would be physically impossible for a vapor-compression heat pump.

## Summary
- Total findings: 6
- Critical: 0
- High: 2 (COP unbounded, battery capacity_kwh div-by-zero)
- Medium: 2 (capacity_fade_pct non-monotonic, AC COP vs EER convention)
- Low: 2 (no NaN guard at Telemetry::set, HPWH COP curve output unclamped)

## Positive Observations
1. **Unit consistency is strong**: All temperature telemetry values are in Celsius (`_C` suffix). All power telemetry correctly distinguishes W vs kW with explicit `/1000` or `*1000` conversions. No mixed-unit bugs found.
2. **Division guards are pervasive**: Every COP, SHR, and efficiency calculation has an explicit zero-denominator guard (`> 1e-6`, `!= 0.0`, `.max(0.1)`, etc.). No NaN-producing code paths exist in current equipment models.
3. **EV SOC bounds are robust**: SOC is clamped to `[0.0, 1.0]` at every update point, with NaN/finite validation on control signal inputs and taper-based discharging to prevent overshoot.
4. **Defrost COP is physically correct**: COP reduction during defrost is implicit — thermal output drops via `defrost_capacity_multiplier` while electrical consumption may increase — producing a realistic efficiency degradation without a separate, possibly inconsistent, COP adjustment.
5. **No separate energy telemetry fields**: HARES reports only power telemetry, not energy. Energy aggregation is performed downstream by consumers via `sum(power * dt)`. This avoids the common bug where independent energy-accumulation variables drift from the power integral.
6. **Biquadratic curve input clamping**: All performance curves clamp inputs to manufacturer-specified bounds before evaluation, matching EnergyPlus convention and preventing extrapolation artifacts.

## Recommendations
1. **Clamp COP to physical ranges before telemetry write**: For air-source heat pumps, clamp COP to `[0.0, 10.0]`. For ground-source, `[0.0, 8.0]`. For AC, `[0.0, 8.0]` W/W. These are generous bounds that accommodate extreme conditions while excluding physically impossible values. Report `0.0` only when compressor is off (per AHRI convention already used).
2. **Guard battery SOC division against zero capacity_kwh**: On the `self.capacity_kwh` division path at `battery/mod.rs:913`, add `if self.capacity_kwh <= 0.0 { self.soc; return; }` to early-return when the battery has zero effective capacity. Alternatively, `.max(1e-9)` on the denominator as a belt-and-suspenders guard.
3. **Rename AC COP telemetry or add EER key**: Either rename the AC key to `cop_w_per_w` for clarity, or add an `eer_btu_per_wh` key computed as `COP * 3.412_141_633`. Document the convention in the key constant docstring.
4. **Add `debug_assert!` to Telemetry::set**: Add `debug_assert!(value.is_finite(), "telemetry key '{key}' = {value}")` to catch NaN/Inf during debug/test builds without runtime overhead in release.
5. **Document capacity_fade_pct BOL non-monotonicity**: The Smith 2017 model's BOL transient is physically intentional but must be documented for downstream consumers. Add a docstring to `CAPACITY_FADE_PCT` in telemetry_keys.rs noting the BOL capacity boost.

## References / Citations
- OCHRE `HVAC.py:575-581` — COP calculation with W/kW unit conversion
- OCHRE `Battery.py:337-341` — SOC update with assertion bounds
- OCHRE `Analysis.py:220-221` — `replace_nans()` post-processing guard
- EnergyPlus `OutputProcessor.cc:1602` — SI base unit is J, with IP conversion to kWh
- EnergyPlus `OutputProcessor.cc:3000-3104` — `SetupOutputVariable` with unit metadata via `Constant::Units`
- EnergyPlus I/O Reference — `Curve:Biquadratic` specifies minimum/maximum value fields; E+ silently clamps
- AHRI Standard 210/240-2023 — COP definition excludes supply fan power; ASHP heating COP range ~1.5-5.0
- Smith et al. 2017 — Li-ion degradation model with three mechanisms (calendar, cycle, SEI growth)
