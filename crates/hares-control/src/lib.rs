//! Control signal definitions, dispatch logic, and OCHRE compatibility mapping.

pub mod capabilities;
pub mod compat;
pub mod dispatch;
pub mod signal;
pub mod types;

pub use capabilities::{ControlCapabilities, can_accept};
pub use compat::ochre_signal_to_control;
pub use dispatch::{DispatchRequest, DispatchTarget, PRIORITY_TIER_COUNT, PriorityTier};
pub use signal::ControlSignalConstructors;
pub use types::PriceSignal;
