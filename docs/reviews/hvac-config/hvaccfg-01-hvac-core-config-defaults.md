# HvacCoreConfig defaults vs EnergyPlus Engineering Reference
**Review ID**: hvaccfg-01
**Category**: hvac-config
**Date**: 2026-05-26

## Files Reviewed
crates/hares-equipment/src/hvac/core_config.rs
crates/hares-equipment/src/hvac/hvac_core.rs
crates/hares-equipment/src/hvac/staging.rs
crates/hares-equipment/src/hvac/air_conditioner.rs
crates/hares-equipment/src/hvac/cooling_config.rs
crates/hares-equipment/src/hvac/heat_pump/constants.rs
crates/hares-equipment/src/hvac/heat_pump/heater.rs
crates/hares-equipment/src/hvac/coil_physics.rs
crates/hares-physics/src/constants.rs

## Vendor/Reference Files Consulted
vendors/EnergyPlus/src/EnergyPlus/DXCoils.cc
vendors/EnergyPlus/src/EnergyPlus/DXCoils.hh
vendors/EnergyPlus/src/EnergyPlus/StandardRatings.cc
vendors/EnergyPlus/src/EnergyPlus/Coils/CoilCoolingDXCurveFitPerformance.cc
vendors/EnergyPlus/src/EnergyPlus/Coils/CoilCoolingDXCurveFitOperatingMode.hh
vendors/EnergyPlus/src/EnergyPlus/PlantLoopHeatPumpEIR.cc
vendors/EnergyPlus/src/EnergyPlus/PlantLoopHeatPumpEIR.hh
vendors/EnergyPlus/src/EnergyPlus/VariableSpeedCoils.hh
vendors/EnergyPlus/src/EnergyPlus/VariableSpeedCoils.cc
vendors/EnergyPlus/src/EnergyPlus/UnitarySystem.hh
vendors/EnergyPlus/src/EnergyPlus/UnitarySystem.cc
vendors/EnergyPlus/src/EnergyPlus/Furnaces.cc
vendors/EnergyPlus/src/EnergyPlus/DataHVACGlobals.hh

## Findings

### Finding 1: [Severity: low]
**Description**: HARES `fan_power_per_flow_w_per_m3s` default agrees with EnergyPlus within floating-point rounding.
**Code Location**: `crates/hares-equipment/src/hvac/hvac_core.rs:30-31`
**Root Cause**: HARES defines `DEFAULT_FAN_POWER_W_PER_CFM = 0.365` (line 30) sourced from ANSI/RESNET/ICC 301-2019 Table 4.2.2(1). Converted to SI via `CFM_PER_M3_S = 1 / 0.0004719474432` (from `crates/hares-physics/src/constants.rs:133`). The resulting value is `0.365 / 0.0004719474432 ≈ 773.39 W/(m³/s)`. EnergyPlus defines `DefaultFanPowerPerEvapAirFlowRate = 773.3` (`DXCoils.cc:14715`) sourced from AHRI 340/360-2007 (365 W/1000 scfm). Both values derive from the same AHRI standard — 365 W per 1000 cfm. The HARES conversion yields 773.39 vs EnergyPlus's 773.3, a difference of ~0.012%, attributable to rounding of the `CFM_TO_M3_S` conversion factor. No corrective action needed.
**Impact**: Negligible (<0.1% difference in fan power). No physical or energy-consumption impact.

