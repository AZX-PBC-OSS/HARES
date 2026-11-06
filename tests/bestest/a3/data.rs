//! The Normative Annex A3 data, pinned to the public addendum.
//!
//! Source: ANSI/ASHRAE/IBPSA Addendum a to ANSI/ASHRAE Standard 140-2023
//! (approved 2025-08-29), the public addendum PDF at
//! <https://www.ashrae.org/file%20library/technical%20resources/standards%20and%20guidelines/standards%20addenda/140_2023_a_20250829.pdf>,
//! sha256 cb993847ccd4a50a10a09e27cbfc199536487490952d1438ee575e52a616e892
//! (verified before transcription). The addendum revises Normative Annex
//! A3 only (plus informative Annex B12); it touches neither Section 7's
//! case definitions nor Annex B8's example results.
//!
//! Every table below is the addendum's printed table, transcribed in the
//! printed layout so a row here reads against the PDF row-for-row. The
//! unit conversions: Tables A3-1 and A3-4 print MWh/yr and the data is
//! carried in kWh (x1000); Tables A3-2 and A3-5 print kWh per hour and
//! the data is carried as printed. The addendum's minus signs are
//! typographic dashes in the PDF; every negative value here is a signed
//! load (heating positive, cooling negative) per the tables' own
//! "Heating > 0 and Cooling < 0" convention.

/// The SHA-256 of the addendum PDF the tables are transcribed from.
pub const ADDENDUM_A_SHA256: &str =
    "cb993847ccd4a50a10a09e27cbfc199536487490952d1438ee575e52a616e892";

/// The addendum PDF's public URL (the pin a reader re-verifies the hash
/// against).
pub const ADDENDUM_A_URL: &str = "https://www.ashrae.org/file%20library/technical%20resources/standards%20and%20guidelines/standards%20addenda/140_2023_a_20250829.pdf";

/// The A3 single-value metrics (Tables A3-1 and A3-4's two load
/// columns). The unit is kWh/yr throughout this module; the addendum
/// prints MWh/yr and the values are converted by x1000.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum A3Metric {
    /// Annual heating load, Section 7.3.1.1's output (Table A3-1's first
    /// limits column).
    AnnualHeatingKwh,
    /// Annual sensible cooling load, Section 7.3.1.2's output (Table
    /// A3-1's second limits column).
    AnnualCoolingKwh,
}

impl A3Metric {
    pub fn column_name(self) -> &'static str {
        match self {
            Self::AnnualHeatingKwh => "annual_heating_kwh",
            Self::AnnualCoolingKwh => "annual_cooling_kwh",
        }
    }
}

/// One applicable single-value range case (A3.1: a software result is
/// within a given range when it is greater than or equal to the lower
/// limit and less than or equal to the upper limit; both bounds are
/// inclusive). A sensitivity row's value is the minuend case's load
/// minus the subtrahend case's load; an absolute row's minuend and
/// subtrahend are the same case, so the value is that case's load.
#[derive(Debug, Clone, Copy)]
pub struct SingleValueRange {
    /// The table's row label (the PDF's typographic dash rendered as an
    /// ASCII hyphen).
    pub range_case: &'static str,
    pub metric: A3Metric,
    /// The case whose load is taken first (the row's left operand).
    pub minuend: &'static str,
    /// The case subtracted (the row's right operand; the same case for
    /// an absolute row).
    pub subtrahend: &'static str,
    pub min_kwh: f64,
    pub max_kwh: f64,
}

