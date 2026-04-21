//! Port contribution types, port slots, and port declarations.
//!
//! Ports are the interface through which equipment communicates thermal,
//! electrical, and fluid contributions to the envelope solver.

use serde::{Deserialize, Serialize};

use crate::{DomainId, FluidType, FuelType, HaresError, LoopId, ZoneId};

pub const CUSTOM_PAYLOAD_LEN: usize = 16;

/// Classification of a thermal contribution's physical origin.
///
/// Used to partition `ThermalAccumulator::sensible_by_category` without
/// allocating. The ordinal of each variant must match its index in that array.
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
        }
    }
}

/// Number of `ThermalCategory` variants -- size of the per-category array.
pub const THERMAL_CATEGORY_COUNT: usize = 5;

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
        active_power_kw: f64,
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
    },
    Custom {
        domain_id: DomainId,
        payload: [f64; CUSTOM_PAYLOAD_LEN],
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
}

/// Port declaration used to pre-size and validate port slot wiring.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortDeclaration {
    pub port_type: PortType,
    pub zone: Option<ZoneId>,
    pub loop_id: Option<LoopId>,
    pub domain_id: Option<DomainId>,
    pub fluid_type: Option<FluidType>,
}

impl PortDeclaration {
    pub fn electrical() -> Self {
        Self {
            port_type: PortType::Electrical,
            zone: None,
            loop_id: None,
            domain_id: None,
            fluid_type: None,
        }
    }

    pub fn thermal(zone: ZoneId) -> Self {
        Self {
            port_type: PortType::Thermal,
            zone: Some(zone),
            loop_id: None,
            domain_id: None,
            fluid_type: None,
        }
    }

    pub fn fuel() -> Self {
        Self {
            port_type: PortType::Fuel,
            zone: None,
            loop_id: None,
            domain_id: None,
            fluid_type: None,
        }
    }

    pub fn fluid(loop_id: LoopId, fluid_type: FluidType) -> Self {
        Self {
            port_type: PortType::Fluid,
            zone: None,
            loop_id: Some(loop_id),
            domain_id: None,
            fluid_type: Some(fluid_type),
        }
    }

    pub fn custom(domain_id: DomainId) -> Self {
        Self {
            port_type: PortType::Custom,
            zone: None,
            loop_id: None,
            domain_id: Some(domain_id),
            fluid_type: None,
        }
    }
}

/// Thermal contribution totals for one zone.
///
/// `sensible_gain_w` is the **convective** sensible total (goes directly to zone
/// air); it is *not* the total sensible gain. Total sensible = sensible_gain_w +
/// radiant_gain_w. `latent_gain_w` is the zone total (sum across all categories).
/// `sensible_by_category` holds per-category **convective** sensible subtotals
/// indexed by `ThermalCategory::index()`. Invariant:
/// `sum(sensible_by_category) == sensible_gain_w` (convective-only total).
/// Use a fixed-size array to avoid HashMap allocation in the hot timestep loop.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ThermalAccumulator {
    pub zone: ZoneId,
    pub sensible_gain_w: f64,
    pub radiant_gain_w: f64,
    pub latent_gain_w: f64,
    pub sensible_by_category: [f64; THERMAL_CATEGORY_COUNT],
    pub radiant_by_category: [f64; THERMAL_CATEGORY_COUNT],
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
    }

    pub fn zero(&mut self) {
        self.sensible_gain_w = 0.0;
        self.radiant_gain_w = 0.0;
        self.latent_gain_w = 0.0;
        self.sensible_by_category = [0.0; THERMAL_CATEGORY_COUNT];
        self.radiant_by_category = [0.0; THERMAL_CATEGORY_COUNT];
    }

    /// Sensible gain total for a specific category.
    pub fn sensible_for_category(&self, cat: ThermalCategory) -> f64 {
        self.sensible_by_category[cat.index()]
    }

    /// Radiant gain total for a specific category.
    pub fn radiant_for_category(&self, cat: ThermalCategory) -> f64 {
        self.radiant_by_category[cat.index()]
    }
}

