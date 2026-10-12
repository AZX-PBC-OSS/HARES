//! Input sources and parsed-input handles: where a run's inputs come from
//! (a path, UTF-8 text, or bytes) and the parsed documents shared
//! immutably (`Arc`) across every dwelling built from them. One parse per
//! unique input: a handle parsed once is reused by reference, and the
//! blueprint build reads the shared handles, never the files again.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration as ChronoDuration, FixedOffset};

use crate::defaults::DefaultsStore;
use crate::forcing::{ForcingKey, ForcingTable};
use crate::hpxml::Building;
use crate::input::{InputRecord, InputRecordSource};
use crate::schedule::ScheduleTimeSeries;
use crate::schedule_resolve::{DEFAULT_SCHEDULES_CSV, DefaultProfiles};
use crate::weather::{WeatherFormat, WeatherMeta, WeatherTimeSeries};
use hares_types::HaresError;
use sha2::Digest;

/// The document's record: where it came from and the SHA-256 of the bytes
/// the parse read. A path source reads the file's bytes (the parse read
/// the same file), a content source hashes its own.
fn input_record(source: &InputSource) -> Result<InputRecord, HaresError> {
    let bytes = source.read()?;
    Ok(InputRecord {
        source: match source {
            InputSource::Path(path) => InputRecordSource::Path(path.clone()),
            InputSource::Text(text) => InputRecordSource::Text(Arc::clone(text)),
            InputSource::Bytes(bytes) => InputRecordSource::Bytes(Arc::clone(bytes)),
        },
        sha256: sha2::Sha256::digest(&bytes).into(),
    })
}

/// The weather input's record, like [`input_record`].
fn weather_record(source: &WeatherInput) -> Result<InputRecord, HaresError> {
    match source {
        WeatherInput::Path(path) => input_record(&InputSource::Path(path.clone())),
        WeatherInput::Text { content, .. } => input_record(&InputSource::Text(Arc::clone(content))),
        WeatherInput::Bytes { content, .. } => {
            input_record(&InputSource::Bytes(Arc::clone(content)))
        }
    }
}

// ── Sources ─────────────────────────────────────────────────────────────────

/// Where one input document comes from: a path, in-memory UTF-8 text, or
/// in-memory bytes. A path is read by the file's own reader (today's
/// behavior, byte for byte); content is parsed from memory.
#[derive(Debug, Clone)]
pub enum InputSource {
    /// The document at this path.
    Path(PathBuf),
    /// In-memory UTF-8 text.
    Text(Arc<str>),
    /// In-memory bytes.
    Bytes(Arc<[u8]>),
}

impl InputSource {
    /// A path source.
    #[must_use]
    pub fn from_path(path: impl Into<PathBuf>) -> Self {
        Self::Path(path.into())
    }

    /// The source's path, when the source is a path.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Path(path) => Some(path.as_path()),
            Self::Text(_) | Self::Bytes(_) => None,
        }
    }

    /// The source's display form: the path, or a marker for in-memory
    /// content. Error messages name the input through this.
    #[must_use]
    pub fn display(&self) -> String {
        match self {
            Self::Path(path) => path.display().to_string(),
            Self::Text(_) => "<content text>".to_string(),
            Self::Bytes(_) => "<content bytes>".to_string(),
        }
    }

    /// The document's content: a path's bytes read from the file, content
    /// source's own bytes.
    pub fn read(&self) -> Result<Vec<u8>, HaresError> {
        match self {
            Self::Path(path) => std::fs::read(path).map_err(|err| {
                HaresError::Io(format!("'{}' could not be read: {err}", path.display()))
            }),
            Self::Text(text) => Ok(text.as_bytes().to_vec()),
            Self::Bytes(bytes) => Ok(bytes.to_vec()),
        }
    }

    /// The document's content as UTF-8 text: a path's bytes read and
    /// validated, content source's own text (a byte source must be UTF-8;
    /// every format this engine reads is a text format).
    pub fn read_text(&self) -> Result<String, HaresError> {
        match self {
            Self::Path(path) => std::fs::read_to_string(path).map_err(|err| {
                HaresError::Io(format!("'{}' could not be read: {err}", path.display()))
            }),
            Self::Text(text) => Ok(text.to_string()),
            Self::Bytes(bytes) => String::from_utf8(bytes.to_vec()).map_err(|err| {
                HaresError::Io(format!(
                    "the input is not valid UTF-8: {err}; every input format is text"
                ))
            }),
        }
    }

    /// The source's identity: a path source is its path; a content source
    /// is its content's digest, so the same content parsed twice is one
    /// parse and a reused allocation is never the same key.
    pub(crate) fn identity(&self) -> SourceIdentity {
        match self {
            Self::Path(path) => SourceIdentity::Path(path.clone()),
            Self::Text(text) => SourceIdentity::Content(content_digest(text.as_bytes())),
            Self::Bytes(bytes) => SourceIdentity::Content(content_digest(bytes)),
        }
    }
}

impl From<PathBuf> for InputSource {
    fn from(path: PathBuf) -> Self {
        Self::Path(path)
    }
}

impl From<&Path> for InputSource {
    fn from(path: &Path) -> Self {
        Self::Path(path.to_path_buf())
    }
}

/// A source's identity for the many-home construction's parse cache: a
/// path source is its path, a content source its content's digest. The
/// digest, not the allocation's address: a dropped source and a different
/// source at a reused address must not share a cache entry.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum SourceIdentity {
    Path(PathBuf),
    Content(u64),
}

/// The content's 64-bit digest: the key a content source identities by,
/// so equal contents share one parse and a reused allocation never does.
fn content_digest(content: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish()
}

/// The weather input: a path (the format detected from the file, today's
/// behavior) or in-memory content with the format given, because a
/// content source cannot be re-detected (the detection reads the path's
/// extension and header lines).
#[derive(Debug, Clone)]
pub enum WeatherInput {
    /// The weather file at this path; the format detected as today.
    Path(PathBuf),
    /// In-memory UTF-8 text with the format it parses as.
    Text {
        content: Arc<str>,
        format: WeatherFormat,
    },
    /// In-memory bytes with the format they parse as.
    Bytes {
        content: Arc<[u8]>,
        format: WeatherFormat,
    },
}

