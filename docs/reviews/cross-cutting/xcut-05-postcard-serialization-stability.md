# Postcard serialization stability: checkpoint format across Rust versions, field change detection
**Review ID**: xcut-05
**Category**: cross-cutting
**Date**: 2026-05-26

## Files Reviewed
- crates/hares-core/src/checkpoint.rs (DwellingCheckpoint struct, save/load, version check)
- crates/hares-core/src/dwelling/mod.rs (save_checkpoint, load_checkpoint methods)
- crates/hares-equipment/src/ev/checkpoint.rs (EvCheckpoint struct)
- crates/hares-equipment/src/lib.rs (save_postcard, load_postcard helpers)
- crates/hares-equipment/src/battery/mod.rs (BatteryCheckpoint, save/load)
- crates/hares-equipment/src/pv/mod.rs (PvCheckpoint)
- crates/hares-equipment/src/generator.rs (GeneratorCheckpoint)
- crates/hares-equipment/src/ventilation.rs (VentilationCheckpoint)
- crates/hares-equipment/src/hvac/heat_pump/heater.rs (HeaterState checkpoint)
- crates/hares-equipment/src/hvac/air_conditioner.rs (AirConditionerState)
- crates/hares-equipment/src/battery/degradation.rs (DegradationState, RainflowCounter)
- crates/hares-fleet/src/fleet.rs (Fleet, SteppableFleet)
- crates/hares-python/src/py_dwelling.rs (Python checkpoint API)
- tests/regression/checkpoint_restart.rs
- Cargo.toml, Cargo.lock (postcard version pinning)

## Vendor/Reference Files Consulted
- postcard v1.1.3 crate documentation (docs.rs): confirms stable wire format since v1.0.0

## Findings

### Finding 1: Inner equipment checkpoint structs carry no version field
**Severity**: high

**Description**: The outer `DwellingCheckpoint` struct correctly includes a `format_version: u32` field (checked against `CHECKPOINT_VERSION = 3` at `crates/hares-core/src/checkpoint.rs:51-56`), which gates the JSON envelope schema. However, every inner equipment checkpoint struct serialized via postcard into `equipment_states: Vec<(EquipmentId, Vec<u8>)>` lacks any version field. These include:

| Struct | File |
|---|---|
| `EvCheckpoint` | `crates/hares-equipment/src/ev/checkpoint.rs:4-29` |
| `BatteryCheckpoint` | `crates/hares-equipment/src/battery/mod.rs:255-276` |
| `GeneratorCheckpoint` | `crates/hares-equipment/src/generator.rs:433-439` |
| `PvCheckpoint` | `crates/hares-equipment/src/pv/mod.rs:87-97` |
| `VentilationCheckpoint` | `crates/hares-equipment/src/ventilation.rs:146-152` |
| `HeaterState` | `crates/hares-equipment/src/hvac/heat_pump/heater.rs:185-228` |
| `AirConditionerState` | `crates/hares-equipment/src/hvac/air_conditioner.rs:106-133` |
| `IdealHvacState` | `crates/hares-equipment/src/hvac/ideal_hvac.rs:124-138` |
| `DehumidifierState` | `crates/hares-equipment/src/hvac/dehumidifier.rs:53-63` |
| `ScheduledLoadState` | `crates/hares-equipment/src/scheduled_load.rs:130-141` |
| + 9 more water heater / baseboard / boiler / furnace / tank states |

Consequently, when any field type in these checkpoint structs changes (e.g., `f64` → `f32`, `u32` → `u64`, enum variant addition), old postcard blobs stored in checkpoints will either deserialize silently to wrong values or fail with a generic `SerdeError` that does not identify the affected equipment.

**Code Location**: All equipment checkpoint structs listed above — `CHECKPOINT_VERSION` is searched for in `crates/hares-equipment/` and returns zero matches.

