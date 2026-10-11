//! Fleet-level parallel simulation with weighted aggregation.

pub mod fleet;
pub mod fold;
pub mod progress;

pub use fleet::{
    DwellingBuildError, FLEET_REPORT_NAME, Fleet, FleetBuildReport, SimError, SimStatus,
    SteppableFleet,
};
pub use fold::{Coverage, FleetReport, FleetResult, HomeResult};