impl WeatherInput {
    /// A path source.
    #[must_use]
    pub fn from_path(path: impl Into<PathBuf>) -> Self {
        Self::Path(path.into())
    }

    /// The source's path, when the source is a path.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Path(path) => Some(path.as_path()),
            Self::Text { .. } | Self::Bytes { .. } => None,
        }
    }

    /// The input's content as UTF-8 text (a byte source must be UTF-8:
    /// every weather format is text).
    pub fn read_text(&self) -> Result<String, HaresError> {
        match self {
            Self::Path(path) => std::fs::read_to_string(path).map_err(|err| {
                HaresError::Io(format!("'{}' could not be read: {err}", path.display()))
            }),
            Self::Text { content, .. } => Ok(content.to_string()),
            Self::Bytes { content, .. } => String::from_utf8(content.to_vec()).map_err(|err| {
                HaresError::Io(format!("the weather input is not valid UTF-8: {err}"))
            }),
        }
    }

    pub(crate) fn identity(&self) -> (SourceIdentity, Option<WeatherFormat>) {
        match self {
            Self::Path(path) => (SourceIdentity::Path(path.clone()), None),
            Self::Text { content, format } => (
                SourceIdentity::Content(content_digest(content.as_bytes())),
                Some(*format),
            ),
            Self::Bytes { content, format } => (
                SourceIdentity::Content(content_digest(content)),
                Some(*format),
            ),
        }
    }
}

/// The defaults source: a base directory plus per-file replacements. Every
/// file the tree carries loads from the base directory except the ones
/// `files` names, which load from the given source instead.
#[derive(Debug, Clone, Default)]
pub struct DefaultsSource {
    /// The defaults directory tree. `None` keeps today's optional
    /// `defaults_path` behavior: the store loads from the working
    /// directory's `defaults` and the schedule profiles are absent.
    pub base: Option<PathBuf>,
    /// Relative path -> replacement content, parsed instead of the base
    /// file. An unknown relative path is an error naming the known files.
    pub files: BTreeMap<String, InputSource>,
}

impl DefaultsSource {
    /// A defaults directory with no replacements.
    #[must_use]
    pub fn directory(path: impl Into<PathBuf>) -> Self {
        Self {
            base: Some(path.into()),
            files: BTreeMap::new(),
        }
    }

    /// The source's identity for the many-home construction's parse cache:
    /// the base path and every replacement's path and content digest.
    pub(crate) fn identity(&self) -> DefaultsIdentity {
        DefaultsIdentity {
            base: self.base.clone(),
            files: self
                .files
                .iter()
                .map(|(path, source)| (path.clone(), source.identity()))
                .collect(),
        }
    }
}

/// A defaults source's identity for the parse cache.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct DefaultsIdentity {
    base: Option<PathBuf>,
    files: Vec<(String, SourceIdentity)>,
}

// ── Parsed handles ──────────────────────────────────────────────────────────

/// One HPXML document, parsed once and shared by reference.
#[derive(Debug, Clone)]
pub struct ParsedHpxml {
    building: Arc<Building>,
    source: InputSource,
}

impl ParsedHpxml {
    /// Parse the document from a path or from memory.
    ///
    /// # Errors
    ///
    /// [`HaresError`] when the document cannot be read or parsed.
    pub fn parse(source: &InputSource) -> Result<Self, HaresError> {
        let building = match source {
            InputSource::Path(path) => crate::parse_hpxml(path)
                .map_err(|err| HaresError::Io(format!("HPXML parse failed: {err}")))?,
            InputSource::Text(text) => crate::hpxml::parse_hpxml_str(text)
                .map_err(|err| HaresError::Io(format!("HPXML parse failed: {err}")))?,
            InputSource::Bytes(bytes) => {
                let text = String::from_utf8(bytes.to_vec()).map_err(|err| {
                    HaresError::Io(format!("the HPXML input is not valid UTF-8: {err}"))
                })?;
                crate::hpxml::parse_hpxml_str(&text)
                    .map_err(|err| HaresError::Io(format!("HPXML parse failed: {err}")))?
            }
        };
        Ok(Self {
            building: Arc::new(building),
            source: source.clone(),
        })
    }

    /// A handle over an already-parsed building: the synthetic dwelling's
    /// constructor family, whose buildings are built in code, not parsed.
    #[must_use]
    pub fn from_building(building: Building) -> Self {
        Self {
            building: Arc::new(building),
            source: InputSource::Text(Arc::from("")),
        }
    }

    /// The parsed building, shared by reference.
    #[must_use]
    pub fn building(&self) -> Arc<Building> {
        Arc::clone(&self.building)
    }

    /// Where the document came from: the identity the shared-parse cache
    /// keys on, and the provenance record's seed.
    #[must_use]
    pub fn record(&self) -> &InputSource {
        &self.source
    }
}

/// One weather file, parsed once and shared by reference; each target
/// step's resampled series is computed once and cached on the handle, so
/// blueprints at the same resolution share one copy, and so is the
/// surface-independent forcing: one [`ForcingTable`] per time grid,
/// resolved once per grid key and cached on the handle. The handle is the
/// weather content's identity, so the tables live and die with it: a
/// dropped handle drops its tables, and a re-parse of a changed file is a
/// new handle with its own.
#[derive(Debug)]
pub struct ParsedWeather {
    series: Arc<WeatherTimeSeries>,
    resampled: Mutex<HashMap<u32, Arc<WeatherTimeSeries>>>,
    forcing: Mutex<HashMap<ForcingKey, Arc<ForcingTable>>>,
    source: WeatherInput,
}

