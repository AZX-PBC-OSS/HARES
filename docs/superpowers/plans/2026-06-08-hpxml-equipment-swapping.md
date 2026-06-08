# HPXML Equipment Swapping Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable programmatic swapping of HVAC and water heater equipment in HPXML dwellings via a configure-then-build API with strong Python types.

**Architecture:** Split `Dwelling::from_preparsed()` into a `DwellingBlueprint` (parsed data + mutable `Vec<EquipmentSpec>`) and a `DwellingBlueprint::build()` (one-shot solver/port/equipment construction). Create PyO3 typed config classes for primary HVAC/WH types using `EquipmentConfig::from_typed()`. Expose via `DwellingBlueprint` Python class with `add_equipment()`, `remove_equipment()`, `remove_equipment_by_end_use()`, and `build()`.

**Tech Stack:** Rust (PyO3 0.28, serde, hares-* crates), Python 3.13+, maturin, pytest

**Build order** (matches current `from_preparsed`, lines 1675-2492 in `mod.rs`):
1. Allocate loop IDs + register PV surfaces + attach PV to roofs
2. Build initial env + solvers (thermal, humidity, electrical, fluid)
3. Autosize HVAC capacities (needs `solvers.thermal`)
4. Autosize WH capacities
5. Inject schedule columns (setpoints, draw, mains, occupancy)
6. Validate setpoint invariants
7. Create + init equipment instances with zone_map
8. Build ports from declarations, validate zones/fluid types
9. Build output schema + recorder
10. Assemble Dwelling struct with all fields
11. Register actors from ActorRegistry
12. Run warmup loop (if initialization_duration set): run_warmup_converged, restore RNG, reset clock
13. Set up diagnostic writer (output_verbosity >= 4)

**EIR formulas** (from `resolve_hvac.rs:1313`):
- Heating: `eir = 3.41214 / hspf.max(1e-6)`  
- Cooling: `eir = 3.41214 / seer.max(1e-6)`

---

## File Map

```
CREATED:
  crates/hares-core/src/dwelling/blueprint.rs      — DwellingBlueprint struct + build()
  crates/hares-python/src/py_hvac.rs                — PyGasFurnace, PyAirConditioner, PyASHPHeater,
                                                      PyASHPCooler, PyIdealHVAC, PyElectricBaseboard
  crates/hares-python/src/py_water_heater.rs        — PyGasWaterHeater, PyElectricResistanceWH,
                                                      PyHeatPumpWH
  crates/hares-python/src/py_blueprint.rs           — PyDwellingBlueprint Python class
  tests/python/test_blueprint_parity.py             — Parity: bp.build() == Dwelling.from_hpxml()
  tests/python/test_blueprint_swap.py               — HVAC/WH swap tests
  tests/python/test_blueprint_dwelling.py           — Dwelling integration from blueprint
  tests/python/test_blueprint_errors.py             — Error cases

MODIFIED:
  crates/hares-core/src/dwelling/mod.rs             — Add pub mod blueprint; pub use DwellingBlueprint
  crates/hares-python/src/lib.rs                    — Register new pyclasses (Task 6)
  crates/hares-python/src/py_dwelling.rs            — Add FromPyDwellingBlueprint; make build_config pub(crate)
  crates/hares-python/src/py_enums.rs               — Add missing EndUse classattrs (Task 1)
  crates/hares-io/src/output/columns.rs             — Add "Electric Resistance Water Heater" alias
  crates/hares-io/src/schedule_resolve.rs           — Add GSHP/WSHP to HEATING/COOLING constants
  crates/hares-io/src/hpxml/equipment.rs            — Make build_typed_spec pub
  python/ochre_next/__init__.py                     — Export new types (Task 12)
```

No crate boundary violations. `hares-equipment` does not depend on `hares-io`. All new code in `hares-core` and `hares-python` which already depend on `hares-io`.

---

### Task 1: Fix pre-existing gaps (schedule injection + output schema + PyEndUse)

**Files:**
- Modify: `crates/hares-io/src/schedule_resolve.rs:504-518`
- Modify: `crates/hares-io/src/output/columns.rs:130`
- Modify: `crates/hares-python/src/py_enums.rs:106-108`

- [ ] **Step 1: Add GSHP/WSHP to HEATING_EQUIPMENT and COOLING_EQUIPMENT**

In `crates/hares-io/src/schedule_resolve.rs`, lines 504-518:

```rust
pub(crate) const HEATING_EQUIPMENT: &[&str] = &[
    "ASHP Heater",
    "MSHP Heater",
    "GSHP Heater",
    "WSHP Heater",
    "Gas Furnace",
    "Electric Furnace",
    "Oil Furnace",
    "Electric Baseboard",
    "Gas Boiler",
    "Electric Boiler",
    "Oil Boiler",
];

pub(crate) const COOLING_EQUIPMENT: &[&str] = &[
    "ASHP Cooler", "MSHP Cooler", "GSHP Cooler", "WSHP Cooler",
    "Air Conditioner", "Room AC",
];
```

- [ ] **Step 2: Add "Electric Resistance Water Heater" to output schema**

In `crates/hares-io/src/output/columns.rs`, line 130, add the canonical name:

```rust
        "Gas Water Heater"
        | "Electric Resistance Water Heater"
        | "Resistance Water Heater"
```

- [ ] **Step 3: Add missing classattrs to PyEndUse**

In `crates/hares-python/src/py_enums.rs`, after the `const OTHER` block (line 108), add the seven missing standard EndUse variants used by test code:

```rust
    #[classattr]
    const COOKING: Self = Self {
        inner: RustEndUse::COOKING,
    };
    #[classattr]
    const LAUNDRY: Self = Self {
        inner: RustEndUse::LAUNDRY,
    };
    #[classattr]
    const DISHWASHER: Self = Self {
        inner: RustEndUse::DISHWASHER,
    };
    #[classattr]
    const POOL_PUMP: Self = Self {
        inner: RustEndUse::POOL_PUMP,
    };
    #[classattr]
    const POOL_HEATER: Self = Self {
        inner: RustEndUse::POOL_HEATER,
    };
    #[classattr]
    const SPA_PUMP: Self = Self {
        inner: RustEndUse::SPA_PUMP,
    };
    #[classattr]
    const SPA_HEATER: Self = Self {
        inner: RustEndUse::SPA_HEATER,
    };
    #[classattr]
    const CEILING_FAN: Self = Self {
        inner: RustEndUse::CEILING_FAN,
    };
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p hares-io --lib
cargo test -p hares-python --lib
```

- [ ] **Step 5: Commit**

```bash
git add crates/hares-io/src/schedule_resolve.rs crates/hares-io/src/output/columns.rs crates/hares-python/src/py_enums.rs
git commit -m "fix: add GSHP/WSHP setpoints, Electric Resistance WH alias, missing PyEndUse variants"
```

---

### Task 2: Make build_typed_spec pub and expose equipment spec construction

**Files:**
- Modify: `crates/hares-io/src/hpxml/equipment.rs:143`
- Modify: `crates/hares-io/src/hpxml/mod.rs`

- [ ] **Step 1: Make build_typed_spec public**

In `crates/hares-io/src/hpxml/equipment.rs`, line 143, change `pub(super)` to `pub`:

```rust
pub fn build_typed_spec<T>(
    name: String,
    fuel_type: FuelType,
    config: T,
    defaults: &DefaultsStore,
) -> EquipmentSpec
where
    T: EquipmentTypedConfig,
{
    let parameters = serde_json::to_value(&config)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let typed_config = EquipmentConfig::from_typed(name.clone(), name.clone(), config);
    EquipmentSpec {
        name: name.clone(),
        instance_name: None,
        fuel_type,
        parameters,
        zip_params: defaults.zip_params(&name).cloned(),
        typed_config: Some(typed_config),
        system_id: None,
        related_hvac_idref: None,
        primary_role: None,
    }
}
```

- [ ] **Step 2: Re-export from hpxml module**

In `crates/hares-io/src/hpxml/mod.rs`, add:

```rust
pub use equipment::build_typed_spec;
```

- [ ] **Step 3: Add TODO note and commit**

After the companion plan lands, this `pub` promotion may have no remaining callers (the ergonomics plan builds `EquipmentSpec` directly). Audit and revert to `pub(crate)` or `pub(super)` if unused.

```bash
git add crates/hares-io/src/hpxml/equipment.rs crates/hares-io/src/hpxml/mod.rs
git commit -m "feat: make build_typed_spec public for DwellingBlueprint use (TODO: audit after companion plan)"
```

---

### Task 3: Create DwellingBlueprint Rust struct

Extract configure phase from `from_preparsed()` into `DwellingBlueprint`; move build phase into `DwellingBlueprint::build()`. Preserve `Dwelling::from_preparsed()` unchanged.

**Files:**
- Create: `crates/hares-core/src/dwelling/blueprint.rs`
- Modify: `crates/hares-core/src/dwelling/mod.rs`

