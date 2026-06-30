OCHRE HVAC Implementation: Detailed Architecture & Physics Analysis

**OCHRE Reference:**
- Repository: https://github.com/NREL/OCHRE
- Vendored commit (in `vendors/OCHRE/`): `ffc8b56e99c61eb4e42af625bbbc310e198ec58d`
- Publication: Blonsky, M., Maguire, J., McKenna, K., Cutler, D., Balamurugan, S. P., & Jin, X. (2021). OCHRE: The Object-oriented, Controllable, High-resolution Residential Energy Model for Dynamic Integration Studies. *Applied Energy*, *290*, 116732. https://doi.org/10.1016/j.apenergy.2021.116732

1. Architecture: Class Hierarchy and Structure
Class Hierarchy (1497 lines, HVAC.py)
Simulator (ochre/Simulator.py)
  └── Equipment (Equipment.py:10)
        └── HVAC (HVAC.py:63) — base HVAC, holds capacity/EIR/SHR/fan/duct logic
              ├── Heater (HVAC.py:644) — end_use="HVAC Heating"
              │     ├── ElectricFurnace (664)
              │     ├── ElectricBoiler (668)
              │     ├── ElectricBaseboard (672)
              │     ├── GasFurnace (682)
              │     └── GasBoiler (687)
              ├── Cooler (HVAC.py:654) — end_use="HVAC Cooling"
              │
              └── DynamicHVAC (735) — biquadratic EIR/capacity model
                    ├── AirConditioner (1057, inherits DynamicHVAC + Cooler)
                    │     └── RoomAC (1080)
                    ├── ASHPCooler (1089, inherits DynamicHVAC + Cooler via AirConditioner)
                    ├── HeatPumpHeater (1112, inherits DynamicHVAC + Heater)
                    │     └── ASHPHeater (1176, HeatPumpHeater) — adds ER backup
                    └── MinisplitHVAC (1094, DynamicHVAC) — remaps 10→4 speeds
                          ├── MinisplitAHSPCooler (1106, MinisplitHVAC + AirConditioner)
                          └── MinisplitAHSPHeater (1480, MinisplitHVAC + ASHPHeater)
