# Review 04 — HVAC Equipment, Water Heater, and HPXML Wiring
**Base commit**: `c1abb8af9d3087873ba4d7010f3383a647a7fd79`
**HEAD**: `d5de344` (review: create tickets for hvac fixes and improvements)
**Reviewer**: first-principles physics and test-integrity audit
**Date**: 2026-04-23

---

## Executive Summary

The diff spans silent-default removal in the HPXML resolver (AFUE now required, lat/lon required
for duct DSE, HPWH storage setpoint required), `radiant_gain_w: 0.0` field additions throughout
equipment port emission, `tank_height_m` promoted to `Option`, and heating-setpoint wiring for all
heating-side equipment builders. All of these are correct improvements and the test suite confirms
they work as intended.

**Test suite is clean**: hares-equipment 1310 passing / 0 failing / 1 ignored (unrelated doctest in
`macros.rs`); hares-io 718 passing / 0 failing / 0 ignored. Clippy: 0 warnings, 0 errors.

Three persistent physics/wiring defects are not addressed by this diff and remain open:

- **F1 (HIGH)** — `apply_default_hvac_speed_fallback` uses `.unwrap_or(0.0)` for SEER; a
  CoolingSystem that only specifies EER gets `seer = 0.0` → single-speed. Directly contradicts the
  no-silent-defaults enforcement being applied everywhere else in this diff.
- **F2 (HIGH)** — `BackupHeatingSwitchoverTemperature` is mapped to both `hp_lockout_temp_c` AND
  `er_lockout_temp_c`. HPXML defines this field as backup activation threshold only, not compressor
  cutout. Causes premature compressor lockout when the field is present without
  `CompressorLockoutTemperature`.
- **F3 (MEDIUM)** — ER backup modulated by PLR at `heater.rs:1039` contradicts the comment at
  line 1078 ("ER is not modulatable"); ticket-015 open and unresolved.
- **F4 (MEDIUM)** — `IdealHvac` non-ideal fallback uses rated capacity without biquadratic
  correction; ticket-002 open and unresolved.

One new finding introduced by this diff:

- **N3 (MEDIUM)** — `reconcile_setpoint_pair` silently mutates HPXML setpoints without returning
  an error. A valid 1°C gap becomes 2°C without the caller having a code path to detect the
  change.

Combi boiler + indirect tank (`WaterHeatingType=space-heating boiler with storage tank`) is **not
implemented**. No ticket tracks this gap.

---

## 1. Constants Audit Table