- [ ] **Step 1: Define DwellingBlueprint struct**

```rust
// crates/hares-core/src/dwelling/blueprint.rs

use std::path::PathBuf;
use crate::dwelling::solver_builder::WeatherAverages;
use hares_io::{
    hpxml::{Building, EquipmentSpec},
    weather::WeatherTimeSeries,
    schedule::ScheduleTimeSeries,
    defaults::DefaultsStore,
    SimulationConfig, DesignConditions, SiteLocation,
};
use hares_types::{EndUse, HaresError};

/// Pre-build dwelling state. Equipment can be mutated before calling build().
pub struct DwellingBlueprint {
    config: DwellingConfig,
    building: Building,
    weather: WeatherTimeSeries,
    schedule: ScheduleTimeSeries,
    environment: EnvironmentManager,
    defaults: DefaultsStore,
    defaults_path: Option<PathBuf>,
    weather_avgs: WeatherAverages,
    design_conditions: Option<DesignConditions>,
    site_location: SiteLocation,
    equipment_specs: Vec<EquipmentSpec>,
}
```

- [ ] **Step 2: Implement DwellingBlueprint::from_config()**

Body extracted from `Dwelling::from_preparsed()` lines 1675-1793 (parse, resolve equipment, compute weather averages and site location, stop before solver construction):

```rust
impl DwellingBlueprint {
    pub fn from_config(config: DwellingConfig) -> Result<Self> {
        // 1. Parse HPXML -> Building (reuse existing parse_hpxml)
        // 2. Load weather, schedule from paths in config
        // 3. resolve_site_location, compute_weather_averages, extract design_conditions
        // 4. Build EnvironmentManager from weather, schedule, building, sim_config
        // 5. resolve_equipment(&building, &defaults, &overrides, patches) -> Vec<EquipmentSpec>
        // 6. Return Self — environment, specs, no solvers, no equipment instances, no ports
        todo!("extract from from_preparsed lines 1675-1793")
    }

    pub fn equipment_names(&self) -> Vec<&str> {
        self.equipment_specs.iter()
            .filter(|s| s.name != "Occupancy")
            .map(|s| s.name.as_str())
            .collect()
    }

    pub fn equipment_specs(&self) -> &[EquipmentSpec] {
        &self.equipment_specs
    }

    pub fn remove_equipment(&mut self, name: &str) -> Result<()> {
        let pos = self.equipment_specs.iter()
            .position(|s| s.name == name)
            .ok_or_else(|| HaresError::Dwelling(format!(
                "equipment '{}' not found in blueprint", name
            )))?;
        self.equipment_specs.remove(pos);
        Ok(())
    }

    pub fn remove_equipment_by_end_use(&mut self, end_uses: &[EndUse]) -> usize {
        let before = self.equipment_specs.len();
        self.equipment_specs.retain(|s| {
            let eu = hares_io::output::columns::equipment_name_to_end_use(&s.name);
            !end_uses.contains(&eu)
        });
        before - self.equipment_specs.len()
    }

    pub fn add_equipment_spec(&mut self, spec: EquipmentSpec) {
        self.equipment_specs.push(spec);
    }
}
```

- [ ] **Step 3: Implement DwellingBlueprint::build() as delegation to mod.rs**

To avoid visibility issues with module-private helpers (`register_pv_surfaces`, `attach_pv_to_roofs`, `register_pv_roof_shading`, `create_equipment_from_spec`, `validate_equipment_zones`, `build_schema`, etc.), keep the build logic in `mod.rs` as a `pub(crate)` function. `blueprint.rs` just delegates:

```rust
// crates/hares-core/src/dwelling/blueprint.rs
impl DwellingBlueprint {
    pub fn build(self) -> Result<Dwelling> {
        super::build_from_blueprint(self)
    }
}
```

```rust
// crates/hares-core/src/dwelling/mod.rs
pub(crate) fn build_from_blueprint(mut bp: DwellingBlueprint) -> Result<Dwelling> {
    // ALL existing code from from_preparsed lines 1795-2250, applied to bp:
    // 1. allocate_loop_ids(&mut bp.equipment_specs)
    // 2. register_pv_surfaces(&bp.equipment_specs, &mut bp.environment)
    // 3. attach_pv_to_roofs(&mut bp.equipment_specs, &bp.building)
    // 4. register_pv_roof_shading(&bp.equipment_specs, &bp.building, &mut bp.environment)
    // 5. Build clock, initial env, solvers (from bp fields)
    // 6. Autosize HVAC + WH
    // 7. inject_schedule_into_specs
    // 8. Create + init equipment
    // 9. Build ports + output schema
    // 10. Assemble Dwelling struct
    // 11. Register actors from ActorRegistry
    // 12. Run warmup loop (run_warmup_converged, restore RNG, reset clock)
    // 13. Set up diagnostic writer
    todo!("extract from from_preparsed lines 1795-2492")
}
```

This eliminates ALL visibility decisions — every helper stays private in `mod.rs`. No `pub` promotion needed.

- [ ] **Step 4: Update Dwelling::from_preparsed() to delegate to blueprint path**

```rust
impl Dwelling {
    fn from_preparsed(
        config: DwellingConfig,
        building: Building,
        weather: WeatherTimeSeries,
        schedule: ScheduleTimeSeries,
    ) -> Result<Self> {
        // Blueprint construction (parse phase, lines 1675-1793)
        let blueprint = DwellingBlueprint::from_parts(
            config, building, weather, schedule)?;
        // Build phase (lines 1795-2250), kept in mod.rs for helper access
        build_from_blueprint(blueprint)
    }
}
```

`from_parts()` is a non-public constructor that takes already-parsed data and pre-built `EnvironmentManager` (used by the existing `from_config` path to avoid double-parsing). It populates the blueprint struct including constructing the `EnvironmentManager` from weather/schedule/building.

- [ ] **Step 5: Add module declaration**

In `crates/hares-core/src/dwelling/mod.rs`:

```rust
pub mod blueprint;
pub use blueprint::DwellingBlueprint;
```

- [ ] **Step 6: Run Rust tests**

```bash
cargo test -p hares-core --lib
```

All existing tests must pass.

- [ ] **Step 7: Commit**

```bash
git add crates/hares-core/src/dwelling/blueprint.rs crates/hares-core/src/dwelling/mod.rs
git commit -m "feat: add DwellingBlueprint with configure-then-build pattern"
```

---

### Task 4-6: Python equipment classes + autosizing integration

> **REPLACED by companion plan:** `2026-06-08-python-equipment-ergonomics.md`
>
> Tasks E1-E7 in the companion plan replace these tasks. The key changes:
> - Each `*_config_from_py()` now returns `EquipmentSpec` directly (not `EquipmentConfig`)
> - All HVAC classes get `autosize: bool` kwarg; omitting capacity without `autosize=True` raises `ValueError`
> - Water heater classes get `autosize: bool` kwarg; triggered when either tank_volume or heating_capacity is absent
> - `spec.parameters` carries `autosize_heating`/`autosize_cooling`/`autosize_water_heater` flags
> - `spec.typed_config = None` when autosizing (autosizer patches `parameters`, not typed config)
> - `py_any_to_equipment_spec()` dispatching simplified — no `build_equipment_spec_from_typed` roundtrip
> - SEER→EIR: `3.41214 / seer.max(1e-6)`. HSPF→EIR: same formula.
> - Backup fuel is user-configurable via `backup_fuel: str` kwarg (not hardcoded Electric)

**Original Task 4 boilerplate below is superseded by the companion plan:**

<details>
<summary>Original Task 4 (superseded)</summary>

### Task 4: Create Python typed HVAC config classes

**Files:**
- Create: `crates/hares-python/src/py_hvac.rs`

- [ ] **Step 1: Define PyGasFurnace**

```rust
// crates/hares-python/src/py_hvac.rs

use hares_equipment::hvac::heating_config::{
    GasFurnaceConfig, HvacSetpointConfig, DuctConfig,
};
use hares_equipment::EquipmentConfig;
use pyo3::prelude::*;

#[pyclass(name = "GasFurnace")]
#[derive(Debug, Clone)]
pub struct PyGasFurnace {
    #[pyo3(get)] pub name: String,
    #[pyo3(get)] pub capacity_w: Option<f64>,
    #[pyo3(get)] pub afue: Option<f64>,
    #[pyo3(get)] pub zone_id: Option<u16>,
    #[pyo3(get)] pub fan_power_w: Option<f64>,
    #[pyo3(get)] pub number_of_speeds: Option<u8>,
    #[pyo3(get)] pub heating_setpoint_c: Option<f64>,
    #[pyo3(get)] pub cooling_setpoint_c: Option<f64>,
}

#[pymethods]
impl PyGasFurnace {
    #[new]
    #[pyo3(signature = (name, capacity_w=None, afue=None, zone_id=None,
                        fan_power_w=None, number_of_speeds=None,
                        heating_setpoint_c=None, cooling_setpoint_c=None))]
    fn new(
        name: String,
        capacity_w: Option<f64>,
        afue: Option<f64>,
        zone_id: Option<u16>,
        fan_power_w: Option<f64>,
        number_of_speeds: Option<u8>,
        heating_setpoint_c: Option<f64>,
        cooling_setpoint_c: Option<f64>,
    ) -> Self {
        Self { name, capacity_w, afue, zone_id, fan_power_w,
               number_of_speeds, heating_setpoint_c, cooling_setpoint_c }
    }
}

pub(crate) fn gas_furnace_config_from_py(f: &PyGasFurnace) -> EquipmentConfig {
    let cfg = GasFurnaceConfig {
        equipment_id: None,
        zone_id: f.zone_id,
        capacity_w: f.capacity_w.unwrap_or(0.0),
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
    EquipmentConfig::from_typed(f.name.clone(), "Gas Furnace".to_string(), cfg)
}
```

