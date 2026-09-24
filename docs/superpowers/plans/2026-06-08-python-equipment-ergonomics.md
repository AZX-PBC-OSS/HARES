# Python Equipment Ergonomics Companion Plan

> **Depends on:** `2026-06-08-hpxml-equipment-swapping.md` — this plan replaces/corrects Task 4, Task 5, and Task 6's `*_config_from_py()` functions.

**Goal:** Integrate autosizing into the Python equipment API so omitting capacity automatically sizes from the building envelope + weather, matching HPXML behavior.

**Architecture:** Each `*_spec_from_py()` returns an `EquipmentSpec` directly (not `EquipmentConfig`). When `autosize=True` and capacity is omitted, the spec carries `autosize_*` flags in `parameters` and `typed_config = None` for HVAC or `typed_config = Some(placeholder)` for WH. The autosizer computes capacity, patches params, and (for HVAC) calls `rebuild_hvac_typed_config` to rebuild the typed config. When `autosize=False`, capacity is required and a typed config is built directly.

---

## How Autosizing Works (reference)

The autosizer in `autosize.rs` checks `spec.parameters` for flags:

```
spec.parameters["autosize_heating"] = true   → run HVAC heating sizing (ACCA Manual S 1.4×)
spec.parameters["autosize_cooling"] = true   → run HVAC cooling sizing (ACCA Manual S 1.15×)
spec.parameters["autosize_water_heater"] = true → run WH tank/capacity sizing
```

Optional override keys consumed by the autosizer:
```
autosize_heating_factor: f64    — oversizing factor (default 1.4)
autosize_heating_min_w: f64     — floor clamp
autosize_heating_max_w: f64     — ceiling clamp
autosize_cooling_factor: f64    — (default 1.15)
autosize_cooling_min_w: f64
autosize_cooling_max_w: f64
autosize_water_heater_factor: f64
autosize_water_heater_min_w: f64
autosize_water_heater_max_w: f64
```

After autosizing, the computed capacity is injected as `heating_capacity_w` / `cooling_capacity_w` into `spec.parameters` and the autosizing flags are removed. `equipment_config_from_spec()` then builds the final `EquipmentConfig` from the now-complete parameters map.

**Critical:** The autosizer operates on `spec.parameters` only. It does not require a typed config. The typed config is constructed later from the autosized parameters.

---

## Design

### Python class signature pattern

```python
# Autosize — capacity computed from envelope
furnace = GasFurnace("Furnace", autosize=True, afue=0.95)

# Explicit capacity
furnace = GasFurnace("Furnace", capacity_w=12000, afue=0.95)

# Error (no capacity and no autosizing)
furnace = GasFurnace("Furnace", afue=0.95)
# ValueError: "capacity_w is required unless autosize=True"
```

### Internal: `*_config_from_py()` returns `EquipmentSpec`

Each conversion function builds an `EquipmentSpec` with:
- `name`: canonical equipment name ("Gas Furnace", "ASHP Heater", etc.)
- `parameters`: all user-provided fields as `serde_json::Map` + autosizing flags
- `typed_config`: `Some(EquipmentConfig::from_typed(...))` only when `autosize=False` and capacity is known; `None` when `autosize=True`
- `fuel_type`: derived from equipment type

The `py_any_to_equipment_spec()` dispatch in `py_blueprint.rs` just calls the conversion function and returns the spec directly — no roundtrip through `build_typed_spec` / `require_typed`.

---

### Task E1: Rewrite PyGasFurnace with autosizing support

**Files:**
- Modify: `crates/hares-python/src/py_hvac.rs` (replace PyGasFurnace section)

- [ ] **Step 1: PyGasFurnace with autosize kwarg**

```rust
const BTU_PER_HR_PER_W: f64 = 3.412_141_633;

#[pyclass(name = "GasFurnace")]
#[derive(Debug, Clone)]
pub struct PyGasFurnace {
    #[pyo3(get)] pub name: String,
    #[pyo3(get)] pub autosize: bool,
    #[pyo3(get)] pub capacity_w: Option<f64>,
    #[pyo3(get)] pub afue: Option<f64>,
    #[pyo3(get)] pub zone_id: Option<u16>,
    #[pyo3(get)] pub fan_power_w: Option<f64>,
    #[pyo3(get)] pub number_of_speeds: Option<u8>,
    #[pyo3(get)] pub heating_setpoint_c: Option<f64>,
    #[pyo3(get)] pub cooling_setpoint_c: Option<f64>,
    #[pyo3(get)] pub oversizing_factor: Option<f64>,
}

#[pymethods]
impl PyGasFurnace {
    #[new]
    #[pyo3(signature = (name, autosize=false, capacity_w=None, afue=None,
                        zone_id=None, fan_power_w=None, number_of_speeds=None,
                        heating_setpoint_c=None, cooling_setpoint_c=None,
                        oversizing_factor=None))]
    fn new(
        name: String,
        autosize: bool,
        capacity_w: Option<f64>,
        afue: Option<f64>,
        zone_id: Option<u16>,
        fan_power_w: Option<f64>,
        number_of_speeds: Option<u8>,
        heating_setpoint_c: Option<f64>,
        cooling_setpoint_c: Option<f64>,
        oversizing_factor: Option<f64>,
    ) -> PyResult<Self> {
        if !autosize && capacity_w.is_none() {
            return Err(PyValueError::new_err(
                "GasFurnace: capacity_w is required unless autosize=True"
            ));
        }
        if let Some(v) = capacity_w {
            if !v.is_finite() || v <= 0.0 {
                return Err(PyValueError::new_err(
                    "GasFurnace: capacity_w must be finite and > 0"
                ));
            }
        }
        Ok(Self { name, autosize, capacity_w, afue, zone_id,
             fan_power_w, number_of_speeds, heating_setpoint_c,
             cooling_setpoint_c, oversizing_factor })
    }
}

pub(crate) fn gas_furnace_spec_from_py(f: &PyGasFurnace) -> EquipmentSpec {
    use serde_json::{Map, Value, json};
    let mut params: Map<String, Value> = Map::new();

    if f.autosize {
        params.insert("autosize_heating".into(), json!(true));
    } else if let Some(cap) = f.capacity_w {
        params.insert("capacity_w".into(), json!(cap));
    }
    if let Some(v) = f.afue { params.insert("afue".into(), json!(v)); }
    if let Some(v) = f.zone_id { params.insert("zone_id".into(), json!(v)); }
    if let Some(v) = f.fan_power_w { params.insert("fan_power_w".into(), json!(v)); }
    if let Some(v) = f.number_of_speeds { params.insert("number_of_speeds".into(), json!(v)); }
    if let Some(v) = f.heating_setpoint_c { params.insert("heating_setpoint_c".into(), json!(v)); }
    if let Some(v) = f.cooling_setpoint_c { params.insert("cooling_setpoint_c".into(), json!(v)); }
    if let Some(v) = f.oversizing_factor {
        params.insert("autosize_heating_factor".into(), json!(v));
    }

    let typed = if !f.autosize {
        // Build typed config from explicit params
        let cfg = GasFurnaceConfig {
            equipment_id: None,
            zone_id: f.zone_id,
            capacity_w: f.capacity_w.unwrap(), // validated in __new__
            afue: f.afue.unwrap_or(0.80),
            fan_power_w: f.fan_power_w,
            number_of_speeds: f.number_of_speeds.unwrap_or(1),
            stage_heating_capacities_w: None,
            stage_heating_eirs: None,
            setpoint: HvacSetpointConfig {
                heating_setpoint_c: f.heating_setpoint_c,
                heating_setpoint_source: None,
                cooling_setpoint_c: f.cooling_setpoint_c,
                cooling_setpoint_source: None,
            },
            ducts: DuctConfig::default(),
        };
        Some(EquipmentConfig::from_typed(
            f.name.clone(), "Gas Furnace".into(), cfg))
    } else {
        None
    };

    EquipmentSpec {
        name: "Gas Furnace".to_string(),
        instance_name: Some(f.name.clone()),
        fuel_type: FuelType::Gas,
        parameters: params,
        zip_params: None,
        typed_config: typed,
        system_id: None,
        related_hvac_idref: None,
        primary_role: None,
    }
}
```

