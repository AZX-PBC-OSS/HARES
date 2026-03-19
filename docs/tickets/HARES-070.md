---
id: HARES-070
title: "hares-fleet — Fleet Checkpoint and Restart"
kind: implement
depends_on: [HARES-045, HARES-047]
phase: 5
crate: hares-fleet
files_to_touch:
  - crates/hares-fleet/src/checkpoint.rs
  - crates/hares-fleet/src/fleet.rs
references:
  - docs/architecture/08-operations.md
verification:
  - cargo test -p hares-fleet
  - cargo clippy -p hares-fleet -- -D warnings
---

## Background/Context

`08-operations.md` specifies fleet-level checkpoint/restart for long fleet runs. HARES-045 provides per-dwelling `save_state`/`load_state`. This ticket adds fleet-level orchestration: periodic checkpointing of all dwelling states to disk and restart from a saved checkpoint. Without this, interrupting a long fleet run (power failure, preemption, OOM kill) discards all work.

## Work to Do

- [ ] Define `FleetCheckpoint` struct in `crates/hares-fleet/src/checkpoint.rs`:
  - [ ] Fields: `timestep_index: u64`, `wall_clock_unix_s: u64`, `dwellings: Vec<DwellingCheckpoint>`
  - [ ] `DwellingCheckpoint`: `{ bldg_id: BuildingId, equipment_states: Vec<u8> }` — `equipment_states` is the raw `save_state()` bytes per dwelling
  - [ ] `FleetCheckpoint` must be `Send + Sync`
  - [ ] Serialization via `bincode` (add to workspace dev-dependencies if not present)
- [ ] Add configurable checkpoint policy to fleet config:
  - [ ] `checkpoint_every_n_steps: Option<u64>` — checkpoint after every N timesteps
  - [ ] `checkpoint_every_n_wall_secs: Option<u64>` — checkpoint after every N wall-clock seconds (whichever fires first)
  - [ ] `checkpoint_dir: Option<PathBuf>` — directory for checkpoint files; disabled if `None`
- [ ] Implement checkpoint writes in the fleet simulation loop:
  - [ ] Atomic write: write to `{dir}/checkpoint_{timestep}.tmp`, then `rename` to `{dir}/checkpoint_{timestep}.bin`
  - [ ] Keep only the two most recent checkpoint files; delete older ones after successful rename
- [ ] Implement `Fleet::resume_from_checkpoint(path: &Path, config: FleetConfig) -> Result<Fleet>`:
  - [ ] Loads `FleetCheckpoint` from `path`
  - [ ] Reconstructs all dwellings at the checkpointed timestep using `load_state()`
  - [ ] Continues simulation from `timestep_index + 1`
- [ ] Add integration test:
  - [ ] Fleet of 10 dwellings, checkpoint at step 100, restart from checkpoint, assert output for steps 101-200 is bitwise identical to uninterrupted run

## Measures of Success

- [ ] Fleet of 10 dwellings checkpointed at step 100, restarted from checkpoint, produces identical output to uninterrupted run for steps 101 onwards
- [ ] Checkpoint write is atomic: a process kill mid-write leaves the previous checkpoint intact and valid
- [ ] `resume_from_checkpoint` with a corrupt file returns `Err` (not a panic)
- [ ] Checkpoint overhead: writing 1000-dwelling checkpoint adds < 5% wall-clock overhead vs. no-checkpoint run

## Verification

- [ ] `cargo test -p hares-fleet` passes
- [ ] `cargo clippy -p hares-fleet -- -D warnings` passes
