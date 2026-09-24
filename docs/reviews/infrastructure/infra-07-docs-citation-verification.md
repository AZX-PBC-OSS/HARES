# Documentation citation verification: physics references, model sources, validation data
**Review ID**: infra-07
**Category**: infrastructure
**Date**: 2026-05-26

## Files Reviewed
docs/ docs/findings/

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/` — EnergyPlus source tree (reference for section numbers, formula fidelity)
- `vendors/OCHRE/` — OCHRE simulator (reference parity oracle)
- `crates/hares-physics/src/` — inline doc comments with equation/section references
- `crates/hares-envelope/src/` — inline doc comments, solver architecture
- `docs/eplus/26-1_*.html.md` — vendor EnergyPlus v26.1 documentation extracts
- `docs/tickets/` — ticket citation audit trails for 131 individual tickets

## Findings

### Finding 1: EnergyPlus version reference is inconsistent across documentation and code [Severity: high]
**Description**: EnergyPlus citations in the codebase reference at least six different versions. `docs/development.md:392-393` cites EnergyPlus v25.2 as the authoritative version, yet inline code comments in `crates/hares-physics/src/` reference E+ versions 9.3 through 24.1. `docs/eplus/` contains v26.1 documentation extracts, implying v26.1 is the reference version, but no other doc mentions v26.1. The `vendors/EnergyPlus/` tree is version 24.1. HARES effectively has five parallel EnergyPlus version anchors (v9.x, v22.1, v23.1, v24.1, v25.2, v26.1) with no statement of which version's algorithms are the target for parity.

**Code Location**:
- `docs/development.md:392-393` — official citation is E+ v25.2
- `docs/eplus/26-1.md:2` — docs extracted from v26.1
- `crates/hares-physics/src/infiltration.rs:28` — "EnergyPlus Engineering Reference §15.4"
- `crates/hares-physics/src/film_coefficients.rs:11` — "EnergyPlus Engineering Reference §9.4"
- `crates/hares-physics/src/ground.rs:4` — "EnergyPlus Engineering Reference Ch. 3.17"
- `docs/thermal-envelope-solver.md:328` — "EnergyPlus Engineering Reference §13.3"
- `docs/tickets/129-solve-ideal-capacity-failure-warn-not-debug.md:52,88,115` — references E+ v24.1
- `vendors/EnergyPlus/` — contains source for v24.1 (not v25.2 or v26.1)

**Root Cause**: No project-wide policy for EnergyPlus version pinning. Each author independently cited whatever version of the EnergyPlus Engineering Reference they were consulting at the time. Big Ladder's HTML-hosted E+ docs use no numeric section markers (only prose headings), making section-number citations like "§15.4" or "§13.3" intrinsically unverifiable against any version.

**Impact**: (1) A developer using the v25.2 or v26.1 Engineering Reference cannot find content at the cited section numbers because the web-hosted versions use heading anchors, not numeric sections. (2) Algorithm changes between E+ versions (e.g., AIM-2 coefficient tables updated between v9.3 and v23.1) could invalidate parity claims without detection. (3) The `docs/eplus/26-1/` extracts are ~538 files that cannot be cited because no other doc references v26.1.

### Finding 2: ASHRAE Handbook of Fundamentals edition drift across modules [Severity: high]
**Description**: Physically adjacent modules in the same crate cite different ASHRAE HOF editions for the same physical constant. `hares-physics/src/constants.rs` cites "ASHRAE 2017 Handbook" for molecular weights (R_da, M_da, M_w), while `hares-physics/src/psychrometrics.rs` cites "ASHRAE HOF 2021 Ch.1 Eq. 35/37" for wet-bulb psychrometer constants. The 2017 and 2021 editions have slightly different psychrometric constants (e.g., saturation pressure polynomial coefficients differ between editions). The `validation.md:252` table claims all psychrometric validation is against "ASHRAE HOF 2021 Ch. 1" yet `validation.md:139` references "ASHRAE HOF Table 2" without specifying an edition.

**Code Location**:
- `crates/hares-physics/src/constants.rs:16` — "ASHRAE 2017 Handbook of Fundamentals, Ch. 1"
- `crates/hares-physics/src/constants.rs:33` — "ASHRAE 2017 HOF Ch. 1, Eq. 20"
- `crates/hares-physics/src/psychrometrics.rs:33` — "ASHRAE HOF 2021 Ch.1 Eq. 35"
- `crates/hares-physics/src/psychrometrics.rs:340` — same 2021 reference
- `crates/hares-physics/src/air_properties.rs:18` — "ASHRAE HOF 2021 Ch.1 Eq.28"
- `crates/hares-envelope/src/infiltration.rs:91` — "ASHRAE HOF 2021 Ch.1 Eq.28"
- `docs/validation.md:138` — "ASHRAE HOF Table 2" (no edition)
- `docs/validation.md:252` — "ASHRAE HOF 2021 Ch. 1"
- `docs/validation.md:253` — "ASHRAE HOF 2021 Ch. 16"
- `docs/validation.md:254` — "ASHRAE HOF 2021 Ch. 25"

**Root Cause**: The codebase was built incrementally with contributors citing whatever edition was on their desk. Edition-sensitive quantities (molecular weight of dry air: 28.9645 g/mol in 2017 vs 28.96546 g/mol in 2021; gas constant R_da: 287.058 J/(kg·K) in 2017 vs 287.042 J/(kg·K) in psychrolib) were not consolidated around a single authoritative edition.

**Impact**: The `air-02` and `air-03` review documents already identified that the molecular weight ratio `1/ε = 1.6077687` (HARES) differs from `1.607858` (ASHRAE HOF 2021 Eq.28), `1.6077687` (EnergyPlus), and `1.607861` (psychrolib/OCHRE). None of these values is wrong, but the code cannot simultaneously claim fidelity to ASHRAE HOF 2017 and 2021 — and it cites both. Cross-validation against a single edition's table values would produce a false positive or negative depending on which edition is consulted.

### Finding 3: EnergyPlus section numbers consistently unverifiable [Severity: high]
**Description**: At least 15 distinct EnergyPlus Engineering Reference "§X.Y" section numbers appear across code comments, tickets, and review documents. None of these section numbers can be verified against the web-hosted EnergyPlus Engineering Reference (any version v8.0–v26.1), which uses prose headings ("Sky Radiation Modeling", "Inside Heat Balance") without numeric section designators. The Big Ladder HTML rendering occasionally uses anchor IDs like `#inside-heat-balance` but never publishes a `§3.5`, `§13.3`, or `§15.4` numbering scheme.