**Root Cause**: The `format_version` field is only on `DwellingCheckpoint`, which protects the outer JSON schema but not the inner binary blobs. Equipment checkpointing was designed as opaque `Vec<u8>` payloads (`equipment_states: Vec<(EquipmentId, Vec<u8>)>`) with the assumption that `EquipmentId` alone suffices for dispatch. There is no per-equipment version token embedded in the blob header.

**Impact**: Adding a field to `HeaterState` (e.g., `defrost_demand_response: bool`) with `#[serde(default)]` would deserialize correctly on new code but older blobs silently default the new field. Adding a variant to `DRLevel` or `EvConnectionState` would cause postcard to fail with a variant-index-out-of-range error on old blobs. Changing `f64` to `f32` in `RainflowCounter::reversals` would silently reinterpret 64-bit IEEE floats as two 32-bit values, producing nonsensical degradation state. The `CHECKPOINT_VERSION` bump on the envelope would not catch any of these.

---

### Finding 2: No CRC or checksum integrity check on checkpoint files
**Severity**: high

**Description**: Neither the JSON envelope (`DwellingCheckpoint::save` at `crates/hares-core/src/checkpoint.rs:34-43`) nor the inner postcard blobs include integrity verification. A single flipped bit in persistent storage (disk corruption, cosmic ray, bad SSD sector) produces a checkpoint that deserializes without error but with corrupted field values — the simulation resumes silently with wrong state. The postcard crate *does* provide CRC32 support as an optional `use-crc` feature (`from_bytes_crc32`, `to_allocvec_crc32`, `to_stdvec_crc32`), but HARES does not enable it.

**Code Location**:
- `DwellingCheckpoint::save()`: `crates/hares-core/src/checkpoint.rs:34-43` — no checksum computed
- `DwellingCheckpoint::load()`: `crates/hares-core/src/checkpoint.rs:46-58` — no checksum verified
- `save_postcard()`: `crates/hares-equipment/src/lib.rs:248-252` — uses `postcard::to_allocvec`, not `to_allocvec_crc32`
- `load_postcard()`: `crates/hares-equipment/src/lib.rs:271-274` — uses `postcard::from_bytes`, not `from_bytes_crc32`
- Cargo.toml: `postcard = { version = "1", features = ["alloc"] }` — the `use-crc` feature is not enabled

**Root Cause**: Checkpoint integrity was not in scope during initial implementation. The existing verifications (version mismatch, equipment-by-ID lookup, humidity-per-zone lookup) guard against schema incompatibilities but not against bit-level corruption.

**Impact**: The `format_version` check at `checkpoint.rs:51` guarantees the schema is intended but does not guarantee the bytes are intact. A corrupted `timestep_index` or `soc` value in a postcard blob inside `equipment_states` passes through the entire deserialization chain (JSON parse → postcard parse) silently because no layer validates integrity. For a fleet of 10,000 dwellings running weeks-long simulations, undetected checkpoint corruption could waste days of compute time before producing erroneous aggregate results.

---

### Finding 3: No fleet-level checkpoint consistency enforcement
**Severity**: medium

**Description**: `SteppableFleet` (`crates/hares-fleet/src/fleet.rs:267-486`) provides step-by-step fleet execution with a shared `current_step: u64` counter, but exposes no checkpoint save or load methods. The `Fleet::simulate()` method (`:213-263`) runs all dwellings independently via Rayon with a progress callback and panic isolation — there is no mechanism to synchronize checkpoint save/restore across the fleet. If an external orchestrator (Python script, batch runner) saves checkpoints for individual dwellings at different timesteps and later restores them, dwellings will be at different simulation steps without any detection mechanism.

**Code Location**:
- `SteppableFleet` struct: `crates/hares-fleet/src/fleet.rs:267-273` — no `save_checkpoint()` or `load_checkpoint()` methods
- `Fleet::simulate()`: `crates/hares-fleet/src/fleet.rs:213-263` — no checkpoint integration during parallel execution
- `DwellingCheckpoint`: `crates/hares-core/src/checkpoint.rs:18` — `timestep_index` is an individual field, not validated against a fleet-wide step counter