/// Table A3-1, Low Mass Building single-value ranges, the applicable
/// cells (kWh/yr, the addendum's MWh/yr x1000). The table's n/a cell is
/// not a range case and is recorded in `SINGLE_VALUE_NOT_APPLICABLE`.
pub const LOW_MASS_SINGLE_VALUE_RANGES: &[SingleValueRange] = &[
    sv(
        "Case 600",
        A3Metric::AnnualHeatingKwh,
        "600",
        "600",
        3750.0,
        4980.0,
    ),
    sv(
        "Case 600",
        A3Metric::AnnualCoolingKwh,
        "600",
        "600",
        5000.0,
        6830.0,
    ),
    sv(
        "Case 610 - Case 600",
        A3Metric::AnnualHeatingKwh,
        "610",
        "600",
        -140.0,
        290.0,
    ),
    sv(
        "Case 610 - Case 600",
        A3Metric::AnnualCoolingKwh,
        "610",
        "600",
        -2260.0,
        -800.0,
    ),
    sv(
        "Case 620 - Case 600",
        A3Metric::AnnualHeatingKwh,
        "620",
        "600",
        -80.0,
        400.0,
    ),
    sv(
        "Case 620 - Case 600",
        A3Metric::AnnualCoolingKwh,
        "620",
        "600",
        -2240.0,
        -1640.0,
    ),
    sv(
        "Case 630 - Case 620",
        A3Metric::AnnualHeatingKwh,
        "630",
        "620",
        20.0,
        740.0,
    ),
    sv(
        "Case 630 - Case 620",
        A3Metric::AnnualCoolingKwh,
        "630",
        "620",
        -1680.0,
        -770.0,
    ),
    sv(
        "Case 640 - Case 600",
        A3Metric::AnnualHeatingKwh,
        "640",
        "600",
        -2170.0,
        -1220.0,
    ),
    sv(
        "Case 640 - Case 600",
        A3Metric::AnnualCoolingKwh,
        "640",
        "600",
        -560.0,
        30.0,
    ),
    sv(
        "Case 650 - Case 600",
        A3Metric::AnnualCoolingKwh,
        "650",
        "600",
        -1540.0,
        -950.0,
    ),
    sv(
        "Case 660 - Case 600",
        A3Metric::AnnualHeatingKwh,
        "660",
        "600",
        -1070.0,
        -160.0,
    ),
    sv(
        "Case 660 - Case 600",
        A3Metric::AnnualCoolingKwh,
        "660",
        "600",
        -3090.0,
        -2500.0,
    ),
    sv(
        "Case 670 - Case 600",
        A3Metric::AnnualHeatingKwh,
        "670",
        "600",
        250.0,
        2980.0,
    ),
    sv(
        "Case 670 - Case 600",
        A3Metric::AnnualCoolingKwh,
        "670",
        "600",
        50.0,
        840.0,
    ),
    sv(
        "Case 680 - Case 600",
        A3Metric::AnnualHeatingKwh,
        "680",
        "600",
        -2540.0,
        -1900.0,
    ),
    sv(
        "Case 680 - Case 600",
        A3Metric::AnnualCoolingKwh,
        "680",
        "600",
        130.0,
        870.0,
    ),
    sv(
        "Case 685 - Case 600",
        A3Metric::AnnualHeatingKwh,
        "685",
        "600",
        330.0,
        770.0,
    ),
    sv(
        "Case 685 - Case 600",
        A3Metric::AnnualCoolingKwh,
        "685",
        "600",
        2700.0,
        3310.0,
    ),
    sv(
        "Case 695 - Case 685",
        A3Metric::AnnualHeatingKwh,
        "695",
        "685",
        -2380.0,
        -1940.0,
    ),
    sv(
        "Case 695 - Case 685",
        A3Metric::AnnualCoolingKwh,
        "695",
        "685",
        -210.0,
        440.0,
    ),
];

