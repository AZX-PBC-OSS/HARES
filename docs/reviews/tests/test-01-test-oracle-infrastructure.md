# Test oracle infrastructure: BESTEST, conditioned/freefloat oracles, parity corpus
**Review ID**: test-01
**Category**: tests
**Date**: 2026-05-26

## Files Reviewed
- `tests/bestest/mod.rs` (692 lines) — BESTEST core case driver, run/compare/evaluate logic
- `tests/bestest/cases.rs` (133 lines) — core + extended case definitions, fixture paths
- `tests/bestest/reference_bands.rs` (126 lines) — ASHRAE 140 reference band data + metric enum
- `tests/bestest/bestest_diagnostic.rs` (214 lines) — diagnostic heat-balance tracing
- `tests/conditioned_oracle.rs` (1042 lines) — conditioned HVAC oracle (ideal + dynamic modes)
- `tests/freefloat_oracle.rs` (~1238 lines) — free-float envelope oracle + solar-override parity
- `tests/envelope_oracle.rs` (~1015 lines) — envelope ballpark comparison (static constants)
- `tests/structural_envelope_oracle.rs` (~1138 lines) — static RC construction validation against ASHRAE reference
- `tests/parity/mod.rs` (809 lines) — parity corpus driver, metric extraction, comparison
- `tests/parity/corpus.rs` (94 lines) — fixture discovery, completeness check
- `tests/parity/tolerance.rs` (134 lines) — tolerance constants and check functions
- `tests/python/generate_conditioned_oracle.py` (251 lines) — conditioned reference generator
- `tests/python/generate_freefloat_oracle.py` (198 lines) — freefloat reference generator
- `tests/python/generate_parity_reference.py` (191 lines) — parity parquet reference generator
- `tests/python/test_ochre_parity.py` (368 lines) — Python-side parity bench/correctness test
- `tests/fixtures/parity/README.md` (52 lines) — parity corpus documentation
- `tests/fixtures/parity/extract_ochre_rc.py` (131 lines) — OCHRE RC reference extraction
- `tests/fixtures/parity/ashrae_rc_reference.json` (167 lines) — independently derived RC reference
- `tests/fixtures/bestest/600.toml`, `600ff.toml`, `900.toml`, `900ff.toml` — BESTEST fixture configs
- `pyproject.toml` (78 lines) — dependency version specifications

## Vendor/Reference Files Consulted
- `vendors/OCHRE/test/` — OCHRE's own test suite (`test_envelope.py`, `test_rcmodel.py`, `test_statespacemodel.py`)
- `vendors/EnergyPlus/testfiles/` — EnergyPlus testfiles including `HybridModel_4Zone_Solve_Infiltration_free_floating.idf`, IECC climate zone fixture (`US+SF+CZ4A+hp+crawlspace+IECC_2006_VRF.idf`)
- `vendors/OCHRE/ochre/defaults/Envelope/` — OCHRE envelope defaults (film resistances, materials)
- `docs/reviews/scripts/scr-03-ochre-bestest-600-py.md` — prior review documenting BESTEST script issues
- `docs/findings/bestest_rca.md` — HARES BESTEST root-cause analysis
- `scripts/ochre_bestest_600.py` — OCHRE-side BESTEST runner (reviewed in scr-03)

---

## Findings

### Finding 1: [Severity: critical] All BESTEST oracle tests are ignored — no ASHRAE 140 validation runs in CI

**Description**: All five core BESTEST test functions (`bestest_case_600`, `bestest_case_900`, `bestest_case_600ff`, `bestest_case_900ff`, `bestest_case_640`) carry `#[ignore = "pending remaining physics fixes: B1, S4/S5, and other consolidated.md items"]`. No BESTEST case runs in any CI pipeline. The reference bands are correctly populated from ASHRAE 140-2017 (Tables B8-2, B8-3a), the fixture geometry matches the ASHRAE 140 specification exactly, and the comparison infrastructure (`evaluate_bands`, `ReferenceBand::contains`) is sound — but the tests are permanently disabled with no gating flag that could be flipped once physics fixes land.

