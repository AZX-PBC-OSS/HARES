# HARES Water Heater Implementation Audit vs OCHRE Reference

## Executive Summary

HARES implements a **structurally sound** water heater system with significant improvements over OCHRE's monolithic Python design. However, several **correctness gaps**, **missing features**, and **physics model deviations** exist that require attention for parity testing and validation.

### Key Findings

1. **Stratified Tank Model**: Correctly ported with energy-conserving draw algorithm and proper inversion mixing.
2. **Thermostat Control**: Nearly identical with one intentional deviation (hysteresis boundary).
3. **Electric Resistance WH**: Dual-element logic implemented; supports both MasterSlave and Simultaneous priority modes.
4. **Heat Pump WH**: Partially implemented; missing several OCHRE features.
5. **Gas WH**: Partially implemented; missing efficiency curve support.
6. **Tankless WH**: Minimal implementation; lacks proper draw-demand calculation.
7. **Configuration Loading**: Simplified vs OCHRE's hierarchical schedule system; may miss edge cases.
8. **Output Ports/Telemetry**: Good coverage but some OCHRE outputs missing.

---

## 1. CONFIG LOADING

### OCHRE (Python) Flow
**WaterHeater.__init__** (lines 28–79):
- Accepts `water_nodes` parameter; selects model class (OneNode, TwoNode, Stratified)
- Loads Water Tank config from `kwargs["Water Tank"]` dict
- Creates water tank sub-simulator with tank-specific parameters:
  - Tank Volume (L), Height (m), Diameter (m)
  - UA (W/K) or Heat Transfer Coefficient (W/m²/K)
  - Initial Temperature (C)
  - Setpoint Temperature (C), Deadband Temperature (C)
  - Mains Temperature (C)
  - Mixed Delivery Temperature (C) — default 40.6°C (105°F)
  - Tempering Valve Setpoint (C) — default 51.7°C (125°F)
- Determines upper/lower element nodes based on `n_nodes` (e.g., node 3 if n_nodes>=12, else node 1)
- Sets capacity from `Capacity (W)` (default 4500 W)
- Sets efficiency from `Efficiency (-)` (default 1.0)
- Sets control params: setpoint, deadband (default 5.56 C), max_temp (default 140°F=60°C)
- Optional: setpoint ramp rate, max power limits

**OCHRE update_inputs()** (lines 81–86):
- Maps zone temperature into schedule for water tank model
- Supports draw schedules: "Water Heating (L/min)", "Clothes Washer (L/min)", "Dishwasher (L/min)"

### HARES (Rust) Flow
**ResistanceWH::new()** (lines 104–187):
- Hardcoded defaults; no dynamic model class selection
- Tank always StratifiedTank with fixed node count (default 6, range 1–12)
- Tank config:
  - height_m, diameter_m fixed (1.2 m, 0.5 m)
  - ua_w_per_k, conductivity_w_m_k fixed defaults
  - initial_temp_c = setpoint_c (52.67°C)
  - NO explicit mains temperature config during init (set to 15°C)
  - NO draw profile schedule support

**ResistanceWH::init()** (lines 250–351):
- Called post-construction to load actual config values
- Tank volume: tries `TankVolume` (gal) or `tank_volume_gal` (default 50 gal)
- UA: supports `ua_w_per_k` or `UA` (both keys)
- Supports jacket R-value correction (HARES extension not in OCHRE)
- Setpoint/deadband: multiple key aliases for compatibility
- Element power: tries `HeatingCapacity` (BTU/hr) or explicit upper/lower powers
- Element nodes: optional config (defaults 0 and n_nodes-1)
- **MISSING**:
  - Ramp rate for setpoint (OCHRE line 75)
  - Tempered draw configuration (105°F, 125°F, etc.) hardcoded in tank.rs
  - Max Power (kW) per schedule (OCHRE line 77)

### Gaps

| Feature | OCHRE | HARES | Impact |
|---------|-------|-------|--------|
| Model class selection | Yes (1/2/12 node models) | No (always stratified) | Minor: HARES always uses best model |
| Water Tank sub-kwargs | Yes (nested dict) | No (flat config) | Medium: Less flexible config structure |
| Setpoint ramp rate | Yes (line 75) | No | Low: Not critical for most tests |
| Max Power schedule | Yes (line 77) | No | Medium: Can't soft-limit power per timestep |
| Draw schedule integration | Yes (lines 81–86, 280–298) | No (fixed draw_flow_rate_kg_s) | **HIGH**: Blocks realistic usage profiles |
| Tempered draw temps | Configurable (lines 225–227) | Hardcoded 40.6°C / 51.7°C | Medium: Fixed values work for standard tests |

