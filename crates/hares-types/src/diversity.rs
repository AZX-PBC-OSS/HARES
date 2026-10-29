//! The generated-schedule family's per-dwelling behavioral diversity
//! draws: the per-home parameter set (the presence timing, the occupancy
//! level, the appliance diversity factors, the water-draw timing and
//! level) that turns the 301 default profiles into one home's behavioral
//! schedule.
//!
//! The draws are keyed deterministically: each diversity dimension draws
//! from its own [`RngStream::schedule_diversity`] stream under the
//! caller's scenario-mixed per-dwelling key
//! (`hares_types::rng::dwelling_seed_scenario`), so the derivation is
//! injective over (master seed, scenario, building, dimension) with no
//! hash of the scenario: the scenario disjointness rides the key's own
//! bytes (the byte discipline the RNG contract pins), the dimension
//! enters through the reserved stream identity, and the same triple
//! reproduces bitwise.
//!
//! Provenance of the parameter table. The CENTRAL profiles are the
//! ANSI/RESNET/ICC 301-2022 Addendum C default schedule fractions
//! (Occupancy Table C.3(5), lighting Tables C.3(3)/(4), the appliance and
//! water-heating rows of the vendored OS-HPXML v1.12.0 Default Schedule
//! Parameters.csv; the provenance contract and its pins live on
//! `hares_io::schedule_resolve::inject_occupancy_schedule`). The SPREADS
//! below are this record's calibrated choice, not published values: each
//! scale is drawn mean-one (a lognormal with mu = -sigma^2/2), so the
//! fleet aggregate's mean stays at the 301 profile's mean whatever the
//! spread, and the timing shifts are bounded integer hours on the
//! profiles' own resolution. The published variance reference the
//! acceptance is stated against is the NREL End-Use Load Profiles report
//! (Pigman, McCarty, et al., NREL 2022,
//! <https://www.osti.gov/servlets/purl/1909353>): per-home annual
//! electricity by end use across the synthetic stock, its moments the
//! comparison the fleet-aggregate witnesses state their tolerances
//! against. The stochastic occupancy FAMILY the layer mirrors (the
//! Markov presence chains, seven ATUS activities 2013-2017) is
//! OS-HPXML v1.12.0's BuildResidentialScheduleFile; the engine runs no
//! ATUS chain (the schedule-sources doc's provenance tiers), and this
//! layer varies the 301 profiles per home instead.

use rand::RngExt;
use rand_chacha::ChaCha8Rng;
use rand_distr::Distribution;

use crate::rng::RngStream;

/// The presence-timing dimension: the home's occupancy schedule shifts by
/// a whole number of hours against the 301 profile's own hours (the
/// household's timing habit).
pub const PRESENCE_TIMING_DIMENSION: &str = "presence-timing";
/// The occupancy-level dimension: the home's occupancy magnitude scale.
pub const OCCUPANCY_LEVEL_DIMENSION: &str = "occupancy-level";
/// The appliance-factors dimension: one diversity factor per appliance
/// end-use profile, drawn in the fixed order of the generated columns'
/// own names, so appending a family never moves an earlier family's
/// draws.
pub const APPLIANCE_FACTORS_DIMENSION: &str = "appliance-factors";
/// The water-draw-timing dimension: the water-heating schedule's hour
/// shift, its own draw so the water timing diverges from the presence
/// timing.
pub const WATER_TIMING_DIMENSION: &str = "water-timing";
/// The water-level dimension: the water-heating magnitude scale.
pub const WATER_LEVEL_DIMENSION: &str = "water-level";

/// The presence shift's band in hours: a discrete uniform over
/// -3..=3. The band is the record's choice: wide enough that a fleet's
/// morning peaks spread across the ramp hours, narrow enough that the
/// weekday/weekend pattern's shape survives (the shift wraps midnight,
/// never the day type).
pub const PRESENCE_SHIFT_BAND_H: i32 = 3;

/// The water-draw shift's band in hours: a discrete uniform over -2..=2.
pub const WATER_SHIFT_BAND_H: i32 = 2;

