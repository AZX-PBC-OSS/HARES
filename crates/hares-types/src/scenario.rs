//! The scenario spec and the sampler: a declared, typed spec of
//! per-parameter distributions over envelope, HVAC and occupancy
//! parameters, and the deterministic sampler that turns
//! `(master_seed, scenario_id, bldg_id)` into each home's parameter set.
//!
//! The sampler composes the landed scenario-key algebra: its draws come
//! from one `RngStream::scenario_sampler` stream under the
//! scenario-mixed per-dwelling key (`dwelling_seed_scenario`), so the
//! derivation is injective over (master seed, scenario, building) with
//! no hash of the scenario (the disjointness rides the key's own bytes),
//! the stream is tagged out of the dwelling's own stream space, and the
//! same triple reproduces bitwise. The spec's parameters draw in
//! declaration order from that one stream, a correlation group's shared
//! factor at its first member, so appending a parameter never moves the
//! values already recorded. A correlated member's value is its
//! distribution's quantile at the one-factor Gaussian copula's
//! probability, which preserves the declared marginal: exactly in exact
//! arithmetic, and to the normal CDF pair's accuracy in doubles (the
//! inverse CDF's 1.15e-9 relative bound and the composed probability's
//! 1e-8, both pinned; the moment, KS and copula correlation witnesses pin
//! the marginal).
//!
//! The sampled record serializes: the per-home payload a run manifest
//! records (the scenario id, the building id, every parameter's name,
//! value and unit), plus the overrides-map form the dwelling
//! constructor's overrides accept, the application path the round-trip
//! witnesses prove reaches the typed config each target names.

use rand::RngExt;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

use crate::error::HaresError;
use crate::rng::{RngStream, dwelling_seed_scenario};

/// One parameter's distribution: the spec's declared marginal, with the
/// units carried by the parameter, not the family.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum ScenarioDistribution {
    /// Uniform: value ~ U(low, high).
    #[serde(rename = "uniform")]
    Uniform { low: f64, high: f64 },
    /// Normal: value ~ N(mean, std_dev^2).
    #[serde(rename = "normal")]
    Normal { mean: f64, std_dev: f64 },
    /// Log-normal: value ~ LogNormal(mu, sigma^2), strictly positive.
    #[serde(rename = "lognormal")]
    LogNormal { mu: f64, sigma: f64 },
    /// Triangular: the min, the mode and the max.
    #[serde(rename = "triangular")]
    Triangular { min: f64, mode: f64, max: f64 },
    /// Discrete: one of `values` by `weights` (the weights need not sum
    /// to one; they are normalized).
    #[serde(rename = "discrete")]
    Discrete { values: Vec<f64>, weights: Vec<f64> },
}

impl ScenarioDistribution {
    /// The distribution's parameters are well-formed: finite, ordered,
    /// positive spreads, non-negative weights.
    pub fn validate(&self) -> Result<(), HaresError> {
        match self {
            Self::Uniform { low, high } if !low.is_finite() || !high.is_finite() || low >= high => {
                Err(HaresError::Equipment(format!(
                    "the uniform distribution requires finite low < high, got {low}, {high}"
                )))
            }
            Self::Normal { mean, std_dev }
                if !mean.is_finite() || !std_dev.is_finite() || *std_dev < 0.0 =>
            {
                Err(HaresError::Equipment(format!(
                    "the normal distribution requires a finite mean and a non-negative \
                     finite std_dev, got {mean}, {std_dev}"
                )))
            }
            Self::LogNormal { mu, sigma }
                if !mu.is_finite() || !sigma.is_finite() || *sigma <= 0.0 =>
            {
                Err(HaresError::Equipment(format!(
                    "the lognormal distribution requires a finite mu and a positive \
                     finite sigma, got {mu}, {sigma}"
                )))
            }
            Self::Triangular { min, mode, max }
                if !min.is_finite()
                    || !mode.is_finite()
                    || !max.is_finite()
                    || !(min <= mode && mode <= max)
                    || min == max =>
            {
                Err(HaresError::Equipment(format!(
                    "the triangular distribution requires min <= mode <= max with \
                     min < max, got {min}, {mode}, {max}"
                )))
            }
            Self::Discrete { values, weights }
                if values.is_empty()
                    || values.len() != weights.len()
                    || values.iter().any(|v| !v.is_finite())
                    || weights.iter().any(|w| !w.is_finite() || *w < 0.0)
                    || weights.iter().all(|w| *w == 0.0) =>
            {
                Err(HaresError::Equipment(
                    "the discrete distribution requires non-empty finite values, \
                     one non-negative weight per value, and at least one positive weight"
                        .to_string(),
                ))
            }
            _ => Ok(()),
        }
    }

    /// The analytical mean the moment witnesses compare against.
    #[must_use]
    pub fn mean(&self) -> f64 {
        match self {
            Self::Uniform { low, high } => (low + high) / 2.0,
            Self::Normal { mean, .. } => *mean,
            // The triangular mean is the average of its three corners; the
            // mode is the mean only when the triangle is symmetric.
            Self::Triangular { min, mode, max } => (min + mode + max) / 3.0,
            Self::LogNormal { mu, sigma } => libm::exp(mu + sigma * sigma / 2.0),
            Self::Discrete { values, weights } => {
                let total: f64 = weights.iter().sum();
                if total == 0.0 {
                    return 0.0;
                }
                values
                    .iter()
                    .zip(weights.iter())
                    .map(|(v, w)| v * w / total)
                    .sum()
            }
        }
    }