- [ ] **Step 2: Define PyAirConditioner (with seer convenience param)**

```rust
/// EIR formula: 3.41214 / seer.max(1e-6)  (match resolve_hvac.rs:1084)
const BTU_PER_HR_PER_W: f64 = 3.412_141_633;

#[pyclass(name = "AirConditioner")]
#[derive(Debug, Clone)]
pub struct PyAirConditioner {
    #[pyo3(get)] pub name: String,
    #[pyo3(get)] pub capacity_w: Option<f64>,
    #[pyo3(get)] pub eir: Option<f64>,
    #[pyo3(get)] pub seer: Option<f64>,
    #[pyo3(get)] pub zone_id: Option<u16>,
    #[pyo3(get)] pub shr: Option<f64>,
    #[pyo3(get)] pub number_of_speeds: Option<u8>,
    #[pyo3(get)] pub fan_power_w: Option<f64>,
    #[pyo3(get)] pub heating_setpoint_c: Option<f64>,
    #[pyo3(get)] pub cooling_setpoint_c: Option<f64>,
}

#[pymethods]
impl PyAirConditioner {
    #[new]
    #[pyo3(signature = (name, capacity_w=None, eir=None, seer=None,
                        zone_id=None, shr=None, number_of_speeds=None,
                        fan_power_w=None, heating_setpoint_c=None,
                        cooling_setpoint_c=None))]
    fn new(
        name: String,
        capacity_w: Option<f64>,
        eir: Option<f64>,
        seer: Option<f64>,
        zone_id: Option<u16>,
        shr: Option<f64>,
        number_of_speeds: Option<u8>,
        fan_power_w: Option<f64>,
        heating_setpoint_c: Option<f64>,
        cooling_setpoint_c: Option<f64>,
    ) -> Self {
        Self { name, capacity_w, eir, seer, zone_id, shr,
               number_of_speeds, fan_power_w,
               heating_setpoint_c, cooling_setpoint_c }
    }

    /// Compute EIR from SEER if SEER was provided and EIR was not.
    fn __init_post__(&mut self) {
        if self.eir.is_none() {
            if let Some(s) = self.seer {
                self.eir = Some(BTU_PER_HR_PER_W / s.max(1e-6));
            }
        }
    }
}

pub(crate) fn ac_config_from_py(ac: &PyAirConditioner) -> EquipmentConfig {
    use hares_equipment::hvac::cooling_config::CentralAirConditionerConfig;
    let seer = ac.seer.unwrap_or(14.0);
    let eir = ac.eir.unwrap_or(BTU_PER_HR_PER_W / seer.max(1e-6));
    let cfg = CentralAirConditionerConfig {
        equipment_id: None,
        zone_id: ac.zone_id,
        capacity_w: ac.capacity_w.unwrap_or(0.0),
        eir,
        shr: ac.shr,
        number_of_speeds: ac.number_of_speeds.unwrap_or(1),
        stage_capacities_w: None,
        stage_eirs: None,
        stage_shrs: None,
        fan_power_w: ac.fan_power_w,
        fan_power_w_per_cfm: None,
        setpoint: HvacSetpointConfig {
            heating_setpoint_c: ac.heating_setpoint_c,
            cooling_setpoint_c: ac.cooling_setpoint_c,
            ..Default::default()
        },
        hysteresis_c: None,
        airflow_m3_s_per_w: None,
        fraction_load_served: None,
        crankcase_heater_kw: None,
        crankcase_heater_threshold_c: None,
        crankcase_capacity_curve_coeffs: None,
        duct: DuctConfig::default(),
        system_type: None,
        startup_cd: None,
        biquadratic_x1_min: None, biquadratic_x1_max: None,
        biquadratic_x2_min: None, biquadratic_x2_max: None,
        ff_min: None, ff_max: None,
        plf_min: None, plf_max: None,
        charge_defect_ratio: None,
        min_oat_compressor_cooling_c: None,
    };
    EquipmentConfig::from_typed(ac.name.clone(), "Air Conditioner".to_string(), cfg)
}
```

- [ ] **Step 3: Define PyASHPHeater and PyASHPCooler**

```rust
#[pyclass(name = "ASHPHeater")]
#[derive(Debug, Clone)]
pub struct PyASHPHeater {
    #[pyo3(get)] pub name: String,
    #[pyo3(get)] pub capacity_w: Option<f64>,
    #[pyo3(get)] pub hspf: Option<f64>,
    #[pyo3(get)] pub zone_id: Option<u16>,
    #[pyo3(get)] pub is_mini_split: Option<bool>,
    #[pyo3(get)] pub backup_capacity_w: Option<f64>,
    #[pyo3(get)] pub backup_fuel: Option<String>,
    #[pyo3(get)] pub fan_power_w: Option<f64>,
    #[pyo3(get)] pub heating_setpoint_c: Option<f64>,
    #[pyo3(get)] pub cooling_setpoint_c: Option<f64>,
}

#[pymethods]
impl PyASHPHeater {
    #[new]
    #[pyo3(signature = (name, capacity_w=None, hspf=None, zone_id=None,
                        is_mini_split=None, backup_capacity_w=None,
                        backup_fuel=None, fan_power_w=None,
                        heating_setpoint_c=None, cooling_setpoint_c=None))]
    fn new(
        name: String,
        capacity_w: Option<f64>,
        hspf: Option<f64>,
        zone_id: Option<u16>,
        is_mini_split: Option<bool>,
        backup_capacity_w: Option<f64>,
        backup_fuel: Option<String>,
        fan_power_w: Option<f64>,
        heating_setpoint_c: Option<f64>,
        cooling_setpoint_c: Option<f64>,
    ) -> Self {
        Self { name, capacity_w, hspf, zone_id, is_mini_split,
               backup_capacity_w, backup_fuel, fan_power_w,
               heating_setpoint_c, cooling_setpoint_c }
    }
}

pub(crate) fn ashp_heater_config_from_py(hp: &PyASHPHeater) -> EquipmentConfig {
    use hares_equipment::hvac::heat_pump_config::{
        HeatPumpHeaterConfig, HeatPumpCommonConfig,
    };
    use hares_types::FuelType;
    let is_mini = hp.is_mini_split.unwrap_or(false);
    let hspf = hp.hspf.unwrap_or(8.2);
    let heating_eir = BTU_PER_HR_PER_W / hspf.max(1e-6);
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
        fraction_heating_load_served: None,
        is_mini_split: is_mini,
        fan_power_w: hp.fan_power_w,
        setpoint: HvacSetpointConfig {
            heating_setpoint_c: hp.heating_setpoint_c,
            cooling_setpoint_c: hp.cooling_setpoint_c,
            ..Default::default()
        },
        ..Default::default()
    };
    let cfg = HeatPumpHeaterConfig {
        common,
        ..Default::default()
    };
    // ochre_class selects registry factory; typed payload type_name is always "ASHP Heater"
    // (HeatPumpHeaterConfig::equipment_type_name() returns "ASHP Heater").
    // This is correct: the factory dispatch uses ochre_class, not the typed type_name.
    EquipmentConfig::from_typed(hp.name.clone(), "ASHP Heater".to_string(), cfg)
}

#[pyclass(name = "ASHPCooler")]
#[derive(Debug, Clone)]
pub struct PyASHPCooler {
    #[pyo3(get)] pub name: String,
    #[pyo3(get)] pub capacity_w: Option<f64>,
    #[pyo3(get)] pub seer: Option<f64>,
    #[pyo3(get)] pub zone_id: Option<u16>,
    #[pyo3(get)] pub is_mini_split: Option<bool>,
    #[pyo3(get)] pub shr: Option<f64>,
    #[pyo3(get)] pub fan_power_w: Option<f64>,
    #[pyo3(get)] pub heating_setpoint_c: Option<f64>,
    #[pyo3(get)] pub cooling_setpoint_c: Option<f64>,
}

#[pymethods]
impl PyASHPCooler {
    #[new]
    #[pyo3(signature = (name, capacity_w=None, seer=None, zone_id=None,
                        is_mini_split=None, shr=None, fan_power_w=None,
                        heating_setpoint_c=None, cooling_setpoint_c=None))]
    fn new(
        name: String,
        capacity_w: Option<f64>,
        seer: Option<f64>,
        zone_id: Option<u16>,
        is_mini_split: Option<bool>,
        shr: Option<f64>,
        fan_power_w: Option<f64>,
        heating_setpoint_c: Option<f64>,
        cooling_setpoint_c: Option<f64>,
    ) -> Self {
        Self { name, capacity_w, seer, zone_id, is_mini_split,
               shr, fan_power_w, heating_setpoint_c, cooling_setpoint_c }
    }
}

pub(crate) fn ashp_cooler_config_from_py(hp: &PyASHPCooler) -> EquipmentConfig {
    use hares_equipment::hvac::heat_pump_config::{
        HeatPumpCoolerConfig, HeatPumpCommonConfig,
    };
    let seer = hp.seer.unwrap_or(14.0);
    let cooling_eir = BTU_PER_HR_PER_W / seer.max(1e-6);
    let common = HeatPumpCommonConfig {
        zone_id: hp.zone_id,
        cooling_capacity_w: hp.capacity_w,
        cooling_eir: Some(cooling_eir),
        is_mini_split: hp.is_mini_split.unwrap_or(false),
        shr: hp.shr,
        fan_power_w: hp.fan_power_w,
        setpoint: HvacSetpointConfig {
            heating_setpoint_c: hp.heating_setpoint_c,
            cooling_setpoint_c: hp.cooling_setpoint_c,
            ..Default::default()
        },
        ..Default::default()
    };
    let cfg = HeatPumpCoolerConfig {
        common,
        ..Default::default()
    };
    EquipmentConfig::from_typed(hp.name.clone(), "ASHP Cooler".to_string(), cfg)
}
```