/// The occupancy level's lognormal spread. The draw is mean-one (mu =
/// -sigma^2/2) and clamped to [`OCCUPANCY_LEVEL_BOUNDS`].
pub const OCCUPANCY_LEVEL_SIGMA: f64 = 0.15;
/// The appliance diversity factor's lognormal spread (every appliance
/// family shares it; a per-family table would claim a calibration the
/// sources do not publish).
pub const APPLIANCE_FACTOR_SIGMA: f64 = 0.25;
/// The water level's lognormal spread (water draws are the widest
/// per-home spread of the residential end uses).
pub const WATER_LEVEL_SIGMA: f64 = 0.30;

/// The occupancy level's clamp: the draw's mass outside the band folds at
/// the bounds, so a home's occupancy scale is never below 0.6 or above
/// 1.6 of the profile's level.
pub const OCCUPANCY_LEVEL_BOUNDS: (f64, f64) = (0.6, 1.6);
/// The appliance factors' clamp.
pub const APPLIANCE_FACTOR_BOUNDS: (f64, f64) = (0.5, 2.0);
/// The water level's clamp.
pub const WATER_LEVEL_BOUNDS: (f64, f64) = (0.5, 2.0);

/// One home's appliance diversity factors, drawn as one family from the
/// appliance-factors dimension's stream in a fixed order (the generated
/// columns' own order), so appending a family never moves an earlier
/// family's draws.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ApplianceFactors {
    /// The MELs profile's factor (plug_loads_other and plug_loads_tv).
    pub mels: f64,
    /// The indoor lighting profile's factor.
    pub lighting: f64,
    /// The dishwasher profile's factor (the electric column and the
    /// appliance's hot-water column share the profile).
    pub dishwasher: f64,
    /// The clothes washer profile's factor.
    pub clothes_washer: f64,
    /// The clothes dryer profile's factor.
    pub clothes_dryer: f64,
    /// The cooking range profile's factor.
    pub cooking_range: f64,
}

impl ApplianceFactors {
    /// The factors' names in draw order (the generated columns' own
    /// order): the record a manifest reports the factors under.
    pub const NAMES: [&str; 6] = [
        "mels",
        "lighting",
        "dishwasher",
        "clothes_washer",
        "clothes_dryer",
        "cooking_range",
    ];

    /// The factors as (name, value) in draw order.
    #[must_use]
    pub fn as_pairs(&self) -> [(&'static str, f64); 6] {
        [
            ("mels", self.mels),
            ("lighting", self.lighting),
            ("dishwasher", self.dishwasher),
            ("clothes_washer", self.clothes_washer),
            ("clothes_dryer", self.clothes_dryer),
            ("cooking_range", self.cooking_range),
        ]
    }

    fn draw(rng: &mut ChaCha8Rng) -> Self {
        let (lo, hi) = APPLIANCE_FACTOR_BOUNDS;
        Self {
            mels: mean_one_lognormal(rng, APPLIANCE_FACTOR_SIGMA, lo, hi),
            lighting: mean_one_lognormal(rng, APPLIANCE_FACTOR_SIGMA, lo, hi),
            dishwasher: mean_one_lognormal(rng, APPLIANCE_FACTOR_SIGMA, lo, hi),
            clothes_washer: mean_one_lognormal(rng, APPLIANCE_FACTOR_SIGMA, lo, hi),
            clothes_dryer: mean_one_lognormal(rng, APPLIANCE_FACTOR_SIGMA, lo, hi),
            cooking_range: mean_one_lognormal(rng, APPLIANCE_FACTOR_SIGMA, lo, hi),
        }
    }
}

/// One home's drawn behavioral diversity parameter set: the numbers the
/// profile transform applies. Serializes so a run manifest can record
/// what a home drew.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ScheduleDiversityDraws {
    /// The occupancy schedule's hour shift (negative: earlier).
    pub presence_shift_hours: i32,
    /// The occupancy magnitude scale (mean-one across the fleet).
    pub occupancy_level_scale: f64,
    /// The appliance end uses' diversity factors.
    pub appliance_factors: ApplianceFactors,
    /// The water-heating schedule's hour shift (negative: earlier).
    pub water_shift_hours: i32,
    /// The water-heating magnitude scale.
    pub water_level_scale: f64,
}

