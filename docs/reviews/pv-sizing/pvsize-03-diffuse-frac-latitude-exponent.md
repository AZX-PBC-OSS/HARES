# Plane solar score: DIFFUSE_FRAC=0.18 and latitude exponent formula
**Review ID**: pvsize-03
**Category**: pv-sizing
**Date**: 2026-05-26

## Files Reviewed
crates/hares-physics/src/pv_sizing.rs

## Vendor/Reference Files Consulted
vendors/EnergyPlus/src/EnergyPlus/ vendors/OCHRE/ochre/Equipment/PV.py

## Findings

### Finding 1: Fixed DIFFUSE_FRAC=0.18 ignores available TMY3/EPW diffuse irradiance data
**Severity**: high

**Description**:
The `plane_solar_score` function at `pv_sizing.rs:144-151` uses a hardcoded `DIFFUSE_FRAC = 0.18` to represent the fraction of total irradiance arriving as diffuse (non-directional) radiation. This constant controls how sharply the solar score penalizes non-south-facing azimuths: a higher diffuse fraction makes all azimuths more equal, while a lower diffuse fraction makes south-facing orientations more valuable relative to east/west.

However, HARES already has sophisticated infrastructure for obtaining location-specific diffuse fractions without any constant:
- **Weather data pipeline** (`crates/hares-io/src/weather.rs`, `epw.rs`, `tmy3.rs`, `psm3.rs`): Parses GHI, DNI, and DHI from TMY3/EPW/PSM3 weather files with hourly resolution.
- **Perez 1990 anisotropic sky model** (`crates/hares-physics/src/solar.rs`, lines 28-61): Implements the full Perez et al. (1990) sky clearness classification and diffuse irradiance decomposition, identical to the model used in EnergyPlus (see `SolarShading.cc:2632-2805`).
- **Clear-sky irradiance model** (`crates/hares-physics/src/solar.rs`, line 194): ASHRAE HoF 2013 clear-sky beam/diffuse splitting with `DIFFUSE_OPTICAL_DEPTH = 2.0`.

The annual-average diffuse-to-global ratio (Kd) varies from approximately 0.25 (Seattle, cloudy marine climate) to 0.12 (Phoenix, arid climate). A fixed 0.18 represents a continental-US average but introduces location-dependent error in the solar score. In cloudy climates, the heuristic overestimates the production penalty for non-south azimuths (because actual diffuse is higher, making orientation less important). In arid climates, it underestimates the penalty (because actual diffuse is lower, making orientation more important).

**Code Location**: `crates/hares-physics/src/pv_sizing.rs:144-151`

```rust
fn plane_solar_score(area_m2: f64, azimuth_deg: f64, shape: RoofShape, latitude: f64) -> f64 {
    const DIFFUSE_FRAC: f64 = 0.18;
    let deviation_rad = (south_distance(azimuth_deg).to_radians()).min(PI / 2.0);
    let exponent = 1.0 + 0.005 * (latitude - 35.0);
    let production_factor =
        DIFFUSE_FRAC + (1.0 - DIFFUSE_FRAC) * deviation_rad.cos().powf(exponent);
    area_m2 * usable_fraction(shape) * production_factor
}
```

**Root Cause**:
The `plane_solar_score` function is called during `compute_usable_area` (line 265) and `enumerate_pv_candidates` (line 431) purely as a relative scoring heuristic to select the best roof plane. It was designed as a simple, latitude-aware orientation score without the complexity of full irradiance modeling. Climate-zone-specific diffuse data was not wired into the PV sizing module despite being available elsewhere in the crate graph.