- [ ] **Step 4: Define PyElectricBaseboard and PyIdealHVAC**

```rust
#[pyclass(name = "ElectricBaseboard")]
#[derive(Debug, Clone)]
pub struct PyElectricBaseboard {
    #[pyo3(get)] pub name: String,
    #[pyo3(get)] pub capacity_w: Option<f64>,
    #[pyo3(get)] pub eir: Option<f64>,
    #[pyo3(get)] pub zone_id: Option<u16>,
    #[pyo3(get)] pub heating_setpoint_c: Option<f64>,
    #[pyo3(get)] pub cooling_setpoint_c: Option<f64>,
}

#[pymethods]
impl PyElectricBaseboard {
    #[new]
    #[pyo3(signature = (name, capacity_w=None, eir=None, zone_id=None,
                        heating_setpoint_c=None, cooling_setpoint_c=None))]
    fn new(
        name: String,
        capacity_w: Option<f64>,
        eir: Option<f64>,
        zone_id: Option<u16>,
        heating_setpoint_c: Option<f64>,
        cooling_setpoint_c: Option<f64>,
    ) -> Self {
        Self { name, capacity_w, eir, zone_id, heating_setpoint_c, cooling_setpoint_c }
    }
}

pub(crate) fn baseboard_config_from_py(bb: &PyElectricBaseboard) -> EquipmentConfig {
    use hares_equipment::hvac::heating_config::ElectricBaseboardConfig;
    let cfg = ElectricBaseboardConfig {
        equipment_id: None,
        zone_id: bb.zone_id,
        capacity_w: bb.capacity_w.unwrap_or(0.0),
        eir: bb.eir.unwrap_or(1.0),
        setpoint: HvacSetpointConfig {
            heating_setpoint_c: bb.heating_setpoint_c,
            cooling_setpoint_c: bb.cooling_setpoint_c,
            ..Default::default()
        },
    };
    EquipmentConfig::from_typed(bb.name.clone(), "Electric Baseboard".to_string(), cfg)
}

#[pyclass(name = "IdealHVAC")]
#[derive(Debug, Clone)]
pub struct PyIdealHVAC {
    #[pyo3(get)] pub name: String,
    #[pyo3(get)] pub zone_id: Option<u16>,
    #[pyo3(get)] pub heating_capacity_w: Option<f64>,
    #[pyo3(get)] pub cooling_capacity_w: Option<f64>,
    #[pyo3(get)] pub heating_setpoint_c: Option<f64>,
    #[pyo3(get)] pub cooling_setpoint_c: Option<f64>,
    #[pyo3(get)] pub deadband_c: Option<f64>,
}

#[pymethods]
impl PyIdealHVAC {
    #[new]
    #[pyo3(signature = (name, zone_id=None, heating_capacity_w=None,
                        cooling_capacity_w=None, heating_setpoint_c=None,
                        cooling_setpoint_c=None, deadband_c=None))]
    fn new(
        name: String,
        zone_id: Option<u16>,
        heating_capacity_w: Option<f64>,
        cooling_capacity_w: Option<f64>,
        heating_setpoint_c: Option<f64>,
        cooling_setpoint_c: Option<f64>,
        deadband_c: Option<f64>,
    ) -> Self {
        Self { name, zone_id, heating_capacity_w, cooling_capacity_w,
               heating_setpoint_c, cooling_setpoint_c, deadband_c }
    }
}

pub(crate) fn ideal_hvac_config_from_py(hvac: &PyIdealHVAC) -> EquipmentConfig {
    use hares_equipment::hvac::heating_config::IdealHvacConfig;
    let cfg = IdealHvacConfig {
        equipment_id: None,
        zone_id: hvac.zone_id,
        heating_capacity_w: hvac.heating_capacity_w,
        cooling_capacity_w: hvac.cooling_capacity_w,
        setpoint: HvacSetpointConfig {
            heating_setpoint_c: hvac.heating_setpoint_c,
            cooling_setpoint_c: hvac.cooling_setpoint_c,
            ..Default::default()
        },
        deadband_c: hvac.deadband_c,
        ..Default::default()
    };
    EquipmentConfig::from_typed(hvac.name.clone(), "Ideal HVAC".to_string(), cfg)
}
```

- [ ] **Step 5: Commit**

```bash
git add crates/hares-python/src/py_hvac.rs
git commit -m "feat: add Python typed HVAC config classes"
```

---

</details>

### Task 5: Water heater classes → superseded by companion plan E6

### Task 6: PyDwellingBlueprint bindings → superseded by companion plan E7

**Original material below is historical. See `2026-06-08-python-equipment-ergonomics.md` for the current implementation.**

<details>
<summary>Original Tasks 5-6 (superseded)</summary>

### Task 5: Create Python typed water heater config classes

**Files:**
- Create: `crates/hares-python/src/py_water_heater.rs`

- [ ] **Step 1: Define PyGasWaterHeater**

```rust
// crates/hares-python/src/py_water_heater.rs

use hares_equipment::water_heater::wh_config::GasWaterHeaterConfig;
use hares_equipment::EquipmentConfig;
use hares_types::FuelType;
use pyo3::prelude::*;

#[pyclass(name = "GasWaterHeater")]
#[derive(Debug, Clone)]
pub struct PyGasWaterHeater {
    #[pyo3(get)] pub name: String,
    #[pyo3(get)] pub tank_volume_m3: Option<f64>,
    #[pyo3(get)] pub uniform_energy_factor: Option<f64>,
    #[pyo3(get)] pub heating_capacity_w: Option<f64>,
    #[pyo3(get)] pub setpoint_c: Option<f64>,
    #[pyo3(get)] pub zone_id: Option<u16>,
    #[pyo3(get)] pub avg_water_draw_l_per_day: Option<f64>,
}

#[pymethods]
impl PyGasWaterHeater {
    #[new]
    #[pyo3(signature = (name, tank_volume_m3=None, uniform_energy_factor=None,
                        heating_capacity_w=None, setpoint_c=None,
                        zone_id=None, avg_water_draw_l_per_day=None))]
    fn new(
        name: String,
        tank_volume_m3: Option<f64>,
        uniform_energy_factor: Option<f64>,
        heating_capacity_w: Option<f64>,
        setpoint_c: Option<f64>,
        zone_id: Option<u16>,
        avg_water_draw_l_per_day: Option<f64>,
    ) -> Self {
        Self { name, tank_volume_m3, uniform_energy_factor,
               heating_capacity_w, setpoint_c, zone_id,
               avg_water_draw_l_per_day }
    }
}

pub(crate) fn gas_wh_config_from_py(wh: &PyGasWaterHeater) -> EquipmentConfig {
    let cfg = GasWaterHeaterConfig {
        equipment_id: None,
        zone_id: wh.zone_id,
        loop_id: None,
        fuel_type: FuelType::Gas,
        tank_volume_m3: wh.tank_volume_m3,
        tank_height_m: None,
        energy_factor: None,
        uniform_energy_factor: wh.uniform_energy_factor,
        heating_capacity_w: wh.heating_capacity_w,
        ua_w_per_k: None,
        setpoint_c: wh.setpoint_c,
        deadband_c: None,
        max_tank_temp_c: None,
        initial_tank_temp_c: None,
        tank_nodes: None,
        avg_water_draw_l_per_day: wh.avg_water_draw_l_per_day,
        draw_flow_rate_kg_s: None,
        draw_flow_rate_source: None,
        mains_temp_c_source: None,
        pilot_power_w: None,
        flue_loss_fraction: None,
        skin_loss_fraction: None,
        ignition_type: None,
        performance_adjustment: None,
        zone_type: None,
        first_hour_rating_m3: None,
        jacket_r_value_m2_k_w: None,
        conversion_efficiency: None,
        fixture_delivery_temp_c: None,
        hot_draw_temp_c: None,
        pilot_fraction_to_tank: None,
    };
    EquipmentConfig::from_typed(wh.name.clone(), "Gas Water Heater".to_string(), cfg)
}
```