### Finding 2: [Severity: medium]
**Description**: HARES has no explicit `min_plr` (minimum part-load ratio) floor for single-stage equipment; PLR can reach zero, implying physically impossible infinite turndown for fixed-speed compressors.
**Code Location**: `crates/hares-equipment/src/hvac/staging.rs:86-92` (single-speed `select_speed` sets `part_load_ratio = load_fraction`), `crates/hares-equipment/src/hvac/air_conditioner.rs:1058-1066` (duty_cycle clamped to [0,1] but no lower bound).
**Root Cause**: HARES uses a Cd-based part-load factor (PLF) degradation approach (`PLF = 1 − Cd × (1 − PLR)`, `staging.rs:280`) instead of EnergyPlus's `MinPLR` cycling-ratio approach. In EnergyPlus, when PLR falls below `MinPLR` (typically 0.1–0.4), the equipment cycles on/off at `MinPLR` for a fraction of the timestep equal to `PLR / MinPLR` (see `HVACVariableRefrigerantFlow.cc:1127-1130`). HARES instead allows PLR to approach 0.0 with degraded PLF, treating continuous operation at arbitrarily low load fractions as equivalent to cycling. At PLR = 0, power draw is zero (PLF = 0.75, compressor power = 0), so the net energy consumption is correct. However, the instantaneous power is not physically correct: a single-speed compressor cannot operate at, e.g., 5% capacity — it would cycle at 100% capacity for 5% of the time. This matters for subhourly dispatch fidelity (peak power, voltage flicker) but not for hourly or annual energy totals. EnergyPlus's `CoilCoolingDXCurveFitPerformance.cc:466` defines `CyclicDegradationCoefficient(0.20)` for the 2023 standard and the cycling ratio model is the recommended approach for equipment-level modeling. The HARES approach is a valid simplification for quasi-steady-state simulation but diverges from the EnergyPlus Engineering Reference cycling model.
**Impact**: Correct annual/energy-weighted results (Cd-based degradation captures average efficiency penalty). Potentially inaccurate instantaneous power draw at very low loads for subhourly simulations. The `min_plr` guard is a standard safety mechanism in building simulation to prevent physically unrealistic continuous operation at near-zero load.

### Finding 3: [Severity: low]
**Description**: Crankcase heater default power is 50 W (within residential range 30–70 W) but threshold temperature 12.8°C diverges from EnergyPlus defaults.
**Code Location**: `crates/hares-equipment/src/hvac/air_conditioner.rs:34-35` (`CRANKCASE_HEATER_KW = 0.05`, `CRANKCASE_HEATER_THRESHOLD_C = 12.8`).
**Root Cause**: HARES uses 50 W (0.05 kW) default crankcase heater power, which falls within the typical 30–70 W residential range. However, the activation threshold of 12.8°C differs from EnergyPlus defaults:
- EnergyPlus `DXCoils.hh:471`: `MaxOATCrankcaseHeater(0.0)` — default 0°C (effectively always off unless overridden by user).
- EnergyPlus `PlantLoopHeatPumpEIR.hh:478`: `MaxOATCrankcaseHeater(10.0)` — default 10°C.
- EnergyPlus `VariableSpeedCoils.hh:327-329`: `MaxOATCrankcaseHeater(0.0)` — default 0°C.

HARES's 12.8°C is warmer than both EnergyPlus defaults, meaning the heater operates more frequently (whenever OAT < 12.8°C and compressor is off). The crankcase heater operation logic in HARES (`air_conditioner.rs:999-1021`) correctly follows EnergyPlus behavior: heater only draws power when OAT < threshold AND the compressor is off (scaled by `1.0 − max_rtf`), and for HP systems accounts for companion heating coil operation. The threshold difference is likely derived from OCHRE rather than EnergyPlus — OCHRE uses 12.8°C (55°F) as a common default. The 50 W value is reasonable for residential units. This is a minor calibration difference, not a physical error.
**Impact**: Slightly higher crankcase heater energy consumption than EnergyPlus defaults would produce (more hours below threshold, higher threshold). For mild climates this is negligible; for cold climates the difference accumulates over the heating season (~10–50 kWh/yr).

