# Development Guide

Tools, build profiles, feature flags, testing, and benchmarking for
working with HARES. For initial setup, see the [README](../README.md).

---

## Toolchain Overview

HARES is a Rust workspace with Python bindings. Here are the tools
involved and what each one does.

### Rust Tools

**[rustup](https://rustup.rs/)** manages Rust toolchain installations
(compiler, standard library, tools). Use it to install and update Rust.

```bash
rustup update stable         # update to the latest stable Rust
rustup show                  # show installed toolchains
```

**[Cargo](https://doc.rust-lang.org/cargo/)** is the Rust build system
and package manager. It compiles code, runs tests, manages dependencies
(defined in `Cargo.toml` files), and runs benchmarks. Most commands you
run during development start with `cargo`.

```bash
cargo check                  # type-check without producing a binary (fastest feedback loop)
cargo build                  # compile the project
cargo test                   # compile and run all tests
cargo clippy -- -D warnings  # run the linter (must pass with zero warnings)
cargo fmt --check            # check code formatting
cargo fmt                    # auto-format code
cargo bench                  # run benchmarks
```

Cargo operates on a **workspace** — the top-level `Cargo.toml` lists all
the crates (libraries) that make up HARES. You can target a single crate
with `-p`:

```bash
cargo test -p hares-physics  # test only the physics crate
cargo bench -p hares-envelope  # benchmark only the envelope solver
```

**[clippy](https://github.com/rust-lang/rust-clippy)** is a linter that
catches common mistakes and enforces idioms. It ships with rustup. All
code must be clippy-clean before merging.

**[rustfmt](https://github.com/rust-lang/rustfmt)** enforces consistent
formatting. Also ships with rustup. Run `cargo fmt` before committing.

### Python Tools

**[uv](https://docs.astral.sh/uv/)** manages Python versions, virtual
environments, and dependencies. It replaces pip, venv, and pip-tools.

```bash
uv venv --python 3.13       # create a virtualenv with Python 3.13
uv sync                     # install all project dependencies into the venv
uv run <command>             # run a command inside the virtualenv
uv add <package>             # add a dependency to pyproject.toml
```

All Python commands in this project are prefixed with `uv run` to ensure
they execute inside the project virtualenv.

**[Maturin](https://www.maturin.rs/)** bridges Rust and Python. It
compiles the Rust extension module (`hares-python` crate) and installs it
into the virtualenv as the `ochre_next._hares` package. See
[Python Bindings](python.md) for the full architecture.

```bash
uv run maturin develop       # compile Rust code and install into the venv
```

**[pytest](https://docs.pytest.org/)** runs the Python test suite.
Tests run in parallel via [pytest-xdist](https://pytest-xdist.readthedocs.io/)
and slow tests are excluded by default.

```bash
uv run pytest                # run fast tests in parallel (~5s)
uv run pytest -m slow        # run only the slow tests (ResStock, PyBaMM)
uv run pytest -m ""          # run everything (fast + slow)
uv run pytest -n0            # disable parallelism (easier to read output)
```

> **Note:** Python tests require the Rust extension to be built first.
> Run `uv run maturin develop` before `uv run pytest`.

---

## Build Profiles

Rust has two build profiles: **debug** and **release**.

**Debug** is the default when you run `cargo build` or `cargo test`. It
compiles quickly but produces unoptimised binaries. HARES enables extra
runtime validation in debug mode (energy balance checks, temperature
bounds) so that physics bugs surface immediately during development.

**Release** is enabled with the `--release` flag. It takes longer to
compile but produces optimised binaries. Runtime validation is disabled
unless you explicitly re-enable it (see [Feature Flags](#feature-flags)
below).

```bash
cargo build              # debug — fast compile, unoptimised, validation enabled
cargo build --release    # release — slower compile, optimised, validation disabled
```

The same applies to Python builds via Maturin:

```bash
uv run maturin develop             # debug
uv run maturin develop --release   # release
```

Use debug for everyday development. Use release when you need
representative runtime performance — benchmarking, profiling, or
comparing against OCHRE.

---

## Feature Flags

Feature flags enable optional capabilities at compile time. When a flag
is disabled, the gated code is excluded from the compiled binary entirely.

Enable one or more flags with `-F` (short for `--features`):

```bash
cargo build --release -F check_invariants
cargo build --release -F "check_invariants,observe"
```

### `check_invariants`

Per-timestep conservation-law checks: energy balance, electrical balance,
temperature bounds, humidity. These halt the simulation with a descriptive
error when a check fails.

Automatically enabled in debug builds. Use this flag to enable them in
release builds — useful for CI or validation runs where you want both
optimisation and correctness checking.

```bash
cargo build --release -F check_invariants
cargo test --release -F check_invariants
```

See [Invariants & Observability](invariants-and-observability.md) for the
full list of checks, tolerances, and error reporting.

### `observe`

Captures a detailed snapshot of simulation state at every phase of each
timestep: environment conditions, per-equipment contributions, solver
outputs, and zone state. Useful for debugging unexpected simulation
behaviour.

```bash
cargo build --release -F observe
```

Also available in Python builds — required for Gymnasium RL environments:

```bash
uv run maturin develop --release -F observe
```

See [Invariants & Observability](invariants-and-observability.md#observer-system-zero-cost-debugging)
for the snapshot data model and API.

### `observe_detailed`

Per-component detail for envelope solver observations. Requires `observe`.

```bash
cargo build --release -F "observe,observe_detailed"
```

### `profiling`

Timing instrumentation for the simulation loop.

```bash
cargo build --release -F profiling
```

### `actor_profiling`

Per-equipment step timing. More granular than `profiling`.

```bash
cargo build --release -F actor_profiling
```

### `dst`

Daylight Saving Time support for schedule indexing (adds `chrono-tz`).

```bash
cargo build --release -F dst
```

### Python-side feature flags

Maturin passes `-F` flags through to Cargo, but only features defined on
the `hares-python` crate are available. Currently that is `observe` only.
Rust-only features like `profiling` and `actor_profiling` do not apply to
the Python extension.

---

## Running and Debugging Tests

### Filtering tests

Cargo runs all tests matching a name filter. The filter is a substring
match against the full test path (module + function name):

```bash
cargo test                           # all tests in the workspace
cargo test -p hares-physics          # all tests in one crate
cargo test -p hares-physics solar    # tests with "solar" in the name
cargo test thermal_balance           # tests matching "thermal_balance" across all crates
```

To run a single exact test:

```bash
cargo test -p hares-core -- --exact tests::invariants::test_electrical_balance
```

### Seeing test output

By default, Cargo captures stdout from passing tests. To see `println!`
output from all tests (including passing ones):

```bash
cargo test -- --nocapture
```

Combine with a filter to focus on one test:

```bash
cargo test -p hares-envelope rc_step -- --nocapture
```

### Python tests

By default `uv run pytest` runs fast tests in parallel across all CPU
cores (via pytest-xdist). Slow tests (marked `@pytest.mark.slow`) are
excluded — these include ResStock network fetches and PyBaMM model
generation.

```bash
uv run pytest                                   # fast tests, parallel (~5s)
uv run pytest -m slow                           # slow tests only
uv run pytest -m ""                             # all tests (fast + slow)
uv run pytest tests/python/test_py_dwelling.py  # single file
uv run pytest -k "test_thermal"                 # name filter
uv run pytest -x                                # stop on first failure
uv run pytest -n0 -s                            # sequential, show print output
```

> Python tests require the Rust extension. Run `uv run maturin develop`
> first. If you change Rust code, rebuild before re-running Python tests.

### Useful Cargo flags for debugging

```bash
cargo test -- --test-threads=1       # run tests sequentially (easier to read output)
RUST_BACKTRACE=1 cargo test          # show backtraces on panic
RUST_BACKTRACE=full cargo test       # show full backtraces with line numbers
cargo test --release                 # run tests with optimisation (faster for integration tests)
```

---

## Rust Patterns Used in HARES

If you are new to Rust, see the [Rust Primer](rust-primer.md) for an
overview of the key language features and design patterns used in the
codebase: data enums, traits, iterators, ownership/borrowing,
pre-allocated buffers, and feature-gated code.

---

## Benchmarking

HARES includes [Criterion](https://bheisler.github.io/criterion.rs/book/)
benchmarks. Criterion builds in release mode automatically.

```bash
cargo bench -p hares-envelope        # envelope solver microbenchmarks
cargo bench --bench single_building  # single-dwelling simulation
cargo bench --bench fleet            # parallel multi-dwelling fleet
cargo bench --bench rl_step          # RL environment step
```

When comparing HARES against OCHRE, build HARES in release mode without
extra feature flags to match production configuration. Debug builds
include runtime checks and disabled optimisations not present in
production.

---

## Quick Reference

| I want to... | Command |
|--------------|---------|
| Type-check without compiling | `cargo check` |
| Build for development | `cargo build` |
| Run all Rust tests | `cargo test` |
| Run a single crate's tests | `cargo test -p hares-physics` |
| Lint | `cargo clippy -- -D warnings` |
| Format code | `cargo fmt` |
| Build optimised | `cargo build --release` |
| Release build with validation | `cargo build --release -F check_invariants` |
| Release build with observer | `cargo build --release -F "check_invariants,observe"` |
| Profile the simulation loop | `cargo build --release -F profiling` |
| Build Python bindings (dev) | `uv run maturin develop` |
| Build Python bindings (optimised) | `uv run maturin develop --release` |
| Build Python bindings with observer | `uv run maturin develop --release -F observe` |
| Build a distributable wheel | `uv run maturin build --release` |
| Run Python tests (fast) | `uv run pytest` |
| Run Python tests (all) | `uv run pytest -m ""` |
| Run Python tests (slow only) | `uv run pytest -m slow` |
| Run benchmarks | `cargo bench` |