Key Architectural Observations
Pattern: Multiple inheritance for composition — AirConditioner inherits from both DynamicHVAC and Cooler (line 1057). ASHPHeater inherits from HeatPumpHeater (itself DynamicHVAC + Heater). This is used to combine:
- A category class (Heater/Cooler) that sets end_use and optional_inputs
- A physics class (DynamicHVAC) that adds biquadratic models
Key risk: MRO (Method Resolution Order) complexity. AirConditioner(DynamicHVAC, Cooler) means DynamicHVAC.run_thermostat_control takes precedence over HVAC.run_thermostat_control (which is correct here, but fragile — a new developer could easily break this).
Pattern: Dual capacity modes — Every HVAC object has a use_ideal_capacity flag (line 241). When True, capacity is back-solved from the envelope RC model to maintain setpoint exactly. When False, it uses on/off thermostat control with fixed per-speed capacity. The auto-selection logic (line 240) uses ideal capacity for time_res >= 5 min OR n_speeds >= 4. This is a critical architectural choice — it means OCHRE has two fundamentally different simulation paths through the same codebase.
Data file coupling — Equipment-specific biquadratic coefficients come from CSV files (e.g., Biquadratic ASHP Heater.csv), loaded at DynamicHVAC.__init__ (line 804-846). Multispeed ratio/COP data comes from HVAC Multispeed Parameters.csv (lines 774-796). The code validates against these files at init time, which is good, but the data file lookup is by string-matching equipment name AND efficiency string (e.g., "9.2 HSPF"), which is fragile.
---
2. Control Logic
2.1 Thermostat Control (HVAC.run_thermostat_control, line 392)
temp_turn_on  = setpoint - hvac_mult * deadband * (1 - deadband_offset)   # line 397
temp_turn_off = setpoint + hvac_mult * deadband * deadband_offset          # line 398
- hvac_mult is +1 for heating, -1 for cooling. This unifies the deadband calculation so the same code handles both directions.
- deadband_offset (default 0.2, line 222) controls asymmetric overshoot: for heating, the setpoint is near the top of the deadband, so the system tends to overshoot slightly above setpoint.
- Returns "On", "Off", or None (within deadband, maintain current mode). The None path is handled by Equipment.update_model (Equipment.py:236), which keeps the current mode when mode is None.
HARES should adopt: The hvac_mult sign-unification pattern is elegant — it halves the branching. The deadband_offset is a useful real-world parameter (matches lab test results per line 221 comment).
HARES should avoid: The thermostat is purely proportional bang-bang. There is no PID, no anticipatory control, no rate-of-change logic. The commented-out setpoint_ramp_rate (line 224, also line 1195-1196) indicates this was considered but abandoned. For accuracy vs. EnergyPlus at sub-minute resolution, this matters.
2.2 Two-Speed Control (DynamicHVAC.run_two_speed_control, line 859)
Three strategies:
1. Time (line 868): Switches to high speed if temperature continues to diverge from setpoint (comparing to previous step temperature).
2. Setpoint (line 884): Uses an overlapping setpoint — high speed uses a lower setpoint (for heating), creating a second deadband within the first.
3. Time2 (line 893): Simplistic — off→low, on→high.
Minimum time-in-speed is enforced (line 903). Speed disabling from external control is supported (line 907).
Limitation: No hysteresis beyond minimum-time enforcement. Cycling rates can be unrealistically high.
2.3 Ideal Capacity Control (HVAC.update_internal_control, line 356)
When use_ideal_capacity=True, the code:
1. Updates setpoint
2. Calls update_capacity() which back-solves from the envelope model
3. Returns "On" if capacity > 0
The actual back-solve is in solve_ideal_capacity() (line 411), which calls self.envelope_model.solve_for_inputs() — a linear solve against the RC state-space model. This is the core coupling mechanism.
Critical detail (line 425-429): The back-solve accounts for fan power heat and SHR in the denominator:
if self.is_heater:
    return h_desired / (self.shr + self.eir * self.fan_power_ratio)
else:
    return -h_desired / (self.shr - self.eir * self.fan_power_ratio)
This is algebraically correct for maintaining setpoint, but it uses the previous time step's SHR and EIR (noted at line 425: "assumes SHR and EIR from previous time step"). This creates a one-step lag that can cause oscillation.
2.4 ASHP Backup Element Control (ASHPHeater, line 1176)
Complex lockout logic for the electric resistance (ER) backup:
- HP Lockout Temp (line 1208, default -17.78°C/0°F): Below this, HP is forced off.
- ER Lockout Temp (line 1209, default 4.44°C/40°F): Above this, ER is forced off.
- Hard Lockout Time (line 1214): After a setpoint increase, ER stays off for this duration.
- Soft Lockout Time (line 1216): After hard lockout expires, ER stays off if temperature is still rising.
- ER Setpoint Offset (line 1211): ER uses a lower effective setpoint (setpoint - offset) than the HP, so it only kicks in when the house is significantly underserved.
This is a production-quality control model that matches real thermostat behavior (e.g., ecobee staging). The commented-out staged backup code (line 1372-1394) shows planned but unfinished work.
2.5 Duty Cycle Control (HVAC.run_duty_cycle_control, line 325)
For external control (grid signals), OCHRE accepts duty cycles and uses a priority-stack approach:
- Maintains ext_mode_counters per mode
- Prioritizes current mode first
- Only switches to modes that haven't "used up" their duty fraction
- Combines with thermostat: takes thermostat mode if it's in the priority stack
---
3. Physics Models
3.1 Biquadratic Capacity/EIR Model (DynamicHVAC, lines 735-969)
The core physics model uses the biquadratic form from Cutler, D., Winkler, J., Kruis, N., Christensen, C., & Brandemuehl, M. (2013). *Improved Modeling of Residential Air Conditioners and Heat Pumps for Energy Calculations*. NREL/TP-5500-56354. (referenced at OCHRE HVAC.py line 744-746):
param_ratio = (a + b*T_in + c*T_in² + d*T_ext + e*T_ext² + f*T_in*T_ext)
            * (a + b*ff + c*ff²)
            / (a + b*plr + c*plr²)