- [ ] **Step 2: Commit**

```bash
git add crates/hares-python/src/py_hvac.rs
git commit -m "feat: integrate autosizing into PyGasFurnace via EquipmentSpec pattern"
```

---

### Task E2: Rewrite PyAirConditioner with autosizing

- [ ] **Step 1: PyAirConditioner with autosize + seer convenience**

```rust
#[pyclass(name = "AirConditioner")]
#[derive(Debug, Clone)]
pub struct PyAirConditioner {
    #[pyo3(get)] pub name: String,
    #[pyo3(get)] pub autosize: bool,
    #[pyo3(get)] pub capacity_w: Option<f64>,
    #[pyo3(get)] pub eir: Option<f64>,
    #[pyo3(get)] pub seer: Option<f64>,
    #[pyo3(get)] pub zone_id: Option<u16>,
    #[pyo3(get)] pub shr: Option<f64>,
    #[pyo3(get)] pub number_of_speeds: Option<u8>,
    #[pyo3(get)] pub fan_power_w: Option<f64>,
    #[pyo3(get)] pub heating_setpoint_c: Option<f64>,
    #[pyo3(get)] pub cooling_setpoint_c: Option<f64>,
    #[pyo3(get)] pub oversizing_factor: Option<f64>,
}

#[pymethods]
impl PyAirConditioner {
    #[new]
    #[pyo3(signature = (name, autosize=false, capacity_w=None, eir=None,
                        seer=None, zone_id=None, shr=None, number_of_speeds=None,
                        fan_power_w=None, heating_setpoint_c=None,
                        cooling_setpoint_c=None, oversizing_factor=None))]
    fn new(
        name: String,
        autosize: bool,
        capacity_w: Option<f64>,
        eir: Option<f64>,
        seer: Option<f64>,
        zone_id: Option<u16>,
        shr: Option<f64>,
        number_of_speeds: Option<u8>,
        fan_power_w: Option<f64>,
        heating_setpoint_c: Option<f64>,
        cooling_setpoint_c: Option<f64>,
        oversizing_factor: Option<f64>,
    ) -> PyResult<Self> {
        if !autosize && capacity_w.is_none() {
            return Err(PyValueError::new_err(
                "AirConditioner: capacity_w is required unless autosize=True"
            ));
        }
        Ok(Self { name, autosize, capacity_w, eir, seer, zone_id,
             shr, number_of_speeds, fan_power_w,
             heating_setpoint_c, cooling_setpoint_c, oversizing_factor })
    }
}

pub(crate) fn ac_spec_from_py(ac: &PyAirConditioner) -> EquipmentSpec {
    let mut params: Map<String, Value> = Map::new();

    if ac.autosize {
        params.insert("autosize_cooling".into(), json!(true));
    } else if let Some(cap) = ac.capacity_w {
        params.insert("capacity_w".into(), json!(cap));
    }

    // EIR: prefer explicit eir, else derive from seer, else default
    let eir = ac.eir.or_else(|| {
        ac.seer.map(|s| BTU_PER_HR_PER_W / s.max(1e-6))
    }).unwrap_or(BTU_PER_HR_PER_W / 14.0_f64.max(1e-6));
    params.insert("eir".into(), json!(eir));

    if let Some(v) = ac.zone_id { params.insert("zone_id".into(), json!(v)); }
    if let Some(v) = ac.shr { params.insert("shr".into(), json!(v)); }
    if let Some(v) = ac.number_of_speeds { params.insert("number_of_speeds".into(), json!(v)); }
    if let Some(v) = ac.fan_power_w { params.insert("fan_power_w".into(), json!(v)); }
    if let Some(v) = ac.heating_setpoint_c { params.insert("heating_setpoint_c".into(), json!(v)); }
    if let Some(v) = ac.cooling_setpoint_c { params.insert("cooling_setpoint_c".into(), json!(v)); }
    if let Some(v) = ac.oversizing_factor {
        params.insert("autosize_cooling_factor".into(), json!(v));
    }

    let typed = if !ac.autosize {
        let cfg = CentralAirConditionerConfig {
            equipment_id: None, zone_id: ac.zone_id,
            capacity_w: ac.capacity_w.unwrap(),
            eir,
            shr: ac.shr,
            number_of_speeds: ac.number_of_speeds.unwrap_or(1),
            stage_capacities_w: None, stage_eirs: None, stage_shrs: None,
            fan_power_w: ac.fan_power_w, fan_power_w_per_cfm: None,
            setpoint: HvacSetpointConfig {
                heating_setpoint_c: ac.heating_setpoint_c,
                cooling_setpoint_c: ac.cooling_setpoint_c,
                ..Default::default()
            },
            hysteresis_c: None, airflow_m3_s_per_w: None,
            fraction_load_served: None,
            crankcase_heater_kw: None, crankcase_heater_threshold_c: None,
            crankcase_capacity_curve_coeffs: None,
            duct: DuctConfig::default(), system_type: None, startup_cd: None,
            biquadratic_x1_min: None, biquadratic_x1_max: None,
            biquadratic_x2_min: None, biquadratic_x2_max: None,
            ff_min: None, ff_max: None, plf_min: None, plf_max: None,
            charge_defect_ratio: None, min_oat_compressor_cooling_c: None,
        };
        Some(EquipmentConfig::from_typed(ac.name.clone(), "Air Conditioner".into(), cfg))
    } else {
        None
    };

    EquipmentSpec {
        name: "Air Conditioner".to_string(),
        instance_name: Some(ac.name.clone()),
        fuel_type: FuelType::Electric,
        parameters: params,
        zip_params: None,
        typed_config: typed,
        system_id: None,
        related_hvac_idref: None,
        primary_role: None,
    }
}
```