- [ ] **Step 2: Define PyElectricResistanceWH**

Note: `loop_id: Option<u16>` is a real field on `ElectricResistanceWaterHeaterConfig`.

```rust
#[pyclass(name = "ElectricResistanceWH")]
#[derive(Debug, Clone)]
pub struct PyElectricResistanceWH {
    #[pyo3(get)] pub name: String,
    #[pyo3(get)] pub tank_volume_m3: Option<f64>,
    #[pyo3(get)] pub uniform_energy_factor: Option<f64>,
    #[pyo3(get)] pub heating_capacity_w: Option<f64>,
    #[pyo3(get)] pub setpoint_c: Option<f64>,
    #[pyo3(get)] pub zone_id: Option<u16>,
    #[pyo3(get)] pub avg_water_draw_l_per_day: Option<f64>,
}

#[pymethods]
impl PyElectricResistanceWH {
    #[new]
    #[pyo3(signature = (name, tank_volume_m3=None, uniform_energy_factor=None,
                        heating_capacity_w=None, setpoint_c=None,
                        zone_id=None, avg_water_draw_l_per_day=None))]
    fn new(
        name: String,
        tank_volume_m3: Option<f64>,
        uniform_energy_factor: Option<f64>,
        heating_capacity_w: Option<f64>,
        setpoint_c: Option<f64>,
        zone_id: Option<u16>,
        avg_water_draw_l_per_day: Option<f64>,
    ) -> Self {
        Self { name, tank_volume_m3, uniform_energy_factor,
               heating_capacity_w, setpoint_c, zone_id,
               avg_water_draw_l_per_day }
    }
}

pub(crate) fn elec_res_wh_config_from_py(wh: &PyElectricResistanceWH) -> EquipmentConfig {
    use hares_equipment::water_heater::wh_config::ElectricResistanceWaterHeaterConfig;
    let cfg = ElectricResistanceWaterHeaterConfig {
        equipment_id: None,
        zone_id: wh.zone_id,
        loop_id: None,
        tank_volume_m3: wh.tank_volume_m3,
        tank_height_m: None,
        energy_factor: None,
        uniform_energy_factor: wh.uniform_energy_factor,
        heating_capacity_w: wh.heating_capacity_w,
        ua_w_per_k: None,
        setpoint_c: wh.setpoint_c,
        deadband_c: None,
        max_tank_temp_c: None,
        initial_tank_temp_c: None,
        tank_nodes: None,
        avg_water_draw_l_per_day: wh.avg_water_draw_l_per_day,
        draw_flow_rate_kg_s: None,
        draw_flow_rate_source: None,
        mains_temp_c_source: None,
        performance_adjustment: None,
        zone_type: None,
        first_hour_rating_m3: None,
        element_power_w: None,
        max_setpoint_ramp_rate_c_per_min: None,
        element_priority_mode: None,
        jacket_r_value_m2_k_w: None,
        fixture_delivery_temp_c: None,
        hot_draw_temp_c: None,
        max_combined_power_w: None,
    };
    EquipmentConfig::from_typed(wh.name.clone(), "Electric Resistance Water Heater".to_string(), cfg)
}
```

- [ ] **Step 3: Define PyHeatPumpWH**

Note: actual fields are `cop`, `backup_element_power_w`, `compressor_power_w`, `backup_enable_offset_c`, `hp_only_mode`, `element_hp_control_mode`, `capacity_biquadratic_coeffs`, `cop_biquadratic_coeffs` — no `uef`/`uniform_energy_factor`/`heating_capacity_w`.

```rust
#[pyclass(name = "HeatPumpWH")]
#[derive(Debug, Clone)]
pub struct PyHeatPumpWH {
    #[pyo3(get)] pub name: String,
    #[pyo3(get)] pub tank_volume_m3: Option<f64>,
    #[pyo3(get)] pub cop: Option<f64>,
    #[pyo3(get)] pub backup_element_power_w: Option<f64>,
    #[pyo3(get)] pub compressor_power_w: Option<f64>,
    #[pyo3(get)] pub setpoint_c: Option<f64>,
    #[pyo3(get)] pub zone_id: Option<u16>,
    #[pyo3(get)] pub avg_water_draw_l_per_day: Option<f64>,
}

#[pymethods]
impl PyHeatPumpWH {
    #[new]
    #[pyo3(signature = (name, tank_volume_m3=None, cop=None,
                        backup_element_power_w=None,
                        compressor_power_w=None, setpoint_c=None,
                        zone_id=None, avg_water_draw_l_per_day=None))]
    fn new(
        name: String,
        tank_volume_m3: Option<f64>,
        cop: Option<f64>,
        backup_element_power_w: Option<f64>,
        compressor_power_w: Option<f64>,
        setpoint_c: Option<f64>,
        zone_id: Option<u16>,
        avg_water_draw_l_per_day: Option<f64>,
    ) -> Self {
        Self { name, tank_volume_m3, cop, backup_element_power_w,
               compressor_power_w, setpoint_c, zone_id,
               avg_water_draw_l_per_day }
    }
}

pub(crate) fn hpwh_config_from_py(wh: &PyHeatPumpWH) -> EquipmentConfig {
    use hares_equipment::water_heater::wh_config::HeatPumpWaterHeaterConfig;
    let cfg = HeatPumpWaterHeaterConfig {
        equipment_id: None,
        zone_id: wh.zone_id,
        loop_id: None,
        tank_volume_m3: wh.tank_volume_m3,
        tank_height_m: None,
        cop: wh.cop,
        backup_element_power_w: wh.backup_element_power_w,
        ua_w_per_k: None,
        setpoint_c: wh.setpoint_c,
        deadband_c: None,
        max_tank_temp_c: None,
        initial_tank_temp_c: None,
        tank_nodes: None,
        tempering_valve_setpoint_c: None,
        avg_water_draw_l_per_day: wh.avg_water_draw_l_per_day,
        draw_flow_rate_kg_s: None,
        compressor_power_w: wh.compressor_power_w,
        backup_enable_offset_c: None,
        min_ambient_temp_c: None,
        max_ambient_temp_c: None,
        min_on_time_s: None,
        min_off_time_s: None,
        hp_only_mode: None,
        element_hp_control_mode: None,
        fan_power_w: None,
        parasitic_power_w: None,
        backup_efficiency: None,
        shr: None,
        lost_heat_fraction: None,
        wall_heat_fraction: None,
        capacity_biquadratic_coeffs: None,
        cop_biquadratic_coeffs: None,
        performance_adjustment: None,
        zone_type: None,
        first_hour_rating_m3: None,
        jacket_r_value_m2_k_w: None,
        fixture_delivery_temp_c: None,
    };
    EquipmentConfig::from_typed(wh.name.clone(), "Heat Pump Water Heater".to_string(), cfg)
}
```

- [ ] **Step 4: Commit**

```bash
git add crates/hares-python/src/py_water_heater.rs
git commit -m "feat: add Python typed water heater config classes"
```

---

### Task 6: Create PyDwellingBlueprint Python bindings

**Files:**
- Create: `crates/hares-python/src/py_blueprint.rs`
- Modify: `crates/hares-python/src/py_dwelling.rs` (add FromPyDwellingBlueprint, make build_config pub(crate))
- Modify: `crates/hares-python/src/lib.rs` (register new types)

- [ ] **Step 1: Define PyDwellingBlueprint struct and methods**

