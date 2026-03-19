//! Shared types used across HARES crates.
//!
//! This crate is the leaf of the dependency DAG. It defines the cross-cutting
//! types that multiple sibling crates need without introducing circular deps:
//! port contributions, environment state, control signals, equipment descriptors.

pub mod control_signal;
pub mod environment;
pub mod equipment;
pub mod ports;
