# HANDOFF items remaining as pre-release technical debt
**Review ID**: infra-06
**Category**: infrastructure
**Date**: 2026-05-26

## Files Reviewed
- `docs/HANDOFF.md` (commit `ba133ca`, lines 1–322)

## Vendor/Reference Files Consulted
- `crates/hares-envelope/tests/bestest_900ff_root_cause.rs` — root cause verification for Item 1
- `crates/hares-envelope/src/longwave_radiation.rs` — solar absorptance constant (Item 3a)
- `crates/hares-envelope/src/boundary_rc.rs` — air density constant (Item 3b)
- `crates/hares-physics/src/constants.rs` — occupant gain constants (Item 3c)
- `crates/hares-core/src/dwelling/conversions.rs` — mass multiplier match (Item 3d)
- `crates/hares-physics/src/infiltration.rs` — garage ELA coefficients (Item 3e)
- `crates/hares-equipment/src/hvac/thermostat.rs` — deadband offset (Item 3f)
- `crates/hares-io/tests/hpxml_parity.rs` — stale `#[ignore]` comments (Item 4)
- `tests/bestest/mod.rs` — BESTEST test status (Items 1, 2, 7)
- `.github/workflows/ci.yml` — CI pipeline (Item 6)
- `pyproject.toml` — pytest configuration (Item 6)
- `tests/bestest/reference_bands.rs`, `tests/bestest/cases.rs` — ASHRAE 140 bands
- OCHRE reference: `vendors/OCHRE/` — ballpark reference only (per repo rule `feedback_ashrae_not_ochre.md`)

---

## Findings

### Finding 1: BESTEST 900FF root cause identified but fix not applied; all BESTEST tests now ignored [Severity: critical]

**Description**: The HANDOFF document identifies Case 900FF as the "only genuinely-wide physics gap" with a measured min-zone-temperature +2.50 °C above the ASHRAE 140-2017 band. Seven root-cause hypotheses were listed with a detailed investigation protocol. Subsequent work has confirmed two root causes via the dedicated regression test file `crates/hares-envelope/tests/bestest_900ff_root_cause.rs`:
1. Initial zone temperature of 21 °C (HVAC setpoint default) for a free-float building with no HVAC — concrete walls start unrealistically warm (~2.4 °C of the 2.50 °C outlier).
2. Internal gains 100% convective; EnergyPlus BESTEST IDF specifies Fraction Radiant = 0.3 (~0.295 °C impact).

RC discretization, zone air density, interior LWR, and TARP film coefficients have all been ruled out by measurement.

**HOWEVER**: The root cause is documented but the fix has NOT been applied. The actual test at `tests/bestest/mod.rs:142-148` still carries both `#[should_panic]` and `#[ignore]`. More critically, ALL five BESTEST tests (600, 600FF, 640, 900, 900FF) now carry `#[ignore]` attributes (lines 77, 98, 120, 142, 167) — a **regression from the HANDOFF state** where "all 3013 lib tests + all integration suites pass with two documented xfails (BESTEST 900 / 900FF)," meaning Cases 600, 600FF, and 640 previously PASSED.

**Code Location**:
- Root cause analysis: `crates/hares-envelope/tests/bestest_900ff_root_cause.rs` (entire file, 303 lines)
- Ignored tests: `tests/bestest/mod.rs:77-81` (600), `:98-103` (900), `:120-123` (600FF), `:142-148` (900FF), `:167-171` (640)

**Root Cause**: The initialization fix (setting initial zone temp to outdoor-appropriate values for free-float buildings, adding radiant fraction to internal gains) has been diagnosed but the code changes were not implemented. The blanket `#[ignore]` on previously-passing tests (600, 600FF, 640) may indicate a regression in auxiliary physics (B1 internal gain radiant fraction, S4/S5 warmup initialization) referenced in the consolidated findings.

**Impact**:
- Five BESTEST tests are dead code in CI — no ASHRAE 140 validation runs automatically
- A previously passing test (Case 600FF confirmed pass in HANDOFF line 28–30) is now ignored, potentially masking new regressions
- The 900FF root cause fix (which would also resolve 900 per HANDOFF line 76–82) remains unmerged despite diagnosis
- Per `infra-02`, the `#[should_panic]` pattern on 900FF is a soundness hazard that could produce false PASSes