impl ParsedWeather {
    /// Parse the weather file from a path or from memory.
    ///
    /// # Errors
    ///
    /// [`HaresError`] when the input cannot be read or parsed.
    pub fn parse(source: &WeatherInput) -> Result<Self, HaresError> {
        let series = match source {
            WeatherInput::Path(path) => crate::parse_weather(path)
                .map_err(|err| HaresError::Io(format!("weather parse failed: {err}")))?,
            WeatherInput::Text { content, format } => parse_weather_str(content, *format)
                .map_err(|err| HaresError::Io(format!("weather parse failed: {err}")))?,
            WeatherInput::Bytes { content, format } => {
                let text = String::from_utf8(content.to_vec()).map_err(|err| {
                    HaresError::Io(format!("the weather input is not valid UTF-8: {err}"))
                })?;
                parse_weather_str(&text, *format)
                    .map_err(|err| HaresError::Io(format!("weather parse failed: {err}")))?
            }
        };
        Ok(Self {
            series: Arc::new(series),
            resampled: Mutex::new(HashMap::new()),
            forcing: Mutex::new(HashMap::new()),
            source: source.clone(),
        })
    }

    /// A handle over an already-parsed series: the synthetic dwelling's
    /// constructor family, whose weather is built in code, not parsed.
    #[must_use]
    pub fn from_series(series: WeatherTimeSeries) -> Self {
        Self {
            series: Arc::new(series),
            resampled: Mutex::new(HashMap::new()),
            forcing: Mutex::new(HashMap::new()),
            source: WeatherInput::Path(PathBuf::new()),
        }
    }

    /// The parsed series at the file's own step, shared by reference.
    #[must_use]
    pub fn series(&self) -> Arc<WeatherTimeSeries> {
        Arc::clone(&self.series)
    }

    /// The file's header metadata (the site-location resolution reads it).
    #[must_use]
    pub fn meta(&self) -> &WeatherMeta {
        &self.series.meta
    }

    /// The series resampled to `step_secs`, computed once and cached on
    /// the handle: blueprints at the same resolution share one copy.
    ///
    /// # Errors
    ///
    /// [`HaresError`] when the resample fails.
    pub fn resampled(&self, step_secs: u32) -> Result<Arc<WeatherTimeSeries>, HaresError> {
        if let Some(series) = self
            .resampled
            .lock()
            .expect("resample cache")
            .get(&step_secs)
        {
            return Ok(Arc::clone(series));
        }
        let series = Arc::new(
            self.series
                .resample(step_secs)
                .map_err(|err| HaresError::Io(format!("weather resample failed: {err}")))?,
        );
        self.resampled
            .lock()
            .expect("resample cache")
            .insert(step_secs, Arc::clone(&series));
        Ok(series)
    }

    /// Where the file came from: the identity the shared-parse cache keys
    /// on, and the provenance record's seed.
    #[must_use]
    pub fn record(&self) -> &WeatherInput {
        &self.source
    }

    /// The surface-independent forcing table for one time grid, resolved
    /// once per grid key and cached on the handle: every dwelling built
    /// from this handle on that grid reads the same table through its own
    /// `Arc`. The key is the grid key alone, because this handle is the
    /// weather content's identity: a table belongs to the weather it was
    /// built from, and a table built from another handle's weather is that
    /// handle's, never shared through a process-global map. The lock is
    /// held only for the map's own operations, once per construction,
    /// never on the step path, where the table is an `Arc` read-only. A
    /// dropped handle drops its map and every table in it, so the resident
    /// tables are bounded by the handles' lifetimes.
    ///
    /// # Errors
    ///
    /// [`HaresError`] when the table's build fails.
    pub fn forcing_table(
        &self,
        key: &ForcingKey,
        build: impl FnOnce() -> Result<ForcingTable, HaresError>,
    ) -> Result<Arc<ForcingTable>, HaresError> {
        if let Some(table) = self.forcing.lock().expect("forcing cache").get(key) {
            return Ok(Arc::clone(table));
        }
        let built = Arc::new(build()?);
        crate::forcing::FORCING_COMPUTATIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.forcing
            .lock()
            .expect("forcing cache")
            .insert(key.clone(), Arc::clone(&built));
        Ok(built)
    }
}

/// Parse in-memory weather content as the given format.
fn parse_weather_str(
    content: &str,
    format: WeatherFormat,
) -> Result<WeatherTimeSeries, crate::weather::WeatherError> {
    match format {
        WeatherFormat::Epw => crate::epw::parse_epw_str(content),
        WeatherFormat::Psm3 => crate::psm3::parse_psm3_str(content),
        WeatherFormat::Tmy3 => crate::tmy3::parse_tmy3_str(content),
        WeatherFormat::ResStockCsv => {
            tracing::warn!(
                "ResStock CSV parsed with default sea-level elevation and equator location; \
                 the site-location resolution supplies the site"
            );
            crate::resstock_csv::parse_resstock_csv_str(content, 0.0, 0.0, 0.0, 0.0)
        }
    }
}

/// One HPXML document's `Site` alone, read from a path as today or from
/// in-memory content: the elevation, the latitude, the longitude and the
/// `TimeZone/UTCOffset` the site-location resolution reads, with no
/// `Building` built. The fleet's alignment pre-pass reads a home's site
/// through this.
///
/// # Errors
///
/// [`HaresError`] when the document cannot be read or its site extracted.
pub fn read_site_input(source: &InputSource) -> Result<crate::hpxml::Site, HaresError> {
    let xml = source.read_text()?;
    crate::hpxml::building::read_site_str(&xml)
        .map_err(|err| HaresError::Io(format!("HPXML site read failed: {err}")))
}

/// The fleet's alignment pre-pass reads a home's weather metadata through
/// this.
pub fn read_weather_meta_input(input: &WeatherInput) -> Result<WeatherMeta, HaresError> {
    match input {
        WeatherInput::Path(path) => crate::read_weather_meta(path)
            .map_err(|err| HaresError::Io(format!("weather header read failed: {err}"))),
        WeatherInput::Text { content, format } => weather_meta_from_lines(content, *format),
        WeatherInput::Bytes { content, format } => {
            let text = String::from_utf8(content.to_vec()).map_err(|err| {
                HaresError::Io(format!("the weather input is not valid UTF-8: {err}"))
            })?;
            weather_meta_from_lines(&text, *format)
        }
    }
}

