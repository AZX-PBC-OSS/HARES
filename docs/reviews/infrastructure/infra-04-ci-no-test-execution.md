# CI runs only fmt + clippy — no test execution, Python tests, or benchmarks
**Review ID**: infra-04
**Category**: infrastructure
**Date**: 2026-05-26

## Files Reviewed
- `.github/workflows/ci.yml` (35 lines)

## Vendor/Reference Files Consulted
- `Cargo.toml` (workspace root; 66 lines) — workspace members, dependency manifests, build profiles
- `crates/hares-core/Cargo.toml` (59 lines) — benchmark registrations, feature flags, dev-dependencies
- `crates/hares-python/Cargo.toml` (31 lines) — PyO3 extension crate, `observe` feature passthrough
- `pyproject.toml` (78 lines) — Python build system, optional deps (helics, PyBaMM, PySAM, RL), pytest config
- `.cargo/config.toml` (20 lines) — build profile overrides (opt-level=0, debug=0, sccache)
- `docs/development.md` (426 lines) — canonical test commands, feature flags, benchmark instructions

## Findings

### Finding 1: [Severity: critical]
**Description**: The CI pipeline executes zero tests. The workflow contains only two jobs — `cargo fmt --check` and `cargo clippy --workspace -- -D warnings` — with no `cargo test`, `cargo nextest run`, or any test execution step whatsoever. The project contains approximately 3,000+ inline Rust unit tests (spread across 9 crates), 660+ crate-level integration tests (distributed across `crates/*/tests/`), and an additional 50+ top-level integration tests (under `tests/` covering regression, parity, bestest, and oracle suites). None of these are exercised in CI. Any regression in simulation correctness — energy balance, thermal solver accuracy, HPXML parsing, equipment modelling, schedule interpolation — will merge undetected.

**Code Location**: `.github/workflows/ci.yml:16-35` — the `jobs:` section contains only `format` (lines 17–25) and `clippy` (lines 27–35). No `test`, `nextest`, or `pytest` job exists.

**Root Cause**: The CI workflow was configured as a minimal skeleton — likely a bootstrap that was never extended to cover test execution. The `docs/development.md` (lines 407–426) documents `cargo nextest run`, `uv run pytest`, and `cargo bench` as standard development commands, but the CI file has not been updated to reflect these.

**Impact**:
1. **Zero correctness guard**: All simulation behaviour changes (envelope solver, HVAC models, water heater algorithms, battery dispatch, thermal balance, HPXML ingestion) merge without automated verification.
2. **Crate test isolation untested**: Each crate's `tests/` directory contains integration tests that exercise cross-crate interactions (e.g., `hares-core/tests/bestest.rs` validates the full thermal stack against ASHRAE 140; `hares-equipment/tests/hvac_tests.rs` exercises dynamic DX coil behaviour). None of these ever run in CI.
3. **Top-level integration suites unreachable**: The `tests/regression/`, `tests/parity/`, `tests/bestest/`, and oracle suites at the workspace root lack `[[test]]` entries in any `Cargo.toml` — making them undiscoverable by Cargo even if a test job were added (confirming finding from review `infra-01`).
4. **Release-mode validation gap**: `docs/development.md` line 197 recommends `cargo nextest run --release -F check_invariants` for CI validation; this is never executed.

### Finding 2: [Severity: critical]
**Description**: The 672 Python tests in `tests/python/` are never executed in CI. These tests cover the full Python companion layer: dwelling creation and simulation (`test_py_dwelling.py`, `test_py_dwelling_integration.py`), fleet management (`test_py_fleet.py`, `test_py_fleet_bindings.py`), HELICS co-simulation (`test_helics_dwelling.py`, `test_helics_fleet.py`, `test_helics_integration.py`, `test_helics_broker_runner.py`), Gymnasium RL environments (`test_gym_env.py`), OCHRE parity (`test_ochre_parity.py`), PyBaMM/SAM adapters (`test_adapters.py`), and tariff handling (`test_tariff_builder.py`, `test_tariff_integration.py`). Python tests depend on the Rust extension being compiled via `uv run maturin develop`, which CI never performs.