    /// Draw one value directly from the distribution: the uncorrelated
    /// parameter's path. The distribution validates before it draws, so a
    /// malformed one is an error naming it, never a panic or a
    /// substitute value.
    ///
    /// # Errors
    ///
    /// [`HaresError`] when the distribution's parameters are malformed.
    pub fn sample(&self, rng: &mut ChaCha8Rng) -> Result<f64, HaresError> {
        self.validate()?;
        match self {
            Self::Uniform { low, high } => Ok(*low + rng.random::<f64>() * (high - low)),
            Self::Normal { mean, std_dev } => {
                let z = normal_standard(rng);
                Ok(mean + std_dev * z)
            }
            Self::LogNormal { mu, sigma } => {
                let z = normal_standard(rng);
                Ok(libm::exp(mu + sigma * z))
            }
            Self::Triangular { min, mode, max } => {
                let u: f64 = rng.random();
                Ok(triangular_quantile(*min, *mode, *max, u))
            }
            Self::Discrete { values, weights } => {
                let total: f64 = weights.iter().sum();
                let u: f64 = rng.random::<f64>() * total;
                let mut cumulative = 0.0;
                for (v, w) in values.iter().zip(weights.iter()) {
                    cumulative += w;
                    if u < cumulative {
                        return Ok(*v);
                    }
                }
                values.last().copied().ok_or_else(|| {
                    HaresError::Equipment(
                        "the discrete distribution carries no values to draw".to_string(),
                    )
                })
            }
        }
    }

    /// The distribution's quantile function at probability `p` in (0, 1):
    /// the correlated parameter's path (the one-factor Gaussian copula's
    /// transform, which preserves this marginal exactly). The distribution
    /// validates before it maps, so a malformed one is an error naming it,
    /// never a panic.
    ///
    /// # Errors
    ///
    /// [`HaresError`] when the distribution's parameters are malformed.
    pub fn quantile(&self, p: f64) -> Result<f64, HaresError> {
        self.validate()?;
        let p = p.clamp(f64::EPSILON, 1.0 - f64::EPSILON);
        Ok(match self {
            Self::Uniform { low, high } => low + p * (high - low),
            Self::Normal { mean, std_dev } => mean + std_dev * normal_inverse_cdf(p),
            Self::LogNormal { mu, sigma } => libm::exp(mu + sigma * normal_inverse_cdf(p)),
            Self::Triangular { min, mode, max } => triangular_quantile(*min, *mode, *max, p),
            Self::Discrete { values, weights } => {
                let total: f64 = weights.iter().sum();
                let u = p * total;
                let mut cumulative = 0.0;
                for (v, w) in values.iter().zip(weights.iter()) {
                    cumulative += w;
                    if u < cumulative {
                        return Ok(*v);
                    }
                }
                values.last().copied().ok_or_else(|| {
                    HaresError::Equipment(
                        "the discrete distribution carries no values to quantile".to_string(),
                    )
                })?
            }
        })
    }
}

/// A standard normal draw (Box-Muller from two uniforms; the pairs the
/// draw order produces are independent and deterministic like any other
/// ChaCha8 draw).
fn normal_standard(rng: &mut ChaCha8Rng) -> f64 {
    let u1 = open_unit(rng);
    let u2: f64 = rng.random::<f64>();
    let r = libm::sqrt(-2.0 * libm::log(u1));
    r * (2.0 * std::f64::consts::PI * u2).cos()
}

/// A uniform draw on the open interval (0, 1]: the uniform sampler yields
/// [0, 1), and the single degenerate 0 would make the Box-Muller radius
/// infinite, so it is resampled (probability 2^-53) rather than
/// substituted with a plausible value.
fn open_unit(rng: &mut ChaCha8Rng) -> f64 {
    let mut u: f64 = rng.random::<f64>();
    while u <= 0.0 {
        u = rng.random::<f64>();
    }
    u
}

/// The standard normal CDF at `z`: Phi(z) = erfc(-z / sqrt(2)) / 2.
#[must_use]
pub fn normal_cdf(z: f64) -> f64 {
    0.5 * libm::erfc(-z / std::f64::consts::SQRT_2)
}

/// The standard normal inverse CDF (Acklam's rational approximation,
/// relative error below 1.15e-9 over the whole open interval): the
/// copula's path from a probability to a normal quantile.
#[must_use]
pub fn normal_inverse_cdf(p: f64) -> f64 {
    const A: [f64; 6] = [
        -3.969_683_028_665_376e1,
        2.209_460_984_245_205e2,
        -2.759_285_104_469_687e2,
        1.383_577_518_672_69e2,
        -3.066_479_806_614_716e1,
        2.506_628_277_459_239,
    ];
    const B: [f64; 5] = [
        -5.447_609_879_822_406e1,
        1.615_858_368_580_409e2,
        -1.556_989_798_598_866e2,
        6.680_131_188_771_972e1,
        -1.328_068_155_288_572e1,
    ];
    const C: [f64; 6] = [
        -7.784_894_002_430_293e-3,
        -3.223_964_580_411_365e-1,
        -2.400_758_277_161_838,
        -2.549_732_539_343_734,
        4.374_664_141_464_968,
        2.938_163_982_698_783,
    ];
    const D: [f64; 4] = [
        7.784_695_709_041_462e-3,
        3.224_671_290_700_398e-1,
        2.445_134_137_142_996,
        3.754_408_661_907_416,
    ];
    let p_low = 0.024_25;
    if p < p_low {
        let q = libm::sqrt(-2.0 * libm::log(p));
        (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    } else if p <= 1.0 - p_low {
        let q = p - 0.5;
        let r = q * q;
        (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q
            / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
    } else {
        let q = libm::sqrt(-2.0 * libm::log(1.0 - p));
        -(((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    }
}

/// The triangular distribution's quantile at `p` in [0, 1].
fn triangular_quantile(min: f64, mode: f64, max: f64, p: f64) -> f64 {
    let fc = (mode - min) / (max - min);
    if p < fc {
        libm::sqrt(p * (mode - min) * (max - min)) + min
    } else {
        max - libm::sqrt((1.0 - p) * (max - min) * (max - mode))
    }
}

/// Where a sampled parameter applies: through the dwelling's overrides
/// map (the named equipment's, or the wildcard's, override field), or
/// through the HPXML data patches the assembly consumes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum ScenarioTarget {
    /// The value applies through the overrides path: the named
    /// equipment's (or the wildcard's) override field, validated by the
    /// assembly's own loud unknown-field rejection.
    #[serde(rename = "override")]
    Override {
        /// The override's equipment key: an equipment's name, or the
        /// wildcard `all` / `*`.
        equipment: String,
        /// The override field's name in the equipment's config schema.
        field: String,
    },
    /// The value applies through the HPXML data patches: the named patch
    /// field (`number_of_bedrooms`).
    #[serde(rename = "patch")]
    Patch {
        /// The patch field's name (the patch schema's).
        field: String,
    },
}

/// A parameter's correlation: the one-factor Gaussian copula's group and
/// this member's loading. Two members' LATENT factors correlate at
/// sqrt(rho1 * rho2); the sampled values' correlation is that latent
/// correlation through each marginal's own transform, so it equals
/// sqrt(rho1 * rho2) only for normal marginals (two uniform marginals at
/// rho 0.9 correlate at about 0.891, the copula's closed form).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioCorrelation {
    /// The group's name: members sharing it draw a shared factor.
    pub group: String,
    /// The member's rho in [0, 1): the shared factor's variance share.
    pub rho: f64,
}

/// One declared parameter: what it is, where it applies, its
/// distribution and its correlation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioParameter {
    /// The parameter's name: unique in the spec, the sampled record's
    /// key.
    pub name: String,
    /// The value's unit, carried into the sampled record ("C", "W",
    /// "count", "fraction").
    #[serde(default)]
    pub unit: String,
    /// Where the value applies.
    pub target: ScenarioTarget,
    /// The declared marginal.
    pub distribution: ScenarioDistribution,
    /// The correlation group, or `None` for an independent draw.
    #[serde(default)]
    pub correlation: Option<ScenarioCorrelation>,
}

/// The declared scenario: the named spec the sampler consumes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioSpec {
    /// The scenario's name: the study's label for this spec.
    #[serde(default)]
    pub name: String,
    /// The declared parameters, in draw order.
    #[serde(default)]
    pub parameters: Vec<ScenarioParameter>,
}

/// One sampled parameter: the record a manifest carries.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SampledValue {
    /// The parameter's name (the spec's).
    pub name: String,
    /// The drawn value.
    pub value: f64,
    /// The value's unit (the spec's).
    #[serde(default)]
    pub unit: String,
}