This is not a single-file concern — it is a systematic mislabeling across the entire codebase. The ticket audit trails (`docs/tickets/128-interior-lwr-method-explicit-starmesh-callsites.md:163-172`, `docs/tickets/125-berdahl-martin-coefficient-citation.md:112-117`, `docs/tickets/consolidated/054-beam-floor-fraction-minimum-clamp.md:104-105`, `docs/tickets/consolidated/042-ground-temp-shallow-depth-selection.md:72`, etc.) independently confirm this for multiple section numbers.

**Code Location** (non-exhaustive):
| Claimed § Ref | Doc/Code Location | Actual E+ Content | Verifiable? |
|---|---|---|---|
| §9.4 (ASHRAE Simple / TARP) | `film_coefficients.rs:11` | Heading exists ("Interior Convection") | No § number |
| §9.5 (DOE-2 exterior) | `film_coefficients.rs:13` | Heading exists ("Exterior Convection") | No § number |
| §13.3 (semi-implicit infiltration) | `thermal-envelope-solver.md:328` | Heading-based, no §13.3 | No § number |
| §15.4 (AIM-2 / Sherman-Grimsrud) | `infiltration.rs:28` | Heading exists | No § number |
| §3.17 (Kusuda-Achenbach) | `ground.rs:4,193` | Heading exists | No § number |
| §3.5.6 (sky emissivity) | tickets 125, 126 | "Sky Radiation Modeling" | No §3.5.6 |
| §14.5 (interior solar dist.) | `consolidated/054-beam-floor-fraction-minimum-clamp.md:82,102` | Shading Module chapter | No §14.5 |
| §14.8 (stratified tank) | `equip-wh-05-wh-skin-loss-end-caps.md:111` | Water Heater chapter | No §14.8 |