| Constant | File:Line | Value | Primary Source | Verdict |
|----------|-----------|-------|----------------|---------|
| `SEER2_TO_SEER_FACTOR` | `resolve_hvac.rs:27` | `1/0.95 ≈ 1.053` | OpenStudio-HPXML consensus shortcut. AHRI 210/240-2023 redefines test conditions (ESP 0.5 in/wc → 0.1 in/wc) — no fixed scalar ratio is defined in the standard. The 0.95 is widely used in ResStock/OpenStudio-HPXML for residential split systems. | APPROXIMATION — add citation comment |
| `HSPF2_TO_HSPF_FACTOR` | `resolve_hvac.rs:28` | `1/0.95 ≈ 1.053` | Same approximation. AHRI 210/240-2023 Appendix M1. ResStock uses this for residential ASHP. | APPROXIMATION — add citation comment |
| BTU/Wh conversion | multiple sites | `3.412_141_633` | NIST: 1 Wh = 3600 J; 1 BTU = 1055.05585262 J → 3600/1055.05585 = 3.41214 ✓ | Correct |
| `DEFAULT_FAN_POWER_W_PER_CFM` | `hvac_core.rs:29` | `0.365` | ANSI/RESNET/ICC 301-2019 §4.2.2(1) Table 4.2.2(1) supply fan default ✓ | Correct |
| `DEFROST_ENABLE_TEMP_C` | `constants.rs:6` | `4.4445` (40°F) | EnergyPlus Engineering Reference §16.3.4 "Maximum Outdoor Dry-Bulb Temperature for Defrost Operation" default = 40°F ✓ | Correct |
| `DEFAULT_DEFROST_TIME_FRACTION` | `constants.rs:24` | `0.058` (~3.5 min/hr) | EnergyPlus Engineering Reference §16.3.4 timed defrost default ✓ | Correct |
| `TIMED_DEFROST_CAP_MULT_BASE` | `constants.rs:26` | `0.909` | EnergyPlus DXCoils.cc DOE-2 regression coefficients ✓ | Correct |
| `TIMED_DEFROST_CAP_MULT_SLOPE` | `constants.rs:27` | `107.33` | EnergyPlus DXCoils.cc ✓ | Correct |
| `TIMED_DEFROST_PWR_MULT_BASE` | `constants.rs:29` | `0.90` | EnergyPlus DXCoils.cc ✓ | Correct |
| `TIMED_DEFROST_PWR_MULT_SLOPE` | `constants.rs:30` | `36.45` | EnergyPlus DXCoils.cc ✓ | Correct |
| `DEFROST_EIR_CURVE_TEMP_MIN_C` | `constants.rs:32` | `15.555` (60°F) | EnergyPlus Engineering Reference defrost EIR biquadratic floor ✓ | Correct |
| `DEFAULT_HP_LOCKOUT_TEMP_C` | `constants.rs:34` | `-17.78` (0°F) | 0°F is a common residential ASHP limit; uncited in code. Should reference manufacturer default range or AHRI 210/240 Appendix C. | Uncited — add citation |
| `DEFAULT_ER_LOCKOUT_TEMP_C` | `constants.rs:38` | `4.44` (40°F) | OCHRE HVAC.py backup_lockout_temp default ✓ | Correct — cite OCHRE |
| `AIRFLOW_HEATING_M3_S_PER_W` | `hvac_core.rs:73` | `4.697e-5` (~350 CFM/ton) | OCHRE/ResStock heating baseline; HERS Addendum 82 ✓ | Correct |
| `AIRFLOW_CENTRAL_AC_M3_S_PER_W` | `hvac_core.rs:74` | `5.368e-5` (~400 CFM/ton) | RESNET HERS Addendum 82; OpenStudio-HPXML ✓ | Correct |
| `AIRFLOW_MSHP_COOLING_M3_S_PER_W` | `hvac_core.rs:75` | `4.187e-5` (~312 CFM/ton) | OCHRE ductless split assumption ✓ | Correct — cite OCHRE |
| `DEFAULT_BIQUADRATIC_X1_BOUNDS` | `hvac_core.rs:45` | `(-100, 100)` | No physical basis. AHRI 210/240 test envelope is 55–70°F WB / 65–125°F ODB (cooling) and 17–70°F ODB / 60–75°F IDB (heating). These bounds are far too wide for physically meaningful extrapolation guard. | DEFECT — ticket-003 |
| `HR_RATED_DB_C` / `HR_RATED_WB_C` | `coil_physics.rs:25–26` | `26.667` / `19.444` (80/67°F) | AHRI 210/240 §6.1 rated indoor conditions ✓ | Correct |
| `WATER_DENSITY_KG_PER_M3` | `water_heater/mod.rs` | `1000.0` | Approximation: at 50°C actual is ~988 kg/m³ (NIST WebBook). Overestimates thermal mass by ~1.2%. | APPROXIMATION — uncited |
| `WATER_SPECIFIC_HEAT_J_PER_KG_K` | `tank.rs:14` | `4183.0` | NIST at 60°C: 4183 J/(kg·K) ✓ | Correct |
| Gas WH recovery efficiency default | `water_heater_ua.rs:155` | `0.78` | Physically reasonable for mid-efficiency gas WH; should cite DOE 10 CFR 430 Subpart B or require field. | Uncited — add citation or require field |
| `T_IN_EF_F` | `water_heater_ua.rs:14` | `58.0°F` | DOE 10 CFR 430 Appendix E EF test mains temperature ✓ | Correct |
| `T_ENV_F` | `water_heater_ua.rs:18` | `67.5°F` | DOE 10 CFR 430 Appendix E ambient ✓ | Correct |
| `T_SETPOINT_EF_F` | `water_heater_ua.rs:20` | `135.0°F` | DOE 10 CFR 430 Appendix E EF test setpoint ✓ | Correct |
| `T_SETPOINT_UEF_F` | `water_heater_ua.rs:22` | `125.0°F` | DOE 10 CFR 430 Appendix E UEF test setpoint ✓ | Correct |
| HPWH UA bins (3.6/4.0/4.7 BTU/hr·°F) | `water_heater_ua.rs:101–107` | — | ResStock `waterheater.rb` L765 ✓ | Correct |
| EF→UEF regression | `resolve_water_heater.rs:189` | `(0.60522 + ef) / 1.2101` | RESNET 301-2022 Appendix B regression — not from AHRI 1600 first principles | APPROXIMATION — cite RESNET 301 |
| UEF→COP multiplier | `resolve_water_heater.rs:196` | `1.174_536_058` | Linear fit from OCHRE hpxml.py. AHRI 1600 defines a more rigorous test protocol; this is a shortcut approximation without range bounds. | APPROXIMATION — cite OCHRE; add range bounds |
| `DEFAULT_RATED_COP` (HPWH compressor) | `hpwh_compressor.rs` | `3.45` | OCHRE GE GeoSpring default ✓ | Correct |
| `DEFAULT_DEADBAND_C` (HPWH) | `hpwh_compressor.rs` | `8.166` (14.7°F) | OCHRE HPWH-specific default (HVAC.py) ✓ | Correct |
| `MSHP_PAN_HEATER_DEFAULT_KW` | `constants.rs:62` | `0.150` | OCHRE MinisplitAHSPHeater (HVAC.py:1482) ✓ | Correct |
| Default tank nodes (HPWH) | `heat_pump_wh.rs` | `6` via `unwrap_or(6)` | OCHRE uses 12 nodes. 6 nodes gives coarser stratification. No citation for degrading to 6. | Uncited degradation from OCHRE default |
| `SETPOINT_RECONCILE_GAP_C` | `resolve_hvac.rs` (new constant) | `2.0` | Derived from thermostat `validate_for_deadband` invariant (2×1°C hysteresis). Internally justified but no HPXML/ASHRAE citation. | Acceptable — justification present in comment |

---

## 2. Formulae Audit Table