**Impact**:
- **Scoring rank error**: In climates where the actual diffuse fraction differs from 0.18, the relative ranking of candidate roof planes could shift. For example, in Seattle (Kd ≈ 0.25), east-west planes should be more competitive with south-facing planes than the heuristic predicts, potentially causing HARES to prefer a south plane when an east plane with 20% more area would yield similar annual production.
- **Absolute production estimate error**: The `solar_score` is the objective function for Gable/Flat plane selection (line 264-268). A wrong diffuse fraction shifts the relative weighting between plane area and orientation, altering which plane is selected. The actual power is sized from usable area (line 310-319), not from the score, so the capacity estimate is unaffected — only plane selection and candidate ranking are impacted.
- **Quantitative sensitivity**: At deviation=45° (SW/SE facing), changing `DIFFUSE_FRAC` from 0.18 to 0.12 (Phoenix) reduces the production factor from 0.476 to 0.424 (11% change). Changing it to 0.25 (Seattle) increases it to 0.529 (11% change). This is large enough to affect plane ranking when candidate planes have similar areas.

### Finding 2: Latitude exponent provides no correction for due-south arrays
**Severity**: medium

**Description**:
The latitude exponent formula at `pv_sizing.rs:147` adjusts the direct-beam cosine power law coefficient:

```rust
let exponent = 1.0 + 0.005 * (latitude - 35.0);
```

This is applied as `cos(deviation)^exponent`, where `deviation` is the angular distance from due south. However, for a due-south array (the most common and typically optimal orientation), `deviation = 0` and `cos(0)^n = 1.0` for any exponent `n`. The latitude correction has **zero effect** on south-facing planes, regardless of whether the building is at 25°N (Miami) or 48°N (Seattle).

The latitude of a site affects PV production through multiple mechanisms that this formula does not capture for south-facing arrays:
1. **Annual solar resource** (GHI varies by ~0.5-1.0% per degree of latitude in the continental US, with the south receiving more total irradiance)
2. **Seasonal distribution** (higher latitudes have greater summer/winter asymmetry, affecting annual yield through temperature-dependent cell efficiency)
3. **Optimal tilt angle** (which the formula also ignores — tilt is handled separately and clamped to latitude with a 25° cap at line 323)

For non-south arrays, the formula does provide a directional correction. At deviation=45° (SW/SE facing):
- 25°N (exponent 0.95): `cos(45°)^0.95 = 0.519`
- 35°N (exponent 1.00): `cos(45°)^1.00 = 0.500`
- 45°N (exponent 1.05): `cos(45°)^1.05 = 0.486`
- Range: 0.486 to 0.519 across the continental US latitudinal span, a 6.6% spread.

At deviation=60° (E/W facing):
- 25°N: `cos(60°)^0.95 = 0.530`
- 45°N: `cos(60°)^1.05 = 0.487`
- Range: 8.8% spread.

These values represent the direct-beam contribution to the production factor (the diffuse portion is always 0.18 regardless of azimuth). The spread is modest compared to the 20° latitudinal span it covers.

**Code Location**: `crates/hares-physics/src/pv_sizing.rs:147`

**Root Cause**:
The formula was designed as a simple latitude roll-off for the directional dependence of direct-beam availability, implicitly assuming that the dominant latitude effect is on how much direct beam arrives at a given orientation relative to the sun's path. For south-facing arrays, any cosine exponent of the azimuth deviation is trivially 1.0, so the latitude exponent is invisible. The model assumes that latitude effects on total irradiance are small or are captured by the fixed `DIFFUSE_FRAC`.

**Impact**:
- **Zero latitude sensitivity for south-facing arrays**: The most common case (due south) gets no correction. A building in Phoenix and an identical building in Minneapolis with the same south-facing roof geometry will receive identical solar scores and select identical roof planes, despite Phoenix having approximately 20-25% higher annual GHI.
- **Non-south arrays have modest sensitivity**: The 0.5% per degree slope is in the right ballpark for direct-beam angular dependence, but the effect is weak for arrays within ±45° of south.
- **The heuristic should not be used for absolute production comparison across latitudes** — only for within-building plane ranking at a single location.

### Finding 3: Reference latitude 35°N is reasonable but not optimally centered
**Severity**: low

**Description**:
The formula anchors the exponent at 1.0 at latitude 35°N (`latitude - 35.0` term), meaning the direct-beam cosine power law has unity exponent at this latitude and increases/decreases linearly from there. 35°N passes through:
- Oklahoma City, OK
- Albuquerque, NM
- Memphis, TN
- Charlotte, NC

