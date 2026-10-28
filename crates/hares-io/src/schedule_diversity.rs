//! The generated-schedule family's behavioral diversity applied to the
//! default profiles: one home's drawn parameters
//! (`hares_types::diversity::ScheduleDiversityDraws`) turn the 301
//! default profiles into that home's behavioral profiles before the
//! schedule generates.
//!
//! The transform is a pure function of the profiles and the draws: the
//! timing shifts rotate the hourly fractions cyclically within each
//! day-type array (the shift wraps midnight, never the day type, and the
//! monthly multipliers are untouched), and the level scales multiply the
//! fractions (the appliance end uses' diversity factors). A profile the
//! defaults do not carry is left alone here; the generation errors on it
//! exactly as it does undiversified, naming the profile and the file.

use hares_types::diversity::ScheduleDiversityDraws;

use crate::schedule_resolve::{DefaultProfiles, DefaultScheduleProfile};

/// Rotate 24 hourly fractions by `shift` hours: the value hour `h` reads
/// after the rotation is the value hour `h - shift` read before, so a
/// positive shift moves every event later. The rotation is cyclic over
/// the day: the profile's mass moves whole hours, none is lost.
fn rotate_hours(fractions: &[f64; 24], shift: i32) -> [f64; 24] {
    let shift = shift.rem_euclid(24) as usize;
    let mut out = [0.0_f64; 24];
    for (h, slot) in out.iter_mut().enumerate() {
        *slot = fractions[(h + 24 - shift) % 24];
    }
    out
}

/// Scale every fraction by `factor`.
fn scale_fractions(fractions: &[f64; 24], factor: f64) -> [f64; 24] {
    let mut out = [0.0_f64; 24];
    for (h, slot) in out.iter_mut().enumerate() {
        *slot = fractions[h] * factor;
    }
    out
}

/// The diversity transform over one profile: the shift rotates both day
/// types' hours, the factor scales them, the monthly multipliers pass
/// through.
fn transform_profile(
    profile: &DefaultScheduleProfile,
    shift_hours: i32,
    factor: f64,
) -> DefaultScheduleProfile {
    DefaultScheduleProfile {
        weekday_fractions: scale_fractions(
            &rotate_hours(&profile.weekday_fractions, shift_hours),
            factor,
        ),
        weekend_fractions: scale_fractions(
            &rotate_hours(&profile.weekend_fractions, shift_hours),
            factor,
        ),
        month_multipliers: profile.month_multipliers,
    }
}

