//! Fleet-level parallel simulation with weighted aggregation.

// The unit-test binary installs the counting allocator so the fleet's
// step-handoff allocation witness reads real per-thread counts; it applies
// to this crate's test build alone.
#[cfg(test)]
#[global_allocator]
static TEST_ALLOCATOR: hares_types::alloc_count::CountingAllocator =
    hares_types::alloc_count::CountingAllocator;

pub mod fleet;
pub mod fold;
pub mod progress;

pub use fleet::{
    DwellingBuildError, FLEET_REPORT_NAME, Fleet, FleetBuildReport, FleetError, HomeStatus,
    SimError, SteppableFleet, home_status,
};
pub use fold::{
    Alignment, Coverage, FleetClosureTelemetry, FleetReport, FleetResult, HomeClosureTelemetry,
    HomeResult,
};