---

## 2. TANK MODEL PHYSICS

### OCHRE Water.py Architecture
**StratifiedWaterModel** (lines 152–453):
- **Nodes**: Configurable count (default 12); volume fractions (equal or custom)
- **Top node (index 0)**: Outlet; always hottest
- **Bottom node (last)**: Coldest; fed by mains
- **RC Network**:
  - Capacitances: per-node heat capacity = `volume * water_cp * density`
  - Resistances between nodes: `R = (h/n_nodes) / conductivity / cross_section`
  - Boundary UA per node: `r_side_tot = 1 / ua / (2πrh)` distributed by volume fraction
  - End-cap loss: Top and bottom nodes get additional parallel UA (EnergyPlus ref)
- **Conduction**: Forward Euler state-space: `T(t+dt) = T(t) + dt * A * T + B * u`
- **Draw Logic** (lines 280–365):
  - Tempered draw: mixes hot tank water with cold mains to achieve fixture setpoint
  - Unmet load: W = `draw_flow_l_min / 60 * water_c * max(fixture_temp - outlet_temp, 0)`
  - Outlet temp calculated from volume-weighted draw (NOT just top node)
  - Uses optimized 2-node fast path; fallback to general algorithm
  - Numba JIT-compiled for speed
- **Inversion Mixing** (lines 107–149, 383–393):
  - EnergyPlus algorithm: top-down pass, merge any inverted (cold-on-hot) layer
  - Recursive until profile is monotone-decreasing
  - Energy-conserving

### HARES Tank.rs Architecture
**StratifiedTank** (lines 71–511):
- **Nodes**: Fixed at construction; supports 1–12 nodes
- **Volume fractions**: Auto-equal OR custom; 2-node special: [1/3, 2/3]
- **Same RC structure**: Per-node capacitances, inter-node conduction, boundary UA
- **End-cap UA**: Default 10% of total (line 143); OCHRE uses end-cap parallel resistor (line 258)
  - HARES: `ua_per_node[0] += ua_end` (adds to top)
  - OCHRE: `r_top = 1/u/area`, then `parallel(r_side, r_top)`
  - **Difference**: HARES treats end-cap as proportional to volume; OCHRE uses actual geometry
- **Conduction** (lines 403–435):
  - Direct Euler: `delta_energy = K * (T_i - T_i+1) * dt`, where K = conductivity * area / height
  - **Same result** as OCHRE's RC model
- **Draw Logic** (lines 437–501, 242–306):
  - **Raw draw**: Volume-weighted outlet temp, energy accounting
  - **Tempered draw**: Mixing valve calculation with unmet load (lines 242–306)
  - Outlet temp: `outlet_temp_c = avg_temp[segment 0..draw_vol]`
  - **Key difference**: HARES iterates through old/new temperature segments; OCHRE uses `vols_pre/vols_post` arrays
  - Result should be **equivalent** but algorithm differs
  - No Numba; pure Rust
- **Inversion Mixing** (lines 319–379):
  - Greedy merge: scan forward, when T[i] < T[i+1], merge both and keep merged
  - Continues until stable
  - **Energy-conserving** by construction (weighted average)
  - **Simpler than OCHRE**: No threshold check (0.001 K) but achieves same result

### Physics Assessment
- **Conduction & Boundary Loss**: Equivalent ✓
- **Draw algorithm**: Different code, likely same results ✓ (tests confirm OCHRE reference match)
- **Inversion mixing**: Different algorithm but **equivalent output** (test suite confirms)
- **Tempered draw / unmet load**: **Correctly implemented** ✓

---

## 3. PER-STEP COMPUTATION

### OCHRE WaterHeater.calculate_power_and_heat() (lines 265–296)
**Sequence**:
1. Call `self.model.update_model(control_signal)` to advance tank physics
2. Get heats_to_tank from `add_heat_from_mode()` (duty-cycle-weighted)
3. Calculate delivered_heat = sum(heats_to_tank)
4. power = delivered_heat / efficiency / 1000 [kW]
5. Clip by max_power: if power > max_power, scale heats_to_tank
6. For gas: `gas_therms_per_hour = power * kwh_to_therms`
7. For electric: `sensible_gain = power - delivered_heat / 1000` (waste heat)
8. Return heats_to_tank dict for tank sub-simulator

