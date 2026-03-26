//! Built-in actor implementations for the dwelling orchestrator.

mod ideal_thermostat;
mod occupant;
mod solver_feedback;

pub use ideal_thermostat::{IdealThermostat, OverrideState};
pub use occupant::{EquipmentBehavior, Occupant, Presence};
pub use solver_feedback::SolverFeedbackActor;