**Root Cause**: The EnergyPlus Engineering Reference was historically published as a numbered-section PDF, but the canonical online reference (bigladdersoftware.com/epx/docs/) uses a heading-anchored HTML format. Contributors cite the section numbers from either obsolete PDF editions or from memory of the PDF layout. The section numbers have never existed in the online HTML versions.

**Impact**: Any reader attempting to verify a physics claim against the EnergyPlus Engineering Reference will fail to locate the cited section. This undermines the auditability of the entire codebase. Affects approximately 400+ individual citation sites. A developer implementing a new model based on "E+ §15.4" will read the wrong section if they consult a different E+ edition.

### Finding 4: OCHRE publication provenance is absent [Severity: high]
**Description**: OCHRE is the primary reference oracle for HARES parity testing (documented at `docs/validation.md:30-32`). Yet no OCHRE publication — conference paper, journal article, NREL technical report — is cited anywhere in the HARES documentation. The `docs/validation.md` standards table lists 12 references (ASHRAE standards, Walker & Wilson, Burch & Christensen, Henderson & Rengarajan, AHRI 210/240) but does not include the OCHRE copyright notice, repository URL, or publication provenance. The `docs/findings/hvac/ochre_comparison.md:77` refers to "the OCHRE HVAC.py file" as the source, not a published paper.

**Code Location**:
- `docs/validation.md:250-262` — Standards and References table (OCHRE absent)
- `docs/validation.md:16-24` — Validation layers (OCHRE "simulator" listed without citation)
- `docs/development.md:385-401` — Key Reference Documentation (OCHRE absent)
- `docs/findings/hvac/ochre_comparison.md` — Entire document describes OCHRE architecture without a publication citation

**Root Cause**: OCHRE was developed at NREL but the publication status is unclear — it may exist only as a GitHub repository (github.com/NREL/OCHRE) without a formal paper. HARES vendors OCHRE as a submodule (`vendors/OCHRE/`) and uses it for parity testing, but treating a source-code-only reference as an oracle without documenting its provenance violates research reproducibility standards.

**Impact**: A reviewer or downstream user cannot verify what version of OCHRE HARES was compared against. If OCHRE's algorithms change in a future commit, HARES parity tests may begin failing without an auditable trail of the original reference version. The `vendors/OCHRE/` submodule pins a specific commit, but this is not documented as the parity-test reference commit hash.

### Finding 5: Cutler et al. (2013) citation contains three independent errors [Severity: medium]
**Description**: Ticket `docs/tickets/010-default-biquadratic-performance-curves.md:150,218-222` cites the foundational biquadratic HVAC performance curve reference as "Cutler, B., et al. (2013). *Improved Control Strategies for Residential Heat Pump Systems*. NREL/TP-5500-57501." All three bibliographic fields — first author initial, title, and report number — are wrong. The correct reference is: Cutler, D., Winkler, J., Kruis, N., Christensen, C., Brandemuehl, M. (2013). *Improved Modeling of Residential Air Conditioners and Heat Pumps for Energy Calculations*. NREL/TP-5500-56354. This citation is also referenced in `docs/findings/hvac/ochre_comparison.md:77` as "Cutler 2013" without any bibliographic details, making the error invisible to casual readers.

**Code Location**:
- `docs/tickets/010-default-biquadratic-performance-curves.md:150,218-222` — ticket audit confirms all three fields wrong
- `docs/findings/hvac/ochre_comparison.md:77,87` — "Cutler 2013" (no report number, no title)
- `docs/reviews/equipment-hvac/equip-hvac-11-speed-control-modes-completeness.md:117` — "Cutler et al. (2013)" (no report number)

**Root Cause**: The original ticket author misremembered or mis-transcribed the reference. The ticket was audited (ticket 010 audit) and flagged as "Incorrect on multiple points," but the erroneous citation persists in `ochre_comparison.md` and `equip-hvac-11`.

**Impact**: A reader attempting to look up NREL/TP-5500-57501 will find no document. A search for "Cutler, B. + heat pumps" will miss the authoritative reference. The biquadratic model is the core HVAC performance model — its foundational paper should be correctly cited.

