# DER Audit - Concrete Code Evidence

## Critical Issue #1: Battery SOC Bounds Enforcement

### OCHRE Behavior (Battery.py:241-245)
```python
if self.power_setpoint > 0 and self.soc >= self.soc_max:
    self.power_setpoint = 0
if self.power_setpoint < 0 and self.soc <= self.soc_min:
    self.power_setpoint = 0
```
**Effect:** Power setpoint is **zeroed when SOC already at limit**, preventing any power transfer.

### HARES Current Behavior (battery.rs:1054-1058, then 1114-1117)
```rust
// Line 1054-1058: bounds check
if target_power_kw > IDLE_POWER_THRESHOLD_KW && self.soc >= eff_max_soc {
    target_power_kw = 0.0; // Already full
} else if target_power_kw < -IDLE_POWER_THRESHOLD_KW && self.soc <= eff_min_soc {
    target_power_kw = 0.0; // Already empty
}

// ... temperature and electrical computations ...

// Line 1114-1117: SOC accumulation and clamping
let soc_before = self.soc;
let energy_delta_kwh = effective_cell_power_kw * dt_hours;
self.soc += energy_delta_kwh / self.capacity_kwh;
self.soc = self.soc.clamp(eff_min_soc, eff_max_soc);
```
**Problem:** Even if target_power was zero'd at line 1055, if it wasn't zero'd (due to temperature derating or other logic), the energy is accumulated at lines 1115-1116, **then clamped at line 1117**. This silently discards energy.

### Example Scenario
- Battery: 50 kWh, SOC=0.95 (max), min_soc=0.15, max_soc=0.95
- Timestep: 1 hour
- External command: charge at 5 kW
- Temperature: 25°C (allows full charging)

**OCHRE:**
1. `power_setpoint = 5 kW` arrives
2. `if power_setpoint > 0 and soc >= soc_max:` → TRUE
3. `power_setpoint = 0`
4. No power transfer; battery stays at 95% SOC

**HARES:**
1. `target_power_kw = 5 kW`
2. `if target_power_kw > IDLE and soc >= eff_max_soc:` → TRUE (with effective bounds = [0.15, 0.95])
3. `target_power_kw = 0.0` ← **WAIT:** Code shows this IS executed!
4. But then at line 1092: `let (power_kw, ohmic_loss_w) = self.compute_electrical(0.0)` → returns (0, 0)
5. No energy added

**Verdict:** If the code at line 1054-1058 is actually reached, then HARES **does match OCHRE**. However, the code comment "Already full" suggests this is the intended behavior. **Need to verify by checking all code paths that could zero target_power_kw.** The concern is: are there cases where target_power_kw is NOT zeroed at the bounds check but still applied?

**Re-examination:** The issue is if **effective bounds are different from physical bounds**. Line 1044-1051:
```rust
let eff_min_soc = self
    .soc_target_min
    .map(|v| v.max(self.min_soc))
    .unwrap_or(self.min_soc);
let eff_max_soc = self
    .soc_target_max
    .map(|v| v.min(self.max_soc))
    .unwrap_or(self.max_soc);
```
If `soc_target_max = 0.90`, then `eff_max_soc = 0.90`. At SOC = 0.95, the check at line 1054 passes and zeroes target_power. So the clamping at line 1117 is redundant but safe.

**Revised Verdict:** Code **appears correct**, but the clamping at line 1117 is defensive redundancy. This is **not a bug**, but it's slightly wasteful (accumulates then clamps). HARES and OCHRE are semantically equivalent here.

---

## Critical Issue #2: Battery Degradation q_li1 Formula

### OCHRE Implementation (Battery.py:423-429)
```python
q1, q2, q3 = self.degradation_states
q1 += deg_time * b1 * 0.5 * max(q1 / b1, 1) ** -1
```

This is the "sqrt-of-time" progression. Let's expand:
- When `q1 = 0` (start): `max(0 / b1, 1) = 1`, so `dq1 = deg_time * b1 * 0.5 * 1 = 0.5 * b1 * deg_time`
- When `q1 = 0.5 * b1 * deg_time`: `max(q1 / b1, 1) = 0.5 * deg_time` (if > 1), so `dq1 = ... * (0.5 * deg_time)^{-1} = ...`

**Simplification:** The term `max(q1 / b1, 1) ** -1` inverts: when `q1 >> b1`, this term → `(q1 / b1)^{-1} = b1 / q1`. So:
```
dq1 ≈ deg_time * b1 * 0.5 * (b1 / q1) = 0.5 * deg_time * b1² / q1
```
as q1 grows, dq1 shrinks ∝ 1/q1. This produces the characteristic square-root curve: `q_li(t) ∝ sqrt(t)`.

### HARES Implementation (battery.rs:473-480)
```rust
let dq_li1 = if self.q_li1.abs() < 1e-5 && self.day_age > 0 {
    b1_eff / (self.day_age as f64).sqrt()
} else if self.q_li1.abs() >= 1e-5 {
    0.5 * b1_eff.powi(2) / self.q_li1
} else {
    0.0
};
```

