# Dehumidifier Equipment Review

**Reviewer**: Code Review Agent  
**Date**: 2026-03-31  
**Files Analyzed**:
- `crates/hares-equipment/src/hvac/dehumidifier.rs` (797 lines)
- `crates/hares-equipment/src/hvac/cooling_config.rs` (DehumidifierConfig, lines 396-452)
- `crates/hares-io/src/hpxml/resolve_hvac.rs` (HPXML parsing, lines 821-844, 1516-1547)
- `crates/hares-types/src/telemetry_keys.rs` (telemetry key constants)
- `crates/hares-core/src/dwelling/mod.rs` (thermal integration)

---

## 1. HPXML Parsing

### Attributes Parsed from HPXML

| HPXML Element | Unit | HARES Key | Notes |
|---------------|------|-----------|-------|
| `<Capacity>` | pints/day | `capacity_liters_per_day` | Converted via 0.473176 L/pint |
| `<EnergyFactor>` | L/kWh | `energy_factor` | Pre-2019 DOE test standard |
| `<IntegratedEnergyFactor>` | L/kWh | `integrated_energy_factor` | Post-2019 DOE test standard |
| `<DehumidistatSetpoint>` | fraction (0-1) or percent (0-100) | `target_rh` | Normalized to fraction |
| `<FractionDehumidificationLoadServed>` | fraction | `fraction_served` | Clamped to [0,1] |

### Wiring Location

**Issue - Partial HPXML Parsing**: Dehumidifiers are parsed from the `Systems/HVAC` section in `resolve_hvac.rs:1516-1547`. However, HPXML also allows dehumidifiers in the `Appliances` section. The `resolve_loads.rs` function has an explicit allowlist at lines 71-78 that does NOT include `Dehumidifier`:

```rust
for (tag, name) in [
    ("ClothesWasher", "Clothes Washer"),
    ("ClothesDryer", "Clothes Dryer"),
    ("Dishwasher", "Dishwasher"),
    ("Refrigerator", "Refrigerator"),
    ("Freezer", "Freezer"),
    ("CookingRange", "Cooking Range"),
] // Dehumidifier is missing!
```

**Impact**: HPXML files with dehumidifiers in the Appliances section will silently produce no equipment spec. The test fixture `base-appliances-dehumidifier.xml` places the dehumidifier in Appliances, so it would be silently ignored.

### Unused HPXML Attributes

| HPXML Element | Status |
|---------------|--------|
| `<Type>` (portable/whole-home) | Not consumed - no type differentiation |
| `<Location>` | Not consumed - always assigned to ZoneId(1) |
| `<UsageMultiplier>` | Not supported |

---

## 2. OCHRE Defaults Comparison

### OCHRE Status

**Critical Finding**: OCHRE has NO dehumidifier implementation. Evidence:
- `vendors/OCHRE/ochre/utils/hpxml.py` line 1678: `# TODO: add dehumidifier`
- `vendors/OCHRE/ochre/Models/Humidity.py` line 44: `# FUTURE: Dehumidifier?`
- No dehumidifier equipment class exists
- Dehumidifiers are NOT exercised in any OCHRE parity runs

This means HARES has no reference implementation to validate against. The HARES implementation is entirely original and based on EnergyPlus `ZoneHVAC:Dehumidifier:DX` formulation.

### HARES Defaults vs Typical Values

| Parameter | HARES Default | Typical Range | Assessment |
|-----------|---------------|---------------|------------|
| Capacity | 30 L/day | 20-50 L/day | Reasonable |
| Energy Factor | 1.8 L/kWh | 1.2-3.0 L/kWh | LOW - Energy Star minimum is 1.7, but most models are 2.0+ |
| Target RH | 50% (0.5) | 40-60% | Standard |
| Deadband | ±5% (0.025) | ±3-10% | Reasonable |
| Latent Heat of Vaporization | 2,454,000 J/kg | 2,257,000-2,501,000 J/kg (temp-dependent) | HIGH - HARES uses constant |

### OCHRE Parity

Since OCHRE has no dehumidifier, there is **no parity target**. HARES is the reference implementation for this equipment type.

---

## 3. HARES Wiring (HPXML → Equipment)

### Config Structure

The typed config `DehumidifierConfig` (in `cooling_config.rs:399-417`) properly maps HPXML to equipment:

```rust
pub struct DehumidifierConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    pub capacity_liters_per_day: Option<f64>,    // From <Capacity>
    pub energy_factor: Option<f64>,               // From <EnergyFactor>
    pub integrated_energy_factor: Option<f64>,   // From <IntegratedEnergyFactor>
    pub fraction_served: Option<f64>,             // From <FractionDehumidificationLoadServed>
    pub target_rh: Option<f64>,                   // From <DehumidistatSetpoint>
}
```

### Wiring Verification

**Status**: Properly wired for HVAC-section dehumidifiers. The `try_build_dehumidifier_config` function (lines 821-844 in resolve_hvac.rs) creates typed config correctly.

---

## 4. Control Logic

### Activation Algorithm

The dehumidifier uses a two-threshold deadband hysteresis based on zone relative humidity:

```rust
// From dehumidifier.rs:133-151
fn update_is_on(&mut self, current_rh: f64) {
    self.is_on = match maybe_override {
        Some(OperatingMode::Off) => false,
        Some(OperatingMode::Cooling) | Some(OperatingMode::Standby) => true, // BUG: Standby
        Some(_) | None => {
            if self.is_on {
                current_rh >= self.min_rh   // Stay ON while >= min (inclusive)
            } else {
                current_rh > self.max_rh    // Turn ON only when > max (strict)
            }
        }
    };
}
```

### Control Parameters