- [ ] **Step 2: Commit**

```bash
git add crates/hares-python/src/py_hvac.rs
git commit -m "feat: integrate autosizing into PyAirConditioner"
```

---

### Task E3: Rewrite PyASHPHeater with autosizing

- [ ] **Step 1: PyASHPHeater — autosize + HSPF→EIR**

```rust
#[pyclass(name = "ASHPHeater")]
#[derive(Debug, Clone)]
pub struct PyASHPHeater {
    #[pyo3(get)] pub name: String,
    #[pyo3(get)] pub autosize: bool,
    #[pyo3(get)] pub capacity_w: Option<f64>,
    #[pyo3(get)] pub hspf: Option<f64>,
    #[pyo3(get)] pub zone_id: Option<u16>,
    #[pyo3(get)] pub is_mini_split: Option<bool>,
    #[pyo3(get)] pub backup_capacity_w: Option<f64>,
    #[pyo3(get)] pub backup_fuel: Option<String>,
    #[pyo3(get)] pub fan_power_w: Option<f64>,
    #[pyo3(get)] pub heating_setpoint_c: Option<f64>,
    #[pyo3(get)] pub cooling_setpoint_c: Option<f64>,
    #[pyo3(get)] pub oversizing_factor: Option<f64>,
}

#[pymethods]
impl PyASHPHeater {
    #[new]
    #[pyo3(signature = (name, autosize=false, capacity_w=None, hspf=None,
                        zone_id=None, is_mini_split=None,
                        backup_capacity_w=None, backup_fuel=None,
                        fan_power_w=None, heating_setpoint_c=None,
                        cooling_setpoint_c=None, oversizing_factor=None))]
    fn new(
        name: String,
        autosize: bool,
        capacity_w: Option<f64>,
        hspf: Option<f64>,
        zone_id: Option<u16>,
        is_mini_split: Option<bool>,
        backup_capacity_w: Option<f64>,
        backup_fuel: Option<String>,
        fan_power_w: Option<f64>,
        heating_setpoint_c: Option<f64>,
        cooling_setpoint_c: Option<f64>,
        oversizing_factor: Option<f64>,
    ) -> PyResult<Self> {
        if !autosize && capacity_w.is_none() {
            return Err(PyValueError::new_err(
                "ASHPHeater: capacity_w is required unless autosize=True"
            ));
        }
        Ok(Self { name, autosize, capacity_w, hspf, zone_id,
             is_mini_split, backup_capacity_w, backup_fuel,
             fan_power_w, heating_setpoint_c, cooling_setpoint_c,
             oversizing_factor })
    }
}

pub(crate) fn ashp_heater_spec_from_py(hp: &PyASHPHeater) -> EquipmentSpec {
    let mut params: Map<String, Value> = Map::new();
    let hspf = hp.hspf.unwrap_or(8.2);
    let heating_eir = BTU_PER_HR_PER_W / hspf.max(1e-6);
    let is_mini = hp.is_mini_split.unwrap_or(false);

    if hp.autosize {
        params.insert("autosize_heating".into(), json!(true));
    } else if let Some(cap) = hp.capacity_w {
        params.insert("heating_capacity_w".into(), json!(cap));
    }
    params.insert("heating_eir".into(), json!(heating_eir));
    params.insert("hspf".into(), json!(hspf));
    params.insert("is_mini_split".into(), json!(is_mini));

    if let Some(v) = hp.zone_id { params.insert("zone_id".into(), json!(v)); }
    if let Some(v) = hp.backup_capacity_w { params.insert("backup_capacity_w".into(), json!(v)); }
    if let Some(ref v) = hp.backup_fuel { params.insert("backup_fuel".into(), json!(v)); }
    if let Some(v) = hp.fan_power_w { params.insert("fan_power_w".into(), json!(v)); }
    if let Some(v) = hp.heating_setpoint_c { params.insert("heating_setpoint_c".into(), json!(v)); }
    if let Some(v) = hp.cooling_setpoint_c { params.insert("cooling_setpoint_c".into(), json!(v)); }
    if let Some(v) = hp.oversizing_factor {
        params.insert("autosize_heating_factor".into(), json!(v));
    }

    let typed = if !hp.autosize {
        let backup_fuel = hp.backup_fuel.as_deref().map(|s| match s.to_lowercase().as_str() {
            "gas" | "natural gas" => FuelType::Gas,
            "propane" => FuelType::Propane,
            "oil" | "fuel oil" => FuelType::Oil,
            _ => FuelType::Electric,
        });
        let common = HeatPumpCommonConfig {
            zone_id: hp.zone_id,
            heating_capacity_w: hp.capacity_w,
            heating_eir: Some(heating_eir),
            backup_capacity_w: hp.backup_capacity_w,
            backup_eir: hp.backup_capacity_w.map(|_| 1.0),
            backup_fuel,
            is_mini_split: is_mini,
            fan_power_w: hp.fan_power_w,
            setpoint: HvacSetpointConfig {
                heating_setpoint_c: hp.heating_setpoint_c,
                cooling_setpoint_c: hp.cooling_setpoint_c,
                ..Default::default()
            },
            ..Default::default()
        };
        let cfg = HeatPumpHeaterConfig { common, ..Default::default() };
        Some(EquipmentConfig::from_typed(hp.name.clone(), "ASHP Heater".into(), cfg))
    } else {
        None
    };

    EquipmentSpec {
        name: "ASHP Heater".to_string(),
        instance_name: Some(hp.name.clone()),
        fuel_type: FuelType::Electric,
        parameters: params,
        zip_params: None,
        typed_config: typed,
        system_id: None, related_hvac_idref: None, primary_role: None,
    }
}
```