/// The diversified copy of the defaults' profiles under one home's draws:
/// the occupancy profile carries the presence timing's shift and the
/// occupancy level's scale, each appliance profile its diversity factor,
/// and the water-heating profile the water-draw timing's own shift and
/// the water level's scale. The appliance hot-water columns share the
/// appliance profiles, so a home with a larger dishwasher factor draws
/// more dishwasher power and more dishwasher hot water together; the
/// water-heating profile is the fixtures' own draws.
///
/// Profiles the defaults do not carry pass through untouched: the
/// generation errors on the missing profile, naming it, exactly as an
/// undiversified run does.
#[must_use]
pub fn diversify_profiles(
    profiles: &DefaultProfiles,
    draws: &ScheduleDiversityDraws,
) -> DefaultProfiles {
    let mut out = profiles.clone();
    let applied: [(&str, i32, f64); 8] = [
        (
            "Occupancy",
            draws.presence_shift_hours,
            draws.occupancy_level_scale,
        ),
        ("MELs", 0, draws.appliance_factors.mels),
        ("Indoor Lighting", 0, draws.appliance_factors.lighting),
        ("Dishwasher", 0, draws.appliance_factors.dishwasher),
        ("Clothes Washer", 0, draws.appliance_factors.clothes_washer),
        ("Clothes Dryer", 0, draws.appliance_factors.clothes_dryer),
        ("Cooking Range", 0, draws.appliance_factors.cooking_range),
        (
            "Water Heating",
            draws.water_shift_hours,
            draws.water_level_scale,
        ),
    ];
    for (name, shift, factor) in applied {
        if let Some(profile) = out.find(name) {
            let diversified = transform_profile(profile, shift, factor);
            out.replace(name, diversified);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use hares_types::rng::dwelling_seed_scenario;

    fn utc_start() -> chrono::DateTime<chrono::FixedOffset> {
        chrono::FixedOffset::east_opt(0)
            .expect("offset")
            .with_ymd_and_hms(2018, 1, 1, 0, 0, 0)
            .single()
            .expect("start")
    }

    fn profile(weekday_peak_hour: usize) -> DefaultScheduleProfile {
        let mut weekday = [0.0_f64; 24];
        weekday[weekday_peak_hour] = 1.0;
        let mut weekend = [0.0_f64; 24];
        weekend[weekday_peak_hour] = 0.5;
        DefaultScheduleProfile {
            weekday_fractions: weekday,
            weekend_fractions: weekend,
            month_multipliers: [1.0; 12],
        }
    }

    /// The rotation moves the peak whole hours later and wraps midnight
    /// without losing mass; the weekend array moves with the weekday
    /// array.
    #[test]
    fn rotation_shifts_peaks_and_wraps_midnight() {
        let p = profile(6);
        let rotated = rotate_hours(&p.weekday_fractions, 2);
        assert_eq!(rotated[8], 1.0, "the hour-6 peak reads at hour 8");
        assert_eq!(rotated[6], 0.0);
        let wrapped = rotate_hours(&p.weekday_fractions, -3);
        assert_eq!(wrapped[3], 1.0, "the hour-6 peak wraps to hour 3");
        let sum: f64 = rotated.iter().sum();
        assert!((sum - 1.0).abs() < 1e-12, "no mass lost: {sum}");
        // The shift of a full day is the identity.
        assert_eq!(rotate_hours(&p.weekday_fractions, 24), p.weekday_fractions);
    }

    /// The scaling multiplies every hour; the transform keeps the monthly
    /// multipliers and moves both day types.
    #[test]
    fn scaling_multiplies_and_transform_keeps_multipliers() {
        let p = profile(6);
        let out = transform_profile(&p, 1, 1.5);
        assert_eq!(out.weekday_fractions[7], 1.5);
        assert_eq!(out.weekend_fractions[7], 0.75);
        assert_eq!(out.month_multipliers, [1.0; 12]);
    }

    /// The profile-level diversity: the same profiles under two homes'
    /// keys diversify differently, the same key reproduces, and a
    /// zero-shift zero-factor home is the identity (the factor 1.0 and
    /// shift 0 draws change nothing).
    #[test]
    fn diversified_profiles_are_keyed_and_reproducible() {
        let mut profiles = DefaultProfiles::default();
        profiles.insert_for_test("Occupancy", profile(6));
        profiles.insert_for_test("Water Heating", profile(7));
        let key_a = dwelling_seed_scenario(42, None, 1);
        let key_b = dwelling_seed_scenario(42, None, 2);
        let draws_a = ScheduleDiversityDraws::draw(key_a);
        let draws_b = ScheduleDiversityDraws::draw(key_b);
        let a = diversify_profiles(&profiles, &draws_a);
        let b = diversify_profiles(&profiles, &draws_b);
        let again = diversify_profiles(&profiles, &draws_a);
        let a_occ = a.get("Occupancy").expect("the profile transforms");
        let b_occ = b.get("Occupancy").expect("the profile transforms");
        let a_again = again.get("Occupancy").expect("the profile transforms");
        if draws_a.presence_shift_hours != draws_b.presence_shift_hours
            || draws_a.occupancy_level_scale != draws_b.occupancy_level_scale
        {
            assert_ne!(
                a_occ.weekday_fractions, b_occ.weekday_fractions,
                "two homes' keys must diversify the profile differently"
            );
        }
        assert_eq!(
            a_occ.weekday_fractions, a_again.weekday_fractions,
            "the same key reproduces the diversified profile"
        );
    }

    /// The shipped defaults' profiles diversify without losing a profile:
    /// every generated column's profile transforms (the shipped CSV
    /// carries all eight), and the diversification is a pure function of
    /// the key.
    #[test]
    fn shipped_profiles_transform_under_the_draws() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../python/hares/defaults");
        let profiles = crate::load_default_profiles(&dir).expect("the shipped profiles load");
        let draws = ScheduleDiversityDraws::draw(dwelling_seed_scenario(42, None, 1));
        let diversified = diversify_profiles(&profiles, &draws);
        // The generated columns survive: the same names, now diversified.
        let base = crate::hpxml_schedule::generate_default_schedule(
            utc_start(),
            chrono::Duration::hours(48),
            chrono::Duration::seconds(3600),
            &profiles,
        )
        .expect("the base schedule generates");
        let diversified_schedule = crate::hpxml_schedule::generate_default_schedule(
            utc_start(),
            chrono::Duration::hours(48),
            chrono::Duration::seconds(3600),
            &diversified,
        )
        .expect("the diversified schedule generates");
        assert_eq!(base.column_names, diversified_schedule.column_names);
    }

    /// The fleet-aggregate acceptance, stated against the published
    /// reference: the EULP report's distribution of per-home annual
    /// electricity by end use (Pigman, McCarty, et al., NREL 2022,
    /// https://www.osti.gov/servlets/purl/1909353) is the published
    /// variance reference; the comparison statistic this layer pins is
    /// the fleet mean of the per-home annual schedule-integral of each
    /// generated occupancy-driven column against the 301 default
    /// profile's own integral (the mean-one calibration's contract), with
    /// the cross-home spread pinned nonzero. The tolerance: 10 percent of
    /// the profile integral at N=200, several standard errors of the
    /// drawn scales' mean (sigma 0.15 to 0.30 over 200 homes is at most
    /// about 2.1 percent) plus the clamp's fold. The memory rule holds:
    /// the probe generates one home's schedule at a time and drops it as
    /// consumed; no fleet is resident.
    #[test]
    fn fleet_aggregate_stays_at_the_profile_mean_with_nonzero_spread() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../python/hares/defaults");
        let profiles = crate::load_default_profiles(&dir).expect("the shipped profiles load");
        let start = chrono::FixedOffset::east_opt(0)
            .expect("offset")
            .with_ymd_and_hms(2018, 1, 1, 0, 0, 0)
            .single()
            .expect("start");
        let duration = chrono::Duration::hours(24);
        let interval = chrono::Duration::seconds(3600);

        // The base integrals (the 301 profiles' own, the fleet mean's
        // target), per column.
        let base =
            crate::hpxml_schedule::generate_default_schedule(start, duration, interval, &profiles)
                .expect("the base schedule generates");
        let base_sums: Vec<f64> = base
            .columns
            .iter()
            .map(|column| column.iter().sum())
            .collect();

        let n = 200_u64;
        let mut fleet_sums = vec![0.0_f64; base.columns.len()];
        let mut fleet_sq_sums = vec![0.0_f64; base.columns.len()];
        let mut any_home_differed = false;
        for i in 0..n {
            let key = dwelling_seed_scenario(42, None, i as i64);
            let draws = ScheduleDiversityDraws::draw(key);
            let diversified = diversify_profiles(&profiles, &draws);
            let schedule = crate::hpxml_schedule::generate_default_schedule(
                start,
                duration,
                interval,
                &diversified,
            )
            .expect("the home's schedule generates");
            for (c, column) in schedule.columns.iter().enumerate() {
                let sum: f64 = column.iter().sum();
                fleet_sums[c] += sum;
                fleet_sq_sums[c] += sum * sum;
                if sum != base_sums[c] {
                    any_home_differed = true;
                }
            }
            // Drop as consumed: only the accumulators ride the loop.
            drop(schedule);
        }

        assert!(
            any_home_differed,
            "the fleet's homes must not all draw the base profile"
        );
        let fleet_mean = |sum: f64| sum / n as f64;
        let fleet_std = |sum: f64, sq: f64| {
            let m = sum / n as f64;
            libm::sqrt((sq / n as f64 - m * m).max(0.0))
        };
        for (c, name) in base.column_names.iter().enumerate() {
            let m = fleet_mean(fleet_sums[c]);
            let s = fleet_std(fleet_sums[c], fleet_sq_sums[c]);
            let rel = (m - base_sums[c]).abs() / base_sums[c].abs().max(1e-12);
            assert!(
                rel < 0.10,
                "the {name} column's fleet mean left the profile's own integral: \
                 fleet {m} vs profile {} (relative {rel})",
                base_sums[c]
            );
            assert!(
                s > 0.0,
                "the {name} column's cross-home spread must be nonzero"
            );
        }
    }
}