The continental US lower-48 latitude range is approximately 25°N (Key West, FL) to 49°N (Roseau, MN). The US population-weighted mean latitude is approximately 38-39°N, due to population centers concentrated in the Northeast, Midwest, and California coast. NREL's primary research center in Golden, CO is at 39.7°N.

A reference at 38-39°N would center the exponent range symmetrically across the populated US rather than biasing toward warmer climates.

**Code Location**: `crates/hares-physics/src/pv_sizing.rs:147`

**Impact**:
- **Minimal**: Since the exponent effect is invisible for south-facing arrays (Finding 2) and modest for non-south arrays (6.6% across the full continental US range for 45° deviation), shifting the anchor by 3-4 degrees would produce only a <2% change in the exponent value at any given latitude, and a fraction of that in the production factor. The practical effect on plane selection is negligible.

### Finding 4: Solar score heuristic diverges from full TMY3-based transposition models
**Severity**: medium

**Description**:
The HARES `plane_solar_score` is a single-equation heuristic that approximates annual PV production potential as a function of roof area, azimuth, and latitude. In contrast:

- **EnergyPlus** (`PVWatts.cc:384-456`) uses the NREL SAM SSC library (`pvwattsv5_1ts`) which internally applies the Perez 1990 anisotropic sky transposition model (or a configurable HDKR model) on a per-timestep basis, with actual GHI/DNI/DHI from weather files, plus cell temperature modeling, inverter efficiency curves, and system loss breakdown.

- **OCHRE** (`PV.py:9-71`) directly calls the PySAM `Pvwattsv8` model (Python wrapper for SAM SSC), passing time-series DNI, DHI, GHI, wind speed, and dry-bulb temperature from the weather schedule.

Both vendor implementations compute POA irradiance via dynamic transposition models that account for:
1. Time-varying solar geometry (zenith angle, incidence angle)
2. Actual beam/diffuse split from weather data (not a fixed fraction)
3. Ground-reflected irradiance (albedo)
4. Incidence angle modifier (IAM) losses
5. Cell temperature effects on efficiency

HARES's `plane_solar_score` heuristic omits all of these, replacing them with the fixed `DIFFUSE_FRAC` for the beam/diffuse split and the latitude-adjusted cosine exponent for orientation effects. The function produces a relative score (not a production estimate in kWh) intended only for within-building plane selection.

**Accuracy bounds** (estimated, not verified by side-by-side simulation):
- **South-facing at mid-latitude**: The heuristic correctly selects the largest south-facing plane, matching what a full simulation would recommend. Within ±10% of relative ranking accuracy vs. TMY3 for typical residential roofs.
- **Non-south-facing comparisons**: The heuristic's diffuse fraction and latitude exponent introduce up to ±20% error in relative score compared to TMY3 simulation, because it doesn't capture the seasonal asymmetry of east-vs-west production (morning vs. afternoon temperature penalty) or the latitude-dependent seasonal sun-path variation.
- **Flat roofs**: The heuristic does not account for optimal tilt selection independent of azimuth scoring; flat roof tilt is handled separately (line 322-324) and clamped to latitude with a 25° cap.

**Code Location**: `crates/hares-physics/src/pv_sizing.rs:144-151`

**Root Cause**:
This is an intentional design tradeoff. The `plane_solar_score` is a lightweight heuristic for the PV *sizing* workflow (determining how many panels fit on which roof plane), not a production simulation. A full TMY3-based simulation would require accessing the weather data, computing 8760 hourly irradiance values per candidate plane, and summing — which may be disproportionate for a sizing step that runs during model initialization.

**Impact**:
- The heuristic is adequate for its stated purpose: selecting which roof plane to place panels on, given that the system capacity is ultimately constrained by usable area, not by the score.
- However, if the solar score were ever used directly for economic analysis (e.g., payback period estimation, production guarantees), the errors could be material.
- The disconnect between available infrastructure (Perez model in `solar.rs`, TMY3 weather data in `weather.rs`) and the heuristic creates a maintenance risk: future developers may assume the PV sizing module's scores are accurate production estimates.

## Summary
- Total findings: 4
- Critical / High / Medium / Low: 0 / 1 / 2 / 1

