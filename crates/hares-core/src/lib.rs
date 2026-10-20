//! Core simulation engine: dwelling, clock, scheduler, and checkpoint.

// The unit-test binary installs the counting allocator so the dwelling's
// snapshot-path allocation tests read real per-thread counts; it applies
// to this crate's test build alone.
#[cfg(test)]
#[global_allocator]
static TEST_ALLOCATOR: hares_types::alloc_count::CountingAllocator =
    hares_types::alloc_count::CountingAllocator;

pub mod actor;
pub mod actor_registry;
pub mod actors;
mod ambient_air;
#[cfg(test)]
// The build identity's file-based git parsing, included into the test
// build beside the build script's own include of the same file.
#[path = "../build_support/git.rs"]
mod build_support;
pub mod checkpoint;
pub mod checksum;
pub mod clock;
pub mod diagnostics;
pub mod dwelling;
pub mod engine;
pub mod environment;
pub mod health;
pub mod input_schema;
pub mod invariants;
pub mod observation;
#[cfg(feature = "observe")]
pub mod observer;
#[cfg(feature = "observe")]
mod observer_capture;
pub mod replay;
pub mod resolved;
pub mod result;
pub mod rng;
pub mod scheduler;
pub mod step_counts;
pub mod telemetry;
#[cfg(test)]
#[path = "../../../tests/support/temp_file.rs"]
mod temp_file;

pub use actor::{Actor, ActorEquipment, ActorInterest, ActorTarget};
pub use actor_registry::{ActorConfig, ActorFactory, ActorRegistry};
pub use checkpoint::CHECKPOINT_VERSION;
pub use checkpoint::DwellingCheckpoint;
pub use clock::SimClock;
pub use dwelling::COLLECTOR_AREA_PER_W;
pub use dwelling::{
    BatteryLutData, Dwelling, DwellingBlueprint, DwellingBlueprintBuilder, DwellingConfig,
    DwellingSettings, HpxmlInputs, PremiseZip, StepResult, building_to_boundary_inputs,
    building_to_zone_inputs, shipped_defaults_dir,
};
pub use engine::{KernelTimer, SimulationEngine};
pub use environment::{
    EnvironmentInitOptions, EnvironmentManager, FORCE_PER_STEP_FORCING, FORCING_COMPUTATIONS,
    FORCING_NANOS, resample_weather,
};
pub use hares_io::SimulationConfig;
pub use hares_io::{InputRecord, InputRecordSource, InputRecords};
#[cfg(feature = "profiling")]
pub use health::{ActorTiming, ActorTimings};
pub use health::{RunHealth, WarmupOutcome, WarmupResiduals};
pub use input_schema::input_schema;
pub use observation::ObservationPlan;
pub use rand_chacha::ChaCha8Rng;
pub use replay::{RecordedBuild, Replay};
pub use resolved::{
    RValueSource, ResolvedBoundary, ResolvedClock, ResolvedEnvelope, ResolvedEquipment,
    ResolvedInfiltration, ResolvedLayer, ResolvedModel, ResolvedWindow,
};
pub use result::{
    BuildInfo, EquipmentProvenance, Provenance, ReportOutput, ReportResult, RunMetrics, RunResult,
    TariffRecord,
};
pub use rng::SeededStream;
pub use rng::derive_dwelling_rng;
pub use telemetry::DwellingTelemetry;
pub use telemetry::ObservationWarnOnce;
