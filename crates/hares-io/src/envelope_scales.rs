//! The reserved `envelope` override: the two factors that scale the
//! envelope's resolved layers for calibration. They multiply the values
//! the envelope LUT row (or the input's own material layers) resolved to,
//! after that selection: the resistance scale never re-selects a row, so
//! the fit surface in R is continuous, and capacitance moves only through
//! the thermal-mass scale.

use hares_types::HaresError;

/// The reserved override-map key the scales deserialise from.
pub const ENVELOPE_OVERRIDE_KEY: &str = "envelope";

/// The scales every opaque boundary's construction layers multiply by:
/// `opaque_r_scale` on every layer's resistance, `thermal_mass_scale` on
/// every such layer's capacitance. The films are unscaled and the row
/// selection reads the declared R unscaled, so no scale ever changes the
/// selected row. Slab boundaries (the ASHRAE F-factor perimeter method)
/// are outside both scales: their single resistor is the perimeter
/// method's, not a construction layer's.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EnvelopeScales {
    /// Every opaque LUT or material layer's resistance multiplies by this,
    /// films unscaled; the row selection reads the declared R unscaled, so
    /// the scale never re-selects a row. Slab boundaries (the F-factor
    /// perimeter method) are outside the scale. Default 1.0, bounds
    /// (0, 10].
    pub opaque_r_scale: f64,
    /// Every opaque LUT or material layer's capacitance multiplies by
    /// this; the resistance never moves with it. Slab boundaries are
    /// outside the scale. Default 1.0, bounds (0, 10].
    pub thermal_mass_scale: f64,
}

impl Default for EnvelopeScales {
    fn default() -> Self {
        Self {
            opaque_r_scale: 1.0,
            thermal_mass_scale: 1.0,
        }
    }
}

impl EnvelopeScales {
    /// The one check every surface runs: each field finite and inside its
    /// bounds (0, 10]. The error names the field, its value and the
    /// bounds.
    ///
    /// # Errors
    ///
    /// [`HaresError::Dwelling`] naming the field, its value and the bounds
    /// (0, 10], one error per call (the first offending field).
    pub fn validate(&self) -> Result<(), HaresError> {
        for (value, name) in [
            (self.opaque_r_scale, "opaque_r_scale"),
            (self.thermal_mass_scale, "thermal_mass_scale"),
        ] {
            if !value.is_finite() || value <= 0.0 || value > 10.0 {
                return Err(HaresError::Dwelling(format!(
                    "the 'envelope' override's {name} must be finite and within \
                     (0, 10], got {value}"
                )));
            }
        }
        Ok(())
    }
}

/// The scales as the input schema publishes them: the two fields with
/// their default 1.0 and bounds (0, 10], described beside the definition
/// so the schema cannot drift from the type (the input-schema row's owed
/// fold addition).
impl hares_types::DescribeFields for EnvelopeScales {
    fn fields() -> Vec<hares_types::FieldSchema> {
        use hares_types::{Bounds, FieldMeta, describe};
        let bounds = Bounds {
            min: Some(0.0),
            min_exclusive: true,
            max: Some(10.0),
            max_exclusive: false,
        };
        describe(
            &EnvelopeScales::default(),
            &[
                (
                    "opaque_r_scale",
                    FieldMeta {
                        bounds: Some(bounds),
                        description: Some(
                            "every opaque construction layer's resistance multiplies by \
                             this, films unscaled; the LUT row is selected on the \
                             declared R unscaled, so the scale never re-selects a row. \
                             Slab boundaries (the F-factor perimeter method) and the \
                             fenestration are outside the scale",
                        ),
                        ..Default::default()
                    },
                ),
                (
                    "thermal_mass_scale",
                    FieldMeta {
                        bounds: Some(bounds),
                        description: Some(
                            "every opaque construction layer's capacitance multiplies \
                             by this; the resistance never moves with it. Slab \
                             boundaries and the fenestration are outside the scale",
                        ),
                        ..Default::default()
                    },
                ),
            ],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_unit() {
        let scales = EnvelopeScales::default();
        assert_eq!(scales.opaque_r_scale, 1.0);
        assert_eq!(scales.thermal_mass_scale, 1.0);
        scales.validate().expect("the defaults validate");
    }

    #[test]
    fn bounds_are_half_open_zero_exclusive() {
        for (value, field) in [
            (0.0, "opaque_r_scale"),
            (-1.0, "opaque_r_scale"),
            (10.5, "opaque_r_scale"),
            (f64::NAN, "opaque_r_scale"),
            (f64::INFINITY, "opaque_r_scale"),
            (0.0, "thermal_mass_scale"),
            (10.5, "thermal_mass_scale"),
        ] {
            let scales = EnvelopeScales {
                opaque_r_scale: if field == "opaque_r_scale" {
                    value
                } else {
                    1.0
                },
                thermal_mass_scale: if field == "thermal_mass_scale" {
                    value
                } else {
                    1.0
                },
            };
            let err = scales.validate().expect_err("out of bounds");
            assert!(
                err.to_string().contains(field)
                    && err.to_string().contains("(0, 10]")
                    && err.to_string().contains(&format!("{value}")),
                "the error names the field, the value and the bounds: {err}"
            );
        }
        for value in [0.5, 1.0, 10.0] {
            let scales = EnvelopeScales {
                opaque_r_scale: value,
                thermal_mass_scale: value,
            };
            scales
                .validate()
                .expect("the bounds are inclusive at 10 and exclusive at 0");
        }
    }

    #[test]
    fn deserialises_with_defaults_and_rejects_unknown_fields() {
        let scales: EnvelopeScales = serde_json::from_value(serde_json::json!({}))
            .expect("an empty object takes the defaults");
        assert_eq!(scales, EnvelopeScales::default());
        let scales: EnvelopeScales = serde_json::from_value(serde_json::json!({
            "opaque_r_scale": 1.25,
            "thermal_mass_scale": 0.8,
        }))
        .expect("the two fields deserialise");
        assert_eq!(scales.opaque_r_scale, 1.25);
        assert_eq!(scales.thermal_mass_scale, 0.8);
        let err = serde_json::from_value::<EnvelopeScales>(serde_json::json!({
            "opaque_r_scales": 2.0,
        }))
        .expect_err("an unknown field is rejected");
        assert!(
            err.to_string().contains("opaque_r_scales"),
            "the error names the unknown field: {err}"
        );
    }
}