Where:
- T_in = indoor wet bulb (cooling) or dry bulb (heating) — see line 949
- T_ext = ambient dry bulb
- ff = flow fraction (always 1 for constant-speed equipment)
- plr = part load ratio (used for cycling degradation / PLF)
The numerator is a 6-coefficient biquadratic in temperatures, the denominator is the Part Load Factor (PLF) correction for cycling losses. This is JIT-compiled via @nb.njit (line 24) for performance.
HARES should adopt: The biquadratic + PLF structure is the EnergyPlus standard (ref: Cutler, D., Winkler, J., Kruis, N., Christensen, C., & Brandemuehl, M. (2013). *Improved Modeling of Residential Air Conditioners and Heat Pumps for Energy Calculations*. NREL/TP-5500-56354). It's well-validated and the CSV-based parameterization makes it extensible.
HARES should avoid: The min/max clamping at lines 43-45 silently clips inputs rather than warning. If conditions are outside the curve's valid range (e.g., extreme cold), the model returns the boundary value with no notification.
3.2 SHR / Sensible-Latent Split (HVAC.update_shr, lines 458-512)
The SHR calculation is the most physically detailed part of OCHRE's HVAC:
1. Coil Ao factor (line 199-215): At initialization, the coil bypass factor parameter Ao = UA/Cp is computed from rated conditions using utils_equipment.coil_ao_factor() (equipment.py:763-792). This inverts the SHR equation at rated conditions.
2. Runtime SHR (line 486-510): At each timestep, SHR is computed via utils_equipment.calculate_shr() (equipment.py:756-760), which calls the JIT-compiled _calculate_shr_jit() (psychrolib_jit.py:315-349). This iteratively solves for the Apparatus Dew Point (ADP) temperature, then computes:
      SHR = (h_Tin_Wadp - h_ADP) / (h_in - h_ADP)
      This is the EnergyPlus coil bypass factor method — see equipment.py:683-753 for the legacy Python version with detailed comments.
3. Speed interpolation (lines 496-510): For fractional speed indices, SHR is linearly interpolated between adjacent integer speeds.
4. Fan power heating effect (lines 475-478): Fan power raises the coil entering dry bulb temperature:
      self.coil_input_db += self.fan_power_per_flow_rate / 1000 / rho_air / cp_air
   