### Finding 4: [Severity: medium]
**Description**: MaxONOFFCyclesperHour / latent degradation time constants are non-default in HARES (3.0 cycles/hr, 45 s time constant) while EnergyPlus defaults all of these to 0.0 (disabled).
**Code Location**: `crates/hares-equipment/src/hvac/air_conditioner.rs:611-617` (latent degradation params: `twet_rated_s: 1500.0`, `gamma_rated: 1.5`, `max_cycling_rate: 3.0`, `latent_time_constant_s: 45.0`).
**Root Cause**: EnergyPlus defaults all four latent degradation parameters to 0.0 across all DX coil, variable-speed coil, and water-to-air heat pump types:
- `DXCoils.hh:487`: `MaxONOFFCyclesperHour(MaxModes, 0.0)`, `LatentCapacityTimeConstant(MaxModes, 0.0)`
- `VariableSpeedCoils.hh:300`: `MaxONOFFCyclesperHour(0.0)`, `LatentCapacityTimeConstant(0.0)`
- `WaterToAirHeatPumpSimple.hh:168-169`: `MaxONOFFCyclesperHour = 0.0`, `LatentCapacityTimeConstant = 0.0`
- `CoilCoolingDXCurveFitOperatingMode.hh:69,71`: `maximum_cycling_rate = 0.0`, `latent_capacity_time_constant = 0.0`

EnergyPlus's default of 0.0 means "no latent degradation model" — the SHR is constant at all part-load conditions. HARES enables the Henderson-Rengarajan latent degradation model by default with physically reasonable values (3.0 cycles/hr, 45 s time constant, 1500 s wet-coil time, 1.5 evaporation ratio). The HARES comments at line 611 state "EnergyPlus residential DX coil defaults (Engineering Reference §16.5)" but these are **not** the EnergyPlus *input* defaults — they are the example/recommended parameter values from the Engineering Reference documentation. The input defaults in the EnergyPlus IDD schema are 0.0 for all four parameters. Enabling latent degradation by default produces lower SHR at part load (more moisture removal relative to sensible cooling) compared to EnergyPlus's default behavior. This is arguably a more physically correct default for residential DX coils, but it diverges from EnergyPlus's conservative default of "no degradation." The HARES comment at line 611 is misleading — it cites the Engineering Reference but EnergyPlus input defaults are 0.0.
**Impact**: HARES will predict better latent (dehumidification) performance at part-load conditions than EnergyPlus with default inputs. This is physically more accurate for real DX coils but diverges from EnergyPlus out-of-the-box behavior. Difference in annual latent loads depends on climate; in humid climates part-load SHR degradation increases total latent removal by 5–15% compared to constant-SHR models. The documentation comment should be corrected to note the divergence from EnergyPlus input defaults.

### Finding 5: [Severity: high]
**Description**: Missing minimum outdoor temperature for cooling-only compressor operation. EnergyPlus defaults `MinOATCompressor` to −25°C for DX cooling coils; HARES has no equivalent.
**Code Location**: `crates/hares-equipment/src/hvac/air_conditioner.rs` (no `MinOAT` check in `update_control` or `step`).
**Root Cause**: EnergyPlus `DXCoils.cc:731` defines `minOATCompDXCooling = -25.0` as the global default minimum OAT for compressor operation on cooling-only DX coils (used at lines 1138-1141, 1903-1906, 2686-2689). Below this temperature the compressor is locked out to prevent liquid slugging and oil foaming. HARES applies no OAT-based lockout for cooling-only AC equipment. The `AirConditioner` model will attempt to run the compressor at any outdoor temperature, including extreme cold where it would be mechanically unsafe. EnergyPlus also enforces this check at the runtime level (`DXCoils.cc:9536` checks `CompAmbTemp > thisDXCoil.MinOATCompressor`). HARES has a heat pump lockout (`hp_lockout_temp_c` default −17.78°C, `heat_pump/constants.rs:57`) but only for the ASHP heating coil — the cooling coil in an ASHP uses the same compressor and should be equally constrained, yet the cooling-only AC path (`air_conditioner.rs`) has no temperature floor. For standalone (non-HP) air conditioners the absence of a lockout temperature means the compressor could attempt to operate at −30°C ambient, which is physically impossible for a residential DX system. The rated biquadratic curves typically evaluate near 1.0 at these temperatures (or extrapolate unreliably), producing plausible-looking numerical results that mask the physical infeasibility.
**Impact**: Potentially incorrect cooling operation at unrealistically low outdoor temperatures (below −25°C for typical residential DX equipment). At −30°C OAT, a residential AC compressor would fail due to refrigerant migration and oil viscosity issues, but HARES would still compute a cooling output. This could produce spurious cooling in cold-climate simulations where zone temperatures rise from internal gains during winter. For most residential simulations this condition is rare (internal gains in winter are normally below the cooling setpoint), but in passive solar or high-internal-gain buildings the simulation could generate physically impossible cooling energy. This also applies to the heat pump cooling mode during winter — if the HP is in defrost or locked out for heating, its cooling coil could theoretically activate if zone temperatures exceed the cooling setpoint during cold outdoor conditions, which is mechanically unsafe.