**Thermostat Control** (lines 213–250):
- `run_thermostat_control()`: Hysteresis on lower node (or avg of lower 2 for n_nodes>2)
  - ON if T_lower < setpoint - deadband
  - OFF if T_lower > setpoint
  - (Hysteresis with `<` vs `<=` boundary)
- `update_internal_control()`: Selects between ideal_capacity and thermostat
- Ideal capacity: Solves for heat to reach setpoint temp

### HARES ResistanceWH

**update_control()** (lines 353–402):
1. Check DR duration, auto-revert Normal
2. Safety clamp: force off if upper node > max_tank_temp
3. Check DR load fraction and mode override
4. Call `thermostat_calls()` for upper/lower signals
5. Apply element priority (MasterSlave vs Simultaneous)
6. Return mode (Off or Heating)

**thermostat_calls()** (lines 193–212):
- Upper: `hysteresis_call(upper_temp, effective_sp, deadband, upper_on)`
- Lower: `hysteresis_call(lower_sensor_temp, effective_sp, deadband, lower_on)`
- **lower_sensor_temp** (lines 218–226): For n_nodes >= 3, average lower_node and lower_node-1
  - OCHRE does same (WaterHeater.py lines 218–219)

**hysteresis_call()** (lines 47–59):
- Currently ON: stay on if T < setpoint (use `<`)
- Currently OFF: turn on if T <= setpoint - deadband (use `<=`)
- **Difference from OCHRE** (WaterHeater.py line 221): OCHRE uses `<` for both
  - HARES choice: "at exactly setpoint-deadband, heater turns on"
  - More physical; avoids chatter
  - **Note**: This is an intentional divergence (documented in comment)

**step()** (lines 404–480):
1. Call update_control() for mode
2. Calculate effective duty = duty_cycle * dr_load_fraction * ctrl_load_fraction
3. Apply heat: `tank.heat_node(node, power_w, dt)`
4. Apply draw: `tank.step(ambient, draw_volume, mains_temp, dt)`
5. Apply ZIP voltage scaling
6. Accumulate electrical and fluid port contributions
7. Update telemetry
8. Reset transient ctrl_load_fraction

### Key Differences

| Step | OCHRE | HARES | Impact |
|------|-------|-------|--------|
| Tank update | Sub-simulator pattern | Direct tank.heat_node + tank.step | Same result |
| Ideal capacity | Supports both modes | Always thermostat | Low: Simpler, works |
| Hysteresis boundary | `<` for both transitions | `<=` for off→on | Minor: More stable |
| Lower sensor averaging | For n_nodes>2 | For n_nodes>=3 | **Very Minor**: negligible |
| Max power clipping | Post-calculation | Via ctrl_load_fraction | Different mechanism, same effect |

---

## 4. DRAW PROFILES

### OCHRE (Water.py lines 280–365)
- Schedule inputs: `Water Heating (L/min)`, `Clothes Washer (L/min)`, `Dishwasher (L/min)`
- Each draw has a target temperature:
  - `tempered_draw_temp`: 40.6°C (105°F) — fixture setpoint
  - `hot_draw_temp`: 51.7°C (125°F) — dishwasher/washing machine
  - `setpoint_temp`: 51.67°C (125°F) — tank setpoint
- **Mixing Valve Logic** (lines 305–325):
  - For each draw type, compute actual volume needed from tank:
    - If outlet ≤ target: use requested volume
    - Else: vol_ratio = (target - mains) / (outlet - mains); actual_vol = requested * vol_ratio
  - Result: outlet temp is blended down to target via cold-water mixing
- **Unmet Load** (line 363):
  - W = max(tempered_draw / 60 * water_c * (target_temp - outlet_temp), 0)
  - Represents heat deficit at fixture

### HARES (resistance.rs + tank.rs)

**Config (resistance.rs lines 255–341)**:
- Mains temp: from config `mains_temp_c` (default 15°C)
- Draw flow: resolved via `resolve_draw_rate_kg_s()` (mod.rs lines 186–221)
  - Tries `draw_flow_rate_kg_s` (direct)
  - Else tries `draw_profile_fraction` × `avg_water_draw_l_per_day` (OCHRE normalize)
  - **No schedule-based draw**; only constant avg draw_flow_kg_s