**Code Location**: `tests/bestest/mod.rs:77,99,120,143,167` — all five `#[test]` functions have `#[ignore = "..."].`

**Root Cause**: Known unfixed physics defects (internal gain radiant fraction B1, warmup/initialization S4/S5). These are tracked in `docs/findings/consolidated.md` but there is no ticket reference in the ignore message, no `#[cfg(feature = "bestest")]` gate to enable opt-in CI, and no `#[allow(unused)]` annotation to prevent silent decay.

**Impact**: The single most valuable validation test suite — ASHRAE 140 published oracle bands against identical building descriptions — is completely absent from any automated regression detection. A physics regression that pushes a BESTEST metric outside its band will never be caught until someone manually runs `cargo test --ignored`. This defeats the purpose of having a validation oracle.

---

### Finding 2: [Severity: critical] Parity corpus has zero active fixtures — no reference_output.parquet files exist

**Description**: The parity corpus driver (`tests/parity/mod.rs:89-157`) calls `discover_fixtures()` which requires each fixture directory to contain `reference_output.parquet` alongside `building.xml`, `schedule.csv`, `weather.epw`, and `config.toml`. Of the 12 fixture directories under `tests/fixtures/parity/`, **none** contain a `reference_output.parquet` file. Every directory is classified as `DiscoveredFixture::Incomplete`, the test prints a skip message, and then exits having validated nothing.

The corpus directory contains inputs for 10 climate-zone-named fixtures (`cz2a_*`, `cz4a_*`, `cz5a_*`, `cz6b_*`) plus `beopt_smoke_1h` (which has only `ochre_reference.csv`, wrong format) and `resstock_bldg0112631_24h`. All lack reference outputs.

**Code Location**: `tests/parity/corpus.rs:5-11` requires `reference_output.parquet`; `tests/parity/mod.rs:110-113` exits with "no complete fixtures found."

**Root Cause**: Reference parquet files are generated by `tests/python/generate_parity_reference.py`, which requires OCHRE to be importable and installed (via `uv run --group ochre`). This script has apparently never been run for the 10 climate-zone fixtures. The `generate_parity_reference.py` script works correctly for a single fixture at a time but no batch run has been committed.

**Impact**: The parity corpus — the test suite designed to cover diverse climate zones with different equipment mixes — validates exactly zero fixtures. All zone-temperature, HVAC energy, water-heater energy, battery SOC, and equipment-mode-cycle comparisons are dead code until someone generates `reference_output.parquet` files.

---

### Finding 3: [Severity: high] Freefloat and conditioned oracle tests are gated behind `#[cfg(feature = "observe")]` and never run in standard CI

**Description**: Both `tests/conditioned_oracle.rs` and `tests/freefloat_oracle.rs` wrap their entire test module in `#[cfg(feature = "observe")]` (`freefloat_oracle.rs:10-11`, `conditioned_oracle.rs:18-19`). The `observe` feature is a debugging/instrumentation feature, not a test-gating feature. Standard `cargo test` (with no `--features observe`) compiles these modules but excludes all test functions, producing zero test results. The freefloat oracle's solar-override parity tests (lines 1192+) are also gated behind this feature.

**Code Location**: `tests/freefloat_oracle.rs:10-11`, `tests/conditioned_oracle.rs:18-19`

**Root Cause**: These tests use the `Dwelling::enable_observer()` API to collect per-step telemetry (component gains, zone temperatures, surface irradiance). The observer is currently behind the `observe` feature flag in `hares-core`. The test authors likely intended this for diagnostic environments but never ported the oracle tests to a standard feature or separate test binary where observer instrumentation is always available.

**Impact**: Three freefloat scenarios (spring 72h, summer 48h, winter 48h) and six conditioned scenarios (ideal + dynamic each across three seasons) produce zero validation signal in CI. The reference data exists and the comparison logic is sound — but neither ever executes.

---

### Finding 4: [Severity: high] Parity corpus covers only 4 of 10 IECC climate zones and lacks hot-dry, marine, very-cold, and subarctic

**Description**: The parity README (`tests/fixtures/parity/README.md:27-40`) documents 10 fixture directories, but the climate zone coverage is:

| IECC Zone | Climate Type | Covered? | Fixture |
|-----------|-------------|----------|---------|
| 1A | Hot-humid | No | — |
| 2A | Hot-humid | **Yes** | `cz2a_gas_furnace_ac_res_wh`, `cz2a_pv_ev` |
| 2B | Hot-dry | No | — |
| 3A/B/C | Warm-humid/dry/marine | No | — |
| 4A | Mixed-humid | **Yes** | `cz4a_ashp_hpwh`, `cz4a_pv_only`, `cz4a_pv_battery`, `cz4a_battery_only` |
| 4B/C | Mixed-dry/marine | No | — |
| 5A | Cool-humid | **Yes** | `cz5a_minisplit_gas_wh`, `cz5a_ev_only` |
| 5B/C | Cool-dry/marine | No | — |
| 6A/B | Cold-humid/dry | **Partially** | `cz6b_resistance_res_wh`, `cz6b_pv_battery_ev` (6B only) |
| 7 | Very cold | No | — |
| 8 | Subarctic | No | — |

Key missing climate types: **hot-dry** (2B/3B — Phoenix, Las Vegas — solar-dominated cooling), **marine** (3C/4C — San Francisco, Seattle — mild with high humidity), **very cold** (7 — Fairbanks — heating-dominated, frost penetration), and **subarctic** (8 — extreme heating).

Furthermore, the building archetypes across zones are not documented — there is no manifest describing whether each fixture uses a slab-on-grade vs. crawlspace vs. basement, whether it has attic vs. cathedral ceiling, or what vintage/insulation level is modeled. Without this documentation, it is impossible to verify that the corpus exercises the full range of HARES envelope physics.

**Code Location**: `tests/fixtures/parity/README.md:27-40`, `tests/fixtures/parity/*/` directory listing

**Root Cause**: The corpus appears to have been scaffolded from a ResStock sampling of a few climate zones (2A, 4A, 5A, 6B) without a systematic coverage design. The 4A zone has disproportionate weight (4 fixtures vs. 1-2 for others) based on equipment mix diversity rather than climate diversity.

**Impact**: Heating-dominated physics (zones 7-8, extreme cold) and solar-driven cooling physics (zones 2B-3B, hot-dry with large diurnal swing) are not exercised. A regression in, e.g., ground-coupling for frost-depth penetration or window IAM at high incidence angles (high-latitude zones 7-8) would go undetected by the parity corpus.

---

### Finding 5: [Severity: high] BESTEST fixture SHGC (0.789) is inconsistent with ASHRAE 140-2017 specification

**Description**: All five HARES BESTEST TOML fixtures use `shgc = 0.789` for the south-facing windows (e.g. `tests/fixtures/bestest/600.toml:208,216`). The prior review scr-03 documented that the OCHRE BESTEST script uses `SHGC = 0.767` (`scripts/ochre_bestest_600.py:113`). ASHRAE 140-2017 specifies the window as "double pane, clear glass, 3 mm panes, 13 mm air gap" — which per ASHRAE HoF Ch. 15 Table 10 yields SHGC ≈ 0.76 (center-of-glass), not 0.789. The HARES value of 0.789 corresponds to the full-window NFRC-rated SHGC (including frame effects subtracted), not center-of-glass.

**Code Location**: `tests/fixtures/bestest/600.toml:208,216`; `tests/fixtures/bestest/600ff.toml:202,210`; `tests/fixtures/bestest/900.toml:208,216`; `tests/fixtures/bestest/900ff.toml:202,210`

**Root Cause**: The provenance of 0.789 is undocumented. The value may originate from an EnergyPlus test file or a third-party BESTEST implementation, but no citation exists in the fixtures or documentation. The 2.9% difference relative to 0.767 translates to a ~2.9% solar gain discrepancy that would compound over an annual simulation.

**Impact**: For BESTEST Case 600 with 12 m² of south-facing glazing under Denver TMY3, a 2.9% SHGC delta could shift annual heating/cooling loads by tens of kWh — potentially pushing results across ASHRAE 140 reference band boundaries independently of actual physics model quality.

