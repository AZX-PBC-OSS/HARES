//! Identity of a ChaCha8 random stream.
//!
//! A ChaCha8 generator is addressed by a 256-bit key and a 64-bit stream
//! nonce. `ChaCha8Rng::get_seed` returns the key alone, so handing a stream
//! to its consumer as a seed rebuilds it on stream 0. [`RngStream`] carries
//! both halves so every consumer draws from the stream it was given.

use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

/// Event-load streams set the top bit of the stream nonce, which keeps them
/// disjoint from the small indexed streams (the dwelling's own stream 0 and
/// the EV-driver streams counted up from 1).
pub const EVENT_LOAD_STREAM_TAG: u64 = 1 << 63;

/// The reserved identity prefix of the generated-schedule diversity
/// streams. Each diversity dimension's full identity is this prefix plus
/// the dimension's name, so the dimensions derive distinct streams under
/// one dwelling key while every dimension's stream stays tagged out of the
/// dwelling's and the EV drivers' stream space. The identities are
/// reserved: no event load's name is ever prefixed with this.
pub const SCHEDULE_DIVERSITY_IDENTITY_PREFIX: &str = "hares/schedule-diversity/v1/";

/// The reserved identity of the scenario sampler's stream: one draw
/// stream per (master seed, scenario id, building id), kept out of the
/// dwelling's own stream space by the same top-bit tag.
pub const SCENARIO_SAMPLER_IDENTITY: &str = "hares/scenario-sampler/v1";

/// One ChaCha8 stream: the generator key and the stream nonce.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RngStream {
    pub seed: [u8; 32],
    pub stream: u64,
}

/// A component's current random-stream state, read back for the
/// reproducibility checks: the generator key, the stream nonce and the
/// word position, the triple that pins the component's draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RngStreamState {
    pub seed: [u8; 32],
    pub stream: u64,
    pub word_pos: u128,
}

impl RngStream {
    /// The stream an event load with the stable `identity` draws from under
    /// the dwelling key `seed`.
    ///
    /// The nonce depends on the identity only, so adding, removing or
    /// reordering other loads never moves this load's draws.
    #[must_use]
    pub fn event_load(seed: [u8; 32], identity: &str) -> Self {
        Self {
            seed,
            stream: EVENT_LOAD_STREAM_TAG | (fnv1a_64(identity.as_bytes()) >> 1),
        }
    }

    /// The generated-schedule diversity stream of the diversity dimension
    /// `dimension` (a `SCHEDULE_DIVERSITY_IDENTITY_PREFIX`-prefixed name)
    /// under the per-dwelling key `dwelling_key`.
    ///
    /// The derivation composes the landed algebra with no new rule: the
    /// key is the caller's scenario-mixed per-dwelling key
    /// (`dwelling_seed_scenario`), whose byte layout is injective over
    /// (master seed, scenario, building) with no hash, and the stream
    /// nonce is the tagged identity hash over the dimension's reserved
    /// identity, so one dimension's draws never move another's and the
    /// scenario disjointness rides the key's own bytes. Two dimension
    /// names derive distinct nonces unless their FNV-1a words collide
    /// (the same residual probabilistic surface every identity-keyed
    /// derivation here shares); the shipped dimensions' identities are
    /// fixed constants whose distinctness the derivation tests pin.
    #[must_use]
    pub fn schedule_diversity(dwelling_key: [u8; 32], dimension: &str) -> Self {
        Self::event_load(
            dwelling_key,
            &format!("{SCHEDULE_DIVERSITY_IDENTITY_PREFIX}{dimension}"),
        )
    }

    /// The scenario sampler's draw stream for the per-dwelling key
    /// `dwelling_key` (the caller's scenario-mixed key): one stream the
    /// spec's parameters draw in declaration order.
    ///
    /// The sampler's stream is tagged (the event-load tag) and keyed on
    /// the reserved [`SCENARIO_SAMPLER_IDENTITY`] identity, so the
    /// sampler's draws never share a stream with the dwelling's own RNG
    /// (stream 0), the EV drivers (the indexed range) or any event load
    /// (whose identity is an equipment name, never the reserved
    /// constant), and the same (master seed, scenario, building) reproduces.
    #[must_use]
    pub fn scenario_sampler(dwelling_key: [u8; 32]) -> Self {
        Self::event_load(dwelling_key, SCENARIO_SAMPLER_IDENTITY)
    }

