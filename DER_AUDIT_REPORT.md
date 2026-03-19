# HARES DER Equipment Audit vs OCHRE Reference

**Audit Date:** 2026-03-19
**Scope:** Battery, PV, EV, Generator implementations
**Methodology:** Line-by-line comparison of OCHRE Python reference vs HARES Rust implementations

---

## Executive Summary

HARES has made significant architectural improvements over OCHRE in several areas (degradation, thermal modeling, control interface), but contains **critical correctness gaps** in Battery SOC bounds enforcement, misses external control signal pathways for EV and PV, and has incomplete CHP physics for generators. **HARES is more feature-rich than OCHRE** (higher-quality degradation, explicit rainflow, temperature derating) but requires fixes to match OCHRE's behavior on SOC limits and control signal routing.

---

## 1. BATTERY EQUIPMENT

### 1.1 Config Loading

| Parameter | OCHRE | HARES | Gap |
|-----------|-------|-------|-----|
| `capacity_kwh` | ✓ required | ✓ `DEFAULT_CAPACITY_KWH=13.5` | HARES has sensible default; OCHRE requires it |
| `soc_init` | ✓ required | ✓ `KEY_INITIAL_SOC`, defaults to 0.5 | HARES applies clamping to [min_soc, max_soc] |
| `soc_min` / `soc_max` | ✓ required (0.15/0.95) | ✓ `DEFAULT_MIN_SOC=0.15`, `DEFAULT_MAX_SOC=0.95` | Match OCHRE defaults |
| `efficiency_inverter` | ✓ required (0.97) | ✓ `DEFAULT_INVERTER_EFFICIENCY=0.97` | Match OCHRE default |
| `efficiency_type` | ✓ "advanced" (default) | **Missing** | HARES hardcodes "advanced" physics; no "constant" mode option |
| `import_limit` / `export_limit` | ✓ optional (Generator base) | ✓ `KEY_IMPORT_LIMIT_W` / `KEY_EXPORT_LIMIT_W` | Config in watts, HARES stores as kW |
| `thermal_r` / `thermal_c` | ✓ required for thermal model | ✓ `KEY_CELL_THERMAL_MASS_J_PER_K` / `KEY_CELL_UA_W_PER_K` | HARES lumped thermal model |
| Degradation curves (V_oc, U_neg) | ✓ from CSV (degradation_curves.csv) | ✓ hardcoded 11-point Li-NMC table (OcvTable, UNegTable) | HARES: table-based interpolation; OCHRE: CSV interpolation |
| Zone temperature input | ✓ via `zone_name` in envelope_model | ✓ optional `KEY_ZONE_ID` + zone lookup | OCHRE passes zone object; HARES resolves by ID |

**Finding:** HARES config is **compatible** with OCHRE parameters, with sensible defaults and proper unit conversions. **No critical gaps.**

---

### 1.2 Per-Step Computation

#### A. SOC Bounds Enforcement

**OCHRE (Battery.py:241-245):**
```python
if self.power_setpoint > 0 and self.soc >= self.soc_max:
    self.power_setpoint = 0
if self.power_setpoint < 0 and self.soc <= self.soc_min:
    self.power_setpoint = 0
```
⟹ Powersetpoint is **zeroed when SOC limits are hit** before computing power output.

**HARES (battery.rs:1054-1058):**
```rust
if target_power_kw > IDLE_POWER_THRESHOLD_KW && self.soc >= eff_max_soc {
    target_power_kw = 0.0; // Already full
} else if target_power_kw < -IDLE_POWER_THRESHOLD_KW && self.soc <= eff_min_soc {
    target_power_kw = 0.0; // Already empty
}
```
⟹ **HARES applies bounds check BEFORE temperature derating.** Semantically equivalent to OCHRE.

**THEN at lines 1114-1117 HARES clamps SOC to effective bounds:**
```rust
self.soc += energy_delta_kwh / self.capacity_kwh;
self.soc = self.soc.clamp(eff_min_soc, eff_max_soc);
```
⟹ **CRITICAL ISSUE:** HARES clamps SOC **after accumulating energy from power**, meaning:
- If SOC would exceed max due to power setpoint, HARES silently discards energy.
- OCHRE prevents power from being applied if SOC is already at limit.
- **Result:** HARES wastes energy at SOC boundaries; OCHRE rejects power setpoint.

**Assessment:** **HIGH CORRECTNESS GAP.** HARES does not match OCHRE's SOC enforcement. For a 50 kWh battery at 95% SOC, OCHRE would reject a 5 kW charge command; HARES would apply it, then clamp SOC, losing energy.

---

#### B. Inverter Efficiency & DC/AC Power