**Mapping to review areas**:
- BESTEST 900FF: `test-01` (BESTEST validation), `infra-02` (xfail soundness), `envelope-10` (steady-state initialization)
- BESTEST 900: `envelope-11` / `envelope-12` (RC topology / tolerance), `core-03` (solver feedback loop)
- Internal gain radiant fraction: `core-10` (occupancy gains), `wiring-08` (interior LWR fraction)

---

### Finding 2: Solar absorptance default corrected to EnergyPlus 0.70 — RESOLVED [Severity: low]

**Description**: The HANDOFF item 3a identified `SOLAR_ABSORPTANCE_DEFAULT = 0.60` inherited from OCHRE, which is below EnergyPlus's default of 0.70. This has been **corrected**.

**Code Location**:
- `crates/hares-envelope/src/longwave_radiation.rs:63`: `pub const SOLAR_ABSORPTANCE_DEFAULT: f64 = 0.70;` with EnergyPlus Material IDD citation (line 61–62)
- `crates/hares-envelope/src/longwave_radiation.rs:72`: `INTERIOR_SOLAR_ABSORPTANCE_DEFAULT: f64 = 0.70;` was also updated

**Root Cause**: N/A (resolved).

**Impact**: None. This item is complete; the 16.7% relative increase in absorbed shortwave was correctly applied. The BESTEST tests (which pin absorptance to 0.60 explicitly) are unaffected.

---

### Finding 3: Air density altitude correction implemented for zone capacitances but constant remains for other paths — PARTIALLY RESOLVED [Severity: medium]

**Description**: The HANDOFF item 3b called for replacing the sea-level constant `AIR_DENSITY_KG_M3 = 1.2041` with a function of `(temperature, pressure, humidity)`. The `derive_zone_capacitances` function now accepts `site_pressure_pa` and computes density from the ideal gas law. This is verified by tests in `bestest_900ff_root_cause.rs:87-125` showing ~17% reduction in Denver capacitance vs sea level.

**HOWEVER**: The `AIR_DENSITY_KG_M3 = 1.2041` constant (`crates/hares-envelope/src/boundary_rc.rs:18`) still exists and is used at multiple call sites (line 280 in the root cause test file uses it, as does `crates/hares-equipment/src/ventilation.rs:457, 804`). The HANDOFF specifically requested replacement with a function call at each timestep for sensible-load calculation — this has not been done.

Additionally, a separate `AIR_DENSITY_KG_M3: f64 = 1.2` exists in `crates/hares-equipment/src/ventilation.rs:123`, which is rounded differently and unconnected to the envelope constant, creating an inconsistency.

**Code Location**:
- Zone capacitance fix: `crates/hares-envelope/src/boundary_rc.rs` (`derive_zone_capacitances` — now altitude-aware)
- Remaining constant: `crates/hares-envelope/src/boundary_rc.rs:18`
- Ventilation constant: `crates/hares-equipment/src/ventilation.rs:123` (`const AIR_DENSITY_KG_M3: f64 = 1.2;`)

**Root Cause**: Partial implementation — the zone capacitance path was updated but other consumers of the constant (infiltration mass flow, sensible load calc) were not.

**Impact**:
- Zone thermal time constants now correct for altitude (~17% improvement at Denver)
- Infiltration mass flow and sensible load still use sea-level density → ~15–17% error at Denver elevation
- Two different AIR_DENSITY_KG_M3 values (1.2041 in envelope, 1.2 in ventilation) introduce a ~0.34% inconsistency

**Mapping to review areas**: `air-03` (dry air density formula), `air-02` (moist air density), `infil-deep-01` (infiltration exponent)

---

### Finding 4: Occupant heat gain constants still at OCHRE values (66/51.2 W), not updated to ASHRAE 75/55 W — NOT RESOLVED [Severity: medium]

**Description**: The HANDOFF item 3c identified that `OCCUPANT_SENSIBLE_GAIN_W = 66.0` and `OCCUPANT_LATENT_GAIN_W = 51.2` are OCHRE legacy values (400 BTU/h total, 0.563 sensible fraction). ASHRAE 62.1-2022 and HoF 2021 Ch.18 Table 1 specify **75 W sensible / 55 W latent = 130 W total** for seated adult office work. These have **not been updated**.

**Code Location**: `crates/hares-physics/src/constants.rs:180-186` — still cites OCHRE as source, values unchanged.

**Root Cause**: Repo rule `feedback_ashrae_not_ochre.md` dictates ASHRAE/EnergyPlus values should be used, but these constants were not included in the physics-constants audit.