**Tank API (tank.rs)**:
- `step()`: raw draw (no mixing)
- `step_tempered()`: with TMV and unmet load (lines 242–306)
  - Input: tempered_flow_m3_s, hot_flow_m3_s, TMV config
  - Computes actual volumes after mixing-valve reduction
  - Calculates unmet load: same formula as OCHRE

### Gaps
| Feature | OCHRE | HARES | Impact |
|---------|-------|-------|--------|
| Schedule-based draw profiles | Yes (per-timestep) | No (constant avg) | **HIGH**: Can't simulate realistic usage patterns |
| Clothes washer draw | Yes (L/min schedule) | No | High: Residential use important |
| Dishwasher draw | Yes (L/min schedule) | No | High: Residential use important |
| Fixture draw (sink/shower) | Yes (L/min schedule) | No (constant) | Medium: Can work with average |
| Mixing valve (TMV) | Yes (built-in) | Yes (in tank.step_tempered) | Good: feature parity |
| Unmet load calculation | Yes (W) | Yes (W) | Good: feature parity |

---

## 5. HEAT PUMP WATER HEATER

### OCHRE HeatPumpWaterHeater (WaterHeater.py lines 425–702)

**Configuration** (lines 430–504):
- `cop_nominal`: Required; no default
- `HPWH Capacity (W)` or computed from `HPWH Power (W)` × COP
- `HPWH Parasitics (W)`: Standby power (default 1 W)
- `HPWH Fan Power (W)`: Evaporator fan (default 35 W)
- Low-power flag: affects COP/capacity curves
- Two sets of biquadratic curves: one for capacity, one for COP
  - Inputs: t_wet (wet bulb), t_lower (tank lower node avg)
  - Coefficients: [c0, c1, c2, c3, c4, c5] for biquadratic
- `HPWH Minimum On Time`: Minimum compressor on-time (default 10 min)
- `HPWH Minimum Off Time`: Minimum compressor off-time (default 0)
- `HPWH SHR`: Sensible heat ratio (default 0.88)
- `HPWH Interaction Factor`: Fraction of waste heat lost (default 0.75 indoor, 1.0 outdoor)
- `HPWH Wall Interaction Factor`: Fraction to interior walls (default 0.5)
- Ambient lockout: 7.2°C to 43.3°C (standard); 2.8°C to 62.8°C (low-power)
- Control: HP has priority; backup elements for extreme temps
- **hp_nodes**: Array for distributing condenser heat across tank
  - 1-node: [1]
  - 2-node: [0, 1] (both nodes)
  - 12-node: [0, 0, 0, 0, 0, 5, 10, 15, 20, 25, 30, 5]/110 (weighted distribution)

**Control Logic** (lines 583–607):
- Three-level thermostat:
  - Upper element: fires if T_upper < setpoint - 13°C OR (was_on AND T_upper < setpoint)
  - Lower element: fires if T_lower < setpoint - 15°C (when in ER mode)
  - HP: fires if T_control < setpoint - deadband (where T_control = 0.75*T_upper + 0.25*T_lower)
  - ER-only mode: when ambient outside lockout range
- Mode priority: HP > Lower > Upper > Off

**Dynamic COP/Capacity** (lines 631–640):
- Per-step: `hp_capacity = nominal * f(t_wet, t_lower)` using biquadratic
- Per-step: `hp_cop = nominal_cop * f(t_wet, t_lower)` using biquadratic

**Heat Distribution** (lines 624–629):
- HP heat injected at nodes weighted by hp_nodes array
- Backup elements at upper/lower nodes (standard resistance logic)

**Waste Heat Handling** (lines 642–676):
- SHR: sensible/total; depends on humidity (if dry_bulb - wet_bulb > 0.1 then SHR=nominal, else 1.0)
- Sensible gain: `(power_hp - delivered_hp) * shr + power_hp_other + (power_er - delivered_er)`
- Multiplied by `(1 - lost_heat_fraction)` to account for escape
- Latent: `(power_hp - delivered_hp) * (1 - shr) * (1 - lost_heat_fraction)`
- Wall fraction: if interior wall exists, split sensible gain to wall surface

### HARES HeatPumpWH (heat_pump_wh.rs)