### Finding 6: Winkler thesis year is misattributed in multiple documents [Severity: medium]
**Description**: Multiple HARES documents refer to the "Winkler (2011)" or "Winkler 2011" startup capacity degradation model, but the Winkler doctoral dissertation is from 2009, not 2011. The OCHRE source code does not state a year; the DRUM repository at `https://drum.lib.umd.edu/handle/1903/9493` confirms the 2009 date. One prior HARES audit incorrectly stated "2013."

**Code Location**:
- `docs/findings/hvac/hp_review.md:13,132` — "Winkler (2011)"
- `docs/findings/hvac/core_review.md:10` — "Winkler 2011 startup ramp"
- `docs/equipment/hvac.md:39` — "Winkler 2011"
- `docs/reviews/equipment-hvac/equip-hvac-14-startup-capacity-degradation-ramp.md:1,126` — "Winkler 2011"
- `docs/tickets/019-speed-startup-telemetry-gaps.md:184,226` — prior audit corrected to "Winkler (2009)" with verified DRUM link

**Root Cause**: Copy-paste propagation of the erroneous year from early documentation into later review and finding documents.

**Impact**: A researcher attempting to cite the correct reference will search for Winkler 2011 and find nothing. The correct thesis is Winkler, J.M. (2009). "Development of a Component Based Simulation Tool for the Steady State and Transient Analysis of Vapor Compression Systems." University of Maryland.

### Finding 7: Berdahl-Martin sky emissivity citation still incomplete [Severity: medium]
**Description**: Ticket 125 (`docs/tickets/125-berdahl-martin-coefficient-citation.md`) identified that the Berdahl-Martin sky emissivity coefficients `0.758/0.521/0.625` at `crates/hares-io/src/epw.rs:523-533` were cited to the wrong Martin & Berdahl 1984 paper (Solar Energy 33 instead of Solar Energy 32). The ticket was partially addressed — the code now distinguishes the two 1984 papers — but the citation still omits Li, Jiang & Coimbra (2017), which EnergyPlus exclusively credits as the source of the recalibrated coefficients. The same gap persists at `crates/hares-core/src/dwelling/synthetic.rs:753-754`. The ticket audit (2026-05-21) confirmed the ticket is "Partially Legitimate" but the fix is incomplete.

**Code Location**:
- `crates/hares-io/src/epw.rs:523-533` — partially fixed, still missing Li et al. 2017
- `crates/hares-core/src/dwelling/synthetic.rs:753-754` — identical gap
- `docs/tickets/125-berdahl-martin-coefficient-citation.md:1-140` — full audit trail

**Root Cause**: The ticket resolution partially updated the citation but omitted the primary provenance (Li et al. 2017) that EnergyPlus itself cites.

**Impact**: Low for code correctness (coefficients are numerically correct). Medium for provenance — a future developer who consults the original Berdahl & Martin 1984 paper expecting to verify `0.758/0.521/0.625` will find `0.711/0.56/0.73` instead and may incorrectly "correct" the code.

### Finding 8: ASHRAE chapter numbers vary by edition and are inconsistently cited [Severity: medium]
**Description**: ASHRAE HOF reorganizes chapter numbering across editions. Several HARES documents cite chapter numbers that are correct for one edition but wrong for the claimed edition. Examples:

- `docs/validation.md:254` cites "ASHRAE HOF 2021 Ch. 25" for natural convection — Chapter 25 in the 2021 edition is "Heat, Air, and Moisture Control in Building Assemblies." The natural convection TARP correlation is in Chapter 4 ("Heat Transfer"), not Chapter 25.
- `docs/tickets/consolidated/042-ground-temp-shallow-depth-selection.md:53` cites "ASHRAE Handbook of Fundamentals 2021 Ch. 18.31 (Below-Grade Heat Transfer)" — Chapter 18 in the 2021 edition is "Nonresidential Cooling and Heating Load Calculations." Below-grade residential heat transfer is Chapter 17 ("Residential Cooling and Heating Load Calculations").
- `docs/tickets/consolidated/027-doe2-ground-diffusivity-units.md:62` cites the same wrong chapter (Ch. 18.31 vs Ch. 17).
- `docs/reviews/equipment-hvac/equip-hvac-04-ac-shr-latent-degradation.md:87` cites "ASHRAE 2017 Handbook of Fundamentals, Ch.18 Eq.63" for the coil bypass factor — Chapter 18 in the 2017 edition is "Nonresidential Cooling and Heating Load Calculations," which contains the bypass factor method, but it is not clear which equation is referred to as "Eq.63" since the standard psychrometric bypass factor is `BF = exp(-NTU)`.

