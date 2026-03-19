# Comprehensive DER Equipment Comparison: OCHRE vs HARES (Detailed Analysis)

**Analysis Date:** March 19, 2026
**Scope:** Complete feature-by-feature audit of Battery, PV, EV, and Generator equipment
**Goal:** Identify ALL areas where OCHRE has capabilities HARES lacks, OR where HARES is inferior

---

## FINDINGS SUMMARY

### Critical Gaps in HARES (Features OCHRE Has, HARES Lacks)

#### BATTERY
1. **Import/Export Grid Limits** — OCHRE: ✓ `import_limit`, `export_limit` in Generator (inherited by Battery)
   - HARES: **Only in Generator, NOT in Battery module** (`battery.rs` lines 513–580 show NO export/import limiting)
   - **Impact:** HARES Battery cannot directly enforce grid exchange constraints; must be handled externally

2. **Minimum Operating Power (MOL)** — OCHRE: ✓ `capacity_min` for self-consumption cycling prevention
   - HARES: ✗ No equivalent
   - **Impact:** HARES may exhibit excessive on/off cycling in self-consumption mode

#### PV
1. **Real-Time SAM Integration** — OCHRE: ✓ `PySAM.Pvwattsv8` with live irradiance
   - HARES: ✓ LUT-based (pre-computed) — **Different approach, not inferior**
   - **Assessment:** Trade-off — OCHRE flexible, HARES faster/reproducible

2. **Soiling Losses** — OCHRE: ✗ Not implemented | HARES: ✗ Not implemented
   - **Both lack this**

3. **Snow Loss Modeling** — OCHRE: ✗ Not implemented | HARES: ✗ Not implemented
   - **Both lack this**

4. **Module Degradation** — OCHRE: ✗ Not implemented | HARES: ✗ Not implemented
   - **Both lack this**

5. **Single-Point MPPT Limit** — Both OCHRE and HARES
   - HARES: ✓ Multi-array support (up to 16 different tilt/azimuth), but single MPPT
   - OCHRE: Single array, single MPPT
   - **HARES actually BETTER here** (multi-array > single array)

#### EV
1. **DC Fast Charging (DCFC)** — OCHRE: ✗ Not implemented | HARES: ✗ Not implemented
   - **Both lack CCS/CHAdeMO support (50–350 kW)**
   - **Critical gap in both**

2. **Charging Flexibility** (ACTUALLY HARES IS BETTER)
   - OCHRE: Event-based PDF only; no TOU or schedule-aware charging
   - HARES: ✓ TOU avoidance, immediate targets, ready-by-time
   - **HARES advantage**

3. **V2G/V2H Discharge** — OCHRE: ✗ No code | HARES: ✗ "v1" (disabled)
   - **Both lack this**, but HARES has stub

#### GENERATOR
1. **Fuel Consumption Tracking** — Both: ✓ Equal (therms/hour)

2. **Maintenance Mode / Standby Losses** — OCHRE: ✗ | HARES: ✗
   - **Both lack this**

3. **Minimum Operating Power** — OCHRE: ✓ `capacity_min` | HARES: ✗
   - **OCHRE advantage**

---

## DETAILED AREA-BY-AREA ANALYSIS

### 1. BATTERY COMPARISON

#### 1.1 Dispatch Strategies

**OCHRE Capabilities:**
```python
# Generator.py (inherited by Battery)
self.self_consumption_mode = False  # Toggle mode
self.import_limit = 0.0  # Max grid draw
self.export_limit = 0.0  # Max grid export
```

**HARES Capabilities:**
```rust
// battery.rs lines 512–580
self_consumption_enabled: bool,
solar_only_charging: bool,
grid_connected: bool,
power_setpoint_kw: Option<f64>,
soc_target: Option<f64>,
soc_target_min: Option<f64>,
soc_target_max: Option<f64>,
```

**Gap Analysis:**
- OCHRE supports `import_limit` and `export_limit` (inherited from Generator parent class via composition)
- HARES **does NOT have these fields** in Battery struct
- HARES has `solar_only_charging` (OCHRE also has via `charge_solar_only` in Battery.__init__)
- **HARES DEFICIT:** No import/export limiting in Battery module directly
  - **Workaround:** Must be implemented at control layer, not equipment layer
  - **OCHRE ADVANTAGE:** Cleaner encapsulation

#### 1.2 Battery Thermal Management

**OCHRE Implementation:**
```python
# Battery.py lines 22–123
class BatteryThermalModel(OneNodeRCModel):  # Inherits RC from Models
    # Lumped RC: single thermal node
    # Optional zone coupling
thermal_model: BatteryThermalModel = None  # if zone_name specified
```
- Features: Single thermal node, zone coupling, Arrhenius cell capacity derating
- **No cell heater; no temperature derating for discharge power**