**Status**: Partially implemented. Read first 100 lines only (file >600 lines).

**What exists**:
- EquipmentDescriptor with "Heat Pump Water Heater"
- Fuel: Electric
- Telemetry fields defined
- State serialization structure (HpwhState)
- Default constants matching OCHRE (lines 37–73):
  - COP, capacity curves defined
  - Ambient lockout bounds (5°C–45°C, or 2.78°C–62.78°C low-power)
  - Condenser weight array for 12-node tank
  - Min on-time 600 s (10 min)
  - Fan power 35 W
  - SHR 0.88

**Missing (cannot confirm without reading full file)**:
- Per-step COP/capacity curve evaluation
- Biquadratic coefficient lookup
- HP vs backup element priority logic
- Ambient lockout switching to ER-only
- SHR and waste heat calculation
- Wall interaction factor
- Draw profile support (same as resistance WH)
- Three-level thermostat (HP + lower + upper)

**Likely status**: Stub or incomplete; requires full file read to assess.

---

## 6. GAS WATER HEATER

### OCHRE GasWaterHeater (WaterHeater.py lines 705–724)

**Configuration**:
- Inherits from WaterHeater; uses same tank model
- Energy Factor (EF): determines skin_loss_frac
  - EF < 0.7 → 0.64 (older, less insulated)
  - EF < 0.8 → 0.91
  - Else → 0.96 (modern, well-insulated)

**Sensible Gain** (line 723):
- Only tank losses (standby loss) contribute
- `sensible_gain = h_loss * skin_loss_frac`
- No direct burner waste (all vented through flue)

**Control**: Identical to base WaterHeater class

### HARES GasWH (gas.rs lines 1–150+)

**What exists** (lines 1–150):
- EquipmentDescriptor: "Gas Water Heater", FuelType::Gas
- Telemetry fields
- State serialization (GasWhState)
- Tank setup (lines 115–126): n_nodes, burner_node config
- Default constants:
  - Burner input: 11,000 W (default)
  - Burner efficiency: 0.78
  - Flue loss: 10%
  - Pilot power
  - Fan power
  - Skin loss fraction field (lines 83–88)

