//! Port contribution types, port slots, and port declarations.
//!
//! Ports are the interface through which equipment communicates thermal,
//! electrical, and fluid contributions to the envelope solver.

use serde::{Deserialize, Serialize};

use crate::{DomainId, FluidNodeId, FluidType, FuelType, HaresError, LoopId, ZoneId};

pub const CUSTOM_PAYLOAD_LEN: usize = 16;

/// Classification of a thermal contribution's physical origin.
///
/// Used to partition `ThermalAccumulator::sensible_by_category`,
/// `radiant_by_category`, and `latent_by_category` without allocating. The
/// ordinal of each variant must match its index in those arrays.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ThermalCategory {
    /// Intentional zone heating (HVAC systems).
    HvacHeating,
    /// Intentional zone cooling (HVAC systems).
    HvacCooling,
    /// Waste heat from appliances, lighting, occupancy.
    #[default]
    InternalGain,
    /// Equipment shell/jacket losses (water heaters, boilers).
    JacketLoss,
    /// Distribution system inefficiency (duct losses).
    DuctLoss,
    /// Standalone dehumidifier: intentional mechanical moisture removal.
    /// EnergyPlus classifies the ZoneDehumidifier as zone HVAC equipment, not
    /// an internal gain source (Eng. Ref., Zone Equipment and Zone Forced Air Units).
    HvacDehumidification,
}

impl ThermalCategory {
    /// Array index for per-category storage. Must be kept in sync with the
    /// variant ordering and `THERMAL_CATEGORY_COUNT`.
    #[inline]
    pub fn index(self) -> usize {
        match self {
            ThermalCategory::HvacHeating => 0,
            ThermalCategory::HvacCooling => 1,
            ThermalCategory::InternalGain => 2,
            ThermalCategory::JacketLoss => 3,
            ThermalCategory::DuctLoss => 4,
            ThermalCategory::HvacDehumidification => 5,
        }
    }
}

/// Number of `ThermalCategory` variants -- size of the per-category array.
pub const THERMAL_CATEGORY_COUNT: usize = 6;

/// Whether equipment acts as a heat source (injects heat into the fluid) or a
/// heat sink (extracts heat from the fluid).
///
/// Used to sign fluid port contributions so that `net_power_w` correctly
/// reflects the algebraic sum of heat injected and extracted, rather than
/// conflating both with the same sign convention.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HeatTransferDirection {
    /// Equipment injects heat into the fluid loop (e.g. boiler, heat pump in
    /// heating mode).
    #[default]
    Source,
    /// Equipment extracts heat from the fluid loop (e.g. distribution coil,
    /// radiant floor, cooling coil).
    Sink,
}

/// Per-step equipment contribution into a typed simulation port.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PortContribution {
    Thermal {
        zone: ZoneId,
        /// Convective sensible gain [W]: goes directly to zone air.
        sensible_gain_w: f64,
        /// Radiant sensible gain [W]: distributed to surface nodes via TMULT.
        /// sensible_gain_w + radiant_gain_w = total sensible gain.
        radiant_gain_w: f64,
        latent_gain_w: f64,
        category: ThermalCategory,
    },
    Electrical {
        active_power_w: f64,
        reactive_power_kvar: f64,
    },
    Fuel {
        fuel_type: FuelType,
        consumption_w: f64,
    },
    Fluid {
        loop_id: LoopId,
        flow_rate_kg_s: f64,
        supply_temp_c: f64,
        return_temp_c: f64,
        fluid_type: FluidType,
        /// Declared thermal power delivered to this loop [W].
        /// None when the contributor does not quantify thermal energy
        /// (e.g. static temperature/flow, or no thermal recovery active).
        thermal_power_w: Option<f64>,
        /// Hydraulic node within the loop topology.
        /// `FluidNodeId(0)` is the default serial node.
        node_id: FluidNodeId,
        /// Whether this equipment is a heat source (injecting heat) or a
        /// heat sink (extracting heat). Used to correctly sign the power
        /// contribution when computing `net_power_w`.
        direction: HeatTransferDirection,
    },
    Custom {
        domain_id: DomainId,
        payload: [f64; CUSTOM_PAYLOAD_LEN],
    },
    Humidity {
        zone: ZoneId,
        moisture_mass_flow_kg_s: f64,
    },
}

/// Port kind tag used for init-time wiring validation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PortType {
    Thermal,
    Electrical,
    Fuel,
    Fluid,
    Custom,
    Humidity,
}

/// Port declaration used to pre-size and validate port slot wiring.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortDeclaration {
    pub port_type: PortType,
    pub zone: Option<ZoneId>,
    pub loop_id: Option<LoopId>,
    pub domain_id: Option<DomainId>,
    pub fluid_type: Option<FluidType>,
    /// When present, declares this fluid port for a specific hydraulic
    /// node within the loop topology. `None` defaults to `FluidNodeId(0)`
    /// (serial loop). Set when contributing to a parallel-branch node.
    pub fluid_node_id: Option<FluidNodeId>,
}

impl PortDeclaration {
    pub fn electrical() -> Self {
        Self {
            port_type: PortType::Electrical,
            zone: None,
            loop_id: None,
            domain_id: None,
            fluid_type: None,
            fluid_node_id: None,
        }
    }

    pub fn thermal(zone: ZoneId) -> Self {
        Self {
            port_type: PortType::Thermal,
            zone: Some(zone),
            loop_id: None,
            domain_id: None,
            fluid_type: None,
            fluid_node_id: None,
        }
    }

    pub fn fuel() -> Self {
        Self {
            port_type: PortType::Fuel,
            zone: None,
            loop_id: None,
            domain_id: None,
            fluid_type: None,
            fluid_node_id: None,
        }
    }

    pub fn fluid(loop_id: LoopId, fluid_type: FluidType) -> Self {
        Self {
            port_type: PortType::Fluid,
            zone: None,
            loop_id: Some(loop_id),
            domain_id: None,
            fluid_type: Some(fluid_type),
            fluid_node_id: None,
        }
    }

    /// Declares a fluid port for a specific hydraulic node within the loop
    /// topology. Use for parallel-branch equipment that needs per-node flow
    /// resolution.
    pub fn fluid_with_node(loop_id: LoopId, fluid_type: FluidType, node_id: FluidNodeId) -> Self {
        Self {
            port_type: PortType::Fluid,
            zone: None,
            loop_id: Some(loop_id),
            domain_id: None,
            fluid_type: Some(fluid_type),
            fluid_node_id: Some(node_id),
        }
    }

    pub fn custom(domain_id: DomainId) -> Self {
        Self {
            port_type: PortType::Custom,
            zone: None,
            loop_id: None,
            domain_id: Some(domain_id),
            fluid_type: None,
            fluid_node_id: None,
        }
    }

    pub fn humidity(zone: ZoneId) -> Self {
        Self {
            port_type: PortType::Humidity,
            zone: Some(zone),
            loop_id: None,
            domain_id: None,
            fluid_type: None,
            fluid_node_id: None,
        }
    }
}

/// Thermal contribution totals for one zone.
///
/// `sensible_gain_w` is the **convective** sensible total (goes directly to zone
/// air); it is *not* the total sensible gain. Total sensible = sensible_gain_w +
/// radiant_gain_w. `latent_gain_w` is the zone total (sum across all categories).
/// `sensible_by_category`, `radiant_by_category`, and `latent_by_category` hold
/// per-category subtotals indexed by `ThermalCategory::index()`. Invariants:
/// `sum(sensible_by_category) == sensible_gain_w` (convective-only total);
/// `sum(latent_by_category) == latent_gain_w`.
/// `sensible_by_category[HvacCooling]` includes the fan heat offset — the
/// positive fan waste heat is folded into the negative coil cooling. For
/// coil-only cooling output (without fan heat), use equipment telemetry
/// fields `coil_sensible_cooling_w` and `coil_latent_cooling_w`.
/// Use fixed-size arrays to avoid HashMap allocation in the hot timestep loop.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ThermalAccumulator {
    pub zone: ZoneId,
    pub sensible_gain_w: f64,
    pub radiant_gain_w: f64,
    pub latent_gain_w: f64,
    pub sensible_by_category: [f64; THERMAL_CATEGORY_COUNT],
    pub radiant_by_category: [f64; THERMAL_CATEGORY_COUNT],
    pub latent_by_category: [f64; THERMAL_CATEGORY_COUNT],
}

