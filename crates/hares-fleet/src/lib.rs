//! Fleet-level parallel simulation with weighted aggregation.

pub mod fleet;
pub mod fold;
pub mod progress;

pub use fleet::{
    DwellingBuildError, FLEET_REPORT_NAME, Fleet, FleetBuildReport, FleetError, HomeStatus,
    SimError, SteppableFleet, home_status,
};
pub use fold::{Alignment, Coverage, FleetReport, FleetResult, HomeResult};
