# PV LUT SAM path: 6-D interpolation grid parameters and LUT generation
**Review ID**: dercat-10
**Category**: der-catalog
**Date**: 2026-05-26

## Files Reviewed
crates/hares-equipment/src/pv/lut.rs

## Vendor/Reference Files Consulted
vendors/OCHRE/ochre/Equipment/PV.py
vendors/EnergyPlus/src/EnergyPlus/PVWatts.cc, Photovoltaics.cc, DataPhotovoltaics.hh
vendors/EnergyPlus/third_party/ssc/shared/lib_pvwatts.h, lib_pvwatts.cpp
vendors/EnergyPlus/third_party/ssc/ssc/cmod_pvwattsv5.cpp, cmod_pvwattsv1_1ts.cpp
python/ochre_next/adapters/sam_pv.py (LUT generator)

## Findings

### Finding 1: [Severity: high]
**Description**: The LUT is indexed on month/hour temporal bins instead of solar-position-aware dimensions, making it weather-file-specific. SAM's PVWatts internally computes POA irradiance from sun position (latitude/longitude/date/time), so the LUT output for a given GHI/DNI/DHI/temperature tuple depends on the solar geometry at generation time. A LUT generated from a Phoenix EPW will produce different AC power for the same weather inputs than one generated from a Seattle EPW, because the sun angles are encoded in the output values but not in the query keys. If a user generates a LUT from one weather file but runs with different weather data, the results will be systematically biased.

**Code Location**: `python/ochre_next/adapters/sam_pv.py:153-155` (month/hour binning from hour_of_year); `crates/hares-equipment/src/pv/lut.rs:35-41` (PvLut struct with month/hour axes); `crates/hares-equipment/src/pv/mod.rs:215-218` (query uses calendar month/hour from current_time).

**Root Cause**: The LUT generator bins by `month` and `hour` (derived from sequential hour-of-year index), which are not parametric on solar geometry. A correct approach would either: (a) include solar zenith/azimuth as LUT dimensions, (b) normalize the LUT output by reference POA so that the consumer multiplies by actual POA, or (c) include latitude/longitude/day-of-year in the cache key and generate a fresh LUT per location.

**Impact**: If a LUT is reused across simulations with different locations, output power will be systematically biased (typically 5-15% error in tilted-plane irradiance translation). Even within the same location, the month-binning approximation of solar declination is granular and does not account for intra-month variation.

---

### Finding 2: [Severity: high]
**Description**: The LUT path and the non-LUT (direct equation) path apply inverter efficiency and system losses inconsistently, producing different results for the same physical array. The LUT path back-calculates DC power by dividing SAM's AC output by the configured inverter efficiency (`ac_power_kw / inverter_efficiency`), but SAM already applied its own internal inverter efficiency when producing the AC output. The system_losses_fraction is also NOT applied in the LUT path — SAM's internal losses are baked in. If the user configures `inverter_efficiency=0.96` and the SAM LUT was generated with SAM's default `inv_eff=0.96`, the LUT path double-applies the inverter loss: SAM applies one 96% loss, then HARES divides by 0.96 to "recover" DC, producing a DC value that is systematically 4% too high. Additionally, the non-LUT path subtracts `system_losses_fraction` from DC power, but the LUT path does not.

**Code Location**: LUT path DC back-calculation: `crates/hares-equipment/src/pv/mod.rs:232`; non-LUT path: `crates/hares-equipment/src/pv/mod.rs:256-258`. SAM LUT generator does not export SAM's internal `inv_eff` or `losses`: `python/ochre_next/adapters/sam_pv.py:121-125`.

**Root Cause**: The LUT records SAM's AC output, which is the final system output after SAM has applied its internal inverter model and system losses. HARES then treats this as a value from which it can recover "DC power" by dividing by its own configured inverter efficiency. The semantic mismatch between SAM's internal AC output (post-losses) and HARES' expectation creates a double-counting error.

**Impact**: DC power values from the LUT path are inflated by `1/inverter_efficiency` (approximately +4% for default 0.96 efficiency). System losses override is silently ignored. This can produce a 4-18% systematic over-estimate relative to the non-LUT path, depending on configured inverter efficiency and losses.

---

### Finding 3: [Severity: medium]
**Description**: The default NOCT value of 47°C does not match either of SAM's PVWatts v8 array-type defaults. SAM uses 45°C for open-rack/fixed-open-rack installs and 49°C for roof-mounted installs (see `cmod_pvwattsv5.cpp:195-197` and `lib_pvwatts.h:26`). The HARES default of 47°C is an arbitrary midpoint. When the LUT path is used, SAM's internal NOCT (based on `array_type`) is correct, but the configured `noct_c` is only used in the non-LUT path. When the non-LUT path is used, the 47°C default produces a cell temperature estimate that is ~2°C too low for roof-mounted and ~2°C too high for open-rack, translating to approximately ±1% error in DC power output.