HARES should adopt: The full ADP iteration method for SHR. It's physically correct and matches EnergyPlus. The JIT compilation makes it performant.
HARES should avoid: The fallback to Ao_list = [10] for ideal coolers (line 215) — this is a magic number with no documented justification. Also, the fallback to ambient humidity when zone.humidity is None (line 469) can produce wrong SHR in winter (indoor humidity ≠ outdoor humidity).
3.3 Startup Capacity Degradation (DynamicHVAC, lines 971-988)
Implements the Winkler thesis model (referenced at equipment.py:473-474):
t_full = 20.0 * c_d + 0.4  # time to full capacity, minutes
capacity_mult = max(0, min(1.0, -1.025 * exp(-3.79936 * (t_from_start / t_full)) + 1.025))
Where c_d is the degradation coefficient, computed by calc_c_d() (equipment.py:470-500) based on SEER/HSPF. Single-speed low-efficiency equipment has the highest degradation (c_d=0.2 for SEER<<13 or HSPF<<7), while variable-speed equipment has none (c_d=0).
The EIR is corrected inversely: eir *= 1 / startup_cap_mult (line 1035), meaning degraded capacity also has degraded efficiency.
HARES should adopt: This is a real physical effect that causes significant energy waste at short cycling times. The exponential curve matches lab data.
3.4 Defrost Model (HeatPumpHeater, lines 1112-1166)
Implements EnergyPlus on-demand reverse-cycle defrost (reference at line 1139):
1. Triggers when t_ext_db < 4.4445°C (line 1141)
2. Calculates outdoor coil temperature: T_coil_out = 0.82 * t_ext - 8.589 (line 1144)
3. Computes saturation humidity ratio at coil surface
4. Defrost time fraction: 1 / (1 + (0.01446 / delta_omega)) (line 1148)
5. Capacity multiplier: 0.875 * (1 - defrost_time_frac) (line 1149)
6. Power multiplier: 0.954 / 0.875 — relative increase (line 1150)
7. Additional defrost power: 0.01 * defrost_time_frac * (7.222 - t_ext) * (capacity_max / 1.01667) (line 1151)
Limitation: This is a steady-state defrost model, not a transient one. It doesn't model the actual defrost cycle (which involves reversing the refrigerant flow and melting ice over minutes). It's an average-effect correction applied continuously when conditions warrant.
3.5 Gas Boiler Part-Load Efficiency (GasBoiler, lines 687-732)
Unique among heaters: the GasBoiler has a biquadratic efficiency correction based on part-load ratio and indoor/outlet temperature. Two curves are provided — one for condensing boilers (AFUE>90%, line 697) and one for non-condensing (line 702). The EIR is adjusted as eir_max / eff_curve_output.
HARES should adopt: Condensing boiler efficiency drops significantly at high firing rates; this is a real effect that matters for annual energy.
3.6 Duct Loss / Distribution System Efficiency
ASHRAE Standard 152 calculation (equipment.py:161-467):
The DSE (Distribution System Efficiency) calculation is one of the most complex functions in the codebase. It:
1. Finds the nearest climate station by great-circle distance (line 218-227)
2. Loads zone temperatures from ASHRAE 152 tables (line 245-246)
3. Computes supply/return duct leakage and conduction losses (lines 298-333)
4. Calculates uncorrected delivery effectiveness (lines 335-384)
5. Applies load factors, equipment factors, and cycle losses (lines 386-428)
6. Combines into final DSE (line 456)
Critical assumption: DSE is computed once at initialization and held constant. The self.duct_dse (line 167) never changes during simulation. This means:
- No transient duct effects (e.g., duct thermal mass during cycling)
- No dependence on actual operating conditions
- No distinction between heating and cooling DSE if both use the same ducts (but the code does compute separately per HVAC unit)
HARES should avoid: Static DSE is a significant simplification. In reality, duct losses depend heavily on attic/garage temperature, which varies. A dynamic duct model (even a simple one) would be more accurate. However, the ASHRAE 152 calculation itself is the standard method, so using it as a starting point is reasonable.
3.7 Zone Fraction Distribution (HVAC.init, lines 188-197)
Heat is distributed to multiple zones based on:
self.zone_fractions = {self.zone: self.duct_dse * (1 - basement_heat_frac)}   # Indoor zone
# duct_zone gets (1 - duct_dse) if ducts are elsewhere
# basement_zone gets duct_dse * basement_heat_frac
This means duct losses go to the duct zone (e.g., attic), not to outdoor. This is physically correct — duct losses heat/cool the space they're in, which then affects the envelope through boundaries.
---
4. Envelope Coupling
4.1 The Ideal Capacity Back-Solve (HVAC.solve_ideal_capacity, line 411)
This is the central coupling mechanism. It uses the RC state-space model's inverse:
h_desired = self.envelope_model.solve_for_inputs(
    self.zone.t_idx,     # output index (indoor temperature)
    zone_idxs,           # input indices (heat injection per zone)
    x_desired,           # target = setpoint temperature
    zone_ratios          # distribution ratios
)
The solve_for_inputs method (RCModel.py:305-333) performs a linear algebraic solve:
u_factor = self.B[y_idx, u_idxs].dot(u_ratios)
u_desired = (y_desired - a_i.dot(self.states) - b_i.dot(inputs)) / u_factor
This computes the heat injection needed to maintain the setpoint exactly in the next time step, given the current state and all other inputs. This is algebraically exact for the linear RC model.
4.2 Heat Gain Injection (HVAC.add_gains_to_zone, line 563)
def add_gains_to_zone(self):
    for zone, fraction in self.zone_fractions.items():
        zone.hvac_sens_gain += self.sensible_gain * fraction
        zone.hvac_latent_gain += self.latent_gain * fraction