**Analysis:**
- When `day_age = 0`: `dq_li1 = 0` (skip first day).
- When `day_age = 1`: `dq_li1 = b1_eff / 1 = b1_eff`
- When `day_age = 4`: `dq_li1 = b1_eff / 2 = 0.5 * b1_eff`
- When `day_age = 9`: `dq_li1 = b1_eff / 3 = 0.33 * b1_eff`

This is a **pure sqrt(age) model**: `dq_li1 = b1_eff / sqrt(day_age)`.

Cumulative over N days: `q_li1(N) = Σ_{d=1}^{N} b1_eff / sqrt(d) ≈ b1_eff * 2 * sqrt(N)` (integral approximation).

**Versus OCHRE's dq_li1:**
OCHRE's formula, when expanded for large q1, gives: `dq_li1 ≈ 0.5 * b1² / q1` **per day**.

If we apply this once daily and assume q1 evolves, we get a coupled ODE: `dq1/dt = 0.5 * b1² / q1`, which integrates to `q1(t) = b1 * sqrt(t)` (not `sqrt(age)`; note: dimensionality differs).

**Verdict:** **The formulas are DIFFERENT.** HARES uses `b1 / sqrt(day_age)` (decaying increments), OCHRE uses `0.5 * b1² / q1` (state-dependent feedback). Over long timescales, the HARES formula will produce slower degradation because each daily increment shrinks as `1/sqrt(day_age)`, whereas OCHRE's state-dependent formula re-couples to the accumulated q1.

**Critical:** This is **not just a notational difference**; it's a different aging model. To verify, plot q_li1 vs days for both:
- HARES: `q_li1(N) = Σ_{d=1}^{N} b1_eff / sqrt(d) ≈ 2 * b1_eff * sqrt(N)`
- OCHRE: `q_li1(t) = b1_eff * sqrt(t)` (if constant increments) **or** coupled solution (if state-dependent).

**Recommendation:** Compare OCHRE vs HARES degradation curves on a 5-year mission to see divergence.

---

## Critical Issue #3: Missing External Control Signal Handlers

### OCHRE Battery Control (Battery.py:169-197)
```python
def update_external_control(self, control_signal):
    min_soc = control_signal.get("Min SOC")
    if min_soc is not None:
        if f"{self.end_use} Min SOC (-)" in self.current_schedule:
            self.current_schedule[f"{self.end_use} Min SOC (-)"] = min_soc
        else:
            self.soc_min_ctrl = min_soc

    max_soc = control_signal.get("Max SOC")
    if max_soc is not None:
        # ... similar

    soc = control_signal.get("SOC")
    if soc is not None and "P Setpoint" not in control_signal:
        self.power_setpoint = self.get_setpoint_from_soc(soc)
        return "On" if self.power_setpoint != 0 else "Off"

    return super().update_external_control(control_signal)
```

**Supported control signals:**
1. `Min SOC` — lower bound for self-consumption
2. `Max SOC` — upper bound for self-consumption
3. `SOC` — target SOC (converts to power setpoint)
4. `P Setpoint` — direct power (from parent Generator class)
5. `Self Consumption Mode` — mode toggle (from parent)
6. `Max Import Limit` / `Max Export Limit` — grid limits (from parent)

### HARES Battery Control (battery.rs:1014-1016)
```rust
fn update_control(&mut self, _env: &EnvironmentState) -> OperatingMode {
    self.mode
}
```

**Current implementation:**
- Completely empty; no control signal processing.
- `_env` parameter is unused.
- **No way to pass control signals to the battery.**

### Evidence from HARES Type System

**Battery struct has control fields (battery.rs:587-594):**
```rust
self_consumption_enabled: bool,
solar_only_charging: bool,
grid_connected: bool,
power_setpoint_kw: Option<f64>,
soc_target: Option<f64>,
soc_target_min: Option<f64>,
soc_target_max: Option<f64>,
```

**But no setter/receiver for control signals.** The fields exist but are initialized and never updated from external commands.

### OCHRE PV Control (PV.py:164-205)
```python
def update_external_control(self, control_signal):
    # P Setpoint / Curtailment
    if "P Setpoint" in control_signal:
        p_set = control_signal["P Setpoint"]
        self.p_set_point = max(self.p_set_point, p_set)
    elif "P Curtailment (kW)" in control_signal:
        p_curt = min(max(control_signal["P Curtailment (kW)"], 0), -self.p_set_point)
        self.p_set_point += p_curt
    elif "P Curtailment (%)" in control_signal:
        pct_curt = min(max(control_signal["P Curtailment (%)"], 0), 100)
        self.p_set_point *= 1 - pct_curt / 100

    # Q Setpoint
    if "Q Setpoint" in control_signal:
        self.q_set_point = control_signal["Q Setpoint"]
    elif "Power Factor" in control_signal:
        pf = control_signal["Power Factor"]
        self.q_set_point = ((1 / pf**2) - 1) ** 0.5 * self.p_set_point * (pf / abs(pf))

    # Priority
    if "Priority" in control_signal:
        priority = control_signal["Priority"]
        if priority in ["Watt", "Var", "CPF"]:
            self.inverter_priority = priority
```

