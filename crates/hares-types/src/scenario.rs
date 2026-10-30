//! The scenario spec and the sampler: a declared, typed spec of
//! per-parameter distributions over the dwelling's overridable surfaces
//! (the envelope scales, the equipment configs and the HPXML data
//! patches) and the deterministic sampler that turns
//! `(master_seed, scenario_id, bldg_id)` into each home's parameter set. A
//! target must name a surface the assembly's override path accepts: the
//! `Occupancy` spec is handled outside that path (its internal-gain load
//! is applied directly in the simulation loop), and the assembly refuses
//! an override of it exactly as it refuses a hand-written one, so an
//! occupancy count is not a scenario target here; the behavioral
//! occupancy is the generated schedule's own diversity dimension.
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
//! values already recorded (a member appended to an existing group reads
//! the group's already-drawn factor); the draws are the declaration
//! order's alone, so inserting or reordering a parameter moves every
//! draw from the first changed position on. A correlated member's value
//! is its
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

    /// The family's name, for the errors that name the distribution.
    fn family(&self) -> &'static str {
        match self {
            Self::Uniform { .. } => "uniform",
            Self::Normal { .. } => "normal",
            Self::LogNormal { .. } => "lognormal",
            Self::Triangular { .. } => "triangular",
            Self::Discrete { .. } => "discrete",
        }
    }

    /// The analytical mean the moment witnesses compare against. The
    /// distribution validates before it computes, so a malformed one is
    /// an error naming it, never a substitute value: a discrete with no
    /// positive weight has no mean to stand in for one. A validated
    /// distribution's closed form can still leave the finite f64 range
    /// (a corner sum, an exponentiation, a weight product on an absurd
    /// finite support), and an IEEE infinity or NaN from a public method
    /// is a silent substitute value, so the result must be finite or the
    /// error names the family and the parameters.
    ///
    /// The gates are per method, not one predicate over the distribution:
    /// each method refuses exactly the overflow its own arithmetic reaches
    /// on parameters validation accepts. A uniform on [-1.5e308, 1.5e308]
    /// means the representable 0 (its corners sum to exactly 0) while its
    /// half-span overflows, so it draws and quantiles neither; a discrete
    /// whose weights' sum overflows refuses all three, because that sum is
    /// both the probability comparison's denominator and this mean's
    /// normaliser, and dividing by an infinity would return a finite,
    /// wrong mean.
    ///
    /// The finite gate passes values the f64 fold has already rounded, and
    /// that accuracy is the contract: the corners' sum and the discrete's
    /// value-weight quotients' fold carry an absolute rounding error
    /// bounded by the largest term's own ulp scale, so the mean's relative
    /// error is unbounded only where the true mean sits inside that
    /// absolute error, which is a support whose terms nearly cancel at the
    /// exponent boundary (a triangular on [-1e308, 1e-10, 1e308] computes
    /// the fold's 0.0 where the exact mean is 3.3e-11, the uniform's
    /// symmetric corner sum's own class one corner further). The sampler
    /// never reads the mean (the draws ride the quantile), and no shipped
    /// support approaches the exponent range; the boundary's fold values
    /// are pinned (`the_means_near_cancellation_boundary_is_the_folds_own_value`).
    ///
    /// # Errors
    ///
    /// [`HaresError`] when the distribution's parameters are malformed or
    /// the closed form leaves the finite f64 range.
    pub fn mean(&self) -> Result<f64, HaresError> {
        self.validate()?;
        let mean = match self {
            Self::Uniform { low, high } => (low + high) / 2.0,
            Self::Normal { mean, .. } => *mean,
            // The triangular mean is the average of its three corners; the
            // mode is the mean only when the triangle is symmetric.
            Self::Triangular { min, mode, max } => (min + mode + max) / 3.0,
            Self::LogNormal { mu, sigma } => libm::exp(mu + sigma * sigma / 2.0),
            // Validation holds at least one positive finite weight, so
            // the total is strictly positive here; the same overflow gate
            // the sampler's probability comparison runs guards the
            // normaliser, because dividing by an IEEE infinity collapses
            // every value-weight quotient to zero (a finite, wrong mean
            // rather than a refused one).
            Self::Discrete { values, weights } => {
                let total = self.discrete_total(weights, "mean")?;
                values
                    .iter()
                    .zip(weights.iter())
                    .map(|(v, w)| v * w / total)
                    .sum()
            }
        };
        self.finite("mean", mean)
    }

    /// The public method's result is finite: a validated distribution's
    /// closed form can still leave the f64 range (the sampler's own
    /// non-finite refusal would then have nothing left to refuse), and an
    /// IEEE infinity or NaN from a public method is a silent substitute
    /// value. The error names the family and the parameters.
    fn finite(&self, method: &str, value: f64) -> Result<f64, HaresError> {
        if value.is_finite() {
            return Ok(value);
        }
        Err(HaresError::Equipment(format!(
            "the {} distribution's {method} is not finite ({value}): the closed form's \
             arithmetic on the declared parameters {self:?} leaves the finite f64 range",
            self.family()
        )))
    }

    /// Draw one value directly from the distribution: the uncorrelated
    /// parameter's path. The distribution validates before it draws, so a
    /// malformed one is an error naming it, never a panic or a
    /// substitute value, and a closed form that leaves the finite f64
    /// range (an exponentiation on an absurd finite support) is refused
    /// the same way: a public draw is finite or the error names the
    /// family and the parameters.
    ///
    /// # Errors
    ///
    /// [`HaresError`] when the distribution's parameters are malformed or
    /// the draw leaves the finite f64 range.
    pub fn sample(&self, rng: &mut ChaCha8Rng) -> Result<f64, HaresError> {
        self.validate()?;
        let value = match self {
            Self::Uniform { low, high } => *low + rng.random::<f64>() * (high - low),
            Self::Normal { mean, std_dev } => {
                let z = normal_standard(rng);
                mean + std_dev * z
            }
            Self::LogNormal { mu, sigma } => {
                let z = normal_standard(rng);
                libm::exp(mu + sigma * z)
            }
            Self::Triangular { min, mode, max } => {
                let u: f64 = rng.random();
                triangular_quantile(*min, *mode, *max, u)
            }
            Self::Discrete { values, weights } => {
                let total = self.discrete_total(weights, "sample")?;
                let u: f64 = rng.random::<f64>() * total;
                let mut cumulative = 0.0;
                let mut chosen = None;
                for (v, w) in values.iter().zip(weights.iter()) {
                    cumulative += w;
                    if u < cumulative {
                        chosen = Some(*v);
                        break;
                    }
                }
                match chosen {
                    Some(v) => v,
                    // The draw's `u` reached the total: the last value is
                    // the boundary's own.
                    None => values.last().copied().ok_or_else(|| {
                        HaresError::Equipment(
                            "the discrete distribution carries no values to draw".to_string(),
                        )
                    })?,
                }
            }
        };
        self.finite("sample", value)
    }

    /// The distribution's quantile function at probability `p` in (0, 1):
    /// the correlated parameter's path (the one-factor Gaussian copula's
    /// transform, which preserves this marginal exactly). The distribution
    /// validates before it maps, so a malformed one is an error naming it,
    /// never a panic. A finite `p` outside (0, 1) saturates at the boundary
    /// probability: the probability is clamped to [EPSILON, 1 - EPSILON]
    /// (about 8.2 standard deviations for the normal and lognormal
    /// families), so a degenerate tail returns the boundary quantile
    /// rather than an infinity. The clamp is reachable from the copula: a
    /// normal CDF of a finite draw rounds to 1.0 at about 8.6 sigma
    /// (probability about 1e-16), and the boundary quantile is the
    /// substituted value. The saturation keeps the mapped value inside
    /// the family's own range, but the family's exponentiation can still
    /// leave the finite f64 range (a lognormal's `mu`, a uniform's
    /// half-span on an absurd finite support), and that is refused: a
    /// public quantile is finite or the error names the family and the
    /// parameters. A non-finite `p` (NaN or an infinity) is outside
    /// every probability's domain and is an error naming the distribution.
    ///
    /// # Errors
    ///
    /// [`HaresError`] when the distribution's parameters are malformed,
    /// `p` is not finite, or the mapped value leaves the finite f64 range.
    pub fn quantile(&self, p: f64) -> Result<f64, HaresError> {
        self.validate()?;
        if !p.is_finite() {
            return Err(HaresError::Equipment(format!(
                "the {} distribution's quantile requires a finite probability, got {p}",
                self.family()
            )));
        }
        let p = p.clamp(f64::EPSILON, 1.0 - f64::EPSILON);
        let value = match self {
            Self::Uniform { low, high } => low + p * (high - low),
            Self::Normal { mean, std_dev } => mean + std_dev * normal_inverse_cdf(p),
            Self::LogNormal { mu, sigma } => libm::exp(mu + sigma * normal_inverse_cdf(p)),
            Self::Triangular { min, mode, max } => triangular_quantile(*min, *mode, *max, p),
            Self::Discrete { values, weights } => {
                let total = self.discrete_total(weights, "quantile")?;
                let u = p * total;
                let mut cumulative = 0.0;
                let mut chosen = None;
                for (v, w) in values.iter().zip(weights.iter()) {
                    cumulative += w;
                    if u < cumulative {
                        chosen = Some(*v);
                        break;
                    }
                }
                match chosen {
                    Some(v) => v,
                    None => values.last().copied().ok_or_else(|| {
                        HaresError::Equipment(
                            "the discrete distribution carries no values to quantile".to_string(),
                        )
                    })?,
                }
            }
        };
        self.finite("quantile", value)
    }

    /// The discrete family's weight total: the probability comparison's
    /// denominator and the mean's normaliser. A validated discrete's
    /// weights are finite and non-negative, but their sum can still
    /// overflow to an infinity; `u = p * total` is then uncomputable
    /// (every probability would silently read the last value) and the
    /// mean's value-weight division would collapse to zero (a finite,
    /// wrong value). All three public methods run this gate, so the
    /// weights' total is one contract rather than three. The error names
    /// the parameters.
    ///
    /// The total is the weights' DECLARATION-ORDER f64 fold, and that
    /// order is the contract: `sample`'s and `quantile`'s probability
    /// comparisons accumulate their `cumulative` in exactly this fold,
    /// so the gate's total and the comparisons' own sums are bitwise one
    /// value and can never disagree (a re-ordered or compensated gate
    /// sum would compute a total the comparisons do not replay). The
    /// weights are validated non-negative, so the fold is monotone: it
    /// overflows exactly when the running sum crosses the exponent
    /// range, and its order sensitivity is the accumulated rounding any
    /// f64 fold carries (a few ulps at the exponent boundary): inside
    /// that band the fold's own value is the total, and the refusal
    /// fires whenever the fold itself overflows.
    fn discrete_total(&self, weights: &[f64], method: &str) -> Result<f64, HaresError> {
        let total: f64 = weights.iter().sum();
        if total.is_finite() {
            return Ok(total);
        }
        Err(HaresError::Equipment(format!(
            "the discrete distribution's {method}: the weights' sum is {total}, outside \
             the finite f64 range the normalisation needs; the declared parameters {self:?}"
        )))
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
    /// with it, every patch target names a field the settings'
    /// patches carry, every value's name is unique (the names are the
    /// record's application keys; a repeat would apply last wins), every
    /// value's target is unique (a target is the application's map key
    /// too, so two values at one target would apply last wins and the
    /// earlier value would be dropped), and every target's name carries a
    /// value (an orphan target would be silently ignored). The sampler
    /// produces none of the faults; a hand-written record (the shape a
    /// hand-edited manifest deserializes to) can carry each of them. A
    /// record written before the target map existed carries values with no
    /// target and fails naming the value; a hand-edited record naming an
    /// unknown patch field fails naming the field. The application calls
    /// this before touching the settings, so nothing is applied and then
    /// dropped. When a record carries more than one fault, the FIRST fault
    /// is the one named: the values' walk in declaration order (a repeated
    /// name at its second occurrence, then a value with no target, then a
    /// repeated target at its second value, then an unknown patch field),
    /// then the orphan targets in the target map's order. The check
    /// refuses the whole record on the first fault it reaches, so the
    /// caller fixes that fault and the next attempt names the next.
    ///
    /// # Errors
    ///
    /// [`HaresError`] naming the value, name, target or field the record
    /// cannot apply.
    pub fn validate_application(&self) -> Result<(), HaresError> {
        let mut seen = std::collections::BTreeSet::new();
        let mut seen_targets: Vec<(&str, &ScenarioTarget)> = Vec::with_capacity(self.values.len());
        for value in &self.values {
            if !seen.insert(value.name.as_str()) {
                return Err(HaresError::Equipment(format!(
                    "the scenario sample repeats the value name '{}': the names are \
                     the record's application keys and a repeat would apply last wins",
                    value.name
                )));
            }
            let Some(target) = self.targets.get(&value.name) else {
                return Err(HaresError::Equipment(format!(
                    "the scenario sample carries '{}' with no recorded target: the value \
                     cannot be applied",
                    value.name
                )));
            };
            if let Some((earlier, _)) = seen_targets
                .iter()
                .find(|(_, earlier_target)| *earlier_target == target)
            {
                return Err(HaresError::Equipment(format!(
                    "the scenario sample's values '{earlier}' and '{}' target the same \
                     field {target:?}: the application keeps one value per target, so \
                     the earlier value would be dropped silently",
                    value.name
                )));
            }
            seen_targets.push((value.name.as_str(), target));
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
        for name in self.targets.keys() {
            if !seen.contains(name.as_str()) {
                return Err(HaresError::Equipment(format!(
                    "the scenario sample's target '{}' records no value: the target \
                     map's keys are the values' names, and an orphan target's value \
                     would be silently ignored",
                    name
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
    /// [0, 1), every patch target names a field the patches carry, and
    /// every application target carries one parameter. Every refusal names
    /// the parameter it belongs to (the distribution's own parameters and
    /// family included), so a multi-parameter spec's first fault is
    /// addressable. Two parameters
    /// writing one field would sample two values for one map key and the
    /// application would keep the later, dropping the earlier silently;
    /// targets are compared as recorded (the assembly's own wildcard
    /// refusal covers the two spellings it treats as one equipment).
    pub fn validate(&self) -> Result<(), HaresError> {
        let mut seen = std::collections::BTreeSet::new();
        let mut targets: Vec<(&str, &ScenarioTarget)> = Vec::with_capacity(self.parameters.len());
        for parameter in &self.parameters {
            parameter.distribution.validate().map_err(|err| {
                HaresError::Equipment(format!(
                    "the scenario parameter '{}' has an invalid distribution: {err}",
                    parameter.name
                ))
            })?;
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
            if let Some((earlier, _)) = targets
                .iter()
                .find(|(_, target)| *target == &parameter.target)
            {
                return Err(HaresError::Equipment(format!(
                    "the parameters '{earlier}' and '{}' target the same field {:?}: \
                     the application keeps one value per target, so the earlier \
                     sample would be dropped silently",
                    parameter.name, parameter.target
                )));
            }
            targets.push((parameter.name.as_str(), &parameter.target));
        }
        Ok(())
    }

    /// The home's sampled scenario: the parameters draw in declaration
    /// order from the sampler's own stream under the scenario-mixed
    /// per-dwelling key; a correlation group's shared factor draws at the
    /// group's first member, so appending a parameter (a new group
    /// included) moves no earlier parameter's draw, and a member appended
    /// to an existing group reads the group's already-drawn factor. The
    /// draws are the declaration order's alone: inserting or reordering
    /// a parameter moves every draw from the first changed position on.
    /// Each member's value is
    /// its distribution's quantile at the copula's probability, which
    /// preserves the declared marginal (exactly in exact arithmetic, to
    /// the CDF pair's accuracy in doubles: the inverse's 1.15e-9 relative
    /// bound, the composition's 1e-8).
    ///
    /// # Errors
    ///
    /// [`HaresError`] when the spec does not validate, or when a draw's
    /// closed form leaves the finite f64 range (the distribution's own
    /// refusal, carried through the error naming this parameter); the
    /// error names the parameter either way.
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
                None => parameter.distribution.sample(&mut rng),
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
                    parameter.distribution.quantile(normal_cdf(z))
                }
            }
            .map_err(|err| {
                HaresError::Equipment(format!(
                    "the scenario parameter '{}' cannot draw from its declared \
                     distribution: {err}",
                    parameter.name
                ))
            })?;
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

    /// A parameter's application target, unique per name: a spec declares
    /// one value per target, so multi-parameter fixtures must not collide.
    fn unique_target(name: &str) -> ScenarioTarget {
        override_target("all", &format!("field_{name}"))
    }

    fn parameter(name: &str, distribution: ScenarioDistribution) -> ScenarioParameter {
        ScenarioParameter {
            name: name.to_string(),
            unit: "W".to_string(),
            target: unique_target(name),
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

    /// The sampler is stateless: a sample is a pure function of the spec
    /// and the key. Two specs sampled interleaved, and on separate
    /// threads, are bitwise the sequential samples; a shared stream or a
    /// process-global counter anywhere in the path would move a value
    /// here.
    #[test]
    fn interleaved_and_concurrent_samples_are_the_sequential_ones() {
        let a = spec(vec![
            parameter("a", uniform(0.0, 1.0)),
            ScenarioParameter {
                correlation: Some(ScenarioCorrelation {
                    group: "g".to_string(),
                    rho: 0.5,
                }),
                ..parameter("b", uniform(0.0, 1.0))
            },
        ]);
        let b = spec(vec![parameter(
            "b",
            ScenarioDistribution::Normal {
                mean: 0.0,
                std_dev: 1.0,
            },
        )]);
        let sequential: Vec<_> = (0..32_i64)
            .map(|i| a.sample(1, i as u64, 3).expect("samples"))
            .collect();

        // Interleaved: the two specs' draws alternate on one thread.
        let mut interleaved = Vec::with_capacity(sequential.len());
        for i in 0..32_i64 {
            b.sample(1, i as u64, 3).expect("the other spec samples");
            interleaved.push(a.sample(1, i as u64, 3).expect("samples"));
        }
        assert_eq!(
            interleaved, sequential,
            "another spec's sampling between two draws must not move them"
        );

        // Concurrent: the same draws on four threads, joined in order.
        let concurrent: Vec<_> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..4)
                .map(|_| {
                    scope.spawn(|| {
                        (0..32_i64)
                            .map(|i| a.sample(1, i as u64, 3).expect("samples"))
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            let mut out = Vec::new();
            for handle in handles {
                out.push(handle.join().expect("the sampling thread joins"));
            }
            out
        })
        .into_iter()
        .flatten()
        .collect();
        assert_eq!(
            concurrent.len(),
            sequential.len() * 4,
            "every thread returns the whole sequence"
        );
        for (i, chunk) in concurrent.chunks(sequential.len()).enumerate() {
            assert_eq!(
                chunk,
                sequential.as_slice(),
                "thread {i}'s samples must be the sequential ones"
            );
        }
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
            let expected = distribution
                .mean()
                .expect("the moment witness's distribution validates");
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
            target: unique_target(name),
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
            target: unique_target(name),
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

    /// The declaration-order contract covers a member appended to an
    /// EXISTING group: the group's factor drew at the group's first
    /// member, the appended member reads that already-drawn factor, and
    /// no earlier draw moves. Drawing the group's factor again for the
    /// appended member (per-member factors) would move nothing either,
    /// but would not correlate the member with the group; the members'
    /// distinct values under one shared factor are what this pin holds.
    #[test]
    fn appending_to_an_existing_group_moves_no_earlier_draw() {
        let member = |name: &str, group: &str| ScenarioParameter {
            name: name.to_string(),
            unit: "W".to_string(),
            target: unique_target(name),
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
            member("c", "load"),
        ]);

        let base = base.sample(42, 1, 7).expect("the base spec samples");
        let extended = extended
            .sample(42, 1, 7)
            .expect("the extended spec samples");
        assert_eq!(
            extended.values.len(),
            base.values.len() + 1,
            "the appended member joins the record's tail"
        );
        for (i, base_value) in base.values.iter().enumerate() {
            assert_eq!(
                extended.values[i], *base_value,
                "appending the group member 'c' moved parameter \
                 '{}'s draw",
                base_value.name
            );
        }
        // The appended member drew: its value is the copula's transform
        // of a probability inside (0, 1), so it stays in the uniform
        // marginal's support and is distinct from its group's first
        // member's draw (the chance two copula draws land on one double
        // is the quantile grid's own, far under any witness's band).
        assert!(
            (0.0..=1.0).contains(&extended.values[2].value),
            "the appended member's value stays in the declared marginal's \
             support"
        );
        assert_ne!(
            extended.values[2].value, extended.values[0].value,
            "the appended member's draw is its group's copula transform, \
             not a repetition of the first member's"
        );
    }

    /// The stream is the declaration order's, and the guarantee is
    /// append-stability alone: a parameter INSERTED at position k leaves
    /// exactly the first k values bitwise unchanged and moves the tail
    /// (the inserted parameter's draws sit in the stream there, so every
    /// later draw shifts). The inserted parameter opens a new correlation
    /// group, so the pin trips on the group-phase layout too: a sampler
    /// that drew the groups' factors ahead of the values (the shape the
    /// fix replaced) would move the head's draws the moment a splice
    /// opened a group. A study reordering or splicing its spec re-draws
    /// from the first changed position on.
    #[test]
    fn inserting_a_parameter_moves_only_its_tail() {
        let member = |name: &str, group: &str| ScenarioParameter {
            name: name.to_string(),
            unit: "W".to_string(),
            target: unique_target(name),
            distribution: uniform(0.0, 1.0),
            correlation: Some(ScenarioCorrelation {
                group: group.to_string(),
                rho: 0.9,
            }),
        };
        let base = spec(vec![member("a", "load"), member("b", "load")]);
        let mut spliced = base.parameters.clone();
        spliced.insert(1, member("c", "weather"));
        let inserted = spec(spliced);

        let base = base.sample(42, 1, 7).expect("the base spec samples");
        let inserted = inserted.sample(42, 1, 7).expect("the spliced spec samples");
        assert_eq!(
            inserted.values[0], base.values[0],
            "the draw before the insertion position is bitwise unchanged"
        );
        assert_eq!(
            inserted.values[1].name, "c",
            "the inserted parameter sits at its position"
        );
        assert_ne!(
            inserted.values[1].value, inserted.values[0].value,
            "the inserted member's draw is its own group's copula transform"
        );
        assert_ne!(
            inserted.values[2].value, base.values[1].value,
            "the draw after the insertion moved (the inserted parameter's \
             draws sit in the stream before it)"
        );
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

    /// The copula preserves a non-uniform family's marginal too: a
    /// correlated Normal(20, 1.5) and LogNormal(0, 0.25) member's sampled
    /// values hold the declared family's mean and spread. The uniform
    /// pair's KS and Pearson witness beside this one pins the marginal's
    /// shape and its correlation; this pins the moments the record says
    /// the moment witnesses cover, correlated parameters included.
    #[test]
    fn correlated_members_hold_their_declared_family_moments() {
        let member = |name: &str, distribution: ScenarioDistribution| ScenarioParameter {
            name: name.to_string(),
            unit: "W".to_string(),
            target: unique_target(name),
            distribution,
            correlation: Some(ScenarioCorrelation {
                group: "load".to_string(),
                rho: 0.9,
            }),
        };
        let s = spec(vec![
            member(
                "normal",
                ScenarioDistribution::Normal {
                    mean: 20.0,
                    std_dev: 1.5,
                },
            ),
            member(
                "lognormal",
                ScenarioDistribution::LogNormal {
                    mu: 0.0,
                    sigma: 0.25,
                },
            ),
        ]);
        let n = 4_000_u64;
        let mut normal = Vec::with_capacity(n as usize);
        let mut lognormal = Vec::with_capacity(n as usize);
        for i in 0..n {
            let sample = s.sample(6, i, 0).expect("samples");
            normal.push(sample.values[0].value);
            lognormal.push(sample.values[1].value);
        }
        let mean = |v: &[f64]| v.iter().sum::<f64>() / n as f64;
        let sd = |v: &[f64]| {
            let m = mean(v);
            libm::sqrt(v.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / n as f64)
        };
        // Normal(20, 1.5): the mean's standard error is 1.5/sqrt(4,000) =
        // 0.0237, so the 0.1 band is four standard errors; the sd's is
        // about 1.5/sqrt(2*4,000) = 0.0168, so the 0.075 band is four and
        // a half.
        let normal_mean = mean(&normal);
        assert!(
            (normal_mean - 20.0).abs() < 0.1,
            "the correlated normal member's mean {normal_mean} left 20"
        );
        let normal_sd = sd(&normal);
        assert!(
            (normal_sd - 1.5).abs() < 0.075,
            "the correlated normal member's spread {normal_sd} left 1.5"
        );
        // LogNormal(0, 0.25): mean exp(sigma^2/2) = 1.0317434074991028 and
        // sd exp(sigma^2/2)*sqrt(exp(sigma^2)-1) = 0.2620190721092012;
        // the mean's standard error is 0.0041 (the 0.02 band is five) and
        // the sd's about 0.006 at this kurtosis (the 0.04 band is six).
        let lognormal_mean = mean(&lognormal);
        assert!(
            (lognormal_mean - 1.031_743_407_499_102_8).abs() < 0.02,
            "the correlated lognormal member's mean {lognormal_mean} left the \
             analytical mean"
        );
        let lognormal_sd = sd(&lognormal);
        assert!(
            (lognormal_sd - 0.262_019_072_109_201_2).abs() < 0.04,
            "the correlated lognormal member's spread {lognormal_sd} left the \
             analytical spread"
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

    /// A hand-written record (the shape a hand-edited manifest
    /// deserializes to) can carry two values under one name or a target
    /// whose name carries no value; the sampler produces neither shape,
    /// but the application's contract is the record's own check, so both
    /// are refused naming the offense: a repeated name because the names
    /// are the record's application keys and a repeat would apply last
    /// wins, an orphan target because its value would be silently
    /// ignored.
    #[test]
    fn a_hand_written_records_duplicate_name_and_orphan_target_are_refused() {
        let duplicate: ScenarioSample = serde_json::from_value(serde_json::json!({
            "scenario_id": 1,
            "bldg_id": 7,
            "values": [
                {"name": "bedrooms", "value": 3.0, "unit": "count"},
                {"name": "bedrooms", "value": 4.0, "unit": "count"}
            ],
            "targets": {"bedrooms": {"patch": {"field": "number_of_bedrooms"}}}
        }))
        .expect("the record parses");
        let err = duplicate
            .validate_application()
            .expect_err("a repeated value name must be refused");
        assert!(
            err.to_string().contains("bedrooms") && err.to_string().contains("repeats"),
            "the error must name the repeated value: {err}"
        );

        let orphan: ScenarioSample = serde_json::from_value(serde_json::json!({
            "scenario_id": 1,
            "bldg_id": 7,
            "values": [{"name": "bedrooms", "value": 3.0, "unit": "count"}],
            "targets": {
                "bedrooms": {"patch": {"field": "number_of_bedrooms"}},
                "ghost": {"override": {"equipment": "MELs", "field": "usage_multiplier"}}
            }
        }))
        .expect("the record parses");
        let err = orphan
            .validate_application()
            .expect_err("a target with no value must be refused");
        assert!(
            err.to_string().contains("ghost"),
            "the error must name the orphan target: {err}"
        );
    }

    /// Two values at one application target would apply last wins, the
    /// earlier value dropped silently: a target is the application's map
    /// key exactly as a name is, so the record refuses the shape naming
    /// both values, and the spec refuses the authoring shape at validation
    /// naming both parameters (the sampler therefore cannot produce it).
    #[test]
    fn two_values_at_one_target_are_refused_naming_both() {
        let record: ScenarioSample = serde_json::from_value(serde_json::json!({
            "scenario_id": 1,
            "bldg_id": 7,
            "values": [
                {"name": "a", "value": 0.8, "unit": "fraction"},
                {"name": "b", "value": 1.2, "unit": "fraction"}
            ],
            "targets": {
                "a": {"override": {"equipment": "MELs", "field": "usage_multiplier"}},
                "b": {"override": {"equipment": "MELs", "field": "usage_multiplier"}}
            }
        }))
        .expect("the record parses");
        let err = record
            .validate_application()
            .expect_err("two values at one target must be refused");
        assert!(
            err.to_string().contains("'a'") && err.to_string().contains("'b'"),
            "the error must name both values: {err}"
        );

        let target = || ScenarioTarget::Override {
            equipment: "MELs".to_string(),
            field: "usage_multiplier".to_string(),
        };
        let spec = ScenarioSpec {
            name: "same-target".to_string(),
            parameters: vec![
                ScenarioParameter {
                    name: "a".to_string(),
                    unit: "fraction".to_string(),
                    target: target(),
                    distribution: uniform(0.7, 1.0),
                    correlation: None,
                },
                ScenarioParameter {
                    name: "b".to_string(),
                    unit: "fraction".to_string(),
                    target: target(),
                    distribution: uniform(1.0, 1.3),
                    correlation: None,
                },
            ],
        };
        let err = spec
            .validate()
            .expect_err("two parameters at one target must fail validation");
        assert!(
            err.to_string().contains("'a'") && err.to_string().contains("'b'"),
            "the error must name both parameters: {err}"
        );
        assert!(
            spec.sample(1, 1, 1).is_err(),
            "the sampler must refuse the spec it cannot apply"
        );

        // The check is joint over the spec's parameters, correlated or
        // not: two members of one correlation group share the group's
        // factor and keep one target each, so two correlated parameters
        // written to one target hit the same refusal. (One parameter
        // carrying two targets is not a shape the spec can author: the
        // type holds exactly one target per parameter.)
        let correlated = ScenarioSpec {
            name: "correlated-same-target".to_string(),
            parameters: vec![
                ScenarioParameter {
                    name: "a".to_string(),
                    unit: "fraction".to_string(),
                    target: target(),
                    distribution: uniform(0.7, 1.0),
                    correlation: Some(ScenarioCorrelation {
                        group: "load".to_string(),
                        rho: 0.9,
                    }),
                },
                ScenarioParameter {
                    name: "b".to_string(),
                    unit: "fraction".to_string(),
                    target: target(),
                    distribution: uniform(1.0, 1.3),
                    correlation: Some(ScenarioCorrelation {
                        group: "load".to_string(),
                        rho: 0.9,
                    }),
                },
            ],
        };
        let err = correlated
            .validate()
            .expect_err("two correlated parameters at one target must fail validation");
        assert!(
            err.to_string().contains("'a'") && err.to_string().contains("'b'"),
            "the error must name both correlated parameters: {err}"
        );
        assert!(
            correlated.sample(1, 1, 1).is_err(),
            "the sampler must refuse the correlated spec it cannot apply"
        );
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

    /// Every public distribution method refuses a malformed distribution
    /// and none of them panics: `validate`, `mean`, `sample` and
    /// `quantile` each run the same validation before they index, divide
    /// or map, so the family's whole public surface names the malformed
    /// distribution instead of panicking or substituting a value. The
    /// table is every family against the malformation classes (a
    /// non-finite parameter, a degenerate or inverted support, an empty
    /// or mismatched discrete, a negative or mass-less weight list).
    #[test]
    fn every_public_distribution_method_refuses_a_malformed_distribution() {
        let malformed = [
            ScenarioDistribution::Uniform {
                low: 1.0,
                high: 1.0,
            },
            ScenarioDistribution::Uniform {
                low: f64::NAN,
                high: 1.0,
            },
            ScenarioDistribution::Uniform {
                low: 0.0,
                high: f64::INFINITY,
            },
            ScenarioDistribution::Normal {
                mean: 0.0,
                std_dev: -1.0,
            },
            ScenarioDistribution::Normal {
                mean: f64::NAN,
                std_dev: 1.0,
            },
            ScenarioDistribution::LogNormal {
                mu: 0.0,
                sigma: 0.0,
            },
            ScenarioDistribution::LogNormal {
                mu: f64::INFINITY,
                sigma: 1.0,
            },
            ScenarioDistribution::Triangular {
                min: 1.0,
                mode: 0.0,
                max: 2.0,
            },
            ScenarioDistribution::Triangular {
                min: 1.0,
                mode: 1.0,
                max: 1.0,
            },
            ScenarioDistribution::Triangular {
                min: 1.0,
                mode: 1.0,
                max: f64::NAN,
            },
            ScenarioDistribution::Discrete {
                values: Vec::new(),
                weights: Vec::new(),
            },
            ScenarioDistribution::Discrete {
                values: vec![1.0],
                weights: Vec::new(),
            },
            ScenarioDistribution::Discrete {
                values: vec![1.0, 2.0],
                weights: vec![1.0, -1.0],
            },
            ScenarioDistribution::Discrete {
                values: vec![f64::NAN],
                weights: vec![1.0],
            },
            ScenarioDistribution::Discrete {
                values: vec![1.0, 2.0],
                weights: vec![0.0, 0.0],
            },
        ];
        let mut rng = RngStream::scenario_sampler([0_u8; 32]).rng_at(0);
        for distribution in malformed {
            let family = match &distribution {
                ScenarioDistribution::Uniform { .. } => "uniform",
                ScenarioDistribution::Normal { .. } => "normal",
                ScenarioDistribution::LogNormal { .. } => "lognormal",
                ScenarioDistribution::Triangular { .. } => "triangular",
                ScenarioDistribution::Discrete { .. } => "discrete",
            };
            assert!(
                distribution.validate().is_err(),
                "{distribution:?} must not validate"
            );
            for (method, error) in [
                ("mean", distribution.mean().expect_err("refused")),
                (
                    "sample",
                    distribution.sample(&mut rng).expect_err("refused"),
                ),
                ("quantile", distribution.quantile(0.5).expect_err("refused")),
            ] {
                assert!(
                    error.to_string().contains(family),
                    "{method} must name the {family} distribution: {error}"
                );
            }
        }
    }

    /// The quantile's probability contract: a finite probability outside
    /// (0, 1) saturates at the [EPSILON, 1 - EPSILON] clamp (about 8.2
    /// standard deviations for the normal family), a non-finite one is an
    /// error naming the distribution, and the saturated values are finite.
    /// The clamp is reachable from the copula: a normal CDF of a finite
    /// draw rounds to exactly 1.0 at about 8.6 sigma.
    #[test]
    fn the_quantile_saturates_a_finite_probability_and_refuses_a_non_finite_one() {
        let normal = ScenarioDistribution::Normal {
            mean: 0.0,
            std_dev: 1.0,
        };
        let low = normal
            .quantile(0.0)
            .expect("a finite probability saturates at the clamp");
        let high = normal
            .quantile(1.0)
            .expect("a finite probability saturates at the clamp");
        assert!(
            low.is_finite() && high.is_finite(),
            "the saturated quantiles are finite: {low}, {high}"
        );
        assert_eq!(
            low,
            normal
                .quantile(f64::EPSILON)
                .expect("the clamp's own probability"),
            "p at or below zero reads the clamp's lower boundary"
        );
        assert_eq!(
            high,
            normal
                .quantile(1.0 - f64::EPSILON)
                .expect("the clamp's own probability"),
            "p at or above one reads the clamp's upper boundary"
        );
        // Acklam's Q(2^-52) is 8.2095..., so the saturated tail is the
        // clamp's 8.2 standard deviations, not an infinity.
        assert!(
            (low + 8.2).abs() < 0.1 && (high - 8.2).abs() < 0.1,
            "the saturated quantiles are the clamp's 8.2 sigma: {low}, {high}"
        );
        for p in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let err = normal
                .quantile(p)
                .expect_err("a non-finite probability has no quantile");
            assert!(
                err.to_string().contains("normal") && err.to_string().contains("finite"),
                "the error must name the distribution and the finite domain: {err}"
            );
        }
    }

    /// The mean has the same arm the draw and the quantile have: a
    /// discrete with no positive weight (an empty value list, or
    /// weights that all vanish) has no mean to substitute for, so the
    /// analytical mean refuses it naming the distribution. A number
    /// returned from that arm would be a silent substitute value on a
    /// public path.
    #[test]
    fn the_mean_refuses_a_distribution_with_no_mass() {
        let empty = ScenarioDistribution::Discrete {
            values: Vec::new(),
            weights: Vec::new(),
        };
        let err = empty
            .mean()
            .expect_err("an empty discrete has no mean to return");
        assert!(
            err.to_string().contains("discrete"),
            "the error must name the distribution: {err}"
        );
        let zero_total = ScenarioDistribution::Discrete {
            values: vec![1.0, 2.0],
            weights: vec![0.0, 0.0],
        };
        let err = zero_total
            .mean()
            .expect_err("a zero-total discrete has no mean to return");
        assert!(
            err.to_string().contains("discrete"),
            "the error must name the distribution: {err}"
        );
    }

    /// A distribution whose parameters validate can still have a closed
    /// form that leaves the finite f64 range: a corner sum past the
    /// exponent range, an exponentiation past it, weight products past
    /// it. An IEEE infinity or NaN from a public method is a silent
    /// substitute value (and serde_json maps it to null downstream), so
    /// `mean`, `sample` and `quantile` refuse naming the family and the
    /// parameters. The support's own values never overflow here (a
    /// finite support's draws stay inside it): the refusal is the closed
    /// form's arithmetic, which is why the first entry's `sample` and
    /// `quantile` stay finite while its mean refuses. A discrete whose
    /// weights' sum overflows is refused outright: its probability
    /// comparison `u = p * total` is uncomputable, and every probability
    /// would silently read the last value.
    #[test]
    fn a_distribution_whose_closed_form_overflows_refuses_naming_it() {
        // Every entry validates: the parameters are finite and ordered.
        let mean_overflows = [
            (
                ScenarioDistribution::Uniform {
                    low: 1e308,
                    high: 1.5e308,
                },
                "uniform",
                "the corners' sum 2.5e308 overflows",
            ),
            (
                ScenarioDistribution::Triangular {
                    min: 1e308,
                    mode: 1.2e308,
                    max: 1.7e308,
                },
                "triangular",
                "the corners' sum 3.9e308 overflows",
            ),
            (
                ScenarioDistribution::LogNormal {
                    mu: 800.0,
                    sigma: 1.0,
                },
                "lognormal",
                "exp(800 + sigma^2/2) overflows",
            ),
            (
                ScenarioDistribution::Discrete {
                    values: vec![1e308, 1e308],
                    weights: vec![1e308, 1e308],
                },
                "discrete",
                "the value-weight products overflow",
            ),
            (
                // Each product stays finite, so only the total's overflow
                // is left: dividing by it collapses every quotient to
                // zero, and the true mean is the hand-computed
                // (1.0 * 1e308 + 0.0 * 1e308) / (1e308 + 1e308) = 0.5.
                ScenarioDistribution::Discrete {
                    values: vec![1.0, 0.0],
                    weights: vec![1e308, 1e308],
                },
                "discrete",
                "the weights' sum overflows while the value-weight products stay finite",
            ),
        ];
        for (distribution, family, why) in &mean_overflows {
            distribution.validate().unwrap_or_else(|err| {
                panic!("the parameters validate for {family} ({why}): {err}")
            });
            let error = distribution
                .mean()
                .expect_err("an overflowing closed form must refuse");
            assert!(
                error.to_string().contains(family),
                "the mean must name the {family} distribution ({why}): {error}"
            );
        }
        // The support's own values are representable: the draws and the
        // quantiles stay inside it, so the refusal above is the closed
        // form's corner sum and nothing else.
        let mut rng = RngStream::scenario_sampler([0_u8; 32]).rng_at(0);
        let drawn = mean_overflows[0]
            .0
            .clone()
            .sample(&mut rng)
            .expect("the support's values are representable");
        assert!(
            (1e308..=1.5e308).contains(&drawn),
            "the draw {drawn} stays inside the declared support"
        );

        // The quantile's and the draw's own overflow classes: an
        // exponentiation past the range (every probability, every draw:
        // the standard normal's quantile stays within about 8.2 sigma),
        // a half-span subtraction past it, and the weights' sum past it.
        let quantile_overflows = [
            (
                ScenarioDistribution::LogNormal {
                    mu: 800.0,
                    sigma: 1.0,
                },
                "lognormal",
            ),
            (
                ScenarioDistribution::Uniform {
                    low: -1.5e308,
                    high: 1.5e308,
                },
                "uniform",
            ),
            (
                ScenarioDistribution::Discrete {
                    values: vec![1.0, 2.0],
                    weights: vec![1e308, 1e308],
                },
                "discrete",
            ),
        ];
        for (distribution, family) in quantile_overflows {
            let error = distribution
                .quantile(0.5)
                .expect_err("an overflowing quantile must refuse");
            assert!(
                error.to_string().contains(family),
                "the quantile must name the {family} distribution: {error}"
            );
        }
        let sample_overflows = [
            (
                ScenarioDistribution::LogNormal {
                    mu: 800.0,
                    sigma: 1.0,
                },
                "lognormal",
            ),
            (
                ScenarioDistribution::Discrete {
                    values: vec![1.0, 2.0],
                    weights: vec![1e308, 1e308],
                },
                "discrete",
            ),
        ];
        for (distribution, family) in sample_overflows {
            let error = distribution
                .sample(&mut rng)
                .expect_err("an overflowing draw must refuse");
            assert!(
                error.to_string().contains(family),
                "the draw must name the {family} distribution: {error}"
            );
        }

        // The gates are per method, not one predicate over the
        // distribution: each refuses its own arithmetic's overflow, and
        // the stored parameters stay finite throughout. A symmetric
        // uniform's mean is the representable 0 (its corners sum to
        // exactly 0), while its half-span `high - low` overflows, so it
        // draws and quantiles neither. The discrete's weight total is
        // shared by all three, so an overflowing sum refuses every
        // method, the mean included (the hand-computed mean of the entry
        // above is 0.5, not the 0.0 an ungated division returns).
        let symmetric = ScenarioDistribution::Uniform {
            low: -1.5e308,
            high: 1.5e308,
        };
        assert_eq!(
            symmetric.mean().expect("the mean's corner sum is finite"),
            0.0,
            "the symmetric uniform's mean is representable"
        );
        let mut rng = RngStream::scenario_sampler([1_u8; 32]).rng_at(0);
        assert!(
            symmetric.sample(&mut rng).is_err(),
            "the symmetric uniform's half-span overflows: the draw must refuse"
        );
        assert!(
            symmetric.quantile(0.5).is_err(),
            "the symmetric uniform's half-span overflows: the quantile must refuse"
        );
    }

    /// The mean's near-cancellation boundary is the f64 fold's own value:
    /// the corners' sum and the discrete's value-weight quotients' fold
    /// carry the largest term's own rounding (about 2e292 at the exponent
    /// boundary), so a support whose terms nearly cancel computes the
    /// fold's value, not the exact mean's. The uniform's symmetric corner
    /// sum's 0.0 is the same contract one corner fewer (pinned beside the
    /// overflow refusals above); this pin holds the fold's values for the
    /// triangular's and the discrete's folds, so a change to a fold's
    /// order or its compensation fails here to be chosen deliberately.
    /// The exact means are (-1e308 + 1e-10 + 1e308)/3 = 3.3e-11 and the
    /// same support's quotients'/3: the computed 0.0 is the fold's
    /// rounding, inside the absolute bound the contract states on
    /// `mean`.
    #[test]
    fn the_means_near_cancellation_boundary_is_the_folds_own_value() {
        let triangular = ScenarioDistribution::Triangular {
            min: -1e308,
            mode: 1e-10,
            max: 1e308,
        };
        // The mode's 1e-10 sits below the corners' ulp: the fold loses it
        // at the first addition and computes
        // ((-1e308 + 1e-10) + 1e308) / 3 = 0.0.
        assert_eq!(
            triangular.mean().expect("the fold is finite"),
            0.0,
            "the near-cancelling triangular's mean is the f64 fold's value"
        );
        // The discrete's quotients' fold over the same support: the
        // weights' total is the exact 3, each quotient v*w/3 rounds to
        // the corners' magnitude, and the fold computes 0.0 the same way.
        let discrete = ScenarioDistribution::Discrete {
            values: vec![-1e308, 1e-10, 1e308],
            weights: vec![1.0, 1.0, 1.0],
        };
        assert_eq!(
            discrete.mean().expect("the fold is finite"),
            0.0,
            "the near-cancelling discrete's mean is the f64 fold's value"
        );
    }

    /// The spec's sampling error names the parameter the distribution's
    /// refusal belongs to: the record's consumers fix the declared
    /// parameter, not the family's abstract parameters.
    #[test]
    fn the_specs_overflow_refusal_names_the_parameter() {
        let s = spec(vec![ScenarioParameter {
            name: "huge".into(),
            unit: "W".into(),
            target: override_target("all", "heating_capacity_w"),
            distribution: ScenarioDistribution::LogNormal {
                mu: 800.0,
                sigma: 1.0,
            },
            correlation: None,
        }]);
        let err = s
            .sample(1, 1, 1)
            .expect_err("an overflowing closed form must refuse");
        assert!(
            err.to_string().contains("huge") && err.to_string().contains("lognormal"),
            "the error must name the parameter and the family: {err}"
        );
    }

    /// The overflow refusals consume no stream word: the discrete family's
    /// total gate runs before the draw, so a refused draw leaves the
    /// generator exactly where it was and the next draw is the one a
    /// fresh generator yields. The finiteness gates themselves add no
    /// arithmetic to a successful path and call no draw, so the accepted
    /// draws are the sequence the sampler had before the gates: this pin
    /// holds that sequence's values.
    #[test]
    fn the_overflow_gates_leave_the_draw_sequence_alone() {
        let mut rng = RngStream::scenario_sampler([7_u8; 32]).rng_at(0);
        let mut untouched = rng.clone();
        let overflowing = ScenarioDistribution::Discrete {
            values: vec![1.0, 2.0],
            weights: vec![1e308, 1e308],
        };
        assert!(
            overflowing.sample(&mut rng).is_err(),
            "the overflowing total must refuse"
        );
        let normal = ScenarioDistribution::Normal {
            mean: 0.0,
            std_dev: 1.0,
        };
        assert_eq!(
            normal.sample(&mut rng).expect("the accepted draw"),
            normal.sample(&mut untouched).expect("the reference draw"),
            "a refused draw must consume no stream word: the next draw is \
             the one a generator that saw no refusal yields"
        );
    }

    /// The sampler's draw sequence, pinned as literals for one fixed key:
    /// one parameter per family plus a correlated pair, so a change to
    /// the draw order, to a family's mapping or to the stream derivation
    /// moves a value here instead of passing the self-comparison pins.
    /// The values are the sequence the gates left unmoved (the accepted
    /// path's arithmetic and draw calls are unchanged).
    #[test]
    fn the_samplers_draw_sequence_is_pinned() {
        let s = spec(vec![
            parameter("u", uniform(0.0, 1.0)),
            parameter(
                "n",
                ScenarioDistribution::Normal {
                    mean: 20.0,
                    std_dev: 1.5,
                },
            ),
            parameter(
                "l",
                ScenarioDistribution::LogNormal {
                    mu: 0.0,
                    sigma: 0.25,
                },
            ),
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
                    values: vec![2.0, 3.0, 5.0],
                    weights: vec![0.2, 0.5, 0.3],
                },
            ),
            ScenarioParameter {
                correlation: Some(ScenarioCorrelation {
                    group: "load".to_string(),
                    rho: 0.5,
                }),
                ..parameter("c1", uniform(0.0, 1.0))
            },
            ScenarioParameter {
                correlation: Some(ScenarioCorrelation {
                    group: "load".to_string(),
                    rho: 0.5,
                }),
                ..parameter("c2", uniform(0.0, 1.0))
            },
        ]);
        let sample = s.sample(42, 1, 7).expect("the spec samples");
        let values: Vec<f64> = sample.values.iter().map(|value| value.value).collect();
        assert_eq!(
            values,
            vec![
                0.36856114217327696,
                20.399528715482926,
                0.736351153000037,
                2.9433325182832113,
                2.0,
                0.04149018942285346,
                0.30675311543324435,
            ],
            "the pinned draw sequence"
        );
    }

    /// A malformed distribution's refusal names the scenario parameter that
    /// carries it: a spec with several parameters must tell the author which
    /// one to fix, not only which distribution family is wrong. The sampler's
    /// draw errors already name the parameter; validation is the earlier
    /// surface and names it too.
    #[test]
    fn a_malformed_distribution_names_its_parameter() {
        let s = spec(vec![
            parameter("ok", uniform(0.0, 1.0)),
            parameter("inverted", uniform(5.0, 1.0)),
        ]);
        let err = s
            .validate()
            .expect_err("an inverted uniform must fail the spec's validation");
        assert!(
            err.to_string().contains("'inverted'"),
            "the error must name the malformed parameter: {err}"
        );
        assert!(
            err.to_string().contains("uniform"),
            "the error must name the distribution family: {err}"
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
        let err = dup
            .validate()
            .expect_err("a repeated name must fail validation");
        assert!(
            err.to_string().contains("'p'") && err.to_string().contains("unique"),
            "a repeated name must name the parameter and state the uniqueness rule: {err}"
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
        let err = bad_rho
            .validate()
            .expect_err("a rho of 1.0 must fail validation");
        assert!(
            err.to_string().contains("'p'") && err.to_string().contains("[0, 1)"),
            "the rho refusal must name the parameter and its domain: {err}"
        );
        let bad_uniform = spec(vec![parameter("p", uniform(5.0, 1.0))]);
        let err = bad_uniform
            .validate()
            .expect_err("an inverted uniform must fail validation");
        assert!(
            err.to_string().contains("'p'") && err.to_string().contains("uniform"),
            "a malformed distribution must name its parameter and family: {err}"
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
        let err = bad_patch
            .validate()
            .expect_err("an unknown patch field must fail validation");
        assert!(
            err.to_string().contains("'p'") && err.to_string().contains("not_a_field"),
            "the patch refusal must name the parameter and the field: {err}"
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
        let mean = asymmetric
            .mean()
            .expect("the triangular witness's distribution validates");
        assert!(
            (mean - 2.4).abs() < 1e-12,
            "the triangular mean is (min + mode + max) / 3 = 2.4, got {mean}"
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