/// One home's sampled scenario: the per-home payload a run manifest
/// records.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScenarioSample {
    /// The scenario id the sample drew under.
    pub scenario_id: u64,
    /// The building id the sample drew for.
    pub bldg_id: i64,
    /// The sampled values, in the spec's declaration order.
    pub values: Vec<SampledValue>,
    /// The parameters' targets, captured from the spec at sampling: the
    /// application paths the record's forms read. Serialized for
    /// transparency; a manifest recorded before the field existed
    /// deserializes against an empty map (a legacy record's application
    /// surface is the overrides form's own shape, not the targets).
    #[serde(default)]
    targets: std::collections::BTreeMap<String, ScenarioTarget>,
}

impl ScenarioSample {
    /// The overrides-map form the dwelling constructor's overrides
    /// accept: every Override target's value at
    /// `{"<equipment>": {"<field>": value}}`, merged without clobbering
    /// other keys. The Patch targets do not appear here (they apply
    /// through the settings' patches, not the overrides map).
    #[must_use]
    pub fn overrides_json(&self) -> serde_json::Value {
        let mut out = serde_json::Map::new();
        // The targets' (equipment, field, value) triples in declaration
        // order; a repeated equipment key merges into one map.
        for (equipment, field, value) in self.override_triples() {
            let entry = out
                .entry(equipment)
                .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
            if let serde_json::Value::Object(map) = entry {
                map.insert(field, serde_json::json!(value));
            }
        }
        serde_json::Value::Object(out)
    }

    /// The patch values the sample carries: (field name, value) in
    /// declaration order.
    #[must_use]
    pub fn patches(&self) -> Vec<(String, f64)> {
        self.values
            .iter()
            .filter_map(|v| match self.targets.get(&v.name) {
                Some(ScenarioTarget::Patch { field }) => Some((field.clone(), v.value)),
                _ => None,
            })
            .collect()
    }

    /// The sample is applicable: every value carries the target recorded
    /// with it, and every patch target names a field the settings'
    /// patches carry. A record written before the target map existed
    /// carries values with no target and fails naming the value; a
    /// hand-edited record naming an unknown patch field fails naming the
    /// field. The application calls this before touching the settings, so
    /// nothing is applied and then dropped.
    ///
    /// # Errors
    ///
    /// [`HaresError`] naming the value or field the record cannot apply.
    pub fn validate_application(&self) -> Result<(), HaresError> {
        for value in &self.values {
            let Some(target) = self.targets.get(&value.name) else {
                return Err(HaresError::Equipment(format!(
                    "the scenario sample carries '{}' with no recorded target: the value \
                     cannot be applied",
                    value.name
                )));
            };
            if let ScenarioTarget::Patch { field } = target
                && !PATCH_FIELDS.contains(&field.as_str())
            {
                return Err(HaresError::Equipment(format!(
                    "the scenario sample's parameter '{}' patches the unknown field '{}': \
                     the patch fields are {PATCH_FIELDS:?}",
                    value.name, field
                )));
            }
        }
        Ok(())
    }

    fn override_triples(&self) -> Vec<(String, String, f64)> {
        let mut out = Vec::new();
        for v in &self.values {
            if let Some(ScenarioTarget::Override { equipment, field }) = self.targets.get(&v.name) {
                out.push((equipment.clone(), field.clone(), v.value));
            }
        }
        out
    }
}

// The sample's target map is captured at sampling (the spec's
// (name, target) pairs) and serialized with the sample, so the record's
// application needs no second lookup. See the struct's field doc.

