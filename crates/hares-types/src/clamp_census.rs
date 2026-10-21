//! The census of physics clamps: every clamp the physics applies to a
//! value is counted at its site, none are silent.
//!
//! A physics clamp keeps a model inside its valid domain by overwriting
//! the value it is given (a range clamp, a cap, a floor, a truncation).
//! The clamp is intended behavior, but a clamp firing is information the
//! run's consumer needs: a pressure exponent that keeps falling outside
//! its band, a draw that never fits the tank, a humidity ratio that
//! outruns saturation all say something about the inputs. Each firing is
//! recorded on the calling thread's per-step tally; the dwelling drains
//! the tally once per timestep the way it drains the equipment health
//! counts, and the per-site run totals surface through the dwelling's
//! diagnostics. The census is observe-gated: without the `observe`
//! feature no counting code exists, and with it the census rides the
//! diagnostics accessor alone, never the run result's health totals or
//! the output frames.

use serde::{Deserialize, Serialize};

/// The named surfaces where a physics clamp can fire.
///
/// Every member names one clamp the physics applies: the site the count
/// answers "how often did this clamp fire over the run". The enum's
/// variants are the census's keys; [`ClampSite::ALL`] carries the same
/// sites in a stable order for iteration and serialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ClampSite {
    /// The AIM-2 pressure exponent `n_i` clamped into [0.5, 0.7]
    /// (`N_I_MIN`/`N_I_MAX`): the model's wind and stack terms scale
    /// with `ΔP^n_i`, so an out-of-band exponent from the input's
    /// blower-door interpretation is held to the band. Recorded at both
    /// clamp sites: the per-step ASHRAE wind-stack evaluation and the
    /// ACH50-to-coefficients conversion.
    Aim2PressureExponent,
    /// The AIM-2 leakage-distribution factors clamped into their valid
    /// ranges: the stack factor `m_i` (Walker & Wilson Eq. 10-11) and
    /// the ceiling asymmetry `x_i` (Eq. 25). With the fixed leakage-class
    /// table these are guards that valid inputs cannot trip; a change to
    /// the table that lets them fire is counted.
    Aim2LeakageDistribution,
    /// The natural ventilation's comfort-base adjustment ratio clamped
    /// into [0, 1]: the fraction of the indoor-outdoor difference that
    /// lies above the comfort base, held when the base sits below the
    /// outdoor temperature.
    NaturalVentilationAdj,
    /// The natural ventilation flow capped at 20 air changes per hour
    /// (OCHRE's `max_nat_flow`): an open area large enough to ventilate
    /// more is held to the cap.
    NaturalVentilationAchCap,
    /// Relative humidity clamped into [0, 1]: a humidity ratio above
    /// saturation at the local pressure is held to saturation.
    RelativeHumidity,
    /// The exterior film coefficient floored at 1.0 W/(m²·K): at zero
    /// wind and a near-zero surface temperature difference the DOE-2
    /// model's convection coefficient approaches zero, and the floor
    /// keeps the film resistance finite.
    ExteriorFilmFloor,
    /// A step's tank draw truncated to the tank's volume: the requested
    /// withdrawal exceeds what the tank holds, so the delivered draw is
    /// the volume.
    TankDrawTruncation,
    /// A biquadratic curve index out of bounds, clamped to the last
    /// same-type curve. The same events the equipment health counter
    /// `curve_index_clamps` drains: that counter carries the policy
    /// limit and the run result's totals, this site carries the census.
    CurveIndex,
}

impl ClampSite {
    /// Every clamp site, in a stable order.
    pub const ALL: [ClampSite; Self::COUNT] = [
        Self::Aim2PressureExponent,
        Self::Aim2LeakageDistribution,
        Self::NaturalVentilationAdj,
        Self::NaturalVentilationAchCap,
        Self::RelativeHumidity,
        Self::ExteriorFilmFloor,
        Self::TankDrawTruncation,
        Self::CurveIndex,
    ];

    /// The number of clamp sites.
    pub const COUNT: usize = 8;

    /// The site's own name: the key the census reports the site's count
    /// under.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Aim2PressureExponent => "aim2_pressure_exponent",
            Self::Aim2LeakageDistribution => "aim2_leakage_distribution",
            Self::NaturalVentilationAdj => "natural_ventilation_adj",
            Self::NaturalVentilationAchCap => "natural_ventilation_ach_cap",
            Self::RelativeHumidity => "relative_humidity",
            Self::ExteriorFilmFloor => "exterior_film_floor",
            Self::TankDrawTruncation => "tank_draw_truncation",
            Self::CurveIndex => "curve_index",
        }
    }
}

/// Per-site clamp counts for one run.
///
/// The dwelling drains the recording threads' per-step tallies into this
/// struct once per timestep and holds the run totals; read them through
/// [`ClampCensus::count`] or serialize the census to report it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClampCensus {
    counts: [u64; ClampSite::COUNT],
}

