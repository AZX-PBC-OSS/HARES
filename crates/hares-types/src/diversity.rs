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
//! clamp's two folded tails (the upper at `hi`, the lower at `lo`, which
//! enter the mean with opposite signs) leave the fleet mean of the drawn
//! scale at 0.999965 (occupancy, sigma 0.15), 0.999854 (appliance, sigma
//! 0.25) and 0.999254 (water, sigma 0.30): a low bias of at most 0.075
//! percent, not a drift that compounds with the spread. The clamp folds
//! the variance too: the clamped lognormal's variance is 0.022687,
//! 0.063662 and 0.089734, which is 0.30, 1.29 and 4.71 percent below the
//! unclamped exp(sigma^2) - 1. The fleet's per-home spread is therefore
//! the clamped draw's, narrower than the log-space sigma's.
//! The closed forms are held to an independent quadrature and to the
//! drawn fleet's moments by
//! `the_fleet_scale_moments_are_the_clamped_lognormals`. The timing
//! shifts are bounded integer hours on the profiles' own resolution, a
//! permutation of each day-type array's mass, so a home's profile
//! integral is unmoved by its timing draw. The published reference the
//! fleet-aggregate acceptance is stated against is the NREL End-Use Load
//! Profiles report (Pigman, McCarty, et al., NREL 2022,
//! <https://www.osti.gov/servlets/purl/1909353>): its per-home annual
//! electricity by end use across the synthetic stock is the published
//! scale for the fleet's spread, while the witnesses pin the fleet MEAN
//! at the 301 profile's own integral and the cross-home spread at the
//! clamped closed form, never at a table read from the report. The
//! stochastic occupancy FAMILY the layer mirrors (the
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
/// The clamp's two folded tails move the mean and the variance; the
/// closed forms and the numbers per spread are on the fleet test
/// (`the_fleet_scale_moments_are_the_clamped_lognormals`).
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
    /// binomial standard error is sqrt(2000*6/7) = 41.4, so the
    /// 4-standard-error band is under 170).
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

    /// The fleet calibration's invariants, at the distribution level: the
    /// mean of a drawn scale is the CLAMPED lognormal's analytic mean, not
    /// 1, and its variance is the clamped variance, not sigma's. For
    /// X = exp(sigma*Z - sigma^2/2) (E[X] = 1) clamped to [lo, hi], with
    /// mu = -sigma^2/2 and Phi the standard normal CDF,
    ///   clamp^k = X^k - (X^k - hi^k)^+ + (lo^k - X^k)^+,
    ///   E[X^k] = exp(k mu + k^2 sigma^2 / 2),
    ///   E[X^k 1{X > c}] = E[X^k] Phi((mu + k sigma^2 - ln c) / sigma),
    ///   E[X^k 1{X < c}] = E[X^k] Phi((ln c - mu - k sigma^2) / sigma),
    /// so E[(X^k - c)^+] = E[X^k 1{X>c}] - c^k P(X>c) and
    /// E[(c^k - X^k)^+] = c^k P(X<c) - E[X^k 1{X<c}], and the two folded
    /// tails ENTER the mean with opposite signs: the upper fold lowers it,
    /// the lower fold raises it. The mean is 0.999965 (occupancy, sigma
    /// 0.15, [0.6, 1.6]), 0.999854 (appliance, sigma 0.25, [0.5, 2.0]) and
    /// 0.999254 (water, sigma 0.30, [0.5, 2.0]), a stated low bias of at
    /// most 0.075 percent; the variance is 0.022687, 0.063662 and
    /// 0.089734, which is 0.30, 1.29 and 4.71 percent below the unclamped
    /// exp(sigma^2) - 1. The closed form is held to an independent Simpson
    /// quadrature of the normal density at 1e-6 relative, and 50,000 drawn
    /// homes hold it at four standard errors of the mean and of the
    /// variance estimator (the fourth moment gives the latter's spread).
    /// The standardized fourth central moment is at least 1 for every
    /// distribution (Jensen: E[(X-m)^4] >= E[(X-m)^2]^2); the closed
    /// form's kurtosis is pinned on that side of 1, the direction the
    /// variance estimator's spread (fourth - variance^2 below) needs to
    /// be real.
    #[test]
    fn the_fleet_scale_moments_are_the_clamped_lognormals() {
        /// The closed-form first, second and fourth moments of the clamped
        /// lognormal, from the folded-tail identity above, with the
        /// standardized fourth central moment (the kurtosis) beside them.
        fn clamped_moments(sigma: f64, lo: f64, hi: f64) -> [f64; 4] {
            let mu = -sigma * sigma / 2.0;
            let phi = crate::scenario::normal_cdf;
            let moment = |k: u32| -> f64 {
                let kf = f64::from(k);
                let e_k = libm::exp(kf * mu + kf * kf * sigma * sigma / 2.0);
                let (hi_k, lo_k) = match k {
                    1 => (hi, lo),
                    2 => (hi * hi, lo * lo),
                    3 => (hi * hi * hi, lo * lo * lo),
                    _ => (hi * hi * hi * hi, lo * lo * lo * lo),
                };
                let log_hi = libm::log(hi);
                let log_lo = libm::log(lo);
                let above = e_k * phi((mu + kf * sigma * sigma - log_hi) / sigma);
                let below = e_k * phi((log_lo - mu - kf * sigma * sigma) / sigma);
                let p_above = phi((mu - log_hi) / sigma);
                let p_below = phi((log_lo - mu) / sigma);
                e_k - (above - hi_k * p_above) + (lo_k * p_below - below)
            };
            let mean = moment(1);
            let second = moment(2);
            let variance = second - mean * mean;
            // The standardized fourth central moment, by the expansion
            // E[(X-m)^4] = E[X^4] - 4 m E[X^3] + 6 m^2 E[X^2] - 3 m^4.
            let central_fourth = moment(4) - 4.0 * mean * moment(3) + 6.0 * mean * mean * second
                - 3.0 * mean * mean * mean * mean;
            let kurtosis = central_fourth / (variance * variance);
            [mean, variance, moment(4), kurtosis]
        }

        /// The same moments by direct Simpson quadrature of the standard
        /// normal density, an independent reference: with z standard
        /// normal and x = exp(mu + sigma*z),
        ///   E[g(clamp(x))] = integral g(clamp(exp(mu + sigma z))) phi(z) dz.
        /// The grid is z in [-12, 12] (the truncated mass is exp(-72)) with
        /// 100,000 intervals, so the rule's error on this integrand is
        /// about 1e-11 absolute: far below the 1e-6 relative band.
        fn quadrature_moments(sigma: f64, lo: f64, hi: f64) -> [f64; 3] {
            const INTERVALS: usize = 100_000;
            let mu = -sigma * sigma / 2.0;
            let (a, b) = (-12.0_f64, 12.0_f64);
            let step = (b - a) / INTERVALS as f64;
            let density_scale = 1.0 / libm::sqrt(2.0 * std::f64::consts::PI);
            let mut sums = [0.0_f64; 3];
            for i in 0..=INTERVALS {
                let z = a + step * i as f64;
                let weight = if i == 0 || i == INTERVALS {
                    1.0
                } else if i % 2 == 1 {
                    4.0
                } else {
                    2.0
                };
                let x = libm::exp(mu + sigma * z).clamp(lo, hi);
                let w = weight * density_scale * libm::exp(-0.5 * z * z) * step / 3.0;
                sums[0] += w * x;
                sums[1] += w * x * x;
                sums[2] += w * x * x * x * x;
            }
            let mean = sums[0];
            [mean, sums[1] - mean * mean, sums[2]]
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
        // (name, sigma, bounds, the record's mean, the record's variance
        // fold below exp(sigma^2) - 1, in percent).
        let cases = [
            (
                "occupancy",
                OCCUPANCY_LEVEL_SIGMA,
                OCCUPANCY_LEVEL_BOUNDS,
                0.999_965_f64,
                0.30_f64,
            ),
            (
                "appliance",
                APPLIANCE_FACTOR_SIGMA,
                APPLIANCE_FACTOR_BOUNDS,
                0.999_854_f64,
                1.29_f64,
            ),
            (
                "water",
                WATER_LEVEL_SIGMA,
                WATER_LEVEL_BOUNDS,
                0.999_254_f64,
                4.71_f64,
            ),
        ];
        for (c, (name, sigma, (lo, hi), documented_mean, documented_fold)) in
            cases.iter().enumerate()
        {
            let [analytic, analytic_variance, fourth, kurtosis] = clamped_moments(*sigma, *lo, *hi);
            let [q_mean, q_variance, _] = quadrature_moments(*sigma, *lo, *hi);
            // The closed form against the quadrature: an identity whose
            // folded-tail sign is wrong cannot hold this band.
            assert!(
                (analytic - q_mean).abs() < 1e-6 * q_mean,
                "the {name} closed-form mean {analytic} left the quadrature \
                 reference {q_mean} by {:.3e} relative",
                (analytic - q_mean).abs() / q_mean
            );
            assert!(
                (analytic_variance - q_variance).abs() < 1e-6 * q_variance,
                "the {name} closed-form variance {analytic_variance} left the \
                 quadrature reference {q_variance} by {:.3e} relative",
                (analytic_variance - q_variance).abs() / q_variance
            );
            // The record's stated numbers are the closed form's: the six
            // decimals of the mean and the two of the variance fold.
            assert!(
                (analytic - documented_mean).abs() < 1e-6,
                "the record states the {name} clamped mean as {documented_mean}, \
                 the closed form computes {analytic}"
            );
            let unclamped_variance = libm::exp(sigma * sigma) - 1.0;
            let fold_percent = (1.0 - analytic_variance / unclamped_variance) * 100.0;
            assert!(
                (fold_percent - documented_fold).abs() < 0.005,
                "the record states the {name} variance fold as {documented_fold} \
                 percent, the closed form computes {fold_percent}"
            );
            assert!(
                (1.0 - analytic).abs() < 0.00075,
                "the {name} scale's clamp folds {:.4} percent of the mean: the \
                 record states at most 0.075 percent",
                (1.0 - analytic).abs() * 100.0
            );
            // Jensen's bound: the standardized fourth central moment is at
            // least 1 for every distribution. The variance estimator's
            // spread below (fourth - variance^2) is real only on that
            // side of 1, and a sign error in a folded tail would show
            // here as an imaginary spread before any mean moved.
            assert!(
                kurtosis >= 1.0,
                "the {name} clamped scale's standardized fourth moment \
                 {kurtosis} is below 1: no distribution's is"
            );

            // The drawn homes at the estimators' resolution: the mean's
            // standard error is sd/sqrt(N); the sample variance's is
            // sqrt((E[clamp^4] - var^2)/N).
            let mean = sums[c] / n as f64;
            let variance = (sq_sums[c] / n as f64 - mean * mean).max(0.0);
            let sd = libm::sqrt(variance);
            let se_mean = libm::sqrt(analytic_variance) / libm::sqrt(n as f64);
            assert!(
                (mean - analytic).abs() < 4.0 * se_mean,
                "the {name} scale's fleet mean {mean} left the clamped \
                 lognormal's analytic mean {analytic} beyond four standard \
                 errors ({se_mean})"
            );
            let se_variance =
                libm::sqrt((fourth - analytic_variance * analytic_variance) / n as f64);
            assert!(
                (variance - analytic_variance).abs() < 4.0 * se_variance,
                "the {name} scale's fleet variance {variance} left the clamped \
                 lognormal's analytic variance {analytic_variance} beyond four \
                 standard errors of the variance estimator ({se_variance})"
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

    /// The factors' names are the drawn pairs' names, in the same order:
    /// one list feeds the record a manifest reports and the drawn values,
    /// so the two cannot name different orders.
    #[test]
    fn the_appliance_factor_names_are_the_drawn_pairs() {
        let draws = ScheduleDiversityDraws::draw(dwelling_seed_scenario(9, None, 0));
        let pairs = draws.appliance_factors.as_pairs();
        let names: Vec<&str> = pairs.iter().map(|(name, _)| *name).collect();
        assert_eq!(
            names,
            ApplianceFactors::NAMES,
            "the record's names must be the drawn pairs' names in draw order"
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