---

### Finding 6: [Severity: medium] Freefloat oracle tolerance of 5.0°C MAE is not physically justified and masks model divergence

**Description**: The freefloat oracle sets indoor temperature MAE tolerance at 5.0°C (`tests/freefloat_oracle.rs:826`), justified as: "ASHRAE 140-2017 Table B8-3a publishes ±1 °C residuals on annual-mean zone temperatures across validated simulation tools. For a short free-floating window with no HVAC to clamp zone behavior, infiltration and solar-distribution model differences broaden that residual; 5 °C MAE is a conservative bound."

This reasoning is flawed: ASHRAE 140's ±1°C annual-mean band across tools like EnergyPlus, DOE-2, and ESP-r reflects the spread between *independently validated* simulation engines — it is not a free pass to accept 5× larger error against a single reference implementation. A 5°C MAE bound over a 48-72h window means peak instantaneous errors could be 15-20°C and the test would still pass, which would mask fundamental errors in RC construction, solar distribution, or infiltration physics.

The attic tolerance of 8.0°C MAE is similarly unjustified.

**Code Location**: `tests/freefloat_oracle.rs:823-828` (indoor), `834-841` (attic)

**Root Cause**: These bands were chosen to make the tests pass against OCHRE output given known HARES-OCHRE model divergence, rather than being derived from physics-based error budgets (e.g., ASHRAE HoF Ch. 19 uncertainty propagation for surface heat balance). The comment at line 1162 ("Tolerance assertions -- widen these until we verify envelope physics") explicitly acknowledges the defensiveness.

**Impact**: The freefloat oracle would pass even with severe envelope errors, making it a low-sensitivity test that provides false confidence.

---

### Finding 7: [Severity: medium] Oracle generation scripts do not pin OCHRE's transitive dependency versions — reference data is not guaranteed reproducible

**Description**: The oracle generation scripts (`generate_conditioned_oracle.py`, `generate_freefloat_oracle.py`, `generate_parity_reference.py`) record the OCHRE git hash (`ochre_git_hash()`) in generated `config.json` files. However, OCHRE's own dependencies are specified with minimum-version bounds only in `pyproject.toml` (`scipy>=1.17.1`, `numpy>=2.4.3`, `pvlib>=0.15.0`, `numba>=0.64.0`, etc.). Different machines installing OCHRE at different times will receive different dependency versions, which can produce numerically different simulation output (e.g., scipy linear solver versions, numpy floating-point behavior, numba JIT compilation).

The `ochre_reference.csv` files committed to the repository (in `tests/fixtures/`) have no metadata recording the OCHRE dependency versions that produced them beyond the git hash.

**Code Location**: `pyproject.toml:47-60` (ochre dependency group, min-version bounds); `tests/python/generate_freefloat_oracle.py:176` (config.json omits dependency versions)

**Root Cause**: Python's standard dependency management (pip/uv) does not natively support transitive-dep locking in the way Cargo.lock does. The `uv.lock` file in the repo records `vendors/OCHRE` as a path dependency but the REPO_ROOT `uv.lock` does not track the vendored OCHRE's own transitive dependencies.

**Impact**: The same version of OCHRE code (same git hash) run on two different machines with different scipy/numpy/pvlib versions could produce different reference CSV outputs. This means reference data claimed as "reproducible" may not actually be bit-for-bit reproducible. For integration-level envelope comparisons where MAE tolerances are 0.5-5.0°C, this is unlikely to matter — but for equipment-level parity or battery SOC comparisons at 0.01 absolute MAE tolerance, floating-point divergence from dependency version drift could cause spurious failures.

---

### Finding 8: [Severity: medium] Parity tolerance bands for short-window HVAC (25%) and peak power (20%) are not physically derived

**Description**: `tests/parity/tolerance.rs:17` sets `SHORT_WINDOW_HVAC_ENERGY_REL_PCT_MAX = 25.0`, justified as: "Single-cycle phase offsets between HARES and OCHRE routinely shift integrated energy by 5–25 % for fixtures whose duration barely exceeds the on-time of one compressor cycle. Tighter bands require ≥24 h windows." Similarly, `PEAK_HVAC_POWER_REL_PCT_MAX = 20.0` (line 27) comments about a "step-0 ideal-capacity back-solve" improvement.

