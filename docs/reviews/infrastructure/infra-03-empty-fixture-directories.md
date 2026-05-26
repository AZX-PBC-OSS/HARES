# Empty golden/ and schedules/ test fixture directories
**Review ID**: infra-03
**Category**: infrastructure
**Date**: 2026-05-26

## Files Reviewed
tests/fixtures/golden/ tests/fixtures/schedules/

## Vendor/Reference Files Consulted

None applicable — the review concerns directory structure and fixture population only.

## Findings

### Finding 1: [Severity: medium]
**Description**: `tests/fixtures/golden/` is an empty directory containing only `.gitkeep`. The name implies golden-file regression testing (compare current output against a known-good reference file), but no golden files exist, no test code references the directory, and no snapshot/golden testing framework (insta, goldenfile, expect_test) is present in `Cargo.toml` or `Cargo.lock`. The directory was created in the initial bootstrap commit (`34e201e`) and has never been populated.

**Code Location**: `tests/fixtures/golden/.gitkeep:1` (empty file)
`tests/fixtures/golden/` — only entry is `.gitkeep`

**Root Cause**: The directory was scaffolded at project inception as a placeholder for future golden-file tests but the implementation was never followed through. The codebase already has regression protection via:
- `tests/regression/` — determinism, fleet-scale, checkpoint-restart, multi-instance, and aggregation checks (`tests/regression/mod.rs:32`)
- `tests/conditioned_oracle.rs` — OCHRE-vs-HARES channel comparisons at 15% tolerance over 72h windows
- `tests/envelope_oracle.rs` — envelope-channel OCHRE oracle
- `tests/parity/` — component-level parity suites comparing HARES against OCHRE reference outputs
- `tests/bestest/` — ANSI/ASHRAE 140-2017 Cases 600–900FF against published reference bands
- `tests/regression/smoke_test.rs:327-446` — physics-bounds smoke tests with NaN/sign-flip guards

These existing suites provide substantial regression coverage, but they are all *active-comparison* tests (run-then-compare) rather than *golden-file* tests (compare-against-precomputed). Golden-file tests offer a complementary benefit: they catch unintentional output drift without requiring a reference simulator to be run, and they document expected output for new contributors.

**Impact**: The empty `golden/` directory is dead infrastructure that:
1. Creates false expectations of golden-file test coverage that does not exist
2. Wastes reviewer/contributor time investigating whether golden tests exist
3. Adds clutter to the repository tree
4. Represents an acknowledged-but-unrealized gap in regression protection — golden-file tests would provide an additional layer of defense against unintentional output drift that the current parity/oracle suites (which depend on OCHRE being available) cannot catch independently

### Finding 2: [Severity: low]
**Description**: `tests/fixtures/schedules/` is an empty directory containing only `.gitkeep`. No test code references this path. Schedule fixture files (CSV occupancy/appliance profiles) already exist in the codebase at their point of use: `tests/fixtures/parity/*/schedule.csv`, `tests/fixtures/resstock/*/in.schedules.csv`, `data/examples/BEopt_example_schedule.csv`, and `data/examples/bldg0112631_schedule.csv`. The codebase follows a co-located fixture pattern where schedules live alongside their paired building XML and weather EPW files in scenario-specific directories.

**Code Location**: `tests/fixtures/schedules/.gitkeep:1` (empty file)
`tests/fixtures/schedules/` — only entry is `.gitkeep`

**Root Cause**: Same as Finding 1 — the directory was scaffolded at bootstrap as a placeholder for a centralized schedule fixture store, but the codebase organically adopted a co-located fixture pattern (schedule, building, weather all in the same scenario directory). The centralized directory was never populated and the co-located pattern proved to be the correct design. Example of the prevailing pattern: `tests/regression/helpers.rs:33-51` loads BEopt and ResStock fixtures from `data/examples/`, with schedule, HPXML, and weather files as siblings; `tests/bestest/mod.rs:192` reads fixture path as a single directory containing config, weather, and reference data together.