**HARES Implementation:**
```rust
// battery.rs lines 551–563, 791–806
cell_thermal_mass_j_per_k: f64,  // 90,000 J/K default
cell_ua_w_per_k: f64,             // 5 W/K default
heater_power_w: f64,              // Resistive cell heater
heater_threshold_c: f64,          // Activation temp
min_discharge_temp_c: f64,        // Hard limit
full_power_temp_c: f64,           // Ramp point
discharge_derate_factor() -> f64  // Linear derating
```

**Thermal Feature Comparison:**

| Feature | OCHRE | HARES | Winner |
|---------|-------|-------|--------|
| Lumped RC thermal model | ✓ Yes | ✓ Yes | Tie |
| Cell heater (resistive) | ✗ No | ✓ Yes (optional, 0–500 W) | **HARES** |
| Discharge power derating | ✗ Implicit via SOC | ✓ Yes (explicit, T-dependent) | **HARES** |
| Charge temp blocking | ✗ Implicit | ✓ Yes (0 °C minimum, plating prevention) | **HARES** |
| Temperature-capacity coupling | ✓ Yes (d0 Arrhenius) | ✓ Yes (identical) | Tie |

**Conclusion:** **HARES significantly superior** in thermal management. Includes heater and explicit temperature derating; OCHRE lacks both.

---

#### 1.3 Degradation Modeling

**OCHRE:**
```python
# Battery.py lines 365–441
# Smith 2017 degradation with 3 states: Q_Li, Q_sei, Q_plating
degradation_states = (q1, q2, q3)  # Updated daily
# Rainflow cycle counting via external library
cycles = rainflow.extract_cycles(df["soc"])
# Arrhenius/Tafel equations for temp/voltage dependence
```

**HARES:**
```rust
// battery.rs lines 194–296
pub struct RainflowCounter { ... }  // Custom ASTM E1049-85 impl
struct DegradationState { ... }     // Smith 2017 (v1 stub)

impl Battery {
    degradation: DegradationState,  // Tracks but not computed (v1)
    rainflow: RainflowCounter,      // Fully functional
}
```

**Degradation Comparison:**

| Feature | OCHRE | HARES | Status |
|---------|-------|-------|--------|
| Smith 2017 model | ✓ Full implementation | ✓ Struct present, logic stubbed | **OCHRE** |
| Rainflow counting | ✓ Yes (external lib) | ✓ Yes (custom ASTM) | Tie (different approach) |
| Temperature coupling | ✓ Yes (Arrhenius) | ✓ Struct, logic stubbed | **OCHRE** |
| Daily update cycle | ✓ Yes | ✓ Checkpointed | Tie |

**Critical Gap:** **HARES has degradation framework but logic is NOT COMPUTED** (lines 1–5 in battery.rs: "degradation tracking (stubbed in v1)"). OCHRE actively computes degradation daily.

---

#### 1.4 Grid Interaction (Import/Export)

**OCHRE Implementation:**
```python
# Generator.py lines 62–90 (inherited by Battery)
self.import_limit = self.parameters.get("import_limit", 0)
self.export_limit = self.parameters.get("export_limit", 0)

# Lines 110–119: Self-consumption dispatch
if self.self_consumption_mode:
    desired_power = max(min(net_power, self.import_limit), -self.export_limit)
    self.power_setpoint = desired_power - net_power
```

**HARES Implementation:**
```rust
// battery.rs: NO import_limit or export_limit fields
// Only in Generator (lines 328, 432, 530)
pub struct Generator {
    import_limit_kw: f64,
    export_limit_kw: f64,
}
```

**Grid Control Comparison:**

| Capability | OCHRE | HARES Battery | HARES Generator | Notes |
|-----------|-------|---------------|--------------------|-------|
| Import limit | ✓ Yes | ✗ **Missing** | ✓ Yes | HARES Battery lacks; Generator has |
| Export limit | ✓ Yes | ✗ **Missing** | ✓ Yes | HARES Battery lacks; Generator has |
| Self-consumption dispatch | ✓ Yes | ✓ Yes | ✓ Yes | Both support |
| Net load clamping | ✓ Yes | ✓ Yes | ✓ Yes | Both support |

**Assessment:** **OCHRE ADVANTAGE.** HARES Battery module is missing explicit grid limits. Generator has them, but Battery should too for consistency.

---

### 2. PV COMPARISON

#### 2.1 Solar Irradiance Modeling Approach

**OCHRE (PV.py lines 9–71):**
```python
def run_sam(capacity, tilt, azimuth, weather, location, ...):
    # Real-time SAM invocation
    system_model = pvwatts.default("PVWattsNone")
    system_model.value("system_capacity", capacity)
    system_model.value("tilt", tilt)
    system_model.value("azimuth", (azimuth + 180) % 360)  # SAM convention
    system_model.execute()
    ac = -pd.Series(system_model.Outputs.ac, index=time) / 1000  # in kW
    return ac
```