**Root Cause**: Fleet checkpoint management is deferred to external orchestration (Python layer at `crates/hares-python/src/py_dwelling.rs:559-562`). Neither `SteppableFleet` nor `Fleet` enforce that all dwellings checkpoint at the same timestep.

**Impact**: If dwelling-1 restores from step 1000 and dwelling-2 from step 999, their subsequent aggregate results are out of sync by one timestep. The fleet aggregator (`crates/hares-fleet/src/aggregation.rs`) would produce subtly incorrect totals and load shapes because one dwelling's output is shifted relative to others. This error is silent — no warning or error is emitted.

---

### Finding 4: Checkpoint size inefficiency from JSON-encoded binary blobs
**Severity**: medium

**Description**: `DwellingCheckpoint` is serialized with `serde_json::to_vec()` at `crates/hares-core/src/checkpoint.rs:35`. The `equipment_states: Vec<(EquipmentId, Vec<u8>)>` and `rng_state: [u8; 32]` fields contain binary data but are encoded as JSON arrays of integers — each byte becomes `"N,"` (2–4 characters). The postcard blobs inside `equipment_states` are already binary-optimized, but embedding them inside JSON inflates them 3–6×. For a dwelling with 5 equipment items each having ~200-byte postcard blobs, plus ~200 double-precision floats in `envelope_state` and `fluid_states`, the checkpoint is approximately 3–5 KB. For a 10,000-dwelling fleet, a single checkpoint snapshot is 30–50 MB — manageable. However, if checkpoints are saved every simulated day for a year-long simulation, total storage reaches 11–18 GB. No compression (gzip, zstd) is applied at any layer.

**Code Location**:
- `DwellingCheckpoint::save()`: `crates/hares-core/src/checkpoint.rs:34-43` — JSON only, no compression
- `DwellingCheckpoint` fields: `crates/hares-core/src/checkpoint.rs:17, 19, 21-29` — binary fields embedded as JSON arrays

**Root Cause**: JSON was selected for human readability and cross-platform compatibility. Binary blobs within JSON are a natural consequence of the two-layer design (JSON envelope + postcard equipment states). No compression wrapper was added because checkpoint storage volume was not anticipated for large fleets.

**Impact**: For small studies (1–100 dwellings), checkpoint size is negligible. For ResStock-scale fleets (5,000–10,000 dwellings), multi-day checkpoint archives may exhaust disk space on shared CI runners or HPC scratch filesystems.

---

### Finding 5: No `#[serde(default)]` on equipment checkpoint structs for forward compatibility
**Severity**: medium

**Description**: None of the 15+ equipment checkpoint structs use `#[serde(default)]` on individual fields or at the struct level. This means adding a new mandatory field (without a `default` attribute) to any checkpoint struct will cause `load_postcard()` to fail for all existing checkpoints — postcard rejects missing fields by default. Structs like `HeaterState` (28 fields), `BatteryCheckpoint` (20 fields), and `EvCheckpoint` (22 fields) are frequently modified to support new features (e.g., the recently added `rainflow` field in `BatteryCheckpoint`). Each new feature requiring a field addition to a checkpoint struct breaks backward compatibility with all existing checkpoint files.

**Code Location**:
- `EvCheckpoint`: `crates/hares-equipment/src/ev/checkpoint.rs:4-29` — 22 fields, no `#[serde(default)]`
- `BatteryCheckpoint`: `crates/hares-equipment/src/battery/mod.rs:255-276` — 20 fields, no `#[serde(default)]`
- `HeaterState`: `crates/hares-equipment/src/hvac/heat_pump/heater.rs:185-228` — 28 fields, no `#[serde(default)]`
- `PvCheckpoint`: `crates/hares-equipment/src/pv/mod.rs:87-97` — 7 fields, no `#[serde(default)]`
- All remaining equipment checkpoint state structs follow the same pattern