fn weather_meta_from_lines(
    content: &str,
    format: WeatherFormat,
) -> Result<WeatherMeta, HaresError> {
    let mut lines = content.lines();
    let first = lines
        .next()
        .ok_or_else(|| HaresError::Io("empty weather input".to_string()))?;
    match format {
        WeatherFormat::Epw => crate::epw::read_meta_str(first),
        WeatherFormat::Psm3 => {
            let values = lines.next().ok_or_else(|| {
                HaresError::Io("missing PSM3 field values header (line 2)".to_string())
            })?;
            crate::psm3::read_meta_str(first, values)
        }
        WeatherFormat::Tmy3 => crate::tmy3::read_meta_str(first),
        WeatherFormat::ResStockCsv => Ok(WeatherMeta {
            location: String::new(),
            latitude: 0.0,
            longitude: 0.0,
            timezone_offset_h: 0.0,
            elevation_m: 0.0,
            wf_allows_leap_years: true,
            source_step_secs: 3600,
            midpoint_offset_secs: 1800,
            has_embedded_location: false,
            station_wmo: None,
        }),
    }
    .map_err(|err| HaresError::Io(format!("weather header read failed: {err}")))
}

/// One schedule: a parsed CSV or a generated schedule, parsed (generated)
/// once and shared by reference.
#[derive(Debug, Clone)]
pub struct ParsedSchedule {
    schedule: Arc<ScheduleTimeSeries>,
    source: Option<InputSource>,
}

impl ParsedSchedule {
    /// Parse the schedule CSV from a path or from memory. `weather_meta`
    /// supplies the timezone fallback a schedule without a time column
    /// resolves against, the same value the config path passes.
    ///
    /// # Errors
    ///
    /// [`HaresError`] when the source cannot be read or parsed.
    pub fn parse(
        source: &InputSource,
        weather_meta: Option<&WeatherMeta>,
    ) -> Result<Self, HaresError> {
        let read = |contents: String, display: &str| {
            crate::schedule::parse_schedule_csv_str(&contents, &[], weather_meta, None).map_err(
                |err| {
                    HaresError::Io(format!(
                        "schedule file '{display}' could not be read: {err}"
                    ))
                },
            )
        };
        let schedule = match source {
            InputSource::Path(path) => {
                let text = std::fs::read_to_string(path).map_err(|err| {
                    HaresError::Io(format!(
                        "schedule file '{}' could not be read: {err}",
                        path.display()
                    ))
                })?;
                read(text, &path.display().to_string())?
            }
            InputSource::Text(_) | InputSource::Bytes(_) => read(source.read_text()?, "<content>")?,
        };
        Ok(Self {
            schedule: Arc::new(schedule),
            source: Some(source.clone()),
        })
    }

    /// The generated schedule: the default profiles' columns over the run's
    /// span, the schedule a config with no schedule source requests.
    ///
    /// # Errors
    ///
    /// [`HaresError`] when a profile column cannot be generated.
    pub fn generate(
        start: DateTime<FixedOffset>,
        duration: ChronoDuration,
        interval: std::time::Duration,
        profiles: &DefaultProfiles,
    ) -> Result<Self, HaresError> {
        let interval =
            ChronoDuration::seconds(i64::try_from(interval.as_secs()).map_err(|_| {
                HaresError::Io("the time resolution exceeds i64 seconds".to_string())
            })?);
        let schedule =
            crate::hpxml_schedule::generate_default_schedule(start, duration, interval, profiles)?;
        Ok(Self {
            schedule: Arc::new(schedule),
            source: None,
        })
    }

    /// A handle over an already-built series: the synthetic dwelling's
    /// constructor family, whose schedule is built in code, not parsed.
    #[must_use]
    pub fn from_series(schedule: ScheduleTimeSeries) -> Self {
        Self {
            schedule: Arc::new(schedule),
            source: None,
        }
    }

    /// The parsed (generated) schedule, shared by reference.
    #[must_use]
    pub fn schedule(&self) -> Arc<ScheduleTimeSeries> {
        Arc::clone(&self.schedule)
    }

    /// The schedule's source file, or `None` for a generated schedule (it
    /// is a function of the HPXML and the profiles, not of a document).
    #[must_use]
    pub fn record(&self) -> Option<&InputSource> {
        self.source.as_ref()
    }
}

/// The defaults tree and the default schedule profiles, parsed once and
/// shared by reference.
#[derive(Debug, Clone)]
pub struct ParsedDefaults {
    store: Arc<DefaultsStore>,
    profiles: Arc<DefaultProfiles>,
    source: DefaultsSource,
}

impl ParsedDefaults {
    /// The defaults files the load read, in load order: the relative
    /// paths of the store's input log (the input digest's material), the
    /// read list the input schema's `defaults_files` publishes.
    #[must_use]
    pub fn read_files(&self) -> Vec<String> {
        self.store
            .input_log()
            .iter()
            .map(|(path, _)| path.clone())
            .collect()
    }

    /// Load the defaults tree and the schedule profiles from a directory
    /// plus per-file replacements, parsed once.
    ///
    /// # Errors
    ///
    /// [`HaresError`] when the tree cannot be read or a replacement names
    /// an unknown file.
    pub fn parse(source: &DefaultsSource) -> Result<Self, HaresError> {
        Self::parse_ordered(source, None, false)
    }

    /// [`Self::parse`] with the profiles already parsed: the generated
    /// schedule's caller reads them once (before the store, so a profile
    /// the generator is missing is the error the config path reports).
    pub fn parse_with_profiles(
        source: &DefaultsSource,
        profiles: Option<DefaultProfiles>,
    ) -> Result<Self, HaresError> {
        let load_profiles = || match (profiles.clone(), &source.base) {
            (Some(profiles), _) => Ok(profiles),
            (None, Some(base)) => match source.files.get(DEFAULT_SCHEDULES_CSV) {
                Some(overrides) => {
                    let content = overrides.read_text()?;
                    crate::parse_default_profiles_str(&content, &base.join(DEFAULT_SCHEDULES_CSV))
                }
                None => crate::load_default_profiles(base),
            },
            (None, None) => Ok(DefaultProfiles::default()),
        };
        let profiles = load_profiles()?;
        let store = Self::load_store(source)?;
        Ok(Self {
            store: Arc::new(store),
            profiles: Arc::new(profiles),
            source: source.clone(),
        })
    }