| Formula | File:Line | Physics Claim | Primary Source | Verdict |
|---------|-----------|---------------|----------------|---------|
| EIR from SEER: `3.412141633 / seer` | `resolve_hvac.rs` | EIR = BTU/Wh ÷ SEER | AHRI 210/240 §11; 1 Wh = 3.41214 BTU ✓ | Correct |
| EIR from HSPF: `3.412141633 / hspf` | `resolve_hvac.rs` | Same basis for heating | AHRI 210/240 ✓ | Correct |
| Coil BF: `(h_out − h_ADP) / (h_in − h_ADP)` | `coil_physics.rs` | Enthalpy-based bypass factor | ASHRAE HOF 2021 Ch.18 Eq.63 ✓ | Correct |
| Ao factor: `−ln(BF) × ṁ_da` | `coil_physics.rs` | Coil Ao per ASHRAE HOF Ch.18 Eq.65 | ASHRAE HOF 2021 ✓ | Correct |
| Henderson-Rengarajan Twet/γ normalization | `coil_physics.rs` | `γ_eff` scaled by DB-WB depression; `ton` from max cycling rate | H&R 1996 Table 2 ✓ | Correct |
| H-R `To` fixed-point solve (20 iter, 0.1% tol) | `coil_physics.rs` | Iterative per H&R 1996 Eq.6 | H&R 1996 ✓ | Correct |
| H-R LHR multiplier: `(ton − To) / (ton + τ·(e^{−ton/τ} − 1))` | `coil_physics.rs` | Moisture re-evaporation ratio | H&R 1996 Eq.4 ✓ | Correct |
| `SHR_eff = 1 − (1 − SHR_ss) × lhr_mult` | `coil_physics.rs` | Effective SHR with cycling degradation | H&R 1996 Eq.3 ✓ | Correct |
| Biquadratic: `c0 + c1·x1 + c2·x1² + c3·x2 + c4·x2² + c5·x1·x2` | `hvac_core.rs` | Standard biquadratic form | EnergyPlus Engineering Reference §16.2 ✓ | Correct |
| PLF degradation: `PLF = 1 − Cd·(1 − PLR)` | `staging.rs` | Part-load factor from cycling degradation | AHRI 210/240 §11.12 ✓ | Correct |
| RTF: `PLR / PLF` | `air_conditioner.rs` | Runtime fraction | AHRI 210/240 ✓ | Correct |
| OnDemand defrost time fraction: `1 / (1 + 0.01446 / Δω)` | `defrost.rs:138` | Humidity-based defrost fraction | EnergyPlus Engineering Reference §16.3.4 ✓ | Correct |
| Timed defrost cap mult: `0.909 − 107.33 × Δω` clamped to [0,1] | `defrost.rs:180–182` | DOE-2/EnergyPlus regression | EnergyPlus DXCoils.cc ✓ | Correct |
| Duct DSE distribution: conditioned=`dse·(1−bsmt_frac)`, duct=`1−dse` | `duct_distribution.rs:40–58` | Capacity fraction allocation | ASHRAE 152-2004 §7; OCHRE HVAC.py:188–197 ✓ | Correct |
| Fan heat: added to `total_sensible_w` before DSE | `furnace.rs:173–174`, `heater.rs:1065` | Fan waste heat to supply air stream | OCHRE HVAC.py:543 ✓ | Correct |
| Burch & Erickson EF→UA: `Q_load·(1/EF − 1) / ((T_sp − T_env) × 24)` | `water_heater_ua.rs:128` | Electric storage standby UA | Burch & Erickson 2004 §3 ✓ | Correct |
| Maguire & Roberts UEF→UA (mixed-draw denominator) | `water_heater_ua.rs:134` | UEF test protocol UA | Maguire & Roberts 2020 ✓ | Correct |
| Gas storage UA (with RE) | `water_heater_ua.rs:151` | Uses recovery efficiency; `re = unwrap_or(0.78)` | Burch & Erickson 2004 with gas recovery term ✓; RE default uncited | Partially uncited |
| HPWH UA via volume bins | `water_heater_ua.rs:101–107` | Three-bin volume lookup | ResStock waterheater.rb L765 ✓ | Correct |
| Tank stratification: N-node 1-D FD + inversion stack mixing | `tank.rs` | Energy balance per node, buoyancy correction | EnergyPlus Engineering Reference §14.8 ✓ | Correct |
| 2-node volume split: top 1/3, bottom 2/3 | `tank.rs:134–135` | OCHRE TwoNodeWaterModel fractions | OCHRE Water.py:475 ✓ | Correct |
| End-cap UA: `0.1 × ua_side` per boundary node | `tank.rs:157` | Flat cap heat loss fraction | EnergyPlus Engineering Reference §14.8 ✓ | Correct |
| Non-ideal HVAC fallback: `rated_capacity_w × load_fraction` | `ideal_hvac.rs:526–527` | No biquadratic correction applied | **DEFECT** — ticket-002 |
| ER backup non-ideal: `backup_capacity_w × plr` | `heater.rs:1039` | Modulates on/off equipment by PLR | **DEFECT** — ticket-015; ER strip is binary |
| EF→UEF: `(0.60522 + ef) / 1.2101` | `resolve_water_heater.rs:189` | RESNET regression for HPWH | RESNET 301-2022 Appendix B (regression, not physics) ✓ | Correct as approximation |
| UEF→COP: `1.174536058 × uef` | `resolve_water_heater.rs:196` | Linear fit | OCHRE hpxml.py (no physics derivation); no range bounds | APPROXIMATION — add bounds |

---

## 3. HPXML → Equipment Wiring Audit

### (a) Central AC + Gas Furnace

**HPXML path**: `HeatingSystem[HeatingSystemType/Furnace/gas]` + `CoolingSystem[CoolingSystemType/central air conditioner]`

| Field | HPXML Element | Config Field | Status |
|-------|--------------|--------------|--------|
| Heating capacity | `HeatingCapacity` (Btu/hr) | `GasFurnaceConfig::capacity_w` | Wired |
| AFUE | `AnnualHeatingEfficiency[AFUE]` | `GasFurnaceConfig::afue` | Required — `MissingField` error if absent (fixed in this diff) |
| Fan power | `extension/FanPowerWattsPerCFM` or `FanPowerWatts` | `GasFurnaceConfig::fan_power_w` | Wired |
| Number of speeds | `CompressorType` or SEER inference | `number_of_speeds` | Wired |
| Duct DSE | Computed from duct geometry + lat/lon (ASHRAE 152) | `GasFurnaceConfig::ducts.dse_heat` | Wired; lat/lon and conditioned volume now required when ducts exist (fixed in this diff) |
| Heating setpoint | `HVACControl/extension/WeekdaySetpointTempsHeatingSeason` | `heating_setpoint_source` | Wired in this diff |
| Cooling capacity | `CoolingCapacity` (Btu/hr) | `capacity_w` | Wired |
| SEER → EIR | `AnnualCoolingEfficiency[SEER]` | `eir = 3.412/SEER` | Wired; SEER absent → `seer = 0.0` silent default — F1 |
| SHR | `SensibleHeatFraction` | `shr` | Wired |
| Cooling setpoint | `HVACControl/extension/WeekdaySetpointTempsCoolingSeason` | `cooling_setpoint_source` | Wired in this diff |
| `CrankcaseHeaterWatts` | `CrankcaseHeaterWatts` | — | **NOT WIRED** — ticket-023 G1 |