/// Table A3-4, High Mass Building single-value ranges, the applicable
/// cells (kWh/yr, the addendum's MWh/yr x1000).
pub const HIGH_MASS_SINGLE_VALUE_RANGES: &[SingleValueRange] = &[
    sv(
        "Case 900",
        A3Metric::AnnualHeatingKwh,
        "900",
        "900",
        1040.0,
        2280.0,
    ),
    sv(
        "Case 900",
        A3Metric::AnnualCoolingKwh,
        "900",
        "900",
        2350.0,
        2600.0,
    ),
    sv(
        "Case 900 - Case 910",
        A3Metric::AnnualHeatingKwh,
        "900",
        "910",
        -520.0,
        -20.0,
    ),
    sv(
        "Case 900 - Case 910",
        A3Metric::AnnualCoolingKwh,
        "900",
        "910",
        350.0,
        1740.0,
    ),
    sv(
        "Case 920 - Case 900",
        A3Metric::AnnualHeatingKwh,
        "920",
        "900",
        1510.0,
        1920.0,
    ),
    sv(
        "Case 920 - Case 900",
        A3Metric::AnnualCoolingKwh,
        "920",
        "900",
        80.0,
        480.0,
    ),
    sv(
        "Case 930 - Case 920",
        A3Metric::AnnualHeatingKwh,
        "930",
        "920",
        200.0,
        1150.0,
    ),
    sv(
        "Case 930 - Case 920",
        A3Metric::AnnualCoolingKwh,
        "930",
        "920",
        -1190.0,
        -440.0,
    ),
    sv(
        "Case 940 - Case 900",
        A3Metric::AnnualHeatingKwh,
        "940",
        "900",
        -820.0,
        -370.0,
    ),
    sv(
        "Case 940 - Case 900",
        A3Metric::AnnualCoolingKwh,
        "940",
        "900",
        -190.0,
        60.0,
    ),
    sv(
        "Case 950 - Case 900",
        A3Metric::AnnualCoolingKwh,
        "950",
        "900",
        -2000.0,
        -1560.0,
    ),
    sv(
        "Case 960 - Case 900",
        A3Metric::AnnualHeatingKwh,
        "960",
        "900",
        960.0,
        1120.0,
    ),
    sv(
        "Case 960 - Case 900",
        A3Metric::AnnualCoolingKwh,
        "960",
        "900",
        -1810.0,
        -1270.0,
    ),
    sv(
        "Case 980 - Case 900",
        A3Metric::AnnualHeatingKwh,
        "980",
        "900",
        -1650.0,
        -1000.0,
    ),
    sv(
        "Case 980 - Case 900",
        A3Metric::AnnualCoolingKwh,
        "980",
        "900",
        1090.0,
        1410.0,
    ),
    sv(
        "Case 985 - Case 900",
        A3Metric::AnnualHeatingKwh,
        "985",
        "900",
        640.0,
        810.0,
    ),
    sv(
        "Case 985 - Case 900",
        A3Metric::AnnualCoolingKwh,
        "985",
        "900",
        3520.0,
        4180.0,
    ),
    sv(
        "Case 995 - Case 985",
        A3Metric::AnnualHeatingKwh,
        "995",
        "985",
        -1830.0,
        -1070.0,
    ),
    sv(
        "Case 995 - Case 985",
        A3Metric::AnnualCoolingKwh,
        "995",
        "985",
        630.0,
        1150.0,
    ),
];

/// The Tables A3-1 and A3-4 cells the addendum prints n/a: no limits,
/// so no range case exists for the cell. Case 650 and Case 950 disable
/// heating by their case configurations, and the addendum prints no
/// heating limits for the two rows.
pub const SINGLE_VALUE_NOT_APPLICABLE: &[(&str, A3Metric)] = &[
    ("Case 650 - Case 600", A3Metric::AnnualHeatingKwh),
    ("Case 950 - Case 900", A3Metric::AnnualHeatingKwh),
];

const fn sv(
    range_case: &'static str,
    metric: A3Metric,
    minuend: &'static str,
    subtrahend: &'static str,
    min_kwh: f64,
    max_kwh: f64,
) -> SingleValueRange {
    SingleValueRange {
        range_case,
        metric,
        minuend,
        subtrahend,
        min_kwh,
        max_kwh,
    }
}

/// The two dates the addendum's time-series criteria apply to: the
/// daily hourly zone loads of February 1 and July 14 (the foreword:
/// "the daily hourly zone loads for February 1 and July 14 included in
/// the standard output reports for the thermal fabric low-mass and
/// high-mass test suite (Section 7)").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum A3Date {
    February1,
    July14,
}

