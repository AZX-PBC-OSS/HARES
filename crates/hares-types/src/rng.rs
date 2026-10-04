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
const EVENT_LOAD_STREAM_TAG: u64 = 1 << 63;

/// One ChaCha8 stream: the generator key and the stream nonce.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RngStream {
    pub seed: [u8; 32],
    pub stream: u64,
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

/// FNV-1a 64-bit: stable across platforms and releases, unlike `std`'s
/// hasher, so stream identities are reproducible.
fn fnv1a_64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
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
}
