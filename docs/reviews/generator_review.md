# Generator Equipment Review

**Reviewer**: Code Review Agent  
**Date**: 2026-03-31  
**Scope**: Generator equipment (Gas Generator, Gas Fuel Cell) in HARES  

---

## 1. HPXML Parsing

### 1.1 Parsed Attributes

The HPXML parser in `crates/hares-io/src/hpxml/resolve_der.rs` extracts the following attributes from generator elements:

| HPXML Field | HARES Config Field | Notes |
|-------------|-------------------|-------|
| `FuelType` | `fuel_type` | Mapped via `parse_fuel()` - supports electricity, natural gas, propane, oil |
| `ElectricalPowerOutput` | `rated_power_kw` | Falls back to 10.0 kW if not provided |
| `AnnualOutputkWh` | Used to compute | Combined with `AnnualConsumptionkBtu` to derive efficiency |
| `AnnualConsumptionkBtu` | Used to compute | Converted to kWh via `0.29307107` kBtu/kWh factor |

**Efficiency Calculation** (lines 198-209 in `resolve_der.rs`):
```rust
let eta_electric = if let (Some(out_kwh), Some(cons_kbtu)) = (annual_output_kwh, annual_consumption_kbtu) {
    let cons_kwh = cons_kbtu * KBTU_TO_KWH;
    if cons_kwh > 0.0 {
        Some((out_kwh / cons_kwh).clamp(0.0, 1.0))
    } else {
        None
    }
} else {
    None
};
```

### 1.2 Missing HPXML Attributes

The following HPXML generator attributes are **NOT parsed**:

| HPXML Field | HARES Config | Severity | Notes |
|-------------|--------------|----------|-------|
| `NumberOfUnits` | Not parsed | Low | Would allow specifying multiple generators |
| `Manufacturer` | Not parsed | Low | Metadata, not physics-relevant |
| `ModelNumber` | Not parsed | Low | Metadata, not physics-relevant |
| `YearInstalled` | Not parsed | Low | Could affect degradation modeling |
| `GeneratorRatedOutput` | Not parsed | Low | Alternative to ElectricalPowerOutput |

### 1.3 HPXML Parsing Issues

**Issue 1.1: No FuelCell HPXML support** (Severity: Medium)
- HPXML parser always creates "Gas Generator" equipment type, never "Gas Fuel Cell"
- Fuel cells have different default efficiency curves (curve vs constant)
- No way to specify fuel cell via HPXML

**Issue 1.2: No standby power parsing** (Severity: Medium)  
- HPXML has no standby power field, but HARES `GeneratorConfig` has no standby_power_w field either
- OCHRE Generator.py does not model standby power either - this may be acceptable

---

## 2. OCHRE Defaults/Fallbacks

### 2.1 HARES Defaults (from `generator.rs`)

| Config Field | HARES Default | OCHRE Equivalent |
|--------------|---------------|------------------|
| `rated_power_kw` | 10.0 | capacity = 10 (kw) |
| `eta_electric` | 0.30 | efficiency_rated = 0.30 |
| `eta_thermal` | 0.0 | efficiency_chp = 0 |
| `efficiency_type` | "constant" (GasGenerator), "curve" (FuelCell) | efficiency_type = "constant" / "curve" |
| `delta_kw_per_s` | 1.0 | ramp_rate = None (unlimited) |
| `grid_import_limit_kw` | 0.0 | import_limit = 0 |
| `export_limit_kw` | 0.0 | export_limit = 0 |
| `capacity_min_kw` | None | capacity_min = None |
| `flow_rate_kg_s` | 0.1 | Not in OCHRE (CHP stub) |
| `supply_temp_c` | 70.0 | Not in OCHRE (CHP stub) |
| `return_temp_c` | 60.0 | Not in OCHRE (CHP stub) |

### 2.2 Efficiency Curve Defaults

**OCHRE default curve** (Generator.py line 57-58):
```python
# Load efficiency curve
df = self.initialize_parameters(efficiency_file, name_col="Capacity Ratio", value_col=None)
self.efficiency_curve = interp1d(df.index, df["Efficiency Ratio"])
```

**HARES default curve** (generator.rs line 366-368):
```rust
fn default_curve_points() -> Vec<(f4, f4)> {
    // OCHRE default curve: (0,0), (0.5,1), (1,1)
    vec![(0.0, 0.0), (0.5, 1.0), (1.0, 1.0)]
}
```

**Issue 2.1: HARES uses different default efficiency curve interpolation** (Severity: Low)
- OCHRE uses scipy.interpolate.interp1d (linear interpolation)
- HARES uses custom piecewise-linear interpolation
- Tests show they produce the same results (test line 1221-1237), but the implementation differs

### 2.3 Fallback Logic

- If `eta_electric` is not specified, HARES defaults to 0.30 (matches OCHRE)
- If `efficiency_type` is not specified:
  - GasGenerator → "constant"
  - FuelCell → "curve"
