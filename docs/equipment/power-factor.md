# Reactive Power & Power Factor

[Back to Architecture](../architecture.md)

**Source**: `crates/hares-types/src/zip.rs`, `crates/hares-equipment/src/config.rs`

Canonical reference for the ZIP load model, power-factor → reactive-power conversion, sign convention, per-equipment default power factors, configuration override mechanism, control precedence, and known divergences from OCHRE and EnergyPlus.

## ZIP Load Model

### Math

HARES models voltage-dependent real and reactive power via the ZIP polynomial formalism ported from OCHRE `Equipment.py` `run_zip` (lines 200–218) but with corrected coefficient wiring (see [OCHRE Divergences](#ochre-divergences)):

```
P_actual = P · (zp·V² + ip·V + pp)        where V = voltage_pu / v0
Q_actual = P_actual · tan(acos(pf)) · (zq·V² + iq·V + pq)
```

The real and reactive polynomials each normalise to sum ≈1.0 so that the model is a pure redistribution at reference voltage (validated at init via `validate_zip_sums` in `crates/hares-equipment/src/config.rs:459-478`).

### `ZipLoad` struct (`crates/hares-types/src/zip.rs:30-49`)

| Field | Meaning |
|-------|---------|
| `zp`, `ip`, `pp` | Real-power Z/I/P fractions (must sum to ≈1.0) |
| `zq`, `iq`, `pq` | Reactive-power Z/I/P fractions (must sum to ≈1.0 if pf ≠ 0) |
| `pf` | Power-factor magnitude; `0.0` = sentinel "no reactive" |
| `v0` | Reference voltage [pu], default 1.0 |

The struct is the single source of truth for ZIP parameters. Field names match `defaults/zip_parameters.toml` rows, so the struct deserialises directly from that file. A drift test in `hares-io` asserts that every toml row matches the in-code class table `zip_defaults_for_class()`.

### pf=0 Sentinel

When `pf ≈ 0` (`|pf| < 1e-9`), `tan_phi()` returns 0.0, and no reactive power is produced. This avoids computing `tan(acos(0.0))` which diverges. `ZipLoad::constant_power()` constructs with pf=0.0 by default.

### Rule R1: Reactive-Only ZIP for Typed Equipment

Typed equipment (HVAC, water heaters, fans, pumps, etc.) computes its real electric power through its own physics (compressor curves, COP, fan power, etc.). To guarantee bit-identical real power at all voltages, typed equipment uses a **Q-only** path:

1. Real power is computed exactly as before (no ZIP scaling on P)
2. `ZipLoad::reactive_only(zq, iq, pq, pf)` forces the real side to `(0, 0, 1)` — `apply()` leaves P untouched
3. Reactive power is derived **from** the already-computed real power: `Q = P · tan_phi() · (zq·V² + iq·V + pq)` via `ZipLoad::reactive_kvar(p_kw, voltage_pu)`

This is enforced by `resolve_reactive_zip()` in `crates/hares-equipment/src/config.rs:439-449`, which resolves the effective ZIP through the full precedence chain and then forces `(zp, ip, pp) = (0, 0, 1)`.

Floating-point hazard avoided: coefficient sums in the literature (e.g. HPWH 0.825 − 0.44 + 0.615) are not exactly 1.0 in binary. Running P through the full ZIP polynomial at nominal voltage would introduce a sub-ULP change relative to the original P. Rule R1 guarantees real-power bit-identity by construction.

Only `ScheduledLoad` and `EventBasedLoad` retain full ZIP (real + reactive), matching their pre-existing behaviour. Those load types use `ZipLoad::apply()` with byte-identical arithmetic to the legacy `ZipCoefficients::apply()`.

## Sign Convention

**Positive = inductive/lagging/absorbing vars. Negative = capacitive/leading/supplying vars.**

This is normative across all of HARES, matching:
- `CoreFlows::reactive_power_kvar` doc: "Positive = inductive/lagging (IEEE 1547)"
- The "Total Reactive Power (kVAR)" output column
- AMI convention
- OCHRE Q-sign-follows-P convention

The same signed value is pushed to **three channels per equipment** with **no negation anywhere**:
1. **Port** (`PortContribution::Electrical.reactive_power_kvar`)
2. **CoreOutput** (`CoreFlows.reactive_power_kvar`)
3. **Telemetry** (`REACTIVE_POWER_KVAR` key)

A debug-build validator `validate_port_core_electrical_consistency` (`crates/hares-types/src/equipment.rs:1252`) asserts these three channels agree on every equipment step. It fires in all three equipment dispatch phases of the dwelling step (search `crates/hares-core/src/dwelling/mod.rs` for `validate_port_core_electrical_consistency`; line numbers shift too often to cite).

**Generation implications:** A generating PV at pf < 1 *supplies* vars, producing a negative bus Q. The PV module computes one signed bus Q and uses it identically for port, CoreOutput, and telemetry with no additional negation (`pv/mod.rs:1053-1117`).

## Class-Defaults Table

Every class name and coefficient set below is verified in `crates/hares-types/src/zip.rs:228-458`. Rows marked "HARES extension" are not in the OCHRE ZIP Parameters.csv — each carries a one-line justification in the source.

| Class | pf | zq | iq | pq | Source / Note |
|-------|----|----|----|----|---------------|
| Lighting (all subtypes) | 1.0 | 0.46 | 0.51 | 0.03 | Bokhari et al. 2014 |
| Refrigerator, Freezer | 0.80 | 17.44 | −28.62 | 12.18 | Bokhari et al. 2014 |
| MELs, Basement MELs | 0.80 | 8.40 | −14.17 | 6.77 | Bokhari et al. 2014 |
| Plug Loads | 0.80 | 8.40 | −14.17 | 6.77 | HARES extension: same population as MELs |
| TV | 0.80 | 8.40 | −14.17 | 6.77 | HARES extension: split-out plug load (HPXML PlugLoadType="TV other"); MELs row — no TV-specific row in OCHRE CSV |
| Well/Pool/Spa Pump | 0.84 | 14.78 | −23.71 | 9.93 | Hajagos & Danai 1998 |
| Pool/Spa Heater | 1.0 | 0.15 | 0.86 | −0.01 | Same as RESISTANCE |
| Ceiling/Ventilation Fan | 0.87 | 0.50 | 0.62 | −0.12 | OCHRE `fans` row |
| HRV, ERV | 0.87 | 0.50 | 0.62 | −0.12 | HARES extension: ventilation fan motors |
| Clothes Washer | 0.65 | −0.56 | 2.20 | −0.64 | OCHRE |
| Clothes Dryer | 0.99 | 1.0 | 0.0 | 0.0 | OCHRE |
| Dishwasher | 0.99 | 0.0 | 0.0 | 1.0 | OCHRE |
| Range, Cooking Range | 1.0 | 1.0 | 0.0 | 0.0 | OCHRE |
| ASHP/MSHP Heater | 0.84 | 14.78 | −23.71 | 9.93 | Hajagos & Danai 1998 (same as PUMPS) |
| GSHP/WSHP Heater | 0.84 | 14.78 | −23.71 | 9.93 | HARES extension: same compressor class as ASHP |
| Air Conditioner / ASHP/MSHP Cooler / Room AC | 0.96 | 12.53 | −21.11 | 9.58 | Hajagos & Danai 1998 |
| GSHP/WSHP Cooler | 0.96 | 12.53 | −21.11 | 9.58 | HARES extension: same compressor class as ASHP |
| Dehumidifier | 0.96 | 12.53 | −21.11 | 9.58 | HARES extension: cooling compressor behaviour |
| Electric Baseboard/Furnace/Boiler | 1.0 | 0.15 | 0.86 | −0.01 | RESISTANCE row (Bokhari et al. 2014) |
| Gas Furnace | 0.87 | 0.50 | 0.62 | −0.12 | HARES extension: blower fan motor |
| Gas Boiler | 0.84 | 14.78 | −23.71 | 9.93 | HARES extension: circulation pump motor |
| Gas Water Heater | 0.87 | 0.50 | 0.62 | −0.12 | HARES extension: draft-inducer fan motor |
| Electric Resistance WH | 1.0 | 1.0 | 0.0 | 0.0 | Constant impedance (OCHRE row 17) |
| Heat Pump Water Heater | 0.97 | 7.47 | −11.43 | 4.96 | OCHRE lab-blended |
| Tankless/Gas Tankless WH | 1.0 | 0.0 | 0.0 | 1.0 | HARES extension: control electronics, unity pf |
| Ideal Cooler/Heater/HVAC | 1.0 | 0.0 | 0.0 | 1.0 | OCHRE Ideal = unity |

The real-power polynomial (`zp`, `ip`, `pp`) is also defined per class in the source — see `zip_defaults_for_class()` for the full coefficient set.

### Per-Equipment PF Summary

| Equipment | PF | Controllable? | Notes |
|-----------|----|---------------|-------|
| ASHP/MSHP/GSHP/WSHP Heater | 0.84 | No | Compressor PF is physics, not a control surface |
| AC/ASHP/MSHP Cooler | 0.96 | No | Whole-unit (compressor + fan + crankcase) |
| Room AC | 0.96 | No | |
| GSHP/WSHP Cooler | 0.96 | No | |
| Dehumidifier | 0.96 | No | |
| Gas Furnace (blower) | 0.87 | No | Fan motor only; electric elements pf=1.0 (Q=Some(0.0)) |
| Gas Boiler (pump) | 0.84 | No | Circulation pump; electric elements pf=1.0 (Q=Some(0.0)) |
| Ventilation/HRV/ERV | 0.87 | No | Same PF whether ScheduledLoad or typed ventilation model |
| HPWH | 0.97 | No | Blended on total (compressor + backup + fan) |
| Resistance WH | 1.0 | No | Q=Some(0.0) — resistive element |
| Gas WH | 0.87 | No | Draft-inducer fan |
| Tankless WH | 1.0 | No | Control electronics only |
| Ideal HVAC | 1.0 | No | Q=Some(0.0) |
| Electric Baseboard | 1.0 | No | Q=Some(0.0) |
| ScheduledLoad/EventLoad | Per-class | No | Full ZIP with byte-identical arithmetic |
| PV | 1.0 (configurable) | **Yes** | ReactiveSetpoint, PowerFactorSetpoint, PowerSetpoint-Q |
| Battery | 1.0 (configurable) | **Yes** | ReactiveSetpoint, PowerFactorSetpoint, PowerSetpoint-Q; kVA clamp |
| EV | 1.0 (configurable) | **Yes** | ReactiveSetpoint, PowerFactorSetpoint, PowerSetpoint-Q; kVA clamp; default unity = bit-identical |
| Generator | 0.0 (Q≡0) | **No** | Genset excitation out of scope |

## Config Override

### Precedence

Resolved in `resolve_zip()` at `crates/hares-equipment/src/config.rs:411-416`:

1. `EquipmentConfig.zip` sidecar (from `EquipmentSpec.zip_params` + `"zip"` override object)
2. Class-table defaults via `zip_defaults_for_class(&config.ochre_class)`
3. `ZipLoad::constant_power()` (no reactive, P untouched)

For typed equipment, `resolve_reactive_zip()` additionally forces real-power coefficients to `(0, 0, 1)` (Rule R1) and validates coefficient sums.

### Rust TOML Example

`ConfigPayload` is internally tagged with `kind` (`"raw"` or `"typed"`); typed
payloads carry `type_name`, `version`, and the config object in `data`
(`crates/hares-equipment/src/config.rs:137-151`). The ZIP sidecar
(`EquipmentConfig.zip`) travels outside the payload:

```toml
[[equipment]]
name = "ASHP Heater"
ochre_class = "ASHP Heater"

[equipment.payload]
kind = "typed"
type_name = "ASHP Heater"  # EquipmentTypedConfig::equipment_type_name()
version = 1                # EquipmentTypedConfig::schema_version()

[equipment.payload.data]
# ... typed HeatPumpHeaterConfig fields ...

[equipment.zip]
zp = 0.0
ip = 0.0
pp = 1.0
zq = 14.78
iq = -23.71
pq = 9.93
pf = 0.88
```

### Python Dict Example

```python
overrides = {
    "ASHP Heater": {
        "zip": {
            "pf": 0.88,
            "zq": 14.78,
            "iq": -23.71,
            "pq": 9.93,
        }
    }
}
```

The `"zip"` key is reserved in the override system — it is peeled out of the merged JSON map so that `#[serde(deny_unknown_fields)]` typed config structs never see it, then merged field-wise over `EquipmentSpec.zip_params`.

A malformed `"zip"` override is a hard configuration error at dwelling build time (matching the `deny_unknown_fields` ethos): an unknown key (`{"zip": {"fp": 0.9}}`), a non-numeric value, or a non-object value all fail loudly, listing the valid keys `zp/ip/pp/zq/iq/pq/pf/v0`.

## Control Precedence

For equipment that supports reactive control (PV, battery, EV):

1. **`q_setpoint_kvar`** (`Option<f64>`, from `ReactiveSetpoint` or `PowerSetpoint.reactive_power_kvar`): absolute override, passed through as-commanded. Positive = absorbing vars, negative = supplying vars. A commanded `0.0` is a real override (`Some(0.0)`) that forces Q = 0 over any pf < 1 baseline — `None` means "no override", not zero.
2. **PowerFactorSetpoint-updated `power_factor`**: sets the displacement power factor and clears `q_setpoint_kvar` to `None`, so future steps use the updated pf for baseline computation.
3. **Baseline ZIP `pf`** (from config sidecar → class defaults → 1.0): static pf producing `Q = P · tan(acos(pf))` at reference voltage.

On PV (generator at pf < 1), the baseline path produces negative Q (supplying vars): `bus_q_kvar = -|P_gen| · tan(acos(pf))` (see the `bus_q_kvar` computation in `pv/mod.rs`).

On battery and EV, reactive power is clamped to respect the inverter's apparent-power rating: `|Q| ≤ sqrt(max(0, S² − P²))` with active-power priority — real power is never curtailed to make room for reactive (the kVA clamp in `battery/mod.rs` `step()` and `ev/mod.rs` `compute_reactive_kvar()`).

## Per-Equipment Reactive Behaviour

### PV

- Supports `ReactiveSetpoint`, `PowerFactorSetpoint`, and `PowerSetpoint.reactive_power_kvar`
- Unified sign convention: one signed bus Q used identically on port, CoreOutput, and telemetry (no negation)
- At pf=1.0 (default): Q=0, unchanged from pre-PF behaviour
- Config fields: `power_factor` (default 1.0), `inverter_capacity_kva`

### Battery

- Supports `ReactiveSetpoint`, `PowerFactorSetpoint`, and `PowerSetpoint.reactive_power_kvar`
- `PowerFactorSetpoint` clears the `q_setpoint_kvar` override to `None` (same as PV)
- kVA clamp with active-power priority: real power is never reduced for reactive
- Config fields in `BatteryConfig`: `power_factor` (default 1.0), `inverter_capacity_kva` (default `max(max_charge_kw, max_discharge_kw)`)
- Charging battery at pf < 1 produces positive Q (absorbing vars); discharging at pf < 1 produces negative Q (supplying vars baseline — follows the sign of P)

### EV

- Supports `ReactiveSetpoint`, `PowerFactorSetpoint`, and `PowerSetpoint.reactive_power_kvar` — same smart-inverter var control as the battery (IEEE 1547-2018 / SAE J3072)
- `PowerFactorSetpoint` clears the `q_setpoint_kvar` override to `None` (same as battery)
- kVA clamp with active-power priority: real power is never reduced for reactive
- Config fields in `EvConfig`: `power_factor` (default 1.0 for PFC unity bit-identical baseline), `charger_capacity_kva` (default `max(max_charging_power_kw, v2g_max_discharge_kw, v2l_max_discharge_kw)`)
- Charging EV at pf < 1 produces positive Q (absorbing vars); V2G/V2L discharge at pf < 1 produces negative Q (supplying vars — baseline sign follows P)
- Checkpoint version 3 persists `q_setpoint_kvar` (`Option<f64>`) and `power_factor`; `reactive_power_kvar` resets to 0.0 on load (recomputed on next step)

### Generator

- Q ≡ 0 for all fuel types and operating modes
- Justification: detailed synchronous genset excitation / power-factor control is out of scope for this model; Q=0 keeps parity with OCHRE
- **Important disambiguation:** `generator.rs:453` contains a local variable `fuel_curve_power_factor` — this is the stack-cooler polynomial's power-scaling term for the fuel cell thermal model, NOT the electrical power factor. It is a combustion/thermal term purely internal to `compute_stack_cooler_heat()`.

### HVAC (all types)

- Baseline-only: compressor/fan/pump PF is physics, not a control surface
- All typed HVAC equipment uses Rule R1 (Q derived from already-computed P)
- PF 0.84 for heat pump compressors and pumps; 0.96 for cooling compressors; 0.87 for fans/blowers; 1.0 for resistive elements
- Folded unit PF (compressor + fan + crankcase on one electrical port) — matches OCHRE whole-unit convention

### Water Heaters

- All types now carry `REACTIVE` core capability and report `reactive_power_kvar: Some(q)`
- Port, CoreOutput, and telemetry are consistent (the pre-existing port-vs-CoreOutput bug is fixed)
- Checkpoint version 2 on all WH types — `REACTIVE_POWER_KVAR` persists across save/load
- HPWH: blended 0.97 on total (compressor + backup + fan on single electrical port)

## OCHRE Divergences

These are deliberate, documented improvements over OCHRE:

| Item | OCHRE behaviour | HARES behaviour |
|------|-----------------|-----------------|
| Coefficient wiring at off-nominal V | Cross-wires real/reactive arrays (`Equipment.py:211-214`) | Real coefficients → P, reactive coefficients → Q (physically correct) |
| Voltage bypass at V=V0 | Skips ZIP polynomial at nominal voltage | Always evaluates ZIP (ScheduledLoad byte-identity preserved) |
| Signed power factor on PV | Uses negative signed pf for gen-P/consume-Q case (`PV.py:194-196`) | Uses unsigned pf magnitude (0,1], sign is inherent in generation direction |
| Power factor setpoint on PV | Not uniform across ports | Unified sign: one signed bus Q on port/CoreOutput/telemetry |
| HPWH per-component PF | Single blended PF 0.97 | Same (documented; per-component split is future work) |

## Extension Hooks (Future Work)

These are documented as potential enhancements, not currently implemented:

- **Load-dependent PF**: compressors do not maintain constant pf across the full load range; a PLR-dependent pf curve would improve accuracy at part load
- **Per-speed PF in multi-speed heat pumps**: each compressor speed stage could carry its own pf
- **Per-component PF splitting for blended units** (HPWH and heat pump heaters): separate pf values for compressor, fan, and resistive backup instead of one folded value. Until this lands, resistive ER-backup power inside a blended HP heater is assigned the compressor pf (0.84) and produces phantom kvar during backup events — see the caveat in [hvac.md](./hvac.md#reactive-power)
- **PV-style priority modes on battery**: Watt/Var/Cpf inverter priority modes (currently only active-power priority clamping is implemented)
- **Typed equipment real-power ZIP**: allowing real-power voltage sensitivity on typed equipment where it is justified by physics (currently Rule R1 blocks this by design, guaranteeing bit-identical real power)

## Source Files

| File | Purpose |
|------|---------|
| `crates/hares-types/src/zip.rs` | `ZipLoad` struct, `zip_defaults_for_class()` table, `reactive_only()` constructor |
| `crates/hares-equipment/src/config.rs` | `resolve_zip()`, `resolve_reactive_zip()`, `validate_zip_sums()`, `EquipmentConfig.zip` sidecar |
| `crates/hares-equipment/src/battery/config.rs` | `BatteryConfig.power_factor`, `BatteryConfig.inverter_capacity_kva` |
| `crates/hares-equipment/src/ev/config.rs` | `EvConfig.power_factor`, `EvConfig.charger_capacity_kva` |
| `crates/hares-equipment/src/ev/mod.rs` | `Ev::compute_reactive_kvar()`, `Ev::apply_control_unchecked()` |
| `defaults/zip_parameters.toml` | TOML mirror of class table (drift-tested against `zip_defaults_for_class()`) |
