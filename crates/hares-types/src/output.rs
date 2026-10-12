//! The typed output-variable model: units, kinds, aggregations, sampling
//! and the `TelemetryField` declaration every observable makes about itself.
//!
//! The catalogue (`hares-io`'s `output::catalogue`) is built from these
//! declarations, so a variable the engine produces but nothing declares
//! cannot reach an output column: the catalogue is derived, never
//! hand-listed.

use std::borrow::Cow;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// One distinct unit in use across the engine's output surfaces: the union
/// of the telemetry declarations' `unit` strings and the parenthesised
/// units in the output column names, with spellings of one unit merged.
///
/// `Unit::symbol()` returns the spelling today's column names use. The
/// fleet's unit-keyed aggregation rules key on that symbol string until
/// they fold by the declared kinds instead, so `"enum"` stays distinct from
/// `"code"` (the fleet averages `enum` columns and sums `code` ones) even
/// though both decode as `VariableKind::Enum`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Unit {
    /// The empty unit: a value with no unit at all.
    None,
    /// Dimensionless, written `-` (SOC, power factor, runtime fraction).
    Dimensionless,
    /// Degrees Celsius; merges the `°C` spelling.
    Celsius,
    /// Watts.
    Watt,
    /// Kilowatts.
    Kilowatt,
    /// Kilowatt-hours.
    KilowattHour,
    /// Kilovolt-amperes reactive.
    VoltAmpereReactive,
    /// Therms per hour (gas power; the OCHRE-style column name is historical).
    ThermPerHour,
    /// Watts per square metre.
    WattPerSquareMeter,
    /// Seconds.
    Second,
    /// Minutes.
    Minute,
    /// Degrees (the wind angle's `deg`).
    Degree,
    /// Volts.
    Volt,
    /// Amperes.
    Ampere,
    /// Ampere-hours.
    AmpereHour,
    /// Joules per kilogram-kelvin (specific heat).
    JoulePerKilogramKelvin,
    /// Kilowatt-hours per mile.
    KilowattHourPerMile,
    /// Kilograms per second.
    KilogramPerSecond,
    /// Cubic metres per second (volumetric flow).
    CubicMeterPerSecond,
    /// Kilograms per cubic metre (moist-air density).
    KilogramPerCubicMeter,
    /// Kilograms per kilogram (a humidity ratio).
    KilogramPerKilogram,
    /// Kilopascals (the outdoor pressure).
    Kilopascal,
    /// Metres per second (the wind speed).
    MeterPerSecond,
    /// Litres per day.
    LitrePerDay,
    /// Joules.
    Joule,
    /// Cubic metres.
    CubicMeter,
    /// Watts per kelvin (thermal conductance).
    WattPerKelvin,
    /// A count of things.
    Count,
    /// A count of bytes.
    Byte,
    /// Percent.
    Percent,
    /// A fraction in [0, 1]; merges the `ratio` spelling.
    Fraction,
    /// A code whose value space is a label table; merges the `index`
    /// spelling. Kind `Enum`; the fleet sums code columns until the
    /// unit-keyed rules fold by kind.
    Code,
    /// The `enum` metadata unit the fleet path averages; distinct from
    /// [`Unit::Code`] until the unit-keyed rules fold by kind and the two
    /// merge.
    Enum,
    /// A boolean 0/1 flag.
    Boolean,
}

impl Unit {
    /// Every variant, for symbol round-trips and validation messages.
    pub const ALL: &[Unit] = &[
        Self::None,
        Self::Dimensionless,
        Self::Celsius,
        Self::Watt,
        Self::Kilowatt,
        Self::KilowattHour,
        Self::VoltAmpereReactive,
        Self::ThermPerHour,
        Self::WattPerSquareMeter,
        Self::Second,
        Self::Minute,
        Self::Degree,
        Self::Volt,
        Self::Ampere,
        Self::AmpereHour,
        Self::JoulePerKilogramKelvin,
        Self::KilowattHourPerMile,
        Self::KilogramPerSecond,
        Self::CubicMeterPerSecond,
        Self::KilogramPerCubicMeter,
        Self::KilogramPerKilogram,
        Self::Kilopascal,
        Self::MeterPerSecond,
        Self::LitrePerDay,
        Self::Joule,
        Self::CubicMeter,
        Self::WattPerKelvin,
        Self::Count,
        Self::Byte,
        Self::Percent,
        Self::Fraction,
        Self::Code,
        Self::Enum,
        Self::Boolean,
    ];