- If `efficiency_curve_points` not provided with "curve" type → uses OCHRE default points

---

## 3. HARES Wiring

### 3.1 Configuration Flow

```
HPXML Generator Element
    │
    ▼
resolve_generators() [resolve_der.rs:185-237]
    │
    ├── parse_fuel(FuelType)
    ├── child_f64(ElectricalPowerOutput) → rated_power_kw
    ├── annual_* → eta_electric calculation
    │
    ▼
build_typed_spec() [equipment.rs]
    │
    ▼
GeneratorConfig (typed)
    │
    ▼
EquipmentRegistry.create("Gas Generator", config)
    │
    ▼
Generator::new(config, GeneratorKind)
```

### 3.2 Config Mapping Completeness

All GeneratorConfig fields are properly mapped from HPXML except:

| Config Field | HPXML Source | Status |
|--------------|--------------|--------|
| equipment_id | SystemIdentifier/@id | Not parsed |
| zone_id | Not in HPXML | Not parsed |
| fuel_type | FuelType | ✓ Parsed |
| rated_power_kw | ElectricalPowerOutput | ✓ Parsed (with fallback) |
| eta_electric | AnnualOutput/AnnualConsumption | ✓ Parsed (computed) |
| eta_thermal | Not in HPXML | Not parsed (default 0) |
| efficiency_type | Not in HPXML | Not parsed (default per kind) |
| efficiency_curve_points | Not in HPXML | Not parsed |
| delta_kw_per_s | Not in HPXML | Not parsed |
| capacity_min_kw | Not in HPXML | Not parsed |
| grid_import_limit_kw | Not in HPXML | Not parsed |
| export_limit_kw | Not in HPXML | Not parsed |

### 3.3 Wiring Issues

**Issue 3.1: equipment_id not extracted from HPXML** (Severity: Medium)
- HPXML `SystemIdentifier/@id` is available but not parsed
- GeneratorConfig has `equipment_id: Option<u32>` but it's always None from HPXML

---

## 4. Control Logic

### 4.1 Supported Control Signals

HARES Generator supports three control signal types:

1. **PowerSetpoint** (`ControlSignal::PowerSetpoint`)
   - Sets explicit power output target in kW
   - Value persists across timesteps (no need to resend each step)
   - Clamped to [0, rated_power_kw]

2. **ModeOverride** (`ControlSignal::ModeOverride`)
   - `OperatingMode::Off` - forces generator off (sets setpoint to 0)
   - `OperatingMode::Standby` - returns to self-consumption control

3. **SelfConsumption** (`ControlSignal::SelfConsumption`)
   - `enabled: bool` - toggles self-consumption mode on/off
   - When disabled, generator stays off unless PowerSetpoint is active

### 4.2 Self-Consumption Control Algorithm

HARES implements OCHRE's self-consumption controller (generator.rs lines 604-631):

```rust
// Self-consumption: match OCHRE formula exactly.
// desired_import = clamp(net_load, -export_limit, import_limit)
// target = net_load - desired_import
let desired_import = net_load_kw
    .min(self.grid_import_limit_kw)
    .max(-self.export_limit_kw);
(net_load_kw - desired_import).clamp(0.0, self.rated_power_kw)
```

This matches OCHRE lines 108-116:
```python
desired_power = max(min(net_power, self.import_limit), -self.export_limit)
self.power_setpoint = desired_power - net_power
```

### 4.3 Ramp Rate Limiting

- Only constrains power **increases** (matches OCHRE Generator.py:129)
- Power decreases/shutdown are instantaneous
- HARES default 1.0 kW/s vs OCHRE default (unlimited, or ~0.1 kW/min if specified)

**Issue 4.1: Ramp rate units differ from OCHRE** (Severity: Info - Not a bug)
- HARES: kW/s (1.0 kW/s default)
- OCHRE: kW/min (typically unset/unlimited)
- This is an improvement - residential reciprocating generators ramp in seconds

### 4.4 Capacity Min Logic