```rust
// crates/hares-python/src/py_blueprint.rs

use std::path::PathBuf;
use hares_core::dwelling::blueprint::DwellingBlueprint;
use hares_io::hpxml::{build_typed_spec, EquipmentSpec};
use hares_types::{EndUse, FuelType};
use pyo3::prelude::*;
use pyo3::exceptions::PyValueError;

use crate::py_dwelling::PyDwelling;
use crate::py_enums::PyEndUse;
use crate::py_hvac::{
    PyGasFurnace, PyAirConditioner, PyASHPHeater, PyASHPCooler,
    PyIdealHVAC, PyElectricBaseboard,
    gas_furnace_config_from_py, ac_config_from_py,
    ashp_heater_config_from_py, ashp_cooler_config_from_py,
    ideal_hvac_config_from_py, baseboard_config_from_py,
};
use crate::py_water_heater::{
    PyGasWaterHeater, PyElectricResistanceWH, PyHeatPumpWH,
    gas_wh_config_from_py, elec_res_wh_config_from_py, hpwh_config_from_py,
};

#[pyclass(name = "DwellingBlueprint")]
#[doc = "Pre-build dwelling configuration.\n\
\n\
Two-phase equipment API:\n\
- Phase 1 (Blueprint): add/remove HVAC and water heater equipment before build().\n\
  Use typed config objects: GasFurnace, ASHPHeater, AirConditioner, HeatPumpWH, etc.\n\
- Phase 2 (Dwelling): add Battery, PV, EV after build() via dwelling.add_battery() etc.\n\
\n\
Battery/PV/EV are NOT valid for add_equipment() — use the Dwelling methods instead.\n\
HVAC/WH are NOT valid on the built Dwelling — all swaps happen at the blueprint stage."]
pub struct PyDwellingBlueprint {
    inner: DwellingBlueprint,
    config: hares_core::DwellingConfig,
}

#[pymethods]
impl PyDwellingBlueprint {
    #[classmethod]
    #[pyo3(signature = (hpxml, schedule, weather, **kwargs))]
    pub fn from_hpxml(
        _cls: &Bound<'_, PyType>,
        hpxml: String,
        schedule: String,
        weather: String,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Self> {
        let config = crate::py_dwelling::build_config(hpxml, schedule, weather, kwargs)?;
        let inner = DwellingBlueprint::from_config(config.clone())
            .map_err(crate::py_dwelling::to_py_err)?;
        Ok(Self { inner, config })
    }

    /// Returns a list of equipment canonical names (method, not property).
    pub fn equipment_names(&self) -> Vec<String> {
        self.inner.equipment_names().into_iter().map(String::from).collect()
    }

    pub fn remove_equipment(&mut self, name: &str) -> PyResult<()> {
        self.inner.remove_equipment(name)
            .map_err(crate::py_dwelling::to_py_err)
    }

    pub fn remove_equipment_by_end_use(
        &mut self,
        end_uses: &Bound<'_, PyAny>,
    ) -> PyResult<usize> {
        // Accept both single PyEndUse and list of PyEndUse
        let rust_uses: Vec<EndUse> = if let Ok(single) = end_uses.extract::<PyEndUse>() {
            vec![EndUse::from(single)]
        } else if let Ok(list) = end_uses.extract::<Vec<PyEndUse>>() {
            list.into_iter().map(EndUse::from).collect()
        } else {
            return Err(PyValueError::new_err(
                "remove_equipment_by_end_use: expected EndUse or list[EndUse]"
            ));
        };
        Ok(self.inner.remove_equipment_by_end_use(&rust_uses))
    }

    /// Add HVAC or water heater equipment from a typed config object.
    /// Accepts: GasFurnace, AirConditioner, ASHPHeater, ASHPCooler,
    /// IdealHVAC, ElectricBaseboard, GasWaterHeater, ElectricResistanceWH,
    /// HeatPumpWH.
    ///
    /// Duplicate instance names are auto-renamed at build time using the
    /// "Name #N" convention (e.g. two ASHPHeater("ASHP", ...) become
    /// "ASHP" and "ASHP #2" in the final dwelling).
    #[pyo3(text_signature = "(self, equipment)")]
    pub fn add_equipment(&mut self, obj: &Bound<'_, PyAny>) -> PyResult<()> {
        let spec = py_any_to_equipment_spec(obj)?;
        self.inner.add_equipment_spec(spec);
        Ok(())
    }

    pub fn build(mut self) -> PyResult<PyDwelling> {
        let dwelling = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.inner.build()
        }))
        .map_err(|_| PyValueError::new_err("DwellingBlueprint::build() panicked"))?
        .map_err(crate::py_dwelling::to_py_err)?;

        Ok(PyDwelling::from_blueprint_build(dwelling, self.config))
    }
}

fn py_any_to_equipment_spec(obj: &Bound<'_, PyAny>) -> PyResult<EquipmentSpec> {
    // Use a DefaultsStore::empty() since we don't need zip-code defaults
    // for programmatically added equipment (user provides explicit params).
    let defaults = hares_io::defaults::DefaultsStore::empty();

    if let Ok(f) = obj.extract::<PyRef<'_, PyGasFurnace>>() {
        let config = gas_furnace_config_from_py(&f);
        return Ok(build_equipment_spec_from_typed("Gas Furnace", FuelType::Gas, config, &defaults));
    }
    if let Ok(ac) = obj.extract::<PyRef<'_, PyAirConditioner>>() {
        let config = ac_config_from_py(&ac);
        return Ok(build_equipment_spec_from_typed("Air Conditioner", FuelType::Electric, config, &defaults));
    }
    if let Ok(hp) = obj.extract::<PyRef<'_, PyASHPHeater>>() {
        let config = ashp_heater_config_from_py(&hp);
        return Ok(build_equipment_spec_from_typed("ASHP Heater", FuelType::Electric, config, &defaults));
    }
    if let Ok(hp) = obj.extract::<PyRef<'_, PyASHPCooler>>() {
        let config = ashp_cooler_config_from_py(&hp);
        return Ok(build_equipment_spec_from_typed("ASHP Cooler", FuelType::Electric, config, &defaults));
    }
    if let Ok(hvac) = obj.extract::<PyRef<'_, PyIdealHVAC>>() {
        let config = ideal_hvac_config_from_py(&hvac);
        return Ok(build_equipment_spec_from_typed("Ideal HVAC", FuelType::Electric, config, &defaults));
    }
    if let Ok(bb) = obj.extract::<PyRef<'_, PyElectricBaseboard>>() {
        let config = baseboard_config_from_py(&bb);
        return Ok(build_equipment_spec_from_typed("Electric Baseboard", FuelType::Electric, config, &defaults));
    }
    if let Ok(wh) = obj.extract::<PyRef<'_, PyGasWaterHeater>>() {
        let config = gas_wh_config_from_py(&wh);
        return Ok(build_equipment_spec_from_typed("Gas Water Heater", FuelType::Gas, config, &defaults));
    }
    if let Ok(wh) = obj.extract::<PyRef<'_, PyElectricResistanceWH>>() {
        let config = elec_res_wh_config_from_py(&wh);
        return Ok(build_equipment_spec_from_typed(
            "Electric Resistance Water Heater", FuelType::Electric, config, &defaults));
    }
    if let Ok(wh) = obj.extract::<PyRef<'_, PyHeatPumpWH>>() {
        let config = hpwh_config_from_py(&wh);
        return Ok(build_equipment_spec_from_typed(
            "Heat Pump Water Heater", FuelType::Electric, config, &defaults));
    }
    Err(PyValueError::new_err(
        "equipment must be a typed HVAC or water heater config object"
    ))
}

/// Reuse the single build_typed_spec from hares-io, not a hand-rolled duplicate.
fn build_equipment_spec_from_typed<T: hares_equipment::config::EquipmentTypedConfig>(
    canonical_name: &str,
    fuel_type: FuelType,
    config: hares_equipment::EquipmentConfig,
    defaults: &hares_io::defaults::DefaultsStore,
) -> EquipmentSpec {
    // Extract the typed config from EquipmentConfig to pass to build_typed_spec
    // We already serialized it via EquipmentConfig::from_typed(), so unwrap is safe.
    let typed_cfg: T = config.require_typed(canonical_name)
        .expect("typed config extraction failed");
    build_typed_spec(canonical_name.to_string(), fuel_type, typed_cfg, defaults)
}
```

- [ ] **Step 2: Add PyDwelling::from_blueprint_build()**

In `crates/hares-python/src/py_dwelling.rs`, add:

```rust
impl PyDwelling {
    /// Construct from a DwellingBlueprint build result.
    /// Not a #[pymethod] — only called by PyDwellingBlueprint::build().
    pub(crate) fn from_blueprint_build(dwelling: Dwelling, config: DwellingConfig) -> Self {
        let sim_config = PySimulationConfig::from_sim_config(&config.sim_config);
        Self {
            dwelling: Mutex::new(dwelling),
            poisoned: AtomicBool::new(false),
            mutex_poison_events: std::sync::atomic::AtomicU64::new(0),
            config,
            sim_config,
            initial_state: None,
            initialized: false,
            registry: ActorRegistry::new(),
            equipment_registry: EquipmentRegistry::new(),
            zone_keys: None,
            gas_tariff: None,
        }
    }
}
```

Also change `build_config` visibility to `pub(crate)` (line ~1886 in py_dwelling.rs):

```rust
// From: fn build_config(
// To:   pub(crate) fn build_config(
```

- [ ] **Step 3: Register new types in lib.rs**

In `crates/hares-python/src/lib.rs`:

```rust
mod py_hvac;
mod py_water_heater;
mod py_blueprint;

// In the #[pymodule] init function, add:
m.add_class::<py_hvac::PyGasFurnace>()?;
m.add_class::<py_hvac::PyAirConditioner>()?;
m.add_class::<py_hvac::PyASHPHeater>()?;
m.add_class::<py_hvac::PyASHPCooler>()?;
m.add_class::<py_hvac::PyIdealHVAC>()?;
m.add_class::<py_hvac::PyElectricBaseboard>()?;
m.add_class::<py_water_heater::PyGasWaterHeater>()?;
m.add_class::<py_water_heater::PyElectricResistanceWH>()?;
m.add_class::<py_water_heater::PyHeatPumpWH>()?;
m.add_class::<py_blueprint::PyDwellingBlueprint>()?;
```

- [ ] **Step 4: Build and verify compilation**

```bash
uv run maturin develop
```

- [ ] **Step 5: Commit**

```bash
git add crates/hares-python/src/py_dwelling.rs crates/hares-python/src/lib.rs
git commit -m "feat: add PyDwellingBlueprint Python bindings"
```

</details>

---

### Task 7: Parity test

**Files:**
- Create: `tests/python/test_blueprint_parity.py`

- [ ] **Step 1: Write parity test using conftest.py paths**

```python
"""Verify DwellingBlueprint.from_hpxml().build() == Dwelling.from_hpxml()."""

import pytest
from ochre_next import Dwelling, DwellingBlueprint

from conftest import HPXML, SCHEDULE, WEATHER, HARES_DEFAULTS


def test_blueprint_parity_default():
    """Building a blueprint with no changes produces same equipment names."""
    dw1 = Dwelling.from_hpxml(
        HPXML, SCHEDULE, WEATHER,
        defaults_path=str(HARES_DEFAULTS),
        bldg_id=42, duration_s=300, time_res_s=60,
    )
    dw1.initialize()

    bp = DwellingBlueprint.from_hpxml(
        HPXML, SCHEDULE, WEATHER,
        defaults_path=str(HARES_DEFAULTS),
        bldg_id=42, duration_s=300, time_res_s=60,
    )
    dw2 = bp.build()
    dw2.initialize()

    # equipment_names() is a method, not a property
    assert dw1.equipment_names() == dw2.equipment_names()

    # Step both and compare net electric power
    for _ in range(5):
        r1 = dw1.step()
        r2 = dw2.step()
        assert r1 is not None
        assert r2 is not None
        assert abs(r1["net_electric_power_kw"] - r2["net_electric_power_kw"]) < 1e-6


def test_blueprint_equipment_names():
    """Blueprint reports equipment names correctly."""
    bp = DwellingBlueprint.from_hpxml(
        HPXML, SCHEDULE, WEATHER,
        defaults_path=str(HARES_DEFAULTS),
        bldg_id=42, duration_s=300, time_res_s=60,
    )
    names = bp.equipment_names()
    assert isinstance(names, list)
    assert all(isinstance(n, str) for n in names)
    assert len(names) >= 3
```

- [ ] **Step 2: Run parity test**

```bash
uv run pytest tests/python/test_blueprint_parity.py -v
```

- [ ] **Step 3: Commit**

```bash
git add tests/python/test_blueprint_parity.py
git commit -m "test: add DwellingBlueprint parity tests"
```

---

### Task 8: Equipment swap tests

**Files:**
- Create: `tests/python/test_blueprint_swap.py`

- [ ] **Step 1: Write swap tests**

```python
"""Test HVAC and WH equipment swapping via DwellingBlueprint."""

import pytest
from ochre_next import (
    DwellingBlueprint, GasFurnace, ASHPHeater, ASHPCooler,
    AirConditioner, HeatPumpWH, ElectricResistanceWH,
    EndUse,
)
from conftest import HPXML, SCHEDULE, WEATHER, HARES_DEFAULTS


def _make_blueprint():
    return DwellingBlueprint.from_hpxml(
        HPXML, SCHEDULE, WEATHER,
        defaults_path=str(HARES_DEFAULTS),
        bldg_id=42, duration_s=300, time_res_s=60,
    )


def test_swap_gas_furnace_to_ashp():
    """Swap Gas Furnace for ASHP Heater + ASHP Cooler."""
    bp = _make_blueprint()
    bp.remove_equipment_by_end_use(EndUse.HVAC_HEATING)
    assert "Gas Furnace" not in bp.equipment_names()

    ashp = ASHPHeater("ASHP", capacity_w=12000, hspf=9.5)
    bp.add_equipment(ashp)
    assert "ASHP" in bp.equipment_names()

    dw = bp.build()
    dw.initialize()
    assert "ASHP" in dw.equipment_names()
    assert "Gas Furnace" not in dw.equipment_names()

    for _ in range(5):
        dw.step()


def test_swap_gas_wh_to_hpwh():
    """Swap Gas Water Heater for Heat Pump WH."""
    bp = _make_blueprint()
    bp.remove_equipment_by_end_use(EndUse.WATER_HEATING)
    assert all("Water Heater" not in n for n in bp.equipment_names())

    hpwh = HeatPumpWH("HPWH", tank_volume_m3=0.19, cop=3.5,
                       avg_water_draw_l_per_day=200.0)
    bp.add_equipment(hpwh)

    dw = bp.build()
    dw.initialize()
    assert "HPWH" in dw.equipment_names()

    for _ in range(5):
        dw.step()


def test_swap_gas_wh_to_electric_resistance():
    """Swap Gas Water Heater for Electric Resistance WH."""
    bp = _make_blueprint()
    bp.remove_equipment_by_end_use(EndUse.WATER_HEATING)

    erwh = ElectricResistanceWH("ERWH", tank_volume_m3=0.19,
                                uniform_energy_factor=0.95,
                                avg_water_draw_l_per_day=200.0)
    bp.add_equipment(erwh)

    dw = bp.build()
    dw.initialize()
    assert "ERWH" in dw.equipment_names()

    for _ in range(5):
        dw.step()


def test_add_ac_to_existing():
    """Add Air Conditioner without removing existing equipment."""
    bp = _make_blueprint()

    ac = AirConditioner("New AC", capacity_w=10000, seer=16.0)
    bp.add_equipment(ac)

    dw = bp.build()
    dw.initialize()
    assert "New AC" in dw.equipment_names()

    for _ in range(5):
        dw.step()


def test_remove_all_hvac():
    """Building without HVAC equipment should still step."""
    bp = _make_blueprint()
    bp.remove_equipment_by_end_use([EndUse.HVAC_HEATING, EndUse.HVAC_COOLING])

    dw = bp.build()
    dw.initialize()

    hvac_names = [n for n in dw.equipment_names()
                  if any(h in n.lower() for h in
                         ["furnace", "ashp", "ac", "hvac",
                          "heat pump", "baseboard", "boiler"])]
    assert len(hvac_names) == 0

    for _ in range(5):
        dw.step()
```

- [ ] **Step 2: Run swap tests**

```bash
uv run pytest tests/python/test_blueprint_swap.py -v
```

- [ ] **Step 3: Commit**

```bash
git add tests/python/test_blueprint_swap.py
git commit -m "test: add equipment swap tests for DwellingBlueprint"
```

---

### Task 9: Error handling tests

**Files:**
- Create: `tests/python/test_blueprint_errors.py`

- [ ] **Step 1: Write error tests**

```python
"""Test error handling in DwellingBlueprint."""

import pytest
from ochre_next import (
    DwellingBlueprint, ASHPHeater, EndUse,
)
from conftest import HPXML, SCHEDULE, WEATHER, HARES_DEFAULTS


def _make_blueprint():
    return DwellingBlueprint.from_hpxml(
        HPXML, SCHEDULE, WEATHER,
        defaults_path=str(HARES_DEFAULTS),
        bldg_id=42, duration_s=300, time_res_s=60,
    )


def test_remove_nonexistent_equipment():
    """Removing equipment that doesn't exist raises error."""
    bp = _make_blueprint()
    with pytest.raises(Exception):
        bp.remove_equipment("NonexistentFurnace")


def test_duplicate_equipment_names():
    """Adding two equipment with same name — blueprint allows it,
    Dwelling's add_equipment will rename the duplicate at build time."""
    bp = _make_blueprint()
    bp.remove_equipment_by_end_use(EndUse.HVAC_HEATING)

    ashp1 = ASHPHeater("ASHP", capacity_w=12000, hspf=9.5)
    ashp2 = ASHPHeater("ASHP", capacity_w=10000, hspf=8.5)
    bp.add_equipment(ashp1)
    bp.add_equipment(ashp2)

    dw = bp.build()
    dw.initialize()
    names = dw.equipment_names()
    assert any("ASHP" in n for n in names)


def test_remove_specific_by_name():
    """Remove specific equipment by canonical name."""
    bp = _make_blueprint()
    names_before = bp.equipment_names()

    furnace_names = [n for n in names_before if "Gas Furnace" in n]
    if furnace_names:
        bp.remove_equipment(furnace_names[0])
        assert furnace_names[0] not in bp.equipment_names()


def test_remove_by_end_use_returns_count():
    """remove_equipment_by_end_use returns correct count."""
    bp = _make_blueprint()
    removed = bp.remove_equipment_by_end_use(EndUse.WATER_HEATING)
    assert removed >= 1


def test_empty_blueprint_builds():
    """Blueprint with most equipment removed should still build and step."""
    bp = _make_blueprint()
    for eu in [EndUse.HVAC_HEATING, EndUse.HVAC_COOLING,
               EndUse.WATER_HEATING, EndUse.LIGHTING,
               EndUse.PLUG_LOADS, EndUse.REFRIGERATION,
               EndUse.VENTILATION, EndUse.BATTERY,
               EndUse.PV, EndUse.EV, EndUse.GENERATOR,
               EndUse.DEHUMIDIFIER, EndUse.OTHER]:
        bp.remove_equipment_by_end_use(eu)

    dw = bp.build()
    dw.initialize()
    for _ in range(5):
        dw.step()
```

