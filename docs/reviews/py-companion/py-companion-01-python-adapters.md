# Python adapter models: PyBaMM battery, SAM battery, SAM PV correctness
**Review ID**: py-companion-01
**Category**: py-companion
**Date**: 2026-05-26

## Files Reviewed
- `python/ochre_next/adapters/pybamm_battery.py`
- `python/ochre_next/adapters/sam_battery.py`
- `python/ochre_next/adapters/sam_pv.py`

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Equipment/Battery.py` (OCHRE battery — degradation model, efficiency, thermal)
- `vendors/OCHRE/ochre/Equipment/PV.py` (OCHRE PV — SAM PVWatts integration, inverter handling)

## Findings

### Finding 1: [Severity: high] PyBaMM efficiency LUT uses hardcoded 3.6 V cell voltage regardless of chemistry
**Description**: In `_run_pybamm_efficiency`, the conversion from pack power (kW) to cell current (A) uses `(power_kw * 1000.0) / (3.6 * n_series)`, where `3.6` is a hardcoded per-cell voltage. This is approximately correct for NMC/NCA cells (~3.6 V nominal), but produces incorrect currents for other chemistries: LFP (~3.2 V, 12.5% error), LTO (~2.4 V, 50% error). The function accepts `chemistry` as a parameter but never uses it to select the correct nominal voltage.
**Code Location**: `python/ochre_next/adapters/pybamm_battery.py:253`
**Root Cause**: A literal `3.6` is used as cell voltage instead of deriving it from the chemistry or accepting it as a parameter. The `capacity_ah` and `n_series`/`n_parallel` parameters are accepted and used but the voltage is not.
**Impact**: For non-NMC chemistries, the computed current fed into PyBaMM is wrong, producing incorrect terminal voltage / efficiency values in the LUT. The downstream Rust `Battery` model would then interpolate wrong efficiency values for LFP/LTO batteries, silently corrupting round-trip efficiency calculations. The default fallback path (`_build_default_efficiency_table`) does not have this issue since it computes efficiency without a cell-voltage conversion.

### Finding 2: [Severity: high] SAM PV LUT AC/DC reconstruction assumes fixed 96% inverter efficiency
**Description**: SAM's `Outputs.ac` already has inverter efficiency (default 96%) and system losses (14%) baked in. The Rust PV LUT consumer (`crates/hares-equipment/src/pv/mod.rs:232`) divides the LUT's AC power by `self.inverter_efficiency` to reconstruct DC, then applies its own cell temperature, soiling, and shading. If a user overrides `inverter_efficiency` in `PvConfig` (e.g. to 97%), the Rust code divides by 0.97 while SAM baked in 0.96 — silently wrong DC reconstruction. The Rust non-LUT path does NOT have this problem because it applies efficiency directly.
**Code Location**: `python/ochre_next/adapters/sam_pv.py:119-134` (adapter does not expose `inv_eff` to SAM); `crates/hares-equipment/src/pv/mod.rs:232` (Rust divides LUT AC by config's `inverter_efficiency`)
**Root Cause**: The adapter hardcodes SAM's defaults by not passing `inv_eff` or `dc_ac_ratio` to SAM. The Rust side attempts to reconstruct DC but cannot know what efficiency SAM applied internally. In contrast, the OCHRE reference (`vendors/OCHRE/ochre/Equipment/PV.py:62`) passes `inv_efficiency` directly to SAM, so the AC output already matches the user's specification.
**Impact**: Any deviation of `PvConfig.inverter_efficiency` from SAM's default (0.96) yields a DC power that is off by a factor of `SAM_efficiency / config_efficiency`. For a 1% mismatch this produces ~1% error in DC power, which cascades into inverter clipping and grid export calculations.
**Recommendation**: Either (a) expose `inv_eff` through the adapter API and pass it to SAM, so the LUT AC output is consistent with a single config value; or (b) document that `inverter_efficiency` in `PvConfig` MUST match SAM's default (0.96) when using a SAM LUT, and ignore the config field in the LUT path.

### Finding 3: [Severity: high] PyBaMM degradation parameters are never actually computed from PyBaMM
**Description**: In `generate_degradation_params`, the PyBaMM branch (lines 413-422) runs `_pybamm.ParameterValues("Chen2020")` to check PyBaMM availability, constructs a `params` dict that is a copy of `_DEFAULT_DEGRADATION` plus a few extra keys (`chemistry`, `capacity_ah`, `reference_temperature_c`), but does not extract any degradation-relevant parameters from the PyBaMM model. The resulting degradation parameters are always the hardcoded defaults regardless of whether PyBaMM is available or not. The `calendar_q`, `cycle_q`, etc. values are constant across all chemistries in the PyBaMM path.
**Code Location**: `python/ochre_next/adapters/pybamm_battery.py:412-422`
**Root Cause**: The PyBaMM integration is skeletal — it loads a parameter set but does not query or simulate the model to derive chemistry-specific degradation coefficients. The entire `if _HAS_PYBAMM:` block behaves identically to the `else` block for degradation params.
**Impact**: Users who expect chemistry-specific degradation parameters from PyBaMM will silently get generic NMC defaults for all chemistries. The adapter provides no value over the built-in defaults for degradation. This defeats the purpose of having a PyBaMM-backed degradation path.

### Finding 4: [Severity: medium] SAM battery adapter extracts minimal parameters from PySAM — most are hardcoded defaults
**Description**: `_extract_from_sam` only reads `Vnom_default` (nominal cell voltage) and `resistance` (internal resistance) from PySAM's `BatteryStateful`. All other parameters — SOC-OCV curves, thermal parameters, loss parameters, degradation coefficients — come from the `_BUILTIN_DEFAULTS` dict and are never queried from PySAM. The SOC-OCV curves for each chemistry are static lists hardcoded in the adapter, not extracted from PySAM's cell model.
**Code Location**: `python/ochre_next/adapters/sam_battery.py:294-316`
**Root Cause**: The adapter only accesses `batt.ParamsCell.Vnom_default` and `batt.ParamsCell.resistance`. PySAM's `BatteryStateful` provides more parameters (OCV curves, thermal resistance, etc.) that are not utilized. The OCHRE reference similarly uses SAM defaults but OCHRE loads separate CSV OCV curves (`degradation_curves.csv`), which the adapter does not.
**Impact**: Battery models for different chemistries will differ only in nominal voltage, internal resistance, and the hardcoded OCV table, while real SAM models would provide more nuanced parameter sets. Users may be misled into thinking PySAM provides a full parameter extraction. Additionally, the OCV table hardcoded for NMC (`soc_ocv.v_oc = [3.0, 3.4, ..., 4.2]`) does not match what a PySAM `NMCGraphite` model would produce under parameter defaults.

### Finding 5: [Severity: medium] PyBaMM OCV curve generation runs 21 separate full SPM solves, each 1-second duration
**Description**: `generate_ocv_curve` runs a full PyBaMM SPM simulation for each of `n_points` (default 21) SOC values. Each simulation solves for only 1 second (`sim.solve([0, 1], initial_soc=...)`). The OCV is read from `entries[0]` (the initial condition), not from the end of the simulation. Since the initial condition already establishes the OCV, running a full solve is wasteful and slow — the OCV could be evaluated directly from the parameter set's OCP functions without running a simulation. Additionally, the OCV read from `entries[0]` may include transient initialization effects.
**Code Location**: `python/ochre_next/adapters/pybamm_battery.py:717-729`
**Root Cause**: The function uses `pybamm.Simulation` with `sim.solve()` for what should be a direct function evaluation. PyBaMM parameter sets store OCP functions (`U_p`, `U_n`) as `pybamm.Function` objects that can be evaluated at given stoichiometries directly, avoiding the overhead of a full solve.
**Impact**: Unnecessarily slow execution (~21× overhead per call vs. direct evaluation). Low practical impact since OCV generation is typically a one-time offline task, but could matter in iterative workflows or for users without optimized PyBaMM builds.

### Finding 6: [Severity: medium] Charging curve LUT fails silently at min/max operating conditions, returns zero power
**Description**: In `_simulate_cc_cv_single`, when the PyBaMM solve fails (e.g., at extreme temperatures or very low C-rates), the function returns `soc_traj = [initial_soc, 1.0]` and `power_fraction = [0.0, 0.0]` (line 662). This zero power-fraction trajectory propagates into the 4D LUT as zeros, meaning the charging curve will indicate zero allowable charge power at those grid points. The downstream Rust model will then clamp charge power to zero at those conditions, effectively disabling charging at extreme but physically possible operating points.
**Code Location**: `python/ochre_next/adapters/pybamm_battery.py:657-662`
**Root Cause**: The fallback returns `[0.0, 0.0]` for power fraction, which is a conservative but incorrect choice for edge cases. A better fallback would be to use the nearest successfully-solved grid point's trajectory or compute a conservative estimate from the parameter set.
**Impact**: Edge-case grid points in the LUT (extreme low temperature, high C-rate, low SOH) will report zero charge power capacity, causing the Rust battery model to reject charging even when the battery could accept charge (albeit at reduced power). This creates unexplained "charging disabled" behavior at the system boundaries.

### Finding 7: [Severity: medium] PyBaMM efficiency LUT runs 1-minute transient simulations — conflates transient and steady-state efficiency
**Description**: The `_run_pybamm_efficiency` function runs each grid point as a 60-second pulse (`sim.solve([0, 60])`) and computes efficiency from the final voltage and current. A 60-second transient includes ohmic and concentration overpotential transients that are not representative of steady-state behavior. For example, at high C-rates, the 60-second terminal voltage will be below the steady-state voltage due to incomplete mass transport, producing an efficiency that is lower than the true steady-state value. The OCHRE reference computes efficiency analytically using OCV and internal resistance with an algebraic voltage solution, avoiding this transient artifact.
**Code Location**: `python/ochre_next/adapters/pybamm_battery.py:253-257`
**Root Cause**: The simulation duration of 60 seconds is arbitrary. No analysis was performed to determine whether 60 seconds is sufficient for the model voltage/current to approach steady-state at the grid point conditions.
**Impact**: Efficiency LUT values are systematically biased at high C-rates (>1C), where transient overpotentials have not settled. The magnitude of bias is chemistry-dependent but could reach several percentage points at 2C, exceeding the <1% relative error tolerance specified in the review criteria.

### Finding 8: [Severity: low] LUT grid coarseness limits interpolation accuracy
**Description**: The efficiency LUT uses only 5 SOC points, 5 power points, and 3 temperature points (75 total evaluations). The 4D charging curve LUT uses higher-resolution grids (24 SOC, 7 temperature, 8 C-rate, 4 SOH = 1372 evaluations), but the 1D interpolation along SOC within each (T, C, SOH) cell assumes a smooth monotonic SOC→power_fraction relationship. In reality, CC-CV charging has a sharp knee at the transition from CC to CV, which linear interpolation across grid points can miss, causing errors up to 10-20% in the transition region.
**Code Location**: `python/ochre_next/adapters/pybamm_battery.py:235-237` (efficiency LUT grid sizes); `python/ochre_next/adapters/pybamm_battery.py:441-448` (charging curve SOC grid — note the non-uniform spacing `0.85, 0.88, 0.91, 0.94, 0.96, 0.98, 1.00` which partially mitigates this)
**Root Cause**: The efficiency LUT uses uniformly-spaced grid points with low resolution. The charging curve grid partially addresses this by offering higher-resolution grid in the high-SOC region where the CC→CV knee occurs, but the efficiency LUT does not.
**Impact**: For the efficiency LUT, interpolation errors of 1-3% in the high-SOC region are possible. The charging curve LUT is better resolved thanks to the non-uniform spacing. Neither exceeds typical engineering tolerance for residential load modeling, but falls short of the stated <1% tolerance for LUT accuracy.

### Finding 9: [Severity: low] SAM PV adapter does not wire `module_type` and `array_type` end-to-end
**Description**: The adapter API accepts `module_type` and `array_type` and passes them to SAM's `Pvwattsv8`, and includes them in the cache hash. However, these fields are never populated from user configuration in the Python-to-Rust binding layer — `py_dwelling.rs:426` hardcodes `module_type: None`. The Rust `PvConfig` has no `array_type` field at all. Consequently, all SAM LUTs are generated with `module_type=0` (Standard) and `array_type=0` (Fixed Roof) regardless of user intent.
**Code Location**: `python/ochre_next/adapters/sam_pv.py:123-125` (correct adapter code); `crates/hares-python/src/py_dwelling.rs:426` (never wired); `crates/hares-equipment/src/pv/config.rs:31` (has `module_type` but no `array_type`)
**Root Cause**: Incomplete integration — the adapter is more capable than the configuration system supports. This was likely planned but not yet implemented.
**Impact**: All PV LUTs use the PVWatts Standard module type temperature coefficient (-0.0047/°C) and Fixed Roof mounting losses, which is generally conservative. Premium panels would produce ~3-5% more energy in hot climates, but this is currently inaccessible through the configuration system.

### Finding 10: [Severity: low] SAM battery adapter TOML degradation section is never consumed by Rust `BatteryConfig`
**Description**: The adapter's `CellParamsDict` includes a `[degradation]` section with `model = "rainflow_arrhenius"`, `calendar_q`, `calendar_a`, `cycle_q`, `cycle_d`. The Rust `BatteryConfig` has NO degradation fields — it uses `#[serde(deny_unknown_fields)]` which would reject the degradation keys, and the Rust degradation model is hardcoded (Smith et al. 2017, implemented in `crates/hares-equipment/src/battery/degradation.rs`). The Python adapter's degradation parameters are never consumed by any downstream consumer.
**Code Location**: `python/ochre_next/adapters/sam_battery.py:95-101` (NMC degradation defaults); `crates/hares-equipment/src/battery/config.rs:12-71` (Rust BatteryConfig — no degradation fields)
**Root Cause**: The adapter was designed with a parameterized degradation model (`rainflow_arrhenius`) that does not correspond to any implemented Rust model. The Rust model implements the OCHRE-compatible Smith 2017 model directly with hardcoded physical constants.
**Impact**: No runtime impact — the degradation TOML section is simply dead data. However, users reading the TOML output may incorrectly assume their degradation parameters are being used. The module documentation does not clarify that degradation parameters are informational only.

