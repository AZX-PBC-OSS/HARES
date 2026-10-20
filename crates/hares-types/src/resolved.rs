//! The resolved-parameter read-back's value type: one parameter the
//! physics actually uses, reported by the equipment's
//! `resolved_parameters` trait method (`hares-equipment`) and carried in
//! `hares-core`'s resolved model.

use crate::output::Unit;

/// One resolved parameter of one equipment: the name the read-back
/// reports it under, the value the physics uses (SI), and the value's
/// unit. The name is the plan's required-parameter table's spelling; a
/// class may report parameters beyond that table's minimum.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedParameter {
    pub name: &'static str,
    pub value: f64,
    pub unit: Unit,
}

impl ResolvedParameter {
    #[must_use]
    pub const fn new(name: &'static str, value: f64, unit: Unit) -> Self {
        Self { name, value, unit }
    }
}
