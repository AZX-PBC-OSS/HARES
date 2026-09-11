//! Core simulation engine: dwelling, clock, scheduler, and checkpoint.

pub mod actor;
pub mod actor_registry;
pub mod actors;
pub mod checkpoint;
pub mod checksum;
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
pub use actor_registry::{ActorConfig, ActorFactory, ActorRegistry};
pub use checkpoint::DwellingCheckpoint;
pub use clock::SimClock;
pub use dwelling::{
    BatteryLutData, Dwelling, DwellingConfig, PremiseZip,
    SimulationResults as DwellingSimulationResults, StepResult, building_to_boundary_inputs,
    building_to_zone_inputs, mass_multiplier_for_zone,
};
pub use engine::{KernelTimer, SimStatus, SimulationEngine, SimulationResults};
pub use environment::{EnvironmentInitOptions, EnvironmentManager};
pub use hares_io::SimulationConfig;
pub use rand_chacha::ChaCha8Rng;
pub use rng::derive_dwelling_rng;
pub use telemetry::DwellingTelemetry;