impl ClampCensus {
    /// Records one clamp firing at `site`.
    pub fn record(&mut self, site: ClampSite) {
        self.counts[site as usize] += 1;
    }

    /// The run total recorded at `site`.
    #[must_use]
    pub fn count(&self, site: ClampSite) -> u64 {
        self.counts[site as usize]
    }

    /// The run total across every site.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.counts.iter().sum()
    }

    /// Whether no clamp fired at any site.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.total() == 0
    }

    /// Adds `other`'s counts into `self`: the per-step drain's merge into
    /// the run totals.
    pub fn merge(&mut self, other: &ClampCensus) {
        for (site, added) in other.iter() {
            self.counts[site as usize] += added;
        }
    }

    /// Takes the counts out: returns them and resets every site to zero,
    /// the take-and-reset the per-step drain uses.
    pub fn take(&mut self) -> ClampCensus {
        let taken = *self;
        *self = Self::default();
        taken
    }

    /// Iterates the sites and their counts, in [`ClampSite::ALL`]'s
    /// order, including the sites whose count is zero.
    pub fn iter(&self) -> impl Iterator<Item = (ClampSite, u64)> + '_ {
        ClampSite::ALL
            .map(|site| (site, self.counts[site as usize]))
            .into_iter()
    }
}

impl Serialize for ClampCensus {
    /// Serializes the census as a map from each site's name to its
    /// count: every site appears, so the reported shape is stable
    /// whether or not a site fired.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(ClampSite::COUNT))?;
        for (site, count) in self.iter() {
            map.serialize_entry(site.name(), &count)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for ClampCensus {
    /// Reads the census back from the site-name map [`ClampCensus`]'s
    /// `Serialize` writes: an unknown site name is a deserialization
    /// error naming it.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let map: std::collections::BTreeMap<String, u64> =
            std::collections::BTreeMap::deserialize(deserializer)?;
        let mut census = ClampCensus::default();
        for (key, count) in map {
            let site = ClampSite::ALL
                .iter()
                .copied()
                .find(|site| site.name() == key)
                .ok_or_else(|| serde::de::Error::custom(format!("unknown clamp site '{key}'")))?;
            census.counts[site as usize] = count;
        }
        Ok(census)
    }
}

thread_local! {
    static STEP_TALLY: std::cell::Cell<ClampCensus> = std::cell::Cell::new(ClampCensus::default());
}

/// Records one clamp firing at `site` on the calling thread's per-step
/// tally. The dwelling drains the tally once per timestep.
pub fn record(site: ClampSite) {
    STEP_TALLY.with(|tally| {
        let mut updated = tally.get();
        updated.record(site);
        tally.set(updated);
    });
}

/// Drains the calling thread's clamp tally since the last drain: returns
/// the per-site counts and resets the tally to zero.
#[must_use]
pub fn take() -> ClampCensus {
    STEP_TALLY.with(|tally| tally.get().take())
}

#[cfg(test)]
mod tests {
    use super::{ClampCensus, ClampSite};

    #[test]
    fn every_site_names_itself_and_appears_in_the_stable_order() {
        for (idx, site) in ClampSite::ALL.iter().enumerate() {
            assert_eq!(*site, ClampSite::ALL[idx]);
            assert!(!site.name().is_empty(), "every site names itself");
        }
        assert_eq!(ClampSite::ALL.len(), ClampSite::COUNT);
    }

    #[test]
    fn record_count_take_and_merge_follow_the_take_and_reset_pattern() {
        let mut census = ClampCensus::default();
        assert!(census.is_empty());
        assert_eq!(census.total(), 0);

        census.record(ClampSite::RelativeHumidity);
        census.record(ClampSite::RelativeHumidity);
        census.record(ClampSite::TankDrawTruncation);
        assert_eq!(census.count(ClampSite::RelativeHumidity), 2);
        assert_eq!(census.count(ClampSite::TankDrawTruncation), 1);
        assert_eq!(census.total(), 3);

        let taken = census.take();
        assert_eq!(taken.count(ClampSite::RelativeHumidity), 2);
        assert!(census.is_empty(), "take resets the tally");

        census.merge(&taken);
        assert_eq!(census.total(), 3);
        assert_eq!(census.count(ClampSite::TankDrawTruncation), 1);
    }

    #[test]
    fn the_census_serializes_as_a_map_of_site_names_to_counts() {
        let mut census = ClampCensus::default();
        census.record(ClampSite::CurveIndex);
        census.record(ClampSite::CurveIndex);
        let json = serde_json::to_string(&census).expect("serialize ClampCensus");
        let decoded: std::collections::BTreeMap<String, u64> =
            serde_json::from_str(&json).expect("deserialize ClampCensus");
        assert_eq!(decoded.len(), ClampSite::COUNT, "every site appears");
        for site in ClampSite::ALL {
            assert_eq!(
                decoded.get(site.name()).copied(),
                Some(census.count(site)),
                "{}'s key is its own name",
                site.name()
            );
        }
        assert_eq!(decoded.get("curve_index"), Some(&2));
    }
}
