use std::fs;
use std::io;
use std::path::PathBuf;

pub const REQUIRED_FILES: [&str; 5] = [
    "building.xml",
    "schedule.csv",
    "weather.epw",
    "reference_output.parquet",
    "config.toml",
];

#[derive(Debug, Clone)]
pub struct ParityFixture {
    pub id: String,
    pub building_xml: PathBuf,
    pub schedule_csv: PathBuf,
    /// The case's weather held at the case's step by the Python adapter's
    /// OCHRE hold (`weather_ochre_hold_<time_res>s.epw`, committed beside the
    /// hourly source, `weather.epw`, the parity run's comparison never
    /// reading the engine's resampling).
    pub weather_ochre_hold: PathBuf,
    pub reference_output_parquet: PathBuf,
    pub config_toml: PathBuf,
}

#[derive(Debug, Clone)]
pub struct IncompleteFixture {
    pub id: String,
    pub root: PathBuf,
    pub missing_files: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum DiscoveredFixture {
    Complete(ParityFixture),
    Incomplete(IncompleteFixture),
}

pub fn parity_fixture_root() -> PathBuf {
    if let Some(explicit_root) = std::env::var_os("HARES_PARITY_FIXTURE_ROOT") {
        return PathBuf::from(explicit_root);
    }

    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/parity")
}

pub fn discover_fixtures() -> io::Result<Vec<DiscoveredFixture>> {
    let root = parity_fixture_root();
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut fixtures = Vec::new();
    for entry in fs::read_dir(&root)? {
        let entry = entry?;
        let entry_path = entry.path();
        if !entry_path.is_dir() {
            continue;
        }

        let fixture_id = entry.file_name().to_string_lossy().trim().to_string();
        if fixture_id.is_empty() {
            continue;
        }

        let mut missing_files = Vec::new();
        for required in REQUIRED_FILES {
            if !entry_path.join(required).exists() {
                missing_files.push(required.to_string());
            }
        }

        // The case's step, from its config: the held weather file's interval
        // must equal it so the environment resample is a no-op. The held
        // file is required beside the hourly source: the comparison reads
        // the adapter's hold, never the engine's resampling.
        let config_path = entry_path.join("config.toml");
        let mut weather_ochre_hold = entry_path.join(String::new());
        if config_path.exists() {
            let config_contents = fs::read_to_string(&config_path)?;
            let config: toml::Table = toml::from_str(&config_contents)
                .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
            let time_res_s: i64 = config
                .get("simulation")
                .and_then(|sim| sim.get("time_res"))
                .and_then(|v| v.as_integer())
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("fixture {fixture_id}: no [simulation] time_res"),
                    )
                })?;
            weather_ochre_hold = entry_path.join(format!("weather_ochre_hold_{time_res_s}s.epw"));
            if !weather_ochre_hold.exists() {
                missing_files.push(format!(
                    "weather_ochre_hold_{time_res_s}s.epw (the Python adapter's OCHRE hold of weather.epw)"
                ));
            }
        }

        if missing_files.is_empty() {
            fixtures.push(DiscoveredFixture::Complete(ParityFixture {
                id: fixture_id,
                building_xml: entry_path.join("building.xml"),
                schedule_csv: entry_path.join("schedule.csv"),
                weather_ochre_hold,
                reference_output_parquet: entry_path.join("reference_output.parquet"),
                config_toml: entry_path.join("config.toml"),
            }));
        } else {
            fixtures.push(DiscoveredFixture::Incomplete(IncompleteFixture {
                id: fixture_id,
                root: entry_path,
                missing_files,
            }));
        }
    }

    fixtures.sort_by_key(|fixture| match fixture {
        DiscoveredFixture::Complete(complete) => complete.id.clone(),
        DiscoveredFixture::Incomplete(incomplete) => incomplete.id.clone(),
    });

    Ok(fixtures)
}
