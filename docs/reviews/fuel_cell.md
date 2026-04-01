# Fuel Cell (CHP) Configuration Review

## Overview

This review examines the Gas Fuel Cell (Combined Heat and Power - CHP) implementation in HARES, comparing it against OCHRE to identify where physics are preserved and where issues exist.

---

## 1. HPXML Parsing

### What HPXML attributes are parsed for fuel cells?

**Location**: `crates/hares-io/src/hpxml/resolve_der.rs:185-237`

**HARES** parses the following HPXML Generator attributes:
- `FuelType` - fuel type (natural gas, propane, etc.)
- `ElectricalPowerOutput` - rated capacity in kW (defaults to 10.0 kW if not specified)
- `AnnualOutputkWh` - annual electric output
- `AnnualConsumptionkBtu` - annual fuel consumption

**Issue**: HARES does NOT distinguish between "Gas Generator" and "Gas Fuel Cell" in HPXML parsing. The parser always creates "Gas Generator" equipment (line 231), even when parsing fuel cell data. There's no HPXML attribute to specify CHP/thermal recovery.

### OCHRE Comparison

OCHRE's `parse_hpxml_equipment()` in `vendors/OCHRE/ochre/utils/hpxml.py` does NOT parse generators at all - they must be added via overrides. Both OCHRE and HARES lack first-class HPXML support for fuel cells.

---

## 2. OCHRE Defaults

### What defaults does OCHRE use for fuel cells?

**Location**: `vendors/OCHRE/ochre/Equipment/Generator.py:218-222`

```python
class GasFuelCell(GasGenerator):
    name = "Gas Fuel Cell"

    def __init__(self, efficiency_type="curve", **kwargs):
        super().__init__(efficiency_type=efficiency_type, **kwargs)
```

**Key OCHRE Defaults** (from test file `test_generator.py`):
- Default capacity: 6 kW
- Default electrical efficiency: 0.95 (95%) - unrealistic for residential fuel cells
- Default efficiency type: **curve** (not constant)
- Default efficiency curve: (0, 0), (0.5, 1), (1, 1) - from `defaults/Gas Generator/efficiency_curve.csv`
- `efficiency_chp` (thermal recovery): defaults to 0 (no CHP)

**HARES Defaults** (from `crates/hares-equipment/src/generator.rs:180-215`):

```rust
const DEFAULT_ETA_ELECTRIC: f64 = 0.30;  // 30% - realistic for residential NG generator
const DEFAULT_ETA_THERMAL: f64 = 0.0;   // CHP disabled by default
const DEFAULT_DELTA_KW_PER_S: f64 = 1.0; // kW/s ramp rate (improvement over OCHRE)
const DEFAULT_RATED_POWER_KW: f64 = 10.0;
```

**Important Difference**:
- OCHRE defaults to 95% electrical efficiency (unrealistic for fuel cells)
- HARES defaults to 30% electrical efficiency (realistic for residential generators)
- **HARES physics are better** here - the OCHRE default is physically implausible

---

## 3. HARES Wiring

### How is fuel cell configuration wired from HPXML to equipment?

**Location**: `crates/hares-equipment/src/registry.rs:52-54` and `crates/hares-equipment/src/generator.rs:965-982`

Both "Gas Generator" and "Gas Fuel Cell" are registered in the equipment registry:

```rust
pub fn register_with_registry(registry: &mut EquipmentRegistry) {
    registry.register("Gas Generator", Box::new(|config| {
        Box::new(GasGenerator { core: Generator::new(config, GeneratorKind::GasGenerator) })
    }));
    registry.register("Gas Fuel Cell", Box::new(|config| {
        Box::new(FuelCell { core: Generator::new(config, GeneratorKind::FuelCell) })
    }));
}
```

**Key Points**:
- Both GasGenerator and FuelCell use the same underlying `Generator` physics implementation
- They differ only in `GeneratorKind` which sets different default efficiency types:
  - GasGenerator: default `efficiency_type = "constant"`
  - FuelCell: default `efficiency_type = "curve"`
- The HPXML parser currently always creates "Gas Generator" - there's no path to create "Gas Fuel Cell" from HPXML

**Issue**: HPXML parsing does not support creating "Gas Fuel Cell" equipment - it's always "Gas Generator".

---

## 4. Control Logic

### How does fuel cell control work?

**Location**: `crates/hares-equipment/src/generator.rs:604-631` (determine_target_kw)

**Control Modes**:
1. **Power Setpoint**: Direct power command (positive = generation output)
2. **Self-Consumption Mode**: Matches generation to net load with import/export limits
3. **Mode Override**: Off or Standby (returns to self-consumption)

