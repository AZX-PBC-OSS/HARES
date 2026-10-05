//! Which of a thermostat's two setpoints a unit serves.
//!
//! OCHRE addresses each HVAC unit's external control by its one end use
//! (`HVAC.py:271-280`, `update_external_control`: `{end_use} Setpoint (C)`),
//! so a single-purpose unit names its axis. A unit that serves both (the
//! ideal unit) does not, and a control that moves one axis must name it.

use serde::{Deserialize, Serialize};

use crate::EndUse;

/// One of a thermostat's two setpoints.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ThermostatAxis {
    Heating,
    Cooling,
}

impl ThermostatAxis {
    /// The axis an HVAC end use serves.
    #[must_use]
    pub fn of_end_use(end_use: &EndUse) -> Option<Self> {
        if *end_use == EndUse::HVAC_HEATING {
            Some(Self::Heating)
        } else if *end_use == EndUse::HVAC_COOLING {
            Some(Self::Cooling)
        } else {
            None
        }
    }
}

/// The setpoints a unit's thermostat serves, as the unit declares them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThermostatAxes {
    One(ThermostatAxis),
    /// Heating and cooling, from one unit.
    Both,
}

impl ThermostatAxes {
    /// The axis a one-axis control on this unit moves: its only axis, or,
    /// for a unit serving both, the one the control names. `None` when the
    /// control names none on a unit serving both, or names the axis a
    /// single-purpose unit does not serve.
    #[must_use]
    pub fn select(self, named: Option<ThermostatAxis>) -> Option<ThermostatAxis> {
        match (self, named) {
            (Self::One(axis), None) => Some(axis),
            (Self::One(axis), Some(named)) => (axis == named).then_some(axis),
            (Self::Both, named) => named,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_one_axis_control_moves_the_axis_the_unit_serves_or_names() {
        use ThermostatAxis::{Cooling, Heating};
        let furnace = ThermostatAxes::One(Heating);
        assert_eq!(furnace.select(None), Some(Heating));
        assert_eq!(furnace.select(Some(Heating)), Some(Heating));
        assert_eq!(furnace.select(Some(Cooling)), None);
        assert_eq!(ThermostatAxes::Both.select(Some(Cooling)), Some(Cooling));
        assert_eq!(ThermostatAxes::Both.select(None), None);
    }
}
