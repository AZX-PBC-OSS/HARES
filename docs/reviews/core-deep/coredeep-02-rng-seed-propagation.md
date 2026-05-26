# RNG seed propagation: config to Dwelling to per-actor streams, checkpoint/restore determinism
**Review ID**: coredeep-02
**Category**: core-deep
**Date**: 2026-05-26

## Files Reviewed
crates/hares-core/src/rng.rs

## Vendor/Reference Files Consulted
None

## Findings
### Finding 1: [Severity: critical]
**Description**: Dwelling RNG is dead state — stored but never consumed during simulation. The `Dwelling.rng` field (`dwelling/mod.rs:643`) is initialized once at construction, cloned once for environment initialization (to randomize initial indoor zone temperature), and thereafter used _only_ for checkpoint save/restore. During `run_timestep()` (`dwelling/mod.rs:2209–2815`), the dwelling RNG is never read, sampled, or advanced. The RNG state written to a checkpoint therefore reflects a generator whose internal state changes only during environment construction, making checkpoint/restore of the dwelling RNG effectively meaningless for the simulation loop.

**Code Location**: `crates/hares-core/src/dwelling/mod.rs:643` (field declaration), lines `931` (creation via `derive_dwelling_rng`), `941` (clone into environment options), and lines `2209–2815` (absence of any `self.rng` usage in `run_timestep`).

**Root Cause**: The dwelling RNG was introduced as seedable state but was never plumbed through to any stochastic actor, equipment, or solver within the timestep loop. The only consumer — environment initialization — receives a detached clone and discards it after construction.

**Impact**: If an RNG-dependent actor or solver is added and begins reading `self.rng`, the checkpoint/restore path (`save_checkpoint`/`load_checkpoint` at lines `1991–2068`) would faithfully persist and restore the RNG. However, any checkpoint captured today restores an RNG that never interacts with the simulation, creating a false sense of determinism coverage. This is a latent correctness hazard for any future stochastic feature.

### Finding 2: [Severity: high]
**Description**: No hierarchical RNG stream splitting from dwelling to actors. The dwelling RNG should act as a master root from which per-actor (thermal noise, occupant behavior, equipment state transitions) independent streams are derived using a cryptographically sound split (e.g., ChaCha with per-stream nonce, or hash-based sub-seed derivation). Instead:

- **EV driver actor** (`actors/ev_driver/mod.rs:3395–3398`) derives its own `u64` seed from a simple fold of the equipment name bytes (`acc.wrapping_mul(31).wrapping_add(b as u64)`), completely bypassing the dwelling RNG.
- **Event-based load equipment** (`hares-equipment/src/event_load.rs:1421–1444`) independently reads `master_seed` and `building_id` from its config and hashes the equipment name with FNV-1a, also bypassing the dwelling RNG.
- **Battery management actor** (`actors/bms`) is entirely deterministic — no RNG usage.

There is no single root RNG that governs all stochastic elements of a dwelling. Multiple independent seed derivations coexist with overlapping seed material, creating a risk of correlated streams.

**Code Location**: 
- `crates/hares-core/src/actors/ev_driver/mod.rs:3395–3398` (name-based seed hash)  
- `crates/hares-equipment/src/event_load.rs:1421–1444` (config-based seed derivation)  
- `crates/hares-core/src/dwelling/mod.rs:1498` (`build_actors_from_seeds` — receives equipment but no dwelling RNG)

**Root Cause**: Actors and equipment are designed to be self-contained with their own seed derivation logic. The dwelling RNG was never integrated into the actor-construction path.

**Impact**: Actor RNG streams are not deterministically linked to the dwelling RNG. Changing the dwelling RNG derivation strategy would not affect actor randomness. This breaks the principle of a single, auditable chain from `master_seed` → dwelling → actor.