/// Electrical contribution totals on the shared v1 bus.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct ElectricalAccumulator {
    pub reactive_power_kvar: f64,
    pub load_power_kw: f64,
    pub generation_power_kw: f64,
}

impl ElectricalAccumulator {
    /// Net active power: load (positive) + generation (negative).
    pub fn net_active_kw(&self) -> f64 {
        self.load_power_kw + self.generation_power_kw
    }

    pub fn zero(&mut self) {
        *self = Self::default();
    }
}

/// Number of fuel types excluding `FuelType::None`.
const FUEL_TYPE_COUNT: usize = 4;

fn fuel_index(fuel_type: FuelType) -> Option<usize> {
    match fuel_type {
        FuelType::Electric => Some(0),
        FuelType::Gas => Some(1),
        FuelType::Propane => Some(2),
        FuelType::Oil => Some(3),
        FuelType::None => None,
    }
}

/// Fuel consumption totals grouped by fuel type.
///
/// Indexed by fuel type ordinal: [Electric=0, Gas=1, Propane=2, Oil=3].
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

/// Fluid contribution totals for one loop and fluid type.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct FluidAccumulator {
    pub loop_id: LoopId,
    pub fluid_type: FluidType,
    pub total_flow_kg_s: f64,
    pub mean_supply_temp_c: f64,
    pub mean_return_temp_c: f64,
}

impl FluidAccumulator {
    pub fn new(loop_id: LoopId, fluid_type: FluidType) -> Self {
        Self {
            loop_id,
            fluid_type,
            total_flow_kg_s: 0.0,
            mean_supply_temp_c: 0.0,
            mean_return_temp_c: 0.0,
        }
    }

