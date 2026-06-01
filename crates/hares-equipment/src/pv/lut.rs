//! PV SAM look-up table: parsing, storage, and interpolation.
//!
//! ## Location independence
//!
//! The LUT is indexed on **solar-position-aware dimensions** (solar zenith and
//! azimuth angles) rather than calendar month/hour. This decouples the LUT
//! from the generating EPW file's geographic location:
//!
//! - **Before (T-0085):** the LUT used `month`/`hour` as temporal-proxy axes.
//!   PVWatts internally translates horizontal irradiance to tilted-surface
//!   irradiance using sun position, so a LUT generated from a Phoenix EPW
//!   would produce different AC power than one from Seattle for the same
//!   (month, hour, GHI, DNI, DHI, temp) tuple — a 5–15% systematic bias.
//!
//! - **Now:** the LUT uses `solar_zenith_deg` and `solar_azimuth_deg` axes.
//!   Sun position is computed by the LUT generator from the EPW's location
//!   metadata. The consumer computes sun position from the simulation site
//!   coordinates and queries the LUT directly. A given (zenith, azimuth,
//!   GHI, DNI, DHI, temperature) tuple yields the same AC power regardless
//!   of which EPW file generated the LUT.
//!
//! ## Cross-location usage
//!
//! The LUT embeds its generating site's latitude and longitude as Parquet
//! file-level metadata. On load the invariant check (gated behind
//! `debug_assertions` or `feature = "check_invariants"`) logs the embedded
//! location for informational purposes and warns if the metadata is absent
//! (lat/lon ≈ 0.0, indicating a pre-T-0085 LUT). A true cross-location
//! comparison against the simulation site is not possible because
//! `EnvironmentState` carries no `site_latitude_deg`/`site_longitude_deg`
//! fields (see Known Limitations below).
//!
//! LUT files in the legacy `month`/`hour` column format are **not
//! supported** — the loader returns a descriptive error if it finds those
//! columns instead of `solar_zenith_deg`/`solar_azimuth_deg`. Re-generate
//! LUTs with the updated Python adapter (`python/ochre_next/adapters/sam_pv.py`).
//!
//! ## Known Limitations
//!
//! - **What:** The invariant check cannot compare LUT location against the
//!   simulation site location because `EnvironmentState` does not carry
//!   latitude/longitude fields.
//!   **Constraint:** `hares-types::EnvironmentState` — adding location fields
//!   requires a coordinated crate-level change.
//!   **Tried:** logging the LUT location at load time (informational only).
//!   **Resolvable when:** `EnvironmentState` gains `site_latitude_deg` and
//!   `site_longitude_deg` fields (tracked in a follow-up ticket).

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
#[cfg(feature = "observe")]
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicBool, Ordering};

use arrow::array::Array;
use hares_types::HaresError;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

/// Axis normalization for nearest-neighbor distance computation.
#[derive(Clone, Debug)]
struct AxisNorm {
    min: f64,
    inv_range: f64,
}

impl AxisNorm {
    fn from_values(values: &[f64]) -> Self {
        let min = values.first().copied().unwrap_or(0.0);
        let max = values.last().copied().unwrap_or(0.0);
        let range = max - min;
        Self {
            min,
            inv_range: if range > 0.0 { 1.0 / range } else { 1.0 },
        }
    }

    #[inline]
    fn normalize(&self, v: f64) -> f64 {
        (v - self.min) * self.inv_range
    }
}

/// Interpolation method used for a single query.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InterpolationMethod {
    /// All 64 bracket corners were populated — true multi-linear.
    Multilinear,
    /// One or more corners missing — fell back to nearest-neighbor.
    NearestNeighbor,
}

#[derive(Debug)]
pub(crate) struct PvLut {
    solar_zenith_values: Vec<f64>,
    solar_azimuth_values: Vec<f64>,
    ghi_values: Vec<f64>,
    dni_values: Vec<f64>,
    dhi_values: Vec<f64>,
    temp_values: Vec<f64>,
    values: HashMap<(usize, usize, usize, usize, usize, usize), f64>,
    nn_entries: Vec<([usize; 6], f64)>,
    nn_norms: [AxisNorm; 6],
    latitude_deg: f64,
    longitude_deg: f64,
    sam_inv_eff: f64,
    sam_losses: f64,
    sam_array_type: Option<u8>,
    nn_warned: AtomicBool,
    #[cfg(feature = "observe")]
    lut_lookup_count: AtomicU64,
}

