# WH config struct: tank volume, UA, setpoint, deadband — no impossible combos
**Review ID**: whlogic-03
**Category**: wh-logic
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-equipment/src/water_heater/wh_config.rs` (primary)
- `crates/hares-equipment/src/water_heater/mod.rs` (defaults & shared utilities)
- `crates/hares-equipment/src/water_heater/tank.rs` (UA distribution across nodes)
- `crates/hares-equipment/src/water_heater/hpwh_compressor.rs` (HPWH-specific defaults)
- `crates/hares-equipment/src/water_heater/resistance.rs` (resistance WH init & defaults)
- `crates/hares-equipment/src/water_heater/gas.rs` (gas WH init & defaults)
- `crates/hares-equipment/src/water_heater/heat_pump_wh.rs` (HPWH init & defaults)

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Equipment/WaterHeater.py` — OCHRE WaterHeater base class, deadband/setpoint/max-temp validation, & element control logic
- `vendors/OCHRE/ochre/utils/hpxml.py` — OCHRE UA calculation from EF/UEF per Burch & Erickson (2004), UA<0 guard, and jacket R-value correction
- `vendors/EnergyPlus/src/EnergyPlus/WaterThermalTanks.cc` — EnergyPlus mixed/stratified tank input validation, deadband range checks, and setpoint vs max-temp enforcement

## Findings

### Finding 1: [Severity: high] Deadband of zero allowed — infinite switching
**Description**: All three storage-type config validators (`GasWaterHeaterConfig`, `ElectricResistanceWaterHeaterConfig`, `HeatPumpWaterHeaterConfig`) use `check_finite(…, 0.0, false)` for deadband validation. A `strict=false` parameter means `v >= 0.0` is accepted, so `deadband_c = Some(0.0)` passes without error. A deadband of zero would cause the thermostat to toggle on every timestep (infinite switching frequency), which is neither physically possible nor numerically stable.

**Code Location**: `wh_config.rs:107`, `wh_config.rs:257`, `wh_config.rs:492`
```
check_finite("gas_wh: deadband_c", self.deadband_c, 0.0, false)?;
check_finite("resistance_wh: deadband_c", self.deadband_c, 0.0, false)?;
check_finite("hpwh: deadband_c", self.deadband_c, 0.0, false)?;
```

**Root Cause**: The `strict` parameter is set to `false` for deadband, when it should be `true` to enforce `> 0.0`. Additionally, there is no upper bound check (EnergyPlus enforces `≤ 20°C` at line 1265 and line 3845, OCHRE defaults to 5.56°C / 10°F).

**Impact**: A user providing `deadband_c: 0` (or a negative value) would cause the thermostat to chatter at the setpoint boundary, producing physically meaningless results and potentially causing numerical instability in the stratified tank solver.

**Vendor Comparison**:
- **EnergyPlus** (`WaterThermalTanks.cc:3845-3850`): Explicitly validates `DeadBandDeltaTemp > 0.0`; defaults to `0.0001` if invalid.
- **EnergyPlus** (`WaterThermalTanks.cc:1265`): HPWH deadband validated as `> 0.0 && <= 20.0`.
- **OCHRE** (`WaterHeater.py:76`): Defaults to `5.56` (10°F) from kwargs, no explicit range check beyond the default.

### Finding 2: [Severity: high] UA=0 allowed — thermodynamically impossible
**Description**: All storage-type config validators use `check_finite(…, 0.0, false)` for UA validation, permitting `ua_w_per_k = Some(0.0)`. A perfectly insulated tank (UA=0) violates the Second Law of Thermodynamics as it could theoretically store heat indefinitely with zero standing loss. The function `apply_jacket_r_value` (`mod.rs:171`) already guards against `ua_base <= 0.0` (returns early), implying downstream code is aware that UA≤0 is problematic, but config validation still allows it.

**Code Location**: `wh_config.rs:100`, `wh_config.rs:250`, `wh_config.rs:485`
```
check_finite("gas_wh: ua_w_per_k", self.ua_w_per_k, 0.0, false)?;
check_finite("resistance_wh: ua_w_per_k", self.ua_w_per_k, 0.0, false)?;
check_finite("hpwh: ua_w_per_k", self.ua_w_per_k, 0.0, false)?;
```

**Root Cause**: Same strictness issue as deadband — the `strict` parameter should be `true` for UA to enforce `> 0.0`.

**Impact**: A UA of zero produces zero standby heat loss, meaning the tank never cools down without a water draw. This hides model errors and gives unrealistically optimistic efficiency results.

