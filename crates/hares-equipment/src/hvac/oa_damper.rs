//! The outdoor-air damper's shared surface: the minimum-position damper an
//! outdoor-air economizer drives, declared as the composition seed any
//! packaged unit's assembly composes (the recirculating fan's shared
//! accounting rides its own row; this surface is the damper's).
//!
//! The damper has two operating positions — the minimum position (the
//! ventilation's minimum: the outdoor-air floor the IAQ requirement sets)
//! and full open (the design flow, the free-cooling position) — and two
//! lockouts that hold it at the minimum position with the mechanical
//! cooling running, the forms EnergyPlus' economizer control applies
//! (`MixedAir.cc` `Checksetpoints` and the `TempLowLim` gate):
//!
//! - the humid lockout: the outdoor dew point above the configured limit
//!   (dampers cannot dehumidify; full-open humid air displaces coil latent
//!   capacity), and
//! - the cold lockout: the outdoor dry bulb below the configured floor
//!   (full-open freezing air risks overcooling the zone and the coil).
//!
//! Either lockout left unconfigured is not operative, the same semantics
//! the EnergyPlus IDD gives the blank "Economizer Lower Temperature Limit"
//! and "Economizer Maximum Limit Dewpoint Temperature" fields.

use hares_types::HaresError;
use serde::{Deserialize, Serialize};

/// The outdoor-air damper's configuration: the flows at its two positions
/// and the lockouts that hold it at the minimum.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OaDamperConfig {
    /// The outdoor-air flow [m³/s] the damper passes at its minimum
    /// position: the ventilation's minimum, the IAQ floor the damper never
    /// passes below while the unit's fan runs.
    pub minimum_oa_flow_m3_s: f64,
    /// The outdoor-air flow [m³/s] at the damper's full-open position: the
    /// design flow the free-cooling position passes.
    pub design_oa_flow_m3_s: f64,
    /// The outdoor dew point [°C] above which the damper is held at the
    /// minimum position (the humid lockout). `None` means the lockout is
    /// not operative.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_oa_dew_point_c: Option<f64>,
    /// The outdoor dry-bulb [°C] below which the damper is held at the
    /// minimum position (the cold lockout). `None` means the lockout is
    /// not operative.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_oa_temp_c: Option<f64>,
}

/// The outdoor-air economizer's configuration: the dry-bulb changeover
/// rule over the damper's two positions and lockouts.
///
/// The changeover is the dry-bulb form: the damper opens for free cooling
/// when the outdoor dry-bulb is below the changeover setpoint. Default
/// 10.0 °C per ASHRAE 90.1-2022 §6.5.1.4 economizer changeover guidance,
/// the citation the cooling compressor's own minimum-OAT lockout already
/// carries. The enthalpy form (comparing total enthalpies) is the
/// alternative this declaration rejects: the design names the dry-bulb
/// form.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EconomizerConfig {
    /// The changeover setpoint [°C]: the outdoor dry-bulb below which the
    /// damper opens for free cooling. Default 10.0 per ASHRAE 90.1-2022
    /// §6.5.1.4 economizer changeover guidance.
    #[serde(default = "default_changeover_setpoint_c")]
    pub changeover_setpoint_c: f64,
    /// The outdoor-air damper the economizer drives.
    pub damper: OaDamperConfig,
}

fn default_changeover_setpoint_c() -> f64 {
    10.0
}

impl EconomizerConfig {
    /// Validates the changeover and the damper's flows: every value finite,
    /// the changeover's setpoint finite, the damper's flows non-negative,
    /// the design flow positive. The minimum's flow above the design flow is
    /// not an error — [`Self::resolved_flows`] clamps it to the design flow
    /// (the damper's minimum position is then its full open) and the clamp
    /// is counted in the run's clamp census.
    pub fn validate(&self) -> crate::Result<()> {
        if !self.changeover_setpoint_c.is_finite() {
            return Err(HaresError::Equipment(format!(
                "EconomizerConfig: changeover_setpoint_c must be finite, got {}",
                self.changeover_setpoint_c
            )));
        }
        self.damper.validate()
    }

    /// The damper's resolved flows: the minimum-position flow clamped into
    /// `[0, design_oa_flow_m3_s]` (the clamp the damper's limits define) and
    /// the design flow. Returns `(minimum_flow, design_flow)`.
    pub fn resolved_flows(&self) -> (f64, f64) {
        self.damper.resolved_flows()
    }
}

impl OaDamperConfig {
    fn validate(&self) -> crate::Result<()> {
        if !self.minimum_oa_flow_m3_s.is_finite() || self.minimum_oa_flow_m3_s < 0.0 {
            return Err(HaresError::Equipment(format!(
                "OaDamperConfig: minimum_oa_flow_m3_s must be finite and >= 0, got {}",
                self.minimum_oa_flow_m3_s
            )));
        }
        if !self.design_oa_flow_m3_s.is_finite() || self.design_oa_flow_m3_s <= 0.0 {
            return Err(HaresError::Equipment(format!(
                "OaDamperConfig: design_oa_flow_m3_s must be finite and > 0, got {}",
                self.design_oa_flow_m3_s
            )));
        }
        if let Some(v) = self.max_oa_dew_point_c
            && !v.is_finite()
        {
            return Err(HaresError::Equipment(format!(
                "OaDamperConfig: max_oa_dew_point_c must be finite, got {v}"
            )));
        }
        if let Some(v) = self.min_oa_temp_c
            && !v.is_finite()
        {
            return Err(HaresError::Equipment(format!(
                "OaDamperConfig: min_oa_temp_c must be finite, got {v}"
            )));
        }
        Ok(())
    }