- [ ] **Step 2: Commit**

```bash
git add crates/hares-python/src/py_hvac.rs
git commit -m "feat: integrate autosizing into PyASHPHeater"
```

---

### Task E4: Rewrite PyASHPCooler with autosizing

- [ ] **Step 1: PyASHPCooler spec builder**

```rust
#[pyclass(name = "ASHPCooler")]
#[derive(Debug, Clone)]
pub struct PyASHPCooler {
    #[pyo3(get)] pub name: String,
    #[pyo3(get)] pub autosize: bool,
    #[pyo3(get)] pub capacity_w: Option<f64>,
    #[pyo3(get)] pub seer: Option<f64>,
    #[pyo3(get)] pub zone_id: Option<u16>,
    #[pyo3(get)] pub is_mini_split: Option<bool>,
    #[pyo3(get)] pub shr: Option<f64>,
    #[pyo3(get)] pub fan_power_w: Option<f64>,
    #[pyo3(get)] pub heating_setpoint_c: Option<f64>,
    #[pyo3(get)] pub cooling_setpoint_c: Option<f64>,
    #[pyo3(get)] pub oversizing_factor: Option<f64>,
}

#[pymethods]
impl PyASHPCooler {
    #[new]
    #[pyo3(signature = (name, autosize=false, capacity_w=None, seer=None,
                        zone_id=None, is_mini_split=None, shr=None,
                        fan_power_w=None, heating_setpoint_c=None,
                        cooling_setpoint_c=None, oversizing_factor=None))]
    fn new(
        name: String,
        autosize: bool,
        capacity_w: Option<f64>,
        seer: Option<f64>,
        zone_id: Option<u16>,
        is_mini_split: Option<bool>,
        shr: Option<f64>,
        fan_power_w: Option<f64>,
        heating_setpoint_c: Option<f64>,
        cooling_setpoint_c: Option<f64>,
        oversizing_factor: Option<f64>,
    ) -> PyResult<Self> {
        if !autosize && capacity_w.is_none() {
            return Err(PyValueError::new_err(
                "ASHPCooler: capacity_w is required unless autosize=True"
            ));
        }
        Ok(Self { name, autosize, capacity_w, seer, zone_id,
             is_mini_split, shr, fan_power_w, heating_setpoint_c,
             cooling_setpoint_c, oversizing_factor })
    }
}

pub(crate) fn ashp_cooler_spec_from_py(hp: &PyASHPCooler) -> EquipmentSpec {
    let mut params: Map<String, Value> = Map::new();
    let seer = hp.seer.unwrap_or(14.0);
    let cooling_eir = BTU_PER_HR_PER_W / seer.max(1e-6);
    let is_mini = hp.is_mini_split.unwrap_or(false);

    if hp.autosize {
        params.insert("autosize_cooling".into(), json!(true));
    } else if let Some(cap) = hp.capacity_w {
        params.insert("cooling_capacity_w".into(), json!(cap));
    }
    params.insert("cooling_eir".into(), json!(cooling_eir));
    params.insert("seer".into(), json!(seer));
    params.insert("is_mini_split".into(), json!(is_mini));

    if let Some(v) = hp.zone_id { params.insert("zone_id".into(), json!(v)); }
    if let Some(v) = hp.shr { params.insert("shr".into(), json!(v)); }
    if let Some(v) = hp.fan_power_w { params.insert("fan_power_w".into(), json!(v)); }
    if let Some(v) = hp.heating_setpoint_c { params.insert("heating_setpoint_c".into(), json!(v)); }
    if let Some(v) = hp.cooling_setpoint_c { params.insert("cooling_setpoint_c".into(), json!(v)); }
    if let Some(v) = hp.oversizing_factor {
        params.insert("autosize_cooling_factor".into(), json!(v));
    }

    let typed = if !hp.autosize {
        let common = HeatPumpCommonConfig {
            zone_id: hp.zone_id,
            cooling_capacity_w: hp.capacity_w,
            cooling_eir: Some(cooling_eir),
            is_mini_split: is_mini,
            shr: hp.shr,
            fan_power_w: hp.fan_power_w,
            setpoint: HvacSetpointConfig {
                heating_setpoint_c: hp.heating_setpoint_c,
                cooling_setpoint_c: hp.cooling_setpoint_c,
                ..Default::default()
            },
            ..Default::default()
        };
        let cfg = HeatPumpCoolerConfig { common, ..Default::default() };
        Some(EquipmentConfig::from_typed(hp.name.clone(), "ASHP Cooler".into(), cfg))
    } else {
        None
    };

    EquipmentSpec {
        name: "ASHP Cooler".to_string(),
        instance_name: Some(hp.name.clone()),
        fuel_type: FuelType::Electric,
        parameters: params,
        zip_params: None,
        typed_config: typed,
        system_id: None, related_hvac_idref: None, primary_role: None,
    }
}
```

- [ ] **Step 2: Commit**

```bash
git add crates/hares-python/src/py_hvac.rs
git commit -m "feat: integrate autosizing into PyASHPCooler"
```

---

### Task E5: Rewrite PyElectricBaseboard and PyIdealHVAC

- [ ] **Step 1: PyElectricBaseboard with autosize**

