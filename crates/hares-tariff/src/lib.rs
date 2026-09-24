//! Electric and gas tariff evaluation for residential simulations.
//!
//! Parses OpenEI URDB tariff JSON into strongly-typed [`ElectricTariff`]
//! and [`GasTariff`] structures, evaluates time-of-use energy rates,
//! tiered energy blocks, demand charges (with optional ratchets), and
//! export compensation, and produces periodic billing summaries.

pub mod billing;
pub mod evaluator;
pub mod types;
pub mod urdb;

pub use billing::{BillingPeriodSummary, BillingState};
pub use evaluator::TariffEvaluator;
pub use types::{
    CppConfig, DemandRate, ElectricTariff, EnergyRate, ExportMode, ExportRate, FixedCharges,
    GasTariff, GasTieredBlock, RatchetConfig, TieredBlock,
};
pub use urdb::{UrdbParseError, parse as parse_urdb};