This accumulates into the zone's hvac_sens_gain and hvac_latent_gain. Then in Envelope.update_model (Envelope.py:1283-1300), these are added to the RC model inputs:
equipment_sens_gains[zone.h_idx] += zone.internal_sens_gain + zone.hvac_sens_gain
control_signal = self.inputs_init + equipment_sens_gains
4.3 Fan Power as Internal Heat (HVAC.calculate_power_and_heat, line 543)
self.delivered_heat = heat_gain * self.shr + self.fan_power  # SHR=1 for fan
Fan power is treated as 100% sensible heat to the zone (correct for in-unit fans). The main compressor power is NOT added to zone heat — only the delivered heat and fan power go to the zone.
4.4 Assumptions and Limitations in Coupling
1. No supply air temperature model: OCHRE never explicitly computes supply air temperature or air-side heat transfer. The coil model computes SHR, but there's no zone air mixing model — heat is injected directly into the zone node.
2. No duct transient model: Duct DSE is static. The actual supply air temperature after duct losses is never computed.
3. Humidity coupling is one-way: The HVAC computes SHR and latent gains, which are injected into the humidity model. But the HVAC doesn't receive feedback from the humidity model about whether the desired latent removal was achieved (e.g., coil might not be cold enough to achieve the computed SHR).
4. Equipment heat gains from other equipment in the same timestep are NOT accounted for (explicitly noted at lines 67-69 of the docstring): "It does not account for heat gains from other equipment in the same time step." This is a known source of error — the ideal capacity algorithm maintains setpoint against envelope loads but not against concurrent internal gains.
---
5. Known Limitations (Relative to EnergyPlus)
Limitation	Location
No refrigerant cycle model	Throughout
No coil finite-volume model	SHR calc
No transient defrost	HP Heater, line 1139
Static DSE	Line 167, equipment.py
No duct thermal mass	equipment.py
No supply air temperature	Throughout
One-step lag in ideal capacity	Line 425
No staged ER backup	Lines 1372-1394
No crankcase heat to zone	AC line 1075-1077
No pan heater heat to zone	MSHP line 1495-1497
No dehumidification mode	Throughout
Minisplit 10→4 speed reduction	Line 1096-1101
No variable-speed compressor map	Variable uses ideal
No ground-source HP	Not implemented
---
## 6. Code Quality Assessment
### Strengths
1. **JIT compilation for hot paths**: `_biquadratic` (line 24), `_calculate_shr_jit` (psychrolib_jit.py:315), `_update_humidity` (psychrolib_jit.py:219), `_solve_interior_radiation` (Envelope.py:20) — these are all `@nb.njit(cache=True)`, which is excellent for simulation performance.
2. **Clear separation of concerns**: Capacity, EIR, SHR, and fan power each have their own update method (`update_capacity`, `update_eir`, `update_shr`, `update_fan_power`), making the code modular.
3. **Dual-mode architecture**: The `use_ideal_capacity` flag provides both fast (ideal) and realistic (dynamic) simulation paths in the same codebase.
4. **External control integration**: Rich set of control signals (Setpoint, Deadband, Capacity, Duty Cycle, Max Capacity Fraction, Disable Speed) — this is production-quality for co-simulation.
5. **Initialization-time validation**: Asserts at lines 99, 159-163, 801, 814-818 catch parameter mismatches early.
### Weaknesses
1. **`speed_idx` type inconsistency** (CRITICAL): `speed_idx` is sometimes an `int` (0, 1, 2...) and sometimes a `float` (e.g., `capacity / capacity_max` at line 451, or interpolated values at line 1018). The code handles both cases with isinstance checks (e.g., line 1034: `isinstance(self.speed_idx, int)`), but this is fragile — `isinstance(1.0, int)` returns `False` in Python, so if `speed_idx` is ever `1.0` instead of `1`, the wrong branch executes. The `eir_list[speed_idx]` at line 113 will fail with a float index.
2. **Dead code / commented-out features**: Lines 130-131 (setpoint calculation), 224 (ramp rate), 876-883 (Time-old control), 1194-1196 (ramp rate again), 1226 (existing_stages), 1372-1394 (staged backup) — there's enough of this to confuse new developers.
3. **Unreachable code** (line 1315): `self.hp_on_prev = hp_on` is after a return statement in `ASHPHeater.update_internal_control` and is never executed.
4. **Global mutable state**: The `zone.hvac_sens_gain` and `zone.hvac_latent_gain` accumulate across equipment (e.g., if both ASHP Heater and ASHP Cooler exist). The reset happens in `Envelope.update_inputs` (Envelope.py:1252-1253), but if the reset order is wrong, gains could leak between steps.
5. **No type annotations**: The entire file is untyped Python. Given the `speed_idx` type confusion, this is a real maintainability risk.
6. **Magic numbers**: `T_coil_out = 0.82 * t_ext - 8.589` (line 1144), `0.875` (line 1149), `0.954` (line 1150), `0.01446` (line 1148), `0.01` (line 1151), `7.222` (line 1151), `1.01667` (line 1151), `0.1528` (line 1161) — all from EnergyPlus but none documented with their physical meaning or source equation numbers.
7. **DSE function is 300+ lines** (equipment.py:161-467) with deeply nested conditionals and multiple commented-out alternative calculations. This would benefit from decomposition.
---
7. Key Patterns HARES Should Adopt
Pattern
hvac_mult sign unification
Biquadratic + PLF structure
JIT-compiled hot paths
ADP iteration for SHR
Startup degradation model
ER lockout with hard/soft timers
Zone fraction distribution
Back-solve ideal capacity from RC model
8. Key Mistakes HARES Should Avoid
Mistake
Mixed int/float speed_idx
Static DSE
One-step lag in ideal capacity
Silent clipping in biquadratic
Magic Ao value for ideal coolers
Ambient humidity fallback for SHR
Crankcase/pan heater not adding to zone
No transient duct model
Ignoring concurrent internal gains in ideal capacity
Commented-out dead code
No type annotations on polymorphic variables
---
9. Summary: Architecture Decision Points for HARES
The fundamental tradeoff OCHRE made: Speed vs. accuracy, resolved by the use_ideal_capacity flag. At coarse timesteps (≥5 min), the ideal algorithm gives exact setpoint maintenance at the cost of unrealistic instantaneous capacity modulation. At fine timesteps, the dynamic algorithm gives realistic on/off cycling but the RC model's thermal capacitance assumptions determine temperature accuracy.
For HARES, I recommend:
1. Adopt the biquadratic + PLF framework as the primary physics model
2. Adopt the ADP iteration for SHR with JIT compilation
3. Adopt hvac_mult sign unification and zone-fraction distribution
4. Do NOT adopt static DSE — implement a simple dynamic duct model
5. Do NOT adopt mixed int/float speed_idx — use a clean current_speed: int + part_load_ratio: float separation
6. Do NOT adopt the one-step lag — iterate or use current-step EIR/SHR estimates
7. Add transient defrost (stateful, not average-effect)
8. Add supply air temperature computation (needed for any duct model)
9. Add type annotations from the start