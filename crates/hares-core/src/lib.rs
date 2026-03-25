//! Core simulation engine: dwelling, clock, scheduler, and checkpoint.

pub mod actor;
pub mod actors;
pub mod checkpoint;
pub mod clock;
pub mod diagnostics;
pub mod dwelling;
pub mod engine;
pub mod environment;
pub mod invariants;
#[cfg(feature = "observe")]
pub mod observer;
#[cfg(feature = "observe")]
mod observer_capture;
pub mod rng;
pub mod scheduler;
pub mod telemetry;

pub use actor::{Actor, ActorInterest};
pub use clock::SimClock;
pub use dwelling::{
    Dwelling, DwellingConfig, SimulationResults as DwellingSimulationResults, StepResult,
    building_to_boundary_inputs, building_to_zone_inputs,
};
pub use engine::{KernelTimer, SimStatus, SimulationEngine, SimulationResults};
pub use environment::EnvironmentManager;
pub use hares_io::SimulationConfig;
pub use rng::derive_dwelling_rng;
pub use telemetry::DwellingTelemetry;