impl Clone for PvLut {
    fn clone(&self) -> Self {
        Self {
            solar_zenith_values: self.solar_zenith_values.clone(),
            solar_azimuth_values: self.solar_azimuth_values.clone(),
            ghi_values: self.ghi_values.clone(),
            dni_values: self.dni_values.clone(),
            dhi_values: self.dhi_values.clone(),
            temp_values: self.temp_values.clone(),
            values: self.values.clone(),
            nn_entries: self.nn_entries.clone(),
            nn_norms: self.nn_norms.clone(),
            latitude_deg: self.latitude_deg,
            longitude_deg: self.longitude_deg,
            sam_inv_eff: self.sam_inv_eff,
            sam_losses: self.sam_losses,
            sam_array_type: self.sam_array_type,
            nn_warned: AtomicBool::new(false),
            #[cfg(feature = "observe")]
            lut_lookup_count: AtomicU64::new(0),
        }
    }
}

impl PvLut {
    pub(crate) fn from_path(path: &Path) -> Result<Self, HaresError> {
        let ext = path
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        match ext.as_str() {
            "parquet" => Self::from_parquet(path),
            "csv" => Self::from_csv(path),
            _ => Err(HaresError::Equipment(format!(
                "unsupported PV SAM LUT format '{}'; expected .csv or .parquet: {}",
                ext,
                path.display()
            ))),
        }
    }

    pub(crate) fn from_parquet(path: &Path) -> Result<Self, HaresError> {
        let file = std::fs::File::open(path).map_err(|e| {
            HaresError::Equipment(format!(
                "failed to open PV SAM LUT '{}': {e}",
                path.display()
            ))
        })?;

        let builder = ParquetRecordBatchReaderBuilder::try_new(file).map_err(|e| {
            HaresError::Equipment(format!(
                "failed to build Parquet reader for PV SAM LUT '{}': {e}",
                path.display()
            ))
        })?;

        // Read file-level key-value metadata for location and SAM configuration.
        let parquet_meta = builder.metadata();
        let file_kv = parquet_meta.file_metadata().key_value_metadata();
        let mut latitude_deg: f64 = 0.0;
        let mut longitude_deg: f64 = 0.0;
        let mut sam_inv_eff: f64 = 0.0;
        let mut sam_losses: f64 = 0.0;
        let mut sam_array_type: Option<u8> = None;
        if let Some(kv_list) = file_kv {
            for kv in kv_list {
                match kv.key.as_str() {
                    "harvest_lut_latitude_deg" => {
                        latitude_deg = kv
                            .value
                            .as_deref()
                            .and_then(|v| v.parse::<f64>().ok())
                            .unwrap_or(0.0);
                    }
                    "harvest_lut_longitude_deg" => {
                        longitude_deg = kv
                            .value
                            .as_deref()
                            .and_then(|v| v.parse::<f64>().ok())
                            .unwrap_or(0.0);
                    }
                    "harvest_lut_sam_inv_eff" => {
                        sam_inv_eff = kv
                            .value
                            .as_deref()
                            .and_then(|v| v.parse::<f64>().ok())
                            .unwrap_or(0.0);
                    }
                    "harvest_lut_sam_losses" => {
                        sam_losses = kv
                            .value
                            .as_deref()
                            .and_then(|v| v.parse::<f64>().ok())
                            .unwrap_or(0.0);
                    }
                    "harvest_lut_sam_array_type" => {
                        sam_array_type = kv.value.as_deref().and_then(|v| v.parse::<u8>().ok());
                    }
                    _ => {}
                }
            }
        }

        let mut reader = builder.build().map_err(|e| {
            HaresError::Equipment(format!(
                "failed to read Parquet batches for PV SAM LUT '{}': {e}",
                path.display()
            ))
        })?;

        let mut rows = Vec::new();
        for batch_result in &mut reader {
            let batch = batch_result.map_err(|e| {
                HaresError::Equipment(format!(
                    "failed reading PV SAM LUT batch '{}': {e}",
                    path.display()
                ))
            })?;

            let zenith = find_column_f64(&batch, &["solar_zenith_deg"])?;
            let azimuth = find_column_f64(&batch, &["solar_azimuth_deg"])?;
            let ghi = find_column_f64(&batch, &["ghi", "ghi_w_m2"])?;
            let dni = find_column_f64(&batch, &["dni", "dni_w_m2"])?;
            let dhi = find_column_f64(&batch, &["dhi", "dhi_w_m2"])?;
            let temp = find_column_f64(&batch, &["temp_c", "temperature_c"])?;
            let ac = find_column_f64(&batch, &["ac_power_kw", "ac_kw"])?;

            let n = batch.num_rows();
            for i in 0..n {
                let Some(zenith) = get_valid_f64(zenith, i) else {
                    continue;
                };
                let Some(azimuth) = get_valid_f64(azimuth, i) else {
                    continue;
                };
                let Some(ghi) = get_valid_f64(ghi, i) else {
                    continue;
                };
                let Some(dni) = get_valid_f64(dni, i) else {
                    continue;
                };
                let Some(dhi) = get_valid_f64(dhi, i) else {
                    continue;
                };
                let Some(temp) = get_valid_f64(temp, i) else {
                    continue;
                };
                let Some(ac) = get_valid_f64(ac, i) else {
                    continue;
                };
                rows.push((zenith, azimuth, ghi, dni, dhi, temp, ac));
            }
        }

        Self::from_rows(
            path,
            rows,
            latitude_deg,
            longitude_deg,
            sam_inv_eff,
            sam_losses,
            sam_array_type,
        )
    }