**Root Cause**: Checkpoint structs are treated as ephemeral (write-once, read-once within a single simulation run), not as persistent artifacts that must survive schema evolution across software versions. The `CHECKPOINT_VERSION` bump on `DwellingCheckpoint` provides a blunt full-clearing mechanism (all old checkpoints become unreadable) rather than per-field migration.

**Impact**: Every checkpoint schema change requires bumping `CHECKPOINT_VERSION` and invalidating all existing checkpoints. This is acceptable during development but will become a problem when users accumulate checkpoints from long-running simulations that they expect to be reusable after a minor code update.

---

### Finding 6: `EvConnectionState` enum silently breaks on variant addition
**Severity**: low

**Description**: `EvConnectionState` (`crates/hares-types/src/equipment.rs:360-365`) is a 3-variant enum (`HomePluggedIn`, `AwayPluggedIn`, `Disconnected`) serialized via postcard using discriminant encoding (varint-encoded index 0, 1, or 2). Adding a fourth variant (e.g., `WorkplacePluggedIn`) would change the encoding for `Disconnected` from index 2 to index 3. Old checkpoints with `Disconnected` (index 2) would deserialize to the new third variant instead, and new code saving `Disconnected` (index 3) would fail on old code expecting only 3 variants. Postcard treats unknown variant indices as `SerdeError::DeserializeAnyUnsupported` — it fails rather than silently decoding to the wrong variant. However, inserting a variant in the middle of the enum definition (before `Disconnected`) **would** cause silent reinterpretation because existing discriminant values would map to different variants. This risk applies to all enums embedded in checkpoint structs: `OperatingMode`, `DRLevel`, `ThermostatMode`, `InverterPriority`, `ScheduleSourceState`, `BatteryChemistry`, `VentilationType`, `GeneratorKind`, `FuelType`, etc.

**Code Location**:
- `EvConnectionState`: `crates/hares-types/src/equipment.rs:360-365`
- Usage in checkpoint: `crates/hares-equipment/src/ev/checkpoint.rs:7` as `connection_state: EvConnectionState`

**Root Cause**: Postcard encodes enums by discriminant index (0-based, varint). Adding variants before the last variant shifts all subsequent discriminants. Postcard does not support named variant serialization like JSON does (which would survive reordering).

**Impact**: Low probability because adding a variant in the middle of an existing enum requires explicit developer intervention. However, the consequences are severe: postcard rejects the deserialization rather than warning about the mismatch. A developer adding `WorkplacePluggedIn` as the second variant would shift `Disconnected` from discriminant 2 to 3, causing old checkpoints to fail with a `SerdeError` when encountering a `Disconnected` blob. The error message does not identify the enum name or the problematic discriminant.

---

### Finding 7: `save_postcard` panics on serialization failure
**Severity**: low

**Description**: `save_postcard()` (`crates/hares-equipment/src/lib.rs:248-252`) calls `panic_serialize(e)` on any postcard error, terminating the simulation with no recovery. This is invoked by every equipment's `save_state()` method, which is called inside `Dwelling::save_checkpoint()` (`crates/hares-core/src/dwelling/mod.rs:2008`) in a `.map()` closure over equipment — meaning any single equipment's serialization failure panics the entire dwelling. The `try_save_postcard()` alternative exists at `lib.rs:259-262` but is unused by all equipment implementations because the `Equipment::save_state() -> Vec<u8>` signature does not return a `Result`. This finding was previously documented in core-07 (Finding 4).

**Code Location**: `crates/hares-equipment/src/lib.rs:248-252`, called by 15+ `save_state()` implementations.

**Root Cause**: `save_postcard` is designed as an infallible helper for a `-> Vec<u8>` signature that cannot propagate errors. Postcard serialization of the types used in HARES (primitives, `Vec`, `Option`, flat enums) is infallible in practice, but this is a latent risk.

**Impact**: If any future equipment state introduces a postcard-incompatible type, the simulation panics during checkpoint save rather than returning a recoverable error. This is mitigated by the fact that all current HARES types are postcard-compatible.

---

## Summary
- Total findings: 7
- Critical / High / Medium / Low: 0 / 2 / 3 / 2