### Finding 3: [Severity: high]
**Description**: No end-to-end reproducibility test. The `rng.rs` test module (`rng.rs:20–57`) only validates that `derive_dwelling_rng` produces distinct seeds for different building IDs and that adding buildings does not affect existing seeds. The EV driver actor tests (`actors/ev_driver/mod.rs:1005–1046`) validate that the same actor seed produces the same stochastic sequence, but only at the actor level. There is no test that constructs two identical `Dwelling` instances with the same `master_seed` and `bldg_id`, runs the full simulation, and asserts that all output steps are identical within floating-point tolerance. Without such a test, changes to any stochastic component could silently break reproducibility.

**Code Location**: `crates/hares-core/src/rng.rs:20–57` (current tests only check seed isolation, not simulation reproducibility)

**Root Cause**: Reproducibility is implicitly assumed from deterministic seed derivation without a validating end-to-end integration test.

**Impact**: Undetected non-determinism (thread scheduling, hash map iteration order, floating-point order-of-operations in shuffled equipment lists, or unseeded RNG usage) could creep in and invalidate A/B comparisons between simulation runs with the same config seed.

### Finding 4: [Severity: medium]
**Description**: `rng.rs` seed construction lacks cryptographic derivation. The `derive_dwelling_rng` function (`rng.rs:12–17`) copies `master_seed` and `bldg_id` directly into the ChaCha8Rng seed bytes with zero-padding. While functionally correct for isolation (different building IDs produce different seeds), this approach:

1. Masks the distinction between two buildings with the same ID in different fleets using different `master_seed` values — a collision could theoretically occur if `master_seed` and `bldg_id` values overlap in the seed space.
2. Does not support extensibility: adding a "purpose" label (e.g., "thermal-noise" vs "occupancy") requires rewriting the seed layout rather than appending a domain-separation tag to a hash.

A proper KDF (e.g., Blake3 keyed or SHA-256 `HKDF-expand`) would provide domain separation, collision resistance, and a natural hierarchy.

**Code Location**: `crates/hares-core/src/rng.rs:12–17`

**Root Cause**: The seed construction was designed for simplicity — mapping two numbers into a 32-byte seed — rather than using a key derivation function that supports hierarchical splitting.

**Impact**: Low today (the dwelling RNG is unused anyway — see Finding 1), but would become a design constraint for any multi-level RNG hierarchy.

### Finding 5: [Severity: medium]
**Description**: EV driver actor seed derivation uses non-cryptographic hash. The seed for each EV driver actor is computed as:
```rust
let seed_val = name.as_bytes().iter()
    .fold(0u64, |acc, &b| acc.wrapping_mul(31).wrapping_add(b as u64));
```
(Line `3395–3398` of `dwelling/mod.rs`). This is a weak polynomial rolling hash with no avalanche property — similar names produce similar seeds, and the collision resistance is poor. While `ChaCha8Rng::from_seed(seed_bytes(seed_val))` at `actors/ev_driver/mod.rs:308` zero-extends this to a 32-byte key, the effective entropy is at most 64 bits with poor distribution. Two equipment pieces with names that hash-collide in this function would share identical random event sequences.

**Code Location**: `crates/hares-core/src/dwelling/mod.rs:3395–3398`

**Root Cause**: The hash was written as a convenience function without cryptographic requirements in mind.

**Impact**: Rare in practice (requires name collisions in a 64-bit space with a weak hash), but violates the principle that each actor should have an independently seeded, non-overlapping RNG stream. The `event_load.rs` FNV-1a approach (lines `1421–1444`) is a better precedent within the same codebase.

### Finding 6: [Severity: low]
**Description**: Checkpoint RNG serialization is structurally correct for the current RNG implementation. `save_checkpoint()` (`dwelling/mod.rs:2010–2015`) captures `rng_state` (32-byte ChaCha seed), `rng_stream` (u64), and `rng_word_pos` (u128), which together constitute the full internal state of `rand_chacha::ChaCha8Rng`. `load_checkpoint()` (`dwelling/mod.rs:2031–2034`) reconstructs the generator with `from_seed`, `set_stream`, and `set_word_pos`. If the dwelling RNG were actually consumed during simulation, this would correctly preserve deterministic replay from any checkpoint. The mechanism is in place; the gap is that the RNG it preserves is unused (Finding 1).