**Impact**: Similar to Finding 1 but lower severity because:
1. The schedules fixture need is already satisfied by co-located fixtures
2. A centralized `schedules/` directory would be an anti-pattern given the co-located design
3. The empty directory is unambiguous dead infrastructure

### Finding 3: [Severity: low]
**Description**: The `.gitkeep` files in these directories are empty (zero bytes) and contain no documentation about what fixtures are expected, why the directories exist, or links to planned implementation. Other fixture directories in the codebase that use `.gitkeep` alongside real fixtures (e.g., `tests/fixtures/hpxml/.gitkeep` co-exists with `tests/fixtures/hpxml/README.md` which documents the curated HPXML sample set and its source) demonstrate the expected pattern.

**Code Location**: `tests/fixtures/golden/.gitkeep:1` and `tests/fixtures/schedules/.gitkeep:1`

**Root Cause**: The `.gitkeep` files were created with no content at bootstrap, and the convention of documenting fixture expectations in README files (as done in `tests/fixtures/hpxml/README.md:1-32` and `tests/fixtures/parity/README.md:1-52`) was established later but never back-applied.

**Impact**: No documentation exists to explain the purpose of these directories to new contributors who encounter them.

## Summary
- Total findings: 3
- Critical: 0 / High: 0 / Medium: 1 / Low: 2

## Recommendations

1. **Remove `tests/fixtures/golden/`** unless golden-file testing is actively planned for the next development cycle. If the directory is kept, add a `README.md` documenting:
   - The intended golden-file testing approach (framework, file format, generation procedure)
   - A link to a tracking issue or milestone for the planned implementation
   - Example: "Golden files will be generated by `cargo test -- generate-goldens` and stored here. Compare with `cargo test -- check-against-goldens`. See issue #XXXX."

2. **Remove `tests/fixtures/schedules/`** — the co-located fixture pattern (schedule files alongside building + weather in scenario directories) is well-established and appropriate. A centralized schedule directory would conflict with this design.

3. **If either directory is retained**, populate the `.gitkeep` or add a `README.md` with the expected fixtures and a link to planned implementation, following the precedent set by `tests/fixtures/hpxml/README.md`.

4. **Consider introducing golden-file testing** as a complement to the existing regression suite. While the current parity/oracle/bestest suites provide strong regression protection, they all depend on running OCHRE or EnergyPlus as a reference. Golden-file tests (using a framework like `insta`, `expect_test`, or a custom snapshot approach) would:
   - Provide regression detection without requiring a reference simulator
   - Document expected output for new contributors reviewing the test corpus
   - Catch unintentional output drift in CI without the runtime cost of running reference simulators
   - A practical starting point: golden-file a few key scalar metrics (total annual energy by end-use, peak zone temperatures) for the existing BESTEST and BEopt fixtures

## References / Citations
- `tests/regression/mod.rs:32-97` — Full regression suite aggregating determinism, fleet-scale, checkpoint-restart, multi-instance, and aggregation checks
- `tests/regression/smoke_test.rs:1-682` — Physics-bounds smoke tests with NaN/sign-flip guards; explicitly states these are NOT parity tests
- `tests/conditioned_oracle.rs` — OCHRE-vs-HARES conditioned-dynamic comparisons
- `tests/envelope_oracle.rs` — Envelope-channel OCHRE oracle
- `tests/parity/mod.rs` — Component-level parity suites
- `tests/bestest/mod.rs` — ANSI/ASHRAE 140-2017 reference band validation
- `tests/fixtures/hpxml/README.md:1-32` — Exemplar of documented fixture directory with curated content
- `tests/fixtures/parity/README.md:1-52` — Exemplar of documented fixture directory with co-located building/schedule/weather pattern
- `tests/regression/helpers.rs:33-68` — Test helper loading BEopt and ResStock fixtures from `data/examples/` (co-located pattern)
- Commit `34e201e` — Initial bootstrap creating `.gitkeep` files with no content