impl A3Date {
    /// The (month, day) of the simulated calendar the date selects.
    pub fn month_day(self) -> (u32, u32) {
        match self {
            Self::February1 => (2, 1),
            Self::July14 => (7, 14),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::February1 => "February 1",
            Self::July14 => "July 14",
        }
    }
}

/// Table A3-2, Low Mass Building hourly representative curves, in the
/// printed layout: 24 hour rows (hour-ending labels 1 to 24), the
/// February 1 columns first (`LOW_MASS_FEB1_CASES`, in order), then the
/// July 14 columns (`LOW_MASS_JUL14_CASES`, in order). Values are
/// hourly zone loads in kWh, signed with heating positive and cooling
/// negative. The hour label is the hour-ending convention: Hour 1 is
/// the interval 00:00 to 01:00 local standard time. The convention is
/// the only one the tables' own day structure supports (a 24-column day
/// starting at Hour 1 must begin at midnight), and it is the EnergyPlus
/// and ASHRAE 140 output convention the standard's reports use: the
/// Case 640 row's morning pick-up spike (3.06 kWh at Hour 8) is the
/// 07:00-08:00 interval the case's 23:00-07:00 setback schedule
/// recovers into, and the single day's 24 hours run midnight to
/// midnight.
// The table is a transcription: a printed value that resembles a
// mathematical constant (the -3.14 in the Case 670 column) is the
// standard's data, not the constant.
#[allow(clippy::approx_constant)]
pub const LOW_MASS_CURVE_TABLE: [[f64; 13]; 24] = [
    [
        2.08, 0.08, 1.82, 2.62, 1.17, 2.12, 1.31, 0.00, 0.00, 0.00, 0.00, -0.05, -0.14,
    ], // hour 1
    [
        2.22, 0.60, 1.90, 2.73, 1.32, 2.24, 1.41, 0.00, 0.00, 0.00, 0.00, 0.03, -0.07,
    ], // hour 2
    [
        2.28, 0.95, 1.94, 2.82, 1.42, 2.29, 1.48, 0.00, 0.00, 0.00, 0.00, 0.13, -0.02,
    ], // hour 3
    [
        2.33, 1.19, 2.00, 2.91, 1.50, 2.33, 1.53, 0.00, 0.00, 0.12, 0.00, 0.25, 0.04,
    ], // hour 4
    [
        2.35, 1.32, 2.02, 2.95, 1.53, 2.35, 1.55, 0.00, 0.00, 0.33, 0.00, 0.34, 0.11,
    ], // hour 5
    [
        2.35, 1.40, 2.03, 2.94, 1.55, 2.35, 1.56, 0.00, 0.00, 0.22, 0.00, 0.24, 0.06,
    ], // hour 6
    [
        2.36, 1.45, 2.05, 2.92, 1.55, 2.36, 1.55, 0.00, 0.00, 0.00, 0.00, -0.10, -0.10,
    ], // hour 7
    [
        1.88, 3.06, 1.71, 2.39, 1.16, 1.88, 1.16, 0.00, 0.00, 0.00, 0.00, -0.46, -0.33,
    ], // hour 8
    [
        0.55, 1.75, 0.82, 0.84, 0.17, 0.55, 0.06, 0.00, 0.00, 0.00, 0.00, -0.91, -0.63,
    ], // hour 9
    [
        0.00, 0.08, 0.08, 0.00, -0.10, -1.11, -1.67, -0.23, -0.08, -0.26, -0.19, -1.55, -1.15,
    ], // hour 10
    [
        -1.28, -0.67, 0.00, -1.14, -1.98, -3.12, -3.67, -1.23, -0.78, -1.51, -1.07, -2.40, -1.87,
    ], // hour 11
    [
        -3.23, -2.69, -0.47, -3.27, -4.13, -4.52, -5.21, -2.22, -1.41, -2.61, -1.89, -3.14, -2.54,
    ], // hour 12
    [
        -4.42, -4.17, -1.63, -4.62, -5.31, -5.41, -6.11, -2.70, -1.85, -3.26, -2.41, -3.49, -3.00,
    ], // hour 13
    [
        -4.67, -4.59, -2.15, -4.89, -5.57, -5.43, -6.13, -2.75, -2.00, -3.31, -2.49, -3.51, -3.05,
    ], // hour 14
    [
        -4.05, -4.02, -1.87, -4.12, -5.05, -4.70, -5.51, -2.59, -1.90, -3.06, -2.26, -3.23, -2.78,
    ], // hour 15
    [
        -2.93, -2.92, -1.28, -2.78, -3.94, -3.59, -4.40, -2.21, -1.69, -2.53, -1.90, -2.85, -2.34,
    ], // hour 16
    [
        -1.33, -1.32, -0.38, -0.99, -2.35, -1.88, -2.73, -1.76, -1.41, -1.94, -1.51, -2.40, -1.95,
    ], // hour 17
    [
        -0.04, -0.04, 0.00, 0.00, -0.51, -0.06, -0.88, -1.37, -1.13, -1.46, -1.19, -1.99, -1.62,
    ], // hour 18
    [
        0.09, 0.09, 0.08, 0.46, 0.00, 1.07, 0.18, -0.98, -0.85, -1.02, -0.88, -1.59, -1.30,
    ], // hour 19
    [
        0.98, 0.98, 0.85, 1.62, 0.00, 1.67, 0.67, -0.57, -0.52, -0.55, -0.57, -1.15, -0.99,
    ], // hour 20
    [
        1.64, 1.64, 1.41, 2.22, 0.20, 2.03, 1.00, -0.23, -0.24, -0.14, -0.33, -0.76, -0.73,
    ], // hour 21
    [
        2.06, 2.06, 1.78, 2.60, 0.72, 2.28, 1.24, -0.02, -0.06, 0.00, -0.17, -0.55, -0.55,
    ], // hour 22
    [
        2.32, 2.32, 2.01, 2.88, 1.08, 2.44, 1.42, 0.00, 0.00, 0.00, -0.05, -0.38, -0.43,
    ], // hour 23
    [
        2.44, 0.00, 2.10, 2.99, 1.29, 2.50, 1.52, 0.00, 0.00, 0.00, 0.00, -0.25, -0.31,
    ], // hour 24
];