impl ThermalAccumulator {
    pub fn new(zone: ZoneId) -> Self {
        Self {
            zone,
            sensible_gain_w: 0.0,
            radiant_gain_w: 0.0,
            latent_gain_w: 0.0,
            sensible_by_category: [0.0; THERMAL_CATEGORY_COUNT],
            radiant_by_category: [0.0; THERMAL_CATEGORY_COUNT],
            latent_by_category: [0.0; THERMAL_CATEGORY_COUNT],
        }
    }

    pub fn add(
        &mut self,
        sensible_gain_w: f64,
        radiant_gain_w: f64,
        latent_gain_w: f64,
        category: ThermalCategory,
    ) {
        self.sensible_gain_w += sensible_gain_w;
        self.radiant_gain_w += radiant_gain_w;
        self.latent_gain_w += latent_gain_w;
        self.sensible_by_category[category.index()] += sensible_gain_w;
        self.radiant_by_category[category.index()] += radiant_gain_w;
        self.latent_by_category[category.index()] += latent_gain_w;
    }

    pub fn zero(&mut self) {
        self.sensible_gain_w = 0.0;
        self.radiant_gain_w = 0.0;
        self.latent_gain_w = 0.0;
        self.sensible_by_category = [0.0; THERMAL_CATEGORY_COUNT];
        self.radiant_by_category = [0.0; THERMAL_CATEGORY_COUNT];
        self.latent_by_category = [0.0; THERMAL_CATEGORY_COUNT];
    }

    /// Sensible gain total for a specific category.
    pub fn sensible_for_category(&self, cat: ThermalCategory) -> f64 {
        self.sensible_by_category[cat.index()]
    }

    /// Radiant gain total for a specific category.
    pub fn radiant_for_category(&self, cat: ThermalCategory) -> f64 {
        self.radiant_by_category[cat.index()]
    }

    /// Latent gain total for a specific category.
    pub fn latent_for_category(&self, cat: ThermalCategory) -> f64 {
        self.latent_by_category[cat.index()]
    }
}

/// Electrical contribution totals on the shared v1 bus.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct ElectricalAccumulator {
    pub reactive_power_kvar: f64,
    pub load_power_w: f64,
    pub generation_power_w: f64,
    /// Total count of electrical contributions accumulated this timestep.
    /// Gated on `observe` so zero overhead in production builds. The
    /// hares-core observer layer reads this counter and cross-references it
    /// with equipment EndUse to identify mis-signed generation contributions.
    #[cfg(feature = "observe")]
    #[serde(skip)]
    pub electrical_contribution_count: usize,
}

impl ElectricalAccumulator {
    /// Net active power [W]: load (positive) + generation (negative).
    pub fn net_active_w(&self) -> f64 {
        self.load_power_w + self.generation_power_w
    }

    pub fn zero(&mut self) {
        *self = Self::default();
    }
}

/// Number of fuel types excluding `FuelType::None`.
pub const FUEL_TYPE_COUNT: usize = 7;

/// All fuel types in their canonical ordinal order (index 0..FUEL_TYPE_COUNT).
/// Use this as the single source of truth for iterating all real fuel types.
pub const ALL_FUEL_TYPES: [FuelType; FUEL_TYPE_COUNT] = [
    FuelType::Electric,
    FuelType::Gas,
    FuelType::Propane,
    FuelType::Oil,
    FuelType::Wood,
    FuelType::Coal,
    FuelType::WoodPellet,
];

/// Maps a `FuelType` to its array index in `FuelAccumulator::totals`.
/// `FuelType::None` returns `None`.
pub fn fuel_index(fuel_type: FuelType) -> Option<usize> {
    match fuel_type {
        FuelType::Electric => Some(0),
        FuelType::Gas => Some(1),
        FuelType::Propane => Some(2),
        FuelType::Oil => Some(3),
        FuelType::Wood => Some(4),
        FuelType::Coal => Some(5),
        FuelType::WoodPellet => Some(6),
        FuelType::None => None,
    }
}

/// Reverse lookup: maps a `FuelAccumulator` index (0..FUEL_TYPE_COUNT) back
/// to the corresponding `FuelType`. Returns `None` for out-of-range indices.
pub fn fuel_index_reverse(idx: usize) -> Option<FuelType> {
    match idx {
        0 => Some(FuelType::Electric),
        1 => Some(FuelType::Gas),
        2 => Some(FuelType::Propane),
        3 => Some(FuelType::Oil),
        4 => Some(FuelType::Wood),
        5 => Some(FuelType::Coal),
        6 => Some(FuelType::WoodPellet),
        _ => None,
    }
}

/// Fuel consumption totals grouped by fuel type.
///
/// Indexed by fuel type ordinal: [Electric=0, Gas=1, Propane=2, Oil=3,
/// Wood=4, Coal=5, WoodPellet=6].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct FuelAccumulator {
    totals: [f64; FUEL_TYPE_COUNT],
}

impl FuelAccumulator {
    pub fn zero(&mut self) {
        self.totals = [0.0; FUEL_TYPE_COUNT];
    }

    pub fn add(&mut self, fuel_type: FuelType, consumption_w: f64) -> Result<(), HaresError> {
        match fuel_index(fuel_type) {
            Some(idx) => {
                self.totals[idx] += consumption_w;
                Ok(())
            }
            None => Err(HaresError::Equipment(
                "FuelType::None should not write fuel port contributions".to_string(),
            )),
        }
    }

    pub fn get(&self, fuel_type: FuelType) -> f64 {
        fuel_index(fuel_type).map_or(0.0, |idx| self.totals[idx])
    }
}

/// Fluid contribution totals for one loop, fluid type, and hydraulic node.
///
/// The `node_id` identifies this accumulator's position in the loop topology.
/// When no explicit topology is configured, `FluidNodeId(0)` is the implied
/// default serial node for the entire loop.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct FluidAccumulator {
    pub loop_id: LoopId,
    pub fluid_type: FluidType,
    /// Hydraulic node this accumulator represents within the loop topology.
    pub node_id: FluidNodeId,
    pub total_flow_kg_s: f64,
    pub mean_supply_temp_c: f64,
    pub mean_return_temp_c: f64,
    /// Sum of declared `thermal_power_w` values from all fluid contributions
    /// targeting this loop. None-aware: when a contributor sets
    /// `thermal_power_w = None` that contribution contributes zero here.
    pub total_thermal_power_w: f64,
    /// Whether the equipment writing to this accumulator is a heat source
    /// (injecting heat into the fluid) or a heat sink (extracting heat).
    /// `None` when the accumulator was created from a declaration and has
    /// not yet received a contribution; set to `Some(direction)` on the
    /// first call to [`add`](Self::add).
    pub direction: Option<HeatTransferDirection>,
}

impl FluidAccumulator {
    /// Creates a new accumulator for the default serial node (`FluidNodeId(0)`).
    pub fn new(loop_id: LoopId, fluid_type: FluidType) -> Self {
        Self {
            loop_id,
            fluid_type,
            node_id: FluidNodeId(0),
            total_flow_kg_s: 0.0,
            mean_supply_temp_c: 0.0,
            mean_return_temp_c: 0.0,
            total_thermal_power_w: 0.0,
            direction: None,
        }
    }

    /// Creates a new accumulator for a specific hydraulic node.
    pub fn with_node(loop_id: LoopId, fluid_type: FluidType, node_id: FluidNodeId) -> Self {
        Self {
            loop_id,
            fluid_type,
            node_id,
            total_flow_kg_s: 0.0,
            mean_supply_temp_c: 0.0,
            mean_return_temp_c: 0.0,
            total_thermal_power_w: 0.0,
            direction: None,
        }
    }

