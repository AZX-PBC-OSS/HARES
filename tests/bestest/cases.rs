use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BestestTier {
    Core,
    Extended,
    /// The in-depth solid-conduction series (ASHRAE 140-2023 Section 7.3 /
    /// 140-2017 5.2.3): Case 600's construction with the windows removed,
    /// zero internal gains and zero infiltration, under the in-depth
    /// radiative properties. The `alignment_oracles_regressions` target
    /// includes `cases.rs` without the BESTEST tests and never constructs
    /// this tier, which is why the allowance is needed.
    #[allow(dead_code)]
    InDepth,
}

#[derive(Debug, Clone)]
pub struct BestestCase {
    pub id: &'static str,
    pub description: &'static str,
    pub tier: BestestTier,
    pub fixture_file: &'static str,
    pub timestep_seconds: i64,
}

impl BestestCase {
    pub fn fixture_path(&self) -> PathBuf {
        fixture_root().join(self.fixture_file)
    }
}

pub fn core_cases() -> Vec<BestestCase> {
    vec![
        BestestCase {
            id: "600",
            description: "Low-mass conditioned building, annual loads",
            tier: BestestTier::Core,
            fixture_file: "600.toml",
            timestep_seconds: 3600,
        },
        BestestCase {
            id: "900",
            description: "High-mass conditioned building, annual loads",
            tier: BestestTier::Core,
            fixture_file: "900.toml",
            timestep_seconds: 3600,
        },
        BestestCase {
            id: "600FF",
            description: "Free-float lightweight envelope",
            tier: BestestTier::Core,
            fixture_file: "600ff.toml",
            timestep_seconds: 3600,
        },
        BestestCase {
            id: "900FF",
            description: "Heavyweight free-float envelope",
            tier: BestestTier::Core,
            fixture_file: "900ff.toml",
            timestep_seconds: 3600,
        },
        BestestCase {
            id: "640",
            description: "Setback thermostat heating energy",
            tier: BestestTier::Core,
            fixture_file: "640.toml",
            timestep_seconds: 3600,
        },
        BestestCase {
            id: "650",
            description: "Night ventilation, annual cooling load",
            tier: BestestTier::Core,
            fixture_file: "650.toml",
            timestep_seconds: 3600,
        },
        BestestCase {
            id: "950",
            description: "High-mass night ventilation, annual cooling load",
            tier: BestestTier::Core,
            fixture_file: "950.toml",
            timestep_seconds: 3600,
        },
    ]
}

pub fn extended_cases() -> Vec<BestestCase> {
    vec![
        BestestCase {
            id: "610",
            description: "South shading overhang",
            tier: BestestTier::Extended,
            fixture_file: "610.toml",
            timestep_seconds: 3600,
        },
        BestestCase {
            id: "620",
            description: "East/west windows",
            tier: BestestTier::Extended,
            fixture_file: "620.toml",
            timestep_seconds: 3600,
        },
        BestestCase {
            id: "920",
            description: "East/west windows, high mass",
            tier: BestestTier::Extended,
            fixture_file: "920.toml",
            timestep_seconds: 3600,
        },
        BestestCase {
            id: "CE100",
            description: "DX cooling equipment case",
            tier: BestestTier::Extended,
            fixture_file: "ce100.toml",
            timestep_seconds: 3600,
        },
        BestestCase {
            id: "CE200",
            description: "DX cooling equipment variant",
            tier: BestestTier::Extended,
            fixture_file: "ce200.toml",
            timestep_seconds: 3600,
        },
        BestestCase {
            id: "S5.4-HP",
            description: "Heat pump heating performance",
            tier: BestestTier::Extended,
            fixture_file: "s54_heat_pump.toml",
            timestep_seconds: 3600,
        },
    ]
}

/// The in-depth solid-conduction cases (ASHRAE 140-2023 Section 7.3 /
/// 140-2017 5.2.3): Case 600's envelope with no windows, zero internal gains
/// and zero infiltration, under the in-depth radiative properties (exterior
/// solar absorptance 0.1, IR emittance 0.1 interior and exterior). Consumed by
/// the `bestest` target and the `regression` target's include of this
/// module; the `alignment_oracles_regressions` target includes only
/// `cases.rs`, which leaves the function unused there.
#[allow(dead_code)]
pub fn in_depth_cases() -> Vec<BestestCase> {
    vec![
        BestestCase {
            id: "195",
            description: "Solid conduction, 20/20 thermostat",
            tier: BestestTier::InDepth,
            fixture_file: "195.toml",
            timestep_seconds: 3600,
        },
        BestestCase {
            id: "395",
            description: "Solid conduction, 20/27 deadband thermostat",
            tier: BestestTier::InDepth,
            fixture_file: "395.toml",
            timestep_seconds: 3600,
        },
    ]
}

pub fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/bestest")
}

#[cfg(test)]
mod tests {
    use super::{BestestTier, core_cases, extended_cases};

    #[test]
    fn extended_cases_are_defined_and_tagged() {
        let cases = extended_cases();
        assert!(!cases.is_empty(), "extended BESTEST cases must be present");
        assert!(
            cases
                .iter()
                .all(|c| matches!(c.tier, BestestTier::Extended))
        );
        assert!(cases.iter().all(|c| !c.description.is_empty()));
        assert!(cases.iter().all(|c| c.timestep_seconds > 0));
    }

    #[test]
    fn core_cases_have_fixture_paths() {
        for c in core_cases() {
            let path = c.fixture_path();
            assert!(!c.id.is_empty());
            assert!(!path.as_os_str().is_empty());
        }
    }
}