impl ScheduleDiversityDraws {
    /// The home's draws under the per-dwelling key `dwelling_key`: one
    /// stream per dimension, each dimension's draws in a fixed order.
    ///
    /// Contract: the key is the caller's scenario-mixed per-dwelling key
    /// (`hares_types::rng::dwelling_seed_scenario`), so two scenarios'
    /// homes draw different behavior and the same (master seed,
    /// scenario, building) reproduces bitwise; the dimension's stream is
    /// tagged and identity-keyed, so the draws never touch the dwelling's
    /// own RNG (the build's and the simulation's streams are unmoved by
    /// this call).
    #[must_use]
    pub fn draw(dwelling_key: [u8; 32]) -> Self {
        let presence_shift_hours = shift_draw(
            &mut dimension_rng(dwelling_key, PRESENCE_TIMING_DIMENSION),
            PRESENCE_SHIFT_BAND_H,
        );
        let occupancy_level_scale = {
            let mut rng = dimension_rng(dwelling_key, OCCUPANCY_LEVEL_DIMENSION);
            mean_one_lognormal(
                &mut rng,
                OCCUPANCY_LEVEL_SIGMA,
                OCCUPANCY_LEVEL_BOUNDS.0,
                OCCUPANCY_LEVEL_BOUNDS.1,
            )
        };
        let appliance_factors = ApplianceFactors::draw(&mut dimension_rng(
            dwelling_key,
            APPLIANCE_FACTORS_DIMENSION,
        ));
        let water_shift_hours = shift_draw(
            &mut dimension_rng(dwelling_key, WATER_TIMING_DIMENSION),
            WATER_SHIFT_BAND_H,
        );
        let water_level_scale = {
            let mut rng = dimension_rng(dwelling_key, WATER_LEVEL_DIMENSION);
            mean_one_lognormal(
                &mut rng,
                WATER_LEVEL_SIGMA,
                WATER_LEVEL_BOUNDS.0,
                WATER_LEVEL_BOUNDS.1,
            )
        };
        Self {
            presence_shift_hours,
            occupancy_level_scale,
            appliance_factors,
            water_shift_hours,
            water_level_scale,
        }
    }
}

/// The dimension's own generator: the tagged, identity-keyed stream under
/// the dwelling key, at word position zero.
fn dimension_rng(dwelling_key: [u8; 32], dimension: &str) -> ChaCha8Rng {
    RngStream::schedule_diversity(dwelling_key, dimension).rng_at(0)
}

/// A discrete-uniform hour shift over -band..=band.
fn shift_draw(rng: &mut ChaCha8Rng, band: i32) -> i32 {
    let n = u32::try_from(2 * band + 1).unwrap_or(1);
    (rng.random::<u32>() % n) as i32 - band
}

