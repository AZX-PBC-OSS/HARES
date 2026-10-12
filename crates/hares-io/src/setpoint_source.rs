//! Where a thermostat's setpoints came from, recorded where they resolve.
//!
//! Each HVAC side's setpoints resolve by the OpenStudio-HPXML rule
//! (`HPXMLtoOpenStudio/resources/hvac.rb` `apply_setpoints`, master
//! `57bd1b88`, lines 1641-1741): per side, a schedule-file column when
//! present ("permit mixing detailed schedules with simple schedules",
//! hvac.rb:1665), else the HPXML setpoints (`get_hvac_setpoints`,
//! hvac.rb:1768-1829: the hourly weekday and weekend extension arrays when
//! both are declared, else the constant `SetpointTemp*Season` element with
//! its setback, hvac.rb:1785-1798), else the default profile (OS-HPXML's
//! manual-thermostat defaults, 68 F heating and 78 F cooling,
//! `defaults.rb:2827-2848` `apply_hvac_control`). The resolved source per
//! side is what [`crate::schedule_resolve`] stamps on the equipment spec;
//! the run's provenance reads it back from there.

use serde::{Deserialize, Serialize};

/// The active source of one side's thermostat setpoints: the resolution
/// tier the OS-HPXML rule picked.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum SetpointSource {
    /// A schedule file's setpoint column
    /// (`DwellingConfig.schedule_path` is `Some`): the column the values
    /// are read from, one value per timestep. OS-HPXML uses the column
    /// when present and warns when HPXML setpoints are also given
    /// (hvac.rb:1669-1675); the values read as degrees Fahrenheit in
    /// OS-HPXML's detailed-schedule convention.
    ScheduleColumn {
        /// The schedule column the setpoints are read from.
        column: String,
    },
    /// The HPXML's own thermostat setpoints: 24 hourly values per day
    /// type, in Celsius, from the `HVACControl` (the hourly weekday and
    /// weekend extension arrays, or the constant `SetpointTemp*Season`
    /// element expanded with its setback).
    Hpxml {
        /// The 24 weekday hourly values, Celsius.
        weekday_c: Vec<f64>,
        /// The 24 weekend hourly values, Celsius.
        weekend_c: Vec<f64>,
    },
    /// The default profile: OS-HPXML's manual-thermostat defaults
    /// (68 F heating, 78 F cooling, `defaults.rb:2827-2848`
    /// `apply_hvac_control`), applied when neither a schedule column nor
    /// the HPXML declares setpoints for the side.
    DefaultProfile {
        /// Which default profile was applied.
        profile: String,
    },
}

impl SetpointSource {
    /// The default profile's name for the OS-HPXML manual-thermostat
    /// defaults (`defaults.rb:2827-2848`).
    pub const MANUAL_THERMOSTAT: &str = "manual_thermostat";

    /// The provenance label: where this side's setpoints came from, in
    /// the form the run's provenance and telemetry record.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::ScheduleColumn { column } => {
                format!("schedule column '{column}'")
            }
            Self::Hpxml { .. } => "HPXML HVACControl setpoints".to_string(),
            Self::DefaultProfile { profile } => {
                format!("default profile '{profile}'")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SetpointSource;

    #[test]
    fn setpoint_source_round_trips_through_serde_with_its_tier_named() {
        let sources = [
            SetpointSource::ScheduleColumn {
                column: "heating_setpoint".to_string(),
            },
            SetpointSource::Hpxml {
                weekday_c: vec![20.0; 24],
                weekend_c: vec![20.5; 24],
            },
            SetpointSource::DefaultProfile {
                profile: SetpointSource::MANUAL_THERMOSTAT.to_string(),
            },
        ];
        for source in &sources {
            let json = serde_json::to_value(source).expect("the source serializes");
            assert!(
                json.get("source").is_some(),
                "the tagged form names the tier: {json}"
            );
            let round = serde_json::from_value::<SetpointSource>(json)
                .expect("the tagged form round-trips");
            assert_eq!(&round, source);
        }
        let column = serde_json::to_value(&sources[0]).expect("serializes");
        assert_eq!(
            column["source"], "schedule_column",
            "the tier's serde spelling is the snake-case variant name"
        );
    }
}