    fn from_csv(path: &Path) -> Result<Self, HaresError> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            HaresError::Equipment(format!(
                "failed to read PV SAM LUT CSV '{}': {e}",
                path.display()
            ))
        })?;
        let mut lines = text.lines();
        let Some(header) = lines.next() else {
            return Err(HaresError::Equipment(format!(
                "PV SAM LUT CSV '{}' is empty",
                path.display()
            )));
        };
        let headers: Vec<&str> = header.split(',').map(str::trim).collect();
        let zenith_idx = csv_column_index(&headers, &["solar_zenith_deg"])?;
        let azimuth_idx = csv_column_index(&headers, &["solar_azimuth_deg"])?;
        let ghi_idx = csv_column_index(&headers, &["ghi", "ghi_w_m2"])?;
        let dni_idx = csv_column_index(&headers, &["dni", "dni_w_m2"])?;
        let dhi_idx = csv_column_index(&headers, &["dhi", "dhi_w_m2"])?;
        let temp_idx = csv_column_index(&headers, &["temp_c", "temperature_c"])?;
        let ac_idx = csv_column_index(&headers, &["ac_power_kw", "ac_kw"])?;

        let mut rows = Vec::new();
        for line in lines {
            let cols: Vec<&str> = line.split(',').map(str::trim).collect();
            let Some(zenith) = parse_csv_f64(cols.get(zenith_idx).copied()) else {
                continue;
            };
            let Some(azimuth) = parse_csv_f64(cols.get(azimuth_idx).copied()) else {
                continue;
            };
            let Some(ghi) = parse_csv_f64(cols.get(ghi_idx).copied()) else {
                continue;
            };
            let Some(dni) = parse_csv_f64(cols.get(dni_idx).copied()) else {
                continue;
            };
            let Some(dhi) = parse_csv_f64(cols.get(dhi_idx).copied()) else {
                continue;
            };
            let Some(temp) = parse_csv_f64(cols.get(temp_idx).copied()) else {
                continue;
            };
            let Some(ac) = parse_csv_f64(cols.get(ac_idx).copied()) else {
                continue;
            };
            rows.push((zenith, azimuth, ghi, dni, dhi, temp, ac));
        }

        Self::from_rows(path, rows, 0.0, 0.0, 0.0, 0.0, None)
    }

    fn from_rows(
        path: &Path,
        rows: Vec<(f64, f64, f64, f64, f64, f64, f64)>,
        latitude_deg: f64,
        longitude_deg: f64,
        sam_inv_eff: f64,
        sam_losses: f64,
        sam_array_type: Option<u8>,
    ) -> Result<Self, HaresError> {
        if rows.is_empty() {
            return Err(HaresError::Equipment(format!(
                "PV SAM LUT '{}' contains no valid rows",
                path.display()
            )));
        }

        let mut zenith_axis = BTreeSet::new();
        let mut azimuth_axis = BTreeSet::new();
        let mut ghi_axis = BTreeSet::new();
        let mut dni_axis = BTreeSet::new();
        let mut dhi_axis = BTreeSet::new();
        let mut temp_axis = BTreeSet::new();

        for &(zenith, azimuth, ghi, dni, dhi, temp, _) in &rows {
            zenith_axis.insert(quantize_lut_axis(zenith));
            azimuth_axis.insert(quantize_lut_axis(azimuth));
            ghi_axis.insert(quantize_lut_axis(ghi));
            dni_axis.insert(quantize_lut_axis(dni));
            dhi_axis.insert(quantize_lut_axis(dhi));
            temp_axis.insert(quantize_lut_axis(temp));
        }

        let solar_zenith_values = lut_axis_to_values(zenith_axis);
        let solar_azimuth_values = lut_axis_to_values(azimuth_axis);
        let ghi_values = lut_axis_to_values(ghi_axis);
        let dni_values = lut_axis_to_values(dni_axis);
        let dhi_values = lut_axis_to_values(dhi_axis);
        let temp_values = lut_axis_to_values(temp_axis);

        let zenith_idx = index_map(&solar_zenith_values);
        let azimuth_idx = index_map(&solar_azimuth_values);
        let ghi_idx = index_map(&ghi_values);
        let dni_idx = index_map(&dni_values);
        let dhi_idx = index_map(&dhi_values);
        let temp_idx = index_map(&temp_values);

        let mut values = HashMap::with_capacity(rows.len());
        let mut nn_entries = Vec::with_capacity(rows.len());
        for &(zenith, azimuth, ghi, dni, dhi, temp, ac) in &rows {
            let indices = [
                *zenith_idx
                    .get(&quantize_lut_axis(zenith))
                    .expect("zenith index exists"),
                *azimuth_idx
                    .get(&quantize_lut_axis(azimuth))
                    .expect("azimuth index exists"),
                *ghi_idx
                    .get(&quantize_lut_axis(ghi))
                    .expect("ghi index exists"),
                *dni_idx
                    .get(&quantize_lut_axis(dni))
                    .expect("dni index exists"),
                *dhi_idx
                    .get(&quantize_lut_axis(dhi))
                    .expect("dhi index exists"),
                *temp_idx
                    .get(&quantize_lut_axis(temp))
                    .expect("temp index exists"),
            ];
            let key = (
                indices[0], indices[1], indices[2], indices[3], indices[4], indices[5],
            );
            values.insert(key, ac);
            nn_entries.push((indices, ac));
        }

        let nn_norms = [
            AxisNorm::from_values(&solar_zenith_values),
            AxisNorm::from_values(&solar_azimuth_values),
            AxisNorm::from_values(&ghi_values),
            AxisNorm::from_values(&dni_values),
            AxisNorm::from_values(&dhi_values),
            AxisNorm::from_values(&temp_values),
        ];

        Ok(Self {
            solar_zenith_values,
            solar_azimuth_values,
            ghi_values,
            dni_values,
            dhi_values,
            temp_values,
            values,
            nn_entries,
            nn_norms,
            latitude_deg,
            longitude_deg,
            sam_inv_eff,
            sam_losses,
            sam_array_type,
            nn_warned: AtomicBool::new(false),
            #[cfg(feature = "observe")]
            lut_lookup_count: AtomicU64::new(0),
        })
    }

    /// LUT geographic location latitude in degrees.
    ///
    /// Only compiled when debug_assertions, check_invariants, or tests are
    /// active (used by `check_lut_location` and test assertions).
    #[cfg(any(test, debug_assertions, feature = "check_invariants"))]
    #[inline]
    pub(crate) fn latitude_deg(&self) -> f64 {
        self.latitude_deg
    }

    /// LUT geographic location longitude in degrees.
    ///
    /// Only compiled when debug_assertions, check_invariants, or tests are
    /// active (used by `check_lut_location` and test assertions).
    #[cfg(any(test, debug_assertions, feature = "check_invariants"))]
    #[inline]
    pub(crate) fn longitude_deg(&self) -> f64 {
        self.longitude_deg
    }

    #[inline]
    pub(crate) fn sam_inv_eff(&self) -> f64 {
        self.sam_inv_eff
    }

    #[inline]
    pub(crate) fn sam_losses(&self) -> f64 {
        self.sam_losses
    }

    /// Raw SAM `array_type` index stored in LUT metadata. Callers that need
    /// the NOCT value should use `sam_noct_c()` instead.
    // Why: sam_noct_c() reads self.sam_array_type directly; this accessor is
    // dormant until an external caller (e.g. a diagnostics layer) needs the
    // raw index. Suppression retained to keep the public(crate) surface stable.
    #[inline]
    #[allow(dead_code)]
    pub(crate) fn sam_array_type(&self) -> Option<u8> {
        self.sam_array_type
    }

    /// SAM's internal NOCT (°C) derived from this LUT's `array_type` metadata.
    ///
    /// Returns `None` when the LUT lacks `harvest_lut_sam_array_type` metadata
    /// (legacy/pre-T-0087 LUTs or CSV format). In that case the caller should
    /// fall back to the array's configured `noct_c`.
    #[inline]
    pub(crate) fn sam_noct_c(&self) -> Option<f64> {
        self.sam_array_type
            .map(|idx| super::array_config::ArrayType::from_sam_index(idx).noct_c())
    }

    pub(crate) fn interpolate(
        &self,
        solar_zenith_deg: f64,
        solar_azimuth_deg: f64,
        ghi: f64,
        dni: f64,
        dhi: f64,
        temp_c: f64,
    ) -> (f64, InterpolationMethod) {
        let solar_zenith_deg = solar_zenith_deg.clamp(
            *self
                .solar_zenith_values
                .first()
                .expect("zenith axis non-empty"),
            *self
                .solar_zenith_values
                .last()
                .expect("zenith axis non-empty"),
        );
        let solar_azimuth_deg = solar_azimuth_deg.clamp(
            *self
                .solar_azimuth_values
                .first()
                .expect("azimuth axis non-empty"),
            *self
                .solar_azimuth_values
                .last()
                .expect("azimuth axis non-empty"),
        );
        let ghi = ghi.clamp(
            *self.ghi_values.first().expect("ghi axis non-empty"),
            *self.ghi_values.last().expect("ghi axis non-empty"),
        );
        let dni = dni.clamp(
            *self.dni_values.first().expect("dni axis non-empty"),
            *self.dni_values.last().expect("dni axis non-empty"),
        );
        let dhi = dhi.clamp(
            *self.dhi_values.first().expect("dhi axis non-empty"),
            *self.dhi_values.last().expect("dhi axis non-empty"),
        );
        let temp_c = temp_c.clamp(
            *self.temp_values.first().expect("temp axis non-empty"),
            *self.temp_values.last().expect("temp axis non-empty"),
        );

        let zenith_br = axis_bracket(&self.solar_zenith_values, solar_zenith_deg);
        let azimuth_br = axis_bracket(&self.solar_azimuth_values, solar_azimuth_deg);
        let ghi_br = axis_bracket(&self.ghi_values, ghi);
        let dni_br = axis_bracket(&self.dni_values, dni);
        let dhi_br = axis_bracket(&self.dhi_values, dhi);
        let temp_br = axis_bracket(&self.temp_values, temp_c);

        #[cfg(feature = "observe")]
        {
            let new_n = self.lut_lookup_count.fetch_add(1, Ordering::Relaxed) + 1;
            tracing::debug!(lut_lookup_count = new_n, "PV LUT interpolate call",);
        }

        let mut weighted_sum = 0.0;
        let mut total_weight = 0.0;

        for (z_idx, z_w) in bracket_corners(zenith_br) {
            for (a_idx, a_w) in bracket_corners(azimuth_br) {
                for (g_idx, g_w) in bracket_corners(ghi_br) {
                    for (d_idx, d_w) in bracket_corners(dni_br) {
                        for (dh_idx, dh_w) in bracket_corners(dhi_br) {
                            for (t_idx, t_w) in bracket_corners(temp_br) {
                                let weight = z_w * a_w * g_w * d_w * dh_w * t_w;
                                if weight <= 0.0 {
                                    continue;
                                }
                                if let Some(ac_kw) = self
                                    .values
                                    .get(&(z_idx, a_idx, g_idx, d_idx, dh_idx, t_idx))
                                {
                                    weighted_sum += weight * ac_kw;
                                    total_weight += weight;
                                }
                            }
                        }
                    }
                }
            }
        }

        if total_weight > 0.0 {
            return (
                weighted_sum / total_weight,
                InterpolationMethod::Multilinear,
            );
        }

        // Sparse-grid fallback: nearest-neighbor with normalized distance.
        // Emit a throttled warn! the first time this LUT falls back.
        if self
            .nn_warned
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            tracing::warn!(
                "PV LUT nearest-neighbor fallback triggered: \
                 LUT has sparse coverage and results may be inaccurate. \
                 Re-generate the LUT with a denser grid for better accuracy.",
            );
        }

        let axes: [&[f64]; 6] = [
            &self.solar_zenith_values,
            &self.solar_azimuth_values,
            &self.ghi_values,
            &self.dni_values,
            &self.dhi_values,
            &self.temp_values,
        ];
        let query = [solar_zenith_deg, solar_azimuth_deg, ghi, dni, dhi, temp_c];
        let query_norm: [f64; 6] = std::array::from_fn(|i| self.nn_norms[i].normalize(query[i]));

        let mut best_dist = f64::INFINITY;
        let mut best_val = 0.0;
        for &(indices, ac) in &self.nn_entries {
            let mut dist2 = 0.0;
            for i in 0..6 {
                let v = self.nn_norms[i].normalize(axes[i][indices[i]]);
                let d = query_norm[i] - v;
                dist2 += d * d;
            }
            if dist2 < best_dist {
                best_dist = dist2;
                best_val = ac;
            }
        }
        (best_val, InterpolationMethod::NearestNeighbor)
    }
}