These are noise-floor bands, not physics-grounded tolerances. A 25% HVAC energy band means that if OCHRE simulates 4.0 kWh of heating over a 1-hour window, HARES could report anywhere from 3.0 to 5.0 kWh and "pass." This is too wide to detect meaningful physics regressions — a mis-specified U-value that changes wall conduction by 15% (a serious error in building physics) would be invisible under a 25% tolerance.

The annual water heater energy tolerance of 0.5% (line 18) is appropriately tight for a continuous-load comparison, confirming that the 25% band is a workaround for short-duration phase-offset noise, not a principled bound.

**Code Location**: `tests/parity/tolerance.rs:17,20,27`

**Root Cause**: Short-duration (≤1h) parity comparisons between HARES and OCHRE are dominated by thermostat-cycle phase offset — when HARES decides to turn on a compressor at minute 3 and OCHRE decides at minute 5, the integrated energy over 60 minutes diverges by the ratio of on-time difference. This is a test design issue (comparing short windows), not a physics accuracy issue.

**Impact**: The parity corpus cannot distinguish between a 25% physics error and a 25% phase-offset noise floor on short windows. Any meaningful HVAC energy parity comparison requires ≥24h windows or a different comparison methodology (e.g., aggregate annual energy, or phase-aligned cycle comparison).

---

### Finding 9: [Severity: medium] `beopt_smoke_1h` fixture uses stale CSV format but parity driver expects Parquet

**Description**: `tests/fixtures/parity/beopt_smoke_1h/` contains only `ochre_reference.csv` — a CSV format file. The parity driver (`tests/parity/mod.rs:346`) calls `read_parquet_columns()` which uses the Arrow Parquet reader. This fixture is used by `tests/envelope_oracle.rs` (which parses CSV directly at line 536) but is **incompatible** with the parity corpus driver. The fixture is listed alongside the climate-zone fixtures but serves a different test and uses a different data format.

**Code Location**: `tests/fixtures/parity/beopt_smoke_1h/ochre_reference.csv`; `tests/parity/corpus.rs:11` (requires `reference_output.parquet`)

**Root Cause**: `beopt_smoke_1h` was likely the original parity fixture before the corpus was expanded to climate zones. The parity corpus driver evolved to expect Parquet but `beopt_smoke_1h` was never migrated.

**Impact**: Minor — `beopt_smoke_1h` will be silently skipped as `Incomplete` by the parity driver. The `envelope_oracle.rs` test still uses it correctly.

---

### Finding 10: [Severity: low] `AnnualHeatingLoadKwh` and `AnnualHeatingEnergyKwh` are duplicate BESTEST metrics

**Description**: `tests/bestest/reference_bands.rs:5-9` defines both `AnnualHeatingLoadKwh` and `AnnualHeatingEnergyKwh` as separate enum variants. In `tests/bestest/mod.rs:229-230`, the `run_case()` function inserts the same `heating_kwh` value under both keys. These are semantically identical in the BESTEST context (where the HVAC model is an ideal system with COP=1). Having both is redundant and adds confusion.

**Code Location**: `tests/bestest/reference_bands.rs:8` (AnnualHeatingLoadKwh), `:9` (AnnualHeatingEnergyKwh); `tests/bestest/mod.rs:228-230`

**Root Cause**: The `AnnualHeatingEnergyKwh` variant was added for Case 640 (setback thermostat), which uses "heating energy" terminology per ASHRAE 140. Case 600 uses "heating load." Both are assigned the same value.

**Impact**: Low — no functional bug, but the duplication adds cognitive overhead when reading the metric definitions and could cause confusion about whether heating load and heating energy should differ (e.g., if a heat pump model with COP>1 is later added to BESTEST).

---

## Summary
- **Total findings**: 10
- **Critical**: 2 (Findings 1, 2)
- **High**: 4 (Findings 3, 4, 5, 6)
- **Medium**: 3 (Findings 7, 8, 9)
- **Low**: 1 (Finding 10)