    /// The load order the config path reports: the profiles and the
    /// generation before the store when the schedule is generated (the
    /// generated schedule's errors precede the store's), the store before
    /// the profiles otherwise (a schedule source's construction never
    /// reads the profiles until the injection).
    fn parse_ordered(
        source: &DefaultsSource,
        profiles: Option<DefaultProfiles>,
        profiles_first: bool,
    ) -> Result<Self, HaresError> {
        let load_profiles = || match (profiles.clone(), &source.base) {
            (Some(profiles), _) => Ok(profiles),
            (None, Some(base)) => match source.files.get(DEFAULT_SCHEDULES_CSV) {
                Some(overrides) => {
                    let content = overrides.read_text()?;
                    crate::parse_default_profiles_str(&content, &base.join(DEFAULT_SCHEDULES_CSV))
                }
                None => crate::load_default_profiles(base),
            },
            (None, None) => Ok(DefaultProfiles::default()),
        };
        let store;
        let profiles;
        if profiles_first {
            profiles = Some(load_profiles()?);
            store = Self::load_store(source)?;
        } else {
            store = Self::load_store(source)?;
            profiles = Some(load_profiles()?);
        }
        let profiles = profiles.expect("the profiles load in either order");
        Ok(Self {
            store: Arc::new(store),
            profiles: Arc::new(profiles),
            source: source.clone(),
        })
    }

    fn load_store(source: &DefaultsSource) -> Result<DefaultsStore, HaresError> {
        DefaultsStore::load_with_overrides(source.base.as_deref(), &source.files).map_err(|err| {
            match &source.base {
                Some(base) => HaresError::Io(format!(
                    "defaults load failed for '{}': {err}",
                    base.display()
                )),
                None => HaresError::Io(format!("defaults load failed: {err}")),
            }
        })
    }

    /// The empty store and no profiles: the synthetic dwelling's inputs
    /// are fully explicit, so no defaults are consulted.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            store: Arc::new(DefaultsStore::empty()),
            profiles: Arc::new(DefaultProfiles::default()),
            source: DefaultsSource::default(),
        }
    }

    /// A handle over an already-loaded store and profiles.
    #[must_use]
    pub fn from_parts(store: DefaultsStore, profiles: DefaultProfiles) -> Self {
        Self {
            store: Arc::new(store),
            profiles: Arc::new(profiles),
            source: DefaultsSource::default(),
        }
    }

    /// The loaded store, shared by reference.
    #[must_use]
    pub fn store(&self) -> Arc<DefaultsStore> {
        Arc::clone(&self.store)
    }

    /// The default schedule profiles, shared by reference.
    #[must_use]
    pub fn profiles(&self) -> Arc<DefaultProfiles> {
        Arc::clone(&self.profiles)
    }

    /// A handle over the same tree with one file replaced: the replaced
    /// file's group is re-parsed over the source handle's shared tables,
    /// every other table is the source's own `Arc`. The unknown relative
    /// path is an error naming the known files.
    ///
    /// # Errors
    ///
    /// [`HaresError`] naming the unknown path and the known files, or the
    /// replacement's own parse failure.
    pub fn with_file(&self, relative_path: &str, source: InputSource) -> Result<Self, HaresError> {
        let known = self.known_files()?;
        if !known.iter().any(|file| file == relative_path) {
            let mut sorted = known;
            sorted.sort();
            return Err(HaresError::Io(format!(
                "'{relative_path}' is not a file the defaults tree carries; the known \
                 files are: {}",
                sorted.join(", ")
            )));
        }
        let mut files = self.source.files.clone();
        files.insert(relative_path.to_string(), source.clone());
        let base = self
            .source
            .base
            .clone()
            .unwrap_or_else(|| PathBuf::from("defaults"));
        let content = source.read_text().map_err(|err| {
            HaresError::Io(format!(
                "'{}' could not be read: {err}",
                base.join(relative_path).display()
            ))
        })?;
        let strict_defaults = std::env::var("HARES_STRICT_DEFAULTS").is_ok();
        let store = self
            .store
            .with_replaced_group(&base, relative_path, &content, strict_defaults)
            .map_err(|err| {
                HaresError::Io(format!(
                    "defaults load failed for '{}': {err}",
                    base.display()
                ))
            })?;
        // The default schedule profiles are the schedule side's table (the
        // store carries none): the profiles re-parse only when the
        // replacement names the profiles file, every other replacement
        // shares this handle's.
        let profiles = if relative_path == DEFAULT_SCHEDULES_CSV {
            Arc::new(crate::parse_default_profiles_str(
                &content,
                &base.join(DEFAULT_SCHEDULES_CSV),
            )?)
        } else {
            Arc::clone(&self.profiles)
        };
        Ok(Self {
            store: Arc::new(store),
            profiles,
            source: DefaultsSource {
                base: self.source.base.clone(),
                files,
            },
        })
    }

    /// Every file the tree carries, relative to the base: the store's read
    /// set plus the schedule profiles file plus the replacements.
    fn known_files(&self) -> Result<Vec<String>, HaresError> {
        let mut files = Vec::new();
        if let Some(base) = &self.source.base {
            collect_tree_files(base, "", &mut files).map_err(|err| {
                HaresError::Io(format!(
                    "the defaults tree '{}' could not be read: {err}",
                    base.display()
                ))
            })?;
        }
        for path in self.source.files.keys() {
            files.push(path.clone());
        }
        Ok(files)
    }

    /// Where the tree came from: the identity the shared-parse cache keys
    /// on, and the provenance record's seed.
    #[must_use]
    pub fn record(&self) -> &DefaultsSource {
        &self.source
    }

    /// The digest over every defaults file this handle's load read, in
    /// load order: the store's input log (the replaced files' entries
    /// carry the replacements' bytes) composed with the schedule profiles
    /// file, which every run with a defaults directory reads. The one
    /// composition the provenance's defaults digest and a replay export's
    /// verification share.
    ///
    /// # Errors
    ///
    /// [`HaresError`] when the profiles file cannot be read.
    pub fn composed_digest(&self) -> Result<[u8; 32], HaresError> {
        let source = &self.source;
        let mut log = self.store.input_log().to_vec();
        if let Some(base) = &source.base {
            // The schedule profiles file: the replacement's content when
            // the defaults source replaced it, the base tree's file
            // otherwise — the bytes the profiles load reads.
            let relative = crate::schedule_resolve::DEFAULT_SCHEDULES_CSV;
            let bytes = match source.files.get(relative) {
                Some(overrides) => overrides.read()?,
                None => {
                    let profiles = base.join(relative);
                    std::fs::read(&profiles).map_err(|err| {
                        HaresError::Io(format!(
                            "the defaults schedule profiles '{}' could not be read \
                             for their input record: {err}",
                            profiles.display()
                        ))
                    })?
                }
            };
            log.push((relative.to_string(), bytes));
        }
        Ok(crate::input::defaults_files_digest(&log))
    }
}