**Energy balance verdict**: Gas furnace fuel_in = gross_capacity / AFUE. Zone receives
gross_capacity × DSE + fan_heat (fan heat added once to `total_sensible_w` before DSE). Duct zone
receives gross_capacity × (1 − DSE). AC: compressor + fan electrical_in; zone receives
total_capacity × SHR_eff sensible (negative) + total_capacity × (1 − SHR_eff) latent (negative);
fan heat offsets (positive). No double-count confirmed.

### (b) ASHP with ER Backup

**HPXML path**: `HeatPump[HeatPumpType/air-to-air]` with `BackupSystemFuel=electricity`

| Field | HPXML Element | Config Field | Status |
|-------|--------------|--------------|--------|
| Heating capacity | `HeatingCapacity` (Btu/hr) | `heating_capacity_w` | Wired |
| HSPF / HSPF2 | `AnnualHeatingEfficiency[HSPF/HSPF2]` | EIR via `3.412/HSPF` or `3.412/(HSPF2 × 1/0.95)` | Wired (HSPF2 uses approximation — see §1) |
| SEER / SEER2 | `AnnualCoolingEfficiency[SEER/SEER2]` | EIR conversion | Wired |
| Backup capacity | `BackupHeatingCapacity` | `backup_capacity_w` | Wired; defaults to `DEFAULT_BACKUP_CAPACITY_W = 5000 W` — uncited fallback |
| Backup efficiency | `BackupAnnualHeatingEfficiency` | `backup_eir` | Wired |
| Backup fuel | `BackupSystemFuel` | `backup_fuel_type` | Wired |
| Compressor lockout temp | `CompressorLockoutTemperature` | `hp_lockout_temp_c` | Wired |
| Backup lockout temp | `BackupHeatingLockoutTemperature` | `er_lockout_temp_c` | Wired |
| `BackupHeatingSwitchoverTemperature` | — | Both `hp_lockout_temp_c` AND `er_lockout_temp_c` | **INCORRECT** — F2; should map to `er_lockout_temp_c` only |
| `DefrostType` | `DefrostType` | — | **NOT WIRED** — ticket-023 G2 |
| `DefrostControl` | `DefrostControl` | — | **NOT WIRED** — ticket-023 G2 |
| `CrankcaseHeaterWatts` | `CrankcaseHeaterWatts` | — | **NOT WIRED** — ticket-023 G1 |
| `MinimumCapacity` | `MinimumCapacity` | — | **NOT WIRED** — ticket-023 G3 |
| `ChargeDefectRatio` | `extension/ChargeDefectRatio` | Inserted in params but never consumed by physics | **NOT CONSUMED** — ticket-023 G5 |
| Heating/cooling setpoints | `HVACControl` | Both sources | Wired in this diff |

**Energy balance verdict**: `hp_electric_w + er_electric_w + fan_w + pan_heater_w` → electrical
port. `hp_capacity_w + er_capacity_w + fan_w` → `write_zone_thermal_contributions` (DSE-distributed).
Reverse-cycle defrost: `q_defrost_w` correctly subtracts from zone output; extra_power_w added to
electrical. ER at `heater.rs:1039` is `backup_capacity_w × plr` in non-ideal mode — **defect F3**.
No other double-counts.

### (c) Mini-Split HP (MSHP)

**HPXML path**: `HeatPump[HeatPumpType/mini-split]`

| Field | HPXML Element | Config Field | Status |
|-------|--------------|--------------|--------|
| Heating/cooling capacities | Same as ASHP | Same | Wired |
| Number of speeds | Hardcoded 4 | `number_of_speeds=4`, `speed_control_mode=variable_speed` | Correct — MSHP always 4-speed in HARES |
| `MinimumCapacity` | `MinimumCapacity` | — | **NOT WIRED** — ticket-023 G3; minimum is 25% fraction hardcoded |
| Pan heater | — | `pan_heater_kw=0.150`, `pan_heater_temp_c=0.0` | OCHRE defaults applied at init |
| Ducts | Skipped (ductless) | `DuctConfig::default()` | Correct |
| Setpoints | `HVACControl` | Both sources | Wired in this diff |

**Energy balance verdict**: Same path as ASHP. Backup is `0.0` for pure MSHP. Pan heater goes to
outdoor unit, not indoor zone. Correct.

### (d) Gas Tankless Water Heater

**HPXML path**: `WaterHeatingSystem[WaterHeaterType=instantaneous water heater]` with gas/propane/oil fuel

| Field | HPXML Element | Config Field | Status |
|-------|--------------|--------------|--------|
| Fuel type | `FuelType` | `fuel_type` | Required |
| EF | `EnergyFactor` | `energy_factor` | Wired (optional) |
| UEF | `UniformEnergyFactor` | `uniform_energy_factor` | Wired (optional) |
| Heating capacity | `HeatingCapacity` (Btu/hr) | `heating_capacity_w` | Wired |
| Setpoint | `HotWaterTemperature` (°F → °C) | `setpoint_c` | Wired |
| Performance adjustment | `PerformanceAdjustment` | `performance_adjustment` | Wired; defaults 0.94 (UEF-only) or 0.92 (EF) per RESNET 301 |
| Bedrooms / draw | `NumberofBedrooms` | `number_of_bedrooms`, `avg_water_draw_l_per_day` | Wired |

**Energy balance verdict**: Fuel input × EF/UEF × performance_adjustment = thermal output. No tank
standby losses. Correct.

