use std::path::Path;

use arrow::array::{Array, Float64Array};
use hares_types::HaresError;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChargingCurveLut {
    pub soc_points: Vec<f64>,
    pub power_fraction_points: Vec<f64>,
}

impl ChargingCurveLut {
    /// Validated constructor for programmatic LUT injection.
    ///
    /// Requires: same length, non-empty, soc strictly increasing,
    /// soc in \[0, 1\], power_fraction in \[0, 1\], all finite.
    pub fn new(soc_points: Vec<f64>, power_fraction_points: Vec<f64>) -> crate::Result<Self> {
        if soc_points.len() != power_fraction_points.len() {
            return Err(HaresError::Equipment(format!(
                "ChargingCurveLut soc_points and power_fraction_points must have same length, got {} and {}",
                soc_points.len(),
                power_fraction_points.len()
            )));
        }
        if soc_points.is_empty() {
            return Err(HaresError::Equipment(
                "ChargingCurveLut cannot be empty".to_string(),
            ));
        }
        for (i, soc) in soc_points.iter().enumerate() {
            if !soc.is_finite() {
                return Err(HaresError::Equipment(format!(
                    "ChargingCurveLut soc_points[{i}] must be finite, got {soc}"
                )));
            }
            if *soc < 0.0 || *soc > 1.0 {
                return Err(HaresError::Equipment(format!(
                    "ChargingCurveLut soc_points[{i}] must be in [0, 1], got {soc}"
                )));
            }
            if i > 0 && *soc <= soc_points[i - 1] {
                return Err(HaresError::Equipment(format!(
                    "ChargingCurveLut soc_points must be strictly increasing, got {:?}",
                    soc_points
                )));
            }
        }
        for (i, frac) in power_fraction_points.iter().enumerate() {
            if !frac.is_finite() {
                return Err(HaresError::Equipment(format!(
                    "ChargingCurveLut power_fraction_points[{i}] must be finite, got {frac}"
                )));
            }
            if *frac < 0.0 || *frac > 1.0 {
                return Err(HaresError::Equipment(format!(
                    "ChargingCurveLut power_fraction_points[{i}] must be in [0, 1], got {frac}"
                )));
            }
        }
        Ok(Self {
            soc_points,
            power_fraction_points,
        })
    }

    pub fn power_fraction_at(&self, soc: f64) -> f64 {
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

pub fn parse_pybamm_lut_csv(path: &Path) -> crate::Result<ChargingCurveLut> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_valid_lut() {
        let lut = ChargingCurveLut::new(vec![0.0, 0.5, 1.0], vec![1.0, 0.8, 0.1]).unwrap();
        assert_eq!(lut.soc_points.len(), 3);
    }

    #[test]
    fn new_rejects_mismatched_lengths() {
        let err = ChargingCurveLut::new(vec![0.0, 0.5], vec![1.0]).unwrap_err();
        assert!(err.to_string().contains("same length"));
    }

    #[test]
    fn new_rejects_empty() {
        let err = ChargingCurveLut::new(vec![], vec![]).unwrap_err();
        assert!(err.to_string().contains("cannot be empty"));
    }

    #[test]
    fn new_rejects_non_increasing_soc() {
        let err = ChargingCurveLut::new(vec![0.5, 0.3, 1.0], vec![1.0, 0.8, 0.1]).unwrap_err();
        assert!(err.to_string().contains("strictly increasing"));
    }

    #[test]
    fn new_rejects_soc_out_of_range() {
        let err = ChargingCurveLut::new(vec![0.0, 1.5], vec![1.0, 0.5]).unwrap_err();
        assert!(err.to_string().contains("[0, 1]"));
    }

    #[test]
    fn new_rejects_power_fraction_out_of_range() {
        let err = ChargingCurveLut::new(vec![0.0, 1.0], vec![1.0, 1.5]).unwrap_err();
        assert!(err.to_string().contains("[0, 1]"));
    }

    #[test]
    fn new_rejects_nan_soc() {
        let err = ChargingCurveLut::new(vec![f64::NAN, 1.0], vec![1.0, 0.5]).unwrap_err();
        assert!(err.to_string().contains("finite"));
    }

    #[test]
    fn new_rejects_nan_power_fraction() {
        let err = ChargingCurveLut::new(vec![0.0, 1.0], vec![f64::NAN, 0.5]).unwrap_err();
        assert!(err.to_string().contains("finite"));
    }

    #[test]
    fn power_fraction_at_interpolates() {
        let lut = ChargingCurveLut::new(vec![0.0, 0.5, 1.0], vec![1.0, 0.8, 0.2]).unwrap();
        assert!((lut.power_fraction_at(0.25) - 0.9).abs() < 1e-10);
        assert!((lut.power_fraction_at(0.75) - 0.5).abs() < 1e-10);
    }

    #[test]
    fn power_fraction_at_clamps_to_bounds() {
        let lut = ChargingCurveLut::new(vec![0.0, 1.0], vec![1.0, 0.2]).unwrap();
        assert!((lut.power_fraction_at(-0.5) - 1.0).abs() < 1e-10);
        assert!((lut.power_fraction_at(1.5) - 0.2).abs() < 1e-10);
    }

    #[test]
    fn single_point_lut() {
        let lut = ChargingCurveLut::new(vec![0.5], vec![0.7]).unwrap();
        assert!((lut.power_fraction_at(0.0) - 0.7).abs() < 1e-10);
        assert!((lut.power_fraction_at(1.0) - 0.7).abs() < 1e-10);
    }
}