    /// A generator on this stream, `word_pos` 32-bit words from its start.
    #[must_use]
    pub fn rng_at(&self, word_pos: u128) -> ChaCha8Rng {
        let mut rng = ChaCha8Rng::from_seed(self.seed);
        rng.set_stream(self.stream);
        rng.set_word_pos(word_pos);
        rng
    }
}

/// Generator key of the dwelling with `bldg_id` under `master_seed`: bytes
/// 0..8 are `master_seed` and 8..16 are `bldg_id` (both little-endian,
/// two's complement for negative ids), the rest zero.
///
/// Contract: this derivation is where the master seed's reach ends. The
/// fleet path threads the seed here and nowhere else: the DR compliance
/// driver's draw, the registry EV driver's key material and a noisy
/// TimeWindows source's keystream key key on their own configured seed or
/// identity, and the fleet path constructs none of them. If the fleet path
/// gains actor or noisy-source construction, the master seed MUST thread
/// into their unspecified-seed derivations the same way, or re-seeding a
/// fleet would silently leave those streams unseeded.
#[must_use]
pub fn dwelling_seed(master_seed: u64, bldg_id: i64) -> [u8; 32] {
    let mut seed = [0_u8; 32];
    seed[..8].copy_from_slice(&master_seed.to_le_bytes());
    seed[8..16].copy_from_slice(&bldg_id.to_le_bytes());
    seed
}

/// The scenario byte `dwelling_seed_scenario` writes into key byte 24
/// when a scenario is declared, so a declared scenario 0 never derives
/// the key the absent scenario does.
pub const SCENARIO_PRESENCE_BYTE: u8 = 1;

/// Generator key of the dwelling with `bldg_id` under `master_seed` with
/// the scenario mixed in: the [`dwelling_seed`] layout, and, when a
/// scenario is declared, the scenario id's little-endian bytes at 16..24
/// and [`SCENARIO_PRESENCE_BYTE`] at byte 24. `None` (the absent
/// scenario) keeps today's bytes exactly: bytes 16..32 stay zero, so
/// every scenario-less key, checkpoint and fixture is unmoved.
///
/// The map is injective with no hash: two scenarios differ at bytes
/// 16..24 whatever the master seed and building id are, and a declared
/// scenario 0 differs from the absent scenario at byte 24 (the presence
/// byte), so "scenario 0" and "no scenario" never share a stream. The
/// mix lands where the master seed's reach ends, so every surface inside
/// the dwelling key (the dwelling's own stream, the built-in EV drivers'
/// streams, every event-load interior) inherits scenario disjointness
/// with no per-site change.
///
/// Named scope, not an oversight: the identity-keyed surfaces outside
/// the derivation (`schedule_source_key`, the registry EV driver's key
/// material) do not receive the scenario. Two scenarios share an
/// unspecified-seed noisy source's keystream only when both dwellings
/// address the same dwelling identity (the same building id); different
/// building ids derive different keys. The contract docs name the scope
/// and the escape (an explicit per-source seed).
#[must_use]
pub fn dwelling_seed_scenario(
    master_seed: u64,
    scenario_seed: Option<u64>,
    bldg_id: i64,
) -> [u8; 32] {
    let mut seed = dwelling_seed(master_seed, bldg_id);
    if let Some(scenario) = scenario_seed {
        seed[16..24].copy_from_slice(&scenario.to_le_bytes());
        seed[24] = SCENARIO_PRESENCE_BYTE;
    }
    seed
}