**Vendor Comparison**:
- **OCHRE** (`hpxml.py:1141-1144`): Explicitly raises `OCHREException` when calculated UA < 0.0 — "A negative water heater standby loss coefficient (UA) was calculated."
- **EnergyPlus**: `OffCycLossCoeff` and `OnCycLossCoeff` are direct user inputs. The code doesn't appear to validate these are positive, but zero-conductance is not a meaningful simulation.

### Finding 3: [Severity: high] Setpoint has no safe temperature range
**Description**: All config validators use `check_finite(…, f64::NEG_INFINITY, false)` for setpoint validation, which checks only that the value is finite. A user could set `setpoint_c: Some(0.0)` (freezing), `setpoint_c: Some(100.0)` (boiling), or `setpoint_c: Some(-273.15)` (absolute zero) and all would pass validation. The residential safe range is 49-60°C (120-140°F). Below 49°C risks Legionella growth; above 60°C risks scalding.

**Code Location**: `wh_config.rs:101-106`, `wh_config.rs:251-256`, `wh_config.rs:486-491`
```
check_finite("gas_wh: setpoint_c", self.setpoint_c, f64::NEG_INFINITY, false)?;
check_finite("resistance_wh: setpoint_c", self.setpoint_c, f64::NEG_INFINITY, false)?;
check_finite("hpwh: setpoint_c", self.setpoint_c, f64::NEG_INFINITY, false)?;
```

**Root Cause**: No `check_range` wrapping — only finiteness is verified. The default value (51.67°C / 125°F) is within the safe range, but user overrides are unconstrained.

**Impact**: Physically absurd setpoints cause simulation results that don't reflect real-world equipment behavior or safety standards.

**Vendor Comparison**:
- **EnergyPlus** (`WaterThermalTanks.cc:6575-6596`): Validates setpoint against `TankTempLimit` — if setpoint exceeds max limit, it is reset to `limit - 1°C` with a warning. Chilled water tanks get a symmetric lower-bound check.
- **EnergyPlus** (`WaterThermalTanks.cc:6641-6651`): HPWH setpoint checked against tank temperature limit.
- **OCHRE** (`WaterHeater.py:72-74`): `setpoint_temp` from kwargs; `max_temp = kwargs.get("Max Tank Temperature", convert(140, "degF", "degC"))` (≈60°C). External setpoint validated against `self.max_temp` in `update_external_control` (line 105-107).

### Finding 4: [Severity: medium] Tank volume has no upper bound
**Description**: While `check_finite(…, 0.0, true)` correctly enforces `tank_volume_m3 > 0`, there is no upper bound. The residential range is 80-450 L (0.08-0.45 m³). A user could set `tank_volume_m3: 10.0` (10,000 L, larger than a swimming pool) and the simulation would silently accept it, producing nonsensical thermal mass and recovery behavior.

**Code Location**: `wh_config.rs:85`, `wh_config.rs:225-230`, `wh_config.rs:476`
```
check_finite("gas_wh: tank_volume_m3", self.tank_volume_m3, 0.0, true)?;
check_finite("resistance_wh: tank_volume_m3", self.tank_volume_m3, 0.0, true)?;
check_finite("hpwh: tank_volume_m3", self.tank_volume_m3, 0.0, true)?;
```

**Root Cause**: No `check_range` or upper-limit clamp. The default `DEFAULT_TANK_VOLUME_M3 = 0.189...` (≈50 gal / 190 L) is a realistic value.

**Impact**: Overly large tanks slow thermal response unrealistically; overly small (but positive) tanks (< 30 L) respond too quickly.

### Finding 5: [Severity: medium] No setpoint vs max_tank_temp_c consistency check
**Description**: Both `setpoint_c` and `max_tank_temp_c` are independently validated for finiteness, but there is no cross-field check that `setpoint_c < max_tank_temp_c`. A config with `setpoint_c: 70.0, max_tank_temp_c: 60.0` would pass validation, causing the tank to never be "at setpoint" and the heater to run continuously.

**Code Location**: `wh_config.rs:84-177` (all `validate()` impls)
- Setpoint: lines 101-106, 251-256, 486-491 (finiteness only)
- Max temp: lines 107-113, 257-263, 492-498 (finiteness only)
- No cross-field check between the two.

**Root Cause**: `validate()` only checks each field in isolation via `check_finite`/`check_range` helpers. No multi-field validation logic is implemented.

**Vendor Comparison**:
- **EnergyPlus** (`WaterThermalTanks.cc:6575-6578`): If setpoint > max temp limit, resets setpoint to `limit - 1°C` and warns.
- **OCHRE** (`WaterHeater.py:105-107`): If external setpoint > max_temp, clamps it with a warning.

### Finding 6: [Severity: medium] No deadband upper-bound check
**Description**: EnergyPlus validates `DeadBandTempDiff <= 20.0` (line 1265 and line 3845). The HARES code has no upper bound, so a deadband of 50°C would pass validation. This would mean the tank cools 50°C below setpoint before the heater turns on — far beyond realistic thermostat behavior.