/// A mean-one lognormal draw clamped to `lo..=hi`: the normal's mu is
/// -sigma^2/2, so the unclamped mean is exp(-sigma^2/2 + sigma^2/2) = 1
/// and the fleet aggregate's mean stays at the profile's own.
fn mean_one_lognormal(rng: &mut ChaCha8Rng, sigma: f64, lo: f64, hi: f64) -> f64 {
    let z: f64 = rand_distr::StandardNormal.sample(rng);
    (libm::exp(sigma * (z - sigma / 2.0))).clamp(lo, hi)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::dwelling_seed_scenario;

    /// The same key reproduces the same draws bitwise; a different
    /// building, scenario or master seed draws differently.
    #[test]
    fn draws_are_keyed_deterministically() {
        let key = dwelling_seed_scenario(42, Some(1), 7);
        let a = ScheduleDiversityDraws::draw(key);
        let b = ScheduleDiversityDraws::draw(key);
        assert_eq!(a, b, "the same key reproduces");

        let other_bldg = ScheduleDiversityDraws::draw(dwelling_seed_scenario(42, Some(1), 8));
        assert_ne!(a, other_bldg, "another building must draw differently");
        let other_scenario = ScheduleDiversityDraws::draw(dwelling_seed_scenario(42, Some(2), 7));
        assert_ne!(a, other_scenario, "another scenario must draw differently");
        let other_master = ScheduleDiversityDraws::draw(dwelling_seed_scenario(43, Some(1), 7));
        assert_ne!(a, other_master, "another master seed must draw differently");
        // The absent scenario's key (None) is the scenario-less key: its
        // draws reproduce the same key's draws, and a declared scenario 0
        // draws differently (the presence byte).
        let absent = ScheduleDiversityDraws::draw(dwelling_seed_scenario(42, None, 7));
        assert_eq!(
            absent,
            ScheduleDiversityDraws::draw(crate::rng::dwelling_seed(42, 7))
        );
        let declared_zero = ScheduleDiversityDraws::draw(dwelling_seed_scenario(42, Some(0), 7));
        assert_ne!(absent, declared_zero, "Some(0) is not None");
    }

    /// The draws land in their declared bands, the scales are finite and
    /// positive, and the shifts are whole hours inside their bands.
    #[test]
    fn draws_hold_their_bands() {
        for bldg in 0..50_i64 {
            let draws = ScheduleDiversityDraws::draw(dwelling_seed_scenario(42, None, bldg));
            assert!(
                (-PRESENCE_SHIFT_BAND_H..=PRESENCE_SHIFT_BAND_H)
                    .contains(&draws.presence_shift_hours),
                "the presence shift {} leaves its band",
                draws.presence_shift_hours
            );
            assert!(
                (-WATER_SHIFT_BAND_H..=WATER_SHIFT_BAND_H).contains(&draws.water_shift_hours),
                "the water shift {} leaves its band",
                draws.water_shift_hours
            );
            assert!(
                (OCCUPANCY_LEVEL_BOUNDS.0..=OCCUPANCY_LEVEL_BOUNDS.1)
                    .contains(&draws.occupancy_level_scale)
            );
            assert!(
                (WATER_LEVEL_BOUNDS.0..=WATER_LEVEL_BOUNDS.1).contains(&draws.water_level_scale)
            );
            for (name, factor) in draws.appliance_factors.as_pairs() {
                assert!(
                    (APPLIANCE_FACTOR_BOUNDS.0..=APPLIANCE_FACTOR_BOUNDS.1).contains(&factor),
                    "the {name} factor {factor} leaves its band"
                );
            }
        }
    }

    /// The mean-one calibration: across a fleet of keys the drawn scales'
    /// mean sits at 1 within the sampling error of the fleet (the fleet
    /// aggregate the 301 profiles' mean pins), and the fleet's cross-home
    /// spread is nonzero (the diversity exists).
    #[test]
    fn fleet_scales_are_mean_one_with_nonzero_spread() {
        let n = 400_u64;
        let mut occupancy_sum = 0.0;
        let mut mels_sum = 0.0;
        let mut water_sum = 0.0;
        let mut mels_first: Option<f64> = None;
        for i in 0..n {
            let draws = ScheduleDiversityDraws::draw(dwelling_seed_scenario(7, None, i as i64));
            occupancy_sum += draws.occupancy_level_scale;
            mels_sum += draws.appliance_factors.mels;
            water_sum += draws.water_level_scale;
            if i == 0 {
                mels_first = Some(draws.appliance_factors.mels);
            } else if Some(draws.appliance_factors.mels) == mels_first {
                panic!("two homes drew the same MELs factor: the streams collided");
            }
        }
        let mean = |sum: f64| sum / n as f64;
        // N=400 at sigma 0.30: the mean's standard error is about 1.5
        // percent, so a 10 percent band is several of them plus the
        // clamp's fold; the band is the fleet witness's stated tolerance.
        assert!(
            (mean(occupancy_sum) - 1.0).abs() < 0.10,
            "the occupancy scale's fleet mean {} left the 1.0 band",
            mean(occupancy_sum)
        );
        assert!(
            (mean(mels_sum) - 1.0).abs() < 0.10,
            "the MELs factor's fleet mean {} left the 1.0 band",
            mean(mels_sum)
        );
        assert!(
            (mean(water_sum) - 1.0).abs() < 0.10,
            "the water level's fleet mean {} left the 1.0 band",
            mean(water_sum)
        );
    }

    /// The dimension's draws never touch the dwelling's own RNG: the
    /// diversity derivation reads no stream the dwelling's build or step
    /// draws from (the tag and the identity partition prove it, and the
    /// dwelling's stream-0 generator's state is unchanged by a draw call).
    #[test]
    fn draws_leave_the_dwelling_stream_untouched() {
        let key = dwelling_seed_scenario(42, None, 7);
        let dwelling_rng = crate::rng::RngStream {
            seed: key,
            stream: 0,
        };
        let before = dwelling_rng.rng_at(0).random::<u64>();
        let _ = ScheduleDiversityDraws::draw(key);
        let after = dwelling_rng.rng_at(0).random::<u64>();
        assert_eq!(before, after, "the diversity draws consumed stream 0");
    }
}