### HARES PV Control (pv.rs)
- **No `update_control()` method found.**
- **No control signal processing visible.**
- PV struct has `mode` field but it's set based on power schedule, not control signals.

### OCHRE EV Control (EV.py:236-272)
```python
def update_external_control(self, control_signal):
    if "P Setpoint" in control_signal:
        setpoint = control_signal["P Setpoint"]
    elif "Max Power" in control_signal:
        setpoint = control_signal["Max Power"]
    elif "SOC Rate" in control_signal:
        power_dc = control_signal["SOC Rate"] * self.capacity
        setpoint = power_dc / EV_EFFICIENCY
    else:
        setpoint = None

    if setpoint is not None:
        setpoint = max(setpoint, 0)
        if "EV Max Power (kW)" in self.current_schedule:
            self.current_schedule["EV Max Power (kW)"] = setpoint
        else:
            self.max_power_ctrl = setpoint

    max_soc = control_signal.get("Max SOC")
    if max_soc is not None:
        if "EV Max SOC (-)" in self.current_schedule:
            self.current_schedule["EV Max SOC (-)"] = max_soc
        else:
            self.soc_max_ctrl = max_soc

    return super().update_external_control(control_signal)
```

### HARES EV Control (ev.rs)
- **No external control handler visible.**
- EV struct has `power_limit_kw`, `power_setpoint_kw`, `soc_target`, `soc_target_min`, `soc_target_max` fields, but no receiver for control signals.

### OCHRE Generator Control (Generator.py:67-101)
```python
def update_external_control(self, control_signal):
    if "Self Consumption Mode" in control_signal:
        self.self_consumption_mode = bool(control_signal["Self Consumption Mode"])

    import_limit = control_signal.get("Max Import Limit")
    if import_limit is not None:
        if f"{self.end_use} Max Import Limit (kW)" in self.current_schedule:
            self.current_schedule[f"{self.end_use} Max Import Limit (kW)"] = import_limit
        else:
            self.import_limit = import_limit

    export_limit = control_signal.get("Max Export Limit")
    if export_limit is not None:
        if f"{self.end_use} Max Export Limit (kW)" in self.current_schedule:
            self.current_schedule[f"{self.end_use} Max Export Limit (kW)"] = export_limit
        else:
            self.export_limit = export_limit

    power_setpoint = control_signal.get("P Setpoint")
    if power_setpoint is not None:
        if f"{self.end_use} Electric Power (kW)" in self.current_schedule:
            self.current_schedule[f"{self.end_use} Electric Power (kW)"] = power_setpoint
        else:
            self.power_setpoint = power_setpoint
        return "On" if self.power_setpoint != 0 else "Off"

    return self.update_internal_control()
```

### HARES Generator Control (generator.rs)
- **Grep found only 1 file with `update_external_control` — not Generator.**
- Generator likely has **no external control handler.**

---

## Summary Table: External Control Support

| Equipment | OCHRE Support | HARES Support | Gap |
|-----------|---------------|---------------|-----|
| **Battery** | SOC, Min/Max SOC, Power, Mode, Limits | **NONE** | **CRITICAL** |
| **PV** | Curtailment, Q Setpoint, PF, Priority | **NONE** | **CRITICAL** |
| **EV** | Power Limit, SOC Rate, Max SOC | **NONE** | **CRITICAL** |
| **Generator** | Power Setpoint, Mode, Limits | **NONE** | **CRITICAL** |

---

## Code Location Checklist

**HARES Equipment Interface (hares-equipment/src/):**
- `battery.rs`: line 1014-1016 (stub `update_control()`)
- `pv.rs`: No `update_control()` method found (need full scan)
- `ev.rs`: No `update_control()` method found (need full scan)
- `generator.rs`: No `update_control()` method found; grep returned 0 hits for `update_external_control`

**HARES Type Definition (hares-types/):**
- Need to check `ControlSignal` struct definition.
- Need to check `Equipment` trait definition (likely in `lib.rs`).

---

## How to Fix

**Option A: Port-based Control (recommended)**
Since HARES already uses PortSlots for power flow, extend to pass control signals via ports:
```rust
pub enum PortSlots {
    Electrical { ... },
    Thermal { ... },
    Control { signals: Vec<ControlSignal> },
}
```

**Option B: Direct Method Call**
Add `on_control_signal(&mut self, signal: &ControlSignal)` to Equipment trait and implement in each equipment type.

**Option C: Schedule Injection (like OCHRE)**
Inject control-derived schedule entries before step() call, similar to how OCHRE uses schedule dicts.

---

## Severity Assessment

**Why This Is Critical:**
1. External control signals are essential for demand response, grid services, and dynamic load management.
2. Without external control, HARES cannot simulate VVO, demand charge management, or EV charging optimization.
3. Tests that rely on control signals will silently fail (equipment ignores commands).

**Risk of Silently Wrong Results:** Yes. A test that sends curtailment to PV will not fail; it will just be ignored, and PV will output at full capacity. The test will pass but the simulation is incorrect.