    pub fn add(
        &mut self,
        flow_rate_kg_s: f64,
        supply_temp_c: f64,
        return_temp_c: f64,
    ) -> Result<(), HaresError> {
        if flow_rate_kg_s < 0.0 {
            return Err(HaresError::Equipment("negative flow rate".to_string()));
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
        Ok(())
    }

    pub fn zero(&mut self) {
        self.total_flow_kg_s = 0.0;
        self.mean_supply_temp_c = 0.0;
        self.mean_return_temp_c = 0.0;
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

/// Preallocated per-timestep accumulation slots.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct PortSlots {
    pub thermal: Vec<ThermalAccumulator>,
    pub electrical: ElectricalAccumulator,
    pub fuel: FuelAccumulator,
    pub fluid: Vec<FluidAccumulator>,
    pub custom: Vec<CustomAccumulator>,
}

impl PortSlots {
    /// Build pre-sized PortSlots from port declarations.
    /// This is the canonical construction path -- ensures accumulators
    /// match declared ports and rejects undeclared contributions at runtime.
    pub fn from_declarations(decls: &[PortDeclaration]) -> Self {
        let mut thermal = Vec::new();
        let mut fluid = Vec::new();
        let mut custom = Vec::new();

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
                        if !fluid.iter().any(|f: &FluidAccumulator| {
                            f.loop_id == loop_id && f.fluid_type == fluid_type
                        }) {
                            fluid.push(FluidAccumulator::new(loop_id, fluid_type));
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
                // Electrical and Fuel are singletons, handled by defaults.
                _ => {}
            }
        }

        Self {
            thermal,
            electrical: ElectricalAccumulator::default(),
            fuel: FuelAccumulator::default(),
            fluid,
            custom,
        }
    }

    pub fn zero(&mut self) {
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
                } else {
                    return Err(HaresError::Equipment(format!(
                        "undeclared thermal zone: {zone:?}"
                    )));
                }
            }
            PortContribution::Electrical {
                active_power_kw,
                reactive_power_kvar,
            } => {
                self.electrical.reactive_power_kvar += reactive_power_kvar;
                if *active_power_kw >= 0.0 {
                    self.electrical.load_power_kw += active_power_kw;
                } else {
                    self.electrical.generation_power_kw += active_power_kw;
                }
            }
            PortContribution::Fuel {
                fuel_type,
                consumption_w,
            } => {
                self.fuel.add(*fuel_type, *consumption_w)?;
            }
            PortContribution::Fluid {
                loop_id,
                flow_rate_kg_s,
                supply_temp_c,
                return_temp_c,
                fluid_type,
            } => {
                if let Some(total) = self
                    .fluid
                    .iter_mut()
                    .find(|entry| entry.loop_id == *loop_id && entry.fluid_type == *fluid_type)
                {
                    total.add(*flow_rate_kg_s, *supply_temp_c, *return_temp_c)?;
                } else {
                    return Err(HaresError::Equipment(format!(
                        "undeclared fluid loop: {loop_id:?} with fluid type {fluid_type:?}"
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
                } else {
                    return Err(HaresError::Equipment(format!(
                        "undeclared custom domain: {domain_id:?}"
                    )));
                }
            }
        }
        Ok(())
    }
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
                sensible_by_category: [1.0, 2.0, 3.0, 4.0, 0.0],
                radiant_by_category: [0.0, 0.0, 3.0, 0.0, 0.0],
            }],
            electrical: ElectricalAccumulator {
                reactive_power_kvar: 1.0,
                load_power_kw: 4.0,
                generation_power_kw: 0.0,
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
                total_flow_kg_s: 1.2,
                mean_supply_temp_c: 45.0,
                mean_return_temp_c: 40.0,
            }],
            custom: vec![CustomAccumulator {
                domain_id: DomainId(12),
                payload: [1.0; 16],
            }],
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
        approx_eq(slots.electrical.load_power_kw, 0.0);
        approx_eq(slots.electrical.generation_power_kw, 0.0);
        approx_eq(slots.fuel.get(FuelType::Electric), 0.0);
        approx_eq(slots.fuel.get(FuelType::Gas), 0.0);
        approx_eq(slots.fluid[0].total_flow_kg_s, 0.0);
        approx_eq(slots.fluid[0].mean_supply_temp_c, 0.0);
        approx_eq(slots.fluid[0].mean_return_temp_c, 0.0);
        assert_eq!(slots.custom[0].payload, [0.0; 16]);
    }

    #[test]
    fn fluid_accumulator_zero_resets_state() {
        let mut fluid = FluidAccumulator::new(LoopId(2), FluidType::Glycol);
        fluid.add(0.4, 50.0, 45.0).unwrap();
        fluid.add(0.6, 46.0, 41.0).unwrap();

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
            })
            .unwrap();
        slots
            .accumulate(&PortContribution::Fluid {
                loop_id: LoopId(7),
                flow_rate_kg_s: 1.0,
                supply_temp_c: 50.0,
                return_temp_c: 45.0,
                fluid_type: FluidType::Water,
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
        fluid.add(0.0, 50.0, 40.0).unwrap();
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
        approx_eq(fuel.get(FuelType::Electric), 100.0);
        approx_eq(fuel.get(FuelType::Gas), 200.0);
        approx_eq(fuel.get(FuelType::Propane), 300.0);
        approx_eq(fuel.get(FuelType::Oil), 400.0);
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
            })
            .unwrap();
        slots
            .accumulate(&PortContribution::Fluid {
                loop_id: LoopId(2),
                flow_rate_kg_s: 2.0,
                supply_temp_c: 50.0,
                return_temp_c: 45.0,
                fluid_type: FluidType::Glycol,
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
        let result = fluid.add(-1.0, 50.0, 40.0);
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
                active_power_kw: 3.0,
                reactive_power_kvar: 0.4,
            })
            .unwrap();
        slots
            .accumulate(&PortContribution::Electrical {
                active_power_kw: -5.0,
                reactive_power_kvar: -0.1,
            })
            .unwrap();
        approx_eq(slots.electrical.net_active_kw(), -2.0);
        approx_eq(slots.electrical.reactive_power_kvar, 0.3);
        approx_eq(slots.electrical.load_power_kw, 3.0);
        approx_eq(slots.electrical.generation_power_kw, -5.0);
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
}