fn csv_column_index(headers: &[&str], candidates: &[&str]) -> Result<usize, HaresError> {
    candidates
        .iter()
        .find_map(|candidate| {
            headers
                .iter()
                .position(|h| h.eq_ignore_ascii_case(candidate))
        })
        .ok_or_else(|| {
            HaresError::Equipment(format!(
                "PV SAM LUT CSV missing required column; expected one of {}",
                candidates.join("|")
            ))
        })
}

fn parse_csv_f64(raw: Option<&str>) -> Option<f64> {
    let value = raw?.parse::<f64>().ok()?;
    if value.is_finite() { Some(value) } else { None }
}

#[cfg(test)]
impl PvLut {
    // Why: from_raw is only compiled in test builds; the dead_code lint fires
    // in non-test builds where cfg(test) modules are excluded from analysis.
    #[allow(dead_code)]
    pub(crate) fn from_raw(
        solar_zenith_values: Vec<f64>,
        solar_azimuth_values: Vec<f64>,
        ghi_values: Vec<f64>,
        dni_values: Vec<f64>,
        dhi_values: Vec<f64>,
        temp_values: Vec<f64>,
        entries: Vec<([usize; 6], f64)>,
    ) -> Self {
        let mut values = HashMap::with_capacity(entries.len());
        for &(indices, ac) in &entries {
            let key = (
                indices[0], indices[1], indices[2], indices[3], indices[4], indices[5],
            );
            values.insert(key, ac);
        }
        let nn_norms = [
            AxisNorm::from_values(&solar_zenith_values),
            AxisNorm::from_values(&solar_azimuth_values),
            AxisNorm::from_values(&ghi_values),
            AxisNorm::from_values(&dni_values),
            AxisNorm::from_values(&dhi_values),
            AxisNorm::from_values(&temp_values),
        ];
        Self {
            solar_zenith_values,
            solar_azimuth_values,
            ghi_values,
            dni_values,
            dhi_values,
            temp_values,
            values,
            nn_entries: entries,
            nn_norms,
            latitude_deg: 0.0,
            longitude_deg: 0.0,
            sam_inv_eff: 0.0,
            sam_losses: 0.0,
            sam_array_type: None,
            nn_warned: AtomicBool::new(false),
            #[cfg(feature = "observe")]
            lut_lookup_count: AtomicU64::new(0),
        }
    }
}