**HARES (pv.rs lines 272–350):**
```rust
// Option 1: Parquet LUT lookup (pre-computed SAM)
fn from_parquet(path: &Path) -> Result<Self, HaresError> {
    // 6-D LUT: month, hour, GHI, DNI, DHI, temp
    // Interpolates nearest-neighbor with fallback
}

// Option 2: Analytical fallback (NOCT + gamma)
fn cell_temperature_noct_wind(...) -> f64 {
    // SAM-NOCT with wind correction per NREL PVWatts v8
}
```

**Comparison:**

| Aspect | OCHRE | HARES |
|--------|-------|-------|
| **Speed** | Live SAM (slower) | Pre-computed LUT (fast) |
| **Flexibility** | High (adjusts for any weather) | Medium (fixed LUT dimensions) |
| **Reproducibility** | Low (depends on SAM version) | High (fixed LUT) |
| **Storage** | Minimal (code-based) | High (parquet files) |
| **Physics** | Current SAM v8 | SAM v8 equivalent |

**Critical Gaps (Both Systems):**
- ✗ No soiling losses
- ✗ No snow accumulation/shedding
- ✗ No module degradation (~0.5%/year in reality)
- ✗ No string-level shading (spatial)
- ✗ Single MPPT (unrealistic for >5 kW systems)

**Assessment:** Different approaches, NOT a gap. HARES LUT is actually superior for reproducibility and speed.

---

#### 2.2 Inverter Clipping & Visibility

**OCHRE (PV.py lines 214–248):**
```python
if s > self.inverter_capacity:  # s = sqrt(p^2 + q^2)
    if self.inverter_priority == "Watt":
        p = -min(-p, self.inverter_capacity)  # Clamp P
        # ... reduce Q by available capacity
    elif self.inverter_priority == "Var":
        # Prioritize Q, reduce P
    # Results are implicit; not tracked separately
```

**HARES (pv.rs lines 928–976):**
```rust
// Track curtailment and clipping separately
self.telemetry.set("curtailment_kw", curtailment_kw);
self.telemetry.set("inverter_clipping_kw", inverter_clipping_kw);

// Smart inverter clipping per priority
if self.inverter_priority == "Watt" {
    // Clamp P first, then Q
} else if self.inverter_priority == "Var" {
    // Clamp Q first, then P
}
```

**Telemetry Comparison:**

| Metric | OCHRE | HARES |
|--------|-------|-------|
| Inverter clipping tracked | ✗ Implicit | ✓ Explicit (inverter_clipping_kw) |
| Curtailment tracked | ✗ Implicit | ✓ Explicit (curtailment_kw) |
| Visibility | Low | High |

**Assessment:** **HARES ADVANTAGE.** Explicit clipping/curtailment telemetry vs. implicit in OCHRE.

---

#### 2.3 Multi-Array Support

**OCHRE:**
```python
# PV.py: Single array model
self.capacity = capacity
self.tilt = tilt
self.azimuth = azimuth
# No array indexing or multiple configurations
```

**HARES:**
```rust
// pv.rs lines 28, 120–195, 576, 1132–1142
const KEY_ARRAY_COUNT: &str = "array_count";

pub struct PvArray {
    pub tilt_deg: f64,
    pub azimuth_deg: f64,
    pub capacity_kw: f64,
    pub noct_c: f64,
    pub module_type: ModuleType,
}

pub struct Pv {
    arrays: Vec<PvArray>,  // Support up to 16 arrays
}

fn parse_arrays_from_config(config: &EquipmentConfig) -> Result<Vec<PvArray>, HaresError> {
    let count = ...;  // array_count
    for idx in 0..count {
        arrays.push(PvArray::from_indexed_config(config, idx)?);
    }
}
```

**Multi-Array Comparison:**

| Feature | OCHRE | HARES |
|---------|-------|-------|
| Multiple arrays | ✗ Single only | ✓ Up to 16 different orientations |
| Independent tilt/azimuth | ✗ No | ✓ Yes |
| Independent module type | ✗ No | ✓ Yes (per array) |
| Aggregated output | — | ✓ Yes |

**Assessment:** **HARES ADVANTAGE.** Multi-array support is superior for modeling roofs with multiple orientations.

---

#### 2.4 Wind Correction in NOCT Model

**OCHRE (PV.py lines 9–71, via SAM):**
```python
# SAM PVWatts includes wind in cell temperature:
# T_cell = T_amb + (E / 800) * (NOCT - 20)
# Wind correction applied by SAM internally; not exposed in OCHRE code
```