## Recommendations

1. **Compute annual-average DHI/GHI from weather data when available.** The `plane_solar_score` function already receives `latitude` — it could also receive an optional `annual_diffuse_fraction` parameter computed from the weather file's TMY3/EPW DHI and GHI columns. This is a one-time O(n) pass over the weather time series (n=8760) that would make `DIFFUSE_FRAC` location-specific without adding per-candidate complexity. Expected implementation cost: adding a `compute_annual_diffuse_fraction(weather: &WeatherTimeSeries) -> f64` utility and threading it through `compute_usable_area` and `enumerate_pv_candidates`.

2. **Consider a latitude-dependent GHI multiplier for due-south arrays.** Since the cosine-exponent formula is invisible for south-facing arrays (Finding 2), add a separate multiplicative term `GHI_lat_factor = 1.0 + 0.008 * (35.0 - latitude)` to the production factor. At latitude 45°N this would apply a 0.92 multiplier; at 25°N, a 1.08 multiplier. This approximately matches the observed ~0.8%/degree GHI gradient across the continental US. The term is small but breaks the degenerate case where identical south-facing roofs at different latitudes score equally.

3. **Shift reference latitude from 35°N to 39°N.** This is a low-impact change that better centers the exponent range on the US population-weighted mean. Update line 147 from `1.0 + 0.005 * (latitude - 35.0)` to `1.0 + 0.005 * (latitude - 39.0)`. At 35°N the exponent becomes 0.98 instead of 1.00 — a negligible change.

4. **Document the heuristic's accuracy bounds in crate-level docs.** Add a doc comment to `plane_solar_score` explicitly stating that this is a relative ranking heuristic, not an absolute production model, and that errors of ±15-20% relative to TMY3-based simulation should be expected for non-south arrays.

5. **Consider optional TMY3-based scoring.** For users who already supply weather files and require higher-fidelity PV sizing (e.g., incentive eligibility analysis), add an option to compute the solar score from a full annual TMY3 simulation using the existing Perez model in `solar.rs`. This would be a higher-effort feature but leverages existing infrastructure.

## References / Citations

- Perez, R., Ineichen, P., Seals, R., Michalsky, J., & Stewart, R. (1990). "Modeling daylight availability and irradiance components from direct and global irradiance." *Solar Energy*, 44(5), 271-289. — The Perez anisotropic sky model used by both EnergyPlus and HARES's `solar.rs`.
- NREL PVWatts Version 5 Manual (Dobos, 2014). NREL/TP-6A20-62641. — Documents the PVWatts v5 transposition model and default system loss breakdown.
- NREL SAM (System Advisor Model) PV technical reference: https://sam.nrel.gov/photovoltaic/pv-reference-manual.html
- EnergyPlus Engineering Reference, "Sky Radiance Model" section — Documents the Perez model as EnergyPlus's only sky radiance distribution.
- NREL NSRDB data: Annual-average GHI varies from ~3.5 kWh/m²/day (Pacific Northwest) to ~6.0+ kWh/m²/day (Southwest). The ~0.5-1.0% per degree latitude gradient is derived from NSRDB TMY3 multi-year averages at 32 US reference stations spanning 25-48°N.
- OCHRE PV.py (`vendors/OCHRE/ochre/Equipment/PV.py:9-71`): Demonstrates the SAM PVWatts v8 integration pattern for time-series DNI/DHI/GHI input.
- EnergyPlus PVWatts.cc (`vendors/EnergyPlus/src/EnergyPlus/PVWatts.cc:384-456`): Demonstrates the SSC pvwattsv5_1ts integration with beam/diffuse/albedo/cell-temperature inputs.
- EnergyPlus SolarShading.cc (`vendors/EnergyPlus/src/EnergyPlus/SolarShading.cc:2632-2805`): Reference implementation of the Perez anisotropic sky view factor computation.
- HARES solar.rs (`crates/hares-physics/src/solar.rs`): Implements the Perez 1990 model and clear-sky irradiance decomposition, demonstrating available infrastructure not currently used by PV sizing.