    /// The variant whose [`Unit::symbol()`] is `symbol`, or `None` for any
    /// other spelling: the parse the declaring surfaces validate against.
    #[must_use]
    pub fn from_symbol(symbol: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|unit| unit.symbol() == symbol)
    }

    /// The spelling today's column names and metadata use.
    #[must_use]
    pub fn symbol(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Dimensionless => "-",
            Self::Celsius => "C",
            Self::Watt => "W",
            Self::Kilowatt => "kW",
            Self::KilowattHour => "kWh",
            Self::VoltAmpereReactive => "kVAR",
            Self::ThermPerHour => "therms/hour",
            Self::WattPerSquareMeter => "W/m2",
            Self::Second => "s",
            Self::Minute => "min",
            Self::Degree => "deg",
            Self::Volt => "V",
            Self::Ampere => "A",
            Self::AmpereHour => "Ah",
            Self::JoulePerKilogramKelvin => "J/(kg*K)",
            Self::KilowattHourPerMile => "kWh/mi",
            Self::KilogramPerSecond => "kg/s",
            Self::CubicMeterPerSecond => "m3/s",
            Self::KilogramPerCubicMeter => "kg/m3",
            Self::KilogramPerKilogram => "kg/kg",
            Self::Kilopascal => "kPa",
            Self::MeterPerSecond => "m/s",
            Self::LitrePerDay => "L/day",
            Self::Joule => "J",
            Self::CubicMeter => "m3",
            Self::WattPerKelvin => "W/K",
            Self::Count => "count",
            Self::Byte => "bytes",
            Self::Percent => "%",
            Self::Fraction => "fraction",
            Self::Code => "code",
            Self::Enum => "enum",
            Self::Boolean => "bool",
        }
    }

    /// Whether the unit measures power (`kW`, `W`, `kVAR`, `therms/hour`);
    /// every such variable is declared [`VariableKind::Power`].
    #[must_use]
    pub fn is_power(self) -> bool {
        matches!(
            self,
            Self::Kilowatt | Self::Watt | Self::VoltAmpereReactive | Self::ThermPerHour
        )
    }

    /// Whether the unit measures energy (`kWh`); a variable in an energy
    /// unit is [`VariableKind::Energy`] only when it is the energy of the
    /// step.
    #[must_use]
    pub fn is_energy(self) -> bool {
        matches!(self, Self::KilowattHour)
    }
}

/// The fleet's across-homes rule for a [`VariableKind::Quantity`], declared
/// by the variable: extensive quantities sum across homes, intensive ones
/// average, weighted in both cases. Nothing reads it before the fleet
/// folds by the declared kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AcrossHomes {
    /// Extensive: mass and volume flows, event counts, currents, charge
    /// capacities, byte counts.
    Sum,
    /// Intensive: irradiance, durations, voltages, specific heat, fuel
    /// economy, degrees.
    Mean,
}

/// The labels of an [`VariableKind::Enum`] variable: code `i` decodes to
/// `labels[i]`, from 0. Serialises as a list of strings; deserialises as
/// [`EnumLabels::Runtime`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EnumLabels {
    /// Labels of the static tables and the equipment and built-in actor
    /// declarations.
    Static(&'static [&'static str]),
    /// Labels declared at runtime (a Python actor) or read back by a
    /// catalogue deserialisation.
    Runtime(Arc<[Arc<str>]>),
}

impl Serialize for EnumLabels {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq((0..self.len()).map(|i| self.get(i).unwrap_or_default()))
    }
}

