use std::path::Path;

use arrow::array::{Array, Float64Array};
use hares_types::HaresError;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ChargingCurveLut {
    pub(super) soc_points: Vec<f64>,
    pub(super) power_fraction_points: Vec<f64>,
}

impl ChargingCurveLut {
    pub(super) fn power_fraction_at(&self, soc: f64) -> f64 {
        if self.soc_points.is_empty() {
            return 1.0;
        }
        let soc = soc.clamp(self.soc_points[0], *self.soc_points.last().unwrap_or(&1.0));

        if self.soc_points.len() == 1 {
            return self.power_fraction_points[0].clamp(0.0, 1.0);
        }

        for i in 0..(self.soc_points.len() - 1) {
            let x0 = self.soc_points[i];
            let x1 = self.soc_points[i + 1];
            if soc <= x1 {
                let y0 = self.power_fraction_points[i];
                let y1 = self.power_fraction_points[i + 1];
                let dx = x1 - x0;
                if dx.abs() <= f64::EPSILON {
                    return y0.clamp(0.0, 1.0);
                }
                let t = (soc - x0) / dx;
                return (y0 + t * (y1 - y0)).clamp(0.0, 1.0);
            }
        }

        self.power_fraction_points
            .last()
            .copied()
            .unwrap_or(1.0)
            .clamp(0.0, 1.0)
    }
}

pub(super) fn parse_pybamm_lut_csv(path: &Path) -> crate::Result<ChargingCurveLut> {
    if path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("parquet"))
    {
        return parse_pybamm_lut_parquet(path);
    }

    let text = std::fs::read_to_string(path).map_err(|e| {
        HaresError::Equipment(format!(
            "failed to read EV PyBaMM LUT '{}': {e}",
            path.display()
        ))
    })?;
    let mut lines = text.lines();
    let Some(header) = lines.next() else {
        return Err(HaresError::Equipment(format!(
            "EV PyBaMM LUT '{}' is empty",
            path.display()
        )));
    };
    let headers: Vec<_> = header.split(',').map(str::trim).collect();
    let soc_idx = headers.iter().position(|h| *h == "soc").ok_or_else(|| {
        HaresError::Equipment(format!(
            "EV PyBaMM LUT '{}' missing 'soc' column",
            path.display()
        ))
    })?;
    let frac_idx = headers
        .iter()
        .position(|h| *h == "power_fraction" || *h == "relative_power")
        .ok_or_else(|| {
            HaresError::Equipment(format!(
                "EV PyBaMM LUT '{}' missing 'power_fraction' column",
                path.display()
            ))
        })?;

    let mut soc_points = Vec::new();
    let mut power_fraction_points = Vec::new();
    for line in lines {
        let cols: Vec<_> = line.split(',').map(str::trim).collect();
        if cols.len() <= soc_idx || cols.len() <= frac_idx {
            continue;
        }
        let soc = cols[soc_idx].parse::<f64>().map_err(|e| {
            HaresError::Equipment(format!(
                "EV PyBaMM LUT '{}' invalid soc '{}': {e}",
                path.display(),
                cols[soc_idx]
            ))
        })?;
        let frac = cols[frac_idx].parse::<f64>().map_err(|e| {
            HaresError::Equipment(format!(
                "EV PyBaMM LUT '{}' invalid power_fraction '{}': {e}",
                path.display(),
                cols[frac_idx]
            ))
        })?;
        if !soc.is_finite() || !frac.is_finite() {
            continue;
        }
        soc_points.push(soc.clamp(0.0, 1.0));
        power_fraction_points.push(frac.clamp(0.0, 1.0));
    }

    finalize_curve_lut(path, soc_points, power_fraction_points)
}

fn parse_pybamm_lut_parquet(path: &Path) -> crate::Result<ChargingCurveLut> {
    let file = std::fs::File::open(path).map_err(|e| {
        HaresError::Equipment(format!(
            "failed to open EV PyBaMM LUT parquet '{}': {e}",
            path.display()
        ))
    })?;
    let mut reader = ParquetRecordBatchReaderBuilder::try_new(file)
        .map_err(|e| {
            HaresError::Equipment(format!(
                "failed to build EV PyBaMM LUT parquet reader '{}': {e}",
                path.display()
            ))
        })?
        .build()
        .map_err(|e| {
            HaresError::Equipment(format!(
                "failed to read EV PyBaMM LUT parquet '{}': {e}",
                path.display()
            ))
        })?;

    let mut soc_points = Vec::new();
    let mut power_fraction_points = Vec::new();
    for batch_result in &mut reader {
        let batch = batch_result.map_err(|e| {
            HaresError::Equipment(format!(
                "failed reading EV PyBaMM LUT parquet batch '{}': {e}",
                path.display()
            ))
        })?;
        let soc_col = find_float64_column(&batch, &["soc"])?;
        let frac_col = find_float64_column(&batch, &["power_fraction", "relative_power"])?;
        for idx in 0..batch.num_rows() {
            if soc_col.is_null(idx) || frac_col.is_null(idx) {
                continue;
            }
            let soc = soc_col.value(idx).clamp(0.0, 1.0);
            let frac = frac_col.value(idx).clamp(0.0, 1.0);
            soc_points.push(soc);
            power_fraction_points.push(frac);
        }
    }

    finalize_curve_lut(path, soc_points, power_fraction_points)
}

fn finalize_curve_lut(
    path: &Path,
    mut soc_points: Vec<f64>,
    mut power_fraction_points: Vec<f64>,
) -> crate::Result<ChargingCurveLut> {
    if soc_points.is_empty() {
        return Err(HaresError::Equipment(format!(
            "EV PyBaMM LUT '{}' has no valid rows",
            path.display()
        )));
    }

    let mut zipped: Vec<(f64, f64)> = soc_points
        .drain(..)
        .zip(power_fraction_points.drain(..))
        .collect();
    zipped.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (soc_points, power_fraction_points): (Vec<_>, Vec<_>) = zipped.into_iter().unzip();

    Ok(ChargingCurveLut {
        soc_points,
        power_fraction_points,
    })
}

fn find_float64_column<'a>(
    batch: &'a arrow::record_batch::RecordBatch,
    names: &[&str],
) -> crate::Result<&'a Float64Array> {
    for &name in names {
        if let Some((idx, _)) = batch.schema().column_with_name(name) {
            let array = batch.column(idx);
            if let Some(values) = array.as_any().downcast_ref::<Float64Array>() {
                return Ok(values);
            }
            return Err(HaresError::Equipment(format!(
                "EV PyBaMM LUT column '{}' must be Float64",
                name
            )));
        }
    }
    Err(HaresError::Equipment(format!(
        "EV PyBaMM LUT missing required column; expected one of {names:?}"
    )))
}