impl ScenarioSpec {
    /// The spec validates: every distribution's parameters are
    /// well-formed, the names are unique, the correlations' rhos are in
    /// [0, 1), and every patch target names a field the patches carry.
    pub fn validate(&self) -> Result<(), HaresError> {
        let mut seen = std::collections::BTreeSet::new();
        for parameter in &self.parameters {
            parameter.distribution.validate()?;
            if !seen.insert(parameter.name.as_str()) {
                return Err(HaresError::Equipment(format!(
                    "the scenario spec repeats the parameter name '{}': names are \
                     the sampled record's keys and must be unique",
                    parameter.name
                )));
            }
            if let Some(correlation) = &parameter.correlation
                && (!correlation.rho.is_finite() || !(0.0..1.0).contains(&correlation.rho))
            {
                return Err(HaresError::Equipment(format!(
                    "the parameter '{}' correlation rho must be finite in [0, 1), \
                     got {}",
                    parameter.name, correlation.rho
                )));
            }
            if let ScenarioTarget::Patch { field } = &parameter.target
                && !PATCH_FIELDS.contains(&field.as_str())
            {
                return Err(HaresError::Equipment(format!(
                    "the parameter '{}' patches unknown field '{}': the patch \
                     fields are {PATCH_FIELDS:?}",
                    parameter.name, field
                )));
            }
        }
        Ok(())
    }

    /// The home's sampled scenario: the parameters draw in declaration
    /// order from the sampler's own stream under the scenario-mixed
    /// per-dwelling key; a correlation group's shared factor draws at the
    /// group's first member, so appending a parameter (a new group
    /// included) moves no earlier parameter's draw. Each member's value is
    /// its distribution's quantile at the copula's probability, which
    /// preserves the declared marginal (exactly in exact arithmetic, to
    /// the CDF pair's accuracy in doubles: the inverse's 1.15e-9 relative
    /// bound, the composition's 1e-8).
    ///
    /// # Errors
    ///
    /// [`HaresError`] when the spec does not validate or when a draw is
    /// not finite (the declared support does not bound it); the error
    /// names the parameter.
    pub fn sample(
        &self,
        master_seed: u64,
        scenario_id: u64,
        bldg_id: i64,
    ) -> Result<ScenarioSample, HaresError> {
        self.validate()?;
        let key = dwelling_seed_scenario(master_seed, Some(scenario_id), bldg_id);
        let mut rng = RngStream::scenario_sampler(key).rng_at(0);

        // The groups' shared factors, drawn at each group's first member:
        // the draw order is the declaration order alone, so a study
        // extending its spec never rewrites the values it already
        // recorded.
        let mut shared: std::collections::BTreeMap<&str, f64> = std::collections::BTreeMap::new();

        let mut values = Vec::with_capacity(self.parameters.len());
        let mut targets = std::collections::BTreeMap::new();
        for parameter in &self.parameters {
            let value = match &parameter.correlation {
                None => parameter.distribution.sample(&mut rng)?,
                Some(correlation) => {
                    let z_group = match shared.entry(correlation.group.as_str()) {
                        std::collections::btree_map::Entry::Occupied(entry) => *entry.get(),
                        std::collections::btree_map::Entry::Vacant(entry) => {
                            *entry.insert(normal_standard(&mut rng))
                        }
                    };
                    let z_own = normal_standard(&mut rng);
                    let loading = libm::sqrt(correlation.rho);
                    let z = loading * z_group + libm::sqrt(1.0 - correlation.rho) * z_own;
                    parameter.distribution.quantile(normal_cdf(z))?
                }
            };
            // A draw the declared support does not bound cannot be
            // carried: serde_json maps a non-finite f64 to null, so the
            // overrides form would silently lose it and the assembly
            // would reject a null without naming this parameter.
            if !value.is_finite() {
                return Err(HaresError::Equipment(format!(
                    "the scenario parameter '{}' drew the non-finite value {value} from its \
                     declared distribution",
                    parameter.name
                )));
            }
            targets.insert(parameter.name.clone(), parameter.target.clone());
            values.push(SampledValue {
                name: parameter.name.clone(),
                value,
                unit: parameter.unit.clone(),
            });
        }
        Ok(ScenarioSample {
            scenario_id,
            bldg_id,
            values,
            targets,
        })
    }
}

/// The patch fields a Patch target may name: the HPXML data patches the
/// assembly consumes, the same fields `HpxmlDataPatches` carries.
pub const PATCH_FIELDS: &[&str] = &["number_of_bedrooms"];

#[cfg(test)]
mod tests {
    use super::*;

    fn uniform(low: f64, high: f64) -> ScenarioDistribution {
        ScenarioDistribution::Uniform { low, high }
    }

    fn override_target(equipment: &str, field: &str) -> ScenarioTarget {
        ScenarioTarget::Override {
            equipment: equipment.to_string(),
            field: field.to_string(),
        }
    }

    fn spec(parameters: Vec<ScenarioParameter>) -> ScenarioSpec {
        ScenarioSpec {
            name: "test".to_string(),
            parameters,
        }
    }

    fn parameter(name: &str, distribution: ScenarioDistribution) -> ScenarioParameter {
        ScenarioParameter {
            name: name.to_string(),
            unit: "W".to_string(),
            target: override_target("all", "heating_capacity_w"),
            distribution,
            correlation: None,
        }
    }

    /// The same (master, scenario, bldg) reproduces the sample bitwise;
    /// a different scenario, building or master seed draws differently.
    #[test]
    fn sampling_is_deterministic_and_keyed() {
        let s = spec(vec![
            parameter("a", uniform(0.0, 1.0)),
            parameter("b", uniform(0.0, 1.0)),
        ]);
        let base = s.sample(42, 1, 7).expect("samples");
        assert_eq!(base, s.sample(42, 1, 7).expect("samples"));
        assert_ne!(base, s.sample(42, 2, 7).expect("samples"));
        assert_ne!(base, s.sample(42, 1, 8).expect("samples"));
        assert_ne!(base, s.sample(43, 1, 7).expect("samples"));
        // Declared scenario 0 is a declared scenario: it draws differently
        // from every other id and reproduces itself.
        let zero = s.sample(42, 0, 7).expect("samples");
        assert_eq!(zero, s.sample(42, 0, 7).expect("samples"));
        assert_ne!(zero, base);
    }