**Self-Consumption Formula** (matches OCHRE exactly):
```rust
let desired_import = net_load_kw
    .min(grid_import_limit_kw)
    .max(-export_limit_kw);
target = (net_load_kw - desired_import).clamp(0.0, rated_power_kw)
```

**Ramp Rate** (HARES improvement):
- HARES: `delta_kw_per_s` (kW per second) - default 1.0 kW/s
- OCHRE: `ramp_rate` (kW per minute) - default 0.1 kW/min = 0.0017 kW/s
- **HARES is significantly better** - OCHRE's default ramp rate is unrealistically slow for residential reciprocating generators

**Capacity Min**:
- When `capacity_min_kw` is set, generator will not operate below that level
- Values between 0 and capacity_min are clamped UP to capacity_min
- Exact 0 keeps generator off (matches OCHRE behavior)

**Issue**: No explicit control mode for "thermal following" - thermal output is always a fixed fraction of fuel input (eta_thermal), not controlled based on thermal demand.

---

## 5. Output Ports & Telemetry

### What state/telemetry is reported?

**Location**: `crates/hares-equipment/src/generator.rs:988-1039`

**Telemetry Fields**:
| Field | Unit | Description |
|-------|------|-------------|
| `electric_output_kw` | kW | Electrical generation output |
| `fuel_input_w` | W | Fuel input power (P_electric / eta) |
| `eta_electric` | - | Effective electrical efficiency at current load |
| `ramp_limited` | - | 1.0 when power change was clamped by ramp-rate |
| `thermal_output_w` | W | CHP thermal recovery (if eta_thermal > 0) |
| `flue_loss_w` | W | Residual flue loss |

**CoreOutput** (for external consumers):
- `electric_kw`: ElectricPower::Generation(output_kw.max(0.0))
- `fuel_w`: FuelPower with fuel_type and consumption_w
- `operating_mode`: Standby when on, Off when off

---

## 6. Dwelling/Thermal Solver Integration

### How does it integrate with dwelling?

**Location**: `crates/hares-equipment/src/generator.rs:758-813`

**Energy Flow Calculation**:
```rust
// Derive fuel and heat flows
fuel_w = output_kw * 1000.0 / eta
electrical_w = output_kw * 1000.0
q_thermal_w = fuel_w * eta_thermal        // CHP thermal recovery
q_flue_w = fuel_w - electrical_w - q_thermal_w  // Residual loss
```

**Port Routing** (HARES improvement over OCHRE):
1. **No CHP (eta_thermal = 0)**: All non-electrical fuel loss → zone as waste heat
2. **CHP with fluid port**: q_thermal → fluid port; q_flue → zone (no double-count)
3. **CHP without fluid port**: q_thermal + q_flue → zone (all waste heat to zone)

**Fluid Port** (when `loop_id` is configured):
- Flow rate: default 0.1 kg/s (≈6 L/min)
- Supply temp: default 70°C
- Return temp: default 60°C

**Issue**: OCHRE's CHP implementation has this comment in the code:
```python
# self.power_chp = 0  # usable output heat for combined heat and power (CHP) uses, in kW
```
OCHRE never fully implemented CHP thermal ports. HARES has fully implemented them.

---

## Summary: Where HARES Physics Are Better / Issues

### HARES Improvements Over OCHRE:

1. **Realistic electrical efficiency default**: 30% vs 95% (OCHRE default is physically implausible)
2. **Realistic ramp rate**: 1.0 kW/s vs 0.0017 kW/s (OCHRE is unrealistically slow)
3. **Full CHP thermal port implementation**: HARES routes thermal to fluid loop; OCHRE is stubbed
4. **Fixed OCHRE bug**: OCHRE line 169 has `min(eff, 0.001)` but should be `max(eff, 0.001)`

### Issues to Document:

1. **HPXML does not support Fuel Cell creation**: Parser always creates "Gas Generator" - no way to specify "Gas Fuel Cell" or enable CHP via HPXML
2. **No thermal following control**: Thermal output is always proportional to fuel input, not controlled by thermal demand
3. **No default thermal efficiency for fuel cells**: Even when using "Gas Fuel Cell", eta_thermal defaults to 0 (no CHP) - must be explicitly configured
4. **No HPXML attributes for CHP**: No way to specify thermal recovery ratio, loop ID, flow rate, or temperatures from HPXML

### Recommendations:

1. Add HPXML support for fuel cell-specific attributes (similar to how HPWH has extended data)
2. Add default thermal efficiency for FuelCell kind (e.g., eta_thermal = 0.4 for typical residential CHP)
3. Consider adding thermal following control mode that modulates electric output based on thermal demand