/// Every file under `dir`, relative, recursive.
fn collect_tree_files(dir: &Path, relative: &str, out: &mut Vec<String>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let rel = if relative.is_empty() {
            name
        } else {
            format!("{relative}/{name}")
        };
        if path.is_dir() {
            collect_tree_files(&path, &rel, out)?;
        } else {
            out.push(rel);
        }
    }
    Ok(())
}

/// The weather cache's key: the input's identity and the format a content
/// input parses as.
type WeatherCacheKey = (SourceIdentity, Option<WeatherFormat>);
type WeatherCache = Mutex<HashMap<WeatherCacheKey, Arc<ParsedWeather>>>;

/// The many-home construction's parse cache: one parse per unique input.
/// The weather and the defaults live for the cache's lifetime (a fleet's
/// homes share them); the HPXML and the schedule documents are the homes'
/// own, dropped by the caller's chunk scope through
/// [`ParsedInputsCache::evict_documents`], so a streaming construction
/// holds one chunk's documents at most.
#[derive(Debug, Default)]
pub struct ParsedInputsCache {
    weather: WeatherCache,
    defaults: Mutex<HashMap<DefaultsIdentity, Arc<ParsedDefaults>>>,
    hpxml: Mutex<HashMap<SourceIdentity, Arc<ParsedHpxml>>>,
    schedules: Mutex<HashMap<SourceIdentity, Arc<ParsedSchedule>>>,
}

impl ParsedInputsCache {
    /// An empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Drop the HPXML and the schedule documents (the per-home inputs);
    /// the weather and the defaults stay shared. The streaming
    /// construction calls this once per chunk, so the documents held are
    /// one chunk's.
    pub fn evict_documents(&self) {
        self.hpxml.lock().expect("parse cache").clear();
        self.schedules.lock().expect("parse cache").clear();
    }

    /// The inputs one config's blueprint is built from: every unique
    /// input parsed once across the cache's lifetime, the rest shared by
    /// reference.
    ///
    /// # Errors
    ///
    /// [`HaresError`] when an input fails to parse.
    pub fn parsed_inputs(
        &self,
        hpxml: &InputSource,
        schedule: Option<&InputSource>,
        weather: &WeatherInput,
        defaults: &DefaultsSource,
        generation: Option<ScheduleGeneration>,
    ) -> Result<ParsedInputs, HaresError> {
        let weather_handle = {
            let key = weather.identity();
            let mut cache = self.weather.lock().expect("parse cache");
            match cache.get(&key) {
                Some(handle) => Arc::clone(handle),
                None => {
                    let handle = Arc::new(ParsedWeather::parse(weather)?);
                    cache.insert(key, Arc::clone(&handle));
                    handle
                }
            }
        };
        let defaults_handle = {
            let key = defaults.identity();
            let mut cache = self.defaults.lock().expect("parse cache");
            match cache.get(&key) {
                Some(handle) => Arc::clone(handle),
                None => {
                    let handle = Arc::new(ParsedDefaults::parse(defaults)?);
                    cache.insert(key, Arc::clone(&handle));
                    handle
                }
            }
        };
        let hpxml_handle = {
            let key = hpxml.identity();
            let mut cache = self.hpxml.lock().expect("parse cache");
            match cache.get(&key) {
                Some(handle) => Arc::clone(handle),
                None => {
                    let handle = Arc::new(ParsedHpxml::parse(hpxml)?);
                    cache.insert(key, Arc::clone(&handle));
                    handle
                }
            }
        };
        let schedule_handle = match schedule {
            Some(source) => {
                let key = source.identity();
                let mut cache = self.schedules.lock().expect("parse cache");
                match cache.get(&key) {
                    Some(handle) => Some(Arc::clone(handle)),
                    None => {
                        let handle =
                            Arc::new(ParsedSchedule::parse(source, Some(weather_handle.meta()))?);
                        cache.insert(key, Arc::clone(&handle));
                        Some(handle)
                    }
                }
            }
            None => None,
        };
        ParsedInputs::from_handles(
            hpxml_handle,
            schedule_handle,
            weather_handle,
            defaults_handle,
            generation,
        )
    }
}

/// The four parsed-input handles one blueprint is built from: parsed once,
/// shared by every dwelling the blueprint builds. The parse resolves the
/// building's effective IECC climate zone (the HPXML's declared zone, else
/// the weather station's, the OS-HPXML default) once, with its warning; a
/// handle built from already-parsed parts carries the building's own zone
/// and no warning.
#[derive(Debug, Clone)]
pub struct ParsedInputs {
    pub hpxml: Arc<ParsedHpxml>,
    pub weather: Arc<ParsedWeather>,
    pub schedule: Arc<ParsedSchedule>,
    pub defaults: Arc<ParsedDefaults>,
    /// The building's effective IECC climate zone for the equipment
    /// resolution, resolved once at parse.
    pub effective_climate_zone: Option<String>,
    /// The zone resolution's warning (the derivation's record), `None`
    /// when the HPXML declared its own zone or the handles were built from
    /// already-parsed parts.
    pub climate_zone_warning: Option<hares_types::Warning>,
    /// The input records of the files the parse read, computed once per
    /// parse and shared with every consumer: the HPXML, the schedule when
    /// it is a file, the weather with its resolved format, and the
    /// defaults digest. `None` when the inputs were assembled from
    /// already-parsed parts whose files the parse did not read.
    records: Option<crate::input::InputRecords>,
}

