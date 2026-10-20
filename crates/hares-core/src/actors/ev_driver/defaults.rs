//! The EV driver's documented defaults, in SI units.
//!
//! The driver's behavioural defaults - the daily distance, the average
//! speed, the range-anxiety band and the away-charge power - were written
//! as mile/kW literals at their use sites before the control layer held
//! SI; they are named constants here, defined once from their documented
//! values and read wherever the seed path, the registry defaults and the
//! Python builder use that value, so no value moves and none is repeated.
//! The literals are the `uom` conversions of the documented values on bits
//! (30 mi = 48 280.32 m, 30 mph = 13.4112 m/s, 20 mi = 32 186.88 m,
//! 6.6 kW = 6600 W); the acceptance test pins each against its `uom`
//! expression.

/// The daily driven distance, 30 mi in metres (SI): the constant-schedule
/// stand-in when no archetype preset or parameter overrides it.
pub const DAILY_DRIVE_DISTANCE_M: f64 = 48_280.32;

/// The average driving speed, 30 mph in metres per second (SI): the trip
/// duration's divisor when no archetype preset or parameter overrides it.
pub const AVERAGE_SPEED_M_PER_S: f64 = 13.4112;

/// The range-anxiety band, 20 mi in metres (SI): the distance the
/// remaining pack energy must cover at the documented fuel economy before
/// the driver cancels a non-essential trip.
pub const RANGE_ANXIETY_DISTANCE_M: f64 = 32_186.88;

/// The away-charge power, 6.6 kW in watts (SI): the external charger's
/// rate on the seed path when no parameter overrides it.
pub const AWAY_CHARGE_POWER_W: f64 = 6_600.0;