/// Table A3-2's February 1 columns, the low-mass cases in print order.
pub const LOW_MASS_FEB1_CASES: [&str; 7] = ["600", "640", "660", "670", "680", "685", "695"];

/// Table A3-2's July 14 columns, the low-mass cases in print order
/// (Case 640 has no July 14 column: Table A3-3's footnote, "there are
/// no July 14 hourly load result for Case 640").
pub const LOW_MASS_JUL14_CASES: [&str; 6] = ["600", "660", "670", "680", "685", "695"];

/// Table A3-5, High Mass Building hourly representative curves, in the
/// printed layout: 24 hour rows, February 1 columns first
/// (`HIGH_MASS_FEB1_CASES`), then July 14 (`HIGH_MASS_JUL14_CASES`).
/// Units and sign convention as Table A3-2; the hour label is
/// hour-ending.
pub const HIGH_MASS_CURVE_TABLE: [[f64; 9]; 24] = [
    [0.98, 0.00, 0.00, 1.13, 0.42, -0.10, -0.22, -0.71, -0.64], // hour 1
    [1.08, 0.00, 0.00, 1.23, 0.54, -0.03, -0.16, -0.62, -0.58], // hour 2
    [1.18, 0.00, 0.00, 1.33, 0.63, 0.00, -0.09, -0.53, -0.50],  // hour 3
    [1.29, 0.00, 0.00, 1.43, 0.71, 0.00, -0.02, -0.42, -0.43],  // hour 4
    [1.37, 0.00, 0.00, 1.50, 0.77, 0.00, 0.00, -0.33, -0.35],   // hour 5
    [1.44, 0.00, 0.10, 1.57, 0.81, 0.00, 0.00, -0.32, -0.37],   // hour 6
    [1.51, 0.00, 0.22, 1.62, 0.84, 0.00, -0.06, -0.40, -0.44],  // hour 7
    [1.36, 1.26, 0.19, 1.45, 0.70, 0.00, -0.16, -0.52, -0.54],  // hour 8
    [0.79, 1.58, 0.00, 0.87, 0.25, -0.11, -0.27, -0.72, -0.67], // hour 9
    [0.27, 0.84, 0.00, 0.35, -0.36, -0.36, -0.49, -1.04, -0.92], // hour 10
    [0.00, 0.30, 0.00, -0.25, -1.05, -0.66, -0.77, -1.43, -1.28], // hour 11
    [0.00, 0.00, -0.11, -0.94, -1.80, -0.98, -1.05, -1.79, -1.58], // hour 12
    [0.00, 0.00, -0.73, -1.59, -2.50, -1.22, -1.27, -2.04, -1.81], // hour 13
    [0.00, 0.00, -1.17, -1.94, -2.79, -1.38, -1.40, -2.17, -1.89], // hour 14
    [-0.02, 0.00, -1.35, -2.01, -2.83, -1.41, -1.39, -2.15, -1.89], // hour 15
    [-0.01, 0.00, -1.28, -1.83, -2.72, -1.36, -1.28, -2.05, -1.78], // hour 16
    [0.00, 0.00, -1.01, -1.36, -2.19, -1.26, -1.19, -1.94, -1.67], // hour 17
    [0.00, 0.00, -0.47, -0.63, -1.40, -1.14, -1.08, -1.80, -1.55], // hour 18
    [0.00, 0.00, -0.05, -0.19, -0.92, -1.01, -0.96, -1.63, -1.40], // hour 19
    [0.00, 0.00, 0.00, 0.10, -0.65, -0.85, -0.81, -1.43, -1.22], // hour 20
    [0.00, 0.09, 0.00, 0.34, -0.42, -0.70, -0.69, -1.27, -1.08], // hour 21
    [0.00, 0.33, 0.00, 0.55, -0.23, -0.59, -0.59, -1.16, -0.98], // hour 22
    [0.08, 0.54, 0.00, 0.71, -0.04, -0.49, -0.51, -1.05, -0.89], // hour 23
    [0.29, 0.00, 0.00, 0.86, 0.10, -0.37, -0.41, -0.94, -0.79], // hour 24
];

