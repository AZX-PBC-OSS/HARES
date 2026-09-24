# ochre_bestest_600.py: OCHRE-side BESTEST runner geometry and materials
**Review ID**: scr-03
**Category**: scripts
**Date**: 2026-05-26

## Files Reviewed
- `scripts/ochre_bestest_600.py` (291 lines)

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Models/Envelope.py` — Envelope, Zone, Boundary, BoundarySurface classes; radiation solvers; RC assembly
- `vendors/OCHRE/ochre/utils/envelope.py` — `calculate_film_resistances()` (lines 342–402), `get_boundary_rc_values()`, `create_rc_data()`, solar irradiance
- `vendors/OCHRE/ochre/utils/hpxml.py` — HPXML parser, internal gain convective/radiative split defaults (line 1567: `"Radiative Gain Fraction (-)": 0`)
- `vendors/OCHRE/ochre/defaults/Envelope/Envelope Boundaries.csv` — 33 boundary-to-zone mappings
- `vendors/OCHRE/ochre/defaults/Envelope/Envelope Materials.csv` — ~700+ material layers
- `tests/fixtures/bestest/600.toml` — HARES-side BESTEST Case 600 fixture (geometry, materials, gains, weather, simulation)
- `tests/bestest/cases.rs` — HARES-side simulation parameters (timestep=3600s)
- `docs/findings/bestest_rca.md` — HARES root-cause analysis documentation

---

## Findings

### Finding 1: [Severity: critical] Script does not actually run an OCHRE simulation — no parity comparison data exists

**Description**: The script title and docstring (lines 1–16) state it "constructs the OCHRE envelope directly (bypassing HPXML) and runs an annual simulation, comparing results to ASHRAE 140 bands." In reality, the script performs only analytical calculations and prints diagnostics. It never instantiates `Envelope()`, never calls `.simulate()`, never constructs boundaries, never loads weather data, and never produces any output results. The script concludes at line 289–291: "Need to run OCHRE Case 600 in both 'full' and 'linear' modes" — acknowledging the simulation was never executed.

**Code Location**: `scripts/ochre_bestest_600.py:28` imports `Envelope` but the class is never instantiated; lines 66–85 call only utility functions (`calculate_film_resistances`); lines 159–291 are purely analytical diagnostics.

**Root Cause**: The script was written as a feasibility exploration / analytical pre-study and not as a functional runner. The docstring and script name are misleading — they describe intended functionality, not actual functionality.

**Impact**: There is no OCHRE-side BESTEST simulation data to compare against the HARES-side results. The HARES-OCHRE parity comparison is completely one-sided, making it impossible to attribute discrepancies between OCHRE and HARES model results to input differences vs. physics model differences.

---

### Finding 2: [Severity: high] Window SHGC discrepancy between OCHRE script and HARES fixtures

**Description**: The OCHRE script comment at line 113 specifies `SHGC = 0.767` for the Case 600 window. All five HARES BESTEST TOML fixtures (including `600.toml:208`) use `shgc = 0.789`. This is a 2.9% difference. Additionally, the HARES documentation inconsistently describes this window as "single-pane clear glass" (`docs/findings/physics_defaults.md:300`) despite U=3.0 W/m²K being physically impossible for single-pane glazing (~5.7 W/m²K expected).

**Code Location**: `scripts/ochre_bestest_600.py:113` and `tests/fixtures/bestest/600.toml:208,216`

**Root Cause**: The provenance of the 0.789 value in HARES is undocumented. The `bestest_rca.md:367` verification table asserts it matches ASHRAE 140-2017 without citing a specific table or page. The OCHRE script's 0.767 value likely comes from a different interpretation (total-window including frame effects vs. center-of-glass only). No source document in the repository resolves which value is correct.

**Impact**: A 2.9% SHGC difference translates to ~2.9% difference in transmitted solar gain. For BESTEST Case 600 with 12 m² of south-facing glazing under Denver TMY3, this compounds over the annual simulation and could push heating/cooling loads across ASHRAE 140 reference band boundaries.

---

### Finding 3: [Severity: high] Internal gains radiant/convective split is triple-inconsistent

**Description**: Three different internal gains configurations reference the "same" Case 600:

| Source | Radiative % | Convective % |
|---|---|---|
| OCHRE script docstring (line 11) | 60% | 40% |
| HARES `600.toml:37` | 30% | 70% |
| OCHRE actual HPXML model (hpxml.py:1567) | 0% | 100% |

The OCHRE script's stated 60/40 split is not what OCHRE's model actually does (OCHRE routes all internal gains 100% convectively — `"Radiative Gain Fraction (-)": 0` is hardcoded at `hpxml.py:1567`). The HARES TOML fixture's 30/70 split contradicts the OCHRE script's stated 60/40. None of the three values match each other.

**Code Location**: `scripts/ochre_bestest_600.py:11`, `tests/fixtures/bestest/600.toml:37`, `vendors/OCHRE/ochre/utils/hpxml.py:1567`

**Root Cause**: The OCHRE script author assumed a 60/40 radiative/convective split for BESTEST when documenting the intended setup, but: (a) OCHRE's actual model cannot inject radiant internal gains (confirmed by `Equipment.py:79–87` TODO comment: "FUTURE: separate convection and radiation"), and (b) HARES chose 30/70 with no documented justification.

**Impact**: The fraction of internal gains delivered as radiation vs. convection affects how quickly gains reach the zone air vs. being absorbed by thermal mass. This changes both steady-state thermal balance and transient response. The 30 percentage point difference (60% vs. 30% radiant) in a 200 W continuous load shifts 60 W between immediate zone-air heating and surface-absorbed heating each timestep, significantly affecting heating/cooling load predictions.

---

### Finding 4: [Severity: medium] Material layer thicknesses in analytical calculations do not match HARES fixtures

**Description**: The OCHRE script uses material thicknesses in its wall thermal mass calculation (lines 240–243) that differ from the HARES `600.toml` fixture:

| Layer | OCHRE script (analytical) | HARES `600.toml` |
|---|---|---|
| Wood siding | 5 mm (rho=544, cp=1210) | 9 mm (rho=530, cp=900) |
| Insulation | 65 mm (rho=12, cp=840) | 66 mm (rho=12, cp=840) |
| Plasterboard | 12 mm (rho=950, cp=840) | 12 mm (rho=950, cp=840) |

Additionally, the wood siding density differs (544 vs. 530 kg/m³) and specific heat differs (1210 vs. 900 J/kg·K). The resulting C_wall = 13,522 J/m²K (line 246) vs. the correct value from HARES materials: (0.009×530×900) + (0.066×12×840) + (0.012×950×840) = 4,293 + 665 + 9,576 = 14,534 J/m²K — an 8.1% under-estimate.

**Code Location**: `scripts/ochre_bestest_600.py:240–246` vs. `tests/fixtures/bestest/600.toml:50–66`

**Root Cause**: The script author used approximate BESTEST material values from memory or a secondary reference rather than reading the actual TOML fixture. The script was written as exploration, not as a verified parity runner.

**Impact**: The analytical time constant calculations (lines 248–260) are based on wrong thermal capacitance, invalidating the script's conclusion that "the difference in time constant is ~0.55 hours" (line 262). The script's diagnostic about transient lag being the root cause of BESTEST failures is not reliably grounded on correct inputs.

---

### Finding 5: [Severity: medium] Film resistance computation uses constant parameters vs. HARES's timestep-varying approach

**Description**: The OCHRE script calls `calculate_film_resistances()` (lines 66–85) which uses:
- Constant interior delta_T clamped to max(12.9, 10°C) = 12.9°C (envelope.py:374)
- Constant average wind speed 4.02 m/s (script line 54)
- Constant average ambient temperature + 5°C for exterior surface temperature estimate (envelope.py:364)
- Hardcoded roughness factor r_f = 1.67 for all surfaces (envelope.py:393)

HARES evaluates TARP natural convection coefficients per-timestep using actual wall-to-zone delta_T and hourly wind speeds from the weather file.

**Code Location**: `scripts/ochre_bestest_600.py:66–85`, `vendors/OCHRE/ochre/utils/envelope.py:374, 390–395`

**Root Cause**: OCHRE's design uses pre-computed film resistances based on annual-average conditions. HARES recomputes convection coefficients at each timestep. This is an architectural difference between the models, not an error per se, but the OCHRE script does not account for the effect of this approximation on the parity comparison.

**Impact**: Constant film resistances cannot capture diurnal or seasonal variation in convection (e.g., higher wind speeds in winter increasing exterior h_forced). For a lightweight building like Case 600, surface temperatures respond quickly to changing conditions, and a constant exterior film resistance flattens this response. This could contribute on the order of 1–3% difference in annual loads, independent of any physics model differences.

---

### Finding 6: [Severity: medium] No simulation settings are configured — timestep, warmup, and solver tolerance are absent

**Description**: The OCHRE script contains zero references to timestep, solver tolerance, or warmup period. The HARES fixture specifies `time_res_s = 3600` (1-hour), `initialization_duration_s = 86400` (1-day warmup for lightweight construction), and uses forward Euler with sub-iterations (`config.rs:396` — 13 sub-steps per timestep). Because the OCHRE script never initializes a simulation, no comparable settings are defined.

**Code Location**: `tests/fixtures/bestest/600.toml:5,7` (HARES settings present); `scripts/ochre_bestest_600.py` (entire file — no simulation settings)

**Root Cause**: The script is a diagnostic stub, not a functional runner.

**Impact**: Even if the script were completed to run OCHRE, the absence of defined simulation settings means OCHRE would run with its defaults (which are undocumented in the script), creating an uncontrolled variable in any future parity comparison.

---

### Finding 7: [Severity: low] Best Practice only — BESTEST Case 600 window area appears as single 12 m² window vs. two 6 m² windows

**Description**: The OCHRE script defines `WINDOW_AREA = 12.0` (line 42) as a single south-facing window. The HARES `600.toml` defines two windows of 6 m² each (lines 203–217), also totaling 12 m² south-facing. The total area is equal. However, the OCHRE script's `SOUTH_WALL_AREA` (line 36) computes `8 × 2.7 − 12 = 9.6 m²`, and the HARES south wall is also 9.6 m². Both agree that the south wall net area = 8 × 2.7 − 12 = 9.6 m².

**Code Location**: `scripts/ochre_bestest_600.py:36` and `tests/fixtures/bestest/600.toml:42,203–217`

**Impact**: The total window area and south wall net area match (12 m² total glazing, 9.6 m² net wall). A known issue exists that 12 m² window on a 9.6 m² wall exceeds the wall area (documented at `docs/reviews/core-deep/coredeep-09-synthetic-building-rc-validity.md:26`), but this is identical in both OCHRE and HARES setups and does not invalidate the parity comparison.

---

### Finding 8: [Severity: low] Weather file is not specified in OCHRE script

**Description**: The OCHRE script mentions "Denver TMY weather" in a comment (line 13) but never references a specific EPW file path. The HARES `600.toml:24` specifies `epw_path = "../../../vendors/OCHRE/ochre/defaults/Weather/USA_CO_Denver.Intl.AP.725650_TMY3.epw"`. Without the same weather file, any future OCHRE run would produce results incomparable to HARES.

**Code Location**: `scripts/ochre_bestest_600.py:13` and `tests/fixtures/bestest/600.toml:24`

**Impact**: Weather file identity is critical for parity comparison. If different EPW files were used, meteorological differences could be misattributed to physics model differences.

---

### Finding 9: [Severity: low] Exterior emittance and solar absorptance not explicit in OCHRE script

**Description**: The HARES `600.toml` explicitly sets `solar_absorptance = 0.6` and `emittance = 0.9` on all opaque boundaries (per ASHRAE 140 specification). The OCHRE script never sets these values. OCHRE's defaults for exterior surfaces are emissivity = 0.90 and solar absorptivity = 0.60 (`Envelope.py:225–227`), which happen to match HARES, but this is not verified in the script and could change if OCHRE defaults are modified.

**Code Location**: `tests/fixtures/bestest/600.toml:45–46,74–75,103–104,133–134,160–161` and `vendors/OCHRE/ochre/Models/Envelope.py:225–227`

**Impact**: Solar absorptance controls opaque solar gain (affecting cooling loads in summer); emittance controls longwave radiation exchange. These currently match by coincidence of defaults, but the coincidence is untested and unverified in the script.

---

### Finding 10: [Severity: low] Comment misstates roof interior film resistance for BESTEST convention

**Description**: Lines 102–106 speculate about roof interior film resistances using ASHRAE convention values (`R_si = 1/9.26 = 0.108` upward, `R_si = 1/6.13 = 0.163` downward). The BESTEST spec uses combined film coefficients (convection + radiation), while OCHRE uses convection-only film coefficients with explicit LWR modeling. The script's analytical comparison conflates "convection-only R_film" with "ASHRAE combined R_si" without clearly distinguishing them. The comment at line 105 states "for roof (heat up through roof in winter), downward flow: R_si = 1/6.13 = 0.163" — this is physically correct for ASHRAE combined coefficients but is not the value OCHRE would compute (OCHRE uses TARP for interior h_natural, which for a horizontal surface with cold-above gives enhanced convection, the opposite of the ASHRAE downward convention).

**Code Location**: `scripts/ochre_bestest_600.py:102–106`

**Impact**: The analytical comparison incorrectly implies that the roof interior film resistance is 0.163 m²K/W, when OCHRE's TARP algorithm for a roof surface (hot interior below, cold exterior above) would produce a different value (~0.11 m²K/W for enhanced convection at delta_T=12.9°C). This confusion propagates through the script's analytical diagnostics.

---

## Summary

- **Total findings**: 10
- **Critical**: 1 (script does not run any OCHRE simulation)
- **High**: 2 (SHGC 0.767 vs 0.789; internal gains split triple-inconsistent at 60/40 vs 30/70 vs 0/100)
- **Medium**: 3 (wrong material thicknesses in analytical calcs; constant vs. timestep-varying film resistances; no simulation settings configured)
- **Low**: 4 (window area representation; weather file not specified; emittance/absorptance implicit; roof film resistance misstatement)

**Critical conclusion**: The script `ochre_bestest_600.py` does not produce any OCHRE-side BESTEST data for parity comparison. It is a diagnostic exploration that:
1. Imports OCHRE but never runs it
2. Uses incorrect material properties (5mm siding, 65mm insulation, wrong density/specific heat) in analytical
3. Assumes an internal gains split (60/40) that matches neither OCHRE's actual model (0/100) nor HARES's configuration (30/70)
4. Uses a different SHGC value (0.767) than HARES (0.789)
5. Does not define timestep, warmup period, solver tolerance, or weather file path

**The HARES-OCHRE parity comparison has no valid OCHRE-side reference data.** The current COR-03 and COR-04 review findings that attribute BESTEST failures to OCHRE's convection-only R_film architecture are based on analytical reasoning from this script, not on actual OCHRE simulation results compared against identical inputs.

## Recommendations

1. **Complete the OCHRE BESTEST runner**: Either extend this script or create a new one that actually instantiates an OCHRE `Envelope` (or `Dwelling`), populates boundaries with materials matching `600.toml` exactly, sets simulation parameters (timestep=3600s, warmup=1 day), and produces annual heating/cooling load outputs.

2. **Resolve SHGC value**: Determine authoritatively whether BESTEST Case 600 uses SHGC=0.789 or 0.767. If the latter, update all HARES TOML fixtures and document the value's provenance. The current 0.789 value lacks a cited source.

3. **Resolve internal gains split**: Align the internal gains radiative/convective split across OCHRE and HARES. Confirm what the BESTEST/EnergyPlus reference actually specifies and use it consistently. Document it.

4. **Fix OCHRE's inability to inject radiant internal gains**: OCHRE's hardcoded `"Radiative Gain Fraction (-)": 0` in `hpxml.py:1567` makes it impossible to match the BESTEST specification (which requires a non-zero radiant fraction). This is a prerequisite for any parity comparison.

5. **Verify material properties**: Before any OCHRE simulation, verify that all material layer thicknesses, conductivities, densities, and specific heats match `600.toml` exactly. The OCHRE script's analytical values (5mm siding, 544 kg/m³) are wrong.

6. **Document the parity comparison methodology**: The review process should establish whether film resistances are compared as pre-computed vs. timestep-varying, and account for this difference in any comparative analysis.

7. **Specify weather file explicitly**: Any OCHRE run MUST use the identical EPW file (`vendors/OCHRE/ochre/defaults/Weather/USA_CO_Denver.Intl.AP.725650_TMY3.epw`).

## References / Citations

- `scripts/ochre_bestest_600.py:1–291` — Script under review
- `vendors/OCHRE/ochre/Models/Envelope.py:342–402` — `calculate_film_resistances()` with TARP/DOE-2 models
- `vendors/OCHRE/ochre/Models/Envelope.py:901–908` — Occupancy internal gains (convective only)
- `vendors/OCHRE/ochre/Models/Envelope.py:1242–1299` — Internal gains injection into RC network
- `vendors/OCHRE/ochre/utils/hpxml.py:1567` — `"Radiative Gain Fraction (-)": 0` hardcoded
- `vendors/OCHRE/ochre/utils/envelope.py:374` — Delta_T clamped to max(12.9, ...) for film resistance
- `vendors/OCHRE/ochre/utils/envelope.py:377` — TODO for ASHRAE 140 film coefficients
- `vendors/OCHRE/ochre/utils/envelope.py:529` — Sea-level density with TODO for altitude correction
- `vendors/OCHRE/ochre/Equipment/Equipment.py:79–87` — TODO for separating convection and radiation
- `tests/fixtures/bestest/600.toml:1–227` — HARES BESTEST Case 600 fixture
- `tests/bestest/cases.rs:24–33` — HARES timestep=3600s for core cases
- `docs/findings/bestest_rca.md:30,137,141,367` — HARES uses SHGC=0.789
- `docs/findings/physics_defaults.md:300` — Incorrect "single-pane" description of BESTEST window
- `docs/reviews/core-deep/coredeep-09-synthetic-building-rc-validity.md:26` — Window-to-wall area ratio issue
- ASHRAE 140-2017 — Paywalled standard; no copy in repository; reference bands per `tests/bestest/reference_bands.rs`