**Code Location**: `wh_config.rs:107`, `wh_config.rs:257`, `wh_config.rs:492`

**Vendor Comparison**:
- **EnergyPlus**: Enforces `DeadBandDeltaTemp > 0.0 && <= 20.0`.
- **OCHRE**: Defaults to 5.56°C; no explicit upper bound in config.

### Finding 7: [Severity: medium] Element power has no upper bound or circuit check
**Description**: `ElectricResistanceWaterHeaterConfig` validates `element_power_w >= 0.0` but has no upper bound. Typical residential elements are 3.0-5.5 kW at 240V. Two elements at 5.5 kW = 11 kW, exceeding a typical 30A × 240V = 7.2 kW circuit unless the non-simultaneous interlock is enforced. The codebase does not verify that simultaneous operation is prevented or that total power respects circuit limits.

**Code Location**: `wh_config.rs:294-299`
```
check_finite("resistance_wh: element_power_w", self.element_power_w, 0.0, false)?;
```

**Root Cause**: No upper-bound check and no cross-field validation with `element_priority_mode` or `heating_capacity_w`.

**Vendor Comparison**:
- **OCHRE**: `default_capacity = 4500` W; `capacity_rated` from kwargs, no explicit upper bound but the thermostat control enforces `Upper On`/`Lower On` mutual exclusion (`ElectricResistanceWaterHeater` uses non-simultaneous control by default via mode priority).

### Finding 8: [Severity: medium] No cross-field correlation between tank volume and UA
**Description**: Tank volume and UA should be positively correlated — a larger tank has more surface area and higher standby losses. The HARES code provides a constant default `DEFAULT_UA_W_PER_K = 2.0` for all tank sizes, and UA is distributed across nodes proportional to their volume fraction (`tank.rs:159-162`). However, there is no validation that a user-supplied UA is physically plausible for the given tank volume. For example:
- A 50 L tank with UA=5.0 W/K would lose ~200 W at 40°C ΔT — roughly the entire element capacity of a small WH just to maintain standby.
- A 400 L tank with UA=0.1 W/K would have unrealistically low standby loss.

**Expected UA range**: For a 190L cylinder (0.5 m diameter, ~1.0 m height = ~2.1 m² lateral + ~0.4 m² ends ≈ 2.5 m² total), with R-12 insulation: UA = 2.5 / (12 × 0.176) ≈ 1.2 W/K. With R-24: UA ≈ 0.6 W/K. The default UA of 2.0 W/K is higher than both these values, corresponding roughly to R-7 insulation (pre-1980 standard).

**Code Location**: `tank.rs:159-162` — `ua_w_per_k * v / total_volume_m3` distributes UA proportionally but doesn't validate that the total UA is sensible for the volume.

**Root Cause**: No multi-field validation or heuristic warning. The single-default-UA approach means a user changing tank volume but not UA gets physically inconsistent behavior.

**Vendor Comparison**:
- **OCHRE** (`hpxml.py:1056-1126`): UA is **derived** from Energy Factor or Uniform Energy Factor using the Burch & Erickson (2004) method, which inherently couples volume, heating capacity, and UA. Heat pump WHs use volume-binned UA defaults (3.6, 4.0, 4.7 Btu/hr-°F for ≤58, ≤73, >73 gal).
- **EnergyPlus**: `OffCycLossCoeff` and `OnCycLossCoeff` are independent inputs (not derived from tank volume). The UA is applied per-node as `SkinLossCoeff × SkinArea + AdditionalLossCoeff` (`WaterThermalTanks.cc:6102`), so it does scale with surface area for stratified tanks.

### Finding 9: [Severity: low] Default UA=2.0 W/K is at the upper end of the expected range
**Description**: `DEFAULT_UA_W_PER_K = 2.0` corresponds to approximately R-7 insulation for a 190L tank — which is realistic for older, poorly insulated units but high for modern units (R-12 to R-24). The review instructions suggest expected values of 1.2 W/K (R-12) to 0.6 W/K (R-24). The code comments note this value is from OCHRE's ResStock HPXML processing (`hpxml.py:1070-1075`) which uses these hardcoded values based on tank volume bins for HPWHs.

**Code Location**: `mod.rs:15`
```
pub(crate) const DEFAULT_UA_W_PER_K: f64 = 2.0;
```

**Root Cause**: The default appears to be calibrated from OCHRE's HPWH storage class (which uses ResStock-binned values of 3.6-4.7 Btu/hr-°F ≈ 1.9-2.5 W/K) rather than derived from a specific insulation R-value. This may be intentional for conservative estimates.