**Impact**:
- 14% underreporting of sensible occupant gain (+9 W per person)
- 7% underreporting of latent occupant gain (+3.8 W per person)
- Larger dwellings (e.g., family of 5) underreport total occupant gain by 60 W — a meaningful cooling-energy shift
- Cooling peak underestimated during occupied hours

**Mapping to review areas**: `core-10` (occupancy gains), `wiring-08` (physics constants parity)

---

### Finding 5: Zone mass multiplier wildcard `_ => 7.0` removed — RESOLVED [Severity: low]

**Description**: The HANDOFF item 3d flagged the catch-all `_ => 7.0` branch in `mass_multiplier_for_zone` that would silently assign conditioned-space thermal mass to any future zone type. This has been **fully resolved** per the `feedback_no_silent_defaults.md` rule. The match now enumerates every `ZoneType` variant explicitly:

```rust
match zone_type {
    ZoneType::Conditioned => 7.0,
    ZoneType::Foundation | ZoneType::Attic | ZoneType::Garage
    | ZoneType::Outdoor | ZoneType::Ground | ZoneType::Adjacent
    | ZoneType::Other(_) => 1.0,
}
```

Adding a new `ZoneType` variant will now produce a compile error, forcing a deliberate mass-multiplier choice.

**Code Location**: `crates/hares-core/src/dwelling/conversions.rs:34-44`

**Root Cause**: N/A (resolved).

**Impact**: None. This item is complete.

---

### Finding 6: Garage ACH heuristic appears resolved via AIM-2 coefficients, but HPXML resolve path was not verified — LIKELY RESOLVED [Severity: low]

**Description**: The HANDOFF item 3e flagged a hard-coded ACH50→ACH20 heuristic (dividing by some factor) for garages in the HPXML resolve path. Investigation found:
- `crates/hares-physics/src/infiltration.rs:693`: `garage_ela_coefficients(garage_height_m)` provides proper AIM-2 Walker-Wilson 1998 coefficients with `hor_lk_frac = 0.4` (mixed leakage).
- `crates/hares-physics/tests/physics_validation_tests.rs:751-819`: validates that ACH50→ELA→flow is climate-variable (not a fixed N-factor), addressing the core concern.
- HPXML parsing in `crates/hares-io/src/hpxml/building.rs:882-904`: ACH50 parsing uses proper ASHRAE 119 power-law conversion (`ach_nat_to_ach50`) when HPXML specifies natural-pressure units.

No hardcoded ACH50/ACH20 division for garages was found in the HPXML resolve code. The HANDOFF-suggested grep at `crates/hares-io/src/hpxml/` produced no garage-specific ACH artifacts.

**Code Location**: `crates/hares-physics/src/infiltration.rs:693-701`, `crates/hares-io/src/hpxml/building.rs:882-904`

**Root Cause**: Likely cleaned up during the dispatch-ordering fix commit `ba133ca`. The existence of `garage_ela_coefficients` with explicit `hor_lk_frac = 0.4` citation suggests the old heuristic was replaced.

**Impact**: Likely none. Recommend a confirmatory code search for any remaining garage-specific ACH division in the HPXML path: `rg -n 'ach.*50.*20|ach.*20.*50|\/.*20|divid.*20' crates/hares-io/src/hpxml/`.

**Mapping to review areas**: `infil-deep-01` (infiltration exponent), `infil-deep-03` (AIM-2 coefficients), `hpxml-06` (infiltration unit conversion)

---

### Finding 7: Thermostat deadband asymmetry is OCHRE-heritage `deadband_offset = 0.2` — acknowledged as intentional, not fixed to ASHRAE symmetric [Severity: low]

**Description**: The HANDOFF item 3f asked whether the 0.2 °C asymmetric deadband is intentional or accidental. Investigation confirms the asymmetry is **intentional and inherited from OCHRE**:

- `crates/hares-equipment/src/hvac/thermostat.rs:21-26`: `deadband_offset` defaults to 0.2, making heating turn-on at `setpoint - hysteresis*(1-offset)` = setpoint − 0.8 °C and turn-off at `setpoint + hysteresis*offset` = setpoint + 0.2 °C (with defaults hysteresis=1.0).
- `crates/hares-equipment/src/hvac/hvac_core.rs:2407-2447`: explicit tests validate the asymmetric thresholds and the `deadband_offset = 0.0` fallback to symmetric behavior.
- `crates/hares-equipment/src/hvac/heat_pump/constants.rs:76`: `DEFAULT_ER_SETPOINT_DEADBAND_OFFSET = 0.2`, cited from "OCHRE / Winkler."
- `crates/hares-equipment/tests/hvac_parity.rs:1600`: documents thresholds as "OCHRE-style thermostat thresholds."