### (e) HPWH with Tank

**HPXML path**: `WaterHeatingSystem[WaterHeaterType=heat pump water heater, FuelType=electricity]`

| Field | HPXML Element | Config Field | Status |
|-------|--------------|--------------|--------|
| Tank volume | `TankVolume` (gal × 0.9 correction) | `tank_volume_m3` | Wired |
| Tank height | `TankHeight` (ft → m) | `tank_height_m` (Option) | Corrected in this diff — no longer forced to 4 ft |
| UEF | `UniformEnergyFactor` | COP via `1.174536058 × UEF` | Wired (approximation) |
| EF | `EnergyFactor` | EF→UEF regression then COP | Wired (regression) |
| Hot water setpoint | `HotWaterTemperature` (°F → °C) | `setpoint_c`, `tempering_valve_setpoint_c` | Now required for non-low-power path — **corrected in this diff** |
| Backup element power | `HeatingCapacity` (Btu/hr → W) | `backup_element_power_w` | Wired |
| Tank nodes | Not in HPXML | `tank_nodes = None` → equipment defaults to 6 | Defaults to 6 nodes; OCHRE uses 12; uncited degradation |
| UA | Computed via volume bins | `ua_w_per_k` | Wired |
| Performance adjustment | `PerformanceAdjustment` | `performance_adjustment` | Wired |
| Zone location | `Location` | `zone_type` | Wired |
| Jacket R-value | `WaterHeaterInsulation/Jacket/JacketRValue` (IP→SI) | `jacket_r_value_m2_k_w` | Wired |
| First hour rating | `FirstHourRating` (gal → m³) | `first_hour_rating_m3` | Wired |

**Energy balance verdict**: Compressor draws from installation zone air (negative sensible and
latent extraction, `InternalGain` category). Heat deposited to tank. Backup element draws from
electrical port. Jacket/skin losses emitted to zone as `JacketLoss` sensible. `wall_heat_fraction`
route to wall mass emitted separately. All `radiant_gain_w: 0.0` additions in this diff correct.
No double-count.

### (f) Combi Boiler + Indirect Tank

**HPXML path**: `WaterHeatingSystem[WaterHeaterType=space-heating boiler with storage tank]`

**Status: NOT IMPLEMENTED.**

Resolver at `resolve_water_heater.rs:503–508` falls through to an unsupported-type error. No
equipment class exists for indirect-tank water heating. HPXML files with combi-boiler DHW will
fail at parse time. No ticket tracks this gap.

### (g) Room Air Conditioner

**HPXML path**: `CoolingSystem[CoolingSystemType=room air conditioner]`

| Field | HPXML Element | Config Field | Status |
|-------|--------------|--------------|--------|
| Cooling capacity | `CoolingCapacity` (Btu/hr → W) | `capacity_w` | Wired |
| EER | `AnnualCoolingEfficiency[EER]` | EIR via `3.412/EER` | Wired — correct (EER not SEER) |
| SEER absent guard | `try_build_room_ac_config` returns `None` when only SEER present | Correct — SEER cannot substitute for EER per AHRI 310/380 | Wired correctly |
| SHR | `SensibleHeatFraction` | `shr` | Wired |
| Setpoints | `HVACControl` | Both sources | Wired in this diff |
| Ducts | Skipped | `DuctConfig::default()` | Correct |

**Energy balance verdict**: Same path as central AC. Fan is internal (no separate routing).
Ticket-022 (ideal-target mode for RoomAC) is open and unaddressed.

---

## 4. Energy-Balance Audit per Equipment Class

| Equipment | Inputs | Outputs | Balance Verdict |
|-----------|--------|---------|-----------------|
| GasFurnace | fuel (gas) + fan (electric) | zone sensible via DSE + duct-zone sensible (DuctLoss category) | Correct. Fan heat added once to `total_sensible_w` before DSE distribution; no double-count |
| ElectricFurnace | element (electric) + fan (electric) | zone sensible via DSE | Correct; no fuel port |
| GasBoiler | fuel (gas) + pump (electric) | zone thermal via fluid loop + jacket loss if set | Correct |
| GasWH | fuel (gas + pilot) | tank heat + flue loss + skin loss to zone (`JacketLoss`) | Correct — `fuel_input_w = burner_input_w + pilot_power_w`; skin loss emitted |
| ResistanceWH | electric (upper + lower elements) | tank heat + skin loss to zone | Correct |
| TanklessWH | fuel or electric | thermal output at efficiency × `performance_adjustment` | Correct |
| HPWH | electric (compressor + backup element + fan + parasitic) | tank heat + zone sensible extraction (negative `InternalGain`) + skin loss + wall sensible | Correct — three zone contribution categories; all `radiant_gain_w: 0.0` verified |
| AirConditioner | electric (compressor + fan + crankcase folded into compressor_kw) | zone sensible (negative) + latent (negative) + fan heat (positive) | Crankcase heat included in `compressor_kw` telemetry but **not routed to zone thermal port when compressor is off** — ticket-023 G1 gap |
| ASHP Heater | electric (compressor + fan + pan_heater + defrost_extra) + optional gas (ER backup) | zone sensible (positive) via DSE; defrost reduces zone output | ER capacity = `backup_capacity_w × PLR` in non-ideal path — **defect F3** (should be binary on/off) |
| Dehumidifier | electric | zone sensible (motor heat) + latent removal | `radiant_gain_w: 0.0` added; correct |
| MSHP Cooler | electric (compressor + fan) | zone sensible (negative) + latent (negative) | Correct; ductless: no DSE |

---

## 5. Port Emission Audit

### ThermalPort