**Vendor Comparison**:
- **OCHRE**: HPWH UA defaults are 3.6/4.0/4.7 Btu/hr-°F (≈1.9/2.1/2.5 W/K) based on tank volume bins. Gas/electric storage WHs compute UA from EF/UEF.
- **EnergyPlus**: Uses `OffCycLossCoeff`/`OnCycLossCoeff` directly from user input with no default — user must supply this.

## Summary
- **Total findings**: 9
- **Critical**: 0
- **High**: 3 (Findings 1, 2, 3 — deadband=0, UA=0, unconstrained setpoint)
- **Medium**: 5 (Findings 4, 5, 6, 7, 8 — volume upper bound, setpoint-vs-max cross-check, deadband upper bound, element power circuit check, volume-UA correlation)
- **Low**: 1 (Finding 9 — default UA on high side of expected range)

## Recommendations

1. **Change deadband validation to `strict=true`** (Findings 1, 6): Replace `check_finite(…, 0.0, false)` with `check_finite(…, 0.0, true)` for deadband fields, and add `check_range(…, 0.1, 20.0)` to match EnergyPlus bounds.

2. **Change UA validation to `strict=true`** (Finding 2): Replace `check_finite(…, 0.0, false)` with `check_finite(…, 0.0, true)` for UA fields to prevent zero-conductance tanks.

3. **Add setpoint range check** (Finding 3): Add `check_range(…, 40.0, 85.0)` for setpoint fields. While 49-60°C is the "safe" range, commercial/industrial WHs can go higher, so 40-85°C provides reasonable bounds without being overly restrictive.

4. **Add tank volume upper bound check** (Finding 4): Add `check_range("tank_volume_m3", …, 0.001, 0.5)` — 1 L minimum to 500 L maximum covers the residential range.

5. **Add setpoint vs max_tank_temp cross-validation** (Finding 5): In each `validate()` method, after checking both fields independently, add: if both `setpoint_c` and `max_tank_temp_c` are `Some`, verify `setpoint_c < max_tank_temp_c - deadband_c`, else emit a warning or error.

6. **Add element circuit safety check** (Finding 7): For `ElectricResistanceWaterHeaterConfig`, add a validation that if `element_power_w > 5500.0`, emit a warning. Optionally check that `heating_capacity_w <= 7200.0` (30A × 240V) unless simultaneous mode is explicitly disabled.

7. **Add volume-UA heuristic warning** (Finding 8): Compute expected UA range from tank surface area (derived from volume and height/diameter assuming cylindrical geometry) and R-8 to R-24 insulation, and emit a warning if user-supplied UA is outside ±50% of the midpoint. Surface area: `A = π × d × h + 2 × π × (d/2)²`. Expected UA: `A / (R_value × 0.176 m²·K/W)`.

8. **Document default UA rationale** (Finding 9): Add a doc comment to `DEFAULT_UA_W_PER_K` explaining that the value 2.0 W/K is calibrated from OCHRE HPXML/ResStock defaults for a 190L tank, and note the equivalent approximate R-value.

## References / Citations

| Reference | Source |
|-----------|--------|
| OCHRE WaterHeater deadband default (10°F = 5.56°C) | `WaterHeater.py:76` |
| OCHRE max tank temp from HPXML (140°F = 60°C) | `WaterHeater.py:74` |
| OCHRE UA negative guard | `hpxml.py:1141-1144` |
| OCHRE UA derivation from EF/UEF (Burch & Erickson 2004) | `hpxml.py:1016-1126` |
| OCHRE HPWH UA by volume bin (ResStock) | `hpxml.py:1070-1075` |
| EnergyPlus deadband validation (0 < x ≤ 20) | `WaterThermalTanks.cc:1265, 3845` |
| EnergyPlus setpoint vs max-temp enforcement | `WaterThermalTanks.cc:6575-6596` |
| EnergyPlus HPWH setpoint vs tank temp limit | `WaterThermalTanks.cc:6641-6651` |
| EnergyPlus stratified UA = SkinLossCoeff × SkinArea | `WaterThermalTanks.cc:6102` |
| EnergyPlus sizing: "Try increasing Deadband Temperature Difference or Tank Volume" | `WaterThermalTanks.cc:7569` |
| HARES deadband default (10°F = 5.556°C) | `mod.rs:14` (via `resistance.rs:42`, `gas.rs:31`) |
| HARES HPWH deadband default (14.7°F = 8.167°C) | `hpwh_compressor.rs:17` |
| HARES setpoint default (125°F = 51.667°C) | `mod.rs:14` |
| HARES UA default (2.0 W/K) | `mod.rs:15` |
| HARES jacket R-value correction guard | `mod.rs:171` |
| HARES UA distribution by volume fraction | `tank.rs:159-162` |