**Code Location**: `.github/workflows/ci.yml` — no Python job exists. The full Python test suite resides at `tests/python/` (36 test files, 672 test functions, configured in `pyproject.toml:73-77`).

**Root Cause**: No CI job provisions Python 3.13+, installs `uv`, builds the PyO3 extension with maturin, and runs pytest. The project's `pyproject.toml` already defines all necessary configuration: `tool.maturin` (lines 65–71) specifies the extension build, `tool.pytest.ini_options` (lines 73–77) sets test discovery, and `dependency-groups.dev` (lines 39–46) lists pytest, maturin, and xdist.

**Impact**:
1. **PyO3 binding breakages undetected**: Changes to the `hares-python` crate or any upstream Rust crate can silently break Python bindings. The `_hares` extension is a `cdylib` with complex FFI — type mismatches, missing exports, or serde changes that affect Python-facing structs would only be caught on a developer's local machine.
2. **HELICS integration untested**: HELICS is the co-simulation backbone for fleet-scale scenarios. Tests in `test_helics_integration.py` (line 24) use `pytestmark = pytest.mark.skipif` to gracefully skip when HELICS is unavailable, but CI never even attempts execution.
3. **RL environment regressions**: `test_gym_env.py` tests the Gymnasium API contract (`reset()`, `step()`, observation space, reward shapes). Breaking this silently breaks any downstream RL training pipeline.
4. **Adapter code untested**: PyBaMM and PySAM adapters (`python/ochre_next/adapters/`) generate critical data (OCV curves, efficiency LUTs, PV performance data) consumed by the Rust simulation. `test_adapters.py` verifies these pipelines but never runs in CI.

**Dependencies needed** (as defined in `pyproject.toml`):
- Python 3.13+ (line 9: `requires-python = ">=3.13"`)
- `uv` for package management
- `maturin>=1.12.6` (dev group, line 41) — builds the `ochre_next._hares` cdylib
- `pytest>=9.0.2` + `pytest-xdist>=3.8.0` (lines 40, 45)
- Optional: `helics>=3.6.1` (line 26) for HELICS tests
- Optional: `pybamm>=26.3.0` (line 24) for PyBaMM adapter tests
- Optional: `NREL-PySAM>=7.1.0` (line 23) for SAM adapter tests

### Finding 3: [Severity: high]
**Description**: Benchmark code is never compiled or executed in CI. The project has 5 benchmark groups registered in Cargo.toml — `single_building`, `fleet`, `rl_step` (all in `benches/` via `crates/hares-core/Cargo.toml:45–58`), and `rc_solver` (in `crates/hares-envelope/benches/rc_solver.rs` via `crates/hares-envelope/Cargo.toml:25–28`). Without at least `cargo bench --no-run` in CI, benchmark code can accumulate compilation errors, use deprecated APIs, or reference deleted types without detection.

**Code Location**: `.github/workflows/ci.yml` — no bench job exists. Benchmark registrations are at:
- `crates/hares-core/Cargo.toml:45–58` (three `[[bench]]` entries: `single_building`, `fleet`, `rl_step`)
- `crates/hares-envelope/Cargo.toml:25–28` (one `[[bench]]` entry: `rc_solver`)

**Root Cause**: Benchmark infrastructure was added after the initial CI skeleton and the CI file was never updated to include compilation verification.

**Impact**:
1. **Benchmark bitrot**: `benches/common.rs` (178 lines) provides shared HPXML fixture builders, temp path management, and TOML case construction used by all three top-level benchmarks. Changes to `hares-io` HPXML parsing or `hares-core` configuration types can break this module silently.
2. **Performance regression blind spot**: Even without running benchmarks to completion, `cargo bench --no-run` verifies that benchmark code compiles against current APIs. Absence means performance-critical code paths (fleet scale-out, single-building year-long simulation, RL step latency) have no compile-time guard.