**HARES (pv.rs lines 51–84):**
```rust
const NOCT_WIND_NUMERATOR: f64 = 9.5;
const NOCT_WIND_CONSTANT: f64 = 5.7;
const NOCT_WIND_COEFFICIENT: f64 = 3.8;

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

**Wind Correction Comparison:**

| Aspect | OCHRE | HARES |
|--------|-------|-------|
| Wind-corrected NOCT | ✓ (via SAM) | ✓ Explicit formula |
| Documented | ✗ Implicit in SAM | ✓ Inline constants (NREL PVWatts v8) |
| Accessibility | Low | High |

**Assessment:** Equivalent functionality; HARES more transparent.

---

### 3. EV COMPARISON

#### 3.1 Charging Levels & Profiles

**OCHRE (EV.py lines 12–16, 58–61):**
```python
EV_MAX_POWER = {
    "Level0": [1.4, 1.4, 1.4, 1.4],  # Test only
    "Level1": [1.4, 1.4, 1.4, 1.4],  # 120V, 1.4 kW
    "Level2": [3.6, 3.6, 7.2, 11.5],  # 240V, 3.6–11.5 kW
}
# No DCFC support
```

**HARES (ev.rs lines 74–95):**
```rust
const L1_CHARGING_POWER_KW: f64 = 1.4;
const L1_MIN_POWER_KW: f64 = 1.0;
const L1_MAX_POWER_KW: f64 = 1.8;
const L2_MIN_POWER_KW: f64 = 3.6;
const L2_MAX_POWER_KW: f64 = 11.5;
// No DCFC support (v1)
```

**Charging Profile Comparison:**

| Level | OCHRE | HARES | Notes |
|-------|-------|-------|-------|
| L1 (120V) | ✓ 1.4 kW | ✓ 1.0–1.8 kW range | HARES has margin |
| L2 (240V) | ✓ 3.6–11.5 kW | ✓ 3.6–11.5 kW | Identical |
| DC Fast (CCS/CHAdeMO) | ✗ **Missing** | ✗ **Missing** | **Critical gap in both** |

**Gap Assessment:** Both lack DCFC, which is increasingly important for modern EV analysis.

---

#### 3.2 Smart Charging Algorithms

**OCHRE (EV.py lines 115–200):**
```python
def generate_events(self, probabilities, event_data, schedule=None, ...):
    # Event-based only: arrival time, SOC, duration from PDF
    # No active charging control beyond power limits
    # All optimization is implicit in charging_level choice

# Options: Max Power, SOC Rate, Max SOC (basic)
```

**HARES (ev.rs lines 45–51, 247–252, 1000–1100+):**
```rust
const KEY_IMMEDIATE_TARGET_SOC: &str = "immediate_target_soc";
const KEY_DELAY_UNTIL_HOUR: &str = "delay_until_hour";
const KEY_TOU_AVOID_PEAK: &str = "tou_avoid_peak";
const KEY_TOU_PEAK_START_HOUR: &str = "tou_peak_start_hour";
const KEY_TOU_PEAK_END_HOUR: &str = "tou_peak_end_hour";
const KEY_READY_BY_HOUR: &str = "ready_by_hour";
const KEY_READY_TARGET_SOC: &str = "ready_target_soc";
```

**Smart Charging Feature Comparison:**

| Feature | OCHRE | HARES | Capability |
|---------|-------|-------|-----------|
| Event-based scheduling | ✓ Yes | ✓ Yes | Both |
| TOU peak avoidance | ✗ No | ✓ Yes | **HARES** |
| Immediate target SOC | ✗ No | ✓ Yes | **HARES** |
| Ready-by-time guarantees | ✗ No | ✓ Yes | **HARES** |
| Delay-until-hour | ✗ No | ✓ Yes | **HARES** |
| Power modulation | ✓ Yes | ✓ Yes | Both |
| Max SOC control | ✓ Yes | ✓ Yes | Both |

**Assessment:** **HARES SIGNIFICANTLY EXCEEDS OCHRE** on smart charging. Includes TOU optimization, immediate targets, and time-of-readiness logic. OCHRE is purely event-based.

---

#### 3.3 Battery Thermal Management (EV-Specific)

**OCHRE (EV.py lines 290–313):**
```python
# No thermal model; no temperature tracking
# Implicit efficiency loss in calculate_power_and_heat()
sensible_gain = ac_power - dc_power  # = power losses
# No heater, no temperature limits
```

**HARES (ev.rs lines 240–250, 1000–1100+):**
```rust
battery_temp_c: f64,
heater_active: bool,
min_charge_temp_c: f64,  // 0 °C default
full_power_temp_c: f64,  // 10 °C default
heater_power_w: f64,     // Resistive heater
thermal_mass_j_per_k: f64,
ua_w_per_k: f64,