    /// The sampled values land inside the declared supports.
    #[test]
    fn samples_hold_their_supports() {
        let s = spec(vec![
            parameter("u", uniform(1.0, 2.0)),
            parameter(
                "t",
                ScenarioDistribution::Triangular {
                    min: 1.0,
                    mode: 1.2,
                    max: 5.0,
                },
            ),
            parameter(
                "d",
                ScenarioDistribution::Discrete {
                    values: vec![2.0, 4.0],
                    weights: vec![0.5, 0.5],
                },
            ),
        ]);
        for i in 0..200_i64 {
            let sample = s
                .sample(7, u64::try_from(i).unwrap_or(0), 0)
                .expect("samples");
            for v in &sample.values {
                match v.name.as_str() {
                    "u" => assert!((1.0..=2.0).contains(&v.value), "{} out of range", v.value),
                    "t" => assert!((1.0..=5.0).contains(&v.value), "{} out of range", v.value),
                    "d" => assert!(v.value == 2.0 || v.value == 4.0, "{} not discrete", v.value),
                    _ => panic!("unexpected parameter"),
                }
            }
        }
    }

    /// The moment witnesses: the sampled fleet's mean sits at the
    /// analytical mean for every family within the sampling error of
    /// N draws (the per-home draws are independent: one scenario id
    /// each).
    #[test]
    fn sampled_moments_match_the_spec() {
        let n = 4000_u64;
        let cases: Vec<(&str, ScenarioDistribution)> = vec![
            ("uniform", uniform(1.0, 5.0)),
            (
                "normal",
                ScenarioDistribution::Normal {
                    mean: 20.0,
                    std_dev: 1.5,
                },
            ),
            (
                "lognormal",
                ScenarioDistribution::LogNormal {
                    mu: 0.0,
                    sigma: 0.25,
                },
            ),
            (
                "triangular",
                // Asymmetric on purpose: (min + mode + max) / 3 = 2.4 is
                // the analytical mean, and a symmetric triangle would let
                // the mode stand in for it.
                ScenarioDistribution::Triangular {
                    min: 1.0,
                    mode: 1.2,
                    max: 5.0,
                },
            ),
            (
                "discrete",
                ScenarioDistribution::Discrete {
                    values: vec![2.0, 3.0, 5.0],
                    weights: vec![0.2, 0.5, 0.3],
                },
            ),
        ];
        for (name, distribution) in cases {
            let s = spec(vec![parameter("p", distribution.clone())]);
            let values: Vec<f64> = (0..n)
                .map(|i| s.sample(9, i, 0).expect("samples").values[0].value)
                .collect();
            let mean = values.iter().sum::<f64>() / n as f64;
            let expected = distribution.mean();
            // The mean's standard error shrinks with N; the band is
            // generous to the tails' skew (the lognormal's).
            let tolerance = (expected.abs() * 0.06).max(0.08);
            assert!(
                (mean - expected).abs() < tolerance,
                "the {name} family's sampled mean {mean} left the analytical mean \
                 {expected} beyond {tolerance}"
            );
        }
    }

    /// The declaration-order contract: appending a parameter moves no
    /// earlier parameter's draw. The parameters draw in declaration
    /// order from the one sampler stream (a correlation group's shared
    /// factor at its first member), so a study extending its spec never
    /// rewrites the recorded values of the parameters it already had,
    /// correlated members included.
    #[test]
    fn appending_a_parameter_moves_no_earlier_draw() {
        let correlated = |name: &str| ScenarioParameter {
            name: name.to_string(),
            unit: "W".to_string(),
            target: override_target("all", "heating_capacity_w"),
            distribution: uniform(0.0, 1.0),
            correlation: Some(ScenarioCorrelation {
                group: "load".to_string(),
                rho: 0.9,
            }),
        };
        let pair = spec(vec![correlated("a"), correlated("b")]);
        let mut extended_parameters = pair.parameters.clone();
        extended_parameters.push(ScenarioParameter {
            name: "c".to_string(),
            unit: "W".to_string(),
            target: override_target("all", "tank_volume_m3"),
            distribution: uniform(0.0, 1.0),
            correlation: None,
        });
        let extended = spec(extended_parameters);

        let base = pair.sample(42, 1, 7).expect("the pair samples");
        let extended = extended
            .sample(42, 1, 7)
            .expect("the extended spec samples");
        assert_eq!(
            extended.values.len(),
            base.values.len() + 1,
            "the appended parameter joins the record's tail"
        );
        for (i, base_value) in base.values.iter().enumerate() {
            assert_eq!(
                extended.values[i], *base_value,
                "appending parameter '{}' moved parameter '{}'s draw",
                extended.values[2].name, base_value.name
            );
        }
        assert_ne!(
            extended.values[0].value, extended.values[1].value,
            "the two correlated members' draws are distinct values"
        );
    }

    /// The declaration-order contract covers a new correlation group: a
    /// parameter appended with a group of its own draws the group's shared
    /// factor at that parameter's own position, so every value already
    /// recorded stands. Drawing all groups' factors ahead of the values
    /// (the first-appearance order) would move every earlier value when a
    /// study appends a parameter that opens a group.
    #[test]
    fn appending_a_new_correlation_group_moves_no_earlier_draw() {
        let member = |name: &str, group: &str| ScenarioParameter {
            name: name.to_string(),
            unit: "W".to_string(),
            target: override_target("all", "heating_capacity_w"),
            distribution: uniform(0.0, 1.0),
            correlation: Some(ScenarioCorrelation {
                group: group.to_string(),
                rho: 0.9,
            }),
        };
        let base = spec(vec![member("a", "load"), member("b", "load")]);
        let extended = spec(vec![
            member("a", "load"),
            member("b", "load"),
            member("c", "weather"),
        ]);

        let base = base.sample(42, 1, 7).expect("the base spec samples");
        let extended = extended
            .sample(42, 1, 7)
            .expect("the extended spec samples");
        assert_eq!(
            extended.values.len(),
            base.values.len() + 1,
            "the appended parameter joins the record's tail"
        );
        for (i, base_value) in base.values.iter().enumerate() {
            assert_eq!(
                extended.values[i], *base_value,
                "appending the new group's parameter 'c' moved parameter \
                 '{}'s draw",
                base_value.name
            );
        }
    }