**Code Location**: `crates/hares-core/src/dwelling/mod.rs:2010–2015` (serialization), `2031–2034` (deserialization).

**Root Cause**: N/A — this is a positive observation. The serialization granularity is appropriate.

**Impact**: No current impact, but validates that the checkpoint schema has the fields needed for future RNG usage.

### Finding 7: [Severity: low]
**Description**: Fleet mode does not clone dwelling RNGs. Each dwelling in a fleet (`fleet.rs:139`) receives its own `DwellingConfig` with a distinct `bldg_id`. The `derive_dwelling_rng` call (`dwelling/mod.rs:931`) uses `(master_seed, bldg_id)` where `bldg_id` is unique per dwelling. This prevents the "cloned RNG" antipattern described in the review brief. However, the fleet uses a single shared `SimulationConfig` with `master_seed: 0` for ResStock runs (`fleet.rs:557`), meaning all dwellings share the same `master_seed`. Per-dwelling isolation relies entirely on the `bldg_id` field being distinct.

**Code Location**: `crates/hares-fleet/src/fleet.rs:139–146` (config construction), `crates/hares-core/src/dwelling/mod.rs:931` (RNG derivation).

**Root Cause**: N/A — this is a verification that the fleet-mode isolation concern is correctly handled for the current design.

**Impact**: No current bug, but fragility: if two dwellings ever receive the same `bldg_id`, they would share identical RNG streams. A defensive check or using a guaranteed-unique per-dwelling index would be more robust.

## Summary
- Total findings: 7
- Critical / High / Medium / Low: 1 / 2 / 2 / 2

## Recommendations
1. **Plumb the dwelling RNG into the simulation loop.** Identify all stochastic elements that need per-timestep randomness (currently: EV driver actor, event-based loads). Refactor so each stochastic component receives a sub-RNG derived from the dwelling RNG via a hierarchical split. Use ChaCha8Rng's `set_stream` with a per-purpose nonce (e.g., stream 0 = thermal noise, stream 1 = EV driver, stream 2 = occupancy, etc.) to ensure non-overlapping streams.
2. **Replace name-based seed hashes with proper derivation from the dwelling RNG.** Remove the raw `fold` hash in `build_actors_from_seeds` and the FNV-1a hash in `event_load::derive_rng_seed`. Instead, pass a sub-RNG (or sub-seed derived via KDF) from the dwelling to each actor/equipment at construction time.
3. **Add an end-to-end reproducibility test.** Construct two identical `Dwelling` instances with the same `master_seed` and `bldg_id`, run `simulate()` to completion on both, and assert that all `StepResult` fields match within `f64::EPSILON`. This guards against inadvertent non-determinism from any source.
4. **Use a cryptographic KDF for seed derivation.** Replace `rng.rs`'s raw byte-copy with a keyed hash (e.g., Blake3 keyed with `master_seed`, domain-separating on `bldg_id` and a purpose tag). This provides collision resistance and natural extensibility for multi-level hierarchies.
5. **Defensively assert unique bldg_id in fleet runs.** Add a construction-time check that no two `DwellingConfig` values in a fleet share the same `(master_seed, bldg_id)` pair, to fail-fast on the shared-RNG antipattern.

## References / Citations
- `rand_chacha` crate documentation: ChaCha8Rng state consists of seed (32 bytes), stream (u64), and word position (u128).
- FNV-1a: Fowler–Noll–Vo hash, non-cryptographic. Used in `event_load.rs:1431` for equipment name hashing.
- The review criteria checklist from coredeep-02 covers: (1) per-actor independent RNG streams, (2) fleet-mode RNG isolation, (3) checkpoint/restore RNG state preservation, (4) same-config-seed reproducibility.