The OCHRE source citation (`HVAC.py line 222`) is present in the thermostat struct doc comment. However, no ASHRAE citation justifies the 0.2 offset over a symmetric ±0.5–1.0 °C deadband (ANSI/ASHRAE 55-2020). The OCHRE-derived top-loaded deadband has physics rationale (offset moves the setpoint ratio to prevent short-cycling) but this has not been reviewed against ASHRAE standards.

**Code Location**: `crates/hares-equipment/src/hvac/thermostat.rs:21-30`, `crates/hares-equipment/src/hvac/hvac_core.rs:639-646`

**Root Cause**: The asymmetry is by design, matching OCHRE. The HANDOFF question was about documentation, not correctness. The comment at `thermostat.rs:21` cites OCHRE HVAC.py line 222 — this is a documented, intentional choice, not an oversight.

**Impact**: Minor. The 0.2 °C offset is within typical thermostat manufacturing tolerances. However, HARES's repo-wide rule `feedback_ashrae_not_ochre.md` means a documented justification for following OCHRE over ASHRAE in this specific case should be present.

**Mapping to review areas**: `control-deep-02` (control signal variant coverage), `core-03` (solver feedback loop)

---

### Finding 8: Stale `#[ignore]` documentation in hpxml_parity.rs — RESOLVED [Severity: low]

**Description**: The HANDOFF item 4 identified stale comments above two test functions (`ac_has_startup_capacity_degradation_default` at line 702, `ashp_backup_lockout_temperature_extracted` at line 742) that said `// Marked #[ignore] because ...` but neither test had an `#[ignore]` attribute. These stale comments have been **removed** and replaced with proper documentation blocks describing the physics being tested.

**Code Location**:
- `crates/hares-io/tests/hpxml_parity.rs:694-731` (cleaned-up AC startup capacity degradation test)
- `crates/hares-io/tests/hpxml_parity.rs:733-804` (cleaned-up ASHP backup lockout test)

**Root Cause**: N/A (resolved).

**Impact**: None. This item is complete.

---

### Finding 9: Review pass over 228-file commit — partially addressed through review manifest system [Severity: medium]

**Description**: The HANDOFF item 5 calls for a structured review pass over the fix commit `ba133ca` (228 files, +3766/−1696). The existence of the review manifest system in `docs/reviews/` with 100+ review documents covering all major crate areas suggests this is being addressed incrementally. However, no single end-to-end review of the dispatch-ordering changes exists.

Key areas flagged for focused review:
- `crates/hares-core/src/dwelling/mod.rs` — dispatch ordering semantics — NO dedicated review found
- `crates/hares-io/src/hpxml/*.rs` — silent unwrap replacement → typed errors — partially covered by `hpxml-01` through `hpxml-12`
- `tests/envelope_oracle.rs`, `tests/freefloat_oracle.rs` — tolerance band changes — partially covered by `envelope-12`

**Code Location**: N/A (process item)

**Root Cause**: Process gap — the review manifest system is comprehensive but lacks explicit coverage of the dispatch-ordering regressions in `crates/hares-core/tests/dispatch_ordering_regressions.rs`, and the tolerance-band audit in the oracle tests.

**Impact**: The dispatch-ordering change (from unordered to stage-prioritized) is a semantic change to the core simulation loop. A review omission here could mask subtle ordering bugs in HVAC/WH/load interaction.

**Mapping to review areas**: `core-03` (solver feedback loop ordering), `core-13` (engine main loop), all HPXML reviews (`hpxml-01` through `hpxml-12`)

---

### Finding 10: Python test suite NOT integrated into CI [Severity: high]

**Description**: The HANDOFF item 6 states Python tests were not run at handoff. Current investigation finds:
1. `.github/workflows/ci.yml` is only 35 lines, containing only `cargo fmt --check` and `cargo clippy` jobs. No Python test job, no `cargo test` job, no build job.
2. `pyproject.toml:73-78` shows pytest is properly configured with `testpaths = ["tests/python"]` and `addopts = "-n auto -m 'not slow'"` — the test infrastructure exists.
3. 45+ Python test files in `tests/python/` including the four flagged in the HANDOFF:
   - `test_py_dwelling_integration.py`
   - `test_py_fleet.py`
   - `test_ochre_parity.py`
   - `test_thermal_trace.py`
