# gen_ocv.py: PyBaMM chemistry parameter sets and stoichiometry verification
**Review ID**: scr-01
**Category**: scripts
**Date**: 2026-05-26

## Files Reviewed
scripts/gen_ocv.py

## Vendor/Reference Files Consulted
None

## Findings

### Finding 1: [Severity: medium]
**Description**: The Rust battery model hardcodes OCV tables as `vec![]` literals in `crates/hares-equipment/src/battery/ocv.rs`. The `gen_ocv.py` script generates Rust `vec![]` literal output (lines 218–224), but there is no automated pipeline to regenerate these hardcoded tables from PyBaMM. If PyBaMM updates the Chen2020/Prada2013/NCA_Kim2011 parameter sets, the Rust OCV tables will silently desync from the Python generation script.
**Code Location**: `scripts/gen_ocv.py:206–224` (Rust output section); `crates/hares-equipment/src/battery/ocv.rs:37–46` (hardcoded NMC OCV)
**Root Cause**: No automated CI step or script hook to regenerate the Rust OCV constants from `gen_ocv.py`. The Rust code does not load OCV data from an external file at runtime by default—the `set_ocv_table()` method exists for runtime injection but is not the default path.
**Impact**: If PyBaMM updates the Chen2020 parameter set (e.g., improves OCP fitting) and a developer re-runs `gen_ocv.py`, the new output will differ from the hardcoded Rust values. Without a regeneration check, the repository could contain stale OCV curves. The `BatteryLutType::Ocv` (`crates/hares-types/src/equipment.rs:250–254`) suggests CSV/TOML-based LUT injection is expected, but `gen_ocv.py` does not produce CSV or TOML output.

### Finding 2: [Severity: low]
**Description**: LTO OCV stoichiometry limits are hardcoded (lines 182–183: `x_n_min_lto = 0.02`, `x_n_max_lto = 0.97`) rather than derived from any physical model. The script comment on line 167 states "the OCP is so flat the ElectrodeSOHSolver would struggle," which is a reasonable justification, but this means the LTO full-cell OCV curve is not balanced against the NMC811 cathode by the same `ElectrodeSOHSolver` mechanism used for the other chemistries.
**Code Location**: `scripts/gen_ocv.py:182–183`
**Root Cause**: LTO has no native PyBaMM parameter set, so stoichiometry matching must be done manually. The script reuses NMC811 positive stoichiometry limits from Chen2020 (`x_p_min_nmc`, `x_p_max_nmc`) with manually selected LTO negative limits.
**Impact**: The resulting LTO full-cell OCV range (2.047V–2.734V, per `ocv.rs:96–103`) is plausible for an NMC811/LTO cell but is not independently verified against published NMC/LTO full-cell data. The flat LTO negative OCP (~1.556V) means the full-cell OCV is dominated by the NMC positive slope, so small errors in positive stoichiometry mapping cause proportionally large full-cell OCV errors.

### Finding 3: [Severity: low]
**Description**: The LFP OCV curves are attributed to "Afshar2017 LFP OCP" in both the script docstring (line 6) and the Rust module docstring (`ocv.rs:9`), but the PyBaMM parameter set used is `"Prada2013"` (line 124). These are distinct parameter sets: Prada2013 (ECS Trans 2013) uses a Parson-type OCP function originally parametrized for LFP by Prada et al., while Afshar2017 is a separate paper that provides LFP OCP data for a graphite/LFP cell. The `ElectrodeSOHSolver` will use whichever OCP function is actually registered in the "Prada2013" PyBaMM parameter set—this may or may not source from Afshar2017 internally.
**Code Location**: `scripts/gen_ocv.py:6` and `crates/hares-equipment/src/battery/ocv.rs:9`
**Root Cause**: The code uses the PyBaMM parameter set name "Prada2013" but comments and documentation inconsistently reference "Afshar2017 LFP OCP." In PyBaMM, the "Prada2013" parameter set may have been updated to incorporate Afshar2017's LFP OCP at some version, but this coupling is implicit and fragile.
**Impact**: If a developer re-runs `gen_ocv.py` with a different PyBaMM version where Prada2013's OCP comes from a different source, the regenerated OCV values would change, and the comments referencing "Afshar2017" would be misleading. This is primarily a documentation/maintenance concern.