## Recommendations

1. **Unblock BESTEST with a feature gate**: Replace `#[ignore = "..."]` with `#[cfg(feature = "bestest")]` on each test, and add a CI job definition `cargo test --features bestest` so the oracle is always compilable and opt-in runnable. Add ticket references to the ignore messages.

2. **Generate and commit reference_output.parquet**: Run `uv run --group ochre python tests/python/generate_parity_reference.py` for all 10 climate-zone fixtures and commit the resulting `reference_output.parquet` files. Consider scripting this as a CI pre-commit hook guarded by a `HARES_GENERATE_REFERENCES=1` env var.

3. **Move oracle tests out of `#[cfg(feature = "observe")]`**: Either promote observer instrumentation to always-available (behind a runtime toggle, not a compile-time feature), or create a separate `oracle-tests` feature that enables observer + tests together. The `observe` feature should not gate validation tests.

4. **Expand parity corpus to cover all IECC climate types**: Add fixtures for: 2B (Phoenix — hot-dry), 3C (San Francisco — marine), 4C (Seattle — marine), 7 (Fairbanks — very cold), and 8 (Barrow — subarctic). Document each fixture's archetype (foundation type, attic type, vintage, insulation level) in the parity README.

5. **Resolve and document the BESTEST window SHGC**: Trace the provenance of 0.789. If it's a full-window NFRC-rating, document the source. If ASHRAE 140 specifies center-of-glass, correct the value to 0.76 with a citation to ASHRAE 140-2017 Appendix B.

6. **Tighten freefloat oracle tolerances**: Reduce indoor MAE from 5.0°C to 1.5°C (1.5× ASHRAE 140 band, acknowledging short-window vs. annual difference) or justify the wider band with uncertainty-propagation calculations from ASHRAE HoF Ch. 19.

7. **Pin OCHRE dependency versions for reference generation**: Create a `requirements-ochre-reference.txt` or use `uv pip compile` on the OCHRE vendor directory's `pyproject.toml` to produce a lockfile of pinned dependency versions, and record this lockfile hash in generated `config.json`.

8. **Lengthen parity comparison windows or switch methodology**: For meaningful HVAC energy parity, use ≥24h windows (as the conditioned oracle already does with 48-72h scenarios) or compare aggregate annual/long-duration energy rather than short-window energy. Short-window (≤1h) HVAC comparisons should be removed or explicitly marked as "phase-offset dominated, diagnostic only."

9. **Remove or unify duplicate BESTEST metric**: Drop `AnnualHeatingLoadKwh` and use only `AnnualHeatingEnergyKwh`, or document clearly that they are semantically identical in the ideal-HVAC context.

10. **Document provenance of tolerance values**: Every tolerance constant in `tests/parity/tolerance.rs` should cite its source (ASHRAE standard section, validation publication, or empirical distribution observed across fixtures). Arbitrary-seeming numbers without citations increase maintenance burden and reduce trust in test failures.

## References / Citations
- ASHRAE 140-2017, Standard Method of Test for the Evaluation of Building Energy Analysis Computer Programs, Tables B8-2 (annual loads) and B8-3a (free-float temperatures)
- ASHRAE Handbook of Fundamentals 2021, Ch. 15 (Fenestration), Ch. 18 (Heat transfer), Ch. 19 (Energy estimating), Ch. 26 (Ventilation and infiltration)
- EnergyPlus Engineering Reference v25.1.0, §3.2 (Simple Window Model — Arasteh et al., LBNL 2009), §9.4-9.5 (TARP/DOE-2 convection models)
- IECC 2021 Climate Zone map: zones 1A-8 by county
- NREL ResStock Technical Documentation (NREL/TP-5500-64459), Appendix C (archetype build-ups)
- BEopt Residential Construction Reference, §3.3 (attic geometry)
- Prior HARES review: `docs/reviews/scripts/scr-03-ochre-bestest-600-py.md` (BESTEST SHGC discrepancy)
- HARES finding: `docs/findings/bestest_rca.md` (physics defect inventory)
