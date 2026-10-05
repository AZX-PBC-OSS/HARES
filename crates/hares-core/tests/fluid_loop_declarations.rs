//! Fluid-loop declarations come from the equipment's own fluid ports.
//!
//! The fluid solver's loop-type map is rebuilt from the instantiated
//! equipment's `ports()` fluid declarations at assembly and again whenever
//! the equipment list changes before the first step. This pins the loops the
//! former spec-name extraction missed: the shared DHW demand loop (declared
//! by water heaters, tankless units and wet appliances, never named in any
//! spec), and loops declared only by equipment added at runtime.

use std::path::PathBuf;

use chrono::{Duration, FixedOffset, TimeZone};
use hares_core::{Dwelling, DwellingConfig, SimulationConfig};
use hares_equipment::{
    DHW_DEMAND_LOOP, ElectricResistanceWaterHeaterConfig, Equipment, EquipmentConfig,
    EquipmentRegistry,
};
use hares_io::OutputFormat;
use hares_types::{
    ControlCapabilities, ControlSignal, CoreCapabilities, CoreOutput, EndUse, EquipmentDescriptor,
    EquipmentId, ExecutionStage, FluidType, FuelType, HaresError, LoopId, OperatingMode,
    PortDeclaration, PortSlots, Telemetry,
};

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// ResStock bldg0000007 fixture directory.
fn fixture_dir() -> PathBuf {
    project_root().join("tests/fixtures/resstock/2025.1/bldg0000007")
}

fn sim_config(duration: Duration) -> SimulationConfig {
    SimulationConfig {
        start_time: FixedOffset::west_opt(10 * 3600)
            .expect("UTC-10 offset is valid")
            .with_ymd_and_hms(2018, 1, 1, 0, 0, 0)
            .unwrap(),
        duration,
        time_res: Duration::seconds(900),
        output_verbosity: 0,
        write_output: false,
        output_path: None,
        output_format: OutputFormat::Csv,
        output_chunk_size: 1024,
        setpoint_deadband_c: None,
        master_seed: 0,
        civil_timezone: None,
        site_location: hares_io::SiteLocationOverride::default(),
        retain_batches: false,
        rotation: hares_io::RotationPolicy::None,
    }
}

fn dwelling_config(
    hpxml_path: PathBuf,
    schedule_path: PathBuf,
    weather_path: PathBuf,
    sim: SimulationConfig,
) -> DwellingConfig {
    DwellingConfig {
        hpxml_path,
        schedule_path: Some(schedule_path),
        weather_path,
        defaults_path: Some(project_root().join("defaults")),
        sim_config: sim,
        overrides: None,
        bldg_id: 100,
        initialization_duration: None,
        resample_overrides: Some(hares_io::ResampleOverrides::ochre_compat()),
        patches: None,
    }
}

fn resstock_bldg0000007_config(sim: SimulationConfig) -> DwellingConfig {
    dwelling_config(
        fixture_dir().join("home.xml"),
        fixture_dir().join("in.schedules.csv"),
        project_root().join("tests/fixtures/resstock/2025.1/weather/G1500030_2018.csv"),
        sim,
    )
}

/// The OCHRE `base-dhw-multiple.xml` sample: a tankless unit, the indirect
/// tank and a boiler, on the BEopt example schedule and the Denver weather
/// file.
fn ochre_base_dhw_multiple_config(sim: SimulationConfig) -> DwellingConfig {
    dwelling_config(
        project_root().join("tests/fixtures/hpxml/ochre_samples/base-dhw-multiple.xml"),
        project_root().join("data/examples/BEopt_example_schedule.csv"),
        project_root().join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        sim,
    )
}