All equipment that emits `PortContribution::Thermal` now explicitly sets `radiant_gain_w: 0.0`
(corrected in this diff at: `gas.rs:524`, `resistance.rs:554`, `heat_pump_wh.rs` three sites,
`boiler.rs`, `dehumidifier.rs`, `duct_distribution.rs:116`). Pre-existing correct site:
`ideal_hvac.rs:568`.

`ThermalCategory` classification:
- HVAC heating/cooling: `HvacHeating` / `HvacCooling` ✓
- Standby/jacket losses: `JacketLoss` ✓
- HPWH compressor zone interaction: `InternalGain` ✓
- Dehumidifier motor heat: `InternalGain` ✓
- Duct loss: `DuctLoss` ✓ (tagged at the zone where duct losses are deposited, not conditioned zone)

No double-counted thermal contributions observed.

### ElectricalPort

All equipment uses `PortContribution::Electrical { active_power_kw, reactive_power_kvar: 0.0 }`.
No reactive power modeled (acceptable for residential simulation). Crankcase heater power folded
into `compressor_kw` telemetry but corresponding heat is not emitted to zone thermal port when
compressor is off — ticket-023 G1 gap.

### FuelPort

`PortContribution::Fuel` emitted by: GasFurnace, GasBoiler, GasWH, HP heater with gas backup.
All guard on `fuel_input_w > 0.0` before emitting. Correct.

### FluidPort

`PortContribution::Fluid` emitted by GasBoiler and ElectricBoiler. Correct.

---

## 6. Test Integrity Audit

### Test counts (verified by running test suite at HEAD)

| Crate | Passing | Failing | Ignored |
|-------|---------|---------|---------|
| hares-equipment | 1310 | 0 | 1 (unrelated doctest in `macros.rs`) |
| hares-io | 718 | 0 | 0 |

### Comment/Attribute Mismatches (misleading `#[ignore]` claims)

Three test functions carry comments claiming `#[ignore]` but lack the actual Rust attribute. The
tests run and pass, so no CI noise results today, but the comments will mislead any developer
expecting these tests to be skipped.

| File | Line | Function | Comment says | Actual annotation |
|------|------|----------|-------------|-------------------|
| `hares-io/tests/hpxml_parity.rs` | ~698 | `ac_has_startup_capacity_degradation_default` | "Marked `#[ignore]`" | `#[test]` only |
| `hares-io/tests/hpxml_parity.rs` | ~738 | `ashp_backup_lockout_temperature_extracted` | "Marked `#[ignore]`" | `#[test]` only |
| `hares-equipment/tests/water_heater_parity.rs` | ~585 | `hpwh_cop_at_multiple_ambient_temps` | "marked `#[ignore]` to prevent CI noise" | `#[test]` only |

Fix: remove the false `#[ignore]` claims from the comments. Do not add the attribute (no-ignore-tests
policy).

### Test Coverage Gaps

1. No test exercises `apply_default_hvac_speed_fallback` with a `CoolingSystem` that has only EER
   (no SEER tag) — the path where `seer = 0.0` is silently substituted (F1).
2. No test covers `BackupHeatingSwitchoverTemperature` alone (no `CompressorLockoutTemperature`, no
   `BackupHeatingLockoutTemperature`) — the path where both lockout params receive the same value (F2).
3. No test verifies the quality of the error message emitted when combi-boiler HPXML is parsed (G1).
4. `reconcile_setpoint_pair` is exercised by two new tests in this diff but no test covers the case
   where only one of heating/cooling setpoints is present (single-mode system); reconciliation is
   silently skipped without a log entry.

### Tolerance and Reference Provenance

All parity tests compare against OCHRE or explicit analytic expectations. No tolerance widening
was observed in this diff. No snapshot tests found. All assertions are value-based.

---

## 7. Tickets 001–023 Verdicts

| Ticket | Title | Verdict |
|--------|-------|---------|
| 001 | Unify h_fg, add humidity port | Not addressed |
| 002 | IdealHVAC biquadratic fallback | **Not addressed — F4 open**. `ideal_hvac.rs:526–527` uses `rated_capacity_w` with no biquadratic correction |
| 003 | Tighten biquadratic default bounds | **Not addressed** — `(-100, 100)` bounds still in place |
| 004 | Semi-implicit infiltration latent | Not addressed |
| 005 | Fan heat diagnostic category | Not addressed |
| 006 | Extract thermostat FSM | Not addressed |
| 007 | Unify variable-speed selection | Not addressed — duplicate selection logic in `air_conditioner.rs` and `staging.rs` persists |
| 008 | Decompose HvacEquipment struct | Not addressed — god-struct `HvacEquipment` unchanged |
| 009 | Consolidate config helpers | **Partially worsened** — setpoint fields added five times across config structs instead of via a shared sub-struct |
| 010 | Default biquadratic performance curves | Not addressed |
| 011 | Discrete defrost cycle | Not addressed |
| 012 | Heating-side SHR | Not addressed |
| 013 | MSHP minimum compressor speed | Not addressed — `MinimumCapacity` not wired; minimum is 25% hardcoded |
| 014 | Defrost typed config | Not addressed — `DefrostType`/`DefrostControl` not wired from HPXML |
| 015 | ER on/off modeling | **Not addressed — F3 open**. `backup_capacity_w × plr` at `heater.rs:1039` |
| 016 | Thermostat decision tracing | Not addressed — no `tracing::debug!` for mode transitions |
| 017 | Missing output columns v7 | Not addressed |
| 018 | CoreOutput HVAC promotion | Not addressed |
| 019 | Speed/startup telemetry gaps | Not addressed |
| 020 | Setpoint chain visibility | Not addressed |
| 021 | Multi-zone thermal attribution | Not addressed |
| 022 | RoomAC ideal target | Not addressed |
| 023 G1 | CrankcaseHeaterWatts wiring | Not addressed |
| 023 G2 | DefrostType/DefrostControl from HPXML | Not addressed |
| 023 G3 | MinimumCapacity from HPXML | Not addressed |
| 023 G5 | ChargeDefectRatio consumed | Not addressed — field is inserted into params map but never consumed |
| 023 G6 | SupplementalLockout temp | Not addressed |
| 023 G8 | Setpoint wiring | **Addressed in this diff** — all heating/cooling equipment builders now call `schedule_source_from_params` |
| — | Combi boiler + indirect tank | **Not tracked** — no ticket; resolver emits parse error for this HPXML type |

