# RNG state management across actors and checkpointing
**Review ID**: core-15
**Category**: core
**Date**: 2026-05-25

## Files Reviewed
- `crates/hares-core/src/rng.rs` (full file, 58 lines)
- `crates/hares-core/src/checkpoint.rs` (full file, 186 lines)
- `crates/hares-core/src/dwelling/mod.rs` (checkpoint save/restore: ~line 1991–2068; RNG construction: ~line 931)
- `crates/hares-equipment/src/event_load.rs` (`derive_rng_seed`: ~line 1421; equipment RNG usage)
- `crates/hares-equipment/src/schedule_helpers.rs` (schedule-source RNG state capture/restore)
- `crates/hares-types/src/schedule.rs` (`StochasticState`: ~line 270; `ScheduleSource` variants)
- `crates/hares-equipment/src/ev/catalog.rs` (EV archetype schedule seeds)
- `crates/hares-core/src/actor_registry.rs` (actor seed plumbing)

## Vendor/Reference Files Consulted
None

## Findings
### Finding 1: Dwelling `self.rng` is never drawn from during simulation
**Severity**: medium
**Description**: The `Dwelling` struct declares `rng: ChaCha8Rng` (`dwelling/mod.rs:643`) and passes a clone to the environment for one-time setpoint deadband noise during initialization (`dwelling/mod.rs:941`). After init, the dwelling's `self.rng` is never advanced — no `.random::<f64>()` or other draw calls appear anywhere in `run_timestep` or elsewhere. The checkpoint saves and restores `rng_state`, `rng_stream`, and `rng_word_pos` for this RNG, but these values are identical at every timestep because the RNG is never consumed. The presence of `self.rng` in the checkpoint schema and dwelling struct implies it governs simulation randomness, but in practice all stochasticity comes from independently seeded per-equipment RNGs and per-schedule-source RNGs.
**Code Location**: `crates/hares-core/src/dwelling/mod.rs:643` (field declaration), line `931` (construction), lines `2010,2014,2015` (checkpoint save — captures static values), lines `2031–2033` (checkpoint restore).
**Root Cause**: The `self.rng` field is a vestige of a design where the dwelling would own the authoritative RNG stream. Responsibility was shifted to per-equipment and per-schedule-source RNGs, but the dwelling-level field was retained for checkpoint continuity.
**Impact**: Wasted state per timestep in checkpoints (32 bytes + stream + word pos). Misleads readers into thinking a dwelling-level RNG stream exists. Does not affect simulation correctness since the stored values are constant. No reproducibility bug — the RNG is externally quiescent.

### Finding 2: No ChaCha stream/jump-ahead architecture for per-actor isolation
**Severity**: medium
**Description**: The review brief references "independent streams via ChaCha/SplitMix jump-ahead" as the expected mechanism for actor RNG isolation. The codebase does not use ChaCha's native `set_stream()` (nonce/stream counter) for actor-level stream isolation. Instead, each stochastic consumer (equipment, schedule source) independently creates its own `ChaCha8Rng::from_seed()` with a distinct deterministic seed. The `set_stream()` and `set_word_pos()` methods are used only for checkpoint restore (`dwelling/mod.rs:2031–2033`, `event_load.rs:637,1147`). This functional approach achieves the same isolation guarantees — adding/removing equipment does not affect existing equipment's random sequences — but relies on seed-space partitioning rather than ChaCha's block-cipher counter mode stream partitioning.
**Code Location**: `crates/hares-core/src/rng.rs:12–17` (`derive_dwelling_rng` — no `set_stream` call). `crates/hares-equipment/src/event_load.rs:1421–1444` (`derive_rng_seed` — creates seed, no `set_stream`). `crates/hares-equipment/src/ev/catalog.rs:436–530` (EV archetypes create independent `ChaCha8Rng` from distinct seeds).
**Root Cause**: Design choice to use independent RNG instances rather than a single master RNG with ChaCha stream-counter fan-out.
**Impact**: The approach is functionally equivalent for reproducibility. The absence of stream-based partitioning is a documentation/architecture issue: the design does not match the stated "jump-ahead" approach. It also forecloses any future use case where a single master seed would need to deterministically spawn an unknown-at-compile-time number of actors without re-deriving their individual seeds.

### Finding 3: Seed construction uses zero-padded concatenation, not cryptographic hashing
**Severity**: low
**Description**: Both `derive_dwelling_rng` (`rng.rs:12–17`) and `derive_rng_seed` (`event_load.rs:1439–1443`) construct 256-bit ChaCha keys by direct byte concatenation, leaving parts of the key zero:
- `derive_dwelling_rng`: bytes 0–8 = master_seed (u64), bytes 8–16 = bldg_id (i64), bytes 16–32 = zeros. **128 bits of zero-padding**.
- `derive_rng_seed`: bytes 0–8 = master_seed (u64), bytes 8–16 = building_id (i64), bytes 16–24 = FNV-1a name hash (u64), bytes 24–32 = zeros. **64 bits of zero-padding**.

ChaCha8 is a stream cipher with a 256-bit key. Zero-padding half or a quarter of the key material wastes the cipher's security margin. While this is not a security application, using a proper hash (e.g., SHA-256 of a tuple) would distribute entropy evenly across all 32 seed bytes and prevent any risk of correlated seeds across the zeroed regions.
**Code Location**: `crates/hares-core/src/rng.rs:12–17`; `crates/hares-equipment/src/event_load.rs:1439–1443`.
**Root Cause**: Optimization for simplicity — direct byte concatenation avoids a hash dependency. FNV-1a was chosen for `derive_rng_seed` as a cheap per-name hash but only covers bytes 16–24.
**Impact**: Negligible for simulation. Two different (master_seed, bldg_id) pairs produce distinct seeds because the starting bytes differ. The zero-padding risks only a pathological collision if ChaCha8 has a structural weakness in zero-extended keys, which has never been demonstrated. No practical impact on reproducibility or independence.

