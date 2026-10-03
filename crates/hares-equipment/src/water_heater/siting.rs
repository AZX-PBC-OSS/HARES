//! Where a water heater sits: a modeled thermal zone or an HPXML location
//! with no modeled zone, and the air it draws there each step.

use hares_types::{
    AmbientAirTemps, AmbientLocation, EnvironmentState, FluidType, HaresError, LoopId,
    PortDeclaration, ZoneId,
};

use crate::EquipmentConfig;
use crate::hvac::helpers::zone_id_from_config;

/// A resolved water heater placement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Placement {
    /// A modeled thermal zone: the tank exchanges heat with that zone.
    Zone(ZoneId),
    /// A location with no modeled zone: the tank's losses leave the model
    /// through that location's ambient air.
    Ambient(AmbientLocation),
}

/// A water heater's placement, unresolved until `init`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Siting(Option<Placement>);

impl Siting {
    /// The placement a constructor can already see: an explicit zone_id.
    pub(crate) fn from_constructor_config(config: &EquipmentConfig) -> Self {
        Self(zone_id_from_config(config).map(Placement::Zone))
    }

    /// Resolve the placement at `init`: the config's zone_id, else the zone
    /// this heater already carries, else the HPXML `location` when it names
    /// a place with no modeled zone. A heater that names no zone, or a
    /// location that is neither a modeled zone (which arrives as a zone_id)
    /// nor a known ambient placement, is unresolved wiring.
    pub(crate) fn resolve(
        &mut self,
        config: &EquipmentConfig,
        location: Option<&str>,
    ) -> crate::Result<()> {
        let name = config.name.as_str();
        let zone = zone_id_from_config(config).or(self.zone());
        let placement = match (zone, location) {
            (Some(zone), _) => Placement::Zone(zone),
            (None, Some(location)) => AmbientLocation::from_location(location)
                .map(Placement::Ambient)
                .ok_or_else(|| {
                    HaresError::Equipment(format!(
                        "{name}: zone_id not resolved and location '{location}' names no \
                         modeled zone or known ambient location"
                    ))
                })?,
            (None, None) => {
                return Err(HaresError::Equipment(format!(
                    "{name}: no zone_id and no location; a water heater must name the zone \
                     or the HPXML location it sits in"
                )));
            }
        };
        self.0 = Some(placement);
        Ok(())
    }

    pub(crate) fn zone(&self) -> Option<ZoneId> {
        match self.0 {
            Some(Placement::Zone(zone)) => Some(zone),
            Some(Placement::Ambient(_)) | None => None,
        }
    }

    pub(crate) fn ambient_location(&self) -> Option<AmbientLocation> {
        match self.0 {
            Some(Placement::Ambient(location)) => Some(location),
            Some(Placement::Zone(_)) | None => None,
        }
    }

    fn placement(&self, name: &str) -> crate::Result<Placement> {
        self.0.ok_or_else(|| {
            HaresError::Equipment(format!(
                "{name}: placement not resolved; init must run before stepping"
            ))
        })
    }

    /// Dry-bulb temperature of the air around the tank this step.
    pub(crate) fn dry_bulb_c(&self, env: &EnvironmentState, name: &str) -> crate::Result<f64> {
        match self.placement(name)? {
            Placement::Zone(zone) => Ok(zone_state(env, zone, name)?.temperature_c),
            Placement::Ambient(location) => Ok(ambient_air(env, location, name)?.dry_bulb_c),
        }
    }

    /// Dry-bulb and wet-bulb temperature of the air a heat pump evaporator
    /// draws this step: the zone's own state, or the precomputed ambient air.
    pub(crate) fn inlet_air(
        &self,
        env: &EnvironmentState,
        name: &str,
    ) -> crate::Result<AmbientAirTemps> {
        match self.placement(name)? {
            Placement::Zone(zone) => {
                let state = zone_state(env, zone, name)?;
                Ok(AmbientAirTemps {
                    dry_bulb_c: state.temperature_c,
                    wet_bulb_c: hares_physics::psychrometrics::zone_wet_bulb_c(
                        state,
                        env.weather.pressure_pa(),
                    ),
                })
            }
            Placement::Ambient(location) => ambient_air(env, location, name),
        }
    }

    /// The thermal port a heater declares: one on its zone, none for a
    /// placement with no modeled zone (or before init).
    pub(crate) fn thermal_port(&self) -> Option<PortDeclaration> {
        self.zone().map(PortDeclaration::thermal)
    }
}

fn zone_state<'a>(
    env: &'a EnvironmentState,
    zone: ZoneId,
    name: &str,
) -> crate::Result<&'a hares_types::ZoneState> {
    env.zones.iter().find(|z| z.id == zone).ok_or_else(|| {
        HaresError::Equipment(format!(
            "{name}: zone {} is not in this step's environment",
            zone.0
        ))
    })
}

fn ambient_air(
    env: &EnvironmentState,
    location: AmbientLocation,
    name: &str,
) -> crate::Result<AmbientAirTemps> {
    env.ambient_air(location).ok_or_else(|| {
        HaresError::Equipment(format!(
            "{name}: ambient air for {location:?} was not computed this step"
        ))
    })
}

/// Scheduled-space air for conditioned 22.0 °C and outdoor 2.0 °C per the
/// OS-HPXML table (heated max(12.0, 20.0) = 20.0, buffer max(12.0, 10.0) =
/// 12.0, non-freezing max(2.0, 4.44) = 4.44, housing unit 22.0), with
/// distinct wet-bulbs so a read proves which slot the heater uses.
#[cfg(test)]
pub(crate) fn test_ambient_air() -> hares_types::AmbientOtherSpaceTemps {
    let air = |dry_bulb_c, wet_bulb_c| {
        Some(AmbientAirTemps {
            dry_bulb_c,
            wet_bulb_c,
        })
    };
    hares_types::AmbientOtherSpaceTemps {
        other_heated_space: air(20.0, 13.0),
        other_multifamily_buffer_space: air(12.0, 8.0),
        other_non_freezing_space: air(4.44, 2.0),
        other_housing_unit: air(22.0, 14.0),
    }
}

/// The ports of a storage water heater: its leading ports, its thermal port
/// when it sits in a modeled zone, its supply loop and the shared DHW demand
/// loop.
pub(crate) fn storage_ports(
    leading: &[PortDeclaration],
    siting: &Siting,
    supply_loop: LoopId,
) -> Vec<PortDeclaration> {
    leading
        .iter()
        .copied()
        .chain(siting.thermal_port())
        .chain([
            PortDeclaration::fluid(supply_loop, FluidType::Water),
            PortDeclaration::fluid(super::DHW_DEMAND_LOOP, FluidType::Water),
        ])
        .collect()
}