    pub fn add(
        &mut self,
        flow_rate_kg_s: f64,
        supply_temp_c: f64,
        return_temp_c: f64,
        thermal_power_w: Option<f64>,
        direction: HeatTransferDirection,
    ) -> Result<(), HaresError> {
        if flow_rate_kg_s < 0.0 {
            return Err(HaresError::Equipment("negative flow rate".to_string()));
        }
        // When the accumulator was created from a declaration without direction
        // info, the first contribution sets the direction. Subsequent
        // contributions must match (same node = same equipment = same role).
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            if let Some(existing) = self.direction {
                if existing != direction {
                    return Err(HaresError::Equipment(format!(
                        "direction mismatch on fluid accumulator {:?}/{:?}/{:?}: \
                         got {direction:?}, expected {existing:?}",
                        self.loop_id, self.fluid_type, self.node_id
                    )));
                }
            }
        }
        if self.direction.is_none() {
            self.direction = Some(direction);
        }
        const MIN_FLOW_KG_S: f64 = 1e-9;
        let new_total_flow = self.total_flow_kg_s + flow_rate_kg_s;
        if new_total_flow.abs() > MIN_FLOW_KG_S {
            self.mean_supply_temp_c = ((self.mean_supply_temp_c * self.total_flow_kg_s)
                + (supply_temp_c * flow_rate_kg_s))
                / new_total_flow;
            self.mean_return_temp_c = ((self.mean_return_temp_c * self.total_flow_kg_s)
                + (return_temp_c * flow_rate_kg_s))
                / new_total_flow;
        }
        self.total_flow_kg_s = new_total_flow;
        if let Some(tpw) = thermal_power_w {
            self.total_thermal_power_w += tpw;
        }
        Ok(())
    }

    pub fn zero(&mut self) {
        self.total_flow_kg_s = 0.0;
        self.mean_supply_temp_c = 0.0;
        self.mean_return_temp_c = 0.0;
        self.total_thermal_power_w = 0.0;
    }
}

/// Summed custom payload for one registered domain.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CustomAccumulator {
    pub domain_id: DomainId,
    pub payload: [f64; CUSTOM_PAYLOAD_LEN],
}

impl CustomAccumulator {
    pub fn new(domain_id: DomainId) -> Self {
        Self {
            domain_id,
            payload: [0.0; CUSTOM_PAYLOAD_LEN],
        }
    }

    pub fn add(&mut self, payload: [f64; CUSTOM_PAYLOAD_LEN]) {
        for (total, value) in self.payload.iter_mut().zip(payload) {
            *total += value;
        }
    }

    pub fn zero(&mut self) {
        self.payload = [0.0; CUSTOM_PAYLOAD_LEN];
    }
}

/// Humidity contribution totals for one zone.
///
/// Accumulates `moisture_mass_flow_kg_s` from equipment that explicitly
/// reports moisture removal/addition as a mass-flow rate (kg/s), bypassing
/// the latent-energy→humidity-ratio conversion that depends on a consistent
/// h_fg constant across all participants.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct HumidityAccumulator {
    pub zone: ZoneId,
    pub moisture_mass_flow_kg_s: f64,
}

impl HumidityAccumulator {
    pub fn new(zone: ZoneId) -> Self {
        Self {
            zone,
            moisture_mass_flow_kg_s: 0.0,
        }
    }

    pub fn add(&mut self, moisture_mass_flow_kg_s: f64) {
        self.moisture_mass_flow_kg_s += moisture_mass_flow_kg_s;
    }

    pub fn zero(&mut self) {
        self.moisture_mass_flow_kg_s = 0.0;
    }
}

/// Preallocated per-timestep accumulation slots.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct PortSlots {
    pub thermal: Vec<ThermalAccumulator>,
    pub electrical: ElectricalAccumulator,
    pub fuel: FuelAccumulator,
    pub fluid: Vec<FluidAccumulator>,
    pub custom: Vec<CustomAccumulator>,
    pub humidity: Vec<HumidityAccumulator>,
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    #[serde(skip)]
    pub write_log: std::collections::HashSet<String>,
}

impl PortSlots {
    /// Build pre-sized PortSlots from port declarations.
    /// This is the canonical construction path -- ensures accumulators
    /// match declared ports and rejects undeclared contributions at runtime.
    pub fn from_declarations(decls: &[PortDeclaration]) -> Self {
        let mut thermal = Vec::new();
        let mut fluid = Vec::new();
        let mut custom = Vec::new();
        let mut humidity = Vec::new();

        for decl in decls {
            match decl.port_type {
                PortType::Thermal => {
                    if let Some(zone) = decl.zone {
                        if !thermal.iter().any(|t: &ThermalAccumulator| t.zone == zone) {
                            thermal.push(ThermalAccumulator::new(zone));
                        }
                    }
                }
                PortType::Fluid => {
                    if let (Some(loop_id), Some(fluid_type)) = (decl.loop_id, decl.fluid_type) {
                        let node_id = decl.fluid_node_id.unwrap_or(FluidNodeId(0));
                        if !fluid.iter().any(|f: &FluidAccumulator| {
                            f.loop_id == loop_id
                                && f.fluid_type == fluid_type
                                && f.node_id == node_id
                        }) {
                            fluid.push(FluidAccumulator::with_node(loop_id, fluid_type, node_id));
                        }
                    }
                }
                PortType::Custom => {
                    if let Some(domain_id) = decl.domain_id {
                        if !custom
                            .iter()
                            .any(|c: &CustomAccumulator| c.domain_id == domain_id)
                        {
                            custom.push(CustomAccumulator::new(domain_id));
                        }
                    }
                }
                PortType::Humidity => {
                    if let Some(zone) = decl.zone {
                        if !humidity
                            .iter()
                            .any(|h: &HumidityAccumulator| h.zone == zone)
                        {
                            humidity.push(HumidityAccumulator::new(zone));
                        }
                    }
                }
                // Electrical and Fuel are singletons; pre-initialized via Default.
                PortType::Electrical | PortType::Fuel => {}
            }
        }

        Self {
            thermal,
            electrical: ElectricalAccumulator::default(),
            fuel: FuelAccumulator::default(),
            fluid,
            custom,
            humidity,
            #[cfg(any(debug_assertions, feature = "check_invariants"))]
            write_log: std::collections::HashSet::new(),
        }
    }