| Severity | Count | Key concern |
|---|---|---|
| Critical | 0 | — |
| High | 2 | No version field on inner equipment checkpoint blobs; no CRC/checksum |
| Medium | 3 | No fleet-level checkpoint sync; checkpoint size inefficiency; no `#[serde(default)]` for forward compat |
| Low | 2 | Enum variant addition breaks postcard; `save_postcard` panics path |

## Recommendations
1. **Add version fields to inner equipment checkpoint structs.** Each struct serialized via postcard (e.g., `BatteryCheckpoint`, `EvCheckpoint`, `HeaterState`) should carry a u32 version tag that is validated on deserialization. The version tags can be independent per-equipment type, allowing schema evolution without bumping the outer `CHECKPOINT_VERSION`. Reject blobs with unknown versions with an error that identifies the equipment type and the expected version range.

2. **Enable postcard CRC32.** Add `use-crc` to the postcard features in Cargo.toml and replace `postcard::to_allocvec`/`postcard::from_bytes` with `postcard::to_allocvec_crc32`/`postcard::from_bytes_crc32`. This adds a 4-byte CRC32 suffix to each postcard blob with negligible CPU overhead. Alternatively, compute a single checksum (SHA-256 or CRC32) over the full JSON envelope and embed it in the checkpoint file alongside `format_version`.

3. **Add fleet-level checkpoint save/load to `SteppableFleet`.** Provide `save_checkpoints(dir: &Path)` and `restore_from_checkpoints(dir: &Path)` methods that save/load all dwellings at the same `current_step`, verifying that all restored timestep indices are identical. Fail with a clear error listing mismatched dwelling IDs if any checkpoint is from a different step.

4. **Apply `#[serde(default)]` to all fields in checkpoint structs.** This allows new optional fields to be added without breaking existing checkpoints. Where a meaningful default exists (e.g., `power_limit_kw: Option<f64>` → `None`), use it. Where no default makes sense, require the field and rely on the version bump.

5. **Add per-equipment version tokens to postcard blobs.** The simplest approach: prepend a u32 version to each postcard blob, or wrap equipment state in a `struct VersionedState { version: u32, #[serde(flatten)] payload }`. Validate the version before deserializing the payload. This isolates equipment schema evolution from the outer `CHECKPOINT_VERSION`.

6. **Consider checkpoint compression.** Wrap the JSON output in gzip or zstd before writing to disk. The JSON representation of binary data compresses extremely well (10:1 or better) because JSON array-of-integers encoding is highly redundant. This would reduce fleet-scale checkpoint storage from tens of GB to a few GB.

7. **Mark new enum variants at the end of enum definitions.** Document this as a coding convention. For enums that must support mid-list insertion, use explicit discriminant values (`#[repr(u32)]`) or switch to string-based serialization. The postcard wire spec reserves the right to fail on unknown discriminants, so adding variants only at the end preserves backward compatibility.

8. **Replace `save_postcard` with `try_save_postcard`.** Change `Equipment::save_state()` from `fn save_state(&self) -> Vec<u8>` to `fn save_state(&self) -> Result<Vec<u8>>`, update all equipment implementations and `save_checkpoint()`, and remove the panicking wrapper. This eliminates a crash path from a long-running simulation.

## References / Citations
- Postcard wire format specification (stable since v1.0.0): https://postcard.jamesmunns.com/wire-format.html
- Postcard v1.1.3 API docs with CRC32 functions: https://docs.rs/postcard/1.1.3/postcard/#functions (see `from_bytes_crc32`, `to_allocvec_crc32`)
- HARES postcard dependency: `Cargo.toml` — `postcard = { version = "1", features = ["alloc"] }`
- HARES postcard lock: `Cargo.lock` — `postcard 1.1.3` (checksum: `6764c3b5...`)
- Previous checkpoint review (core-07): `docs/reviews/core/core-07-checkpoint-round-trip.md`
- Checkpoint regression test: `tests/regression/checkpoint_restart.rs`
