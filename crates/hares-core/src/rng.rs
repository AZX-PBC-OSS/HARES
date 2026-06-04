//! Deterministic RNG management for reproducibility.
//!
//! HARES uses hierarchical ChaCha8 RNG stream partitioning to give each
//! stochastic component (actors, equipment, dwelling-level noise) an
//! independent, non-overlapping random stream derived from a single
//! per-dwelling master seed.  This guarantees end-to-end reproducibility:
//! two dwellings with the same master seed and building ID produce
//! identical simulation outputs, including all internal stochastic draws.

use rand::RngExt;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

/// Stream 0 is reserved for the dwelling's own RNG (thermal noise,
/// per-timestep advancement, etc.).
pub const RNG_STREAM_DWELLING: u64 = 0;
/// Stream range for EV driver actors.  The per-actor stream is
/// `RNG_STREAM_EV_DRIVER_BASE + actor_index`.
pub const RNG_STREAM_EV_DRIVER_BASE: u64 = 1;
/// Stream range for event-based load equipment (EventBasedLoad,
/// WetAppliance).  The per-equipment stream is
/// `RNG_STREAM_EVENT_LOAD_BASE + equipment_index`.
pub const RNG_STREAM_EVENT_LOAD_BASE: u64 = 1000;

/// Derives a deterministic per-dwelling RNG from a master seed and building ID.
///
/// Seed construction: bytes 0..8 are `master_seed` (little-endian), bytes 8..16
/// are `bldg_id` (little-endian, two's-complement for negative IDs), and
/// remaining bytes are zero. This guarantees isolation: adding or removing
/// buildings does not affect any other building's RNG stream.
pub fn derive_dwelling_rng(master_seed: u64, bldg_id: i64) -> ChaCha8Rng {
    let mut seed = [0u8; 32];
    seed[..8].copy_from_slice(&master_seed.to_le_bytes());
    seed[8..16].copy_from_slice(&bldg_id.to_le_bytes());
    ChaCha8Rng::from_seed(seed)
}

/// Derives an independent sub-RNG from a parent RNG by partitioning
/// ChaCha8's 64-bit stream space.  The returned RNG shares the parent's
/// seed but operates on a distinct, non-overlapping stream starting at
/// word position zero.  Callers must use a unique `stream` nonce for each
/// purpose; see `RNG_STREAM_*` constants above.
pub fn derive_sub_rng(parent: &ChaCha8Rng, stream: u64) -> ChaCha8Rng {
    let mut sub = parent.clone();
    sub.set_stream(stream);
    sub.set_word_pos(0);
    sub
}

/// Advances the dwelling RNG by one draw so that checkpoint captures
/// reflect simulation progress.  Returns the drawn value in case the
/// caller wants to use it for per-step non-deterministic decisions.
pub fn advance_dwelling_rng(rng: &mut ChaCha8Rng) -> u64 {
    rng.random()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::RngExt;

    #[test]
    fn same_inputs_produce_identical_rng() {
        let a = derive_dwelling_rng(42, 1);
        let b = derive_dwelling_rng(42, 1);
        assert_eq!(a.get_seed(), b.get_seed());
    }

    #[test]
    fn different_bldg_ids_produce_different_seeds() {
        let a = derive_dwelling_rng(42, 1);
        let b = derive_dwelling_rng(42, 2);
        assert_ne!(a.get_seed(), b.get_seed());
    }

    #[test]
    fn negative_bldg_id_produces_valid_distinct_seed() {
        let neg = derive_dwelling_rng(0, -1);
        let pos = derive_dwelling_rng(0, 1);
        assert_ne!(neg.get_seed(), pos.get_seed());
    }

    #[test]
    fn isolation_adding_buildings_does_not_affect_others() {
        let a1 = derive_dwelling_rng(7, 1);
        let a2 = derive_dwelling_rng(7, 2);

        let _a3 = derive_dwelling_rng(7, 3);

        let b1 = derive_dwelling_rng(7, 1);
        let b2 = derive_dwelling_rng(7, 2);

        assert_eq!(a1.get_seed(), b1.get_seed());
        assert_eq!(a2.get_seed(), b2.get_seed());
    }

    #[test]
    fn derive_sub_rng_produces_different_streams() {
        let parent = derive_dwelling_rng(42, 1);
        let sub_a = derive_sub_rng(&parent, 10);
        let sub_b = derive_sub_rng(&parent, 20);
        assert_eq!(sub_a.get_stream(), 10);
        assert_eq!(sub_b.get_stream(), 20);
        assert_eq!(sub_a.get_word_pos(), 0);
        assert_eq!(sub_b.get_word_pos(), 0);
        assert_eq!(sub_a.get_seed(), parent.get_seed());
        assert_eq!(sub_b.get_seed(), parent.get_seed());
    }

    #[test]
    fn sub_rng_streams_are_independent() {
        let parent = derive_dwelling_rng(42, 1);
        let mut sub_a = derive_sub_rng(&parent, 100);
        let mut sub_b = derive_sub_rng(&parent, 200);

        let draws_a: Vec<f64> = (0..100).map(|_| sub_a.random()).collect();
        let _draws_b: Vec<f64> = (0..100).map(|_| sub_b.random()).collect();

        let word_a = sub_a.get_word_pos();
        let word_b = sub_b.get_word_pos();
        assert!(word_a > 0);
        assert!(word_b > 0);

        let mut sub_a2 = derive_sub_rng(&parent, 100);
        let draws_a2: Vec<f64> = (0..100).map(|_| sub_a2.random()).collect();
        assert_eq!(draws_a, draws_a2);
    }

    #[test]
    fn advance_dwelling_rng_changes_word_position() {
        let mut rng = derive_dwelling_rng(42, 1);
        let pos_before = rng.get_word_pos();
        let _ = advance_dwelling_rng(&mut rng);
        let pos_after = rng.get_word_pos();
        assert!(
            pos_after > pos_before,
            "RNG word position must advance after a draw"
        );
    }

    #[test]
    fn advance_dwelling_rng_is_deterministic() {
        let mut a = derive_dwelling_rng(99, 5);
        let mut b = derive_dwelling_rng(99, 5);
        assert_eq!(advance_dwelling_rng(&mut a), advance_dwelling_rng(&mut b));
        assert_eq!(advance_dwelling_rng(&mut a), advance_dwelling_rng(&mut b));
        assert_eq!(a.get_word_pos(), b.get_word_pos());
    }
}