fn apply_thermal_derating(&self) -> f64 { ... }
fn apply_heater_if_needed(&self) -> (power_out, heater_w) { ... }
```

**EV Thermal Management Comparison:**

| Feature | OCHRE | HARES | Capability |
|---------|-------|-------|-----------|
| Battery temperature tracking | ✗ No | ✓ Yes | **HARES** |
| Charge temp limits (plating) | ✗ No | ✓ Yes (0 °C) | **HARES** |
| Discharge temp limits | ✗ No | ✓ Yes (-20 °C default) | **HARES** |
| Temperature derating | ✗ No | ✓ Yes (linear) | **HARES** |
| Cell heater (pre-conditioning) | ✗ No | ✓ Yes (optional) | **HARES** |
| Thermal model (RC) | ✗ No | ✓ Yes (lumped) | **HARES** |

**Assessment:** **HARES SIGNIFICANTLY EXCEEDS OCHRE.** OCHRE has no thermal management; HARES includes full RC model, heater, and derating.

---

#### 3.4 Driver Behavior & Archetypes

**OCHRE (EV.py lines 88–113):**
```python
# PDF-based event file: pdf_Veh{1..4}_{Level0,Level1,Level2}.csv
# Conditions: weekday, temperature
# Fixed patterns; no parametric variation
if vehicle_type == "PHEV":
    vehicle_num = 1 if range < 35 else 2  # Binary classification
elif vehicle_type == "BEV":
    vehicle_num = 3 if range < 175 else 4  # Binary classification
```

**HARES (ev.rs lines 21–22, 60–65, 200–300+):**
```rust
mod archetype;  // Contains DriverArchetype enum

pub enum DriverArchetype {
    CommutingWorker,
    ShiftWorker,
    HighMileageDriver,
    ...
}

const KEY_DAILY_DRIVE_MILES_MEAN: &str = "daily_drive_miles_mean";
const KEY_DAILY_DRIVE_MILES_STDDEV: &str = "daily_drive_miles_stddev";
const KEY_SHIFT_ROTATION_DAYS: &str = "shift_rotation_days";
const KEY_SHIFT_ON_DAYS: &str = "shift_on_days";
const KEY_SHIFT_DURATION_FUZZ_MINUTES: &str = "shift_duration_fuzz_minutes";
const KEY_PLUG_IN_POLICY: &str = "plug_in_policy";
```

**Driver Behavior Comparison:**

| Feature | OCHRE | HARES | Capability |
|---------|-------|-------|-----------|
| PDF-based events | ✓ Yes | ✓ Yes | Both |
| Vehicle type classification | ✓ Binary (2 BEV, 2 PHEV) | ✓ Archetype enum | Tie |
| Parametric daily drive miles | ✗ No | ✓ Yes (mean ± stddev) | **HARES** |
| Shift-work patterns | ✗ No | ✓ Yes (multi-day rotation) | **HARES** |
| Selective plug-in policy | ✗ No | ✓ Yes (Always vs. LowSOC) | **HARES** |
| Arrival time fuzz | ✓ Yes | ✓ Yes (parametric) | Tie |

**Assessment:** **HARES SIGNIFICANTLY EXCEEDS OCHRE.** HARES includes parametric driver models, shift-work patterns, and selective plug-in logic.

---

#### 3.5 V2G/V2H Support

**OCHRE (EV.py):**
```python
# No V2G/V2H capability; charging only
# No discharge mode, no bidirectional power
```

**HARES (ev.rs lines 1200–1206):**
```rust
if control_signal.contains_key("V2G Discharge Power (kW)") {
    return Err(HaresError::Control("V2G not supported in v1".to_string()));
}

// V2L infrastructure present (lines 71–72, 188–190):
const KEY_V2L_ENABLED: &str = "v2l_enabled";
const KEY_V2L_SOC_RESERVE: &str = "v2l_soc_reserve";
const KEY_V2L_MAX_DISCHARGE_KW: &str = "v2l_max_discharge_kw";
```

**V2G/V2H/V2L Comparison:**

| Feature | OCHRE | HARES | Status |
|---------|-------|-------|--------|
| V2G framework | ✗ None | ✗ Rejected with error | Both lack implementation |
| V2H framework | ✗ None | ✗ None | Both lack |
| V2L framework | ✗ None | ✓ Struct fields present (disabled v1) | **HARES has stubs** |

**Assessment:** Both lack V2G/V2H. HARES has V2L framework (advantage for future). **Not a current gap; both are v1/incomplete.**

---

### 4. GENERATOR COMPARISON

#### 4.1 Efficiency Models

**OCHRE (Generator.py lines 25–60):**
```python
def __init__(self, efficiency_type="constant", efficiency_file="efficiency_curve.csv", ...):
    if self.efficiency_type == "curve":
        df = self.initialize_parameters(efficiency_file, ...)
        self.efficiency_curve = interp1d(df.index, df["Efficiency Ratio"])
    # Quadratic: eff = rated * (-0.5 * cr² + 1.5 * cr)
    # Bug at line 169: return min(eff, 0.001)  # should be max()