    /// Copy `source` into `self`, reusing existing allocations.
    ///
    /// Unlike `clone_into` from the `ToOwned` trait (or `self.clone()`),
    /// this method reuses the pre-allocated Vec capacities in `self`
    /// (via `Vec::clone_from`) and copies the scalar accumulators by value.
    /// When called on a `PortSlots` that was initialised from the same
    /// declarations as `source`, no heap allocation occurs — only
    /// element-wise copies whose cost is proportional to the number of
    /// declared accumulators.
    pub fn copy_into(&mut self, source: &Self) {
        self.thermal.clone_from(&source.thermal);
        self.electrical = source.electrical;
        self.fuel = source.fuel;
        self.fluid.clone_from(&source.fluid);
        self.custom.clone_from(&source.custom);
        self.humidity.clone_from(&source.humidity);
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            self.write_log.clone_from(&source.write_log);
        }
    }

    pub fn zero(&mut self) {
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        self.warn_unwritten();

        for thermal in &mut self.thermal {
            thermal.zero();
        }
        self.electrical.zero();
        self.fuel.zero();
        for fluid in &mut self.fluid {
            fluid.zero();
        }
        for custom in &mut self.custom {
            custom.zero();
        }
        for humidity in &mut self.humidity {
            humidity.zero();
        }
    }

    /// Check whether every declared port slot received at least one write
    /// during the preceding timestep. Logs a warning for each declared port
    /// that received zero contributions.
    ///
    /// Gated behind `debug_assertions` or the `check_invariants` feature so
    /// it imposes zero overhead in release builds.
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    fn warn_unwritten(&mut self) {
        use tracing::warn;

        // Derive expected keys from accumulator vectors built by
        // from_declarations. Only vector-based accumulators are checked;
        // Electrical and Fuel are singletons always present regardless of
        // declarations, so we cannot distinguish "declared" from "default".
        let mut expected: Vec<String> = Vec::new();
        for acc in &self.thermal {
            expected.push(format!("Thermal:{:?}", acc.zone));
        }
        for acc in &self.fluid {
            expected.push(format!("Fluid:{:?}:{:?}", acc.loop_id, acc.fluid_type));
        }
        for acc in &self.custom {
            expected.push(format!("Custom:{:?}", acc.domain_id));
        }
        for acc in &self.humidity {
            expected.push(format!("Humidity:{:?}", acc.zone));
        }

        for key in &expected {
            if !self.write_log.contains(key.as_str()) {
                warn!(
                    port = key.as_str(),
                    "declared port received zero contributions during preceding timestep"
                );
            }
        }

        self.write_log.clear();
    }

    pub fn accumulate(&mut self, contribution: &PortContribution) -> Result<(), HaresError> {
        match contribution {
            PortContribution::Thermal {
                zone,
                sensible_gain_w,
                radiant_gain_w,
                latent_gain_w,
                category,
            } => {
                if let Some(total) = self.thermal.iter_mut().find(|entry| entry.zone == *zone) {
                    total.add(*sensible_gain_w, *radiant_gain_w, *latent_gain_w, *category);
                    #[cfg(any(debug_assertions, feature = "check_invariants"))]
                    {
                        self.write_log.insert(format!("Thermal:{zone:?}"));
                    }
                } else {
                    return Err(HaresError::Equipment(format!(
                        "undeclared thermal zone: {zone:?}"
                    )));
                }
            }
            PortContribution::Electrical {
                active_power_w,
                reactive_power_kvar,
            } => {
                // Defensive sign-convention validation: electrical power values
                // must be finite. Non-finite (NaN, ±∞) values indicate an
                // equipment bug upstream of the port boundary.
                debug_assert!(
                    active_power_w.is_finite(),
                    "PortContribution::Electrical received non-finite active_power_w: {active_power_w}"
                );
                debug_assert!(
                    reactive_power_kvar.is_finite(),
                    "PortContribution::Electrical received non-finite reactive_power_kvar: {reactive_power_kvar}"
                );

                self.electrical.reactive_power_kvar += reactive_power_kvar;
                if *active_power_w >= 0.0 {
                    self.electrical.load_power_w += active_power_w;
                } else {
                    self.electrical.generation_power_w += active_power_w;
                }

                // Defense against unit regression: if a callee re-introduces
                // kW-scaled values into the W port (e.g. writes 15 W from a
                // 15 kW piece of equipment) the accumulator will surge to
                // factor-1000 the true value.

                #[cfg(any(debug_assertions, feature = "check_invariants"))]
                {
                    // Invariant: accumulated load power must be non-negative, and
                    // generation must be non-positive. A sign violation indicates a
                    // contributor mis-classified its contribution (e.g. a generator
                    // writing a positive value to the electrical port).
                    //
                    // Absolute magnitude is not clamped — a single unrealistic
                    // accumulation (e.g. a pathological COP→0 heat-pump test drawing
                    // GW-scale electric power in one step) would be a false positive.
                    // The guard exists to catch sign errors, NaN/Inf propagation,
                    // and the factor-1000 unit regression pattern where a kW value
                    // silently enters the W port.
                    debug_assert!(
                        self.electrical.load_power_w >= 0.0,
                        "electrical load_power_w must be non-negative, got {}",
                        self.electrical.load_power_w
                    );
                    debug_assert!(
                        self.electrical.generation_power_w <= 0.0,
                        "electrical generation_power_w must be non-positive, got {}",
                        self.electrical.generation_power_w
                    );
                }

                #[cfg(feature = "observe")]
                {
                    self.electrical.electrical_contribution_count += 1;
                }

                #[cfg(any(debug_assertions, feature = "check_invariants"))]
                {
                    self.write_log.insert("Electrical".to_string());
                }
            }
            PortContribution::Fuel {
                fuel_type,
                consumption_w,
            } => {
                self.fuel.add(*fuel_type, *consumption_w)?;
                #[cfg(any(debug_assertions, feature = "check_invariants"))]
                {
                    self.write_log.insert(format!("Fuel:{fuel_type:?}"));
                }
            }
            PortContribution::Fluid {
                loop_id,
                flow_rate_kg_s,
                supply_temp_c,
                return_temp_c,
                fluid_type,
                thermal_power_w,
                node_id,
                direction,
            } => {
                if let Some(total) = self.fluid.iter_mut().find(|entry| {
                    entry.loop_id == *loop_id
                        && entry.fluid_type == *fluid_type
                        && entry.node_id == *node_id
                }) {
                    total.add(
                        *flow_rate_kg_s,
                        *supply_temp_c,
                        *return_temp_c,
                        *thermal_power_w,
                        *direction,
                    )?;
                    #[cfg(any(debug_assertions, feature = "check_invariants"))]
                    {
                        self.write_log
                            .insert(format!("Fluid:{loop_id:?}:{fluid_type:?}:{node_id:?}"));
                    }
                } else {
                    return Err(HaresError::Equipment(format!(
                        "undeclared fluid loop: {loop_id:?} with fluid type {fluid_type:?} and node {node_id:?}"
                    )));
                }
            }
            PortContribution::Custom { domain_id, payload } => {
                if let Some(total) = self
                    .custom
                    .iter_mut()
                    .find(|entry| entry.domain_id == *domain_id)
                {
                    total.add(*payload);
                    #[cfg(any(debug_assertions, feature = "check_invariants"))]
                    {
                        self.write_log.insert(format!("Custom:{domain_id:?}"));
                    }
                } else {
                    return Err(HaresError::Equipment(format!(
                        "undeclared custom domain: {domain_id:?}"
                    )));
                }
            }
            PortContribution::Humidity {
                zone,
                moisture_mass_flow_kg_s,
            } => {
                if let Some(total) = self.humidity.iter_mut().find(|entry| entry.zone == *zone) {
                    total.add(*moisture_mass_flow_kg_s);
                    #[cfg(any(debug_assertions, feature = "check_invariants"))]
                    {
                        self.write_log.insert(format!("Humidity:{zone:?}"));
                    }
                } else {
                    return Err(HaresError::Equipment(format!(
                        "undeclared humidity zone: {zone:?}"
                    )));
                }
            }
        }
        Ok(())
    }
}