### Finding 4: EV driver seed is config-supplied, not derived from master_seed
**Severity**: medium
**Description**: The EvDriver actor receives its `seed` as an independent `f64` parameter from `ActorConfig` (`actor_registry.rs:139–141`). It is NOT derived from `master_seed` or `bldg_id`. The seed is converted to `[u8; 32]` with only the first 8 bytes set (`actor_registry.rs:146–150`), then passed to the EV archetype's schedule builders. This means:
- Two buildings in a fleet simulation that share the same EvDriver seed configuration will have identical EV behavior patterns.
- The EV seed is not automatically diversified by building identity — it relies on manual configuration to produce different seeds per dwelling.
**Code Location**: `crates/hares-core/src/actor_registry.rs:139–142` (seed extraction), lines `146–149` (seed byte construction — single u64 with 192 bits of zeros).
**Root Cause**: The EvDriver actor was designed before the `master_seed`/`bldg_id` derivation pattern was established. It accepts an explicit `seed` parameter rather than deriving from the simulation's master seed.
**Impact**: Potential for deterministic collapse in fleet simulations if all dwellings share the same EvDriver seed. Not a bug in current ResStock usage (each dwelling receives its own seed configuration), but a design fragility. Adding a `master_seed` and `bldg_id` parameter to the EvDriver factory and deriving the seed deterministically would eliminate this risk.

### Finding 5: setpoint deadband noise draws are not recoverable from checkpoint
**Severity**: low
**Description**: During environment initialization, `initialize_zones` (`environment.rs:1026–1056`) draws from the `initial_rng` clone to add deadband-scale noise to the initial indoor temperature setpoint. This draw advances the cloned RNG instance, which is consumed and discarded after initialization. The dwelling's `self.rng` (which is checkpointed) was cloned before this draw, so it holds the pre-draw state. A checkpoint restored mid-simulation would contain `self.rng` in its pre-deadband-noise state, but since the setpoint deadband noise occurred during construction and the initial zone temperatures are preserved in `Dwelling::latest_env`, this discrepancy does not cause a replay divergence.
**Code Location**: `crates/hares-core/src/environment.rs:1026–1056` (RNG consumption for initial setpoint noise); `crates/hares-core/src/dwelling/mod.rs:941` (clone point before draw).
**Root Cause**: The setpoint deadband noise is treated as a construction-time concern, not something that needs to be reproducible from a checkpoint. The initial zone temperatures (which embed the noise result) are preserved via `Dwelling::latest_env`.
**Impact**: None for post-init checkpoint restore. If someone wanted to recreate the dwelling from scratch and get identical initial temperatures, they would need to know exactly how many draws the deadband noise consumed. This is theoretically reproducible (it's a fixed count: one draw per zone with HVAC setpoints, at construction), but not explicitly documented.

## Summary
- Total findings: 5
- Critical: 0 / High: 0 / Medium: 3 / Low: 2

## Recommendations
1. **Remove or repurpose `Dwelling::rng`**. Since it carries no simulation-time state, consider removing it from the `Dwelling` struct and checkpoint schema. If it existed for a future "dwelling-level stochastic" purpose, add a clarifying comment and consider deriving per-equipment seeds from it via `set_stream()` fan-out at construction time instead.

2. **Adopt ChaCha stream partitioning for per-actor isolation**. Use `ChaCha8Rng::set_stream()` to give each actor a distinct 64-bit stream ID, derived deterministically from a hash of (master_seed, bldg_id, actor_name). This would centralize RNG management in one master ChaCha8Rng per dwelling, making the architecture match the documented intent and enabling deterministic stream allocation for future actor types without manual seed derivation.

3. **Use a cryptographic hash for seed derivation**. Replace the zero-padded concatenation in `derive_dwelling_rng` and `derive_rng_seed` with a hash-based seed construction. The `blake3` or `sha2` crate could hash a canonical encoding of `(master_seed, bldg_id, component_id)` into a 32-byte seed, distributing entropy across the full ChaCha key. This would eliminate the zero-padding concern and make adding new derivation dimensions (e.g., a version tag) straightforward.

4. **Derive EV driver seed from master_seed**. Pass `master_seed` and `bldg_id` into the EvDriver actor factory in `actor_registry.rs` and derive the driver's individual seed deterministically from `(master_seed, bldg_id, target_name)` rather than requiring a separately-configured `seed` parameter.

5. **Document the RNG architecture**. Add an architecture note to `rng.rs` explaining: which components consume randomness, how their seeds are derived, that per-equipment RNGs are independent (not stream-multiplexed), and what checkpoint fields capture each RNG's state for restart reproducibility.

## References / Citations
- `rand_chacha::ChaCha8Rng` documentation: `set_stream(u64)` sets the ChaCha stream counter for independent parallel streams.
- The `chacha8_set_word_pos_matches_sequential_draws` regression test (`event_load.rs:1988–2021`) validates correct `set_word_pos` behavior.
- Prior review `coredeep-02-rng-seed-propagation.md` notes absence of `self.rng` usage in `run_timestep` and lack of cryptographic seed derivation (corroborates Findings 1 and 3).