fn find_column_f64<'a>(
    batch: &'a arrow::record_batch::RecordBatch,
    names: &[&str],
) -> Result<&'a arrow::array::Float64Array, HaresError> {
    for name in names {
        if let Ok(idx) = batch.schema().index_of(name) {
            let array = batch.column(idx);
            if let Some(casted) = array.as_any().downcast_ref::<arrow::array::Float64Array>() {
                return Ok(casted);
            }
            return Err(HaresError::Equipment(format!(
                "PV SAM LUT column '{}' is not Float64",
                name
            )));
        }
    }

    Err(HaresError::Equipment(format!(
        "PV SAM LUT is missing one of required columns: {}",
        names.join("|")
    )))
}

fn quantize_lut_axis(value: f64) -> i64 {
    (value * 1000.0).round() as i64
}

fn get_valid_f64(values: &arrow::array::Float64Array, idx: usize) -> Option<f64> {
    if values.is_null(idx) {
        return None;
    }
    let value = values.value(idx);
    if value.is_finite() { Some(value) } else { None }
}

fn lut_axis_to_values(axis: BTreeSet<i64>) -> Vec<f64> {
    axis.into_iter().map(|v| (v as f64) / 1000.0).collect()
}

fn index_map(values: &[f64]) -> HashMap<i64, usize> {
    values
        .iter()
        .enumerate()
        .map(|(idx, &value)| (quantize_lut_axis(value), idx))
        .collect()
}

