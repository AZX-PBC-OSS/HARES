# Contributing to HARES

Engineering guidelines for working in the HARES codebase.

## Getting started

See the [README](README.md) for repository setup, prerequisites, and the
crate layout. The short version:

```bash
git clone https://github.com/NREL/HARES.git && cd HARES
git submodule update --init --recursive
uv venv --python 3.13 && uv sync
```

### Pre-commit checklist

Every change should pass these before opening a PR:

```bash
cargo fmt --check
cargo clippy -- -D warnings
cargo test
uv run pytest                # if Python code was touched
```

## Performance

### No allocations in hot loops

The simulation timestep loop runs millions of iterations across a fleet.
Per-timestep code must not allocate.

- **Pre-allocate buffers** outside the loop and reuse them.
  `Vec::with_capacity`, `HashMap::with_capacity`, and buffer fields on
  solver structs are the standard patterns (see `ThermalSolver`'s latent
  buffer, `EnvelopeComponentGains`, etc.).
- **Return into caller-owned storage** (`&mut Vec<f64>`, `&mut HashMap`)
  rather than returning fresh collections from hot functions.
- Avoid `collect()` inside the timestep loop unless the result is a
  fixed-size, pre-known-length container.

### Schedules: compute, don't materialise

`ScheduleSource` (in `hares-types`) is lazily evaluated by design.
Never pre-materialise an 8760-row `Vec<f64>` when a `ScheduleSource`
can be evaluated on demand per timestep. Column-backed schedules
(`ColumnRef`) index into shared Arrow arrays with zero per-step
allocation; `DailyProfile` and `Constant` compute in O(1).

## Correctness

### Deterministic seeding for stochastic code

All randomness flows from a single `master_seed` through
`derive_dwelling_rng(master_seed, bldg_id)` in `hares-core/src/rng.rs`.
This guarantees:

- Identical results across runs with the same seed.
- Per-dwelling isolation: adding/removing a building does not change
  another building's random stream.
- Checkpoint/restore reproduces the stream (`ChaCha8Rng::from_seed`).

When adding stochastic behaviour, draw from the dwelling's `self.rng` --
never construct a new `thread_rng()` or unseeded RNG.

### `debug_assert!` for developer-time guards

Use `debug_assert!` (and `debug_assert_eq!`) for precondition checks
that would be too expensive or unnecessary in release builds but catch
bugs during development and testing.  Include a message with the
offending value so the panic is actionable:

```rust
debug_assert!(
    tilt_deg >= 0.0 && tilt_deg <= 180.0,
    "sky_view_factor: tilt_deg={tilt_deg} outside [0, 180]"
);
```

These compile to nothing in `--release`.  For checks that must fire in
production, use `InvariantChecker` (see below).

### Runtime invariant checking

`InvariantChecker` in `hares-core/src/invariants.rs` runs per-timestep
numerical sanity checks (energy balance, temperature bounds, humidity
ratio sign).  These are gated by
`cfg(any(debug_assertions, feature = "check_invariants"))` so they run
in tests and debug builds by default, and can be opted into in release
via the Cargo feature flag.

When adding a new solver output or domain update, add a corresponding
invariant check.

## Observability

### Observer / capture pattern

The `observe` Cargo feature enables zero-cost step-level introspection
of runtime execution state.  When the feature is off, every observation
call site is compiled out entirely.

- **`observer.rs`** defines the snapshot types (`StepSnapshot`,
  `PhaseSnapshots`, per-phase capture structs).
- **`observer_capture.rs`** contains pure capture functions that read
  struct fields into snapshots -- this is the only coupling point between
  `Dwelling` internals and observation types.
- **`diff_ports`** back-calculates per-equipment contributions by
  diffing port accumulators before and after each equipment step.

When adding a new equipment type or solver output, add corresponding
fields to the capture structs and the capture functions so that the
observer remains complete.

### Telemetry

Each `Equipment` implementation exposes a `telemetry() -> &Telemetry`
method returning a flat key-value map of its current operating state.
This feeds into the observer and CSV output.  Keep telemetry keys stable
across versions -- downstream analysis depends on them.

## Testing

### Test quality over quantity

Tests must be meaningful, not performative.

- **Assert against independent reference values**, not the same formula
  the code under test uses (circular/tautological tests catch nothing).
- **Test physics, not algebra**: prefer known reference values
  (psychrolib, EnergyPlus, ASHRAE tables) over re-deriving the expected
  result in the test.
- **Edge cases matter**: boundary conditions (0, NaN, extreme values)
  and degenerate cases (single-node zone, zero flow, missing weather
  data) are where bugs hide.
- **Energy conservation**: any radiation, convection, or mass-flow model
  must have a conservation test (sum of fluxes = 0 in a closed system).

### Validation and verification

- `cargo test` must pass before any commit.
- `cargo clippy` must be warning-free.
- Physics models cite their reference (ASHRAE chapter, EnergyPlus
  Engineering Reference section, OCHRE source file and line) in doc
  comments.
- Parity tests against OCHRE (`vendors/OCHRE/`) and EnergyPlus
  reference values are the gold standard for verification.

## Physics

### SI units internally

HARES stores and computes in SI units.  The `uom` crate is used for
unit conversions at I/O boundaries; internal code uses raw `f64` in SI.
Never embed manual conversion constants -- use `uom` or the typed
wrappers in `hares-physics`.

### Best available models

Use EnergyPlus-grade (or better) physics models.  The codebase targets
parity with or improvement over OCHRE -- not replication of its legacy
quirks.  Compat shims for OCHRE interop belong in the Python adapter
layer, not in the Rust core.

## Code style

### Small files, DRY code

- Keep files focused on a single concern.
- Extract shared logic rather than duplicating across modules.
- No backward-compatibility shims, `Option` wrappers, or dead code --
  this is a greenfield codebase.

### Comments

Don't add noise comments that restate what the code does.  Do add
comments that explain *why* -- the physics reference, the non-obvious
design decision, the edge case that motivated a guard.