impl<'de> Deserialize<'de> for EnumLabels {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let labels: Vec<String> = Vec::deserialize(deserializer)?;
        Ok(Self::Runtime(
            labels
                .into_iter()
                .map(Arc::<str>::from)
                .collect::<Arc<[Arc<str>]>>(),
        ))
    }
}

impl EnumLabels {
    /// The label at `code`, or `None` past the table.
    #[must_use]
    pub fn get(&self, code: usize) -> Option<&str> {
        match self {
            Self::Static(labels) => labels.get(code).copied(),
            Self::Runtime(labels) => labels.get(code).map(|l| l.as_ref()),
        }
    }

    /// The number of labels; every code the variable publishes below this
    /// count has a label.
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::Static(labels) => labels.len(),
            Self::Runtime(labels) => labels.len(),
        }
    }

    /// Whether the table is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Compares the label strings, whichever variant holds them.
impl PartialEq<&str> for EnumLabels {
    #[allow(clippy::cmp_owned)]
    fn eq(&self, other: &&str) -> bool {
        self.get(0) == Some(*other)
    }
}

/// What a variable measures; decides its over-time and across-homes
/// aggregation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VariableKind {
    /// The energy of the step (a flow), never a stored amount.
    Energy,
    /// A power: every variable in `kW`, `W`, `kVAR` or `therms/hour`.
    Power,
    /// A temperature.
    Temperature,
    /// A fraction, ratio, efficiency, multiplier, COP, EIR or SHR.
    Ratio,
    /// A binary 0/1 flag.
    State,
    /// A multi-valued code; code `i` decodes to `labels[i]`.
    Enum { labels: EnumLabels },
    /// Any other physical quantity: a flow, voltage, irradiance, duration,
    /// count, a stock or a running total.
    Quantity { across_homes: AcrossHomes },
}

/// The over-time aggregation rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Aggregation {
    /// The sum of step values.
    Sum,
    /// The arithmetic mean over valid steps.
    Mean,
    /// The most frequent value in the period, ties to the lowest code.
    Mode,
    /// The value at the period's last valid step (a running total's value
    /// at period end).
    Last,
}

/// When in the step a variable's value is taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Sampling {
    /// The mean over the step's interval: the right reading for a flow.
    IntervalMean,
    /// The value at the step's start.
    InstantAtStart,
    /// The value at the step's end: the right reading for a state, stock,
    /// timer or running total.
    InstantAtEnd,
}

/// Describes one observable channel an equipment or actor produces, typed:
/// the unit, the kind, the aggregation and sampling rules, nullability.
///
/// Equipment declare through `descriptor().telemetry_fields` (the private
/// per-module `telemetry_fields()` helpers); actors through the required
/// `Actor::output_variables`. Both feed the catalogue, and an actor's
/// `Telemetry` is built and sealed from its declarations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TelemetryField {
    /// The channel's key (a telemetry key for equipment, the bare key of an
    /// actor's `actor:{name}:{key}` column).
    pub name: Cow<'static, str>,
    /// The channel's unit.
    pub unit: Unit,
    /// What the channel measures.
    pub kind: VariableKind,
    /// The over-time aggregation rule.
    pub aggregation: Aggregation,
    /// The sampling rule.
    pub sampling: Sampling,
    /// Whether the channel can be missing (null in the report).
    pub nullable: bool,
    /// What the channel measures, for the catalogue and the pinned tables.
    pub description: Cow<'static, str>,
}

/// Shorthand constructors for the declaration vocabulary: each fixes the
/// aggregation and sampling to the shape its kind takes at every declaring
/// site, so a declaration reads as its classification.
impl TelemetryField {
    /// A power channel (`kW`): `Power`, `Mean`, `IntervalMean`.
    #[must_use]
    pub const fn power_kw(name: &'static str, description: &'static str) -> Self {
        Self::power(name, Unit::Kilowatt, description)
    }

    /// A power channel (`W`): `Power`, `Mean`, `IntervalMean`.
    #[must_use]
    pub const fn power_w(name: &'static str, description: &'static str) -> Self {
        Self::power(name, Unit::Watt, description)
    }