```rust
#[pyclass(name = "ElectricBaseboard")]
#[derive(Debug, Clone)]
pub struct PyElectricBaseboard {
    #[pyo3(get)] pub name: String,
    #[pyo3(get)] pub autosize: bool,
    #[pyo3(get)] pub capacity_w: Option<f64>,
    #[pyo3(get)] pub eir: Option<f64>,
    #[pyo3(get)] pub zone_id: Option<u16>,
    #[pyo3(get)] pub heating_setpoint_c: Option<f64>,
    #[pyo3(get)] pub cooling_setpoint_c: Option<f64>,
    #[pyo3(get)] pub oversizing_factor: Option<f64>,
}

#[pymethods]
impl PyElectricBaseboard {
    #[new]
    #[pyo3(signature = (name, autosize=false, capacity_w=None, eir=None,
                        zone_id=None, heating_setpoint_c=None,
                        cooling_setpoint_c=None, oversizing_factor=None))]
    fn new(
        name: String, autosize: bool, capacity_w: Option<f64>,
        eir: Option<f64>, zone_id: Option<u16>,
        heating_setpoint_c: Option<f64>, cooling_setpoint_c: Option<f64>,
        oversizing_factor: Option<f64>,
    ) -> PyResult<Self> {
        if !autosize && capacity_w.is_none() {
            return Err(PyValueError::new_err(
                "ElectricBaseboard: capacity_w is required unless autosize=True"
            ));
        }
        Ok(Self { name, autosize, capacity_w, eir, zone_id,
             heating_setpoint_c, cooling_setpoint_c, oversizing_factor })
    }
}

pub(crate) fn baseboard_spec_from_py(bb: &PyElectricBaseboard) -> EquipmentSpec {
    let mut params: Map<String, Value> = Map::new();
    if bb.autosize {
        params.insert("autosize_heating".into(), json!(true));
    } else if let Some(cap) = bb.capacity_w {
        params.insert("capacity_w".into(), json!(cap));
    }
    params.insert("eir".into(), json!(bb.eir.unwrap_or(1.0)));
    if let Some(v) = bb.zone_id { params.insert("zone_id".into(), json!(v)); }
    if let Some(v) = bb.heating_setpoint_c { params.insert("heating_setpoint_c".into(), json!(v)); }
    if let Some(v) = bb.cooling_setpoint_c { params.insert("cooling_setpoint_c".into(), json!(v)); }
    if let Some(v) = bb.oversizing_factor {
        params.insert("autosize_heating_factor".into(), json!(v));
    }

    let typed = if !bb.autosize {
        let cfg = ElectricBaseboardConfig {
            equipment_id: None, zone_id: bb.zone_id,
            capacity_w: bb.capacity_w.unwrap(),
            eir: bb.eir.unwrap_or(1.0),
            setpoint: HvacSetpointConfig {
                heating_setpoint_c: bb.heating_setpoint_c,
                cooling_setpoint_c: bb.cooling_setpoint_c,
                ..Default::default()
            },
        };
        Some(EquipmentConfig::from_typed(bb.name.clone(), "Electric Baseboard".into(), cfg))
    } else {
        None
    };

    EquipmentSpec {
        name: "Electric Baseboard".to_string(),
        instance_name: Some(bb.name.clone()),
        fuel_type: FuelType::Electric,
        parameters: params, zip_params: None, typed_config: typed,
        system_id: None, related_hvac_idref: None, primary_role: None,
    }
}
```