**Code Location**:
- `crates/hares-envelope/src/longwave_radiation.rs:38` — "ASHRAE HOF 2021, Ch. 25" (correct for 2021)
- `crates/hares-physics/src/film_coefficients.rs:17` — "ASHRAE HoF 2021 Ch. 15, Table 1" (fenestration U-factors — correct for 2021)
- `crates/hares-physics/src/infiltration.rs:477` — "ASHRAE HOF 2021 Ch. 16" (ventilation — correct for 2021)
- `docs/validation.md:254` — "ASHRAE HOF 2021 Ch. 25" for natural convection (incorrect — should be Ch. 4)
- `docs/tickets/consolidated/042-ground-temp-shallow-depth-selection.md:53` — Ch. 18.31 (incorrect — should be Ch. 17)
- `docs/tickets/consolidated/027-doe2-ground-diffusivity-units.md:62` — Ch. 18.31 (incorrect — same error propagated)

**Root Cause**: ASHRAE HOF reorganized its chapter numbering significantly between the 2009, 2013, 2017, and 2021 editions. Contributors citing chapter numbers without checking the specific edition they name produces false references. The 2017 and 2021 editions have the most similar chapter numbering, but the 2009 edition is substantially different.

**Impact**: A reader verifying claims against the claimed edition of ASHRAE HOF will land in the wrong chapter. Since some chapters have shifted topics across editions (e.g., Ch. 25 was "Mechanical Insulation" in 2017 but "Heat, Air, and Moisture Control" in 2021), the reader may find entirely unrelated content.

### Finding 9: BESTEST/ASHRAE 140-2017 reference bands cited without version verification [Severity: medium]
**Description**: `docs/validation.md:87-116` documents HARES's BESTEST implementation as conforming to "ASHRAE 140-2017." Reference bands are cited from "Table B8-2 (loads) and Table B8-3a (temperatures)" and example values are given for Case 600. However:

1. No access instructions are provided for ASHRAE 140-2017 — the standard is behind an ASHRAE paywall and cannot be independently verified by open-source contributors.
2. The reference band values listed (Case 600 heating: 4,296–5,709 kWh/year; cooling: 6,137–7,964 kWh/year; 600FF peak: 64.9–69.5 °C) match the values in the reference source file `tests/bestest/reference_bands.rs`, but whether these are from ASHRAE 140-2017 or an earlier version (e.g., 140-2014 or 140-2001) is not documented.
3. The NREL BESTEST-GSR repository (github.com/NREL/BESTEST-GSR) — which is cited in `docs/findings/summary.md:161` — tracks the ASHRAE 140-2020 standard test cases, which may have slightly different reference bands than 140-2017.
4. `docs/findings/summary.md:159` cites "ASHRAE 140-2017 §5.2.4.3" for the 60/40 radiant/convective internal gain split — this section exists in 140-2017 but the exact section number should be verified for the specific edition.

**Code Location**:
- `docs/validation.md:88-116` — BESTEST section
- `docs/findings/summary.md:159` — "ASHRAE 140-2017 §5.2.4.3"
- `tests/bestest/reference_bands.rs` — reference values source (not independently verifiable without ASHRAE 140 purchase)
- `docs/findings/summary.md:161` — NREL BESTEST-GSR reference

**Root Cause**: ASHRAE 140 is a continuously evolving standard (2001, 2004, 2007, 2011, 2014, 2017, 2020). The reference band values change across editions as new simulation engines are added to the comparative analysis. HARES has not version-pinned its reference bands.

**Impact**: If the reference bands were extracted from BESTEST-GSR (which tracks 140-2020) but labeled as 140-2017, a validator comparing against 140-2017 directly would see different numbers and could wrongly conclude HARES fails validation or that it fails a different set of bounds.