### Finding 4: [Severity: high]
**Description**: The top-level `tests/` directory integration test suites (regression, parity, bestest, conditioned_oracle, freefloat_oracle, structural_envelope_oracle, warmup_regression, resstock_smoke) are orphaned from the workspace build graph. No `[[test]]` entry exists in any `Cargo.toml` to make these test targets discoverable by Cargo. This means even if a `cargo test` job were added to CI, these suites would not compile or execute. This was previously identified in review `infra-01` but remains unfixed.

**Code Location**:
- `tests/regression/mod.rs` — unified regression runner
- `tests/parity/mod.rs` — OCHRE parity validation
- `tests/bestest/mod.rs` — ASHRAE 140 BESTEST cases (all flagged `#[ignore]`)
- `tests/conditioned_oracle.rs`, `tests/freefloat_oracle.rs`, `tests/structural_envelope_oracle.rs` — oracle tests
- `tests/warmup_regression.rs`, `tests/resstock_smoke.rs` — additional integration tests
- No `[[test]]` entries found in any `Cargo.toml` across the workspace (confirmed via grep).

**Root Cause**: The top-level tests were created outside any crate's test harness, likely with the expectation they would be registered via workspace `[[test]]` entries or moved into a crate's `tests/` directory, but neither was completed.

**Impact**: These suites are dead code in CI regardless of whether a test job exists. The parity suite against OCHRE reference outputs and the ASHRAE 140 BESTEST validation tests cannot run until the wiring is completed.

### Finding 5: [Severity: medium]
**Description**: Clippy runs with only `-D warnings` (line 35), using the default lint level. The project has no `.clippy.toml` and does not enable pedantic or nursery lint groups, which catch common correctness issues (e.g., `cast_possible_truncation`, `cast_sign_loss`, `float_cmp`, `indexing_slicing`), performance anti-patterns (e.g., `large_stack_arrays`, `or_fun_call`), and style improvements that prevent bugs.

**Code Location**: `.github/workflows/ci.yml:35` — `cargo clippy --workspace -- -D warnings`

**Root Cause**: The clippy invocation was configured to match the development command (`docs/development.md:32`) without the stricter settings that CI should enforce as a gating check.

**Impact**: Code with potential correctness issues (e.g., lossy numeric casts, unchecked arithmetic, unexpected default behaviour from `unwrap_or` calls) passes CI without warning.

**Recommendation**: Add at minimum:
```bash
cargo clippy --workspace --all-targets --all-features -- \
  -D warnings \
  -W clippy::pedantic \
  -W clippy::nursery \
  -A clippy::module_name_repetitions \
  -A clippy::too_many_lines \
  -A clippy::cast_precision_loss
```
The `--all-targets` flag ensures tests, benches, and examples are also linted. The `--all-features` flag enables all feature-gated code paths in lint analysis. Allow attributes suppress noise that produces high false-positive rates in a domain-specific simulation codebase.

### Finding 6: [Severity: medium]
**Description**: Documentation tests (`cargo test` doc-tests) are never run. The project has no `cargo doc --no-deps` or `cargo test --doc` step. Doc-tests embedded in `///` comments (present in many public API functions across the workspace) are the primary mechanism for ensuring example code in documentation remains correct. Without CI verification, doc examples can silently become invalid.

**Code Location**: `.github/workflows/ci.yml` — no doc step exists. Documentation commands are absent from `docs/development.md` quick reference table (lines 407–426), suggesting doc-test discipline was never established.

**Root Cause**: Documentation tests are an overlooked CI responsibility. The `docs/development.md` reference table omits `cargo doc` and `cargo test --doc` entirely, indicating documentation test execution is not part of the current development workflow.

**Impact**:
1. Example code in public API doc comments (e.g., `crates/hares-core/src/dwelling/mod.rs` which has extensive documentation) may reference outdated types or APIs.
2. Intra-doc links (`[`TypeName`]` syntax) may silently break across refactors.