    /// A power channel with an explicit power unit: `Power`, `Mean`,
    /// `IntervalMean`.
    #[must_use]
    pub const fn power(name: &'static str, unit: Unit, description: &'static str) -> Self {
        assert!(
            matches!(
                unit,
                Unit::Kilowatt | Unit::Watt | Unit::VoltAmpereReactive | Unit::ThermPerHour
            ),
            "a power declaration needs a power unit"
        );
        Self {
            name: Cow::Borrowed(name),
            unit,
            kind: VariableKind::Power,
            aggregation: Aggregation::Mean,
            sampling: Sampling::IntervalMean,
            nullable: true,
            description: Cow::Borrowed(description),
        }
    }

    /// A power channel (`kVAR`): `Power`, `Mean`, `IntervalMean`.
    #[must_use]
    pub const fn power_kvar(name: &'static str, description: &'static str) -> Self {
        Self::power(name, Unit::VoltAmpereReactive, description)
    }

    /// A power channel (`therms/hour`): `Power`, `Mean`, `IntervalMean`.
    #[must_use]
    pub const fn power_therms(name: &'static str, description: &'static str) -> Self {
        Self::power(name, Unit::ThermPerHour, description)
    }

    /// A temperature channel (`C`): `Temperature`, `Mean`, `InstantAtEnd`.
    #[must_use]
    pub const fn temp_c(name: &'static str, description: &'static str) -> Self {
        Self {
            name: Cow::Borrowed(name),
            unit: Unit::Celsius,
            kind: VariableKind::Temperature,
            aggregation: Aggregation::Mean,
            sampling: Sampling::InstantAtEnd,
            nullable: true,
            description: Cow::Borrowed(description),
        }
    }

    /// A dimensionless fraction in [0, 1]: `Ratio`, `Mean`, `InstantAtEnd`.
    #[must_use]
    pub const fn ratio(name: &'static str, description: &'static str) -> Self {
        Self::quantity_kind(
            name,
            Unit::Dimensionless,
            VariableKind::Ratio,
            Aggregation::Mean,
            Sampling::InstantAtEnd,
            description,
        )
    }

    /// A fraction carried as a percent: `Ratio`, `Mean`, `InstantAtEnd`.
    #[must_use]
    pub const fn ratio_percent(name: &'static str, description: &'static str) -> Self {
        Self::quantity_kind(
            name,
            Unit::Percent,
            VariableKind::Ratio,
            Aggregation::Mean,
            Sampling::InstantAtEnd,
            description,
        )
    }

    /// A fraction carried as the `fraction` unit: `Ratio`, `Mean`,
    /// `InstantAtEnd`.
    #[must_use]
    pub const fn ratio_fraction(name: &'static str, description: &'static str) -> Self {
        Self::quantity_kind(
            name,
            Unit::Fraction,
            VariableKind::Ratio,
            Aggregation::Mean,
            Sampling::InstantAtEnd,
            description,
        )
    }

    /// A 0/1 flag: `State`, `Mean`, `InstantAtEnd`.
    #[must_use]
    pub const fn state_flag(name: &'static str, description: &'static str) -> Self {
        Self::quantity_kind(
            name,
            Unit::Dimensionless,
            VariableKind::State,
            Aggregation::Mean,
            Sampling::InstantAtEnd,
            description,
        )
    }

    /// A boolean 0/1 flag with the `bool` unit: `State`, `Mean`,
    /// `InstantAtEnd`.
    #[must_use]
    pub const fn boolean(name: &'static str, description: &'static str) -> Self {
        Self::quantity_kind(
            name,
            Unit::Boolean,
            VariableKind::State,
            Aggregation::Mean,
            Sampling::InstantAtEnd,
            description,
        )
    }