### Finding 10: NREL ResStock dataset version and access instructions are absent [Severity: medium]
**Description**: HARES uses NREL ResStock for fleet-scale simulation validation. The `docs/validation.md` validation table does not mention ResStock at all. The only ResStock methodology citation is in `docs/reviews/core/core-17-synthetic-dwelling-generation.md:95`, which cites Wilson et al. (2021) "End-Use Load Profiles for the U.S. Building Stock" (NREL/TP-5500-79367). This citation does not include:
1. The ResStock dataset release version (e.g., "ResStock 2024.2" or "EULP 2024 Release 1").
2. Access instructions or URLs for the OEDI S3 bucket where weather files and building metadata are hosted.
3. A statement of which BUILDING characteristics were sampled from which ResStock release.
4. The weather year vintage (AMY 2018, TMY3, etc.).

**Code Location**:
- `docs/reviews/core/core-17-synthetic-dwelling-generation.md:95-96` — sole ResStock methodology citation
- `docs/reviews/config-io/config-io-05-resstock-dataset.md` — notes weather path resolution fails for ResStock fleet mode but does not cite dataset version
- `python/ochre_next/data/resstock.py` — implements download from OEDI S3 with hardcoded bucket paths, no version documentation

**Root Cause**: The ResStock pipeline was developed as the statistical bridge from NREL's CPT-based sampling to HARES fleet simulation, but the dataset provenance was never formalized in main documentation.

**Impact**: ResStock dataset releases change building characteristics, weather year, and sample weights across versions. Without version documentation, results are not reproducible. A future ResStock release that changes the bucket layout or metadata schema would break the HARES download pipeline with no warning.

### Finding 11: Documentation claims Crank-Nicolson discretization but code uses ZOH [Severity: medium]
**Description**: `docs/thermal-envelope-solver.md:4-5` and the State-Space Model section describe the solver as implementing a Crank-Nicolson (trapezoidal) implicit scheme with pre-computed matrices `M = I - dt/2·A_c`, `N = I + dt/2·A_c`. However, `crates/hares-envelope/src/state_space.rs:101-106` documents that the solver uses ZOH discretization with optional implicit couplings, not Crank-Nicolson. The stepping code at `crates/hares-envelope/src/thermal_solver/stepping.rs:3` explicitly says "per-timestep ZOH step." The architecture.md does not prescribe a specific discretization method but references the "thermal-envelope-solver.md" for details.

This is confirmed by `state_space.rs:272-276`:
```
/// Constructs from continuous A_c, B_c using ZOH (matrix exponential) discretization.
/// ZOH is unconditionally stable and matches OCHRE's approach.
```

**Code Location**:
- `docs/thermal-envelope-solver.md:94-119` — describes CN matrices M = I − dt/2·A_c, N = I + dt/2·A_c
- `crates/hares-envelope/src/state_space.rs:272-276` — ZOH discretization, not CN
- `crates/hares-envelope/src/thermal_solver/stepping.rs:3` — "ZOH step"
- `docs/architecture.md:488-489` — "Primary (ZOH): A_d = exp(A_c·dt)" — correct for architecture.md
- `docs/reviews/envelope/envelope-02-frozen-interior-film-coefficients.md:80` — correctly states ZOH

**Root Cause**: The CN documentation was written during planning or early implementation when Crank-Nicolson was the intended discretization method. The implementation was later switched to ZOH (which is numerically simpler and matches OCHRE), but the planning document was not updated to match.

**Impact**: A contributor attempting to implement a new feature based on the CN matrices described in `thermal-envelope-solver.md` would produce code that is incompatible with the actual ZOH solver. The described `M = I - dt/2·A_c` matrix and the associated LU preconditioning do not exist in the current codebase. The pre-factorized A_c inversion approach (Van Loan fallback) described is fundamentally different from the ZOH exponential.

### Finding 12: Burch-Christensen model applicability range is undocumented [Severity: low]
**Description**: The Burch-Christensen (2007) mains water temperature model documented at `docs/weather-pipeline.md:302-319` is described as "calibrated for continental US" with no explicit applicability range. Neither the documentation nor the code states the model's valid temperature range, altitude range, or domain of calibration. `docs/reviews/core/core-18-environment-state-management.md:19` notes that EnergyPlus adds a hard floor at 0 °C (32 °F) because the model was not calibrated for freezing conditions. HARES does not apply this floor, potentially producing negative mains water temperatures in cold climates.