/// A schedule source's stochastic keystream key: the configured key when
/// the source carries an explicit seed, otherwise derived from the
/// source's identity (`<dwelling identity>:<owner name>:<source role>`).
/// The dwelling identity part is the caller's config-stable per-dwelling
/// value (the building id the per-dwelling stream derivations key on),
/// without which every home carrying an identically-configured
/// unspecified-seed source (HPXML names equipment by generic type
/// string) would derive the same key and draw the same series. The
/// identity's FNV-1a words fill the key the way [`RngStream::event_load`]
/// fills its stream nonce, so two sources with distinct identities never
/// share a keystream, the same identity is reproducible run to run, and
/// the default is never the all-zero key every unspecified-seed source
/// would otherwise share.
///
/// Named scope: the scenario seed does not enter this derivation. The
/// key depends on the identity alone, so two scenarios that address the
/// same dwelling identity (the same building id) and configure their
/// sources identically and unspecified-seed derive the same keys and
/// share those keystreams; dwellings with different building ids derive
/// different keys. That share is deliberate (an
/// identity-keyed key stays stable across scenario relabeling), and a
/// scenario that needs its own series for a source sets the source's
/// explicit seed. The scope is stated in the RNG contract docs.
#[must_use]
pub fn schedule_source_key(explicit: Option<[u8; 32]>, identity: &str) -> [u8; 32] {
    if let Some(key) = explicit {
        return key;
    }
    let mut key = [0_u8; 32];
    key[..8].copy_from_slice(
        &fnv1a_64_prefixed(b"hares/schedule-source/key-hi", identity.as_bytes()).to_le_bytes(),
    );
    key[8..16].copy_from_slice(
        &fnv1a_64_prefixed(b"hares/schedule-source/key-lo", identity.as_bytes()).to_le_bytes(),
    );
    key
}

/// FNV-1a 64-bit: stable across platforms and releases, unlike `std`'s
/// hasher, so stream identities are reproducible.
fn fnv1a_64(bytes: &[u8]) -> u64 {
    fnv1a_64_prefixed(b"", bytes)
}