/// The generated schedule's span: the run the schedule covers, the values
/// a config with no schedule source generates against.
#[derive(Debug, Clone, Copy)]
pub struct ScheduleGeneration {
    pub start: DateTime<FixedOffset>,
    pub duration: ChronoDuration,
    pub interval: std::time::Duration,
}

impl ParsedInputs {
    /// Assemble the inputs from parsed handles: every handle used as is,
    /// so a handle parsed once shares its parse with every consumer. The
    /// schedule's absent handle generates from the defaults handle's
    /// profiles over `generation`'s span (the config path's rule), and the
    /// effective climate zone resolves once from the HPXML building and
    /// the weather header.
    ///
    /// # Errors
    ///
    /// [`HaresError`] when the schedule must be generated and no
    /// generation span is given.
    pub fn from_handles(
        hpxml: Arc<ParsedHpxml>,
        schedule: Option<Arc<ParsedSchedule>>,
        weather: Arc<ParsedWeather>,
        defaults: Arc<ParsedDefaults>,
        generation: Option<ScheduleGeneration>,
    ) -> Result<Self, HaresError> {
        let schedule = match schedule {
            Some(schedule) => schedule,
            None => {
                let generation = generation.ok_or_else(|| {
                    HaresError::Io(
                        "no schedule source and no generation span: a generated schedule \
                         needs the run's span"
                            .to_string(),
                    )
                })?;
                Arc::new(ParsedSchedule::generate(
                    generation.start,
                    generation.duration,
                    generation.interval,
                    &defaults.profiles(),
                )?)
            }
        };
        let (effective_climate_zone, climate_zone_warning) =
            crate::hpxml::climate_zone::climate_zone_resolution(
                &hpxml.building(),
                weather.meta().station_wmo.as_deref(),
            );
        Ok(Self {
            hpxml,
            weather,
            schedule,
            defaults,
            effective_climate_zone,
            climate_zone_warning,
            records: None,
        })
    }

    /// Parse every input: the HPXML, the weather, the schedule (the CSV
    /// source, or the generated schedule over `generation`'s span when the
    /// source is absent) and the defaults tree.
    ///
    /// # Errors
    ///
    /// [`HaresError`] when any input cannot be read or parsed.
    pub fn parse(
        hpxml: &InputSource,
        schedule: Option<&InputSource>,
        weather: &WeatherInput,
        defaults: &DefaultsSource,
        generation: Option<ScheduleGeneration>,
    ) -> Result<Self, HaresError> {
        // The parse order the config path reports: the HPXML, the weather,
        // the schedule (its generation's profiles before the store), the
        // defaults store, so the first broken input is the error.
        let hpxml = Arc::new(ParsedHpxml::parse(hpxml)?);
        let weather = Arc::new(ParsedWeather::parse(weather)?);
        let mut generated_schedule: Option<Arc<ParsedSchedule>> = None;
        let mut parsed_profiles: Option<DefaultProfiles> = None;
        let schedule = match schedule {
            Some(source) => Some(Arc::new(ParsedSchedule::parse(
                source,
                Some(weather.meta()),
            )?)),
            None => {
                // A generated schedule needs the default profiles: a
                // defaults source with no base directory has none, the
                // config's error naming the setting (the generated
                // schedule's file never comes from a working-directory
                // guess). The profiles load first and the generation
                // fails on the profile it is missing before the store
                // loads, the config path's order; the profiles ride into
                // the defaults handle, one read.
                let generation = generation.ok_or_else(|| {
                    HaresError::Io(
                        "no schedule source and no generation span: a generated schedule \
                         needs the run's span"
                            .to_string(),
                    )
                })?;
                let defaults_base = defaults.base.as_ref().ok_or_else(|| {
                    HaresError::Io(format!(
                        "no defaults directory is configured: a generated schedule \
                         needs the default schedule profiles file '{}' from the \
                         defaults directory; set defaults_path",
                        crate::schedule_resolve::DEFAULT_SCHEDULES_CSV,
                    ))
                })?;
                let profiles = match defaults.files.get(DEFAULT_SCHEDULES_CSV) {
                    Some(overrides) => {
                        let content = overrides.read_text()?;
                        crate::parse_default_profiles_str(
                            &content,
                            &defaults_base.join(DEFAULT_SCHEDULES_CSV),
                        )?
                    }
                    None => crate::load_default_profiles(defaults_base)?,
                };
                let schedule = ParsedSchedule::generate(
                    generation.start,
                    generation.duration,
                    generation.interval,
                    &profiles,
                )?;
                generated_schedule = Some(Arc::new(schedule));
                parsed_profiles = Some(profiles);
                None
            }
        };
        // The generated schedule's profiles were parsed before the
        // generation (the config path's order) and ride in; a schedule
        // source's store loads first, the profiles with it.
        let defaults_handle = match parsed_profiles {
            Some(profiles) => ParsedDefaults::parse_with_profiles(defaults, Some(profiles))?,
            None => ParsedDefaults::parse(defaults)?,
        };
        let defaults = Arc::new(defaults_handle);
        let mut inputs = Self::from_handles(
            hpxml,
            schedule.or(generated_schedule),
            weather,
            defaults,
            generation,
        )?;
        // The records ride the parse, computed once and shared: the
        // handles' sources digested, the defaults load's own log composed
        // with the schedule profiles file.
        inputs.records = Some(Self::build_records(&inputs)?);
        Ok(inputs)
    }