    /// A code channel: `Enum`, `Mode`, `InstantAtEnd`, with a label at
    /// every code from 0.
    #[must_use]
    pub const fn code(
        name: &'static str,
        unit: Unit,
        labels: &'static [&'static str],
        description: &'static str,
    ) -> Self {
        assert!(
            matches!(unit, Unit::Code | Unit::Enum | Unit::Dimensionless),
            "a code declaration needs the code, enum or dimensionless unit"
        );
        Self {
            name: Cow::Borrowed(name),
            unit,
            kind: VariableKind::Enum {
                labels: EnumLabels::Static(labels),
            },
            aggregation: Aggregation::Mode,
            sampling: Sampling::InstantAtEnd,
            nullable: true,
            description: Cow::Borrowed(description),
        }
    }

    /// A code channel under the `code` unit (the fleet sums code columns
    /// until the unit-keyed rules fold by kind).
    #[must_use]
    pub const fn code_code(
        name: &'static str,
        labels: &'static [&'static str],
        description: &'static str,
    ) -> Self {
        Self::code(name, Unit::Code, labels, description)
    }

    /// A code channel under the `enum` unit (the fleet averages enum
    /// columns until the unit-keyed rules fold by kind).
    #[must_use]
    pub const fn code_enum(
        name: &'static str,
        labels: &'static [&'static str],
        description: &'static str,
    ) -> Self {
        Self::code(name, Unit::Enum, labels, description)
    }

    /// A stock or level channel in `kWh`: `Quantity {{ across_homes: Sum }}`,
    /// `Mean`, `InstantAtEnd` (the plan's stock row).
    #[must_use]
    pub const fn stock_kwh(name: &'static str, description: &'static str) -> Self {
        Self::stock(name, Unit::KilowattHour, description)
    }

    /// A stock or level channel carried as a count.
    #[must_use]
    pub const fn stock_count(name: &'static str, description: &'static str) -> Self {
        Self::stock(name, Unit::Count, description)
    }

    /// A stock or level channel: `Quantity {{ across_homes: Sum }}`, `Mean`,
    /// `InstantAtEnd` (the plan's stock row).
    #[must_use]
    pub const fn stock(name: &'static str, unit: Unit, description: &'static str) -> Self {
        Self::quantity_kind(
            name,
            unit,
            VariableKind::Quantity {
                across_homes: AcrossHomes::Sum,
            },
            Aggregation::Mean,
            Sampling::InstantAtEnd,
            description,
        )
    }

    /// A configuration constant summed across homes.
    #[must_use]
    pub const fn constant_sum(name: &'static str, unit: Unit, description: &'static str) -> Self {
        Self::constant(name, unit, AcrossHomes::Sum, description)
    }

    /// A configuration constant averaged across homes (a duration or
    /// intensity).
    #[must_use]
    pub const fn constant_mean(name: &'static str, unit: Unit, description: &'static str) -> Self {
        Self::constant(name, unit, AcrossHomes::Mean, description)
    }

    /// An intensive quantity flow: `Quantity {{ across_homes: Mean }}`,
    /// `Mean`, `IntervalMean`.
    #[must_use]
    pub const fn flow_mean(name: &'static str, unit: Unit, description: &'static str) -> Self {
        Self::quantity_kind(
            name,
            unit,
            VariableKind::Quantity {
                across_homes: AcrossHomes::Mean,
            },
            Aggregation::Mean,
            Sampling::IntervalMean,
            description,
        )
    }

    /// An extensive quantity flow: `Quantity {{ across_homes: Sum }}`,
    /// `Mean`, `IntervalMean`.
    #[must_use]
    pub const fn flow_sum(name: &'static str, unit: Unit, description: &'static str) -> Self {
        Self::quantity_kind(
            name,
            unit,
            VariableKind::Quantity {
                across_homes: AcrossHomes::Sum,
            },
            Aggregation::Mean,
            Sampling::IntervalMean,
            description,
        )
    }

    /// A configuration constant: `Quantity`, `Mean`, `InstantAtEnd`, across
    /// homes by the declaring site's rule.
    #[must_use]
    pub const fn constant(
        name: &'static str,
        unit: Unit,
        across_homes: AcrossHomes,
        description: &'static str,
    ) -> Self {
        Self::quantity_kind(
            name,
            unit,
            VariableKind::Quantity { across_homes },
            Aggregation::Mean,
            Sampling::InstantAtEnd,
            description,
        )
    }

    /// A running total since init: `Quantity {{ across_homes: Sum }}`,
    /// `Last`, `InstantAtEnd`.
    #[must_use]
    pub const fn running_total(name: &'static str, unit: Unit, description: &'static str) -> Self {
        Self::quantity_kind(
            name,
            unit,
            VariableKind::Quantity {
                across_homes: AcrossHomes::Sum,
            },
            Aggregation::Last,
            Sampling::InstantAtEnd,
            description,
        )
    }

    /// A count of the step: `Quantity {{ across_homes: Sum }}`, `Sum`,
    /// `IntervalMean`.
    #[must_use]
    pub const fn step_count(name: &'static str, description: &'static str) -> Self {
        Self::quantity_kind(
            name,
            Unit::Count,
            VariableKind::Quantity {
                across_homes: AcrossHomes::Sum,
            },
            Aggregation::Sum,
            Sampling::IntervalMean,
            description,
        )
    }

    /// A timer channel (seconds): `Quantity {{ across_homes: Mean }}`,
    /// `Mean`, `InstantAtEnd`.
    #[must_use]
    pub const fn timer_s(name: &'static str, description: &'static str) -> Self {
        Self::quantity_kind(
            name,
            Unit::Second,
            VariableKind::Quantity {
                across_homes: AcrossHomes::Mean,
            },
            Aggregation::Mean,
            Sampling::InstantAtEnd,
            description,
        )
    }

    /// A timer channel (minutes): `Quantity {{ across_homes: Mean }}`,
    /// `Mean`, `InstantAtEnd`.
    #[must_use]
    pub const fn timer_min(name: &'static str, description: &'static str) -> Self {
        Self::quantity_kind(
            name,
            Unit::Minute,
            VariableKind::Quantity {
                across_homes: AcrossHomes::Mean,
            },
            Aggregation::Mean,
            Sampling::InstantAtEnd,
            description,
        )
    }

    /// A generic quantity channel with every rule declared.
    #[must_use]
    pub const fn quantity(
        name: &'static str,
        unit: Unit,
        across_homes: AcrossHomes,
        aggregation: Aggregation,
        sampling: Sampling,
        description: &'static str,
    ) -> Self {
        Self::quantity_kind(
            name,
            unit,
            VariableKind::Quantity { across_homes },
            aggregation,
            sampling,
            description,
        )
    }

    /// A channel whose every rule is declared verbatim.
    #[must_use]
    pub const fn full(
        name: &'static str,
        unit: Unit,
        kind: VariableKind,
        aggregation: Aggregation,
        sampling: Sampling,
        description: &'static str,
    ) -> Self {
        Self::quantity_kind(name, unit, kind, aggregation, sampling, description)
    }

    /// A temperature channel with a runtime-composed name or description
    /// (a tank node index): `Temperature`, `Mean`, `InstantAtEnd`.
    #[must_use]
    pub fn temp_c_owned(name: String, description: String) -> Self {
        Self {
            name: Cow::Owned(name),
            unit: Unit::Celsius,
            kind: VariableKind::Temperature,
            aggregation: Aggregation::Mean,
            sampling: Sampling::InstantAtEnd,
            nullable: true,
            description: Cow::Owned(description),
        }
    }

    /// A generic quantity channel with a runtime-composed name (a
    /// per-stage diagnostic): every rule declared.
    #[must_use]
    pub fn quantity_owned(
        name: String,
        unit: Unit,
        across_homes: AcrossHomes,
        aggregation: Aggregation,
        sampling: Sampling,
        description: String,
    ) -> Self {
        Self {
            name: Cow::Owned(name),
            unit,
            kind: VariableKind::Quantity { across_homes },
            aggregation,
            sampling,
            nullable: true,
            description: Cow::Owned(description),
        }
    }

    /// A ratio channel with a runtime-composed name: `Ratio`, `Mean`,
    /// `InstantAtEnd`.
    #[must_use]
    pub fn ratio_owned(name: String, description: String) -> Self {
        Self {
            name: Cow::Owned(name),
            unit: Unit::Dimensionless,
            kind: VariableKind::Ratio,
            aggregation: Aggregation::Mean,
            sampling: Sampling::InstantAtEnd,
            nullable: true,
            description: Cow::Owned(description),
        }
    }

    #[allow(clippy::too_many_arguments)]
    const fn quantity_kind(
        name: &'static str,
        unit: Unit,
        kind: VariableKind,
        aggregation: Aggregation,
        sampling: Sampling,
        description: &'static str,
    ) -> Self {
        Self {
            name: Cow::Borrowed(name),
            unit,
            kind,
            aggregation,
            sampling,
            nullable: true,
            description: Cow::Borrowed(description),
        }
    }
}