/// FNV-1a 64-bit over a domain prefix followed by the bytes: the prefix
/// separates the derivation uses (a stream nonce here, a keystream key
/// there), so the same identity hashes differently in each.
fn fnv1a_64_prefixed(prefix: &[u8], bytes: &[u8]) -> u64 {
    let h = prefix.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    bytes
        .iter()
        .fold(h, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

#[cfg(test)]
mod tests {
    use rand::RngExt;

    use super::*;

    #[test]
    fn event_load_streams_are_tagged_and_identity_keyed() {
        let seed = dwelling_seed(3, 42);
        let a = RngStream::event_load(seed, "Clothes Washer");
        let b = RngStream::event_load(seed, "Dishwasher");
        assert_ne!(a.stream, b.stream);
        assert_eq!(a, RngStream::event_load(seed, "Clothes Washer"));
        assert!(a.stream & EVENT_LOAD_STREAM_TAG != 0);
        assert!(b.stream & EVENT_LOAD_STREAM_TAG != 0);
    }

    #[test]
    fn rng_at_draws_from_the_stream_not_stream_zero() {
        let stream = RngStream::event_load(dwelling_seed(3, 42), "Cooking Range");
        let mut on_stream = stream.rng_at(0);
        let mut on_zero = ChaCha8Rng::from_seed(stream.seed);
        assert_ne!(on_stream.random::<u64>(), on_zero.random::<u64>());
    }

    #[test]
    fn rng_at_resumes_at_the_word_position() {
        let stream = RngStream::event_load(dwelling_seed(3, 42), "Cooking Range");
        let mut continuous = stream.rng_at(0);
        for _ in 0..5 {
            continuous.random::<f64>();
        }
        let mut resumed = stream.rng_at(10);
        assert_eq!(continuous.random::<f64>(), resumed.random::<f64>());
    }

    #[test]
    fn schedule_source_key_explicit_seed_wins() {
        let explicit = [9_u8; 32];
        assert_eq!(
            schedule_source_key(Some(explicit), "Water Heater 1:draw"),
            explicit
        );
    }

    #[test]
    fn schedule_source_key_distinct_identities_distinct_keys() {
        let a = schedule_source_key(None, "Water Heater 1:draw_flow_rate_source");
        let b = schedule_source_key(None, "Water Heater 2:draw_flow_rate_source");
        let c = schedule_source_key(None, "Water Heater 1:mains_temp_c_source");
        assert_ne!(a, b, "two heaters' sources must not share a keystream");
        assert_ne!(a, c, "one heater's two sources must not share a keystream");
        assert_ne!(
            a, [0_u8; 32],
            "the unspecified-seed default must never be the all-zero key"
        );
    }

    #[test]
    fn schedule_source_key_same_identity_is_reproducible() {
        let a = schedule_source_key(None, "Water Heater 1:draw_flow_rate_source");
        let b = schedule_source_key(None, "Water Heater 1:draw_flow_rate_source");
        assert_eq!(a, b);
    }

    #[test]
    fn event_load_nonce_space_is_unchanged_by_the_key_derivation() {
        // The keystream keys and the event-load nonces are different
        // derivations over the same hash: the domain prefixes keep them
        // from colliding, and the event-load nonce's value stays where
        // every existing checkpoint and fixture put it.
        let identity = "Cooking Range";
        let key = schedule_source_key(None, identity);
        let nonce_word = fnv1a_64(identity.as_bytes());
        assert_ne!(
            u64::from_le_bytes(key[..8].try_into().unwrap()),
            nonce_word,
            "the key derivation's first word is domain-separated from the \
             event-load nonce's hash"
        );
        assert_eq!(
            RngStream::event_load([4_u8; 32], identity).stream,
            super::EVENT_LOAD_STREAM_TAG | (nonce_word >> 1),
            "the event-load nonce stays the plain identity hash"
        );
    }

    /// The scenario mix leaves the bytes 0..16 (the master seed and the
    /// building id) and the absent scenario's bytes exactly as they are,
    /// and puts distinct scenarios at distinct keys: the layout map is
    /// injective with no hash involved.
    #[test]
    fn scenario_mixes_are_injective_over_the_spare_bytes() {
        let scenario_less = dwelling_seed(42, 1);
        let absent = dwelling_seed_scenario(42, None, 1);
        assert_eq!(absent, scenario_less, "None keeps today's bytes exactly");

        let a = dwelling_seed_scenario(42, Some(1), 1);
        let b = dwelling_seed_scenario(42, Some(2), 1);
        assert_ne!(a, b, "two scenarios must not share a dwelling key");
        assert_eq!(&a[..16], &scenario_less[..16], "bytes 0..16 unmoved");
        assert_eq!(
            &a[16..24],
            &1_u64.to_le_bytes(),
            "the scenario id rides bytes 16..24, little-endian"
        );
        assert_eq!(a[24], super::SCENARIO_PRESENCE_BYTE, "the presence byte");
        assert_eq!(&a[25..], &[0_u8; 7], "bytes 25..32 stay zero");

        // The master seed and the building id stay in their own bytes
        // under a declared scenario.
        assert_eq!(&a[..8], &42_u64.to_le_bytes(), "the master seed unmoved");
        assert_eq!(&a[8..16], &1_i64.to_le_bytes(), "the building id unmoved");
    }

    /// A declared scenario 0 must not equal the absent scenario, or
    /// "scenario 0" and "no scenario" would share every stream; and no
    /// (master, scenario, building) triple with a declared scenario,
    /// all zeros included, derives the all-zero key.
    #[test]
    fn scenario_zero_is_not_the_absent_scenario_and_never_all_zero() {
        let declared = dwelling_seed_scenario(42, Some(0), 1);
        let absent = dwelling_seed_scenario(42, None, 1);
        assert_ne!(
            declared, absent,
            "Some(0) and None must not derive the same key"
        );

        for (master, scenario, bldg) in [
            (0, Some(0), 0),
            (0, Some(0), 1),
            (0, Some(1), 0),
            (u64::MAX, Some(0), i64::MIN),
            (0, Some(u64::MAX), 0),
        ] {
            assert_ne!(
                dwelling_seed_scenario(master, scenario, bldg),
                [0_u8; 32],
                "the declared scenario ({master}, {scenario:?}, {bldg}) must \
                 never derive the all-zero key"
            );
        }
        // The scenario-less corner (master 0, building 0) is today's
        // all-zero key and stays exactly there: moving it would move
        // every scenario-less key, checkpoint and fixture. The declared
        // scenario's presence byte is what keeps the declared case off
        // it.
        assert_eq!(dwelling_seed(0, 0), [0_u8; 32]);
    }

    /// The diversity and sampler streams derive distinct, reproducible,
    /// scenario-disjoint streams: the shipped dimensions' identities never
    /// collide with each other or with the sampler's, two scenarios'
    /// dimension streams differ at the key alone (the nonce unchanged),
    /// and the same inputs reproduce.
    #[test]
    fn diversity_and_sampler_streams_are_disjoint_reproducible_and_scenario_keyed() {
        let key = dwelling_seed_scenario(42, Some(1), 7);
        let dimensions = [
            "presence-timing",
            "occupancy-level",
            "appliance-factors",
            "water-timing",
            "water-level",
        ];
        let streams: Vec<RngStream> = dimensions
            .iter()
            .map(|d| RngStream::schedule_diversity(key, d))
            .collect();
        // The tagged nonce: every diversity stream is out of the dwelling's
        // and the EV drivers' space, whatever the identity hashes to.
        for stream in &streams {
            assert!(
                stream.stream & EVENT_LOAD_STREAM_TAG != 0,
                "the diversity stream must carry the event-load tag"
            );
        }
        // Distinct dimensions: distinct streams (fixed reserved identities).
        for (i, a) in streams.iter().enumerate() {
            for b in &streams[i + 1..] {
                assert_ne!(a.stream, b.stream, "two dimensions share a nonce");
            }
        }
        // The sampler's stream is distinct from every diversity dimension's.
        let sampler = RngStream::scenario_sampler(key);
        for stream in &streams {
            assert_ne!(
                sampler.stream, stream.stream,
                "the sampler's nonce must not collide with a diversity dimension's"
            );
        }
        // Two scenarios: the nonce stable, the key (and so the draws) moved.
        let other_scenario_key = dwelling_seed_scenario(42, Some(2), 7);
        let a = RngStream::schedule_diversity(key, "presence-timing");
        let b = RngStream::schedule_diversity(other_scenario_key, "presence-timing");
        assert_ne!(a.seed, b.seed, "two scenarios: distinct keys");
        assert_eq!(a.stream, b.stream, "the nonce is the identity hash alone");
        // The same inputs reproduce.
        assert_eq!(
            a,
            RngStream::schedule_diversity(
                dwelling_seed_scenario(42, Some(1), 7),
                "presence-timing"
            )
        );
    }

    /// One identity under two scenario ids derives distinct dwelling keys
    /// and so distinct event-load streams for the same building; the
    /// same (master, scenario, building, identity) reproduces the same
    /// key and stream run to run and interleaved with other scenarios'
    /// derivations; and the scenario mix changes no nonce value: the
    /// event-load nonce stays the plain identity hash under every
    /// scenario.
    #[test]
    fn scenario_streams_are_disjoint_reproducible_and_nonce_stable() {
        let identity = "Clothes Washer";
        let key_a = dwelling_seed_scenario(42, Some(1), 1);
        let key_b = dwelling_seed_scenario(42, Some(2), 1);

        let stream_a = RngStream::event_load(key_a, identity);
        let stream_b = RngStream::event_load(key_b, identity);
        assert_ne!(
            stream_a.seed, stream_b.seed,
            "one identity under two scenarios: distinct keys"
        );
        assert_ne!(stream_a, stream_b, "distinct keys: distinct streams");
        assert_eq!(
            stream_a.stream, stream_b.stream,
            "the stream nonce itself is unchanged by the scenario mix: the \
             disjointness rides the key alone"
        );
        // The nonce stays the plain identity hash over the identity: the
        // top-bit tag and the shift are unchanged; the scenario enters
        // through the key alone.
        assert_eq!(
            stream_a.stream,
            super::EVENT_LOAD_STREAM_TAG | (fnv1a_64(identity.as_bytes()) >> 1),
            "the event-load nonce is the identity hash under every scenario"
        );

        // Reproducible run to run and interleaved with other scenarios'
        // derivations.
        let again = RngStream::event_load(dwelling_seed_scenario(42, Some(1), 1), identity);
        let _interleaved = dwelling_seed_scenario(42, Some(2), 1);
        let _other_building = dwelling_seed_scenario(42, Some(1), 2);
        assert_eq!(
            stream_a, again,
            "the same (master, scenario, bldg, identity) reproduces"
        );
    }
}