**Code Location**:
- `docs/weather-pipeline.md:302-308` — model description
- `crates/hares-physics/src/water_mains.rs` — implementation
- `docs/reviews/core/core-18-environment-state-management.md:19-81` — EnergyPlus 0 °C floor

**Root Cause**: The model was adopted from EnergyPlus/OCHRE without copying the defensive bounds.

**Impact**: Mains water temperatures below 0 °C are non-physical (water freezes). The model may produce values as low as −5 °C for far-northern US climates. The impact on domestic hot water energy is bounded (~2–5% error at extremes), but the model should state its valid range.

### Finding 13: Infiltration model documentation conflates AIM-2 and Sherman-Grimsrud [Severity: low]
**Description**: `docs/validation.md:164` states that the "ASHRAE AIM-2 wind-stack model" is validated against "Walker & Wilson 1998, ASHRAE HOF Ch. 16." However, the `ashrae_wind_stack()` function in `crates/hares-physics/src/infiltration.rs` is the AIM-2 model (correctly identified in the module doc), but the function name suggests "ASHRAE wind-stack" which is ambiguous — EnergyPlus uses "ASHRAE" to refer to the Sherman-Grimsrud model and "AIM-2" for the enhanced Walker-Wilson model. The `docs/reviews/infiltration-deep/infil-deep-01-infiltration-n-exponent.md:85` already flags this naming confusion.

**Code Location**:
- `crates/hares-physics/src/infiltration.rs:26-28` — doc comment correctly identifies AIM-2
- `crates/hares-physics/src/infiltration.rs:133` — function name `ashrae_wind_stack()` is misleading
- `docs/validation.md:164` — "ASHRAE AIM-2 wind-stack model" (mixed naming)
- `docs/reviews/infiltration-deep/infil-deep-01-infiltration-n-exponent.md:85,168` — "naming is slightly confusing"

**Root Cause**: The function was named during early development before the Sherman-Grimsrud vs AIM-2 distinction was well-understood.

**Impact**: Low for simulation correctness. Medium for code-maintainability — a developer adding Sherman-Grimsrud infiltration might place it in the `ashrae_wind_stack()` function, not realizing it's the wrong model.

### Finding 14: Validation data sources lack version and access information [Severity: low]
**Description**: `docs/validation.md:250-262` lists 12 references but does not provide:
1. ASHRAE 140-2017: No purchase/paywall information, no summary of the test procedure.
2. PsychroLib: No version number or repository URL (github.com/psychrometrics/psychrolib).
3. ISA 1976 / ICAO Doc 7488: No summary of the specific atmospheric model used.
4. Henderson & Rengarajan (ASHRAE RP-1120): Year not stated (it is 1996 or 1997 — the final report is dated 1996).

The standards table is a useful reference but is incomplete as a reproducibility document. A researcher attempting to reproduce HARES validation results would need to independently discover these sources.

**Code Location**: `docs/validation.md:250-262`

**Root Cause**: The validation document was written as an internal team reference, not as a reproducibility statement for external researchers.

**Impact**: Low for internal use. Higher for external reproducibility — missing version information for validation data sources is a common barrier to computational reproducibility.

## Summary
- Total findings: 14
- Critical / High / Medium / Low: 0 / 4 / 7 / 3

## Recommendations

1. **Establish an EnergyPlus version policy** (Addresses F1, F3). Pin a single EnergyPlus version (recommend v24.2 or v25.2) as the sole algorithmic reference. Update all code comments and documentation to cite this version. Replace all §-style section references with descriptive heading names that match the web-hosted format, e.g., replace "EnergyPlus Engineering Reference §15.4" with "EnergyPlus Engineering Reference — Airflow Network Model: AIM-2 Enhanced Model."

2. **Pin a single ASHRAE HOF edition** (Addresses F2, F8). Adopt ASHRAE HOF 2021 as the sole authoritative edition for all psychrometric constants, molecular weights, and film coefficient tables. Audit `constants.rs` and `psychrometrics.rs` for 2017-vs-2021 discrepancies and reconcile to 2021 values. Document the choice.

3. **Add OCHRE provenance** (Addresses F4). Add to `docs/validation.md` and `docs/development.md` the OCHRE repository URL (github.com/NREL/OCHRE), the vendored commit hash, and a statement of whether OCHRE has been formally published. If OCHRE has no publication, document that parity testing uses a source-code reference and note the specific OCHRE commit against which parity was validated.