| Parameter | Default | Configurable |
|-----------|---------|--------------|
| Target RH | 50% (0.5) | Via `target_rh` or `DehumidistatSetpoint` |
| Min RH (deadband lower) | 47.5% | Computed: target - 0.025 |
| Max RH (deadband upper) | 52.5% | Computed: target + 0.025 |
| Deadband half-width | 0.025 (5%) | Not configurable |

### Known Control Issues

**BUG-1: Standby mode forces ON** (DC-005)
- `OperatingMode::Standby` is incorrectly mapped to `is_on = true`
- Standby conventionally means idle, but the code forces full-power operation
- Fix: Remove `Some(OperatingMode::Standby)` from the forced-on arm

**BUG-2: Redundant update_control call** (DC-005)
- `step()` calls `self.update_control(env)` at line 411
- The engine also calls `update_control` before `step` per Equipment trait contract
- Control logic runs twice per timestep, wasting work
- Fix: Remove internal `update_control` call in `step`, rely on engine's pre-call

---

## 5. Output Ports & Telemetry

### Telemetry Fields

All declared in `telemetry_fields()` (lines 513-561):

| Telemetry Key | Unit | Description |
|---------------|------|-------------|
| `water_removal_l_day` | L/day | Water removed at current conditions |
| `electric_power_w` | W | Compressor + fan electric power |
| `electric_kw` | kW | Total electric draw (for dwelling aggregation) |
| `latent_removal_w` | W | Latent cooling removed from zone air |
| `sensible_gain_w` | W | Sensible heat gain dumped back to zone |
| `target_rh` | fraction | Active RH setpoint target |
| `min_rh` | fraction | Deadband lower bound |
| `max_rh` | fraction | Deadband upper bound |
| `is_on` | bool | 1 when compressor/fan on, else 0 |

### Port Contributions

**Electrical Port** (lines 328-332):
```rust
ports.accumulate(&PortContribution::Electrical {
    active_power_kw: snapshot.electric_power_w / WATTS_PER_KILOWATT,
    reactive_power_kvar: 0.0,
})?;
```

**Thermal Port** (lines 333-340):
```rust
ports.accumulate(&PortContribution::Thermal {
    zone: self.zone_id,
    sensible_gain_w: snapshot.sensible_gain_w,  // compressor waste heat
    latent_gain_w: -snapshot.latent_removal_w,  // moisture REMOVED = negative latent gain
    category: ThermalCategory::InternalGain,
})?;
```

### Previously Identified Issues (AR-001)

**GAP-1 (FIXED)**: Dehumidifier was writing `electric_power_w` (Watts) but dwelling power fallback chain was looking for `electric_kw` (kW). This was fixed by adding `electric_kw` alongside `electric_power_w` in telemetry.

---

## 6. Dwelling/Thermal Solver Integration

### Latent Load Integration

In `dwelling/mod.rs:2646-2659`, latent gains from all equipment (including dehumidifiers) are aggregated:

```rust
let latent_from_ports: f64 = self
    .ports
    .thermal
    .iter()
    .filter(|e| e.zone == zone.id)
    .map(|e| e.latent_gain_w)
    .sum();
```

The dehumidifier contributes `latent_gain_w = -latent_removal_w` (negative because it REMOVES moisture from the zone air).

### Internal Gains

The sensible gain from the dehumidifier (`sensible_gain_w = latent_removal_w + electric_power_w`) is added to the zone as an internal gain, representing waste heat from the compressor and fan.

### Humidity Solver Integration

The humidity solver (`hares-envelope/src/humidity_solver.rs:108-112`) reads `latent_gain_w` from the thermal ports:

```rust
let latent_gain_w = ports
    .thermal
    .iter()
    .filter(|e| e.zone == zone_id)
    .map(|e| e.latent_gain_w)
    .sum();
```

---

## Summary Assessment

### Where HARES Physics is Better than OCHRE

| Aspect | HARES Advantage |
|--------|-----------------|
| **Physics completeness** | Full implementation vs OCHRE's "TODO" placeholder |
| **Performance modeling** | Biquadratic curves for capacity vs OCHRE's no-model |
| **Latent load handling** | Proper moisture removal calculation |
| **Humidity solver** | Rigorous bisection-based solver (OCHRE uses psychrolib) |
| **Telemerty** | Comprehensive state reporting |

### Issues Found

| Issue | Severity | Status |
|-------|----------|--------|
| HPXML Appliances section not parsed | HIGH | Open - dehumidifiers in Appliances ignored |
| Energy factor default (1.8) may be optimistic | MEDIUM | Low priority - typical units are 2.0+ |
| Standby mode forces ON | MEDIUM | Open bug - DC-005 |
| Redundant update_control call | LOW | Open bug - DC-005 |
| Latent heat constant vs temperature-dependent | LOW | Design choice - EnergyPlus uses constant |
| No portable vs whole-home differentiation | LOW | Out of scope |
| No zone routing from Location | LOW | Acceptable for single-zone default |

### Verification Notes

- Dehumidifier initialization validates config: capacity must be finite and positive, energy factors must be finite and positive
- The physics model correctly calculates: water removal → latent removal → sensible gain
- Energy balance is maintained: `sensible_gain_w = latent_removal_w + electric_power_w`
- Telemetry keys are all properly declared and wired

---

## Recommendations

1. **HIGH**: Add dehumidifier parsing to `resolve_loads.rs` allowlist to support HPXML Appliances section
2. **MEDIUM**: Fix Standby mode bug - remove `OperatingMode::Standby` from forced-on arm
3. **MEDIUM**: Remove redundant `update_control` call inside `step()`
4. **LOW**: Consider raising default energy factor to 2.0 L/kWh (closer to Energy Star average)
5. **LOW**: Consider adding temperature-dependent latent heat of vaporization