/// Typed config for the probe water heater the runtime-add test registers.
fn probe_wh_config() -> ElectricResistanceWaterHeaterConfig {
    ElectricResistanceWaterHeaterConfig {
        equipment_id: None,
        zone_id: Some(1),
        loop_id: Some(7),
        tank_volume_m3: None,
        tank_height_m: None,
        energy_factor: None,
        uniform_energy_factor: None,
        heating_capacity_w: Some(4_500.0),
        ua_w_per_k: None,
        setpoint_c: Some(52.0),
        deadband_c: None,
        max_tank_temp_c: None,
        initial_tank_temp_c: None,
        tank_nodes: None,
        avg_water_draw_l_per_day: None,
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
        max_combined_power_w: None,
        fixture_delivery_temp_c: None,
        hot_draw_temp_c: None,
    }
}

fn built_probe_wh(env: &hares_types::EnvironmentState) -> Box<dyn Equipment> {
    let cfg = EquipmentConfig::from_typed(
        "Probe WH".to_string(),
        "Resistance Water Heater".to_string(),
        probe_wh_config(),
    )
    .expect("typed water-heater config");
    let registry = EquipmentRegistry::new();
    let mut heater = registry
        .create("Electric Resistance Water Heater", cfg.clone())
        .expect("registry must create the water heater");
    heater
        .init(&cfg, env)
        .expect("water heater inits against the dwelling's environment");
    heater
}

/// The fluid solver's loop-type map mirrors the dwelling's own fluid port
/// declarations: every loop an accumulator carries is declared, with the
/// type its declaring ports agree on. Over two inputs (the ResStock fixture's
/// storage water heater and wet appliances, and the OCHRE sample's tankless,
/// indirect tank and boiler) the map equals the union of the equipment's
/// declarations and the DHW demand loop is Water.
#[test]
fn fluid_loops_come_from_port_declarations() {
    for (label, config) in [
        (
            "resstock bldg0000007",
            resstock_bldg0000007_config(sim_config(Duration::hours(1))),
        ),
        (
            "ochre base-dhw-multiple",
            ochre_base_dhw_multiple_config(sim_config(Duration::hours(1))),
        ),
    ] {
        let dwelling = Dwelling::from_config(config)
            .unwrap_or_else(|err| panic!("{label}: dwelling builds from fixture: {err}"));

        let declared = hares_types::ports::fluid_loop_declarations(
            &dwelling
                .equipment()
                .iter()
                .flat_map(|eq| eq.ports().iter().copied())
                .collect::<Vec<_>>(),
        );
        assert!(
            declared.contains(&(DHW_DEMAND_LOOP, FluidType::Water)),
            "{label}: the DHW demand loop must be declared Water by the \
             fixture's equipment ports; declared: {declared:?}"
        );

        for acc in &dwelling.ports.fluid {
            assert_eq!(
                dwelling.fluid_solver.loop_fluid_type(acc.loop_id),
                Some(acc.fluid_type),
                "{label}: accumulator loop {:?} must be declared in the solver's \
                 map with the accumulator's fluid type",
                acc.loop_id
            );
        }
    }
}

/// Equipment added at runtime declares its loops through the same refresh
/// that rebuilds the port-slot table: the fluid solver's map gains the
/// added equipment's loop before the first step, and loses it when the
/// equipment leaves. Stepping with the runtime-added loop must not fail
/// as an undeclared loop.
#[test]
fn runtime_added_equipment_declares_its_loops() {
    let config = resstock_bldg0000007_config(sim_config(Duration::hours(1)));
    let mut dwelling = Dwelling::from_config(config).expect("dwelling builds from fixture");

    dwelling
        .add_equipment(built_probe_wh(dwelling.latest_env()))
        .expect("runtime add before the first step");

    assert_eq!(
        dwelling.fluid_solver.loop_fluid_type(LoopId(7)),
        Some(FluidType::Water),
        "the runtime-added water heater's loop must be declared from its port"
    );

    dwelling
        .remove_equipment("Probe WH")
        .expect("runtime remove while no step has run");
    assert_eq!(
        dwelling.fluid_solver.loop_fluid_type(LoopId(7)),
        Some(FluidType::Water),
        "the map mirrors the port-slot table, whose rebuild retains \
         accumulators whose declaring equipment left: the loop stays \
         declared while its retained accumulator is processed"
    );

    // Re-add and step: the declared loop must survive into the run, and the
    // step must not fail the loop as undeclared. After the first step the
    // map is frozen exactly like the port-slot table it mirrors.
    dwelling
        .add_equipment(built_probe_wh(dwelling.latest_env()))
        .expect("re-add before the first step");
    dwelling
        .step()
        .expect("a step with the runtime-added loop must not fail as undeclared");
}

