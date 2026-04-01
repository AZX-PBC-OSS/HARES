# PV Equipment Review: HARES Implementation

## Executive Summary

The HARES PV implementation is **significantly more advanced** than the original OCHRE implementation, with comprehensive physics modeling including temperature derating (SAM-NOCT with wind correction), soiling (Kimber model), and configurable shading models. The HPXML parsing covers the essential PV attributes. However, there are some **gaps in HPXML parsing**, **missing control signal wiring**, and **opportunities to improve the defaults**.

---

## 1. HPXML Parsing

### Parsed Attributes

The HPXML parser in `crates/hares-io/src/hpxml/resolve_der.rs` extracts the following PV attributes:

| HPXML Element | HARES Config Field | Status |
|---------------|-------------------|--------|
| `MaxPowerOutput` | `capacity_kw` | Parsed (W -> kW) |
| `ArrayTilt` | `tilt_deg` | Parsed |
| `ArrayAzimuth` | `azimuth_deg` | Parsed |
| `ModuleType` | `module_type` | Parsed |
| `SystemLossesFraction` | `system_losses_fraction` | Parsed |
| `InverterEfficiency` (via Inverter element) | `inverter_efficiency` | Parsed |
| `Tracking` | - | Validated: only "fixed" supported |

### Issues Found

**Issue #1: Missing inverter capacity parsing** (Severity: Medium)
- HPXML can specify inverter capacity via `<Inverter><MaxPowerOutput>` but HARES does not parse this.
- The `inverter_capacity_kw` field in `PvConfig` is always `None` when parsed from HPXML.
- **Impact**: DC/AC ratio clipping cannot be simulated when inverter capacity < PV capacity.

**Issue #2: Missing power factor parsing** (Severity: Medium)
- HPXML does not define power factor for PV, but this is a useful configurable parameter.
- Not an HPXML gap, but HARES could use a reasonable default.

**Issue #3: NOCT not parsed** (Severity: Low)
- HPXML does not define NOCT (Nominal Operating Cell Temperature), so this relies on HARES defaults (47°C).

---

## 2. OCHRE Defaults and Fallbacks

### HARES Default Values

| Parameter | HARES Default | OCHRE Default | Comparison |
|-----------|---------------|---------------|------------|
| `inverter_efficiency` | 0.96 (96%) | 0.96 (PVWatts) | Matches |
| `tilt_deg` | 30.0° | Falls back to roof tilt, else latitude | HARES hardcodes 30°, OCHRE uses roof geometry |
| `azimuth_deg` | 180.0° (south) | Falls back to "south-most roof" (180-185°) | HARES hardcodes south, OCHRE infers from roof |
| `noct_c` | 47.0°C | Not directly exposed in OCHRE | Equivalent |
| `system_losses_fraction` | 0.14 (14%) | Not explicitly in OCHRE (uses SAM defaults) | Equivalent to PVWatts |
| `module_type` | "Standard" | Uses PVWatts module type | Matches |
| `power_factor` | 1.0 (unity) | Not explicitly set in OCHRE | Equivalent |
| `surface_resolution_deg` | 5.0° | N/A (HARES-only for surface ID quantization) | HARES-only |

### Default Gamma (Temperature Coefficient)

HARES correctly uses PVWatts v8 defaults:
- Standard: -0.0047 /°C
- Premium: -0.0035 /°C  
- ThinFilm: -0.0020 /°C

**Issue #4: Hardcoded tilt/azimuth defaults** (Severity: Low)
- HARES defaults to 30° tilt, 180° azimuth (south)
- OCHRE defaults to the "most southern-facing roof" when available
- HARES should ideally fall back to roof geometry like OCHRE

---

## 3. HARES Wiring (Configuration Mapping)

### Configuration Flow