### Finding 7: [Severity: medium]
**Description**: The CI workflow lacks a build matrix. Only a single Rust toolchain (`dtolnay/rust-toolchain@stable`) is tested on a single OS (`ubuntu-latest`). The project's `rust-version` is set to `1.87` (`Cargo.toml:29`), but no CI job verifies the minimum supported Rust version (MSRV). Feature flag combinations (`check_invariants`, `observe`, `observe_detailed`, `profiling`, `actor_profiling`, `dst`) are never tested in combination. Some feature combinations are known to be critical — `check_invariants` guards per-timestep conservation checks and should always compile.

**Code Location**: `.github/workflows/ci.yml:22` — `dtolnay/rust-toolchain@stable` with no version matrix. No matrix dimension for features, OS, or Rust version exists.

**Root Cause**: The CI was bootstrapped for a single fast path (fmt + clippy) and never expanded.

**Impact**:
1. **MSRV drift**: Code using features from Rust >1.87 can merge without detection. Downstream consumers requiring MSRV compliance would encounter compilation failures.
2. **Feature combination breakage**: Enabling `observe_detailed` requires `observe` (by design, `crates/hares-core/Cargo.toml:12`), but feature-gated code may silently fail to compile under specific combinations.
3. **Single platform**: macOS-specific compilation issues (e.g., `libc` type differences, filesystem path handling in tempfile usage) are never caught.

### Finding 8: [Severity: low]
**Description**: The CI does not use `cargo nextest run`, which is the project's recommended test runner. `docs/development.md` line 31 documents `cargo nextest run` as the primary test command, and the ticket hooks use nextest. CI using plain `cargo test` would run test binaries sequentially, wasting CI minutes and diverging from the documented workflow.

**Code Location**: `.github/workflows/ci.yml` — no nextest installation step exists. `docs/development.md:271–278` documents nextest installation and usage.

**Root Cause**: Nextest is a recent addition to the development workflow (since it has its own installation step and dedicated documentation section), and CI was never updated.

**Impact**: Test execution time would be unnecessarily long if `cargo test` were used instead of nextest. More critically, developers would see different test behaviour locally vs. CI due to different test harnesses.

## Summary
- **Total findings**: 8
- **Critical / High / Medium / Low**: 2 / 2 / 3 / 1

## Recommendations

### 1. Add Rust test execution job (addresses Finding 1, Finding 8)
Add a job that builds and runs the full test suite using `cargo nextest run`:
```yaml
test:
  name: "cargo nextest run"
  runs-on: ubuntu-latest
  steps:
    - uses: actions/checkout@v4
    - uses: dtolnay/rust-toolchain@stable
    - uses: taiki-e/install-action@cargo-nextest
    - run: cargo nextest run --workspace --no-fail-fast
```
For deeper validation, add a second test matrix entry with release-mode invariant checking:
```yaml
    - run: cargo nextest run --workspace --release -F check_invariants --no-fail-fast
```

### 2. Wire top-level integration tests into workspace (addresses Finding 4, prerequisite for Finding 1)
Register the top-level test targets in `hares-core`'s `Cargo.toml` (since they depend on `hares-core` types):
```toml
[[test]]
name = "regression"
path = "../../tests/regression/mod.rs"

[[test]]
name = "parity"
path = "../../tests/parity/mod.rs"

[[test]]
name = "bestest"
path = "../../tests/bestest/mod.rs"

[[test]]
name = "conditioned_oracle"
path = "../../tests/conditioned_oracle.rs"
# ... repeat for remaining test files
```