```

**HARES (generator.rs lines 99–145):**
```rust
pub enum EfficiencyModel {
    Constant { rated: f64 },
    Curve { rated: f64, points: Vec<(f64, f64)> },  // Up to 16 points
    Quadratic { rated: f64 },
}

impl EfficiencyModel {
    pub fn evaluate(&self, capacity_ratio: f64) -> f64 {
        match self {
            Self::Constant { rated } => *rated,
            Self::Curve { rated, points } => {
                let eff_ratio = Self::interpolate_curve(points, cr);
                rated * eff_ratio
            }
            Self::Quadratic { rated } => {
                let eff = rated * (-0.5 * cr * cr + 1.5 * cr);
                eff.max(0.001)  // FIXED: max instead of min
            }
        }
    }
}
```

**Efficiency Model Comparison:**

| Feature | OCHRE | HARES | Status |
|---------|-------|-------|--------|
| Constant efficiency | ✓ Yes | ✓ Yes | Both |
| Piecewise-linear curve | ✓ Yes (unlimited points) | ✓ Yes (up to 16 points) | Both |
| Quadratic Vishwanathan | ✓ Yes | ✓ Yes | Both |
| Efficiency clamping bug | ✗ **BUG: min() instead of max()** | ✓ **FIXED: max()** | **HARES ADVANTAGE** |

**Assessment:** OCHRE has a subtle but critical bug (min vs max clamp). HARES fixes it.

---

#### 4.2 Ramp Rate

**OCHRE (Generator.py lines 46–47):**
```python
self.ramp_rate = self.parameters.get("ramp_rate")  # in kW/min
self._ramp_minutes = self._dt_minutes if self.ramp_rate is not None else None
# Limits power increase only (line 129): ramp_rate only impacts generating power
min_power = max(min_power, self.electric_kw - self.ramp_rate * self._ramp_minutes)
```

**HARES (generator.rs lines 65–70, 452–479):**
```rust
const DEFAULT_DELTA_KW_PER_S: f64 = 1.0;  // kW/s, not kW/min

delta_kw_per_s: f64,  // Fast: 1 kW/s = 10 s to 10 kW

// Ramp-rate constraint (line 468):
// Only limits power increases; decreases are instant (matching OCHRE)
if target_kw > self.last_power_kw {
    max_power = self.last_power_kw + self.delta_kw_per_s * dt_s;
}
```

**Ramp Rate Comparison:**

| Aspect | OCHRE | HARES | Assessment |
|--------|-------|-------|-----------|
| Units | kW/min (slow) | kW/s (fine-grained) | **HARES more reasonable** |
| Default 10 kW unit | 0.1 kW/min ≈ 100 min ramp | 1.0 kW/s ≈ 10 s ramp | **HARES default is realistic** |
| Ramp-down constraint | ✗ No | ✗ No | Both instant shutdown |
| Soft-start | ✗ No | ✗ No | Neither has it |

**Assessment:** HARES has more sensible default (kW/s vs kW/min). 10 s ramp for a 10 kW unit is realistic; 100 min is not.

---

#### 4.3 Minimum Operating Power

**OCHRE (Generator.py lines 42–44, 136–141):**
```python
self.capacity_min = self.parameters.get("capacity_min")  # in kW

# Generator power limits (lines 136–141):
if self.allow_consumption:
    max_power = self.capacity
elif self.capacity_min is not None:
    max_power = -self.capacity_min  # Min power to prevent cycling
else:
    max_power = 0
```

**HARES (generator.rs):**
```rust
// NO minimum operating power limit in HARES Generator
// Lines 328–366: no capacity_min equivalent
pub struct Generator {
    rated_power_kw: f64,
    // No capacity_min field
}
```

**Minimum Power Comparison:**

| Feature | OCHRE | HARES |
|---------|-------|-------|
| Minimum operating power | ✓ Yes (capacity_min) | ✗ **Missing** |
| Purpose | Prevent inefficient cycling | — |
| Impact | Cleaner dispatch in self-consumption mode | Generator may cycle excessively |

**Assessment:** **OCHRE ADVANTAGE.** HARES lacks minimum power threshold. Could lead to excessive on/off cycling in low-load conditions.

---

#### 4.4 Import/Export Limits

**OCHRE (Generator.py lines 62–90):**
```python
self.import_limit = self.parameters.get("import_limit", 0)
self.export_limit = self.parameters.get("export_limit", 0)