pub(super) fn axis_bracket(axis: &[f64], value: f64) -> (usize, usize, f64) {
    if axis.len() == 1 {
        return (0, 0, 0.0);
    }
    if value <= axis[0] {
        return (0, 0, 0.0);
    }
    let last = axis.len() - 1;
    if value >= axis[last] {
        return (last, last, 0.0);
    }

    // Binary search for the containing interval.
    // partition_point returns the first index where axis[i] > value.
    // Since value > axis[0] and value < axis[last], pos ∈ [1, n-1].
    // Why: no separate O(log N) algorithmic-complexity test — the property
    // is guaranteed by std::slice::partition_point, which the stdlib doc
    // specifies as O(log n) (per partition_point docs: "This method is well
    // suited for binary search on a slice").
    let pos = axis.partition_point(|&v| v <= value);
    let lo = pos - 1;
    let hi = pos;
    let span = axis[hi] - axis[lo];
    if span.abs() < f64::EPSILON {
        return (lo, lo, 0.0);
    }
    let t = (value - axis[lo]) / span;
    (lo, hi, t.clamp(0.0, 1.0))
}

pub(super) fn bracket_corners(bracket: (usize, usize, f64)) -> [(usize, f64); 2] {
    let (lo, hi, t) = bracket;
    if lo == hi {
        [(lo, 1.0), (hi, 0.0)]
    } else {
        [(lo, 1.0 - t), (hi, t)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify `axis_bracket` returns correct index and fraction for a value
    /// exactly on the third axis point.
    #[test]
    fn axis_bracket_exact_on_point() {
        let axis = vec![0.0, 1.0, 3.0, 6.0, 10.0];
        let (lo, hi, t) = axis_bracket(&axis, 3.0);
        assert_eq!(lo, 2);
        assert_eq!(hi, 3);
        assert_eq!(t, 0.0);
    }

    /// Verify interpolation between two interior points.
    #[test]
    fn axis_bracket_between_points() {
        let axis = vec![0.0, 2.0, 10.0];
        let (lo, hi, t) = axis_bracket(&axis, 6.0);
        assert_eq!(lo, 1);
        assert_eq!(hi, 2);
        assert!((t - 0.5).abs() < 1e-12);
    }

    /// Verify clamping below the axis minimum.
    #[test]
    fn axis_bracket_below_min_clamps() {
        let axis = vec![0.0, 1.0, 2.0];
        let (lo, hi, t) = axis_bracket(&axis, -5.0);
        assert_eq!(lo, 0);
        assert_eq!(hi, 0);
        assert_eq!(t, 0.0);
    }

    /// Verify clamping above the axis maximum.
    #[test]
    fn axis_bracket_above_max_clamps() {
        let axis = vec![0.0, 1.0, 2.0];
        let (lo, hi, t) = axis_bracket(&axis, 42.0);
        assert_eq!(lo, 2);
        assert_eq!(hi, 2);
        assert_eq!(t, 0.0);
    }

    /// Exact match at axis[0] clamps to the first point.
    #[test]
    fn axis_bracket_value_equals_first_point() {
        let axis = vec![0.0, 1.0, 2.0];
        let (lo, hi, t) = axis_bracket(&axis, 0.0);
        assert_eq!(lo, 0);
        assert_eq!(hi, 0);
        assert_eq!(t, 0.0);
    }

    /// Exact match at axis[last] clamps to the last point.
    #[test]
    fn axis_bracket_value_equals_last_point() {
        let axis = vec![0.0, 1.0, 2.0];
        let (lo, hi, t) = axis_bracket(&axis, 2.0);
        assert_eq!(lo, 2);
        assert_eq!(hi, 2);
        assert_eq!(t, 0.0);
    }

    /// Single-element axis always returns the single point.
    #[test]
    fn axis_bracket_single_element_axis() {
        let axis = vec![5.0];
        let (lo, hi, t) = axis_bracket(&axis, 5.0);
        assert_eq!(lo, 0);
        assert_eq!(hi, 0);
        assert_eq!(t, 0.0);
    }

    /// Single-element axis with out-of-bounds value — method returns
    /// the single point (callers pre-clamp before reaching axis_bracket).
    #[test]
    fn axis_bracket_single_element_oob() {
        let axis = vec![5.0];
        let (lo, hi, t) = axis_bracket(&axis, -99.0);
        assert_eq!(lo, 0);
        assert_eq!(hi, 0);
        assert_eq!(t, 0.0);
    }

    /// Zero-width span behaviour — the binary search and old linear scan
    /// may split an exact-axis-point match into different adjacent
    /// intervals (e.g. (0,1,1.0) vs (1,2,0.0)), but the weighted
    /// interpolation result is identical.
    #[test]
    fn axis_bracket_zero_width_span() {
        let axis = vec![0.0, 1.0, 1.0, 2.0];
        let (lo, hi, t) = axis_bracket(&axis, 0.5);
        assert_eq!(lo, 0);
        assert_eq!(hi, 1);
        assert!((t - 0.5).abs() < 1e-12);
        // Query at the zero-width span itself: partition_point finds that
        // axis[1] <= 1.0 is true, axis[2] <= 1.0 is true, pos=3, lo=1 (axis[1]=1.0).
        // These two points differ, so span > 0 and we interpolate normally
        // (t = 0.0 means 100% weight on the common value 1.0).
        let (lo, hi, t) = axis_bracket(&axis, 1.0);
        // value == axis[last] = 2.0? No, value is 1.0, axis[last] = 2.0.
        // value > axis[0] = 0.0, value < 2.0.
        // partition_point: pos for v <= 1.0: axis[0]=0.0<=1, axis[1]=1.0<=1, axis[2]=1.0<=1, axis[3]=2.0>1 → pos=3.
        // lo=2, hi=3, span=2.0-1.0=1.0, t=0.0.
        // bracket_corners: (2, 3, 0.0) → [(2, 1.0), (3, 0.0)] — 100% on index 2 (=1.0).
        // The old linear scan would have matched idx=1: lo=1.0, hi=1.0, span=0 → (1, 1, 0.0).
        // bracket_corners: (1, 1, 0.0) → [(1, 1.0), (1, 0.0)] — 100% on index 1 (=1.0).
        // Same end result: 100% weight on 1.0.
        assert_eq!(lo, 2);
        assert_eq!(hi, 3);
        assert_eq!(t, 0.0);
    }

    /// Generate the old linear-scan version for comparison.
    fn old_axis_bracket(axis: &[f64], value: f64) -> (usize, usize, f64) {
        if axis.len() == 1 {
            return (0, 0, 0.0);
        }
        if value <= axis[0] {
            return (0, 0, 0.0);
        }
        let last = axis.len() - 1;
        if value >= axis[last] {
            return (last, last, 0.0);
        }

        for idx in 0..last {
            let lo = axis[idx];
            let hi = axis[idx + 1];
            if value >= lo && value <= hi {
                let span = hi - lo;
                if span.abs() < f64::EPSILON {
                    return (idx, idx, 0.0);
                }
                let t = (value - lo) / span;
                return (idx, idx + 1, t.clamp(0.0, 1.0));
            }
        }

        (last, last, 0.0)
    }

    /// Verify the binary-search implementation produces the same bracket results
    /// as the old linear scan for a wide range of inputs.  The two implementations
    /// may split an exact-axis-point match into different adjacent intervals
    /// (e.g. (0, 1, 1.0) vs (1, 2, 0.0)), but the weighted-interpolation result
    /// is identical because the 100%-weight lands on the same axis index.
    #[test]
    fn axis_bracket_parity_with_old_linear_scan() {
        let axes: [Vec<f64>; 6] = [
            vec![0.0, 15.0, 30.0, 45.0, 60.0, 75.0, 90.0],
            vec![0.0, 45.0, 90.0, 135.0, 180.0, 225.0, 270.0, 315.0, 360.0],
            vec![0.0, 200.0, 400.0, 600.0, 800.0, 1000.0, 1200.0],
            vec![0.0, 200.0, 400.0, 600.0, 800.0, 1000.0],
            vec![0.0, 100.0, 200.0, 300.0, 400.0, 500.0],
            vec![-20.0, -10.0, 0.0, 10.0, 20.0, 30.0, 40.0, 50.0],
        ];

        // Probe each axis with values spanning and exceeding the axis range.
        for axis in &axes {
            let min = axis.first().unwrap();
            let max = axis.last().unwrap();
            let test_values = vec![
                min - 100.0,
                min - 1.0,
                *min,
                *min + 1e-9,
                (min + axis[1]) / 2.0,
                axis[1],
                axis[axis.len() / 2],
                axis[axis.len() / 2] + 1e-9,
                axis[axis.len() - 2],
                (axis[axis.len() - 2] + max) / 2.0,
                *max - 1e-9,
                *max,
                max + 1.0,
                max + 100.0,
            ];
            // Build a synthetic values array: values[i] = axis[i] * 2.0.
            let values: Vec<f64> = axis.iter().map(|&x| x * 2.0).collect();
            for &v in &test_values {
                // 1D interpolation result from old linear scan.
                let old_br = old_axis_bracket(axis, v);
                let old_corners = bracket_corners(old_br);
                let mut old_result = 0.0;
                for (idx, w) in old_corners {
                    old_result += w * values[idx];
                }
                // 1D interpolation result from new binary search.
                let new_br = axis_bracket(axis, v);
                let new_corners = bracket_corners(new_br);
                let mut new_result = 0.0;
                for (idx, w) in new_corners {
                    new_result += w * values[idx];
                }
                assert!(
                    (new_result - old_result).abs() < 1e-12,
                    "axis={axis:?} value={v}: old_result={old_result} new_result={new_result} \
                     old_bracket={old_br:?} new_bracket={new_br:?}",
                );
            }
        }
    }

    /// Regression test: PV power for a known LUT configuration must be
    /// unchanged after the axis_bracket refactor.
    #[test]
    fn pv_lut_power_unchanged_after_bracket_refactor() {
        // Build a minimal 6-D LUT grid.
        let zenith = vec![0.0, 30.0, 60.0, 90.0];
        let azimuth = vec![0.0, 90.0, 180.0, 270.0, 360.0];
        let ghi_vals = vec![0.0, 500.0, 1000.0];
        let dni_vals = vec![0.0, 500.0, 1000.0];
        let dhi_vals = vec![0.0, 300.0, 600.0];
        let temp_vals = vec![-10.0, 0.0, 10.0, 25.0, 40.0];

        // Fill the LUT with deterministic AC power values.
        let mut entries = Vec::new();
        for (zi, &_z) in zenith.iter().enumerate() {
            for (ai, &_a) in azimuth.iter().enumerate() {
                for (gi, &g) in ghi_vals.iter().enumerate() {
                    for (di, &d) in dni_vals.iter().enumerate() {
                        for (dhi, &dh) in dhi_vals.iter().enumerate() {
                            for (ti, &t) in temp_vals.iter().enumerate() {
                                let ac: f64 = g * 0.5 + d * 0.3 + dh * 0.2
                                    - (t - 25.0_f64).max(0.0) * 0.005 * g;
                                entries.push(([zi, ai, gi, di, dhi, ti], ac.max(0.0)));
                            }
                        }
                    }
                }
            }
        }

        let lut = PvLut::from_raw(
            zenith.clone(),
            azimuth.clone(),
            ghi_vals.clone(),
            dni_vals.clone(),
            dhi_vals.clone(),
            temp_vals.clone(),
            entries,
        );

        // Query at a representative operating point.
        let (power_kw, method) = lut.interpolate(45.0, 135.0, 750.0, 600.0, 250.0, 15.0);
        assert_eq!(method, InterpolationMethod::Multilinear);
        assert!(
            power_kw > 0.0,
            "PV power should be positive at this irradiance"
        );
        assert!(
            (power_kw - 605.0).abs() < 0.01,
            "PV power changed after bracket refactor: got {power_kw}",
        );
    }
}