/// The operating-mode code table: one label per `OperatingMode` ordinal,
/// shared by every per-equipment `Mode (-)` column declaration (the
/// catalogue's enum labels are the decoding table).
pub const MODE_LABELS: &[&str] = &[
    "Off",
    "Heating",
    "Cooling",
    "Defrost",
    "Standby",
    "Charging",
    "Discharging",
    "HeatingHP",
    "HeatingER",
    "HeatingHPAndER",
    "HeatPumpWH",
    "BackupElement",
    "On",
];

/// The EV connection-state codes (`ev/telemetry.rs`): 0 to 2.
pub const EV_CONNECTION_STATE_LABELS: &[&str] =
    &["home_plugged_in", "away_plugged_in", "disconnected"];

/// The EV charging-level codes (`ev/telemetry.rs`): publishes 1 and 2, so
/// the label table from 0 declares the placeholder at 0.
pub const EV_CHARGING_LEVEL_LABELS: &[&str] = &["none", "L1", "L2"];

/// The speed-index code labels: a speed stage has no name of its own, so
/// each code is labelled by its decimal value, up to the largest stage
/// count the models declare.
pub const SPEED_INDEX_LABELS: &[&str] = &["0", "1", "2", "3", "4"];

/// Everything a declaration needs is checked here once: the vocabulary
/// constructors above pin the shapes; this asserts the invariants that
/// survive refactors (power units declare `Power`; `Energy` and `Last`'s
/// legality are the catalogue build's checks).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry_keys as tk;

    #[test]
    fn unit_symbols_match_todays_column_spellings() {
        assert_eq!(Unit::None.symbol(), "");
        assert_eq!(Unit::Dimensionless.symbol(), "-");
        assert_eq!(Unit::Celsius.symbol(), "C");
        assert_eq!(Unit::Watt.symbol(), "W");
        assert_eq!(Unit::Kilowatt.symbol(), "kW");
        assert_eq!(Unit::KilowattHour.symbol(), "kWh");
        assert_eq!(Unit::VoltAmpereReactive.symbol(), "kVAR");
        assert_eq!(Unit::ThermPerHour.symbol(), "therms/hour");
        assert_eq!(Unit::WattPerSquareMeter.symbol(), "W/m2");
        assert_eq!(Unit::Second.symbol(), "s");
        assert_eq!(Unit::Minute.symbol(), "min");
        assert_eq!(Unit::Degree.symbol(), "deg");
        assert_eq!(Unit::Volt.symbol(), "V");
        assert_eq!(Unit::Ampere.symbol(), "A");
        assert_eq!(Unit::AmpereHour.symbol(), "Ah");
        assert_eq!(Unit::JoulePerKilogramKelvin.symbol(), "J/(kg*K)");
        assert_eq!(Unit::KilowattHourPerMile.symbol(), "kWh/mi");
        assert_eq!(Unit::KilogramPerSecond.symbol(), "kg/s");
        assert_eq!(Unit::LitrePerDay.symbol(), "L/day");
        assert_eq!(Unit::Count.symbol(), "count");
        assert_eq!(Unit::Byte.symbol(), "bytes");
        assert_eq!(Unit::Percent.symbol(), "%");
        assert_eq!(Unit::Fraction.symbol(), "fraction");
        assert_eq!(Unit::Code.symbol(), "code");
        assert_eq!(Unit::Enum.symbol(), "enum");
        assert_eq!(Unit::Boolean.symbol(), "bool");
    }

    #[test]
    fn unit_from_symbol_round_trips_every_variant() {
        for unit in Unit::ALL {
            assert_eq!(Unit::from_symbol(unit.symbol()), Some(*unit));
        }
        assert_eq!(Unit::from_symbol("furlongs"), None);
        assert_eq!(Unit::from_symbol(""), Some(Unit::None));
    }

    #[test]
    fn constructors_pin_their_kind_shapes() {
        let p = TelemetryField::power_kw(tk::ELECTRIC_KW, "power");
        assert_eq!(p.unit, Unit::Kilowatt);
        assert_eq!(p.kind, VariableKind::Power);
        assert_eq!(p.aggregation, Aggregation::Mean);
        assert_eq!(p.sampling, Sampling::IntervalMean);
        assert!(p.nullable);

        let t = TelemetryField::temp_c("tank_temp_c", "temp");
        assert_eq!(t.kind, VariableKind::Temperature);
        assert_eq!(t.sampling, Sampling::InstantAtEnd);

        let f = TelemetryField::ratio("shr", "shr");
        assert_eq!(f.kind, VariableKind::Ratio);

        let s = TelemetryField::state_flag("defrost_active", "flag");
        assert_eq!(s.kind, VariableKind::State);

        let b = TelemetryField::boolean("away", "flag");
        assert_eq!(b.unit, Unit::Boolean);
        assert_eq!(b.kind, VariableKind::State);

        let e = TelemetryField::code(
            "connection_state",
            Unit::Code,
            EV_CONNECTION_STATE_LABELS,
            "codes",
        );
        assert_eq!(
            e.kind,
            VariableKind::Enum {
                labels: EnumLabels::Static(EV_CONNECTION_STATE_LABELS)
            }
        );
        assert_eq!(e.aggregation, Aggregation::Mode);

        let st = TelemetryField::stock(tk::EBM_ENERGY_KWH, Unit::KilowattHour, "stock");
        assert_eq!(
            st.kind,
            VariableKind::Quantity {
                across_homes: AcrossHomes::Sum
            }
        );
        assert_eq!(st.aggregation, Aggregation::Mean);

        let rt = TelemetryField::running_total(tk::CYCLE_COUNT, Unit::Count, "total");
        assert_eq!(rt.aggregation, Aggregation::Last);

        let sc = TelemetryField::step_count("signals_count", "count");
        assert_eq!(sc.aggregation, Aggregation::Sum);
        assert_eq!(sc.sampling, Sampling::IntervalMean);

        let tm = TelemetryField::timer_s(tk::MODE_DURATION_S, "timer");
        assert_eq!(
            tm.kind,
            VariableKind::Quantity {
                across_homes: AcrossHomes::Mean
            }
        );
    }

    #[test]
    fn enum_labels_round_trip_and_compare_by_string() {
        let static_labels = EnumLabels::Static(EV_CHARGING_LEVEL_LABELS);
        assert_eq!(static_labels.get(0), Some("none"));
        assert_eq!(static_labels.get(2), Some("L2"));
        assert_eq!(static_labels.get(3), None);
        assert_eq!(static_labels.len(), 3);

        let runtime: Arc<[Arc<str>]> = vec![Arc::from("a"), Arc::from("b")].into();
        let runtime_labels = EnumLabels::Runtime(runtime);
        assert_eq!(runtime_labels.get(1), Some("b"));
        assert_eq!(static_labels, EnumLabels::Static(EV_CHARGING_LEVEL_LABELS));
        assert_ne!(static_labels, runtime_labels);
    }

    #[test]
    fn telemetry_field_serialises_compactly() {
        let field = TelemetryField::power_kw("electric_kw", "power");
        let json = serde_json::to_string(&field).expect("serialise");
        let decoded: TelemetryField = serde_json::from_str(&json).expect("decode");
        assert_eq!(decoded, field);
    }
}