/// A water heater declaring `Glycol` on the DHW demand loop, which the
/// fixture's water heater and wet appliances declare `Water`, is the
/// conflicting-type construction error: two ports declaring one loop with
/// different fluid types never build.
#[test]
fn conflicting_port_fluid_types_are_a_construction_error() {
    let config = resstock_bldg0000007_config(sim_config(Duration::hours(1)));
    let mut dwelling = Dwelling::from_config(config).expect("dwelling builds from fixture");

    dwelling
        .add_equipment(Box::new(GlycolDemandPortEquipment::new(
            "Glycol Probe".to_string(),
        )))
        .expect_err(
            "adding a Glycol port on the Water-declared DHW demand loop must \
             fail the construction",
        );
}

/// A minimal equipment whose single fluid port declares the DHW demand loop
/// as `Glycol`.
struct GlycolDemandPortEquipment {
    descriptor: EquipmentDescriptor,
    ports: Vec<PortDeclaration>,
    telemetry: Telemetry,
    core_output: CoreOutput,
}

impl GlycolDemandPortEquipment {
    fn new(name: String) -> Self {
        Self {
            descriptor: EquipmentDescriptor {
                id: EquipmentId(0),
                name,
                end_use: EndUse::OTHER,
                equipment_type: std::borrow::Cow::Borrowed("GlycolDemandPortEquipment"),
                zone: None,
                fuel: FuelType::Electric,
                stage: ExecutionStage::Independent,
                control_capabilities: ControlCapabilities::empty(),
                core_capabilities: CoreCapabilities::empty(),
                telemetry_fields: vec![],
                zone_type: None,
            },
            ports: vec![PortDeclaration::fluid(DHW_DEMAND_LOOP, FluidType::Glycol)],
            telemetry: Telemetry::with_capacity(1),
            core_output: CoreOutput::default(),
        }
    }
}

impl Equipment for GlycolDemandPortEquipment {
    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.descriptor
    }

    fn rename(&mut self, name: String) {
        self.descriptor.name = name;
    }

    fn set_equipment_id(&mut self, id: EquipmentId) -> Result<(), HaresError> {
        hares_equipment::apply_identity_write(self.is_initialized(), &mut self.descriptor, id)
    }

    fn ports(&self) -> &[PortDeclaration] {
        &self.ports
    }

    fn init(
        &mut self,
        _config: &EquipmentConfig,
        _env: &hares_types::EnvironmentState,
    ) -> Result<(), HaresError> {
        Ok(())
    }

    fn update_control(&mut self, _env: &hares_types::EnvironmentState) -> OperatingMode {
        OperatingMode::Off
    }

    fn step(
        &mut self,
        _env: &hares_types::EnvironmentState,
        _dt: std::time::Duration,
        _ports: &mut PortSlots,
    ) -> Result<(), HaresError> {
        Ok(())
    }

    fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    fn core_output(&self) -> &CoreOutput {
        &self.core_output
    }

    fn save_state(&self) -> Result<Vec<u8>, HaresError> {
        Ok(vec![])
    }

    fn load_state(&mut self, _state: &[u8]) -> Result<(), HaresError> {
        Ok(())
    }

    fn apply_signal(&mut self, _signal: &ControlSignal) -> Result<(), HaresError> {
        Ok(())
    }
}