```
HPXML (resolve_der.rs:resolve_pv)
  └── PvConfig struct
       │
       ▼
EquipmentSpec → EquipmentRegistry.create("PV")
  │
  ▼
PV::init_typed() in mod.rs
  │
  ├── tilt_deg: c.tilt_deg.unwrap_or(30.0)
  ├── azimuth_deg: c.azimuth_deg.unwrap_or(180.0)
  ├── noct_c: c.noct_c.unwrap_or(DEFAULT_NOCT_C)
  ├── module_type: ModuleType::from_str()
  ├── inverter_efficiency: c.inverter_efficiency.unwrap_or(0.96)
  ├── inverter_capacity_kw: c.inverter_capacity_kw (None if not in HPXML)
  ├── power_factor: c.power_factor.unwrap_or(1.0)
  └── system_losses_fraction: c.system_losses_fraction.unwrap_or(0.14)
```

### Wiring Issues

**Issue #5: Inverter capacity not wired from HPXML** (Severity: Medium)
- HPXML inverter parsing exists but `inverter_capacity_kw` is hardcoded to `None` in resolve_der.rs:66
- Should pull from `<Inverter><MaxPowerOutput>` when available

---

## 4. Control Logic

### Implemented Control Signals

HARES PV supports these control signals (via `apply_control_unchecked`):

| Control Signal | Implementation | Status |
|----------------|---------------|--------|
| `PowerLimit` | Limits max AC output | Implemented |
| `PowerSetpoint` | Acts as power limit (OCHRE semantics differ) | Implemented |
| `CurtailmentPercent` | Fractional curtailment (0-100%) | Implemented |
| `ReactiveSetpoint` | Direct Q setpoint | Implemented |
| `PowerFactorSetpoint` | Static PF → reactive power | Implemented |
| `InverterPriorityMode` | Watt/Var/CPF priority modes | Implemented |

### Control Logic Analysis

**Positive Findings:**
- Proper inverter clipping logic with three priority modes (Watt, Var, CPF)
- Curtailment properly reports telemetry (`CURTAILMENT_KW`, `INVERTER_CLIPPING_KW`)
- Power factor correctly derives Q = P * tan(acos(PF))
- Reactive power respects inverter capacity constraints

**Issue #6: PowerSetpoint semantics differ from OCHRE** (Severity: Low)
- OCHRE: `p_set_point = max(max_generation, p_set)` — setpoint is a floor
- HARES: Uses as an upper bound (max_power_kw)
- This is documented in mod.rs:637-640 but may cause behavioral differences

---

## 5. Output Ports and Telemetry

### Output Port

| Port Type | Direction | Value |
|-----------|-----------|-------|
| Electrical | Generation | Negative = generation (convention: positive = consumption) |

### Telemetry Fields

| Telemetry Key | Unit | Description | Source |
|---------------|------|-------------|--------|
| `dc_power_kw` | kW | DC output before inverter | Computed |
| `ac_power_kw` | kW | AC output after inverter & curtailment | Computed |
| `reactive_power_kvar` | kVAR | Reactive power output | Computed |
| `cell_temp_c` | °C | Capacity-weighted avg cell temp | SAM-NOCT model |
| `irradiance_w_m2` | W/m² | Capacity-weighted avg POA irradiance | From environment |
| `inverter_efficiency` | - | Inverter efficiency applied | Config |
| `curtailment_kw` | kW | Power curtailed by PowerLimit | Computed |
| `inverter_clipping_kw` | kW | Power lost to inverter capacity | Computed |
| `soiling_ratio` | - | Kimber soiling ratio (1.0=clean) | Soiling model |
| `shading_factor` | - | Shading factor (1.0=no shade) | Shading model |

### Missing Telemetry

**Issue #7: Missing DC/AC conversion losses telemetry** (Severity: Low)
- No explicit telemetry for inverter losses (DC power - AC power)
- Can be derived: `dc_power_kw - ac_power_kw` when both are available

---

## 6. Dwelling/Thermal Solver Aggregation

### PV-to-Roof Shading Integration

HARES implements a **unique and excellent feature** where PV arrays attached to roofs reduce incident solar on those roofs:

```rust
// dwelling/mod.rs:register_pv_roof_shading
let collector_area = cap * 1000.0 / 420.0 * 2.0;  // ~2 m² per 420 W panel
*coverage_by_roof.entry(bid as u32).or_default() += collector_area / roof_area;
```

This reduces envelope solar heat gain proportional to PV coverage - a physically correct interaction.

### Electrical Aggregation

PV power aggregates into the electrical solver:

```
ports.electrical.generation_power_kw = -final_p_kw  // Negative = generation
ElectricalSolver: net_active_kw = load_power_kw + generation_power_kw
```

The electrical solver correctly implements net metering semantics where:
- Positive net = grid import (consumption)
- Negative net = grid export (generation)

### Battery + PV Interaction

The battery system has solar-aware charging logic:
- `solar_only_charging` flag prevents charging from grid when no PV surplus
- Battery checks `generation_power_kw` to detect PV surplus

---

## 7. Advanced Features Analysis

### Soiling Model (Kimber Model)

**Implemented correctly** in `pv/soiling.rs`:
- Rolling rainfall buffer (configurable window)
- Cleaning threshold (6mm default)
- Grace period after rain (14 days default)
- Maximum soiling cap (30% default)
- Configurable regional soiling rates

### Shading Models

**Comprehensive implementation** in `pv/shading.rs`:
- `None` (no shading)
- `FixedLoss` (annual constant)
- `MonthlyLoss` (12 months)
- `ObstructionAngle` (single horizon obstruction)
- `HorizonProfile` (arbitrary horizon polygon)

### Temperature Model

**SAM-NOCT with wind correction** is correctly implemented:
- Uses wind speed to adjust cell temperature
- At 1 m/s (NOCT reference) reduces to basic NOCT formula
- Wind cooling increases PV output (test verified)

---

## Summary of Issues

| Issue | Severity | Description | Recommendation |
|-------|----------|-------------|----------------|
| #1 | Medium | Inverter capacity not parsed from HPXML | Add parsing for `<Inverter><MaxPowerOutput>` |
| #2 | Medium | Power factor default is 1.0 (no VAR) | Consider defaulting to 0.95 for more realistic VAR support |
| #3 | Low | Hardcoded tilt/azimuth (30°/180°) | Consider falling back to roof geometry like OCHRE |
| #4 | Low | Inverter capacity not wired from HPXML | Fix wiring in resolve_der.rs |
| #5 | Low | PowerSetpoint semantics differ from OCHRE | Document difference or align |
| #6 | Low | Missing DC/AC loss telemetry | Add computed telemetry for inverter losses |

---

## Comparison with OCHRE

### Where HARES is Better

1. **Soiling model**: Kimber model with rainfall-based cleaning — OCHRE had no soiling
2. **Shading models**: Multiple configurable models — OCHRE had no shading
3. **Wind-corrected NOCT**: More accurate cell temperature — OCHRE used basic NOCT
4. **PV-to-roof shading**: Proper envelope interaction — OCHRE didn't reduce envelope solar gains
5. **Module type temperature coefficients**: Proper gamma values per module type
6. **Inverter clipping**: Proper DC/AC ratio simulation with priority modes
7. **Telemetry depth**: Much more comprehensive than OCHRE

### Where OCHRE Had Advantages

1. **Tilt/azimuth defaults**: OCHRE infers from roof geometry, HARES uses hardcoded defaults
2. **SAM integration**: OCHRE directly used SAM for full simulation, HARES uses LUT or internal model

---

## Conclusion

The HARES PV implementation is **substantially superior** to the original OCHRE implementation in terms of physics fidelity and feature completeness. The core physics (temperature derating, inverter modeling, irradiance calculation) are correctly implemented. The identified issues are relatively minor and relate primarily to HPXML parsing gaps and default value choices rather than fundamental physics problems.

The most impactful issue to address is **Issue #1** (inverter capacity not parsed) as it prevents accurate simulation of DC/AC ratio clipping scenarios common in residential PV installations.
