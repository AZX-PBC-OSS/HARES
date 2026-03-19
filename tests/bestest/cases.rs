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
            id: "600FF",
            description: "Free-float lightweight envelope",
            tier: BestestTier::Core,
            fixture_file: "600ff.toml",
            timestep_seconds: 60,
        },
        BestestCase {
            id: "900FF",
            description: "Heavyweight free-float envelope",
            tier: BestestTier::Core,
            fixture_file: "900ff.toml",
            timestep_seconds: 60,
        },
        BestestCase {
            id: "640",
            description: "Setback thermostat heating energy",
            tier: BestestTier::Core,
            fixture_file: "640.toml",
            timestep_seconds: 60,
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
            timestep_seconds: 60,
        },
        BestestCase {
            id: "620",
            description: "East/west windows",
            tier: BestestTier::Extended,
            fixture_file: "620.toml",
            timestep_seconds: 60,
        },
        BestestCase {
            id: "CE100",
            description: "DX cooling equipment case",
            tier: BestestTier::Extended,
            fixture_file: "ce100.toml",
            timestep_seconds: 60,
        },
        BestestCase {
            id: "CE200",
            description: "DX cooling equipment variant",
            tier: BestestTier::Extended,
            fixture_file: "ce200.toml",
            timestep_seconds: 60,
        },
        BestestCase {
            id: "S5.4-HP",
            description: "Heat pump heating performance",
            tier: BestestTier::Extended,
            fixture_file: "s54_heat_pump.toml",
            timestep_seconds: 60,
        },
    ]
}

pub fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/bestest")
}