    /// The KS witness: the empirical CDF of the sampled fleet conforms
    /// to the declared CDF (the max deviation within the N=4,000
    /// Kolmogorov band, the 1 percent critical value about 0.026, with
    /// margin).
    #[test]
    fn sampled_cdf_conforms_to_the_spec() {
        type CdfCase<'a> = (&'a str, ScenarioDistribution, Box<dyn Fn(f64) -> f64>);
        let n = 4000_u64;
        let cases: Vec<CdfCase> = vec![
            (
                "uniform",
                uniform(1.0, 5.0),
                Box::new(|x: f64| ((x - 1.0) / 4.0).clamp(0.0, 1.0)),
            ),
            (
                "normal",
                ScenarioDistribution::Normal {
                    mean: 0.0,
                    std_dev: 1.0,
                },
                Box::new(normal_cdf),
            ),
            (
                "lognormal",
                ScenarioDistribution::LogNormal {
                    mu: 0.0,
                    sigma: 0.5,
                },
                Box::new(|x: f64| normal_cdf(libm::log(x) / 0.5)),
            ),
        ];
        for (name, distribution, cdf) in cases {
            let s = spec(vec![parameter("p", distribution)]);
            let mut values: Vec<f64> = (0..n)
                .map(|i| s.sample(11, i, 0).expect("samples").values[0].value)
                .collect();
            values.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
            let mut d = 0.0_f64;
            for (i, v) in values.iter().enumerate() {
                let empirical = (i as f64 + 1.0) / n as f64;
                let theoretical = cdf(*v);
                d = d.max((empirical - theoretical).abs());
                d = d.max((theoretical - i as f64 / n as f64).abs());
            }
            assert!(
                d < 0.035,
                "the {name} family's empirical CDF left the declared CDF: KS = {d}"
            );
        }
    }

    /// The correlation witness: a group's members' sampled values move
    /// together (the sample Pearson correlation of two rho 0.9 members
    /// over 2,000 homes is above 0.5), while uncorrelated parameters do
    /// not (the same witness on two independent parameters stays below
    /// 0.1 in magnitude, flaky-guarded at 0.15 with fixed seeds).
    #[test]
    fn correlated_members_move_together() {
        let make = |corr: Option<ScenarioCorrelation>| {
            spec(vec![
                ScenarioParameter {
                    name: "a".into(),
                    unit: "W".into(),
                    target: override_target("all", "heating_capacity_w"),
                    distribution: uniform(0.0, 1.0),
                    correlation: corr.clone(),
                },
                ScenarioParameter {
                    name: "b".into(),
                    unit: "W".into(),
                    target: override_target("all", "tank_volume_m3"),
                    distribution: uniform(0.0, 1.0),
                    correlation: corr,
                },
            ])
        };
        let pearson = |s: &ScenarioSpec| {
            let n = 2000_i64;
            let mut sum_ab = 0.0;
            let mut sum_a = 0.0;
            let mut sum_b = 0.0;
            let mut sum_a2 = 0.0;
            let mut sum_b2 = 0.0;
            for i in 0..n {
                let sample = s
                    .sample(5, u64::try_from(i).unwrap_or(0), 0)
                    .expect("samples");
                let a = sample.values[0].value;
                let b = sample.values[1].value;
                sum_ab += a * b;
                sum_a += a;
                sum_b += b;
                sum_a2 += a * a;
                sum_b2 += b * b;
            }
            let nf = n as f64;
            let cov = sum_ab / nf - (sum_a / nf) * (sum_b / nf);
            let var_a = sum_a2 / nf - (sum_a / nf) * (sum_a / nf);
            let var_b = sum_b2 / nf - (sum_b / nf) * (sum_b / nf);
            cov / libm::sqrt(var_a * var_b)
        };
        let correlated = make(Some(ScenarioCorrelation {
            group: "load".into(),
            rho: 0.9,
        }));
        let rho = pearson(&correlated);
        assert!(
            rho > 0.5,
            "the correlated members' rho {rho} must exceed 0.5"
        );
        let independent = make(None);
        let rho = pearson(&independent);
        assert!(
            rho.abs() < 0.15,
            "the independent parameters' rho {rho} must stay near zero"
        );
    }

    /// A draw the declared distribution's support does not bound is an
    /// error naming the parameter: an infinite value would reach the
    /// overrides form as JSON null (serde_json maps a non-finite f64 to
    /// Null), and the assembly would then reject a null override without
    /// naming the parameter that overflowed.
    #[test]
    fn a_non_finite_draw_is_an_error_naming_the_parameter() {
        let s = spec(vec![ScenarioParameter {
            name: "overflowing".into(),
            unit: "W".into(),
            target: override_target("all", "heating_capacity_w"),
            // exp(800 + z) is +inf for every z near zero: the declared
            // mu is finite, so validation accepts it.
            distribution: ScenarioDistribution::LogNormal {
                mu: 800.0,
                sigma: 1.0,
            },
            correlation: None,
        }]);
        let err = s
            .sample(1, 1, 1)
            .expect_err("an infinite draw must not be silently carried");
        assert!(
            err.to_string().contains("overflowing"),
            "the error must name the parameter: {err}"
        );
    }

    /// The copula preserves the declared marginal: a correlated member's
    /// values still conform to its own distribution. Two uniform (0, 1)
    /// members in one rho 0.9 group draw values whose empirical CDF
    /// stays inside the Kolmogorov band of U(0, 1) at 2,000 homes (the 1
    /// percent critical value is 1.63/sqrt(2,000) = 0.0365), and whose
    /// Pearson correlation is the Gaussian copula's closed form for
    /// uniform marginals, (6/pi) asin(rho/2) = 0.891456 at rho 0.9
    /// (Kruskal 1958: for uniform marginals Pearson and Spearman
    /// coincide), within 0.02 (the correlation's standard error is about
    /// (1 - rho^2)/sqrt(N) = 0.0042). The latent factors correlate at
    /// rho; the values correlate at the marginal transform of it, which
    /// is what the sampled record carries.
    #[test]
    fn correlated_members_keep_their_declared_marginal() {
        let s = spec(vec![
            ScenarioParameter {
                name: "a".into(),
                unit: "W".into(),
                target: override_target("all", "heating_capacity_w"),
                distribution: uniform(0.0, 1.0),
                correlation: Some(ScenarioCorrelation {
                    group: "load".into(),
                    rho: 0.9,
                }),
            },
            ScenarioParameter {
                name: "b".into(),
                unit: "W".into(),
                target: override_target("all", "tank_volume_m3"),
                distribution: uniform(0.0, 1.0),
                correlation: Some(ScenarioCorrelation {
                    group: "load".into(),
                    rho: 0.9,
                }),
            },
        ]);
        let n = 2_000_u64;
        let mut a: Vec<f64> = Vec::with_capacity(n as usize);
        let mut b: Vec<f64> = Vec::with_capacity(n as usize);
        for i in 0..n {
            let sample = s.sample(5, i, 0).expect("samples");
            a.push(sample.values[0].value);
            b.push(sample.values[1].value);
        }
        // The empirical CDF of one member against U(0, 1).
        let mut sorted = a.clone();
        sorted.sort_by(|x, y| x.partial_cmp(y).expect("finite"));
        let mut d = 0.0_f64;
        for (i, v) in sorted.iter().enumerate() {
            d = d.max(((i as f64 + 1.0) / n as f64 - v).abs());
            d = d.max((v - i as f64 / n as f64).abs());
        }
        assert!(
            d < 0.04,
            "the correlated member's empirical CDF left U(0, 1): KS = {d}"
        );
        let nf = n as f64;
        let mean_a = a.iter().sum::<f64>() / nf;
        let mean_b = b.iter().sum::<f64>() / nf;
        let cov = a
            .iter()
            .zip(&b)
            .map(|(x, y)| (x - mean_a) * (y - mean_b))
            .sum::<f64>()
            / nf;
        let sd_a = libm::sqrt(a.iter().map(|x| (x - mean_a) * (x - mean_a)).sum::<f64>() / nf);
        let sd_b = libm::sqrt(b.iter().map(|y| (y - mean_b) * (y - mean_b)).sum::<f64>() / nf);
        let rho = cov / (sd_a * sd_b);
        let expected = (6.0 / std::f64::consts::PI) * libm::asin(0.9 / 2.0);
        assert!(
            (rho - expected).abs() < 0.02,
            "the uniform pair's Pearson correlation {rho} left the Gaussian \
             copula's closed form {expected}"
        );
    }

    /// The sampled record's forms: the overrides map merges per
    /// equipment without clobbering, the patches list carries the patch
    /// targets, and the record serializes to JSON and back.
    #[test]
    fn sample_forms_round_trip() {
        let s = spec(vec![
            ScenarioParameter {
                name: "wh_capacity".into(),
                unit: "W".into(),
                target: override_target("Water Heater 1", "heating_capacity_w"),
                distribution: uniform(4000.0, 6000.0),
                correlation: None,
            },
            ScenarioParameter {
                name: "bedrooms".into(),
                unit: "count".into(),
                target: ScenarioTarget::Patch {
                    field: "number_of_bedrooms".into(),
                },
                distribution: uniform(2.0, 5.0),
                correlation: None,
            },
            ScenarioParameter {
                name: "all_scale".into(),
                unit: "W".into(),
                target: override_target("Water Heater 1", "setpoint_c"),
                distribution: uniform(45.0, 55.0),
                correlation: None,
            },
        ]);
        let sample = s.sample(42, 1, 7).expect("samples");
        let overrides = sample.overrides_json();
        let map = overrides.as_object().expect("the overrides map");
        assert_eq!(map.len(), 1, "one equipment key, both fields merged");
        let wh = map
            .get("Water Heater 1")
            .and_then(|v| v.as_object())
            .expect("the water heater's map");
        assert_eq!(
            wh.len(),
            2,
            "the two override fields merged, none clobbered"
        );
        let patches = sample.patches();
        assert_eq!(patches.len(), 1);
        assert_eq!(patches[0].0, "number_of_bedrooms");
        // The record serializes to JSON and back, bitwise on the values.
        let json = serde_json::to_string(&sample).expect("serializes");
        let back: ScenarioSample = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(back, sample);
    }

    /// A malformed distribution is an error naming it, never a panic or a
    /// substitute value: an empty discrete has no value to draw or to
    /// quantile, and both public paths validate before they index.
    #[test]
    fn a_malformed_discrete_is_an_error_not_a_panic() {
        let dist = ScenarioDistribution::Discrete {
            values: Vec::new(),
            weights: Vec::new(),
        };
        let mut rng = RngStream::scenario_sampler([0_u8; 32]).rng_at(0);
        let err = dist
            .sample(&mut rng)
            .expect_err("an empty discrete has no value to draw");
        assert!(
            err.to_string().contains("discrete"),
            "the error must name the distribution: {err}"
        );
        let err = dist
            .quantile(0.5)
            .expect_err("an empty discrete has no value to quantile");
        assert!(
            err.to_string().contains("discrete"),
            "the error must name the distribution: {err}"
        );
    }

    /// The spec's loud errors: a repeated name, a rho outside [0, 1), a
    /// malformed distribution, and an unknown patch field all fail
    /// validation naming the parameter.
    #[test]
    fn invalid_specs_are_rejected() {
        let dup = spec(vec![
            parameter("p", uniform(0.0, 1.0)),
            parameter("p", uniform(0.0, 2.0)),
        ]);
        assert!(
            dup.validate().is_err(),
            "a repeated name must fail validation"
        );
        let bad_rho = spec(vec![ScenarioParameter {
            name: "p".into(),
            unit: "W".into(),
            target: override_target("all", "x"),
            distribution: uniform(0.0, 1.0),
            correlation: Some(ScenarioCorrelation {
                group: "g".into(),
                rho: 1.0,
            }),
        }]);
        assert!(
            bad_rho.validate().is_err(),
            "a rho of 1.0 must fail validation"
        );
        let bad_uniform = spec(vec![parameter("p", uniform(5.0, 1.0))]);
        assert!(
            bad_uniform.validate().is_err(),
            "an inverted uniform must fail validation"
        );
        let bad_patch = spec(vec![ScenarioParameter {
            name: "p".into(),
            unit: "count".into(),
            target: ScenarioTarget::Patch {
                field: "not_a_field".into(),
            },
            distribution: uniform(0.0, 1.0),
            correlation: None,
        }]);
        assert!(
            bad_patch.validate().is_err(),
            "an unknown patch field must fail validation"
        );
        // The sampler validates too: a bad spec's sample is an error.
        assert!(dup.sample(1, 1, 1).is_err());
    }

    /// The inverse CDF against the published standard normal quantiles:
    /// the reference values are Wichura's AS241 (1988) quantiles from
    /// Python's `statistics.NormalDist.inv_cdf`, a different rational
    /// approximation, printed as the correctly rounded doubles. Acklam's
    /// published bound is 1.15e-9 relative; the band here is 1.5e-9 (the
    /// grid's worst measurement is 1.13e-9 at p = 0.02425, the mid/tail
    /// branch boundary, leaving the reference's own rounding and the
    /// tail branch's `log` in the margin). p = 0.02425 is the point a
    /// corrupted coefficient pair misses by 5.7e-8, fifty times this
    /// band.
    #[test]
    fn normal_inverse_cdf_matches_the_published_quantiles() {
        let cases = [
            (1e-12, -7.034_483_825_301_132),
            (1e-8, -5.612_001_244_174_79),
            (0.001, -3.090_232_306_167_813),
            (0.02425, -1.972_961_051_311_884_5),
            (0.3, -0.524_400_512_708_040_7),
            (0.7, 0.524_400_512_708_040_7),
            (0.97575, 1.972_961_051_311_884_7),
            (0.999, 3.090_232_306_167_813),
            (1.0 - 1e-8, 5.612_001_243_305_506),
            (1.0 - 1e-12, 7.034_486_910_047_836),
        ];
        for (p, reference) in cases {
            let q = normal_inverse_cdf(p);
            let relative = (q - reference).abs() / reference.abs();
            assert!(
                relative < 1.5e-9,
                "Q({p}) = {q} is {relative} relative from the published \
                 quantile {reference}"
            );
        }
    }

    /// The inverse normal CDF's accuracy: the composition
    /// Phi(Q(p)) returns p to 1e-8, and the symmetry Q(1-p) = -Q(p)
    /// holds to the same tolerance, at points across the interval.
    #[test]
    fn normal_inverse_cdf_is_accurate_and_symmetric() {
        for p in [0.001_f64, 0.024_25, 0.1, 0.25, 0.5, 0.75, 0.9, 0.975, 0.999] {
            let q = normal_inverse_cdf(p);
            let back = normal_cdf(q);
            assert!(
                (back - p).abs() < 1e-8,
                "Phi(Q({p})) = {back} left the 1e-8 band"
            );
            let mirrored = normal_inverse_cdf(1.0 - p);
            assert!(
                (mirrored + q).abs() < 1e-8,
                "Q(1 - {p}) = {mirrored} broke the symmetry with {q}"
            );
        }
    }

    /// The triangular family's analytical mean is (min + mode + max) / 3,
    /// not the mode: the two coincide only on a symmetric triangle. The
    /// witness uses an asymmetric triangle (min 1.0, mode 1.2, max 5.0;
    /// mean (1.0 + 1.2 + 5.0) / 3 = 2.4) whose mode 1.2 is far from it,
    /// and compares the sampled fleet against the same 2.4: the sampler's
    /// triangular path is the quantile inversion, so both paths must
    /// agree with the closed-form mean.
    #[test]
    fn triangular_mean_is_the_analytic_mean() {
        let asymmetric = ScenarioDistribution::Triangular {
            min: 1.0,
            mode: 1.2,
            max: 5.0,
        };
        assert!(
            (asymmetric.mean() - 2.4).abs() < 1e-12,
            "the triangular mean is (min + mode + max) / 3 = 2.4, got {}",
            asymmetric.mean()
        );
        // The triangular variance is
        // (a^2 + b^2 + c^2 - ab - bc - ca) / 18 for (a, b, c) =
        // (min, mode, max) = (1.0, 1.2, 5.0): 0.84666... so the mean's
        // standard error at 4,000 draws is 0.92 / sqrt(4,000) = 0.0146;
        // the 0.04 band is under three standard errors.
        let s = spec(vec![parameter("p", asymmetric)]);
        let n = 4000_u64;
        let total: f64 = (0..n)
            .map(|i| s.sample(13, i, 0).expect("samples").values[0].value)
            .sum();
        let mean = total / n as f64;
        assert!(
            (mean - 2.4).abs() < 0.04,
            "the sampled triangular fleet's mean {mean} left the analytical \
             mean 2.4 (three standard errors of 0.0146)"
        );
    }

    /// The triangular quantile's shape: the mode's probability is
    /// (mode - min) / (max - min), the quantile is continuous there, and
    /// the endpoints return the min and the max.
    #[test]
    fn triangular_quantile_is_the_inversion() {
        let (min, mode, max) = (1.0_f64, 2.0_f64, 6.0_f64);
        let p_mode = (mode - min) / (max - min);
        assert!((triangular_quantile(min, mode, max, p_mode) - mode).abs() < 1e-12);
        assert!((triangular_quantile(min, mode, max, 0.0) - min).abs() < 1e-12);
        assert!((triangular_quantile(min, mode, max, 1.0) - max).abs() < 1e-12);
        let just_below = triangular_quantile(min, mode, max, p_mode - 1e-9);
        let just_above = triangular_quantile(min, mode, max, p_mode + 1e-9);
        assert!(
            (just_below - just_above).abs() < 1e-4,
            "the mode is continuous"
        );
    }
}
