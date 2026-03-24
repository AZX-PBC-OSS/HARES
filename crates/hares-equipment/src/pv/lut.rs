//! PV SAM look-up table: parsing, storage, and interpolation.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

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

#[derive(Clone, Debug)]
pub(crate) struct PvLut {
    month_values: Vec<f64>,
    hour_values: Vec<f64>,
    ghi_values: Vec<f64>,
    dni_values: Vec<f64>,
    dhi_values: Vec<f64>,
    temp_values: Vec<f64>,
    values: HashMap<(usize, usize, usize, usize, usize, usize), f64>,
    nn_entries: Vec<([usize; 6], f64)>,
    nn_norms: [AxisNorm; 6],
}

impl PvLut {
    pub(crate) fn from_parquet(path: &Path) -> Result<Self, HaresError> {
        let file = std::fs::File::open(path).map_err(|e| {
            HaresError::Equipment(format!(
                "failed to open PV SAM LUT '{}': {e}",
                path.display()
            ))
        })?;

        let mut reader = ParquetRecordBatchReaderBuilder::try_new(file)
            .map_err(|e| {
                HaresError::Equipment(format!(
                    "failed to build Parquet reader for PV SAM LUT '{}': {e}",
                    path.display()
                ))
            })?
            .build()
            .map_err(|e| {
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

            let month = find_column_f64(&batch, &["month"])?;
            let hour = find_column_f64(&batch, &["hour"])?;
            let ghi = find_column_f64(&batch, &["ghi", "ghi_w_m2"])?;
            let dni = find_column_f64(&batch, &["dni", "dni_w_m2"])?;
            let dhi = find_column_f64(&batch, &["dhi", "dhi_w_m2"])?;
            let temp = find_column_f64(&batch, &["temp_c", "temperature_c"])?;
            let ac = find_column_f64(&batch, &["ac_power_kw", "ac_kw"])?;

            let n = batch.num_rows();
            for i in 0..n {
                let Some(month) = get_valid_f64(month, i) else {
                    continue;
                };
                let Some(hour) = get_valid_f64(hour, i) else {
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
                rows.push((month, hour, ghi, dni, dhi, temp, ac));
            }
        }

        if rows.is_empty() {
            return Err(HaresError::Equipment(format!(
                "PV SAM LUT '{}' contains no valid rows",
                path.display()
            )));
        }

        let mut month_axis = BTreeSet::new();
        let mut hour_axis = BTreeSet::new();
        let mut ghi_axis = BTreeSet::new();
        let mut dni_axis = BTreeSet::new();
        let mut dhi_axis = BTreeSet::new();
        let mut temp_axis = BTreeSet::new();

        for &(month, hour, ghi, dni, dhi, temp, _) in &rows {
            month_axis.insert(quantize_lut_axis(month));
            hour_axis.insert(quantize_lut_axis(hour));
            ghi_axis.insert(quantize_lut_axis(ghi));
            dni_axis.insert(quantize_lut_axis(dni));
            dhi_axis.insert(quantize_lut_axis(dhi));
            temp_axis.insert(quantize_lut_axis(temp));
        }

        let month_values = lut_axis_to_values(month_axis);
        let hour_values = lut_axis_to_values(hour_axis);
        let ghi_values = lut_axis_to_values(ghi_axis);
        let dni_values = lut_axis_to_values(dni_axis);
        let dhi_values = lut_axis_to_values(dhi_axis);
        let temp_values = lut_axis_to_values(temp_axis);

        let month_idx = index_map(&month_values);
        let hour_idx = index_map(&hour_values);
        let ghi_idx = index_map(&ghi_values);
        let dni_idx = index_map(&dni_values);
        let dhi_idx = index_map(&dhi_values);
        let temp_idx = index_map(&temp_values);

        let mut values = HashMap::with_capacity(rows.len());
        let mut nn_entries = Vec::with_capacity(rows.len());
        for &(month, hour, ghi, dni, dhi, temp, ac) in &rows {
            let indices = [
                *month_idx
                    .get(&quantize_lut_axis(month))
                    .expect("month index exists"),
                *hour_idx
                    .get(&quantize_lut_axis(hour))
                    .expect("hour index exists"),
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
            AxisNorm::from_values(&month_values),
            AxisNorm::from_values(&hour_values),
            AxisNorm::from_values(&ghi_values),
            AxisNorm::from_values(&dni_values),
            AxisNorm::from_values(&dhi_values),
            AxisNorm::from_values(&temp_values),
        ];

        Ok(Self {
            month_values,
            hour_values,
            ghi_values,
            dni_values,
            dhi_values,
            temp_values,
            values,
            nn_entries,
            nn_norms,
        })
    }

    pub(crate) fn interpolate(
        &self,
        month: f64,
        hour: f64,
        ghi: f64,
        dni: f64,
        dhi: f64,
        temp_c: f64,
    ) -> f64 {
        let month = month.clamp(
            *self.month_values.first().expect("month axis non-empty"),
            *self.month_values.last().expect("month axis non-empty"),
        );
        let hour = hour.clamp(
            *self.hour_values.first().expect("hour axis non-empty"),
            *self.hour_values.last().expect("hour axis non-empty"),
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

        let month_br = axis_bracket(&self.month_values, month);
        let hour_br = axis_bracket(&self.hour_values, hour);
        let ghi_br = axis_bracket(&self.ghi_values, ghi);
        let dni_br = axis_bracket(&self.dni_values, dni);
        let dhi_br = axis_bracket(&self.dhi_values, dhi);
        let temp_br = axis_bracket(&self.temp_values, temp_c);

        let mut weighted_sum = 0.0;
        let mut total_weight = 0.0;

        for (m_idx, m_w) in bracket_corners(month_br) {
            for (h_idx, h_w) in bracket_corners(hour_br) {
                for (g_idx, g_w) in bracket_corners(ghi_br) {
                    for (d_idx, d_w) in bracket_corners(dni_br) {
                        for (dh_idx, dh_w) in bracket_corners(dhi_br) {
                            for (t_idx, t_w) in bracket_corners(temp_br) {
                                let weight = m_w * h_w * g_w * d_w * dh_w * t_w;
                                if weight <= 0.0 {
                                    continue;
                                }
                                if let Some(ac_kw) = self
                                    .values
                                    .get(&(m_idx, h_idx, g_idx, d_idx, dh_idx, t_idx))
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
            return weighted_sum / total_weight;
        }

        // Sparse-grid fallback: nearest-neighbor with normalized distance.
        let axes: [&[f64]; 6] = [
            &self.month_values,
            &self.hour_values,
            &self.ghi_values,
            &self.dni_values,
            &self.dhi_values,
            &self.temp_values,
        ];
        let query = [month, hour, ghi, dni, dhi, temp_c];
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
        best_val
    }
}

#[cfg(test)]
impl PvLut {
    pub(crate) fn from_raw(
        month_values: Vec<f64>,
        hour_values: Vec<f64>,
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
            AxisNorm::from_values(&month_values),
            AxisNorm::from_values(&hour_values),
            AxisNorm::from_values(&ghi_values),
            AxisNorm::from_values(&dni_values),
            AxisNorm::from_values(&dhi_values),
            AxisNorm::from_values(&temp_values),
        ];
        Self {
            month_values,
            hour_values,
            ghi_values,
            dni_values,
            dhi_values,
            temp_values,
            values,
            nn_entries: entries,
            nn_norms,
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

pub(super) fn bracket_corners(bracket: (usize, usize, f64)) -> [(usize, f64); 2] {
    let (lo, hi, t) = bracket;
    if lo == hi {
        [(lo, 1.0), (hi, 0.0)]
    } else {
        [(lo, 1.0 - t), (hi, t)]
    }
}