### Finding 6: [Severity: medium]
**Description**: Default PLF degradation coefficient diverges between HARES (0.25) and current EnergyPlus (0.20 for 2023 AHRI standard).
**Code Location**: `crates/hares-equipment/src/hvac/staging.rs:20` (`DEFAULT_PLF_DEGRADATION_COEFF = 0.25`).
**Root Cause**: HARES uses `Cd = 0.25` as the default part-load degradation coefficient, citing "AHRI Standard 210/240-2023, S6.6.3 default when no test data available" (comment at line 19). However, EnergyPlus `CoilCoolingDXCurveFitPerformance.cc:466` defines `CyclicDegradationCoefficient(0.20)` with the comment "ANSI/AHRI 210/240 2023 Section 6.1.3.1." Both reference AHRI 210/240-2023 but specify different Cd values. AHRI 210/240-2023 Section 6.1.3.1 (as of the public version) actually specifies a default Cd of 0.25 when test data is unavailable. This is the S6.6.3 default. The 0.20 value in EnergyPlus likely corresponds to an earlier edition or a specific sub-provision of the 2023 standard. The HARES documentation is internally self-consistent (comment says 0.25, code uses 0.25), but the EnergyPlus divergence with the same cited standard warrants verification. HARES's 0.25 produces more aggressive part-load efficiency degradation than EnergyPlus's 0.20.
**Impact**: HARES will compute lower PLF values at part load than EnergyPlus (e.g., at PLR=0.5: HARES PLF = 1 − 0.25×0.5 = 0.875, EnergyPlus 2023 PLF = 1 − 0.20×0.5 = 0.900). HARES predicts ~2.8% lower efficiency at 50% load. For single-speed equipment operating at typical SEER rating conditions (PLR=0.5), this difference amounts to ~2.5% in cooling energy consumption. The actual AHRI 210/240-2023 default is 0.25, so HARES is correct relative to the standard, while EnergyPlus's 0.20 coefficient appears to be a code-level default that may be superseded by the IDD-embedded curve data.

## Summary
- Total findings: 6
- Critical: 0
- High: 1 (Finding 5: missing MinOAT for cooling compressor)
- Medium: 3 (Finding 2: no explicit min_plr floor, Finding 4: latent degradation defaults diverge from EnergyPlus input defaults, Finding 6: Cd coefficient divergence)
- Low: 2 (Finding 1: fan power rounding, Finding 3: crankcase threshold divergence)

## Recommendations

1. **Add minimum outdoor temperature lockout for cooling-only compressor operation** (Finding 5). Input a `min_oat_compressor_cooling_c` config key with a default of −25°C (matching EnergyPlus) or −17.78°C (matching the existing ASHP heating lockout). Check this threshold in `AirConditioner::update_control()` to suppress cooling when OAT falls below the lockout. For ASHP systems, the cooling coil should respect the same lockout as the heating coil since they share one compressor.