/// Table A3-5's February 1 columns, the high-mass cases in print order.
pub const HIGH_MASS_FEB1_CASES: [&str; 5] = ["900", "940", "980", "985", "995"];

/// Table A3-5's July 14 columns, the high-mass cases in print order
/// (Case 940 has no July 14 column, per Table A3-6's footnote).
pub const HIGH_MASS_JUL14_CASES: [&str; 4] = ["900", "980", "985", "995"];

/// Table A3-3 or A3-6's row: one case's time-series limits. A software
/// result is within the range on a date when the calculated Pearson
/// correlation coefficient is ABOVE the limit AND the calculated root
/// mean square error is BELOW the limit (A3.1). The limits are for the
/// 24 hourly loads of the date in kWh; the Pearson limit is the same
/// value for both dates.
#[derive(Debug, Clone, Copy)]
pub struct TimeSeriesLimits {
    pub case_id: &'static str,
    pub min_pearson: f64,
    /// The July 14 RMSE limit; `None` is the addendum's n/a (the case
    /// has no July 14 hourly result).
    pub max_rmse_feb1_kwh: f64,
    pub max_rmse_jul14_kwh: Option<f64>,
}

/// Table A3-3, Low Mass Building time-series ranges.
pub const LOW_MASS_TIME_SERIES_LIMITS: &[TimeSeriesLimits] = &[
    ts("600", 0.879, 0.4194, Some(0.1921)),
    ts("640", 0.879, 0.6559, None),
    ts("660", 0.879, 0.3087, Some(0.1697)),
    ts("670", 0.879, 0.7881, Some(0.3018)),
    ts("680", 0.879, 0.3990, Some(0.2097)),
    ts("685", 0.879, 0.4643, Some(0.3218)),
    ts("695", 0.879, 0.2069, Some(0.2001)),
];