**PyIdealHVAC** does not participate in autosizing (it's an ideal model — capacity is arbitrary). No change needed from the main plan. Keep its constructor without `autosize`.

- [ ] **Step 2: Commit**

```bash
git add crates/hares-python/src/py_hvac.rs
git commit -m "feat: integrate autosizing into PyElectricBaseboard"
```

---

### Task E6: Update water heater classes with autosizing

**Files:**
- Modify: `crates/hares-python/src/py_water_heater.rs`

- [ ] **Step 1: PyGasWaterHeater — autosize for both capacity and volume**

Water heater autosizing (`autosize_water_heater` flag) sizes both tank volume (by bedroom count) and heating capacity (by FHR methodology). `autosize=True` when either capacity OR volume is omitted.

```rust
#[pyclass(name = "GasWaterHeater")]
#[derive(Debug, Clone)]
pub struct PyGasWaterHeater {
    #[pyo3(get)] pub name: String,
    #[pyo3(get)] pub autosize: bool,
    #[pyo3(get)] pub tank_volume_m3: Option<f64>,
    #[pyo3(get)] pub uniform_energy_factor: Option<f64>,
    #[pyo3(get)] pub heating_capacity_w: Option<f64>,
    #[pyo3(get)] pub setpoint_c: Option<f64>,
    #[pyo3(get)] pub zone_id: Option<u16>,
    #[pyo3(get)] pub avg_water_draw_l_per_day: Option<f64>,
    #[pyo3(get)] pub oversizing_factor: Option<f64>,
}

#[pymethods]
impl PyGasWaterHeater {
    #[new]
    #[pyo3(signature = (name, autosize=false, tank_volume_m3=None,
                        uniform_energy_factor=None, heating_capacity_w=None,
                        setpoint_c=None, zone_id=None,
                        avg_water_draw_l_per_day=None,
                        oversizing_factor=None))]
    fn new(
        name: String, autosize: bool, tank_volume_m3: Option<f64>,
        uniform_energy_factor: Option<f64>, heating_capacity_w: Option<f64>,
        setpoint_c: Option<f64>, zone_id: Option<u16>,
        avg_water_draw_l_per_day: Option<f64>,
        oversizing_factor: Option<f64>,
    ) -> PyResult<Self> {
        let need_autosize = tank_volume_m3.is_none() || heating_capacity_w.is_none();
        if need_autosize && !autosize {
            return Err(PyValueError::new_err(
                "GasWaterHeater: tank_volume_m3 and heating_capacity_w are required unless autosize=True"
            ));
        }
        Ok(Self { name, autosize, tank_volume_m3, uniform_energy_factor,
             heating_capacity_w, setpoint_c, zone_id,
             avg_water_draw_l_per_day, oversizing_factor })
    }
}

pub(crate) fn gas_wh_spec_from_py(wh: &PyGasWaterHeater) -> EquipmentSpec {
    let mut params: Map<String, Value> = Map::new();
    params.insert("fuel_type".into(), json!("natural gas"));

    if wh.autosize {
        params.insert("autosize_water_heater".into(), json!(true));
    }
    if let Some(v) = wh.tank_volume_m3 { params.insert("tank_volume_m3".into(), json!(v)); }
    if let Some(v) = wh.uniform_energy_factor { params.insert("uniform_energy_factor".into(), json!(v)); }
    if let Some(v) = wh.heating_capacity_w { params.insert("heating_capacity_w".into(), json!(v)); }
    if let Some(v) = wh.setpoint_c { params.insert("setpoint_c".into(), json!(v)); }
    if let Some(v) = wh.zone_id { params.insert("zone_id".into(), json!(v)); }
    if let Some(v) = wh.avg_water_draw_l_per_day {
        params.insert("avg_water_draw_l_per_day".into(), json!(v));
    }
    if let Some(v) = wh.oversizing_factor {
        params.insert("autosize_water_heater_factor".into(), json!(v));
    }

    // Always build typed config even when autosizing — schedule injection
    // requires it for draw/mains column derivation. The autosizer patches
    // capacity_w and tank_volume_m3 into the typed config JSON at
    // autosize.rs:804 when `if let Some(ref mut tc) = spec.typed_config`.
    let cfg = GasWaterHeaterConfig {
        equipment_id: None, zone_id: wh.zone_id, loop_id: None,
        fuel_type: FuelType::Gas,
        tank_volume_m3: wh.tank_volume_m3.or(Some(0.19)), // placeholder; autosized
        tank_height_m: None,
        energy_factor: None,
        uniform_energy_factor: wh.uniform_energy_factor,
        heating_capacity_w: wh.heating_capacity_w.or(Some(0.0)), // placeholder
        ua_w_per_k: None,
        setpoint_c: wh.setpoint_c, deadband_c: None,
        max_tank_temp_c: None, initial_tank_temp_c: None,
        tank_nodes: None,
        avg_water_draw_l_per_day: wh.avg_water_draw_l_per_day,
        draw_flow_rate_kg_s: None, draw_flow_rate_source: None,
        mains_temp_c_source: None, pilot_power_w: None,
        flue_loss_fraction: None, skin_loss_fraction: None,
        ignition_type: None, performance_adjustment: None,
        zone_type: None, first_hour_rating_m3: None,
        jacket_r_value_m2_k_w: None, conversion_efficiency: None,
        fixture_delivery_temp_c: None, hot_draw_temp_c: None,
        pilot_fraction_to_tank: None,
    };
    let typed = Some(EquipmentConfig::from_typed(
        wh.name.clone(), "Gas Water Heater".into(), cfg));

    EquipmentSpec {
        name: "Gas Water Heater".to_string(),
        instance_name: Some(wh.name.clone()),
        fuel_type: FuelType::Gas,
        parameters: params, zip_params: None, typed_config: typed,
        system_id: None, related_hvac_idref: None, primary_role: None,
    }
}
```

- [ ] **Step 2: Same pattern for PyElectricResistanceWH and PyHeatPumpWH**

> **IMPORTANT for all water heaters:** Always build `typed_config = Some(...)` even when
> `autosize=True`. Use placeholder values (`0.0` for capacity, `0.19` for volume).
> The autosizer patches `heating_capacity_w`/`backup_element_power_w`/`tank_volume_m3`
> into the typed config JSON at `autosize.rs:804`. Schedule injection
> (`inject_water_heater_schedule_columns`) panics if `typed_config` is `None`.
>
> For `PyElectricResistanceWH`: same fields as PyGasWaterHeater, canonical name
> `"Electric Resistance Water Heater"`, fuel type `FuelType::Electric`.
> For `PyHeatPumpWH`: capacity goes into `backup_element_power_w` (placeholder `0.0`),
> no `heating_capacity_w` field on this struct. Use placeholder `cop: Some(3.5)`.

- [ ] **Step 3: Commit**

```bash
git add crates/hares-python/src/py_water_heater.rs
git commit -m "feat: integrate autosizing into water heater classes"
```

---

### Task E9: DRY up spec construction with a shared Rust macro

The 8 `*_spec_from_py()` functions share a common pattern: validate autosize/capacity, build a `params` map, conditionally build a typed config, assemble `EquipmentSpec`. A declarative macro eliminates the boilerplate.

**Files:**
- Create: `crates/hares-python/src/equipment_spec_macros.rs`

- [ ] **Step 1: Define `equipment_spec!` macro**

```rust
// crates/hares-python/src/equipment_spec_macros.rs

/// Declarative macro for building an EquipmentSpec from a Python config object.
///
/// Usage pattern:
/// ```ignore
/// equipment_spec!(
///     obj: &self.py_obj,
///     canonical: "Gas Furnace",
///     fuel: FuelType::Gas,
///     autosize_field: autosize,
///     capacity_field: capacity_w,
///     autosize_flag: "autosize_heating",
///     params: { ... },
///     typed_config: { ... },
/// )
/// ```
macro_rules! equipment_spec {
    (
        obj: $obj:expr,
        canonical: $canonical:expr,
        fuel: $fuel:expr,
        autosize_field: $autosize:expr,
        capacity_field: $capacity:expr,
        autosize_flag: $flag:expr,
        oversizing_factor: $oversize:expr,
        params: { $($param_key:expr => $param_val:expr),* $(,)? },
        typed_config: { $($tc_field:ident: $tc_val:expr),* $(,)? },
    ) => {{
        let mut params: serde_json::Map<String, serde_json::Value> =
            serde_json::Map::new();

        if $autosize {
            params.insert($flag.into(), serde_json::json!(true));
        } else {
            params.insert("capacity_w".into(), serde_json::json!($capacity));
        }

        $( params.insert($param_key.into(), serde_json::json!($param_val)); )*
        if let Some(v) = $oversize {
            params.insert(
                concat!($flag, "_factor").into(),
                serde_json::json!(v),
            );
        }

        let typed: Option<hares_equipment::EquipmentConfig> = if !$autosize {
            let cfg = { $($tc_field: $tc_val,)* };
            Some(hares_equipment::EquipmentConfig::from_typed(
                $obj.name.clone(),
                $canonical.into(),
                cfg,
            ))
        } else {
            None
        };

        hares_io::hpxml::EquipmentSpec {
            name: $canonical.to_string(),
            instance_name: Some($obj.name.clone()),
            fuel_type: $fuel,
            parameters: params,
            zip_params: None,
            typed_config: typed,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        }
    }};
}

// Same pattern but always produces typed_config (for WH equipment)
macro_rules! equipment_spec_always_typed {
    // ... similar, typed always Some(...)
}
```

Each `*_spec_from_py()` then becomes a thin wrapper:

```rust
pub(crate) fn gas_furnace_spec_from_py(f: &PyGasFurnace) -> EquipmentSpec {
    equipment_spec!(
        obj: f,
        canonical: "Gas Furnace",
        fuel: FuelType::Gas,
        autosize_field: f.autosize,
        capacity_field: f.capacity_w.unwrap_or(0.0),
        autosize_flag: "autosize_heating",
        oversizing_factor: f.oversizing_factor,
        params: {
            "afue" => f.afue.unwrap_or(0.80),
            "zone_id" => f.zone_id,
            "fan_power_w" => f.fan_power_w,
            "number_of_speeds" => f.number_of_speeds.unwrap_or(1),
            "heating_setpoint_c" => f.heating_setpoint_c,
            "cooling_setpoint_c" => f.cooling_setpoint_c,
        },
        typed_config: {
            equipment_id: None::<u32>,
            zone_id: f.zone_id,
            capacity_w: f.capacity_w.unwrap(),
            afue: f.afue.unwrap_or(0.80),
            fan_power_w: f.fan_power_w,
            number_of_speeds: f.number_of_speeds.unwrap_or(1),
            stage_heating_capacities_w: None::<Vec<f64>>,
            stage_heating_eirs: None::<Vec<f64>>,
            setpoint: HvacSetpointConfig {
                heating_setpoint_c: f.heating_setpoint_c,
                heating_setpoint_source: None::<ScheduleSourceConfig>,
                cooling_setpoint_c: f.cooling_setpoint_c,
                cooling_setpoint_source: None::<ScheduleSourceConfig>,
            },
            ducts: DuctConfig::default(),
        },
    )
}
```

- [ ] **Step 2: Replace all 8 manual `*_spec_from_py()` bodies with macro invocations**

- [ ] **Step 3: Commit**

```bash
git add crates/hares-python/src/equipment_spec_macros.rs crates/hares-python/src/py_hvac.rs crates/hares-python/src/py_water_heater.rs
git commit -m "refactor: DRY up spec construction with equipment_spec! macro"
```

---

### Task E7: Simplify py_blueprint.rs dispatch

**Files:**
- Modify: `crates/hares-python/src/py_blueprint.rs`

- [ ] **Step 1: Rewrite py_any_to_equipment_spec() — direct spec return**

Replace the old `py_any_to_equipment_spec` that did `config → build_equipment_spec_from_typed`. New function just calls `*_spec_from_py()` and returns:

```rust
fn py_any_to_equipment_spec(obj: &Bound<'_, PyAny>) -> PyResult<EquipmentSpec> {
    if let Ok(f) = obj.extract::<PyRef<'_, PyGasFurnace>>() {
        return Ok(gas_furnace_spec_from_py(&f));
    }
    if let Ok(ac) = obj.extract::<PyRef<'_, PyAirConditioner>>() {
        return Ok(ac_spec_from_py(&ac));
    }
    if let Ok(hp) = obj.extract::<PyRef<'_, PyASHPHeater>>() {
        return Ok(ashp_heater_spec_from_py(&hp));
    }
    if let Ok(hp) = obj.extract::<PyRef<'_, PyASHPCooler>>() {
        return Ok(ashp_cooler_spec_from_py(&hp));
    }
    if let Ok(hvac) = obj.extract::<PyRef<'_, PyIdealHVAC>>() {
        return Ok(ideal_hvac_spec_from_py(&hvac));
    }
    if let Ok(bb) = obj.extract::<PyRef<'_, PyElectricBaseboard>>() {
        return Ok(baseboard_spec_from_py(&bb));
    }
    if let Ok(wh) = obj.extract::<PyRef<'_, PyGasWaterHeater>>() {
        return Ok(gas_wh_spec_from_py(&wh));
    }
    if let Ok(wh) = obj.extract::<PyRef<'_, PyElectricResistanceWH>>() {
        return Ok(elec_res_wh_spec_from_py(&wh));
    }
    if let Ok(wh) = obj.extract::<PyRef<'_, PyHeatPumpWH>>() {
        return Ok(hpwh_spec_from_py(&wh));
    }
    Err(PyValueError::new_err(
        "equipment must be a typed HVAC or water heater config object. \
         Note: Battery, PV, and EV are added post-build via Dwelling.add_battery() etc. \
         — they are NOT valid for DwellingBlueprint.add_equipment(). \
         See DwellingBlueprint vs Dwelling phase distinction in module docs."
    ))
}
```

No more `build_equipment_spec_from_typed`, no more `DefaultsStore::empty()`, no more `require_typed::<T>()` roundtrip. Each `*_spec_from_py()` owns the spec construction entirely.

- [ ] **Step 2: Remove dead code from py_blueprint.rs**

Delete `build_equipment_spec_from_typed()` helper function — no longer needed.

- [ ] **Step 3: Commit**

```bash
git add crates/hares-python/src/py_blueprint.rs
git commit -m "refactor: simplify py_blueprint dispatch to use direct spec-from-py pattern"
```

---

### Task E8: Update test file for autosizing syntax

**Files:**
- Modify: `tests/python/test_blueprint_swap.py`
- Modify: `tests/python/test_blueprint_errors.py`

- [ ] **Step 1: Update swap tests to use autosize syntax**

Replace `capacity_w=12000` with explicit kwarg or autosize:

```python
# Explicit capacity (unchanged pattern)
ashp = ASHPHeater("ASHP", capacity_w=12000, hspf=9.5)