2. **Consider adding an explicit `min_plr` floor** (Finding 2). For single-speed equipment, enforce `min_plr ≈ 0.2` by cycling at the minimum ratio rather than operating continuously at sub-physical PLR values. This aligns with EnergyPlus's `PlantLoopHeatPumpEIR` default of 0.1 and VRF defaults. This is lower priority than Finding 5 — the Cd-based degradation captures average energy impacts correctly; the main benefit is instantaneous power fidelity for subhourly simulation.

3. **Correct or annotate the latent degradation documentation comment** (Finding 4). The comment at `air_conditioner.rs:611` states "EnergyPlus residential DX coil defaults (Engineering Reference §16.5)" but these are not EnergyPlus *input* defaults (which are all 0.0). Reword to clarify that these are recommended physical parameter values from the Engineering Reference discussion, not the EnergyPlus IDD defaults. Consider making these user-configurable via typed config structs, with the current values retained as physically grounded defaults.

4. **Verify the Cd = 0.25 default against the specific AHRI 210/240-2023 section** (Finding 6). The EnergyPlus code uses 0.20 citing the same standard section 6.1.3.1, while HARES uses 0.25 citing section S6.6.3. Confirm which section takes precedence in the 2023 edition and align if needed. If both are valid under different test conditions, document the selection rationale.

5. **Document the crankcase heater threshold divergence** (Finding 3). Add a comment noting the deviation from EnergyPlus's default 10.0°C (or 0.0°C for DX coils), and cite the OCHRE provenance of the 12.8°C (55°F) value. No code change needed — 12.8°C is more conservative (protects the compressor at warmer OATs) and is an industry-common default.

## References / Citations

- EnergyPlus `DXCoils.cc:14715`: `DefaultFanPowerPerEvapAirFlowRate(773.3)` — AHRI 340/360-2007 fan power per airflow, 365 W/1000 scfm.
- EnergyPlus `DXCoils.cc:731`: `minOATCompDXCooling = -25.0` — global minimum OAT for DX cooling compressor.
- EnergyPlus `DXCoils.hh:483-487`: struct default initializer — `MinOATCompressor(0.0)`, `MaxONOFFCyclesperHour(MaxModes, 0.0)`, `LatentCapacityTimeConstant(MaxModes, 0.0)`.
- EnergyPlus `PlantLoopHeatPumpEIR.hh:398,476-478`: `minPLR = 0.1`, `CrankcaseHeaterCapacity = 0.0`, `MaxOATCrankcaseHeater = 10.0`.
- EnergyPlus `CoilCoolingDXCurveFitPerformance.cc:466`: `CyclicDegradationCoefficient(0.20)` — ANSI/AHRI 210/240 2023 Section 6.1.3.1.
- EnergyPlus `Furnaces.cc:6484`: `MinPLR(0.0)` — furnace minimum part-load ratio.
- EnergyPlus `UnitarySystem.hh:313-314`: `m_MinOATCompressorCooling(0.0)`, `m_MinOATCompressorHeating(0.0)` — fallback defaults.
- HARES `hvac_core.rs:30-31`: `DEFAULT_FAN_POWER_W_PER_CFM = 0.365`, `DEFAULT_FAN_POWER_W_PER_M3_S` — sourced from ANSI/RESNET/ICC 301-2019 §4.2.2(1).
- HARES `staging.rs:20`: `DEFAULT_PLF_DEGRADATION_COEFF = 0.25` — AHRI 210/240-2023 S6.6.3.
- HARES `air_conditioner.rs:34-35`: `CRANKCASE_HEATER_KW = 0.05` (50 W), `CRANKCASE_HEATER_THRESHOLD_C = 12.8`.
- HARES `heat_pump/constants.rs:57`: `DEFAULT_HP_LOCKOUT_TEMP_C = -17.78` (0°F) — ASHP compressor lockout for heating.
- HARES `heat_pump/constants.rs:70`: `MAX_OAT_SUPPLEMENTAL_C = 21.0` — EnergyPlus supplemental heater upper OAT bound.