/// Table A3-6, High Mass Building time-series ranges.
pub const HIGH_MASS_TIME_SERIES_LIMITS: &[TimeSeriesLimits] = &[
    ts("900", 0.879, 0.3716, Some(0.0967)),
    ts("940", 0.879, 0.4298, None),
    ts("980", 0.879, 0.2969, Some(0.0986)),
    ts("985", 0.879, 0.4166, Some(0.2053)),
    ts("995", 0.879, 0.2944, Some(0.2468)),
];

const fn ts(
    case_id: &'static str,
    min_pearson: f64,
    max_rmse_feb1_kwh: f64,
    max_rmse_jul14_kwh: Option<f64>,
) -> TimeSeriesLimits {
    TimeSeriesLimits {
        case_id,
        min_pearson,
        max_rmse_feb1_kwh,
        max_rmse_jul14_kwh,
    }
}

/// Table A3-14's row for one test group: the number of range cases in
/// the group and the minimum count of within-range results the group's
/// pass requires (A3.2: "Software shall pass a test group when the
/// count of results computed for each range case ... meets or exceeds
/// the minimum count required").
#[derive(Debug, Clone, Copy)]
pub struct MinimumCaseRule {
    pub group: &'static str,
    pub tables: &'static str,
    pub range_cases: usize,
    pub minimum_within: usize,
}

/// Table A3-14, the two Section 7 thermal-fabric groups' rows. The
/// range-case counts are the addendum's printed totals: the low-mass
/// group's 34 are Table A3-1's 21 applicable single-value cells plus
/// Table A3-2's 13 curve columns (7 February 1 + 6 July 14); the
/// high-mass group's 28 are Table A3-4's 19 plus Table A3-5's 9
/// (5 February 1 + 4 July 14). The counts are re-derived from the
/// pinned tables in the tests.
pub const FABRIC_MINIMUM_CASE_RULES: &[MinimumCaseRule] = &[
    MinimumCaseRule {
        group: "Thermal Fabric Low Mass",
        tables: "A3-1, A3-2, A3-3",
        range_cases: 34,
        minimum_within: 30,
    },
    MinimumCaseRule {
        group: "Thermal Fabric High Mass",
        tables: "A3-4, A3-5, A3-6",
        range_cases: 28,
        minimum_within: 25,
    },
];

/// The representative curve for one case and date, `None` when the
/// addendum prints no column for the pair (Case 640 and Case 940 have
/// no July 14 columns; their absence is the tables' own, not a missing
/// fixture). The two mass groups' case ids are disjoint (6xx against
/// 9xx), so the lookup is unambiguous.
pub fn representative_curve(case_id: &str, date: A3Date) -> Option<[f64; 24]> {
    // The case list's position indexes the date's block of columns; the
    // block starts after the February columns in the printed row.
    fn extract<const N: usize>(
        table: &[[f64; N]; 24],
        cases: &[&str],
        block_offset: usize,
        case_id: &str,
    ) -> Option<[f64; 24]> {
        let col = cases.iter().position(|c| *c == case_id)? + block_offset;
        assert!(
            col < N,
            "the pinned curve table's column list exceeds its own width"
        );
        let mut out = [0.0; 24];
        for (row, hour) in table.iter().zip(out.iter_mut()) {
            *hour = row[col];
        }
        Some(out)
    }
    match date {
        A3Date::February1 => extract(&LOW_MASS_CURVE_TABLE, &LOW_MASS_FEB1_CASES, 0, case_id)
            .or_else(|| extract(&HIGH_MASS_CURVE_TABLE, &HIGH_MASS_FEB1_CASES, 0, case_id)),
        A3Date::July14 => {
            // The July block starts after the February columns: 7 for
            // the low-mass table, 5 for the high-mass table.
            extract(&LOW_MASS_CURVE_TABLE, &LOW_MASS_JUL14_CASES, 7, case_id)
                .or_else(|| extract(&HIGH_MASS_CURVE_TABLE, &HIGH_MASS_JUL14_CASES, 5, case_id))
        }
    }
}
