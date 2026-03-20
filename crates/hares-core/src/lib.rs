//! Core simulation engine: dwelling, clock, scheduler, and checkpoint.

pub mod checkpoint;
pub mod clock;
pub mod dwelling;
pub mod engine;
pub mod environment;
pub mod rng;
pub mod scheduler;
pub mod telemetry;

pub use checkpoint::{CHECKPOINT_VERSION, DwellingCheckpoint};
pub use clock::SimClock;
pub use dwelling::{
    Dwelling, DwellingConfig, SimulationResults as DwellingSimulationResults, StepResult,
};
pub use engine::{KernelTimer, SimStatus, SimulationEngine, SimulationResults};
pub use environment::EnvironmentManager;
pub use hares_io::SimulationConfig;
pub use rng::derive_dwelling_rng;
pub use telemetry::DwellingTelemetry;