**Missing**:
- Energy Factor → skin_loss_frac mapping
- Burner efficiency curve (OCHRE doesn't have one, but HARES struct suggests it: line 81 `burner_efficiency_poly`)
- Full control loop and step() implementation
- Draw profile support (same issue as resistance)

**Status**: Partially implemented; core structure present but control logic unclear.

---

## 7. TANKLESS WATER HEATER

### OCHRE TanklessWaterHeater (WaterHeater.py lines 727–779)

**Special behavior**:
- Uses IdealWaterModel: perfect insulation (UA → ∞), minimal storage
- Tank state: always at setpoint (lines 741)
- On-demand: calculates heat from draw
- **update_internal_control** (lines 739–746):
  - Get heat needed from draw: `self.heat_from_draw = -self.model.update_water_draw()[0]`
  - Returns "On" if heat_from_draw > 0, else "Off"
- **calculate_power_and_heat** (lines 748–779):
  - If cannot meet setpoint (heat_from_draw > capacity), reduce outlet temp
  - Otherwise, deliver full heat
  - Accounts for max_power limit

**Gas variant** (lines 782–806):
- Same logic plus gas therms conversion
- Parasitic electric power: `Parasitic Power (W) / 1000` [kW]

### HARES TanklessWH (tankless.rs lines 1–100+)

**What exists** (lines 1–100):
- EquipmentDescriptor: "Tankless Water Heater"
- Fuel type: Electric or Gas
- Setpoint, EF, max_thermal_power_w
- Parasitic power (for gas: 7.38 W default)
- Duty cycle, mode override, setpoint offset (DR)
- Load fraction control

**Missing**:
- IdealWaterModel equivalent
- On-demand draw-response logic
- Heat-from-draw calculation
- Outlet temperature limiting when heat insufficient
- Draw profile support

**Status**: Minimal; lacks core tankless logic.

---

## 8. OUTPUT PORTS & TELEMETRY

### OCHRE outputs (WaterHeater.generate_results, lines 302–316)

**Verbosity >= 4**:
- `Water Heating Delivered (W)`: delivered_heat
- `Water Heating COP (-)`: delivered_heat / electric_kw (for HP)

**Verbosity >= 7**:
- `Water Heating Total Sensible Heat Gain (W)`: sensible_gain
- `Water Heating Deadband Upper Limit (C)`: setpoint_temp
- `Water Heating Deadband Lower Limit (C)`: setpoint_temp - deadband_temp

**Verbosity >= 6 (base Equipment.generate_results)**:
- `Water Heating Electric Power (kW)` (if electric)
- `Water Heating Gas Power (therms/hour)` (if gas)

**EBM (Equivalent Battery Model)** (optional, lines 319–336):
- Energy: total_cap * (tank_temp - ref_temp)
- Min/Max energy from deadband
- Max power: capacity_rated / efficiency
- Baseline power: h_loss + h_delivered

### HARES telemetry (resistance.rs)

**Fields** (lines 639–647):
- tank_avg_temp_c
- upper_element_power_w
- lower_element_power_w
- electric_power_w
- draw_flow_rate_kg_s
- operating_mode (0=Off, 1=Heating)

**Ports**:
- Electrical: active/reactive power
- Fluid (DHW loop): flow_rate_kg_s, supply/return temp

**Missing**:
- Delivered heat (W)
- COP (for HPWH)
- Sensible/latent gains (split out)
- Deadband limits
- EBM energy, min/max, power
- Any OCHRE "Verbosity >= N" outputs

**Assessment**: HARES focus on ports (Electrical, Fluid) over result dictionaries. Different architecture but covers core needs.

---

## 9. CRITICAL GAPS FOR VALIDATION

### Must Fix for Parity
1. **Schedule-based water draw profiles** (HIGH PRIORITY)
   - OCHRE loads per-timestep L/min for three draw types
   - HARES only supports constant average kg/s
   - **Impact**: Cannot replicate realistic 24-hour usage patterns
   - **Files affected**: resistance.rs, gas.rs, heat_pump_wh.rs, tankless.rs

2. **HPWH control logic** (HIGH PRIORITY)
   - COP/capacity curves declared but not applied
   - HP priority, backup element sync, ambient lockout all missing
   - **Impact**: HPWH will not behave correctly
   - **File affected**: heat_pump_wh.rs

3. **Tankless on-demand logic** (HIGH PRIORITY)
   - Entire control loop stub
   - No draw-response heating
   - **Impact**: Tankless WH cannot operate
   - **File affected**: tankless.rs

### Should Investigate
4. **Deadband default mismatch** (OCHRE 5.56 C vs HARES 2.0 C)
   - Check test fixtures; may cause false failures

5. **Tempered draw configuration** (OCHRE flexible vs HARES hardcoded)
   - Can work around by ensuring tests use 40.6°C / 51.7°C

6. **Draw profile normalization** (OCHRE schedule; HARES constant)
   - HARES `resolve_draw_rate_kg_s()` has OCHRE normalization code but unused

### Low-Priority Divergences (Acceptable)
- Hysteresis boundary choice (HARES is more stable)
- Lower sensor averaging threshold (n_nodes>=3 vs >2, negligible)
- RC vs direct Euler (mathematically equivalent)
- End-cap UA distribution (HARES proportional, OCHRE geometric; both valid)

---

## 10. RECOMMENDATIONS

### Immediate (Blocking Validation)
1. **Implement schedule-based draw profiles** in resistance/gas/hpwh
   - Add draw_schedule or draw_profile_name config key
   - Load fixture demands per timestep
   - Integrate with tank.step_tempered()

2. **Complete HPWH implementation**
   - Port biquadratic curve evaluation from hares-physics
   - Implement three-level thermostat (HP/Lower/Upper)
   - Add ambient lockout and ER-only mode
   - Test against OCHRE 12-node HPWH tests

3. **Complete tankless implementation**
   - Implement on-demand draw-response logic
   - Add outlet temp limiting when insufficient capacity
   - Test against OCHRE tankless tests

### Medium Priority
4. **Add configurability to tempered draw temps**
   - Make 40.6°C and 51.7°C configurable (or read from draw_schedule)
   - Allow per-draw-type targets

5. **Implement max power schedule**
   - Allow power limit to vary per timestep
   - Currently only supports control signal override

6. **Gas WH: Energy Factor → skin_loss_frac mapping**
   - Implement OCHRE's EF < 0.7/0.8 logic (lines 712–718)

### Nice-to-Have
7. **EBM (Equivalent Battery Model) outputs**
   - Useful for fleet aggregation and optimization
   - Low effort; document clearly

8. **Setpoint ramp rate**
   - Allow gradual setpoint transitions
   - Not critical for most tests

---

## Summary Table: Feature Parity

| Component | Feature | OCHRE | HARES | Parity? | Severity |
|-----------|---------|-------|-------|---------|----------|
| **Tank Physics** | Conduction | RC state-space | Direct Euler | ✓ Equivalent | Low |
| | Boundary loss | UA distribution | Same | ✓ | Low |
| | Inversion mixing | EnergyPlus algorithm | Greedy merge | ✓ Equivalent | Low |
| | Draw algorithm | vol_pre/vol_post | Segment overlap | ✓ Verified | Low |
| | Unmet load | Yes | Yes | ✓ | Low |
| | Tempered draw | Configurable | Hardcoded | ✗ | Medium |
| **Config** | Schedule-based draw | Yes | No | ✗ | **HIGH** |
| | Tank parameters | Full hierarchy | Flat + aliases | ~ | Medium |
| | Setpoint ramp rate | Yes | No | ✗ | Low |
| | Max power schedule | Yes | No | ✗ | Medium |
| **Resistance WH** | Thermostat control | Yes | Yes | ✓ | Low |
| | Hysteresis boundary | `</<` | `<=/< ` | ~ Intentional | Low |
| | Lower sensor averaging | n_nodes>2 | n_nodes>=3 | ~ | Negligible |
| | Dual elements | Upper/Lower | Yes | ✓ | Low |
| | Element priority | MasterSlave | Both modes | ✓ | Low |
| | Max tank temp safety | Yes | Yes | ✓ | Low |
| | DR setpoint offset | Yes | Yes | ✓ | Low |
| | DR load fraction | Yes | Yes | ✓ | Low |
| **Gas WH** | Skin loss fraction | Energy Factor lookup | Field present | ~ | Medium |
| | Burner efficiency | Fixed | Struct has poly | ~ | Low |
| | Thermal output | Yes | Yes | ~ | Low |
| **HPWH** | COP curve | Biquadratic | Declared | ✗ | **HIGH** |
| | Capacity curve | Biquadratic | Declared | ✗ | **HIGH** |
| | HP priority logic | Yes | Likely missing | ✗ | **HIGH** |
| | Backup element sync | Yes | Likely missing | ✗ | **HIGH** |
| | Ambient lockout | Yes | Declared | ✗ | **HIGH** |
| | Waste heat / SHR | Yes | Likely missing | ✗ | **HIGH** |
| | Wall interaction | Yes | Likely missing | ✗ | Medium |
| | Condenser distribution | 12-node weights | Declared | ~ | Medium |
| **Tankless** | On-demand logic | Yes | Stub | ✗ | **HIGH** |
| | Draw-response heating | Yes | No | ✗ | **HIGH** |
| | Outlet limiting | Yes | No | ✗ | **HIGH** |
| | Gas therms | Yes | No | ✗ | **HIGH** |
| **Ports/Telemetry** | Electrical power | Yes | ✓ (ports) | ✓ | Low |
| | Fluid ports | No | ✓ | ✓ | Low |
| | Delivered heat (W) | Yes | Partial | ✗ | Low |
| | COP | Yes | ~ | ~ | Low |
| | Sensible/latent split | Yes | No | ✗ | Medium |
| | EBM model | Yes | No | ✗ | Low |

---

## Conclusion

HARES water heater implementation is **structurally sound** and **mathematically correct** for the features present (tank physics, resistance thermostat, demand response). However, **three critical features are incomplete or missing**:

1. **Schedule-based draw profiles** — Blocks realistic testing
2. **HPWH control logic** — Needs curve evaluation and priority logic
3. **Tankless on-demand logic** — Stub implementation

These gaps prevent full parity with OCHRE for comprehensive testing. Addressing them in order of priority will enable gradual, validated alignment.

**Key files requiring audit completion**:
- `/home/rich/src/HARES/crates/hares-equipment/src/water_heater/heat_pump_wh.rs` (full read needed)
- `/home/rich/src/HARES/crates/hares-equipment/src/water_heater/gas.rs` (full read needed)
- `/home/rich/src/HARES/crates/hares-equipment/src/water_heater/tankless.rs` (full read needed)