# Self-consumption (lines 110–119):
desired_power = max(min(net_power, self.import_limit), -self.export_limit)
```

**HARES (generator.rs lines 44–45, 328, 432, 487–492):**
```rust
const KEY_GRID_IMPORT_LIMIT_KW: &str = "grid_import_limit_kw";
const KEY_EXPORT_LIMIT_KW: &str = "export_limit_kw";

pub struct Generator {
    grid_import_limit_kw: f64,
    export_limit_kw: f64,
}

// Self-consumption (line 487–492):
// desired_import = clamp(net_load, -export_limit, import_limit)
```

**Grid Control Comparison:**

| Feature | OCHRE | HARES |
|---------|-------|-------|
| Import limit | ✓ Yes | ✓ Yes |
| Export limit | ✓ Yes | ✓ Yes |
| Default export (islanding) | 0.0 (no export) | 0.0 (no export) | Both same |

**Assessment:** Equivalent.

---

#### 4.5 CHP Thermal Ports

**OCHRE (Generator.py lines 190–201, Equipment.py):**
```python
# CHP parameters present (lines 53, 190–195)
self.efficiency_chp = self.parameters.get("efficiency_chp", 0)
self.power_chp = self.power_input * self.efficiency_chp

# But: "TODO: add battery node to envelope model"
# Thermal ports are STUBBED: return None from thermal methods
```

**HARES (generator.rs lines 23–27, 320, 568–580):**
```rust
pub struct Generator {
    // Fluid loop for CHP
    loop_id: Option<LoopId>,
    flow_rate_kg_s: f64,
    supply_temp_c: f64,
    return_temp_c: f64,
}

