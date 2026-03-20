//! Deterministic RNG management for reproducibility.

use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

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

#[cfg(test)]
mod tests {
    use super::*;

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

        // Create a third RNG that wouldn't exist in another scenario
        let _a3 = derive_dwelling_rng(7, 3);

        let b1 = derive_dwelling_rng(7, 1);
        let b2 = derive_dwelling_rng(7, 2);

        assert_eq!(a1.get_seed(), b1.get_seed());
        assert_eq!(a2.get_seed(), b2.get_seed());
    }
}
