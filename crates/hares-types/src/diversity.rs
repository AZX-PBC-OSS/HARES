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
//! scale is drawn mean-one BEFORE its clamp (a lognormal with mu =
//! -sigma^2/2), so the unclamped draw's mean is exactly one, and the
//! clamp's fold lowers the fleet mean of the drawn scale to 0.999945
//! (occupancy, sigma 0.15), 0.999561 (appliance, sigma 0.25) and
//! 0.997761 (water, sigma 0.30): a low bias of at most 0.23 percent,
//! not a drift that compounds with the spread (the closed form and its
//! pin are `the_fleet_scale_means_are_the_clamped_lognormal_means`). The
//! timing shifts are bounded integer hours on the profiles' own
//! resolution, a permutation of each day-type array's mass, so a home's
//! profile integral is unmoved by its timing draw. The published variance
//! reference the acceptance is stated against is the NREL End-Use Load
//! Profiles report (Pigman, McCarty, et al., NREL 2022,
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

/// The dimensions' registry: every behavioral dimension of the family,
/// in draw order. `ScheduleDiversityDraws::draw` consumes exactly one
/// stream per entry and the keystream-disjointness pin walks this
/// array with its per-home stream count derived from it, so a
/// dimension joins the family by joining here, which joins the pin.
/// There is no other list.
pub const DIMENSIONS: [&str; 5] = [
    PRESENCE_TIMING_DIMENSION,
    OCCUPANCY_LEVEL_DIMENSION,
    APPLIANCE_FACTORS_DIMENSION,
    WATER_TIMING_DIMENSION,
    WATER_LEVEL_DIMENSION,
];

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
    /// stream per [`DIMENSIONS`] entry (the registry is the dimension
    /// list), each dimension's draws in a fixed order.
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
///
/// The draw is the bounded sampler's, not a modulo of a 32-bit word: two
/// billion is not a multiple of the band's 2*band+1 offsets, so a modulo
/// would hand the low offsets the remainder (4 of 2^32 here, about 1e-9
/// relative), and the conversion of the range would need a fallback for
/// a band the parameter table never declares.
fn shift_draw(rng: &mut ChaCha8Rng, band: i32) -> i32 {
    rng.random_range(-band..=band)
}