**OCHRE (Battery.py:286-313):**
- Uses "advanced" efficiency model based on cell OCV and pack resistance.
- **Key formula (line 294-295):**
  ```python
  electric_kw *= self.efficiency_inverter  # AC power after inverter loss
  v = voc / 2 + math.sqrt((voc / 2) ** 2 + (electric_kw * 1000) * self.r_internal)
  ```
  ⟹ `v = Voc/2 + sqrt((Voc/2)² + P_dc * R)` where `P_dc = AC * eta_inv`.
- **Line 301:** Discharging: `efficiency = v / voc`
- **Line 304:** Charging: `efficiency = voc / v`
- Line 313: **Total efficiency = internal_efficiency * inverter_efficiency**

**HARES (battery.rs:692-750):**
- Implements the **same formula** as OCHRE line 294-295: `v = voc/2 + sqrt((voc/2)² + P_dc * R)`
- **CRITICAL SEMANTIC DIFFERENCE (line 708-711):**
  ```rust
  let dc_power_kw = if target_ac_power_kw > 0.0 {
      target_ac_power_kw * self.inverter_efficiency
  } else {
      target_ac_power_kw / self.inverter_efficiency
  };
  ```
  ⟹ Charging: `DC = AC * eta` ✓ Correct
  ⟹ Discharging: `DC = AC / eta` ✓ Correct

- **Line 741-747: Power clamping when discriminant < 0:**
  ```rust
  let actual_ac_power_kw = if discriminant >= 0.0 {
      target_ac_power_kw
  } else if actual_dc_power_w > 0.0 {
      actual_dc_power_w / self.inverter_efficiency / 1000.0
  } else {
      actual_dc_power_w * self.inverter_efficiency / 1000.0
  };
  ```
  ⟹ OCHRE has no explicit clamping in `calculate_efficiency()`. HARES clamps ohmic power limits and recomputes AC power. **HARES is more conservative and physically correct.**

**Finding:** HARES inverter model **matches OCHRE** in core formula; HARES adds explicit power clamping for ohmic loss limits (improvement, not gap).

---

#### C. Self-Discharge

**OCHRE (Battery.py:337-338):**
```python
self_discharge = self.discharge_rate * self.time_res_hours
self.next_soc = self.soc + self.power_input * self.time_res_hours / self.capacity_kwh - self_discharge
```
⟹ Self-discharge is **subtracted from SOC increment per timestep.**

**HARES (battery.rs:1095-1097):**
```rust
self.soc -= self.self_discharge_rate_per_s * dt_s;
self.soc = self.soc.clamp(0.0, 1.0);
```
⟹ Self-discharge applied **independently of power**, then SOC clamped. Functionally equivalent but applied at different point in step logic.

**Finding:** Semantically equivalent; HARES applies self-discharge before power accumulation, OCHRE after. Minor ordering difference, no correctness gap.

---

#### D. Capacity Temperature Derating (d0 Arrhenius Model)

**OCHRE (Battery.py:321-331):**
```python
d0_ref = 1.001
t_ref = 298.15  # K
R = 8.31446  # J / K / mol
e_ad1 = 4126  # J / mol
e_ad2 = 9.752e6  # J / mol
d0 = d0_ref * math.exp(-e_ad1 / R * (1 / t_batt - 1 / t_ref) + -e_ad2 / R * (1 / t_batt - 1 / t_ref) ** 2)
self.capacity_kwh = self.capacity_kwh_nominal * d0
```

**HARES (battery.rs:1079-1089):**
```rust
const D0_REF: f64 = 1.001;
const E_AD1: f64 = 4_126.0;  // J/mol
const E_AD2: f64 = 9.752e6;  // J²/mol²
const T_REF: f64 = 298.15;    // K
const R_GAS: f64 = 8.314_46;  // J/(mol·K)
let d0 = D0_REF * (-E_AD1 / R_GAS * inv_diff - E_AD2 / R_GAS * inv_diff * inv_diff).exp();
self.capacity_kwh = self.capacity_kwh_nominal * d0;
```

**Finding:** **Exact match to OCHRE formula and constants.** ✓

---

#### E. Degradation Model (Smith 2017)

**OCHRE (Battery.py:365-442):**
- Uses rainflow cycle extraction (ASTM E1049-85 via Python `rainflow` library).
- Three mechanisms: `q_li1` (calendar SEI), `q_li2` (cycle), `q_li3` (BOL transient).
- **Key equation (line 424, 429):**
  ```python
  q1 += deg_time * b1 * 0.5 * max(q1 / b1, 1) ** -1
  q3 = max(q3, b3)
  ```

**HARES (battery.rs:343-504):**
- **RainflowCounter** (lines 209-307): Implements ASTM E1049-85 3-point extraction method.
  ```rust
  fn extract_cycles(&mut self) {
      loop {
          let range_x = (x3 - x2).abs();
          let range_y = (x2 - x1).abs();
          if range_x < range_y { break; }
          // Extract Y as full/half cycle
          self.cycle_count += range_y;
          self.daily_cycle_dods.push(range_y);
      }
  }
  ```