---

## 8. Severity-Ranked Findings

| # | Severity | File:Line | Description | Primary Source |
|---|----------|-----------|-------------|----------------|
| F1 | HIGH | `resolve_hvac.rs:1836` | `apply_default_hvac_speed_fallback` calls `.unwrap_or(0.0)` for SEER. A `CoolingSystem` with only EER specified (no SEER tag) silently gets `seer = 0.0` → single-speed inference. This is exactly the no-silent-defaults violation that this diff is eliminating everywhere else. Fix: return early without inserting `number_of_speeds` when SEER is absent, and let the caller decide whether that is an error. | No-silent-defaults rule enforced by this diff |
| F2 | HIGH | `resolve_hvac.rs:1497–1521` | `BackupHeatingSwitchoverTemperature` is mapped to both `hp_lockout_temp_c` (compressor cutout) and `er_lockout_temp_c` (backup activation). HPXML 4.x defines `BackupHeatingSwitchoverTemperature` as the temperature at which backup takes over — it is semantically equivalent to `er_lockout_temp_c` only. Assigning it to `hp_lockout_temp_c` causes the compressor to lock out at an artificially high temperature (e.g. 40°F) on equipment that only provides this field. Fix: map `BackupHeatingSwitchoverTemperature` to `er_lockout_temp_c` only. | HPXML 4.x specification §8.4 |
| F3 | MEDIUM | `heater.rs:1039` | `er_capacity_w = self.backup_capacity_w * plr` modulates ER by PLR in the non-ideal path. The comment at line 1078 explicitly states "ER is not modulatable". ER strip heating is a binary on/off load; modulation by PLR introduces a systematic under-prediction of ER energy. Fix: `er_capacity_w = if er_on { self.backup_capacity_w } else { 0.0 }`. | Ticket-015; ASHRAE HOF resistive heating |
| F4 | MEDIUM | `ideal_hvac.rs:526–527` | Non-ideal fallback delivers `self.rated_capacity_w * self.load_fraction` with no biquadratic correction. Capacity at non-rated outdoor/indoor conditions is therefore constant regardless of temperature, producing incorrect results during temperature extremes. Ticket-002 open. | EnergyPlus Engineering Reference §16.2 |
| N3 | MEDIUM | `resolve_hvac.rs:2286–2344` | `reconcile_setpoint_pair` silently mutates HPXML setpoint arrays when the heating/cooling gap is below 2°C, emitting only a `tracing::warn!`. A caller that has validated setpoints before calling `resolve_hvac` cannot detect this mutation. A 1°C gap is valid for high-responsiveness thermostats (ASHRAE 90.1 defines no minimum gap between heating and cooling setpoints). Consider returning an `HpxmlError::InvalidField` instead, or at minimum make the reconciled values observable to callers. | No-silent-defaults rule; ASHRAE 90.1 |
| F5 | MEDIUM | `resolve_hvac.rs:366–371` | `resistance_efficiency_from_params` silently returns `1.0` when efficiency absent. For ElectricFurnace/Baseboard/Boiler, EIR=1.0 is physically correct, but this path emits no diagnostic. Per the no-silent-defaults rule either add a `tracing::debug!` or document with an explicit comment why silence is safe. | No-silent-defaults rule |
| F6 | MEDIUM | `resolve_hvac.rs:639,643,680,684` | Gas/electric boiler `flow_rate_kg_s` and `return_temp_c` silently default to `0.5 kg/s` and `40°C`. A real HPXML boiler with different loop parameters silently gets wrong values. A comment acknowledges these are not HPXML-sourced, but no warning is emitted and no documented basis is provided for choosing these specific values. | No-silent-defaults rule |
| F7 | LOW | `resolve_hvac.rs:1203` | `building.foundation_name.as_deref().unwrap_or("")` silently maps absent foundation name to empty string, then zone type falls through to `"attic_vented"` default (line 1232). Should warn when zone type is Foundation and name is absent. | No-silent-defaults rule |
| F8 | LOW | `hvac_core.rs:427–434` | When biquadratic cap/EIR curve counts differ, missing entries are silently filled with `DEFAULT_BIQUADRATIC_COEFFS` (unity polynomial). Count mismatch should emit `tracing::warn!`. | Defensive programming |
| F9 | LOW | `resolve_hvac.rs:1783–1785` | `compressor_type_to_mode` wildcard `_ => "single_speed"` silently maps unrecognised HPXML `CompressorType` strings to single-speed. Add `tracing::warn!` for the wildcard case. | HPXML schema extensibility |
| F10 | LOW | `water_heater_ua.rs:155` | Gas WH recovery efficiency `unwrap_or(0.78)` is correct in typical range but uncited. Should add a citation to DOE 10 CFR 430 or the OCHRE/ResStock value, or require the field when `HeatingCapacity` is present. | No-silent-defaults rule |
| F11 | LOW | `resolve_hvac.rs:960,973,1073,1086` | `heating_capacity_w.or(cooling_capacity_w).unwrap_or(0.0)` — if both capacities are absent, `ref_cap = 0.0` produces a degenerate duct config silently. Should warn or error. | Defensive programming |
| G1 | GAP | `resolve_water_heater.rs:503–508` | Combi boiler + indirect tank (`WaterHeatingType=space-heating boiler with storage tank`) is not implemented. No ticket exists. Resolver emits a parse error. | HPXML 4.x §8.5 |
| G2 | GAP | `constants.rs:34` | `DEFAULT_HP_LOCKOUT_TEMP_C = -17.78°C` (0°F) lacks a citation. Add a reference to manufacturer default range or AHRI 210/240 Appendix C. | |
| G3 | GAP | `water_heater/mod.rs` | `WATER_DENSITY_KG_PER_M3 = 1000.0` overestimates thermal mass by ~1.2% at 50°C (actual: ~988 kg/m³ per NIST WebBook). Consider temperature-dependent model or at least a citation noting the approximation range. | NIST WebBook |
| G4 | GAP | Test comments | Three tests carry misleading `#[ignore]` comments but actually run. Fix the comments. | Policy |
| G5 | GAP | Test coverage | No test for EER-only `CoolingSystem` (no SEER tag) → speed inference path exercises the F1 silent-default. | |
| G6 | GAP | Test coverage | No test for `BackupHeatingSwitchoverTemperature` alone (no other lockout fields) — would catch the F2 double-mapping. | |