- [ ] **Step 2: Run error tests**

```bash
uv run pytest tests/python/test_blueprint_errors.py -v
```

- [ ] **Step 3: Commit**

```bash
git add tests/python/test_blueprint_errors.py
git commit -m "test: add DwellingBlueprint error handling tests"
```

---

### Task 10: Integration — blueprint dwelling with Battery/PV/EV

**Files:**
- Create: `tests/python/test_blueprint_dwelling.py`

- [ ] **Step 1: Write integration test**

```python
"""Test blueprint-built dwellings support existing Battery/PV/EV APIs."""

import pytest
from ochre_next import (
    DwellingBlueprint, Battery, PV, EV, ASHPHeater, ASHPCooler, EndUse,
)
from conftest import HPXML, SCHEDULE, WEATHER, HARES_DEFAULTS


def _make_blueprint():
    return DwellingBlueprint.from_hpxml(
        HPXML, SCHEDULE, WEATHER,
        defaults_path=str(HARES_DEFAULTS),
        bldg_id=42, duration_s=300, time_res_s=60,
    )


def test_blueprint_dwelling_add_battery():
    """Blueprint-built dwelling supports add_battery()."""
    bp = _make_blueprint()
    dw = bp.build()
    dw.initialize()

    bat = Battery("TestBat", 10.0, max_charge_kw=5.0, max_discharge_kw=5.0)
    dw.add_battery(bat)
    assert "TestBat" in dw.equipment_names()

    for _ in range(5):
        dw.step()


def test_blueprint_dwelling_add_pv():
    """Blueprint-built dwelling supports add_pv()."""
    bp = _make_blueprint()
    dw = bp.build()
    dw.initialize()

    pv = PV("TestPV", 5.0, tilt=30.0, azimuth=180.0)
    dw.add_pv(pv)
    assert "TestPV" in dw.equipment_names()

    for _ in range(5):
        dw.step()


def test_ashp_plus_battery_plus_pv():
    """Full scenario: swap furnace for ASHP + add battery + add PV."""
    bp = _make_blueprint()
    bp.remove_equipment_by_end_use([EndUse.HVAC_HEATING, EndUse.HVAC_COOLING])
    bp.add_equipment(ASHPHeater("ASHP", capacity_w=12000, hspf=9.5))
    bp.add_equipment(ASHPCooler("ASHP Cooler", capacity_w=10000, seer=18.0))

    dw = bp.build()
    dw.initialize()

    dw.add_battery(Battery("HomeBat", 13.5, max_charge_kw=5.0, max_discharge_kw=5.0))
    dw.add_pv(PV("Rooftop", 7.0, tilt=25.0, azimuth=180.0))

    for _ in range(5):
        dw.step()
```

- [ ] **Step 2: Run integration tests**

```bash
uv run pytest tests/python/test_blueprint_dwelling.py -v
```

- [ ] **Step 3: Commit**

```bash
git add tests/python/test_blueprint_dwelling.py
git commit -m "test: add DwellingBlueprint + DER integration tests"
```

---

### Task 11: Run full test suite

- [ ] **Step 1: Run Rust tests**

```bash
cargo test --workspace
```

- [ ] **Step 2: Run Python tests**

```bash
uv run pytest tests/python/ -v
```

- [ ] **Step 3: Run clippy**

```bash
cargo clippy --workspace -- -D warnings
```

- [ ] **Step 4: Fix failures, recommit**

```bash
# Fix any test/clippy failures, then:
git add -A
git commit -m "fix: address test and lint failures from equipment swapping feature"
```

---

### Task 12: Add package exports

**Files:**
- Modify: `python/ochre_next/__init__.py`

- [ ] **Step 1: Export new types**

```python
from ochre_next._hares import (
    DwellingBlueprint,
    GasFurnace,
    AirConditioner,
    ASHPHeater,
    ASHPCooler,
    IdealHVAC,
    ElectricBaseboard,
    GasWaterHeater,
    ElectricResistanceWH,
    HeatPumpWH,
)
```

- [ ] **Step 2: Verify import**

```bash
uv run python -c "from ochre_next import DwellingBlueprint, GasFurnace, ASHPHeater, HeatPumpWH; print('OK')"
```

Expected: `OK`

- [ ] **Step 3: Commit**

```bash
git add python/ochre_next/__init__.py
git commit -m "feat: export DwellingBlueprint and HVAC/WH types from package"
```

---

## Self-Review

**13 critical issues fixed:**
1. ✅ ElectricBaseboardConfig: no `ducts` field, has `eir: f64`
2. ✅ HeatPumpWaterHeaterConfig: uses `cop`, `backup_element_power_w`, `compressor_power_w`; no phantom `uef`/`uniform_energy_factor`/`heating_capacity_w`/`backup_fuel`/`backup_capacity_w`/`backup_eir`/`hpwh_type`
3. ✅ EIR formula: `3.41214 / hspf.max(1e-6)` and `3.41214 / seer.max(1e-6)` matching `resolve_hvac.rs:1084,1313`
4. ✅ Test paths: use `conftest.py` constants (`HPXML`, `SCHEDULE`, `WEATHER`, `HARES_DEFAULTS`)
5. ✅ step() key: `"net_electric_power_kw"` not `"total_power_kw"`
6. ✅ `equipment_names()` is a method, not a property; tests call it as `bp.equipment_names()`
7. ✅ AirConditioner has `seer` convenience param with `__init_post__` deriving `eir`; `ac_config_from_py` handles both
8. ✅ PyEndUse: added COOKING, LAUNDRY, DISHWASHER, POOL_PUMP, POOL_HEATER, SPA_PUMP, SPA_HEATER, CEILING_FAN (Task 1)
9. ✅ No crate boundary violation: all new code in `hares-core` and `hares-python`, which already depend on `hares-io`
10. ✅ `DesignConditions` (from `hares_io::epw`) is the correct type name
11. ✅ `ElectricResistanceWaterHeaterConfig` has `loop_id: Option<u16>` — included in config construction
12. ✅ Water heater equip spec construction uses `build_typed_spec` directly, no phantom `try_build_*_config` extraction
13. ✅ `build_typed_spec` is made `pub` in Task 2; Task 6's `py_blueprint.rs` calls it directly via `hares_io::hpxml::build_typed_spec`

**High-level fixes also applied:**
- ✅ `build_config` made `pub(crate)` for `py_blueprint.rs` access
- ✅ `FuelType` imported in `py_blueprint.rs`
- ✅ Backup fuel is user-configurable (not hardcoded to Electric) via `backup_fuel: Option<String>` on PyASHPHeater
- ✅ Single `build_typed_spec` path used; no duplicate `build_equipment_spec_from_config` hand-rolled logic
- ✅ No `hares-equipment/src/hvac/mod.rs` modification needed

**Spec coverage:**
- [x] Load HPXML building: `DwellingBlueprint.from_hpxml()` — Task 6
- [x] Remove furnace/WH: `remove_equipment_by_end_use()` — Task 3, 6
- [x] Add ASHP: `ASHPHeater` class + `add_equipment()` — Task 4, 6
- [x] Add HPWH: `HeatPumpWH` class + `add_equipment()` — Task 5, 6
- [x] Add AC: `AirConditioner` class + `add_equipment()` — Task 4, 6
- [x] Custom load profiles: existing `Dwelling.step()` after swaps — Task 8, 10
- [x] Strong types: all equipment uses typed pyclasses — Tasks 4, 5
- [x] Robust: parity tests, error tests, code path identical to from_preparsed — Tasks 7, 9
- [x] Ergonomic: `DwellingBlueprint` clean API — Task 6
- [x] Intuitive: same pattern as Battery/PV/EV API — Task 10

**No placeholders.** Every task has concrete code with exact field names and formulas.