/// Validate that all `PortDeclaration`s with the same `loop_id` agree on
/// `fluid_type`. A mismatch means two pieces of equipment are wired to the same
/// fluid loop but disagree about the fluid — a configuration error that produces
/// silently incorrect simulation results if not caught.
///
/// Returns `Err` on the first detected mismatch, naming the conflicting loop_id
/// and fluid types.
pub fn validate_fluid_type_consistency(decls: &[PortDeclaration]) -> Result<(), HaresError> {
    use std::collections::HashMap;

    let mut loop_fluid: HashMap<LoopId, FluidType> = HashMap::new();
    for decl in decls {
        if decl.port_type != PortType::Fluid {
            continue;
        }
        let Some(loop_id) = decl.loop_id else {
            continue;
        };
        let Some(fluid_type) = decl.fluid_type else {
            continue;
        };
        if let Some(existing) = loop_fluid.insert(loop_id, fluid_type) {
            if existing != fluid_type {
                return Err(HaresError::Equipment(format!(
                    "loop {loop_id:?} declared with conflicting fluid types: \
                     {existing:?} and {fluid_type:?}"
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(left: f64, right: f64) {
        assert!((left - right).abs() < 1e-9, "left={left}, right={right}");
    }

    #[test]
    fn thermal_contributions_are_summed() {
        let zone = ZoneId(3);
        let mut slots = PortSlots {
            thermal: vec![ThermalAccumulator::new(zone)],
            ..Default::default()
        };

        slots
            .accumulate(&PortContribution::Thermal {
                zone,
                sensible_gain_w: 100.0,
                radiant_gain_w: 0.0,
                latent_gain_w: 20.0,
                category: ThermalCategory::InternalGain,
            })
            .unwrap();
        slots
            .accumulate(&PortContribution::Thermal {
                zone,
                sensible_gain_w: -10.0,
                radiant_gain_w: 0.0,
                latent_gain_w: 5.0,
                category: ThermalCategory::InternalGain,
            })
            .unwrap();
        slots
            .accumulate(&PortContribution::Thermal {
                zone,
                sensible_gain_w: 25.5,
                radiant_gain_w: 0.0,
                latent_gain_w: -2.5,
                category: ThermalCategory::InternalGain,
            })
            .unwrap();

        approx_eq(slots.thermal[0].sensible_gain_w, 115.5);
        approx_eq(slots.thermal[0].latent_gain_w, 22.5);
    }

    #[test]
    fn mixed_category_contributions_route_to_correct_slots() {
        let zone = ZoneId(1);
        let mut slots = PortSlots {
            thermal: vec![ThermalAccumulator::new(zone)],
            ..Default::default()
        };

        slots
            .accumulate(&PortContribution::Thermal {
                zone,
                sensible_gain_w: 100.0,
                radiant_gain_w: 0.0,
                latent_gain_w: 0.0,
                category: ThermalCategory::HvacHeating,
            })
            .unwrap();
        slots
            .accumulate(&PortContribution::Thermal {
                zone,
                sensible_gain_w: -50.0,
                radiant_gain_w: 0.0,
                latent_gain_w: -10.0,
                category: ThermalCategory::HvacCooling,
            })
            .unwrap();
        slots
            .accumulate(&PortContribution::Thermal {
                zone,
                sensible_gain_w: 25.0,
                radiant_gain_w: 0.0,
                latent_gain_w: 0.0,
                category: ThermalCategory::InternalGain,
            })
            .unwrap();
        slots
            .accumulate(&PortContribution::Thermal {
                zone,
                sensible_gain_w: 40.0,
                radiant_gain_w: 0.0,
                latent_gain_w: 0.0,
                category: ThermalCategory::JacketLoss,
            })
            .unwrap();
        slots
            .accumulate(&PortContribution::Thermal {
                zone,
                sensible_gain_w: 15.0,
                radiant_gain_w: 0.0,
                latent_gain_w: 0.0,
                category: ThermalCategory::DuctLoss,
            })
            .unwrap();

        // Aggregate total is sum of all contributions.
        approx_eq(slots.thermal[0].sensible_gain_w, 130.0); // 100 - 50 + 25 + 40 + 15
        approx_eq(slots.thermal[0].latent_gain_w, -10.0);

        // Per-category subtotals.
        approx_eq(
            slots.thermal[0].sensible_for_category(ThermalCategory::HvacHeating),
            100.0,
        );
        approx_eq(
            slots.thermal[0].sensible_for_category(ThermalCategory::HvacCooling),
            -50.0,
        );
        approx_eq(
            slots.thermal[0].sensible_for_category(ThermalCategory::InternalGain),
            25.0,
        );
        approx_eq(
            slots.thermal[0].sensible_for_category(ThermalCategory::JacketLoss),
            40.0,
        );
        approx_eq(
            slots.thermal[0].sensible_for_category(ThermalCategory::DuctLoss),
            15.0,
        );
    }

    #[test]
    fn zero_resets_all_accumulators() {
        let mut slots = PortSlots {
            thermal: vec![ThermalAccumulator {
                zone: ZoneId(1),
                sensible_gain_w: 10.0,
                radiant_gain_w: 3.0,
                latent_gain_w: 5.0,
                sensible_by_category: [1.0, 2.0, 3.0, 4.0, 0.0, 0.0],
                radiant_by_category: [0.0, 0.0, 3.0, 0.0, 0.0, 0.0],
                latent_by_category: [0.0, 0.0, 5.0, 0.0, 0.0, 0.0],
            }],
            electrical: {
                ElectricalAccumulator {
                    reactive_power_kvar: 1.0,
                    load_power_w: 4.0,
                    ..Default::default()
                }
            },
            fuel: {
                let mut f = FuelAccumulator::default();
                f.add(FuelType::Electric, 1.0).unwrap();
                f.add(FuelType::Gas, 2.0).unwrap();
                f
            },
            fluid: vec![FluidAccumulator {
                loop_id: LoopId(9),
                fluid_type: FluidType::Water,
                node_id: FluidNodeId(0),
                total_flow_kg_s: 1.2,
                mean_supply_temp_c: 45.0,
                mean_return_temp_c: 40.0,
                total_thermal_power_w: 0.0,
                direction: None,
            }],
            custom: vec![CustomAccumulator {
                domain_id: DomainId(12),
                payload: [1.0; 16],
            }],
            humidity: vec![HumidityAccumulator {
                zone: ZoneId(1),
                moisture_mass_flow_kg_s: 0.001,
            }],
            #[cfg(any(debug_assertions, feature = "check_invariants"))]
            write_log: std::collections::HashSet::new(),
        };

        slots.zero();

        approx_eq(slots.thermal[0].sensible_gain_w, 0.0);
        approx_eq(slots.thermal[0].radiant_gain_w, 0.0);
        approx_eq(slots.thermal[0].latent_gain_w, 0.0);
        assert_eq!(
            slots.thermal[0].sensible_by_category, [0.0; THERMAL_CATEGORY_COUNT],
            "zero() must clear per-category array"
        );
        assert_eq!(
            slots.thermal[0].radiant_by_category, [0.0; THERMAL_CATEGORY_COUNT],
            "zero() must clear radiant per-category array"
        );
        approx_eq(slots.electrical.reactive_power_kvar, 0.0);
        approx_eq(slots.electrical.load_power_w, 0.0);
        approx_eq(slots.electrical.generation_power_w, 0.0);
        approx_eq(slots.fuel.get(FuelType::Electric), 0.0);
        approx_eq(slots.fuel.get(FuelType::Gas), 0.0);
        approx_eq(slots.fluid[0].total_flow_kg_s, 0.0);
        approx_eq(slots.fluid[0].mean_supply_temp_c, 0.0);
        approx_eq(slots.fluid[0].mean_return_temp_c, 0.0);
        assert_eq!(slots.custom[0].payload, [0.0; 16]);
        approx_eq(slots.humidity[0].moisture_mass_flow_kg_s, 0.0);
    }

    #[test]
    fn fluid_accumulator_zero_resets_state() {
        let mut fluid = FluidAccumulator::new(LoopId(2), FluidType::Glycol);
        fluid
            .add(0.4, 50.0, 45.0, None, HeatTransferDirection::Source)
            .unwrap();
        fluid
            .add(0.6, 46.0, 41.0, None, HeatTransferDirection::Source)
            .unwrap();

        approx_eq(fluid.total_flow_kg_s, 1.0);
        approx_eq(fluid.mean_supply_temp_c, 47.6);
        approx_eq(fluid.mean_return_temp_c, 42.6);

        fluid.zero();
        approx_eq(fluid.total_flow_kg_s, 0.0);
        approx_eq(fluid.mean_supply_temp_c, 0.0);
        approx_eq(fluid.mean_return_temp_c, 0.0);
    }

    #[test]
    fn custom_accumulator_zero_resets_payload() {
        let mut custom = CustomAccumulator::new(DomainId(5));
        custom.add([1.0; 16]);
        custom.add([2.0; 16]);
        assert_eq!(custom.payload, [3.0; 16]);

        custom.zero();
        assert_eq!(custom.payload, [0.0; 16]);
    }

    #[test]
    fn fluid_and_custom_contributions_accumulate() {
        let mut slots = PortSlots {
            fluid: vec![FluidAccumulator::new(LoopId(7), FluidType::Water)],
            custom: vec![CustomAccumulator::new(DomainId(3))],
            ..Default::default()
        };

        slots
            .accumulate(&PortContribution::Fluid {
                loop_id: LoopId(7),
                flow_rate_kg_s: 1.0,
                supply_temp_c: 40.0,
                return_temp_c: 35.0,
                fluid_type: FluidType::Water,
                thermal_power_w: None,
                node_id: FluidNodeId(0),
                direction: HeatTransferDirection::Source,
            })
            .unwrap();
        slots
            .accumulate(&PortContribution::Fluid {
                loop_id: LoopId(7),
                flow_rate_kg_s: 1.0,
                supply_temp_c: 50.0,
                return_temp_c: 45.0,
                fluid_type: FluidType::Water,
                thermal_power_w: None,
                node_id: FluidNodeId(0),
                direction: HeatTransferDirection::Source,
            })
            .unwrap();
        slots
            .accumulate(&PortContribution::Custom {
                domain_id: DomainId(3),
                payload: [0.5; 16],
            })
            .unwrap();
        slots
            .accumulate(&PortContribution::Custom {
                domain_id: DomainId(3),
                payload: [1.5; 16],
            })
            .unwrap();

        assert_eq!(slots.fluid.len(), 1);
        assert_eq!(slots.custom.len(), 1);
        approx_eq(slots.fluid[0].total_flow_kg_s, 2.0);
        approx_eq(slots.fluid[0].mean_supply_temp_c, 45.0);
        approx_eq(slots.fluid[0].mean_return_temp_c, 40.0);
        assert_eq!(slots.custom[0].payload, [2.0; 16]);
    }

    #[test]
    fn fluid_accumulator_zero_flow_on_fresh() {
        let mut fluid = FluidAccumulator::new(LoopId(1), FluidType::Water);
        fluid
            .add(0.0, 50.0, 40.0, None, HeatTransferDirection::Source)
            .unwrap();
        approx_eq(fluid.total_flow_kg_s, 0.0);
        approx_eq(fluid.mean_supply_temp_c, 0.0);
        approx_eq(fluid.mean_return_temp_c, 0.0);
    }

    #[test]
    fn fuel_accumulator_tracks_all_fuel_types() {
        let mut fuel = FuelAccumulator::default();
        fuel.add(FuelType::Electric, 100.0).unwrap();
        fuel.add(FuelType::Gas, 200.0).unwrap();
        fuel.add(FuelType::Propane, 300.0).unwrap();
        fuel.add(FuelType::Oil, 400.0).unwrap();
        fuel.add(FuelType::Wood, 500.0).unwrap();
        fuel.add(FuelType::Coal, 600.0).unwrap();
        fuel.add(FuelType::WoodPellet, 700.0).unwrap();
        approx_eq(fuel.get(FuelType::Electric), 100.0);
        approx_eq(fuel.get(FuelType::Gas), 200.0);
        approx_eq(fuel.get(FuelType::Propane), 300.0);
        approx_eq(fuel.get(FuelType::Oil), 400.0);
        approx_eq(fuel.get(FuelType::Wood), 500.0);
        approx_eq(fuel.get(FuelType::Coal), 600.0);
        approx_eq(fuel.get(FuelType::WoodPellet), 700.0);
    }

    #[test]
    fn fuel_accumulator_rejects_none() {
        let mut fuel = FuelAccumulator::default();
        let result = fuel.add(FuelType::None, 500.0);
        assert!(result.is_err());
    }

    #[test]
    fn accumulate_to_undeclared_zone_returns_error() {
        let mut slots = PortSlots::default();
        let result = slots.accumulate(&PortContribution::Thermal {
            zone: ZoneId(99),
            sensible_gain_w: 50.0,
            radiant_gain_w: 0.0,
            latent_gain_w: 10.0,
            category: ThermalCategory::InternalGain,
        });
        assert!(result.is_err());
    }

    #[test]
    fn accumulate_to_undeclared_fluid_loop_returns_error() {
        let mut slots = PortSlots::default();
        let result = slots.accumulate(&PortContribution::Fluid {
            loop_id: LoopId(1),
            flow_rate_kg_s: 1.0,
            supply_temp_c: 40.0,
            return_temp_c: 35.0,
            fluid_type: FluidType::Water,
            thermal_power_w: None,
            node_id: FluidNodeId(0),
            direction: HeatTransferDirection::Source,
        });
        assert!(result.is_err());
    }

    #[test]
    fn accumulate_to_undeclared_custom_domain_returns_error() {
        let mut slots = PortSlots::default();
        let result = slots.accumulate(&PortContribution::Custom {
            domain_id: DomainId(3),
            payload: [0.5; 16],
        });
        assert!(result.is_err());
    }

    #[test]
    fn fuel_accumulator_zero_clears_all() {
        let mut fuel = FuelAccumulator::default();
        fuel.add(FuelType::Electric, 100.0).unwrap();
        fuel.add(FuelType::Gas, 200.0).unwrap();
        fuel.zero();
        approx_eq(fuel.get(FuelType::Electric), 0.0);
        approx_eq(fuel.get(FuelType::Gas), 0.0);
        approx_eq(fuel.get(FuelType::Propane), 0.0);
        approx_eq(fuel.get(FuelType::Oil), 0.0);
    }

    #[test]
    fn fluid_accumulate_separates_different_loops() {
        let mut slots = PortSlots {
            fluid: vec![
                FluidAccumulator::new(LoopId(1), FluidType::Water),
                FluidAccumulator::new(LoopId(2), FluidType::Glycol),
            ],
            ..Default::default()
        };
        slots
            .accumulate(&PortContribution::Fluid {
                loop_id: LoopId(1),
                flow_rate_kg_s: 1.0,
                supply_temp_c: 40.0,
                return_temp_c: 35.0,
                fluid_type: FluidType::Water,
                thermal_power_w: None,
                node_id: FluidNodeId(0),
                direction: HeatTransferDirection::Source,
            })
            .unwrap();
        slots
            .accumulate(&PortContribution::Fluid {
                loop_id: LoopId(2),
                flow_rate_kg_s: 2.0,
                supply_temp_c: 50.0,
                return_temp_c: 45.0,
                fluid_type: FluidType::Glycol,
                thermal_power_w: None,
                node_id: FluidNodeId(0),
                direction: HeatTransferDirection::Source,
            })
            .unwrap();
        assert_eq!(slots.fluid.len(), 2);
        approx_eq(slots.fluid[0].total_flow_kg_s, 1.0);
        approx_eq(slots.fluid[1].total_flow_kg_s, 2.0);
    }

    #[test]
    fn from_declarations_builds_correct_slots() {
        let decls = &[
            PortDeclaration::thermal(ZoneId(1)),
            PortDeclaration::thermal(ZoneId(2)),
            // Duplicate zone should be deduplicated
            PortDeclaration::thermal(ZoneId(1)),
            PortDeclaration::electrical(),
            PortDeclaration::custom(DomainId(5)),
            // Fluid port with loop_id and fluid_type should create accumulator
            PortDeclaration::fluid(LoopId(10), FluidType::Water),
            // Duplicate (loop_id, fluid_type) should be deduplicated
            PortDeclaration::fluid(LoopId(10), FluidType::Water),
        ];

        let slots = PortSlots::from_declarations(decls);
        assert_eq!(slots.thermal.len(), 2);
        assert_eq!(slots.thermal[0].zone, ZoneId(1));
        assert_eq!(slots.thermal[1].zone, ZoneId(2));
        assert_eq!(slots.custom.len(), 1);
        assert_eq!(slots.custom[0].domain_id, DomainId(5));
        // Fluid accumulator should be created from Fluid PortDeclaration
        assert_eq!(slots.fluid.len(), 1);
        assert_eq!(slots.fluid[0].loop_id, LoopId(10));
        assert_eq!(slots.fluid[0].fluid_type, FluidType::Water);

        // Verify accumulation works on the built slots
        let mut slots = slots;
        slots
            .accumulate(&PortContribution::Thermal {
                zone: ZoneId(1),
                sensible_gain_w: 100.0,
                radiant_gain_w: 0.0,
                latent_gain_w: 10.0,
                category: ThermalCategory::HvacHeating,
            })
            .unwrap();
        approx_eq(slots.thermal[0].sensible_gain_w, 100.0);

        // Fluid accumulation should work
        slots
            .accumulate(&PortContribution::Fluid {
                loop_id: LoopId(10),
                flow_rate_kg_s: 0.5,
                supply_temp_c: 50.0,
                return_temp_c: 30.0,
                fluid_type: FluidType::Water,
                thermal_power_w: None,
                node_id: FluidNodeId(0),
                direction: HeatTransferDirection::Source,
            })
            .unwrap();
        approx_eq(slots.fluid[0].total_flow_kg_s, 0.5);

        // Undeclared zone should fail
        let err = slots.accumulate(&PortContribution::Thermal {
            zone: ZoneId(99),
            sensible_gain_w: 50.0,
            radiant_gain_w: 0.0,
            latent_gain_w: 5.0,
            category: ThermalCategory::InternalGain,
        });
        assert!(err.is_err());
    }

    #[test]
    fn fluid_accumulator_rejects_negative_flow() {
        let mut fluid = FluidAccumulator::new(LoopId(1), FluidType::Water);
        let result = fluid.add(-1.0, 50.0, 40.0, None, HeatTransferDirection::Source);
        assert!(result.is_err());
    }

    #[test]
    fn custom_port_factory_and_from_declarations() {
        let decls = &[
            PortDeclaration::custom(DomainId(7)),
            PortDeclaration::custom(DomainId(7)), // duplicate should be deduplicated
            PortDeclaration::custom(DomainId(8)),
        ];
        let slots = PortSlots::from_declarations(decls);
        assert_eq!(slots.custom.len(), 2);
        assert_eq!(slots.custom[0].domain_id, DomainId(7));
        assert_eq!(slots.custom[1].domain_id, DomainId(8));

        let mut slots = slots;
        slots
            .accumulate(&PortContribution::Custom {
                domain_id: DomainId(7),
                payload: [1.0; 16],
            })
            .unwrap();
        assert_eq!(slots.custom[0].payload, [1.0; 16]);
    }

    #[test]
    fn fuel_port_in_from_declarations() {
        let decls = &[
            PortDeclaration::fuel(),
            PortDeclaration::fuel(), // singletons -- no extra accumulators
        ];
        let mut slots = PortSlots::from_declarations(decls);
        slots
            .accumulate(&PortContribution::Fuel {
                fuel_type: FuelType::Gas,
                consumption_w: 500.0,
            })
            .unwrap();
        approx_eq(slots.fuel.get(FuelType::Gas), 500.0);
    }

    #[test]
    fn electrical_accumulator_tracks_load_and_generation_split() {
        let mut slots = PortSlots::default();
        slots
            .accumulate(&PortContribution::Electrical {
                active_power_w: 3000.0,
                reactive_power_kvar: 0.4,
            })
            .unwrap();
        slots
            .accumulate(&PortContribution::Electrical {
                active_power_w: -5000.0,
                reactive_power_kvar: -0.1,
            })
            .unwrap();
        approx_eq(slots.electrical.net_active_w(), -2000.0);
        approx_eq(slots.electrical.reactive_power_kvar, 0.3);
        approx_eq(slots.electrical.load_power_w, 3000.0);
        approx_eq(slots.electrical.generation_power_w, -5000.0);
    }

    #[test]
    fn positive_active_power_routes_to_load() {
        let mut slots = PortSlots::default();
        slots
            .accumulate(&PortContribution::Electrical {
                active_power_w: 3000.0,
                reactive_power_kvar: 0.0,
            })
            .unwrap();
        approx_eq(slots.electrical.load_power_w, 3000.0);
        approx_eq(slots.electrical.generation_power_w, 0.0);
    }

    #[test]
    fn negative_active_power_routes_to_generation() {
        let mut slots = PortSlots::default();
        slots
            .accumulate(&PortContribution::Electrical {
                active_power_w: -5000.0,
                reactive_power_kvar: 0.0,
            })
            .unwrap();
        approx_eq(slots.electrical.load_power_w, 0.0);
        approx_eq(slots.electrical.generation_power_w, -5000.0);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "non-finite active_power_w")]
    fn non_finite_active_power_triggers_debug_assert() {
        let mut slots = PortSlots::default();
        let _ = slots.accumulate(&PortContribution::Electrical {
            active_power_w: f64::NAN,
            reactive_power_kvar: 0.0,
        });
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "non-finite reactive_power_kvar")]
    fn non_finite_reactive_power_triggers_debug_assert() {
        let mut slots = PortSlots::default();
        let _ = slots.accumulate(&PortContribution::Electrical {
            active_power_w: 0.0,
            reactive_power_kvar: f64::INFINITY,
        });
    }

    #[test]
    fn zero_active_power_routes_to_load() {
        let mut slots = PortSlots::default();
        slots
            .accumulate(&PortContribution::Electrical {
                active_power_w: 0.0,
                reactive_power_kvar: 0.0,
            })
            .unwrap();
        approx_eq(slots.electrical.load_power_w, 0.0);
        approx_eq(slots.electrical.generation_power_w, 0.0);
    }

    #[test]
    fn radiant_gain_w_tracked_in_accumulator() {
        let zone = ZoneId(1);
        let mut slots = PortSlots {
            thermal: vec![ThermalAccumulator::new(zone)],
            ..Default::default()
        };

        slots
            .accumulate(&PortContribution::Thermal {
                zone,
                sensible_gain_w: 140.0,
                radiant_gain_w: 60.0,
                latent_gain_w: 0.0,
                category: ThermalCategory::InternalGain,
            })
            .unwrap();
        slots
            .accumulate(&PortContribution::Thermal {
                zone,
                sensible_gain_w: 100.0,
                radiant_gain_w: 0.0,
                latent_gain_w: 0.0,
                category: ThermalCategory::HvacHeating,
            })
            .unwrap();

        approx_eq(slots.thermal[0].sensible_gain_w, 240.0);
        approx_eq(slots.thermal[0].radiant_gain_w, 60.0);
        approx_eq(
            slots.thermal[0].radiant_for_category(ThermalCategory::InternalGain),
            60.0,
        );
        approx_eq(
            slots.thermal[0].radiant_for_category(ThermalCategory::HvacHeating),
            0.0,
        );
        approx_eq(
            slots.thermal[0].sensible_for_category(ThermalCategory::InternalGain),
            140.0,
        );
        approx_eq(
            slots.thermal[0].sensible_for_category(ThermalCategory::HvacHeating),
            100.0,
        );

        slots.zero();
        approx_eq(slots.thermal[0].radiant_gain_w, 0.0);
        approx_eq(
            slots.thermal[0].radiant_for_category(ThermalCategory::InternalGain),
            0.0,
        );
    }

    #[test]
    fn humidity_port_accumulates_mass_flow_per_zone() {
        let zone = ZoneId(1);
        let mut slots = PortSlots {
            thermal: vec![ThermalAccumulator::new(zone)],
            humidity: vec![HumidityAccumulator::new(zone)],
            ..Default::default()
        };

        slots
            .accumulate(&PortContribution::Humidity {
                zone,
                moisture_mass_flow_kg_s: -0.000_5,
            })
            .unwrap();
        slots
            .accumulate(&PortContribution::Humidity {
                zone,
                moisture_mass_flow_kg_s: -0.000_3,
            })
            .unwrap();

        approx_eq(slots.humidity[0].moisture_mass_flow_kg_s, -0.000_8);
    }

    #[test]
    fn humidity_port_undeclared_zone_returns_error() {
        let mut slots = PortSlots::default();
        let result = slots.accumulate(&PortContribution::Humidity {
            zone: ZoneId(99),
            moisture_mass_flow_kg_s: 0.001,
        });
        assert!(result.is_err());
    }

    #[test]
    fn humidity_declaration_creates_accumulator() {
        let decls = &[
            PortDeclaration::humidity(ZoneId(1)),
            PortDeclaration::humidity(ZoneId(2)),
            PortDeclaration::humidity(ZoneId(1)),
        ];
        let slots = PortSlots::from_declarations(decls);
        assert_eq!(slots.humidity.len(), 2);
        assert_eq!(slots.humidity[0].zone, ZoneId(1));
        assert_eq!(slots.humidity[1].zone, ZoneId(2));
    }

    #[test]
    fn humidity_accumulator_zero_clears_flow() {
        let mut acc = HumidityAccumulator::new(ZoneId(1));
        acc.add(-0.001);
        acc.add(0.0005);
        approx_eq(acc.moisture_mass_flow_kg_s, -0.0005);
        acc.zero();
        approx_eq(acc.moisture_mass_flow_kg_s, 0.0);
    }

    // =======================================================================
    // T-0084: Fluid accumulator thermal_power_w accumulation
    // =======================================================================

    #[test]
    fn fluid_accumulator_sums_thermal_power_w() {
        let mut fluid = FluidAccumulator::new(LoopId(1), FluidType::Water);
        fluid
            .add(0.5, 60.0, 40.0, Some(4186.0), HeatTransferDirection::Source)
            .unwrap();
        fluid
            .add(0.5, 60.0, 40.0, Some(4186.0), HeatTransferDirection::Source)
            .unwrap();
        approx_eq(fluid.total_thermal_power_w, 8372.0);
    }

    #[test]
    fn fluid_accumulator_ignores_none_thermal_power_w() {
        let mut fluid = FluidAccumulator::new(LoopId(1), FluidType::Water);
        fluid
            .add(0.5, 60.0, 40.0, None, HeatTransferDirection::Source)
            .unwrap();
        fluid
            .add(0.5, 60.0, 40.0, Some(5000.0), HeatTransferDirection::Source)
            .unwrap();
        approx_eq(fluid.total_thermal_power_w, 5000.0);
    }

    #[test]
    fn fluid_accumulator_zero_clears_thermal_power_w() {
        let mut fluid = FluidAccumulator::new(LoopId(1), FluidType::Water);
        fluid
            .add(0.5, 60.0, 40.0, Some(4186.0), HeatTransferDirection::Source)
            .unwrap();
        approx_eq(fluid.total_thermal_power_w, 4186.0);
        fluid.zero();
        approx_eq(fluid.total_thermal_power_w, 0.0);
        approx_eq(fluid.total_flow_kg_s, 0.0);
    }

    #[test]
    fn fluid_port_contribution_routes_thermal_power_w_to_accumulator() {
        let mut slots = PortSlots {
            fluid: vec![FluidAccumulator::new(LoopId(7), FluidType::Water)],
            ..Default::default()
        };
        slots
            .accumulate(&PortContribution::Fluid {
                loop_id: LoopId(7),
                flow_rate_kg_s: 0.5,
                supply_temp_c: 60.0,
                return_temp_c: 40.0,
                fluid_type: FluidType::Water,
                thermal_power_w: Some(4186.0),
                node_id: FluidNodeId(0),
                direction: HeatTransferDirection::Source,
            })
            .unwrap();
        approx_eq(slots.fluid[0].total_thermal_power_w, 4186.0);
    }

    #[test]
    fn validate_fluid_type_consistency_passes_for_single_loop_id() {
        let decls = [PortDeclaration::fluid(LoopId(1), FluidType::Water)];
        assert!(validate_fluid_type_consistency(&decls).is_ok());
    }

    #[test]
    fn validate_fluid_type_consistency_rejects_mixed_fluid_types() {
        let decls = [
            PortDeclaration::fluid(LoopId(1), FluidType::Water),
            PortDeclaration::fluid(LoopId(1), FluidType::Glycol),
        ];
        let err = validate_fluid_type_consistency(&decls).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("LoopId(1)"));
        assert!(msg.contains("Water"));
        assert!(msg.contains("Glycol"));
    }

    #[test]
    fn validate_fluid_type_consistency_passes_for_different_loops() {
        let decls = [
            PortDeclaration::fluid(LoopId(1), FluidType::Water),
            PortDeclaration::fluid(LoopId(2), FluidType::Glycol),
        ];
        assert!(validate_fluid_type_consistency(&decls).is_ok());
    }

    #[test]
    fn validate_fluid_type_consistency_ignores_non_fluid_ports() {
        let decls = [
            PortDeclaration::thermal(ZoneId(1)),
            PortDeclaration::fluid(LoopId(1), FluidType::Water),
            PortDeclaration::fluid(LoopId(1), FluidType::Water),
            PortDeclaration::electrical(),
        ];
        assert!(validate_fluid_type_consistency(&decls).is_ok());
    }

    // =======================================================================
    // T-0132: PortContribution unit consistency — all power fields in W
    // =======================================================================

    #[test]
    fn all_power_contributions_use_watts() {
        // Verify that all PortContribution variants with power fields use
        // W-scoped naming and physically plausible W-range values (tens to
        // thousands of watts for residential equipment). This test exists
        // to catch a factor-1000 error if a future contributor reintroduces
        // a kW-scaled field.

        // Electrical with a 1.5 kW load → 1500 W active_power_w.
        let electrical = PortContribution::Electrical {
            active_power_w: 1500.0,
            reactive_power_kvar: 0.0,
        };
        match electrical {
            PortContribution::Electrical { active_power_w, .. } => {
                assert!(
                    active_power_w > 100.0,
                    "active_power_w should be in W range"
                );
                assert!(
                    active_power_w < 100_000.0,
                    "active_power_w exceeds plausible residential W"
                );
            }
            _ => unreachable!(),
        }

        // Thermal — sensible_gain_w already uses W suffix.
        let thermal = PortContribution::Thermal {
            zone: ZoneId(1),
            sensible_gain_w: 500.0,
            radiant_gain_w: 300.0,
            latent_gain_w: 100.0,
            category: ThermalCategory::InternalGain,
        };
        match thermal {
            PortContribution::Thermal {
                sensible_gain_w, ..
            } => {
                assert!(
                    sensible_gain_w < 100_000.0,
                    "sensible_gain_w exceeds plausible W magnitude"
                );
            }
            _ => unreachable!(),
        }

        // Fuel — consumption_w already uses W suffix.
        let fuel = PortContribution::Fuel {
            fuel_type: FuelType::Gas,
            consumption_w: 5000.0,
        };
        match fuel {
            PortContribution::Fuel { consumption_w, .. } => {
                assert!(
                    consumption_w < 1_000_000.0,
                    "consumption_w exceeds plausible W magnitude"
                );
            }
            _ => unreachable!(),
        }

        // Fluid — thermal_power_w already uses W suffix.
        let fluid = PortContribution::Fluid {
            loop_id: LoopId(1),
            flow_rate_kg_s: 0.1,
            supply_temp_c: 60.0,
            return_temp_c: 40.0,
            fluid_type: FluidType::Water,
            thermal_power_w: Some(4186.0),
            node_id: FluidNodeId(0),
            direction: HeatTransferDirection::Source,
        };
        match fluid {
            PortContribution::Fluid {
                thermal_power_w, ..
            } => {
                assert!(
                    thermal_power_w.is_some_and(|w| w > 0.0 && w < 1_000_000.0),
                    "thermal_power_w should be in W range"
                );
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn from_declarations_only_creates_accumulators_for_declared_zones() {
        // Only ZoneId(1) is declared. No accumulator should exist for ZoneId(2).
        let decls = &[PortDeclaration::thermal(ZoneId(1))];
        let slots = PortSlots::from_declarations(decls);
        assert_eq!(slots.thermal.len(), 1);
        assert_eq!(slots.thermal[0].zone, ZoneId(1));
        assert!(!slots.thermal.iter().any(|t| t.zone == ZoneId(2)));
    }

    #[test]
    fn accumulate_to_zone_without_equipment_declarant_returns_error() {
        // Wire-to-slot safety: only declared zones get accumulators.
        // Accumulating to an undeclared zone must fail.
        let decls = &[PortDeclaration::thermal(ZoneId(1))];
        let mut slots = PortSlots::from_declarations(decls);
        let result = slots.accumulate(&PortContribution::Thermal {
            zone: ZoneId(999),
            sensible_gain_w: 500.0,
            radiant_gain_w: 0.0,
            latent_gain_w: 0.0,
            category: ThermalCategory::HvacHeating,
        });
        assert!(result.is_err());
    }
}