### Finding 4: [Severity: low]
**Description**: The LFP OCV from Prada2013 shows a very wide voltage range (2.000V at SOC=0 to 3.600V at SOC=1) per `ocv.rs:64–72`. In the flat-plateau region (SOC ~0.12–0.92), the voltage changes from 3.183V to 3.350V, a slope of approximately 0.21V over 0.80 SOC units (~0.26 V/SOC). This is steeper than expected for high-quality LFP cathode materials, where the plateau is extremely flat (3.35–3.45V, slope < 0.05 V/SOC). The Prada2013 OCP function may produce this artifact from its Parson-type fitting, which is known to over-smooth low-SOC regions for LFP.
**Code Location**: `scripts/gen_ocv.py:124–138` (LFP generation); `crates/hares-equipment/src/battery/ocv.rs:64–72` (hardcoded LFP values)
**Root Cause**: The Prada2013 parameter set uses a smoothed OCP approximation (Parson function) rather than raw experimental LFP half-cell data. This smooths the two-phase transition shoulder that real LFP exhibits below SOC ~0.1, producing a wide low-voltage tail (2.0V at SOC=0).
**Impact**: The test at `ocv.rs:301–306` correctly bounds SOC=0.5 LFP voltage in [3.20, 3.35], and the monotonicity test (`ocv.rs:345–349`) passes for all chemistries. The practical impact on simulation is that SOC estimation at low SOC (< 0.15) would have degraded accuracy, but the battery model clamps between `min_soc=0.15` and `max_soc=0.95` by default (`mod.rs:123–124`), so the low-voltage tail is largely outside the operating window. However, the 2 mV → 10% SOC rule of thumb means the 51-point resolution (0.02 SOC steps) at ~0.26 V/SOC gives ~5.2 mV/step in plateau, corresponding to ~20% SOC per step—coarse but not systematically biased.

### Finding 5: [Severity: low]
**Description**: The `build_ocv_curve` function (lines 27–49) iterates over each SOC point and calls the PyBaMM OCP function per point in a Python loop (`np.array([float(u_pos_fn(xp)) for xp in x_pos])` on line 46). This is correct but may be slow for large SOC grids. For 51 points this is acceptable.
**Code Location**: `scripts/gen_ocv.py:46–47`
**Root Cause**: PyBaMM OCP functions are not vectorized; they expect scalar or array inputs. The list comprehension is a workable approach for 51 points.
**Impact**: No correctness impact. Minor performance note for documentation purposes only.

## Summary
- Total findings: 5
- Critical / High / Medium / Low: 0 / 0 / 1 / 4

## Recommendations
1. Add a CI check that runs `gen_ocv.py` and diffs the generated Rust literals against the hardcoded values in `ocv.rs`, failing if they diverge. This ensures the hardcoded tables stay synchronized with the PyBaMM parameter set versions used by the project.
2. Document the exact PyBaMM version used to generate the current hardcoded tables (e.g., in a comment in `ocv.rs` or in a `pyproject.toml` pinned dependency) so that future regenerations are reproducible.
3. Clarify the LFP OCP source in comments: determine whether the PyBaMM "Prada2013" parameter set currently uses Prada's 2013 OCP function or Afshar2017's OCP data, and update the docstring on line 6 of `gen_ocv.py` and line 9 of `ocv.rs` to match.
4. Consider generating CSV or TOML output from `gen_ocv.py` as an alternative to Rust `vec![]` literals, leveraging the `BatteryLutType::Ocv` enum and the `Battery::set_ocv_table()` runtime injection path for user-customizable chemistries. This would also make the generated data usable by the `BatteryChemistry::for_chemistry` dispatch without recompilation.
5. For LTO, consider adding a literature citation for the NMC811/LTO full-cell stoichiometry limits (lines 182–187) to support the manual selection of `x_n_min_lto = 0.02`, `x_n_max_lto = 0.97`, and the reuse of Chen2020 positive limits.

## References / Citations
- Chen2020 PyBaMM parameter set: Chen CH, et al. (2020) "Development of Experimental Techniques for Parameterization of Multi-scale Lithium-ion Battery Models." JES 167:080534. (LG M50 NMC811/graphite cell)
- Prada2013 PyBaMM parameter set: Prada E, et al. (2013) "Simplified Electrochemical and Thermal Model of LiFePO4-Graphite Li-Ion Batteries." ECS Trans 50(45):63–73.
- NCA_Kim2011 PyBaMM parameter set: Kim GH, et al. (2011) "Multi-Domain Modeling of Lithium-Ion Batteries Encompassing Multi-Physics in Varied Length Scales." JES 158(8):A955–A969.
- Colclasure2011 LTO OCP: Colclasure AM, et al. (2011) "Modeling detailed chemistry and transport for solid-electrolyte-interface (SEI) films in Li–ion batteries." Electrochimica Acta 58:33–43.
- Afshar2017 LFP OCP: Afshar S, et al. (2017) (PyBaMM LFP OCP parameterization source for graphite/LFP cells)