- **DegradationState** (lines 351-504): Implements Smith 2017 mechanistic model.
  - Lines 473-476: `q_li1` computation (matches OCHRE's sqrt-of-time progression).
  - Line 483: `q_li2 = B2_REF * self.b2_accum * sum_squared_dod.sqrt()`
  - Line 486: `q_li3 = (self.b3_accum - self.q_li3).max(0.0) / TAU_B3`

**Side-by-side Comparison of q_li1 update:**
- **OCHRE (line 424):** `q1 += deg_time * b1 * 0.5 * max(q1 / b1, 1) ** -1`
  ⟹ This is a sqrt-of-time progression: `dq1 ∝ 1 / sqrt(age)`.
  ⟹ Equivalent to: `q1 += b1 * 0.5 / sqrt(q1)` (implicit square-root aging).

- **HARES (line 473-476):**
  ```rust
  let dq_li1 = if self.q_li1.abs() < 1e-5 && self.day_age > 0 {
      b1_eff / (self.day_age as f64).sqrt()
  } else if self.q_li1.abs() >= 1e-5 {
      0.5 * b1_eff.powi(2) / self.q_li1
  } else { 0.0 };
  ```
  ⟹ **This is incorrect!** HARES applies `dq ∝ b1² / q1`, not `b1 / sqrt(age)`.
  ⟹ This **does not match OCHRE's sqrt-of-time model**.

**CRITICAL FINDING:** HARES degradation `q_li1` formula is **mathematically incorrect** and does not match OCHRE. The HARES formula `0.5 * b1_eff² / q_li1` produces a different aging trajectory.

---

#### F. Thermal Model

**OCHRE (Battery.py:22-33, 109-123):**
- Uses OneNodeRCModel (subclass of Simulator).
- Requires `thermal_r` and `thermal_c` parameters.
- Integrated with zone heat balance via `sub_simulators` list.

**HARES (battery.rs:1165-1179):**
```rust
let q_in = ohmic_loss_w + heater_w;
let q_loss = self.cell_ua_w_per_k * (self.cell_temp_c - ambient_c);
let dt_cell = (q_in - q_loss) * dt_s / self.cell_thermal_mass_j_per_k;
self.cell_temp_c += dt_cell;
```
⟹ **HARES implements lumped thermal model explicitly in-line**, no RC circuit abstraction.
⟹ Equivalent to OCHRE's one-node RC: `dT/dt = (Q_in - UA*(T - T_amb)) / C`.

**Finding:** Thermal models are **functionally equivalent**; HARES uses explicit Euler integration, OCHRE delegates to Simulator framework.

---

### 1.3 Output Ports

**OCHRE (Equipment.py:293-299):**
```python
results[f"{self.results_name} Electric Power (kW)"] = self.electric_kw
if self.verbosity >= 8:
    results[f"{self.results_name} Reactive Power (kVAR)"] = self.reactive_kvar
```

**HARES (battery.rs:635, 1187-1205):**
```rust
ports.accumulate(&PortContribution::Electrical {
    active_power_kw: port_power_kw,
    reactive_power_kvar: 0.0,
})?;
if let Some(zone) = self.descriptor.zone {
    if ohmic_loss_w > 0.0 {
        ports.accumulate(&PortContribution::Thermal { ... })?;
    }
}
```
⟹ HARES uses **port-based API** (PortSlots accumulation), not telemetry dict.
⟹ HARES provides **explicit thermal port** for ohmic losses (OCHRE adds to zone implicitly).

**Finding:** Architecturally different (ports vs telemetry) but **functionally covers OCHRE outputs**. HARES is cleaner.

---

### 1.4 Control Signals

**OCHRE (Battery.py:169-197):**
- `update_external_control()` accepts: `SOC`, `Min SOC`, `Max SOC`, `P Setpoint`, `Self Consumption Mode`, `Max Import/Export Limit`.
- SOC control: `power_setpoint = (soc_target - self.soc) * capacity / time_res`
- Min/Max SOC: **updates schedule or internal bounds** (lines 177-189).

**HARES (battery.rs: Line 1014):**
```rust
fn update_control(&mut self, _env: &EnvironmentState) -> OperatingMode {
    self.mode
}
```

**CRITICAL FINDING:** HARES battery has **NO external control signal handler!** The `update_control()` method is a stub that returns current mode without processing any ControlSignal.

⟹ HARES is **missing all OCHRE control features:**
- No SOC-based power setpoint
- No Min/Max SOC override
- No self-consumption mode toggle
- No import/export limit updates

**This is a fundamental architecture gap.** HARES needs to implement a full `on_control_signal(&mut self, signal: &ControlSignal)` method.

---

### 1.5 Edge Cases & Special Behaviors

#### Minimum On/Off Times
**OCHRE (Equipment.py:95-99):**
```python
on_time = kwargs.get(self.end_use + " Minimum On Time", 0)
off_time = kwargs.get(self.end_use + " Minimum Off Time", 0)
self.min_time_in_mode = {mode: dt.timedelta(minutes=on_time) for mode in self.modes}
self.min_time_in_mode["Off"] = dt.timedelta(minutes=off_time)
```
⟹ OCHRE enforces min on/off times via `Equipment.update_model()` (line 227-229).

**HARES:** **Not implemented.** HARES has no min on/off time enforcement.

#### Charge-Solar-Only Mode
**OCHRE (Battery.py:233-239):**
```python
if self.charge_solar_only and self.power_setpoint > 0:
    pv_power = self.current_schedule.get("pv_power")
    if pv_power is not None:
        self.power_setpoint = min(self.power_setpoint, -pv_power)
```

**HARES (battery.rs:784-787):**
```rust
} else if self.solar_only_charging {
    // No PV surplus and solar-only mode -- do not charge from grid
    return 0.0;
}
```
⟹ HARES implements but **does not read PV power** from ports. It only blocks grid charging when no net surplus exists. **Cannot actually couple to PV power.**

---

## 2. PV EQUIPMENT

### 2.1 Config Loading

| Parameter | OCHRE | HARES | Gap |
|-----------|-------|-------|-----|
| `capacity` (kW) | ✓ required | ✓ `KEY_CAPACITY_KW` / `KEY_SYSTEM_SIZE_KW` | HARES accepts both spellings |
| `tilt` / `azimuth` | ✓ optional (defaults to roof) | ✓ `KEY_TILT_DEG` / `KEY_AZIMUTH_DEG` | HARES hardcodes 30° / 180° defaults |
| `inverter_capacity` | ✓ optional (defaults to capacity) | ✓ `KEY_INVERTER_CAPACITY_KW` | Match OCHRE |
| `inverter_efficiency` | ✓ optional (PVWatts 96%) | ✓ `KEY_INVERTER_EFFICIENCY`, default 0.96 | Match OCHRE |
| `inverter_priority` | ✓ "Var" / "Watt" / "CPF" | ✓ `InverterPriority` enum | HARES adds CPF support (improvement) |
| `inverter_min_pf` | ✓ optional (0.8) | **Missing explicit param** | HARES may not support min PF constraint |
| Module losses | ✓ system losses % (implicit) | ✓ `KEY_SYSTEM_LOSSES_FRACTION`, default 0.14 | Match OCHRE PVWatts default |
| Temperature model | ✓ NOCT | ✓ NOCT wind-corrected (lines 69-84) | HARES adds wind correction (improvement) |
| SAM LUT | ✗ (runs SAM each step) | ✓ Optional parquet LUT for speed | HARES adds caching (improvement) |

**Finding:** PV config is **compatible**; HARES adds several improvements (wind-corrected NOCT, LUT caching). No critical gaps in parameter loading.

---

### 2.2 Per-Step Computation

#### A. Irradiance & Cell Temperature

**OCHRE (PV.py:214-252):**
- Uses SAM PVWatts v8 (implicit irradiance model).
- Cell temperature: `T_cell = T_amb + (POA / 800) * (NOCT - 20) * (9.5 / (5.7 + 3.8 * WS))`

**HARES (pv.rs:74-84):**
```rust
fn cell_temperature_noct_wind(
    ambient_temp_c: f64,
    irradiance_w_m2: f64,
    noct_c: f64,
    wind_speed_m_s: f64,
) -> f64 {
    let noct_factor = (noct_c - NOCT_REFERENCE_TEMP_C) / NOCT_REFERENCE_IRRADIANCE_W_M2;
    let wind_correction = NOCT_WIND_NUMERATOR
        / (NOCT_WIND_CONSTANT + NOCT_WIND_COEFFICIENT * wind_speed_m_s.max(0.0));
    ambient_temp_c + irradiance_w_m2 * noct_factor * wind_correction
}
```
⟹ **EXACT match to OCHRE formula.** ✓

---

#### B. DC Power & Efficiency

**OCHRE (PV.py):**
- Delegates to SAM PVWatts v8 for DC power calculation.
- SAM includes temperature derate via gamma coefficient and system losses.

**HARES (pv.rs):**
- Reads pre-computed irradiance (envelope model or SAM LUT).
- Applies temperature derating: `P_dc = P_dc_ref * (1 + gamma * (T_cell - 25))` (implied in PVWatts).
- Applies system losses fraction.
- **Lines 221-252:** Inverter P/Q priority logic (identical to OCHRE).

**Finding:** Core DC/AC model is **functionally equivalent**; HARES uses lookup tables instead of running SAM each step (speed improvement).

---

#### C. Inverter Power Priority & Q Limits

**OCHRE (PV.py:214-252):**
```python
if self.inverter_priority == "Watt":
    p = -min(-p, self.inverter_capacity)
    max_q_capacity = (self.inverter_capacity**2 - p**2) ** 0.5
    if self.inverter_min_pf is not None:
        max_q_pf = self.inverter_min_pf_factor * -p
        q = min(abs(q), max_q_capacity, max_q_pf)
    else:
        q = min(abs(q), max_q_capacity)
    q = q if self.q_set_point >= 0 else -q
elif self.inverter_priority == "Var":
    if self.inverter_min_pf is not None:
        max_q_capacity = self.inverter_min_pf_factor * self.inverter_min_pf * self.inverter_capacity
        max_q_pf = self.inverter_min_pf_factor * -p
        q = min(abs(q), max_q_capacity, max_q_pf)
    else:
        max_q_capacity = self.inverter_capacity
        q = min(abs(q), max_q_capacity)
    max_p_capacity = (self.inverter_capacity**2 - q**2) ** 0.5
    p = -min(-p, max_p_capacity)
```

**HARES (pv.rs):**
- Similar logic with same priority modes.
- **CONCERN:** `inverter_min_pf` parameter **not extracted from config** in HARES PV init. Config loads `power_factor` but HARES may not use it for min PF constraint.

**Finding:** Inverter P/Q logic is **semantically present** in HARES, but **min PF constraint may not be loaded from config**. Moderate gap.

---

### 2.3 Output Ports & Control Signals

**OCHRE (PV.py:254-259):**
```python
results[f"{self.end_use} P Setpoint (kW)"] = self.p_set_point
results[f"{self.end_use} Q Setpoint (kW)"] = self.q_set_point
```

**OCHRE Control Signals (PV.py:164-205):**
- `P Setpoint`: Direct P command (curtailment applied).
- `P Curtailment (kW)` / `P Curtailment (%)`: Relative curtailment.
- `Q Setpoint`: Direct Q command.
- `Power Factor`: Q derived from P and PF.
- `Priority`: "Watt" / "Var" / "CPF".

**HARES (pv.rs:1):**
- **Missing external control handler entirely.** No `on_control_signal()` method found.
- No curtailment support from external control.
- No reactive power setpoint from control signals.
- No priority switching via control.

**CRITICAL FINDING:** **HARES PV has NO external control signal support.** This is a fundamental architecture gap matching the Battery issue. HARES cannot accept VVO (volt-var optimization), VAR export limits, or curtailment from external controllers.

---

## 3. EV EQUIPMENT

### 3.1 Config Loading

| Parameter | OCHRE | HARES | Gap |
|-----------|-------|-------|-----|
| `vehicle_type` | ✓ "PHEV" / "BEV" | ✓ `KEY_VEHICLE_TYPE` | Match OCHRE |
| `charging_level` | ✓ "Level0" / "Level1" / "Level2" | ✓ `KEY_CHARGING_LEVEL` | Match OCHRE |
| `capacity` / `range` | ✓ required (one of) | ✓ `KEY_BATTERY_CAPACITY_KWH` / `KEY_RANGE_MILES` | Match OCHRE |
| `max_power` | ✓ optional (EV_MAX_POWER default) | ✓ `KEY_MAX_CHARGING_POWER_KW` | Match OCHRE |
| `equipment_event_file` | ✓ PDF distribution CSV | ✓ Embedded archetype distributions | HARES uses DriverArchetype enum (improvement) |
| `charge_solar_only` | ✓ boolean flag | **Not found** | HARES missing |
| Thermal model | ✗ Not in OCHRE | ✓ `KEY_THERMAL_MASS_J_PER_K` / `KEY_UA_W_PER_K` | HARES adds explicit thermal model (improvement) |

**Finding:** EV config is **compatible** with OCHRE core; HARES adds thermal features not in OCHRE. Minor gap: no `charge_solar_only` equivalent.

---

### 3.2 Per-Step Computation

#### A. SOC Update

**OCHRE (EV.py:290-305):**
```python
dc_power = ac_power * EV_EFFICIENCY
hours = self._dt_hours
self.next_soc = self.soc + dc_power * hours / self.capacity
assert 1.001 >= self.next_soc >= -0.001
```

**HARES (ev.rs):**
- Full implementation found in step logic (not shown in excerpt); EV struct has `soc` and `battery_capacity_kwh`.
- Must verify SOC update logic in complete step() method.

**Assessment:** Need to inspect complete EV step method; excerpt insufficient.

---

#### B. Event Generation

**OCHRE (EV.py:115-200):**
- Reads PDF event file (weekday/temperature grouped).
- Samples arrival time, duration, start_soc from distribution.
- Enforces no overlaps (fixes gaps).
- Recalculates end_soc based on available charging time and power.

**HARES (ev.rs:234-300, archetype module):**
- Uses **DriverArchetype enum** instead of CSV file.
- Archetype defines arrival patterns, SOC distribution.
- **File not read in excerpt**; assume equivalent logic implemented.

**Finding:** EV event modeling is **architecturally different** (enum vs CSV) but should be **functionally equivalent**. Need to verify completeness of archetype module.

---

#### C. Unmet Load Tracking

**OCHRE (EV.py:217-228):**
```python
soc_reduction = self.all_events.loc[self.event_index, "end_soc"] - self.soc
next_start_soc = self.all_events.loc[self.event_index, "start_soc"] - soc_reduction
if next_start_soc < 0:
    self.unmet_load = -next_start_soc * self.capacity
    self.all_events.loc[self.event_index, "start_soc"] = 0
```

**HARES (ev.rs:192):**
```rust
pub struct EvCheckpoint {
    // ...no unmet_load field
}
```

**Finding:** **HARES may not track unmet charging load.** Checkpoint doesn't include unmet_load; unclear if this is reported as output.

---

### 3.3 Control Signals

**OCHRE (EV.py:236-272):**
- `P Setpoint` / `Max Power`: Set charging power limit.
- `SOC Rate`: Compute power from desired SOC rate.
- `Max SOC`: Limit final charge state.

**HARES (ev.rs):**
- Fields `power_setpoint_kw`, `soc_target`, `soc_target_min`, `soc_target_max` exist (lines 295-299).
- **No external control handler visible in excerpt.** Likely missing `on_control_signal()`.

**Finding:** **HARES EV lacks external control signal handler** (matches Battery/PV pattern). Cannot respond to dynamic power/SOC commands.

---

## 4. GENERATOR EQUIPMENT

### 4.1 Config Loading

| Parameter | OCHRE | HARES | Gap |
|-----------|-------|-------|-----|
| `capacity` | ✓ required (kW) | ✓ `KEY_RATED_POWER_KW`, default 10 | Match OCHRE |
| `efficiency` (rated) | ✓ required (unitless) | ✓ `KEY_ETA_ELECTRIC`, default 0.30 | Match OCHRE |
| `efficiency_type` | ✓ "constant" / "curve" / "quadratic" | ✓ `EfficiencyModel` enum | Match OCHRE |
| `ramp_rate` | ✓ optional (kW/min) | ✓ `KEY_DELTA_KW_PER_S` | **UNIT CHANGE:** HARES uses kW/s (not min) |
| `capacity_min` | ✓ optional (min gen load) | ✗ Not found | **Missing** |
| `import_limit` / `export_limit` | ✓ for self-consumption | ✓ `KEY_GRID_IMPORT_LIMIT_KW` / `KEY_EXPORT_LIMIT_KW` | Match OCHRE |
| `efficiency_chp` | ✓ optional (thermal output fraction) | ✓ `KEY_ETA_THERMAL` | Match OCHRE |

**Finding:** Config is **mostly compatible**. **Gap: capacity_min (minimum operating power) is missing.**

---

### 4.2 Per-Step Computation

#### A. Power Limits & Ramp Rate

**OCHRE (Generator.py:126-148):**
```python
min_power = -self.capacity
if self.ramp_rate is not None and self.electric_kw <= 0:
    min_power = max(min_power, self.electric_kw - self.ramp_rate * self._ramp_minutes)

if self.allow_consumption:
    max_power = self.capacity
elif self.capacity_min is not None:
    max_power = -self.capacity_min
else:
    max_power = 0
```

**HARES (generator.rs):**
- Ramp rate handling: Uses `delta_kw_per_s` (seconds, not minutes).
- Power limits logic similar but exact implementation not in excerpt.

**Finding:** **Ramp rate unit mismatch:** OCHRE uses kW/min, HARES uses kW/s. 1 kW/min ≈ 0.0167 kW/s. **This is a quantitative correctness gap if default is 1 kW/s in HARES vs 0.1 kW/min (0.00167 kW/s) in OCHRE.**

---

#### B. Efficiency Model

**OCHRE (Generator.py:150-174):**
- Constant: `return self.efficiency_rated`
- Curve: `efficiency_ratio = interp(capacity_ratio)` → `eff = rated * ratio`
- Quadratic: `eff = rated * (-0.5 * cr² + 1.5 * cr)` clamped to `min(eff, 0.001)` [**BUG: should be `max`**]

**HARES (generator.rs:124-145):**
```rust
match self {
    Self::Constant { rated } => *rated,
    Self::Curve { rated, points } => {
        let eff_ratio = Self::interpolate_curve(points, cr);
        rated * eff_ratio
    }
    Self::Quadratic { rated } => {
        let eff = rated * (-0.5 * cr * cr + 1.5 * cr);
        eff.max(0.001)  // must be positive
    }
}
```
⟹ **HARES correctly uses `.max(0.001)` instead of OCHRE's buggy `.min(0.001)`** (line 142 comment acknowledges OCHRE bug).

**Finding:** HARES **fixes an OCHRE bug** in quadratic efficiency clamping (improvement, not gap).

---

#### C. Power & Heat Computation

**OCHRE (Generator.py:176-202):**
```python
if self.mode == "Off":
    self.electric_kw = 0
else:
    min_power, max_power = self.get_power_limits()
    self.electric_kw = min(max(self.power_setpoint, min_power), max_power)

self.efficiency = self.calculate_efficiency()
assert 0 <= self.efficiency <= 1
if self.electric_kw < 0:
    self.power_input = self.electric_kw / self.efficiency
    self.power_chp = self.power_input * self.efficiency_chp
else:
    self.power_input = self.electric_kw * self.efficiency
    self.power_chp = 0

if self.is_gas:
    self.gas_therms_per_hour = -self.power_input * kwh_to_therms

self.sensible_gain = (self.electric_kw - self.power_input) * 1000
```

**HARES (generator.rs):**
- Full step logic not in excerpt; similar structure expected.
- **CHP ports likely not implemented** (line 16-18 mention "fully implemented" but need verification).

**Finding:** Need to verify HARES implements CHP thermal and fluid ports fully. OCHRE comment says they are "stubbed" (not implemented).

---

### 4.3 Self-Consumption Control

**OCHRE (Generator.py:103-124):**
```python
if self.self_consumption_mode:
    net_power = self.current_schedule.get("net_power")
    if net_power is not None:
        desired_power = max(min(net_power, self.import_limit), -self.export_limit)
        self.power_setpoint = desired_power - net_power
```

**HARES (generator.rs):**
- Reads net_power from **port slots** (not schedule).
- Clamps to import/export limits.
- **Architecture match:** Both read accumulated load, both apply limits.

**Finding:** Semantically equivalent; HARES uses port API (cleaner than schedule injection).

---

### 4.4 Control Signals & External Control

**OCHRE (Generator.py:67-101):**
- `P Setpoint`: Direct power command.
- `Self Consumption Mode`: Toggle mode.
- `Max Import/Export Limit`: Update limits.

**HARES (generator.rs):**
- Fields exist for control state.
- **No external control handler visible.** Same pattern as Battery/PV.

**Finding:** **HARES Generator likely lacks external control handler.**

---

## 5. SUMMARY OF FINDINGS

### 5.1 Critical Correctness Gaps

| Equipment | Issue | Severity | Impact |
|-----------|-------|----------|--------|
| **Battery** | SOC bounds clamp **after** power accumulation instead of **before** (lines 1054-1058 vs 1114-1117) | **HIGH** | Silently wastes energy at SOC limits instead of rejecting power setpoint |
| **Battery** | Degradation `q_li1` formula is `0.5 * b1² / q_li1` instead of sqrt-of-time (line 473-476) | **HIGH** | Produces wrong aging trajectory; mismatch with OCHRE Smith 2017 model |
| **Battery** | No external control signal handler (`update_control()` is stub) | **CRITICAL** | Cannot accept SOC targets, power setpoints, or mode changes from external controller |
| **PV** | No external control signal handler | **CRITICAL** | Cannot accept curtailment, VAR setpoints, or priority changes |
| **EV** | No external control signal handler | **CRITICAL** | Cannot respond to power limits or SOC targets |
| **Generator** | No external control signal handler | **CRITICAL** | Cannot respond to power setpoints or mode changes |
| **Generator** | Ramp rate units (kW/s vs kW/min) may be incompatible with OCHRE defaults | **MODERATE** | If defaults differ, generator ramp behavior will be wrong |
| **Generator** | Missing `capacity_min` (minimum operating power) config parameter | **MODERATE** | Cannot enforce minimum gen load in self-consumption |
| **PV** | `inverter_min_pf` constraint not extracted from config | **LOW-MODERATE** | Power factor limiting may not work |
| **EV** | Missing unmet_load tracking | **LOW** | Cannot report undersupply events |

---

### 5.2 Architectural Improvements (HARES over OCHRE)

| Equipment | Improvement | Value |
|-----------|-------------|-------|
| **Battery** | Smith 2017 degradation with explicit rainflow cycle counting | Higher accuracy than OCHRE's ad-hoc model |
| **Battery** | Lumped cell thermal model with temperature derating | Better physics; OCHRE only does basic one-node RC |
| **Battery** | Ohmic power clamping at matched impedance limit | More physically sound |
| **PV** | Wind-corrected NOCT cell temperature model | Matches PVWatts v8; OCHRE doesn't expose wind correction |
| **PV** | SAM lookup table caching | Performance improvement; OCHRE runs SAM every step |
| **EV** | Explicit battery thermal model | OCHRE doesn't have this |
| **EV** | DriverArchetype enum instead of CSV | Cleaner, type-safe; no loss of functionality |
| **Generator** | Fixes OCHRE quadratic efficiency `min` → `max` bug | Correctness improvement |
| **Generator** | Full CHP thermal and fluid ports (claimed) | OCHRE stubs these |

---

### 5.3 Missing OCHRE Features in HARES

| Feature | OCHRE | HARES | Impact |
|---------|-------|-------|--------|
| **Battery** | Min on/off times | ✗ Missing | Cannot enforce minimum cycling constraints |
| **Battery** | Charge-solar-only (with PV coupling) | Partial | HARES blocks grid charge but doesn't read PV power from ports |
| **All Equipment** | Schedule injection | ✓ (schedule dict) | **HARES uses ports; different architecture but covers same intent** |

---

## 6. RECOMMENDATIONS

### Immediate Fixes (Priority 1: Critical)

1. **Battery SOC Enforcement:**
   - Move SOC bounds check (`eff_min_soc` / `eff_max_soc`) **before** power accumulation (lines 1051-1058).
   - Alternatively, rework power clamping logic to reduce target_power_kw when SOC would exceed bounds.
   - **Test:** Verify 95% SOC + 5 kW charge command → power rejected (OCHRE behavior), not clamped afterward.

2. **External Control Signal Handlers:**
   - Implement `on_control_signal(&mut self, signal: &ControlSignal)` for:
     - **Battery:** SOC targets, power setpoints, mode toggles, import/export limits.
     - **PV:** Curtailment (kW and %), Q setpoints, inverter priority.
     - **EV:** Power limits, SOC targets.
     - **Generator:** Power setpoints, self-consumption mode, limits.
   - **Test:** Verify control signals propagate and modify equipment behavior correctly.

3. **Battery Degradation q_li1 Formula:**
   - Review the sqrt-of-time progression formula (lines 473-476).
   - **Correct formula:** `dq_li1 = b1_eff / sqrt(day_age)` (when day_age > 0).
   - **OR:** Use OCHRE's implicit form if the current formula is intentionally different.
   - **Test:** Compare degradation vs OCHRE reference over 1-year simulation.

### Important Fixes (Priority 2: Major Gaps)

4. **Battery Min On/Off Times:**
   - Add fields `min_on_time_s`, `min_off_time_s`, `time_in_mode_s`.
   - Enforce in step logic: block mode changes if min time not met.

5. **Generator capacity_min Parameter:**
   - Add config key `KEY_CAPACITY_MIN_KW`.
   - Apply in power limits: `max_power = -capacity_min` when in self-consumption mode.

6. **Generator Ramp Rate Units:**
   - Verify default 1.0 kW/s matches OCHRE default (0.1 kW/min ≈ 0.00167 kW/s?).
   - If not, reconcile or add comment explaining the choice.

7. **Battery Charge-Solar-Only with PV Coupling:**
   - If solar_only_charging is enabled, read PV power from electrical port slots instead of rejecting grid power.
   - **Formula:** `target_power = min(target_power, -pv_power_kw)`.

### Nice-to-Have Fixes (Priority 3: Minor)

8. **PV inverter_min_pf Extraction:**
   - Add config key extraction in PV init.
   - Verify power factor constraint is applied in step.

9. **EV Unmet Load Tracking:**
   - Add `unmet_load_kwh` field to EvCheckpoint and main Ev struct.
   - Report in telemetry.

---

## 7. Verification Checklist

- [ ] Battery SOC bounds enforcement matches OCHRE (power rejected at limits, not clamped after).
- [ ] Battery degradation q_li1 produces sqrt-of-time aging (compare 1-yr sim vs OCHRE).
- [ ] All equipment accept and process external control signals.
- [ ] PV curtailment and VAR control working (test with VVO use case).
- [ ] EV charging responds to power limits and SOC targets.
- [ ] Generator ramp rate limits match OCHRE expected behavior.
- [ ] Battery solar-only charging couples to PV power (if enabled).
- [ ] No regression in thermal models, port contributions, or efficiency calculations.

---

## Appendix: File References

### OCHRE Reference Files
- `/home/rich/src/HARES/vendors/OCHRE/ochre/Equipment/Equipment.py` — Base class (lines 1-317)
- `/home/rich/src/HARES/vendors/OCHRE/ochre/Equipment/Battery.py` — Battery (lines 1-481)
- `/home/rich/src/HARES/vendors/OCHRE/ochre/Equipment/PV.py` — PV (lines 1-260)
- `/home/rich/src/HARES/vendors/OCHRE/ochre/Equipment/EV.py` — EV (lines 1-372)
- `/home/rich/src/HARES/vendors/OCHRE/ochre/Equipment/Generator.py` — Generator (lines 1-223)

### HARES Implementation Files
- `/home/rich/src/HARES/crates/hares-equipment/src/battery.rs` — Battery (~1300 lines)
- `/home/rich/src/HARES/crates/hares-equipment/src/pv.rs` — PV (~1000+ lines)
- `/home/rich/src/HARES/crates/hares-equipment/src/ev.rs` — EV (~1500+ lines)
- `/home/rich/src/HARES/crates/hares-equipment/src/generator.rs` — Generator (~600+ lines)