// Full thermal port declaration (lines 568–580):
if self.descriptor.stage == ExecutionStage::Thermal {
    ports.push(PortDeclaration { port_type: PortType::Thermal, ... });
    ports.push(PortDeclaration { port_type: PortType::Fluid, ... });
}
```

**CHP Comparison:**

| Feature | OCHRE | HARES |
|---------|-------|-------|
| Thermal efficiency parameter | ✓ Yes (eta_chp) | ✓ Yes (eta_thermal) |
| Thermal power calculation | ✓ Yes | ✓ Yes |
| Thermal port to zone | ✗ Stubbed | ✓ Full implementation |
| Fluid loop (CHP jacket water) | ✗ Stubbed | ✓ Full implementation |

**Assessment:** **HARES ADVANTAGE.** HARES fully implements CHP thermal and fluid ports; OCHRE has them stubbed.

---

## SUMMARY TABLE: Feature Coverage

### BATTERY
| Feature | OCHRE | HARES | Winner | Gap? |
|---------|-------|-------|--------|------|
| SOC control (target, limits) | ✓ | ✓ | Tie | No |
| Power setpoint | ✓ | ✓ | Tie | No |
| Self-consumption dispatch | ✓ | ✓ | Tie | No |
| **Import/export grid limits** | ✓ | ✗ | **OCHRE** | **YES** |
| Thermal model (RC) | ✓ | ✓ | Tie | No |
| **Temperature derating** | ✗ | ✓ | **HARES** | No (HARES advantage) |
| **Cell heater** | ✗ | ✓ | **HARES** | No (HARES advantage) |
| Degradation (Smith 2017) | ✓ Full | ✓ Stubbed | **OCHRE** | **YES** |
| Rainflow counting | ✓ | ✓ | Tie | No |

### PV
| Feature | OCHRE | HARES | Winner | Gap? |
|---------|-------|-------|--------|------|
| Solar irradiance modeling | ✓ SAM | ✓ LUT | Different | No |
| Inverter clipping tracking | ✗ | ✓ | **HARES** | No (HARES advantage) |
| **Multi-array support** | ✗ | ✓ | **HARES** | No (HARES advantage) |
| Temperature coefficient | ✓ | ✓ | Tie | No |
| **Wind-corrected NOCT** | ✗ Implicit | ✓ Explicit | **HARES** | No (HARES advantage) |
| Multi-MPPT | ✗ | ✗ | Both lack | YES (both) |
| Soiling losses | ✗ | ✗ | Both lack | YES (both) |
| Module degradation | ✗ | ✗ | Both lack | YES (both) |
| Snow losses | ✗ | ✗ | Both lack | YES (both) |

### EV
| Feature | OCHRE | HARES | Winner | Gap? |
|---------|-------|-------|--------|------|
| L1/L2 charging | ✓ | ✓ | Tie | No |
| **Smart charging (TOU, etc)** | ✗ | ✓ | **HARES** | No (HARES advantage) |
| **Temperature management** | ✗ | ✓ | **HARES** | No (HARES advantage) |
| **Cell heater (preconditioning)** | ✗ | ✓ | **HARES** | No (HARES advantage) |
| Event-based scheduling | ✓ | ✓ | Tie | No |
| **Driver archetypes** | ✓ Basic | ✓ Rich | **HARES** | No (HARES advantage) |
| DCFC (CCS/CHAdeMO) | ✗ | ✗ | Both lack | YES (both) |
| V2G/V2H | ✗ | ✗ V1 | Both lack | YES (both) |

### GENERATOR
| Feature | OCHRE | HARES | Winner | Gap? |
|---------|-------|-------|--------|------|
| Constant efficiency | ✓ | ✓ | Tie | No |
| Curve efficiency | ✓ | ✓ | Tie | No |
| Quadratic efficiency | ✓ | ✓ FIXED | **HARES** (bug fix) | No |
| Ramp rate (kW/min vs kW/s) | ✓ | ✓ BETTER | **HARES** | No (HARES advantage) |
| **Minimum operating power** | ✓ | ✗ | **OCHRE** | **YES** |
| Import/export limits | ✓ | ✓ | Tie | No |
| CHP thermal ports | ✗ Stubbed | ✓ Full | **HARES** | No (HARES advantage) |
| Fuel consumption tracking | ✓ | ✓ | Tie | No |

---

## CRITICAL GAPS RANKED BY IMPACT

### Both Systems Lack (Tier 1: Critical)
1. **Time-of-Use (TOU) Dispatch** — No tariff-aware optimization for Battery/EV
   - **Impact:** Cannot model real-world utility pricing

2. **DC Fast Charging (DCFC)** — EV limited to 11.5 kW max
   - **Impact:** Cannot simulate modern EV road-trip behavior

3. **V2G/V2H Support** — No bidirectional discharge
   - **Impact:** Cannot model emergency backup or grid support

4. **PV Soiling/Snow/Degradation** — No long-term seasonal losses
   - **Impact:** Overestimates annual PV generation by 5–15%

### Only HARES Lacks (Tier 2: Important)
1. **Battery Import/Export Grid Limits** — Only in Generator, not Battery
   - **Impact:** Grid constraints must be enforced externally; less encapsulated control
   - **Workaround:** Possible via control signals, but awkward

2. **Generator Minimum Operating Power** — Could cause excessive cycling
   - **Impact:** Unrealistic dispatch in low-load conditions
   - **Workaround:** Manual control signal override

3. **Battery Degradation Logic** — Struct present, but compute is stubbed
   - **Impact:** Cannot track battery aging; capacity assumed constant
   - **Workaround:** None; requires implementation

### Only OCHRE Lacks (Tier 3: Desirable)
1. **EV Smart Charging** — Event-based only; no TOU or time-of-readiness
   - **Impact:** Cannot model adaptive charging strategies
   - **HARES mitigates this**

2. **EV Thermal Management** — No temperature limits or heater
   - **Impact:** Cannot model cold-weather charging restrictions
   - **HARES mitigates this**

3. **PV Multi-Array Support** — Single orientation only
   - **Impact:** Cannot model roofs with mixed orientations
   - **HARES mitigates this**

---

## OVERALL ASSESSMENT

### Advantages HARES Has Over OCHRE
1. **EV thermal management** (full RC model + heater + derating)
2. **EV smart charging** (TOU, immediate targets, time-of-readiness)
3. **Battery thermal management** (cell heater + discharge derating)
4. **PV multi-array support** (up to 16 independent arrays)
5. **PV clipping/curtailment visibility** (explicit telemetry)
6. **Generator efficiency bug fix** (min → max clamping)
7. **Generator ramp rate realism** (kW/s vs. kW/min)
8. **CHP thermal port implementation** (full vs. stubbed)

### Advantages OCHRE Has Over HARES
1. **Battery grid interaction** (import/export limits in Battery struct)
2. **Generator minimum operating power** (capacity_min parameter)
3. **Battery degradation** (full Smith 2017 implementation; HARES stubbed)
4. **PV real-time SAM integration** (vs. HARES LUT; different trade-offs)

### Equivalent Areas
- Basic SOC tracking, power setpoint, self-consumption
- Rainflow cycle counting (different implementation, same result)
- Ramp rate limiting (OCHRE slow, HARES better)

---

## Recommended Actions for HARES

**PRIORITY 1: Fix Critical Gaps**
1. Implement Battery import/export grid limits (move from Generator inheritance)
2. Complete Battery degradation computation (Smith 2017 + Arrhenius)
3. Add Generator minimum operating power threshold

**PRIORITY 2: Add Advanced Features**
1. TOU dispatch modes (time-varying electricity rates)
2. DCFC charging support for EV (CCS, CHAdeMO, 50–350 kW)
3. V2G/V2H discharge logic (currently stubbed)
4. PV soiling, snow, and module degradation

**PRIORITY 3: Polish & Realism**
1. Inverter efficiency curves (load-dependent)
2. Multi-MPPT for PV arrays
3. Demand charge minimization logic
4. Cell balancing in Battery model

