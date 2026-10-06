//! Core simulation engine: dwelling, clock, scheduler, and checkpoint.

pub mod actor;
pub mod actor_registry;
pub mod actors;
mod ambient_air;
pub mod checkpoint;
pub mod checksum;
pub mod clock;
pub mod diagnostics;
pub mod dwelling;
pub mod engine;
pub mod environment;
pub mod health;
pub mod invariants;
#[cfg(feature = "observe")]
pub mod observer;
#[cfg(feature = "observe")]
mod observer_capture;
pub mod rng;
pub mod scheduler;
pub mod telemetry;
#[cfg(test)]
#[path = "../../../tests/support/temp_file.rs"]
mod temp_file;

pub use actor::{Actor, ActorEquipment, ActorInterest, ActorTarget};
pub use actor_registry::{ActorConfig, ActorFactory, ActorRegistry};
pub use checkpoint::DwellingCheckpoint;
pub use clock::SimClock;
pub use dwelling::{
    BatteryLutData, Dwelling, DwellingConfig, HpxmlInputs, PremiseZip,
    SimulationResults as DwellingSimulationResults, StepResult, building_to_boundary_inputs,
    building_to_zone_inputs,
};
pub use engine::{KernelTimer, SimStatus, SimulationEngine, SimulationResults};
pub use environment::{EnvironmentInitOptions, EnvironmentManager};
pub use hares_io::SimulationConfig;
#[cfg(feature = "profiling")]
pub use health::{ActorTiming, ActorTimings};
pub use health::{RunHealth, WarmupOutcome, WarmupResiduals};
pub use rand_chacha::ChaCha8Rng;
pub use rng::derive_dwelling_rng;
pub use telemetry::DwellingTelemetry;