4. The CI file is also missing a `cargo test` job — meaning the 3013 lib tests and integration suites mentioned in the HANDOFF are not CI-verified either.

**Code Location**:
- CI file: `.github/workflows/ci.yml` (entire file, 35 lines)
- Pytest config: `pyproject.toml:73-78`
- Python test directory: `tests/python/` (45 files)
- Python project: `pyproject.toml` (maturin-based PyO3 build)

**Root Cause**: The CI pipeline is a minimal skeleton that was never completed. The Rust toolchain is the only thing configured. Python tests require a `maturin develop` build step preceding `pytest`, which has no CI job definition.

**Impact**:
- PyO3 bindings are completely untested in CI — regressions from Rust-side changes will go undetected
- The four flagged tests (dwelling integration, fleet, OCHRE parity, thermal trace) exercise code paths the Rust tests don't cover
- No CI run ensures `maturin develop` succeeds (build-time binding errors)
- The HANDOFF's concern about "Fixture files that need Site/Latitude added" and "Telemetry shape changes from dispatch-ordering fix" remains unverified

**Mapping to review areas**: `infra-04` (CI completeness), `py-companion-02` (Python companion tests), `fleet-python-01` through `fleet-python-08`

---

### Finding 11: Two xfails — one removed, one remains; all BESTEST tests now ignored instead [Severity: medium]

**Description**: The HANDOFF item 7 describes two `#[should_panic]` xfails (900 and 900FF) that should become "true passes" before release. Current state:
1. Case 900 `#[should_panic]` has been **removed** — but the test is now `#[ignore]` with the message "pending remaining physics fixes" (line 98–103).
2. Case 900FF still carries **both** `#[should_panic]` and `#[ignore]` (lines 142–144).
3. Three additional tests (600, 600FF, 640) that previously PASSED now also carry `#[ignore]` (lines 77, 120, 167).

This is worse than the HANDOFF state: the two xfails were acknowledged technical debt, but the three previously-passing tests being ignored represents a regression in test coverage. The HANDOFF commit message at `ba133ca` mentions "test-integrity blockers" — these ignore flags may be a discovery from the `consolidated.md` findings (B1, S4/S5) that invalidated previously-correct results.

**Code Location**: `tests/bestest/mod.rs:77, 98, 120, 142, 167`

**Root Cause**: Physics regression from `consolidated.md` findings (internal gain radiant fraction, warmup initialization) affected all BESTEST cases, not just the heavyweight ones. The blanket `#[ignore]` is a temporary gate to prevent test noise while physics fixes are being developed.

**Impact**:
- Zero ASHRAE 140 validation runs in CI (all five blocked by `#[ignore]`)
- The `#[should_panic]` soundness hazard (documented in `infra-02`) persists on Case 900FF
- If `#[ignore]` is removed before the physics fixes land, 600/600FF/640 will regress from known-good to failing
- The HANDOFF expectation that "fixing 900FF closes both 900 and 900FF" is still valid — the root cause analysis confirms this — but the broader regression in 600/600FF/640 suggests additional defects (B1 radiant fraction) must be addressed first

**Mapping to review areas**: `infra-02` (xfail soundness), `test-01` (BESTEST validation), `core-10` (occupancy gains radiant fraction), `envelope-10` (warmup initialization)

---

## Summary
- Total findings: 11
- Critical: 1 — BESTEST 900FF root cause diagnosed but fix unapplied; all BESTEST tests ignored (regression from HANDOFF state)
- High: 1 — Python test suite not integrated into CI
- Medium: 4 — Air density partially resolved; occupant gains unchanged; review pass incomplete; xfail→ignore regression on BESTEST tests
- Low: 5 — Three items resolved (solar absorptance, mass multiplier, stale comments); two items likely resolved or intentional (garage ACH, deadband asymmetry)

### Handoff Item Status Summary

| Item | Description | Status |
|------|-------------|--------|
| 1 | BESTEST 900FF root cause | Diagnosed, NOT fixed |
| 2 | BESTEST 900 margin | NOT fixed (blocked on 900FF) |
| 3a | Solar absorptance 0.60→0.70 | **RESOLVED** |
| 3b | Air density altitude correction | Partially resolved |
| 3c | Occupant gain 66/51.2→75/55 W | NOT resolved |
| 3d | Mass multiplier wildcard | **RESOLVED** |
| 3e | Garage ACH heuristic | Likely resolved (needs confirmation) |
| 3f | Thermostat deadband asymmetry | Intentional (needs ASHRAE justification) |
| 4 | Stale `#[ignore]` comments | **RESOLVED** |
| 5 | Review pass over 228-file commit | Partially addressed via review manifest |
| 6 | Python test suite run | NOT resolved (not in CI) |
| 7 | Two xfails → true passes | Regressed (now 5 ignored tests) |