    /// The input records of the files the parse read: the HPXML, the
    /// schedule when it is a file, the weather with its resolved format,
    /// and the defaults digest. The defaults digest runs over every
    /// defaults file the load read, in load order, then the schedule
    /// profiles file. Every run with a defaults directory reads that
    /// file: a generated schedule builds on it, and a file schedule's
    /// injection resolves the profiles the CSV lacks against it, so its
    /// edit moves numbers under either schedule source and the digest
    /// composes it for both.
    ///
    /// # Errors
    ///
    /// [`HaresError`] when a path source's file cannot be read for its
    /// digest.
    fn build_records(inputs: &ParsedInputs) -> Result<crate::input::InputRecords, HaresError> {
        let hpxml = input_record(inputs.hpxml.record())?;
        let weather_source = inputs.weather.record();
        let weather = weather_record(weather_source)?;
        let weather_format = match weather_source {
            WeatherInput::Path(path) => crate::weather::detect_weather_format(path)
                .map_err(|err| HaresError::Io(err.to_string()))?,
            WeatherInput::Text { format, .. } | WeatherInput::Bytes { format, .. } => *format,
        };
        let schedule = match inputs.schedule.record() {
            Some(source) => Some(input_record(source)?),
            None => None,
        };
        let defaults_files_sha256 = inputs.defaults.composed_digest()?;
        let defaults_replacements = inputs
            .defaults
            .record()
            .files
            .iter()
            .map(|(relative, replacement)| Ok((relative.clone(), input_record(replacement)?)))
            .collect::<Result<Vec<_>, HaresError>>()?;
        Ok(crate::input::InputRecords {
            hpxml,
            schedule,
            weather,
            weather_format,
            defaults_files_sha256,
            defaults_replacements,
        })
    }

    /// The input records of the files the parse read, `None` when the
    /// inputs were assembled from already-parsed parts.
    #[must_use]
    pub fn records(&self) -> Option<&crate::input::InputRecords> {
        self.records.as_ref()
    }

    /// [`Self::parse`]'s records for an assembly from sources: the
    /// handles' sources digested (the HPXML, the schedule when it is a
    /// file, the weather with its resolved format) and the defaults
    /// load's log composed with the schedule profiles file. A caller that
    /// assembled the handles from its own sources calls this once, the
    /// parse's one records computation; an assembly from pre-parsed parts
    /// records nothing.
    ///
    /// # Errors
    ///
    /// [`HaresError`] when a path source's file cannot be read for its
    /// digest.
    pub fn compute_records(&mut self) -> Result<(), HaresError> {
        self.records = Some(Self::build_records(self)?);
        Ok(())
    }

    /// The synthetic assembly: every part already parsed, the effective
    /// zone the caller resolved, no warning, and no records (the parse
    /// read no file).
    #[must_use]
    pub fn from_parts_with_zone(
        hpxml: Arc<ParsedHpxml>,
        schedule: Arc<ParsedSchedule>,
        weather: Arc<ParsedWeather>,
        defaults: Arc<ParsedDefaults>,
        effective_climate_zone: Option<String>,
    ) -> Self {
        Self {
            hpxml,
            weather,
            schedule,
            defaults,
            effective_climate_zone,
            climate_zone_warning: None,
            records: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    /// A content source's cache key is its content's digest: equal
    /// contents share one key (one parse through the cache), whatever
    /// allocation each source landed at.
    #[test]
    fn equal_content_sources_share_one_cache_key() {
        let document = std::fs::read_to_string(
            repo_root().join("tests/fixtures/hpxml/ochre_samples/base.xml"),
        )
        .expect("the document reads");
        let first = InputSource::Text(document.clone().into());
        let second = InputSource::Text(document.into());
        assert_eq!(
            first.identity(),
            second.identity(),
            "equal contents are one cache key"
        );

        let weather = WeatherInput::Path(
            repo_root().join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        );
        let defaults = DefaultsSource::directory(repo_root().join("defaults"));
        let schedule =
            InputSource::from_path(repo_root().join("data/examples/BEopt_example_schedule.csv"));
        let cache = ParsedInputsCache::new();
        let before = crate::parse_counts::parse_counts();
        let first_inputs = cache
            .parsed_inputs(&first, Some(&schedule), &weather, &defaults, None)
            .expect("the first source parses");
        let second_inputs = cache
            .parsed_inputs(&second, Some(&schedule), &weather, &defaults, None)
            .expect("the second source parses");
        let after = crate::parse_counts::parse_counts();
        assert!(
            Arc::ptr_eq(&first_inputs.hpxml, &second_inputs.hpxml),
            "equal contents share one parse"
        );
        assert_eq!(
            after.hpxml - before.hpxml,
            1,
            "equal contents parse once, not once per allocation"
        );
    }

    /// A dropped content source and a different source at a reused
    /// address are distinct cache keys: the key is the content's digest,
    /// never the allocation's address, so the second source cannot hit
    /// the first source's cache entry.
    #[test]
    fn a_dropped_source_and_a_reused_address_are_distinct_cache_keys() {
        let first = InputSource::Text("the first document".into());
        let first_identity = first.identity();
        drop(first);
        // The same byte length: the same allocation size class, the
        // address most likely reused.
        let second = InputSource::Text("the second docume".into());
        let second_identity = second.identity();
        assert_ne!(
            first_identity, second_identity,
            "a dropped source and a different source at a reused address \
             are distinct cache keys"
        );
    }

    /// Two different content sources through one cache parse twice: the
    /// second source's entry is never the first's, however the allocator
    /// reuses the first source's address after its drop.
    #[test]
    fn different_content_sources_parse_once_each_through_one_cache() {
        let fixture = repo_root().join("tests/fixtures/hpxml/ochre_samples");
        let first_text =
            std::fs::read_to_string(fixture.join("base.xml")).expect("the first document reads");
        let second_text = std::fs::read_to_string(fixture.join("base-pv.xml"))
            .expect("the second document reads");
        let weather = WeatherInput::Path(
            repo_root().join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        );
        let defaults = DefaultsSource::directory(repo_root().join("defaults"));

        let cache = ParsedInputsCache::new();
        let first_source = InputSource::Text(first_text.into());
        let schedule =
            InputSource::from_path(repo_root().join("data/examples/BEopt_example_schedule.csv"));
        let first_inputs = cache
            .parsed_inputs(&first_source, Some(&schedule), &weather, &defaults, None)
            .expect("the first source parses");
        drop(first_source);
        let second_source = InputSource::Text(second_text.into());
        let second_inputs = cache
            .parsed_inputs(&second_source, Some(&schedule), &weather, &defaults, None)
            .expect("the second source parses");

        assert!(
            !Arc::ptr_eq(&first_inputs.hpxml, &second_inputs.hpxml),
            "different contents are different parses"
        );
    }
}