**Code Location**: `crates/hares-equipment/src/pv/mod.rs:32` (`DEFAULT_NOCT_C = 47.0`); `vendors/EnergyPlus/third_party/ssc/ssc/cmod_pvwattsv5.cpp:195-197` (SAM NOCT defaults); `vendors/EnergyPlus/third_party/ssc/shared/lib_pvwatts.h:26` (`PVWATTS_INOCT = 45.0 + 273.15`).

**Root Cause**: The HARES NOCT default was chosen as a generic midpoint rather than aligning with the two SAM array-type defaults. There is no mechanism to automatically select the correct NOCT based on `array_type` (which is a LUT-generation parameter but not persisted in the LUT metadata or passed to the Rust runtime).

**Impact**: Cell temperature is biased by ±2°C depending on actual mounting configuration. This produces approximately ±1% error in DC power output through the temperature coefficient of power pathway. Small but systematic across all timesteps.

---

### Finding 4: [Severity: medium]
**Description**: The multi-linear interpolation across 6 dimensions frequently falls through to nearest-neighbor due to sparse grid coverage. The LUT is populated from weather data — only points that appear in the 8760-hour EPW file are present. For a typical irradiance binning at 50 W/m² and temperature at 5°C, many of the 2^6=64 corner combinations needed for multi-linear interpolation will be absent because the weather file never simultaneously produced, e.g., GHI=800, DNI=750, DHI=150, temp=25°C at hour=12 in month=6. When any corner is missing, the code falls back to nearest-neighbor, which has significantly higher interpolation error. There is no test or validation quantifying the frequency of this fallback under typical weather conditions.

**Code Location**: `crates/hares-equipment/src/pv/lut.rs:325-379` (interpolate function — multi-linear attempt at lines 322-351, nearest-neighbor fallback at lines 353-379).

**Root Cause**: The multi-linear interpolation requires data at all 64 corners of the hypercube. For a 6-D sparse grid generated from weather bins, this condition is rarely met. The LUT generator produces only points that actually occur in the weather file; it does not fill missing grid points. The sparse-grid nature of the LUT is inherent in the weather-file-driven generation approach, but the fallback to nearest-neighbor is the primary interpolation method in practice rather than an exceptional path.

**Impact**: In practice, most queries may fall through to nearest-neighbor interpolation, which has a maximum error proportional to the grid spacing (50 W/m² for irradiance, 5°C for temperature, 1 month, 1 hour). This can produce stepwise output changes when crossing bin boundaries, which is unphysical. The error is likely >>1% in many operating conditions, far exceeding the expected interpolation accuracy for a properly filled 6-D grid.

---

### Finding 5: [Severity: medium]
**Description**: Albedo is not a LUT dimension and has no configurable parameter in HARES PV. SAM's PVWatts has a configurable albedo parameter (`PVWATTS_ALBEDO = 0.2` per `lib_pvwatts.h:32`) and EnergyPlus adjusts albedo to 0.6 when snow is present (`cmod_pvwattsv1_1ts.cpp:180-184`). The LUT is generated with SAM's default albedo of 0.2, which is physically reasonable for bare ground but too low for snow-covered surfaces. There is no mechanism in HARES to configure albedo for PV arrays or to dynamically adjust it based on snow conditions.

**Code Location**: Albedo absent from `PvConfig`: `crates/hares-equipment/src/pv/config.rs:13-41`; absent from `PvArray`: `crates/hares-equipment/src/pv/array_config.rs:41-51`; SAM albedo default: `vendors/EnergyPlus/third_party/ssc/shared/lib_pvwatts.h:32`; EnergyPlus snow albedo adjustment: `vendors/EnergyPlus/third_party/ssc/ssc/cmod_pvwattsv1_1ts.cpp:180-184`.

**Root Cause**: The initial PV implementation omitted albedo as a configurable parameter. The LUT generator uses SAM's default (0.2) without exposing the parameter for customization.

**Impact**: For bare-ground conditions the 0.2 default is reasonable (error <1%). For snow-covered surfaces (albedo 0.5-0.8), the reflected irradiance component is underestimated by a factor of 2.5-4x, reducing POA irradiance by up to 10-15% for high-latitude winter simulations.

---

### Finding 6: [Severity: low]
**Description**: No interpolation error bound is documented anywhere in the codebase. The LUT approach intentionally trades interpolation error for computational speed, but neither the maximum error under typical operating conditions nor any validation against direct SAM runs is documented. The comment block at `crates/hares-equipment/src/pv/lut.rs:1-2` says "PV SAM look-up table: parsing, storage, and interpolation" with no mention of accuracy or error bounds.