# Autosizing — no capacity kwarg
ashp_auto = ASHPHeater("ASHP Auto", autosize=True, hspf=9.5)

# AC with seer convenience
ac = AirConditioner("New AC", capacity_w=10000, seer=16.0)

# Autosized AC
ac_auto = AirConditioner("Auto AC", autosize=True, seer=18.0)

# HPWH with autosize (omits tank_volume and capacity)
hpwh_auto = HeatPumpWH("HPWH Auto", autosize=True, cop=3.5,
                       avg_water_draw_l_per_day=200.0)
```

- [ ] **Step 2: Add autosizing-specific tests**

```python
def test_autosize_heating():
    """Autosizing a furnace should produce non-zero capacity."""
    bp = _make_blueprint()
    bp.remove_equipment_by_end_use(EndUse.HVAC_HEATING)

    furnace = GasFurnace("AutoFurnace", autosize=True, afue=0.95)
    bp.add_equipment(furnace)

    dw = bp.build()
    dw.initialize()
    assert "AutoFurnace" in dw.equipment_names()

    for _ in range(5):
        result = dw.step()
        # With autosized heating, net electric power should be finite
        assert result is not None
        power = result["net_electric_power_kw"]
        assert abs(power) < 1e9  # not NaN, not infinite


def test_autosize_water_heater():
    """Autosizing a water heater should produce non-zero capacity."""
    bp = _make_blueprint()
    bp.remove_equipment_by_end_use(EndUse.WATER_HEATING)

    wh = GasWaterHeater("AutoWH", autosize=True, uniform_energy_factor=0.65,
                        avg_water_draw_l_per_day=200.0)
    bp.add_equipment(wh)

    dw = bp.build()
    dw.initialize()

    for _ in range(5):
        dw.step()