### 3. Add Python test job (addresses Finding 2)
Add a matrix-based job that builds the PyO3 extension and runs pytest:
```yaml
python-test:
  name: "pytest (Python ${{ matrix.python-version }})"
  runs-on: ubuntu-latest
  strategy:
    matrix:
      python-version: ["3.13"]
  steps:
    - uses: actions/checkout@v4
    - uses: dtolnay/rust-toolchain@stable
    - uses: astral-sh/setup-uv@v5
      with:
        python-version: ${{ matrix.python-version }}
    - run: uv sync --group dev
    - run: uv run maturin develop
    - run: uv run pytest -m "not slow"
```
Consider a separate job or matrix entry for slow tests (`uv run pytest -m slow`) gated on `workflow_dispatch` to avoid per-commit overhead. Install optional HELICS dependency (`helics>=3.6.1`) to exercise the HELICS integration tests.

### 4. Add benchmark compilation check (addresses Finding 3)
```yaml
bench-check:
  name: "cargo bench --no-run"
  runs-on: ubuntu-latest
  steps:
    - uses: actions/checkout@v4
    - uses: dtolnay/rust-toolchain@stable
    - run: cargo bench --no-run --workspace
```

### 5. Strengthen clippy settings (addresses Finding 5)
```yaml
- run: >
    cargo clippy --workspace --all-targets --all-features --
    -D warnings
    -W clippy::pedantic
    -W clippy::nursery
    -A clippy::module_name_repetitions
    -A clippy::too_many_lines
    -A clippy::cast_precision_loss
```

### 6. Add documentation check (addresses Finding 6)
```yaml
doc:
  name: "cargo doc + doc-tests"
  runs-on: ubuntu-latest
  steps:
    - uses: actions/checkout@v4
    - uses: dtolnay/rust-toolchain@stable
    - run: cargo doc --no-deps --workspace --all-features
      env:
        RUSTDOCFLAGS: "-D warnings"
    - run: cargo test --doc --workspace
```

### 7. Add build matrix for feature flags and MSRV (addresses Finding 7)
```yaml
build-matrix:
  name: "build (${{ matrix.rust }}, ${{ matrix.features }})"
  runs-on: ubuntu-latest
  strategy:
    matrix:
      rust: ["1.87", "stable"]
      features:
        - ""
        - "check_invariants"
        - "observe"
        - "check_invariants,observe"
        - "check_invariants,observe,observe_detailed"
    fail-fast: false
  steps:
    - uses: actions/checkout@v4
    - uses: dtolnay/rust-toolchain@master
      with:
        toolchain: ${{ matrix.rust }}
    - run: cargo check --workspace --features "${{ matrix.features }}"
```

### 8. Proposed complete CI matrix summary

| Job | Purpose | Trigger |
|-----|---------|---------|
| `format` | Code formatting (exists) | every push/PR |
| `clippy` | Linting with pedantic/nursery (enhanced) | every push/PR |
| `doc` | Doc build + doc-tests | every push/PR |
| `test` | `cargo nextest run` (debug) | every push/PR |
| `test-release` | `cargo nextest run --release -F check_invariants` | every push/PR |
| `python-test` | `maturin develop` + pytest (fast) | every push/PR |
| `python-test-slow` | pytest slow tests | `workflow_dispatch` only |
| `bench-check` | `cargo bench --no-run` | every push/PR |
| `build-matrix` | Feature flag × Rust version matrix | every push/PR |

## References / Citations
- `docs/development.md:31` — recommends `cargo nextest run` as primary test runner
- `docs/development.md:89–90` — notes Python tests require `uv run maturin develop` first
- `docs/development.md:197` — recommends `cargo nextest run --release -F check_invariants` for CI
- `docs/development.md:407–426` — quick reference table (doc commands absent)
- `pyproject.toml:73–77` — pytest configuration with testpaths and markers
- `pyproject.toml:65–71` — maturin build configuration
- `pyproject.toml:39–46` — dev dependency group (pytest, maturin, xdist)
- `crats/hares-core/Cargo.toml:45–58` — benchmark registrations
- `crates/hares-envelope/Cargo.toml:25–28` — envelope benchmark registration
- `crats/hares-core/Cargo.toml:8–14` — feature flag definitions
- `docs/reviews/infrastructure/infra-01-dead-module-aggregation-check.md` — confirms top-level `tests/` are orphaned from workspace