    /// The damper's resolved flows: the minimum-position flow clamped into
    /// `[0, design_oa_flow_m3_s]` and the design flow. A minimum above the
    /// design clamps to the design: the damper's minimum position is then
    /// its full open, and the clamp is a counted event
    /// ([`hares_types::clamp_census::ClampSite::EconomizerDamperLimit`]) at
    /// the equipment init that applies it.
    pub fn resolved_flows(&self) -> (f64, f64) {
        let design = self.design_oa_flow_m3_s;
        let minimum = self.minimum_oa_flow_m3_s.clamp(0.0, design);
        (minimum, design)
    }

    /// The damper's minimum position as the fraction of the design flow the
    /// published damper-position telemetry reports at the minimum.
    #[must_use]
    pub fn minimum_position_fraction(&self) -> f64 {
        let (minimum, design) = self.resolved_flows();
        minimum / design
    }

    /// Whether the damper's lockouts hold it at the minimum position for the
    /// step's outdoor state: the humid lockout (the outdoor dew point above
    /// the limit) or the cold lockout (the outdoor dry bulb below the
    /// floor). An unconfigured lockout is not operative.
    #[must_use]
    pub fn locked_out(&self, outdoor_temp_c: f64, outdoor_dew_point_c: f64) -> bool {
        let humid = self
            .max_oa_dew_point_c
            .is_some_and(|limit| outdoor_dew_point_c > limit);
        let cold = self
            .min_oa_temp_c
            .is_some_and(|limit| outdoor_temp_c < limit);
        humid || cold
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn damper() -> OaDamperConfig {
        OaDamperConfig {
            minimum_oa_flow_m3_s: 0.05,
            design_oa_flow_m3_s: 0.5,
            max_oa_dew_point_c: None,
            min_oa_temp_c: None,
        }
    }

    #[test]
    fn minimum_position_fraction_is_minimum_flow_over_design_flow() {
        let d = damper();
        assert_eq!(d.minimum_position_fraction(), 0.1);
    }

    #[test]
    fn minimum_flow_above_design_clamps_to_design_flow() {
        let d = OaDamperConfig {
            minimum_oa_flow_m3_s: 0.8,
            design_oa_flow_m3_s: 0.5,
            max_oa_dew_point_c: None,
            min_oa_temp_c: None,
        };
        let (minimum, design) = d.resolved_flows();
        assert_eq!(minimum, 0.5);
        assert_eq!(design, 0.5);
        assert_eq!(d.minimum_position_fraction(), 1.0);
    }

    #[test]
    fn unconfigured_lockouts_are_never_operative() {
        let d = damper();
        assert!(!d.locked_out(-40.0, 40.0));
    }

    #[test]
    fn humid_lockout_fires_when_dew_point_exceeds_limit() {
        let d = OaDamperConfig {
            max_oa_dew_point_c: Some(13.0),
            ..damper()
        };
        assert!(d.locked_out(9.0, 13.5));
        assert!(!d.locked_out(9.0, 12.5));
    }

    #[test]
    fn cold_lockout_fires_when_dry_bulb_below_floor() {
        let d = OaDamperConfig {
            min_oa_temp_c: Some(-5.0),
            ..damper()
        };
        assert!(d.locked_out(-5.5, 0.0));
        assert!(!d.locked_out(-4.5, 0.0));
    }

    #[test]
    fn validate_rejects_non_finite_changeover_and_bad_flows() {
        let mut e = EconomizerConfig {
            changeover_setpoint_c: f64::NAN,
            damper: damper(),
        };
        assert!(e.validate().is_err());
        e.changeover_setpoint_c = 10.0;
        e.damper.minimum_oa_flow_m3_s = -0.1;
        assert!(e.validate().is_err());
        e.damper.minimum_oa_flow_m3_s = 0.05;
        e.damper.design_oa_flow_m3_s = 0.0;
        assert!(e.validate().is_err());
        e.damper.design_oa_flow_m3_s = 0.5;
        assert!(e.validate().is_ok());
    }

    #[test]
    fn serde_rejects_unknown_damper_fields() {
        let text = r#"{
            "changeover_setpoint_c": 10.0,
            "damper": {
                "minimum_oa_flow_m3_s": 0.05,
                "design_oa_flow_m3_s": 0.5,
                "nonsense": 1.0
            }
        }"#;
        let parsed: Result<EconomizerConfig, _> = serde_json::from_str(text);
        assert!(
            parsed.is_err(),
            "an unknown damper key is an error, not a no-op"
        );
    }

    #[test]
    fn changeover_setpoint_defaults_to_the_ashrae_10c_guidance() {
        let e: EconomizerConfig = serde_json::from_str(
            r#"{ "damper": { "minimum_oa_flow_m3_s": 0.05, "design_oa_flow_m3_s": 0.5 } }"#,
        )
        .expect("the changeover setpoint's default fills it");
        assert_eq!(e.changeover_setpoint_c, 10.0);
    }
}