HARES implements OCHRE's capacity_min behavior (generator.rs lines 626-630):
- Values between 0 and capacity_min are clamped **UP** to capacity_min
- Setpoint of exactly 0 keeps generator off (doesn't clamp up)
- This matches OCHRE Generator.py get_power_limits lines 136-138

---

## 5. Output Ports & Telemetry

### 5.1 Port Declarations

| Port Type | Present | Condition |
|-----------|---------|-----------|
| Electrical | Always | Grid interconnection |
| Fuel | Always | Gas/Propane/Oil input |
| Thermal (Zone) | When zone_id set | Waste heat to zone |
| Fluid (CHP loop) | When eta_thermal > 0 && loop_id set | Thermal recovery |

### 5.2 Telemetry Fields

**Non-CHP Generator (4 fields)**:

| Field | Unit | Description | Source |
|-------|------|-------------|--------|
| electric_output_kw | kW | Electrical generation output | Direct |
| fuel_input_w | W | Fuel input power (P_electric / eta) | Derived |
| eta_electric | - | Effective electrical efficiency [0..1] | Derived |
| ramp_limited | - | 1.0 when power change clamped by ramp | Flag |

**CHP Generator (6 fields)** - adds:

| Field | Unit | Description | Source |
|-------|------|-------------|--------|
| thermal_output_w | W | CHP thermal recovery (P_fuel * eta_thermal) | Derived |
| flue_loss_w | W | Residual flue loss (P_fuel - P_electric - Q_thermal) | Derived |

### 5.3 Core Output

The generator reports core flows:

```rust
CoreOutput {
    flows: CoreFlows {
        electric_kw: Some(ElectricPower::Generation(output_kw.max(0.0))),
        reactive_power_kvar: None,
        fuel_w: Some(FuelPower {
            fuel_type: FuelType::Gas,
            consumption_w: fuel_w.max(0.0),
        }),
    },
    state: CoreState {
        operating_mode: None,  // Not reported in core state
        soc: None,
    },
}
```

**Issue 5.1: OperatingMode not in CoreState** (Severity: Low)
- Generator mode (Off/Standby) is tracked in telemetry but not in CoreState
- OCHRE tracks mode in `self.mode` property

---

## 6. Dwelling/Thermal Solver Aggregation

### 6.1 Electrical Integration

The generator integrates with dwelling electrical model via:

1. **Stage 1 electrical accumulation**: Generator reads `ports.electrical.net_active_kw()` to get net load (line 732)
2. **Negative generation convention**: Generator output is written as negative to electrical port (line 778)
3. **Self-consumption**: Uses net load to determine target generation

### 6.2 Thermal Integration

| Scenario | Zone Heat Gain | Notes |
|----------|----------------|-------|
| No CHP (eta_thermal=0) | q_flue = P_fuel - P_electric | All waste heat to zone |
| CHP without fluid port | q_thermal + q_flue | All non-electrical to zone |
| CHP with fluid port | q_flue only | q_thermal → fluid loop |

### 6.3 CHP Fluid Port

- Requires `loop_id` to be set (non-zero)
- Flow rate: configurable via `flow_rate_kg_s` (default 0.1 kg/s)
- Supply/return temps: configurable (default 70°C/60°C)
- Properly separated from zone heat gains (avoids double-counting)

### 6.4 Dwelling Aggregation

- Generator equipment is added to dwelling equipment list
- Runs in Electrical stage (ExecutionStage::Electrical)
- Forced last in update order for self-consumption control (matches OCHRE)
- No explicit "backup power" mode - handled via self-consumption

---

## 7. Summary of Issues

### Critical Issues
None identified.

### Medium Severity Issues
| ID | Description | Location |
|----|-------------|----------|
| 1.1 | HPXML parser always creates Gas Generator, never Gas Fuel Cell | resolve_der.rs:230 |
| 1.2 | No standby_power_w in config (but OCHRE also doesn't model it) | - |
| 3.1 | equipment_id not extracted from HPXML SystemIdentifier | resolve_der.rs:211 |

### Low Severity Issues
| ID | Description | Location |
|----|-------------|----------|
| 2.1 | Default efficiency curve uses custom interpolation (matches OCHRE behavior) | generator.rs:366 |
| 5.1 | OperatingMode not reported in CoreState | generator.rs:840 |

---

## 8. Comparison: HARES vs OCHRE Physics

### Where HARES is Better
1. **Ramp rate units**: HARES uses kW/s (realistic for residential), OCHRE uses kW/min
2. **CHP thermal ports**: HARES fully implements fluid port routing, OCHRE has stub
3. **Efficiency model validation**: HARES validates curve points and efficiency sums
4. **Telemetry units**: HARES uses W for fuel/thermal (correct), OCHRE uses mixed

### Where HARES Matches OCHRE
1. **Constant efficiency model**: Identical behavior
2. **Curve efficiency model**: Same default points, matching interpolation
3. **Quadratic efficiency model**: Fixed OCHRE bug (min→max for floor)
4. **Self-consumption algorithm**: Exact formula match
5. **Capacity min logic**: Exact clamping behavior match

### Where HARES Has Gaps (vs OCHRE features)
1. No HPXML → FuelCell mapping
2. No equipment_id extraction from HPXML

---

## 9. Recommendations

1. **Add FuelCell HPXML support** - Add a way to specify "Gas Fuel Cell" in HPXML (maybe a new element or attribute)
2. **Extract equipment_id** - Parse SystemIdentifier/@id from HPXML Generator elements
3. **Consider standby power modeling** - OCHRE doesn't model it, but real generators have ~50W standby draw
4. **Add control capability for efficiency_type** - Allow external control to switch efficiency models during simulation

---

**End of Review**