**Code Location**: `crates/hares-equipment/src/pv/lut.rs:1-2` (module doc comment); no error quantification anywhere in the file or related tests.

**Root Cause**: No systematic validation of interpolation accuracy was performed or documented. The test at `crates/hares-equipment/src/pv/mod.rs:1576-1609` tests only the nearest-neighbor fallback mechanism, not actual LUT accuracy vs. direct SAM runs.

**Impact**: Users cannot assess the accuracy penalty of using LUT interpolation vs. direct equations. Without documented error bounds, the validity of results for regulatory or engineering use is unverifiable.

---

### Finding 7: [Severity: low]
**Description**: The LUT generator's month binning uses `hour_of_year // 730` (line 154 of sam_pv.py), which produces a rough 12-month approximation of 730 hours per month. The actual calendar month boundaries in an EPW file are not uniform — months range from 672 to 744 hours depending on length. This introduces a systematic monthly misalignment of up to ~1.5 days at month boundaries. The Rust consumer queries by calendar month (`env.current_time.month()`) which uses true calendar months, creating an additional discrepancy between the LUT generation binning and the query-time month values.

**Code Location**: `python/ochre_next/adapters/sam_pv.py:154` (`month = (hour_of_year // 730) + 1`); `crates/hares-equipment/src/pv/mod.rs:215` (`env.current_time.month()`).

**Root Cause**: The LUT generator uses a simplified flat 730-hour-per-month approximation while the Rust consumer uses true calendar months. The mismatch causes values intended for e.g. "December" (generated from hours 8030-8759) to be queried with calendar December dates that may fall in the "November" or "January" LUT bins.

**Impact**: At month boundaries, the LUT returns power values from an adjacent month, producing errors proportional to the seasonal variation in solar resource (typically 5-20% at month transitions for mid-latitude locations).

---

## Summary
- Total findings: 7
- Critical / High / Medium / Low: 0 / 2 / 3 / 2

## Recommendations
1. **Add solar position to the LUT dimensions** (or at minimum latitude/longitude to the cache key and documentation) so that LUTs are not silently reused across locations. Document that the LUT is location-specific. (Addresses Finding 1)
2. **Clarify the semantic boundary between SAM's internal losses and HARES-configured losses.** Either store SAM's inverter efficiency and system losses as LUT metadata and subtract them before applying HARES' own values, or document that `inverter_efficiency` and `system_losses_fraction` are ignored in the LUT path and log a warning when they are configured. (Addresses Finding 2)
3. **Align `DEFAULT_NOCT_C` with SAM array types.** Default to 45°C for open-rack and 49°C for roof-mounted, or make `noct_c` driven by the `array_type` configuration parameter. Add `array_type` to `PvConfig` and `PvArray`. (Addresses Finding 3)
4. **Populate missing grid points for multi-linear interpolation.** The LUT generator should fill every combination of month, hour, and binned GHI/DNI/DHI/temp that produces a physically valid PV output. Use SAM to compute all missing points (combinatorial grid) rather than relying on weather-file-observed points only, or use a regular grid with documented axis values. (Addresses Finding 4)
5. **Add albedo as a configurable parameter** on `PvConfig`/`PvArray` with a default of 0.2, forwarded to the SAM LUT generator. Consider dynamic albedo adjustment based on snow presence. (Addresses Finding 5)
6. **Document interpolation error** by running a representative validation: compare LUT-interpolated power against direct SAM hourly output across a full year for multiple locations and system configurations. Publish the RMSE and max-error under typical operating conditions. (Addresses Finding 6)
7. **Fix month binning in the LUT generator** to use true calendar months rather than fixed 730-hour intervals, matching the Rust consumer's calendar-month query. (Addresses Finding 7)

## References / Citations
- SAM PVWatts v8 Technical Reference, NREL/TP-7A40-80694
- PVPMC cell temperature models: https://pvpmc.sandia.gov/modeling-guide/2-dc-module-iv/cell-temperature/noct-cell-temperature/
- EnergyPlus Engineering Reference, PVWatts model (Section 12.9.1)
- SSC `lib_pvwatts.h:26` — `PVWATTS_INOCT = (45.0 + 273.15)`
- SSC `cmod_pvwattsv5.cpp:191-206` — Array-type NOCT defaults
- EnergyPlus `cmod_pvwattsv1_1ts.cpp:180-184` — Snow albedo adjustment
- EnergyPlus `lib_pvwatts.cpp:117-227` — Fuentes cell temperature iterative model
- EnergyPlus `Photovoltaics.cc:1274-1284` — TRNSYS NOCT-based cell temperature formula
- EnergyPlus `Photovoltaics.cc:1867-1892` — Air mass computation for Sandia model
- OCHRE `PV.py:53-66` — PySAM PVWatts v8 parameterization
- OCHRE `envelope.py:54` — Default albedo = 0.2
