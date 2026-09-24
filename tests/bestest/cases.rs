use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BestestTier {
    Core,
    Extended,
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