---

## 9. Fixes Verified Correct

| Fix | File | Assessment |
|-----|------|------------|
| AFUE required for Gas Furnace / Gas Boiler | `resolve_hvac.rs:521–529`, `613–621` | Correct. `MissingField` error replaces `unwrap_or(0.80)`. |
| Duct DSE errors when lat/lon or volume absent | `resolve_hvac.rs:187–242` | Correct. Two new `MissingField` errors guard the three required inputs. |
| HPWH setpoint required (non-low-power path) | `resolve_water_heater.rs:200–209` | Correct. Low-power preset keeps hardcoded 60°C/51.67°C with explanatory comment. |
| `tank_height_m` propagated as `Option` | `resolve_water_heater.rs:51–57`, `98–99`, `132–133`, `217` | Correct. Default decision now in equipment model where it is documented. |
| `reconcile_setpoint_pair` added | `resolve_hvac.rs:2286–2344` | Correct in intent. Gap: no `HpxmlError` returned; see N3. |
| `radiant_gain_w: 0.0` additions | 7 sites across equipment files | Correct. Prevents accidental use of uninitialised radiant contributions. |
| Heating setpoint fields added to all heating config structs | `heating_config.rs:26–249` | Correct in wiring. DRY violation widened — ticket-009 partially worsened (five `heating_setpoint_c` + `heating_setpoint_source` pairs instead of a shared sub-struct). |
| Setpoint source wired for all heating equipment builders | `resolve_hvac.rs:547–895` | Correct. All builders call `schedule_source_from_params`. |

---

## 10. Prior-Review Claims Re-Verified

All findings from the prior review state (`ad52415`) are confirmed present in HEAD (`d5de344`):

- F1 (`seer = 0.0` unwrap_or): confirmed at `resolve_hvac.rs:1836`.
- F2 (`BackupHeatingSwitchoverTemperature` double-map): confirmed at `resolve_hvac.rs:1497–1521`.
- F3 (ER PLR modulation): confirmed at `heater.rs:1039`.
- F4 (IdealHvac non-ideal fallback): confirmed at `ideal_hvac.rs:526–527`.
- F5 (resistance EIR silent 1.0): confirmed at `resolve_hvac.rs:366–371`.
- F6 (boiler flow/return temp unwrap_or): confirmed at `resolve_hvac.rs:639,643,680,684`.
- F7 (foundation name silent empty): confirmed at `resolve_hvac.rs:1203`.
- F8 (mismatched curve count silent fill): confirmed at `hvac_core.rs:427–434`.
- F9 (`compressor_type_to_mode` wildcard): confirmed at `resolve_hvac.rs:1783–1785`.
- F10 (gas WH RE default uncited): confirmed at `water_heater_ua.rs:155`.
- F11 (both capacities absent → ref_cap 0.0): confirmed at `resolve_hvac.rs:960–1086`.
- Combi boiler not implemented: confirmed — no resolver path.
- Three tests with misleading `#[ignore]` comments: confirmed; no actual attribute present.

New finding introduced by this diff:
- N3 (`reconcile_setpoint_pair` silent mutation): introduced at `resolve_hvac.rs:2286–2344`.

---

## 11. Findings-Doc Errors in Prior Version

The prior review document (as of `ad52415`) contained the following errors corrected here:

1. **Combi boiler omitted from HPXML wiring section**: the prior review listed "(f) PV + Battery"
   as the sixth equipment type, skipping combi boiler + indirect tank. Corrected here.

2. **Test ignore count incorrect**: prior review stated `1189 passing, 0 failed, 0 ignored` for
   hares-equipment. Correct totals at HEAD are 1310 passing (additional test suites run), 1 ignored
   (unrelated doctest in `macros.rs`).

3. **UEF→COP formula origin not cited**: prior review did not flag `1.174_536_058 × uef` as a
   linear approximation or identify its OCHRE origin. Now documented.

4. **Water density approximation not flagged**: `WATER_DENSITY_KG_PER_M3 = 1000.0` was not
   flagged. Now tracked as G3.

5. **`DEFAULT_HP_LOCKOUT_TEMP_C` citation missing**: stated as "reasonable" without flagging the
   absent citation. Now tracked as G2.

---

## Verification

**Test suite**: `uv run cargo test -p hares-equipment -p hares-io`
→ hares-equipment 1310 passing / 0 failing / 1 ignored; hares-io 718 passing / 0 failing / 0 ignored.

**Clippy**: `uv run cargo clippy -p hares-equipment -p hares-io`
→ 0 warnings, 0 errors.

**Unverified gaps** (no executable test exercises these paths):
- No integration test for `apply_default_hvac_speed_fallback` with EER-only input (F1).
- No integration test for `BackupHeatingSwitchoverTemperature` with no other lockout fields (F2).
- No test for combi-boiler error message quality (G1).