## Summary
- **Total findings**: 10
- **High**: 3 (Findings 1, 2, 3)
- **Medium**: 4 (Findings 4, 5, 6, 7)
- **Low**: 3 (Findings 8, 9, 10)

## Recommendations
1. **Parameterize cell voltage in PyBaMM efficiency** — Replace the hardcoded `3.6` in `_run_pybamm_efficiency` (line 253) with a voltage derived from the chemistry or passed as a parameter, matching the nominal voltages already defined in `sam_battery.py:_BUILTIN_DEFAULTS` (3.6 for NMC, 3.2 for LFP, 2.4 for LTO).
2. **Reconcile SAM PV inverter efficiency** — Choose a single approach: either pass `inv_eff` to SAM and remove the Rust-side DC reconstruction, or document that `PvConfig.inverter_efficiency` is ignored in the LUT path and always uses 0.96. If keeping the Rust reconstruction, hardcode 0.96 in the LUT path rather than reading a configurable value.
3. **Implement actual PyBaMM degradation extraction** — Either compute chemistry-specific degradation coefficients from PyBaMM (by aging simulations), or remove the PyBaMM degradation path to avoid user confusion. Currently it is a no-op that claims to use PyBaMM but returns defaults.
4. **Extend SAM battery extraction** — Query PySAM for OCV curves (`batt.ParamsCell.voltage_curve`) and thermal parameters (`batt.ParamsCell.thermal_resistance`) to leverage more of what PySAM provides, reducing reliance on hardcoded defaults.
5. **Add LUT edge-case handling** — When PyBaMM solve fails for a grid point in the charging curve LUT, extrapolate from nearest valid neighbor rather than returning zero power fraction, to avoid holes in the operating envelope.
6. **Wire module_type and array_type through the Python→Rust bridge** — Add `array_type` to `PvConfig` and populate both fields from Python configuration.

## References / Citations
- Smith, K. et al. (2017) "Life prediction model for grid-connected Li-ion battery energy storage system." IEEE Control Systems Society. IEEE 7963578. Used in OCHRE `Battery.calculate_degradation()` and HARES Rust `DegradationState`.
- PySAM PVWatts v8 module — SAM's PVWatts model applies inverter efficiency, temperature derating, and DC/AC ratio internally. `Outputs.ac` is net AC power in Watts.
- PyBaMM SPM/SPMe models — `lithium_ion.SPM()` and `lithium_ion.SPMe()` are reduced-order electrochemical models. Terminal voltage at any timestep includes ohmic, concentration, and kinetic overpotentials.
- OCHRE reference: `vendors/OCHRE/ochre/Equipment/PV.py:58` — azimuth conversion `(azimuth + 180) % 360` ensures SAM receives correct 180=south convention.