def test_missing_capacity_raises():
    """Omitting capacity without autosize=True raises ValueError."""
    with pytest.raises(ValueError, match="capacity_w is required unless autosize=True"):
        GasFurnace("NoCap Furance")


def test_autosize_ashp_heater():
    """Autosized ASHP heater produces valid simulation."""
    bp = _make_blueprint()
    bp.remove_equipment_by_end_use(EndUse.HVAC_HEATING)

    ashp = ASHPHeater("AutoASHP", autosize=True, hspf=9.5)
    bp.add_equipment(ashp)

    dw = bp.build()
    dw.initialize()

    for _ in range(5):
        dw.step()
```

- [ ] **Step 3: Run tests**

```bash
uv run pytest tests/python/test_blueprint_swap.py tests/python/test_blueprint_errors.py -v
```

- [ ] **Step 4: Commit**

```bash
git add tests/python/test_blueprint_swap.py tests/python/test_blueprint_errors.py
git commit -m "test: add autosizing tests for HVAC and water heaters"
```

---

## Updated File Map (replaces Task 4-5-6 from main plan)

```
MODIFIED (replacing previous versions):
  crates/hares-python/src/py_hvac.rs        — PyGasFurnace, PyAirConditioner, PyASHPHeater,
                                             PyASHPCooler, PyElectricBaseboard, PyIdealHVAC
                                             All *_spec_from_py() return EquipmentSpec
  crates/hares-python/src/py_water_heater.rs — PyGasWaterHeater, PyElectricResistanceWH,
                                             PyHeatPumpWH. All *_spec_from_py()
  crates/hares-python/src/py_blueprint.rs    — Simplified py_any_to_equipment_spec(),
                                             no dead helpers

ADDED tests:
  tests/python/test_blueprint_swap.py        — autosize=True test cases
  tests/python/test_blueprint_errors.py      — test_missing_capacity_raises
```

---

## Self-Review

- [x] `autosize=True` sets `autosize_heating`/`autosize_cooling`/`autosize_water_heater` in `spec.parameters`
- [x] `autosize=False` and no capacity → `ValueError`
- [x] When autosizing, `spec.typed_config` carries placeholder values for WH (required by schedule injection); autosizer patches JSON data in-place
- [x] When explicit capacity, `spec.typed_config = Some(...)` — normal path
- [x] No `unwrap_or(0.0)` producing silent 0W equipment
- [x] `debug_assert!` in build verifying all WH specs have `typed_config.is_some()` before schedule injection

**Runtime invariant:** Add in the main plan's `build_from_blueprint()`:

```rust
#[cfg(debug_assertions)]
for spec in &bp.equipment_specs {
    if matches!(spec.name.as_str(), "Electric Resistance Water Heater"
        | "Gas Water Heater" | "Heat Pump Water Heater"
        | "Tankless Water Heater" | "Gas Tankless Water Heater")
    {
        assert!(spec.typed_config.is_some(),
            "WH spec '{}' has no typed_config — schedule injection will panic", spec.name);
    }
}
```
- [x] `equipment_config_from_spec` handles both paths: typed→typed clone, None→raw from params
- [x] Oversizing factor overridable via `oversizing_factor` kwarg → `autosize_heating_factor` etc.
- [x] SEER→EIR derivation uses `BTU_PER_HR_PER_W / seer.max(1e-6)` = `3.412 / seer`
- [x] HSPF→EIR derivation uses same formula
- [x] Heat pump backup fuel is user-configurable, not hardcoded Electric
- [x] Water heater autosizing triggered when either volume or capacity is absent
- [x] No duplicate spec construction — each `*_spec_from_py()` builds the spec directly

**Duplicate equipment behavior:** `DwellingBlueprint::add_equipment_spec()` pushes to the vec without checking for duplicates. `Dwelling::add_equipment()` (called during build) renames duplicates using `"Name #N"` convention. Python `add_equipment()` docstring must document this: adding two `ASHPHeater("ASHP", ...)` objects will result in equipment named `"ASHP"` and `"ASHP #2"` in the final dwelling.

**Known constraint:** If adding an `extra: dict` kwargs escape hatch for fields not in the curated Python class, extra keys go to `spec.parameters` only, never to the typed config JSON. All typed configs use `#[serde(deny_unknown_fields)]` and will reject unknown keys at serialization time.

**Params key convention:** The companion plan inserts `"capacity_w"` into `spec.parameters` for explicit-capacity equipment. `rebuild_hvac_typed_config` reads `"heating_capacity_w"` from params. This is safe because (a) for autosizing, the autosizer inserts `"heating_capacity_w"`, and (b) for non-autosizing, the typed config is used directly — `rebuild_hvac_typed_config` only runs on autosized specs. Add a comment at both `*_spec_from_py()` functions and `rebuild_hvac_typed_config()` noting this asymmetry.

**HVAC vs WH typed_config asymmetry:** HVAC schedule injection (`inject_setpoint_schedules`) silently skips None typed_config (early-return). WH schedule injection (`inject_water_heater_schedule_columns`) panics on None. This is the reason the companion plan always creates typed_config for WH but allows None for HVAC during autosizing. Add a comment at `schedule_resolve.rs:644` (HVAC early-return) and `schedule_resolve.rs:748` (WH panic) cross-referencing this design decision.