/// A mean-one lognormal draw clamped to `lo..=hi`: the normal's mu is
/// -sigma^2/2, so the UNCLAMPED mean is exp(-sigma^2/2 + sigma^2/2) = 1.
/// The clamp's fold lowers the mean; the closed form of the clamped mean
/// and its numbers per spread are on the fleet test
/// (`the_fleet_scale_means_are_the_clamped_lognormal_means`).
fn mean_one_lognormal(rng: &mut ChaCha8Rng, sigma: f64, lo: f64, hi: f64) -> f64 {
    let z: f64 = rand_distr::StandardNormal.sample(rng);
    (libm::exp(sigma * (z - sigma / 2.0))).clamp(lo, hi)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::dwelling_seed_scenario;

    /// The presence shifts' uniformity pin: every offset in the band is
    /// drawn with the same probability, so a home is as likely to run an
    /// hour early as an hour late and the fleet's timing mass has no
    /// preferred hour. 14,000 draws over 7 offsets put 2,000 in each (the
    /// binomial standard error is 41.3, so the 4-standard-error band is
    /// under 170).
    #[test]
    fn shift_draw_is_uniform_over_the_band() {
        use std::collections::BTreeMap;

        let key = dwelling_seed_scenario(42, None, 3);
        let mut rng = dimension_rng(key, PRESENCE_TIMING_DIMENSION);
        let mut counts: BTreeMap<i32, u32> = BTreeMap::new();
        for _ in 0..14_000 {
            *counts
                .entry(shift_draw(&mut rng, PRESENCE_SHIFT_BAND_H))
                .or_insert(0) += 1;
        }
        assert_eq!(counts.len(), 7, "the band holds 2*3+1 offsets: {counts:?}");
        for (offset, count) in counts {
            assert!(
                (-3..=3).contains(&offset),
                "the offset {offset} left the band"
            );
            assert!(
                (2_000_i32 - i32::try_from(count).unwrap_or(i32::MAX)).abs() < 170,
                "the offset {offset} was drawn {count} times: the band is not uniform"
            );
        }
    }

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

    /// The fleet calibration's invariant: the mean of a drawn scale is
    /// the CLAMPED lognormal's analytic mean, not 1. For
    /// X = exp(sigma*Z - sigma^2/2) (E[X] = 1) clamped to [lo, hi],
    ///   E[clamp(X)] = 1 - E[(X-hi)^+] - E[(lo-X)^+],
    ///   E[(X-c)^+] = Phi((sigma^2/2 - ln c)/sigma)
    ///                - c*Phi((-sigma^2/2 - ln c)/sigma),
    ///   E[(c-X)^+] = c*Phi((ln c + sigma^2/2)/sigma)
    ///                - Phi((ln c - sigma^2/2)/sigma),
    /// which gives 0.999945363 (occupancy, sigma 0.15, [0.6, 1.6]),
    /// 0.999560544 (appliance, sigma 0.25, [0.5, 2.0]) and 0.997761048
    /// (water, sigma 0.30, [0.5, 2.0]). The draw is therefore mean-one
    /// before the clamp and the clamp's fold is a stated low bias of at
    /// most 0.23 percent, not an unstated drift: 50,000 homes pin each
    /// family's mean at the closed form within four standard errors
    /// (sigma/sqrt(N) = 1.34e-3 at the widest spread) and the drawn
    /// spread at its own sigma within 20 percent.
    #[test]
    fn the_fleet_scale_means_are_the_clamped_lognormal_means() {
        /// The closed-form mean of the clamped lognormal, above.
        fn clamped_mean(sigma: f64, lo: f64, hi: f64) -> f64 {
            let mu = -sigma * sigma / 2.0;
            let upper = crate::scenario::normal_cdf((mu + sigma * sigma - libm::log(hi)) / sigma)
                - hi * crate::scenario::normal_cdf((mu - libm::log(hi)) / sigma);
            let lower = lo * crate::scenario::normal_cdf((libm::log(lo) - mu) / sigma)
                - crate::scenario::normal_cdf((libm::log(lo) - mu - sigma * sigma) / sigma);
            1.0 - upper - lower
        }

        let n = 50_000_u64;
        let mut sums = [0.0_f64; 3];
        let mut sq_sums = [0.0_f64; 3];
        for i in 0..n {
            let draws = ScheduleDiversityDraws::draw(dwelling_seed_scenario(7, None, i as i64));
            let scales = [
                draws.occupancy_level_scale,
                draws.appliance_factors.mels,
                draws.water_level_scale,
            ];
            for (c, scale) in scales.iter().enumerate() {
                sums[c] += scale;
                sq_sums[c] += scale * scale;
            }
        }
        let cases = [
            ("occupancy", OCCUPANCY_LEVEL_SIGMA, OCCUPANCY_LEVEL_BOUNDS),
            ("appliance", APPLIANCE_FACTOR_SIGMA, APPLIANCE_FACTOR_BOUNDS),
            ("water", WATER_LEVEL_SIGMA, WATER_LEVEL_BOUNDS),
        ];
        for (c, (name, sigma, (lo, hi))) in cases.iter().enumerate() {
            let mean = sums[c] / n as f64;
            let variance = (sq_sums[c] / n as f64 - mean * mean).max(0.0);
            let sd = libm::sqrt(variance);
            let analytic = clamped_mean(*sigma, *lo, *hi);
            let se = sigma / libm::sqrt(n as f64);
            assert!(
                (mean - analytic).abs() < 4.0 * se,
                "the {name} scale's fleet mean {mean} left the clamped \
                 lognormal's analytic mean {analytic} beyond four standard \
                 errors ({se})"
            );
            assert!(
                (analytic - 1.0).abs() < 0.0025,
                "the {name} scale's clamp folds {:.4} percent of the mean: \
                 the record states at most 0.23 percent",
                (analytic - 1.0).abs() * 100.0
            );
            assert!(
                (sd - sigma).abs() < 0.2 * sigma,
                "the {name} scale's drawn spread {sd} left its declared sigma {sigma}"
            );
        }
    }

    /// The draws never share a keystream: over a bounded fleet, each
    /// home's dimension streams (walked from [`DIMENSIONS`], the code's
    /// own registry), the scenario sampler's stream and the dwelling's
    /// own stream 0 yield distinct first words, and the per-home stream
    /// count is derived from the registry, so a dimension that joins it
    /// joins this walk and its count with it. The derivation's
    /// injectivity rides the key's bytes and the reserved identities; a
    /// shared keystream is the collision it forbids, and this is the
    /// constructive check over the fleet's actual draws.
    #[test]
    fn dimension_keystreams_are_disjoint_across_homes_and_dimensions() {
        use std::collections::BTreeMap;

        // + the sampler's stream and the dwelling's own stream 0.
        let streams_per_home = DIMENSIONS.len() + 2;
        let mut seen: BTreeMap<u64, String> = BTreeMap::new();
        let mut homes = 0_usize;
        for bldg in 0..64_i64 {
            let key = dwelling_seed_scenario(42, Some(1), bldg);
            let own = crate::rng::RngStream {
                seed: key,
                stream: 0,
            }
            .rng_at(0)
            .random::<u64>();
            if let Some(other) = seen.insert(own, format!("dwelling stream 0 of bldg {bldg}")) {
                panic!("bldg {bldg}'s dwelling stream shares a word with {other}");
            }
            for dimension in DIMENSIONS {
                let word = RngStream::schedule_diversity(key, dimension)
                    .rng_at(0)
                    .random::<u64>();
                if let Some(other) = seen.insert(word, format!("{dimension} of bldg {bldg}")) {
                    panic!("{dimension} of bldg {bldg} shares a word with {other}");
                }
            }
            let word = RngStream::scenario_sampler(key).rng_at(0).random::<u64>();
            if let Some(other) = seen.insert(word, format!("the sampler of bldg {bldg}")) {
                panic!("bldg {bldg}'s sampler shares a word with {other}");
            }
            homes += 1;
        }
        assert_eq!(
            seen.len(),
            homes * streams_per_home,
            "every home contributes {streams_per_home} distinct keystreams"
        );
    }

    /// The registry's guard against the pin's count: the shipped family
    /// holds five dimensions, and this file's disjointness walk and the
    /// draw's five streams are written against that count. A registry
    /// that outgrows the pin fails here with the work named: extend
    /// `ScheduleDiversityDraws` and the disjointness walk with the new
    /// dimension, then move the count.
    #[test]
    fn the_dimension_registry_cannot_outgrow_the_disjointness_pin() {
        assert_eq!(
            DIMENSIONS.len(),
            5,
            "the family gained a dimension: the draw's streams and the \
             keystream-disjointness walk must name it, and this pin's count \
             moves with them"
        );
        for dimension in [
            PRESENCE_TIMING_DIMENSION,
            OCCUPANCY_LEVEL_DIMENSION,
            APPLIANCE_FACTORS_DIMENSION,
            WATER_TIMING_DIMENSION,
            WATER_LEVEL_DIMENSION,
        ] {
            assert!(
                DIMENSIONS.contains(&dimension),
                "the {dimension} dimension left the registry the disjointness \
                 pin walks"
            );
        }
    }
}