4. **Fix Cutler et al. (2013) citation** (Addresses F5). Update all references to the correct bibliographic entry: Cutler, D., Winkler, J., Kruis, N., Christensen, C., Brandemuehl, M. (2013). *Improved Modeling of Residential Air Conditioners and Heat Pumps for Energy Calculations*. NREL/TP-5500-56354.

5. **Fix Winkler year to 2009** (Addresses F6). Search all docs/ for "Winkler 2011" and replace with "Winkler (2009)."

6. **Complete the Berdahl-Martin citation fix** (Addresses F7). Add Li, Jiang & Coimbra (2017) citation to both `epw.rs` and `synthetic.rs` as the primary source of the `0.758/0.521/0.625` coefficients.

7. **Document BESTEST/ASHRAE 140 version and access** (Addresses F9, F14). Add to `docs/validation.md`: the specific ASHRAE 140 edition used for reference bands, the NREL BESTEST-GSR commit or release tag used, and a note that the standard is behind a paywall. Document whether the reference bands in `reference_bands.rs` were extracted from the standard directly or from the NREL BESTEST-GSR implementation.

8. **Document ResStock dataset versions** (Addresses F10). Add to `docs/validation.md` or a new `docs/resstock.md` the specific ResStock dataset release, weather vintage, and access URL. Document the OEDI S3 bucket paths and authentication requirements.

9. **Align solver documentation with implementation** (Addresses F11). Update `docs/thermal-envelope-solver.md` to describe the ZOH discretization actually implemented. Either remove the CN matrix descriptions or add a "Planned Improvement: Crank-Nicolson" section that clearly distinguishes planned from implemented.

10. **Add Burch-Christensen bounds** (Addresses F12). Document the model's valid range and consider adding the EnergyPlus 0 °C floor.

11. **Clarify AIM-2 vs Sherman-Grimsrud naming** (Addresses F13). Consider renaming `ashrae_wind_stack()` to `aim2_wind_stack()` or adding a prominent doc comment explaining the distinction.

12. **Add access information to validation references** (Addresses F14). Extend `docs/validation.md:250-262` with version numbers, URLs, and access notes for all 12 referenced sources.

## References / Citations
- ASHRAE Handbook of Fundamentals 2021, Chapters 1, 4, 16, 17, 24, 25, 26
- ASHRAE Handbook of Fundamentals 2017, Chapters 1, 16
- ASHRAE 140-2017, Standard Method of Test for the Evaluation of Building Energy Analysis Computer Programs
- EnergyPlus Engineering Reference v24.1, v25.2, v26.1 (Big Ladder Software)
- Walker, I.S. & Wilson, D.J. (1998). "Field Validation of Algebraic Equations for Stack and Wind Driven Air Infiltration Calculations." *HVAC&R Research*, 4(2), 119–139.
- Cutler, D., Winkler, J., Kruis, N., Christensen, C., Brandemuehl, M. (2013). "Improved Modeling of Residential Air Conditioners and Heat Pumps for Energy Calculations." NREL/TP-5500-56354.
- Winkler, J.M. (2009). "Development of a Component Based Simulation Tool for the Steady State and Transient Analysis of Vapor Compression Systems." Ph.D. dissertation, University of Maryland.
- Li, M., Jiang, Y. & Coimbra, C.F.M. (2017). "On the determination of atmospheric longwave irradiance under all-sky conditions." *Solar Energy*, 144, 40–48.
- Berdahl, P. & Martin, M. (1984). "Emissivity of clear skies." *Solar Energy*, 32(5), 663–664.
- Wilson, E. et al. (2021). "End-Use Load Profiles for the U.S. Building Stock." NREL/TP-5500-79367.
- Burch, J. & Christensen, C. (2007). "Towards Development of an Algorithm for Mains Water Temperature." *Proceedings of the 2007 ASES National Solar Conference*.
- NREL OCHRE: https://github.com/NREL/OCHRE
- NREL BESTEST-GSR: https://github.com/NREL/BESTEST-GSR
- NREL ResStock: https://resstock.nrel.gov/datasets
