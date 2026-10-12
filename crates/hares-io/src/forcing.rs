//! The surface-independent forcing shared across dwellings on one weather
//! file and time grid.
//!
//! The forcing is the set of per-step quantities that depend only on the
//! weather and the time grid: the humidity ratio, wet bulb and enthalpy
//! derived from the resampled weather row, the day of year, the solar
//! position and zenith, and the water mains temperature. Nothing in a row
//! depends on a dwelling's geometry, equipment or state, so one table
//! serves every dwelling built over the same weather on the same grid: a
//! fleet's homes on one weather file read one table instead of each
//! recomputing the forcing on every step.
//!
//! A [`ForcingKey`] identifies a grid: it holds plain values (never a
//! `DateTime<FixedOffset>`, whose `Eq` and `Hash` compare the UTC instant
//! alone), so two grids that share a start instant but resolve different
//! site UTC offsets hash differently. The weather content itself is not in
//! the key: a table belongs to the weather it was built from, so the
//! shared cache pairs every key with the content digest of the weather
//! series the table was built from and serves a table only to a
//! construction over the same weather.

/// The weather content's identity, hashed: [`ForcingKey`] deliberately
/// excludes the weather, so a shared table is found under a key that pairs
/// the grid with the content digest of the weather series the table was
/// built from.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ForcingCacheKey {
    /// Digest of the resampled weather series (every field's values, plus
    /// the meta fields the forcing reads). Two constructions over one
    /// weather file at one resolution hash equal; a changed file hashes
    /// differently and never shares a stale table.
    pub content: u64,
    /// The time grid the table was built for.
    pub key: ForcingKey,
}

/// Identifies one forcing table's time grid. Plain values only: the naive
/// local start (the wall clock the forcing reads), the resolved site UTC
/// offset in seconds, the horizon in steps, the resolved site coordinates
/// as bit patterns, and the civil timezone name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ForcingKey {
    /// Step resolution in seconds.
    pub time_res_secs: u32,
    /// The start's local wall clock. The forcing reads the wall clock
    /// (the annual offset, the civil day of year) and the instant (the
    /// solar position converts to UTC), so the key carries both this and
    /// the offset.
    pub local_start: chrono::NaiveDateTime,
    /// The resolved site's UTC offset in seconds.
    pub utc_offset_secs: i32,
    /// The horizon in steps: the number of rows the table holds. The
    /// horizon rule sizes the table to every step index a run can read
    /// (warm-up replays the first day, so a warm-up run's horizon is its
    /// duration's steps and at least one day's).
    pub horizon_steps: u64,
    /// The resolved site latitude, as bit patterns so a NaN or a signed
    /// zero difference keys differently.
    pub latitude_bits: u64,
    pub longitude_bits: u64,
    pub elevation_bits: u64,
    /// The civil timezone's IANA name, when one is configured.
    pub civil_timezone: Option<String>,
}

/// One step's surface-independent forcing: the values a step reads before
/// its per-surface irradiance loop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ForcingRow {
    pub humidity_ratio: f64,
    pub wet_bulb_c: f64,
    pub enthalpy_j_kg: f64,
    /// The civil day of year (DST-aware when a civil timezone is
    /// configured, the fixed-offset day otherwise).
    pub day_of_year: u32,
    pub solar_altitude_deg: f64,
    pub solar_azimuth_deg: f64,
    pub solar_zenith_deg: f64,
    pub mains_temp_c: f64,
    /// The fixed-offset day of year, captured beside the civil one so a
    /// DST transition's discrepancy stays diagnosable.
    #[cfg(feature = "observe")]
    pub raw_day_of_year: u32,
    /// The UTC day of year, captured for the same diagnostics.
    #[cfg(feature = "observe")]
    pub utc_day_of_year: u32,
}

/// The surface-independent forcing, one row per step index, built eagerly
/// in one pass and immutable afterwards. Shared as an `Arc`: every
/// dwelling on the grid reads the same table concurrently during stepping,
/// so the type is plain data (`Send + Sync`) and is never written after
/// construction.
#[derive(Debug, Clone, PartialEq)]
pub struct ForcingTable {
    rows: Vec<ForcingRow>,
}

impl ForcingTable {
    /// Wraps the pass-built rows: one row per step index of the horizon,
    /// in step order. The builder is the forcing step's own computation
    /// run once, so the rows are immutable here and afterwards.
    #[must_use]
    pub fn from_rows(rows: Vec<ForcingRow>) -> Self {
        Self { rows }
    }

    /// The table's own size in bytes: the row payload. The measurement log
    /// records this per resolution and horizon.
    #[must_use]
    pub fn size_bytes(&self) -> usize {
        self.rows.len() * std::mem::size_of::<ForcingRow>()
    }

    /// The number of rows (one per step index of the horizon).
    #[must_use]
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The row for one step index.
    ///
    /// # Panics
    ///
    /// Panics when `step_index` is at or past the table's horizon: a run's
    /// step indices never reach past the horizon the table was sized to.
    #[must_use]
    pub fn row(&self, step_index: usize) -> &ForcingRow {
        &self.rows[step_index]
    }

    /// The row for one step index, when the index is inside the horizon.
    #[must_use]
    pub fn get_row(&self, step_index: usize) -> Option<&ForcingRow> {
        self.rows.get(step_index)
    }
}