---

## Recommendations

1. **Apply the 900FF fix immediately** (critical, blocks Items 1, 2, 7):
   - Modify `determine_initial_indoor_temp_c` in `crates/hares-core/src/environment.rs:967` to use outdoor temperature for free-float zones (no setpoints → outdoor-ambient initialization, not 21 °C fallback)
   - Apply 30% radiant fraction to internal gains for BESTEST cases (EnergyPlus BESTEST IDF specifies Fraction Radiant = 0.3)
   - Re-run all BESTEST cases and remove `#[ignore]` from previously-passing 600, 600FF, 640
   - Remove `#[should_panic]` from 900FF and verify all five pass as plain `#[test]`

2. **Set up CI jobs** (high priority, Item 6):
   - Add `cargo test --workspace` job to `.github/workflows/ci.yml`
   - Add Python test job: `maturin develop && pytest` (using the existing `pyproject.toml` config)
   - Verify all 45+ Python tests pass; triage any failures per HANDOFF guidance (fixture Site/Latitude, telemetry shape)

3. **Update occupant gain constants** (medium priority, Item 3c):
   - Change `OCCUPANT_SENSIBLE_GAIN_W` from 66.0 to 75.0 and `OCCUPANT_LATENT_GAIN_W` from 51.2 to 55.0
    - Update source comment from OCHRE to ASHRAE HoF 2021 Ch.18 Table 1
   - Re-run BESTEST 600/640 (conditioned cases) to verify heating-to-cooling balance shift

4. **Complete air density altitude correction** (medium priority, Item 3b):
   - Replace remaining `AIR_DENSITY_KG_M3` uses in infiltration mass flow with `dry_air_density_kg_m3()` call
   - Unify the two AIR_DENSITY_KG_M3 constants (1.2041 in envelope, 1.2 in ventilation) into a single source-of-truth

5. **Document deadband asymmetry justification** (low priority, Item 3f):
   - Add a comment at `thermostat.rs:26-30` explaining why OCHRE's `deadband_offset = 0.2` is retained vs the ASHRAE symmetric standard, citing the physics benefit (top-loading prevents short-cycling in heating mode)

6. **Confirm garage ACH resolution** (low priority, Item 3e):
   - Run `grep -rn 'ach.*20' crates/hares-io/src/hpxml/` to verify no remaining hardcoded ACH50→ACH20 division
   - If clean, close this item

7. **Promote unresolved items to GitHub issues**:
   - Issue "BESTEST 900FF: apply initialization fix and radiant-fraction correction" — label `critical`, `physics`, `bestest`
   - Issue "CI: add cargo test and Python test jobs" — label `high`, `infrastructure`, `ci`
   - Issue "Physics constants audit: occupant gains 66→75 W, air density unification" — label `medium`, `physics`, `ashrae-parity`
   - Issue "BESTEST: remove #[ignore] and #[should_panic] from all cases" — label `medium`, `testing`, `bestest`

---

## References / Citations
- HANDOFF document: `docs/HANDOFF.md` (commit `ba133ca`)
- 900FF root cause tests: `crates/hares-envelope/tests/bestest_900ff_root_cause.rs`
- Repo rules: `feedback_ashrae_not_ochre.md`, `feedback_no_silent_defaults.md`, `feedback_best_physics.md`
- ASHRAE 140-2017 BESTEST bands: `tests/bestest/reference_bands.rs`
- Related reviews: `infra-02` (xfail soundness), `infra-01` (dead module check), `infra-03` (empty fixture directories), `core-10` (occupancy gains), `core-03` (solver feedback), `air-03` (dry air density)
- ASHRAE HoF 2021 Ch.18 Table 1 — seated adult heat gain: 75 W sensible / 55 W latent
- ASHRAE HoF 2021 Ch.1 Eq.28 — air density from T, P, humidity (ISA 1976 / ICAO Doc 7488)
- EnergyPlus Engineering Reference §3.2.4 — exterior convection and absorption defaults
- Walker & Wilson (1998) AIM-2 model — infiltration coefficients
- ANSI/ASHRAE 55-2020 — symmetric thermostat deadband specification
