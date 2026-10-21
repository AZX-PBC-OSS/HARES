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
#[must_use]
pub fn dwelling_seed(master_seed: u64, bldg_id: i64) -> [u8; 32] {
    let mut seed = [0_u8; 32];
    seed[..8].copy_from_slice(&master_seed.to_le_bytes());
    seed[8..16].copy_from_slice(&bldg_id.to_le_bytes());
    seed
}

/// A schedule source's stochastic keystream key: the configured key when
/// the source carries an explicit seed, otherwise derived from the
/// source's identity (`<owner>:<source role>`). The identity's FNV-1a
/// words fill the key the way [`RngStream::event_load`] fills its stream
/// nonce, so two sources with distinct identities never share a
/// keystream, the same identity is reproducible run to run, and the
/// default is never the all-zero key every unspecified-seed source would
/// otherwise share.
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
}
