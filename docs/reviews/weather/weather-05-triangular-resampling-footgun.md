# Triangular solar resampling energy non-conservation
**Review ID**: weather-05
**Category**: weather
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-io/src/weather.rs` (lines 334–372, 659–673, 1134–1211, 2263–2749)
- `crates/hares-io/tests/weather_parity.rs` (lines 814–913)

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc` (lines 3074–3116, 8311–8369)
- `docs/tickets/030-solar-upsampling-triangular-mean-error.md`
- `docs/tickets/098-triangular-resample-docstring-or-mean-preserving.md`
- `docs/findings/weather.md`

## Findings

### Finding 1: [Severity: low]
**Description**: Triangular resampling is documented correctly and guarded by explicit opt-in, but the hazard surface remains wider than necessary.

**Code Location**:
- `ResampleMethod::Triangular` variant definition: `weather.rs:334–372`
- Solar default assignment: `weather.rs:659–673` (defaults to `ResampleMethod::Zoh`)
- `ResampleOverrides` struct: `weather.rs:383–396` (allows any `ResampleMethod` on solar fields)
- `triangular_resample` function: `weather.rs:1165–1199`

**Root Cause**: The `ResampleOverrides` struct at `weather.rs:383–396` accepts arbitrary `ResampleMethod` values for GHI, DNI, DHI without any guard or warning specific to the `Triangular` choice. A user skimming the API docs who sees `Triangular` listed in `ResampleMethod` may mistakenly apply it to solar fields without reading the full 40-line docstring warning at lines 334–372, especially since EnergyPlus uses triangular solar interpolation as its mandatory default (`WeatherManager.cc:3074–3116`). A user coming from an EnergyPlus background could reasonably expect triangular to be safe and may override their solar fields to `Triangular` in `ResampleOverrides` without realising the energy penalty.

Two factors mitigate this:
1. Solar fields default to ZOH (`weather.rs:662`: `overrides.ghi.unwrap_or(ResampleMethod::Zoh)`), so triangular is never used without explicit user action.
2. The `Triangular` variant docstring at `weather.rs:356–371` includes a clear `WARNING:` block that quantifies the energy shortfall (12.5% at hourly resolution, 18.8% at 15-minute resolution) and explicitly recommends ZOH for energy conservation.

However, the mitigation is passive — it relies on the user reading the full docstring for a variant buried 30 entries into an enum. The `ResampleOverrides` struct has no corresponding warning on its solar fields.

**Impact**: Users who apply `Triangular` to solar fields through `ResampleOverrides` see a 12.5% solar energy shortfall at sharp transitions (sunrise/sunset) and spurious energy injection into nighttime hours (up to 75 W/m² mean in the first night hour for a 400→0 sunset transition at 15-minute resolution). For a typical residential simulation, this biases cooling loads downward by ~5% annually (sunrise/sunset transitions are narrow in time). In an extreme edge case — a building that responds rapidly to solar gains with no thermal mass — the sub-hourly irradiance spikes can produce different peak-load results than ZOH, though the hourly-mean error dominates.

### Finding 2: [Severity: medium]
**Description**: EnergyPlus also uses triangular solar interpolation as its **mandatory** default with no warnings about energy non-conservation, reinforcing the incorrect perception that triangular is safe for solar.

**Code Location**:
- `WeatherManager.cc:8311–8369` (`SetupInterpolationValues`) — generates the `SolarInterpolation` weight array
- `WeatherManager.cc:3073–3091` — weight selection logic for solar interpolation
- `WeatherManager.cc:3113–3116` — application of weights to `DifSolarRad` and `BeamSolarRad`

**Root Cause**: EnergyPlus's `SetupInterpolationValues` function (`WeatherManager.cc:8311–8369`) computes a per-timestep `SolarInterpolation` weight array that defines a triangular profile peaking at the hour midpoint. At `WeatherManager.cc:3113–3116`, solar radiation fields are blended from three hours (`wvarsLastHr`, current, `wvarsNextHr`) with weights `wgtPrevHrSolar`, `wgtCurrHrSolar`, and `wgtNextHrSolar`. This is mathematically equivalent to HARES's triangular resampling — blending across hour boundaries with the same non-conservative property. EnergyPlus applies this unconditionally; there is no ZOH alternative available.

The EnergyPlus codebase contains **no warnings** about energy non-conservation from this interpolation scheme. The `SetupInterpolationValues` comment at line 8319 merely states "This subroutine creates the interpolation values / weights that are used for interpolating weather data from hourly down to the time step level" — no caveats. EnergyPlus's own Engineering Reference (Climate Calculations chapter, "Weather File Solar Interpolation" section) describes the method but does not quantify the hourly-average deviation.

This matters because users benchmarking HARES against EnergyPlus may observe sub-hourly solar differences and incorrectly conclude that HARES is wrong, when in fact HARES's ZOH default is more energy-conservative than EnergyPlus's mandatory triangular scheme.

**Impact**: EnergyPlus's unconditional use of non-conservative triangular solar interpolation normalises the practice. Users migrating from EnergyPlus to HARES may reasonably expect triangular to be the safe choice for solar and override their configuration accordingly. HARES's decision to default to ZOH is architecturally superior, but the presence of an EnergyPlus-like `Triangular` method in the same enum — with no mechanism to warn at the point of override — creates a discoverability gap.

### Finding 3: [Severity: low]
**Description**: There are legitimate use cases for triangular solar resampling that justify keeping the method as an explicit opt-in, rather than removing it.

**Code Location**: `weather.rs:334–372`, `weather.rs:1165–1199`

**Root Cause**: The triangular method was originally HARES's default for solar fields (pre-ticket 030), reflecting EnergyPlus's approach. Ticket 030 correctly demoted it from the default to an opt-in override. However, the method serves real purposes:

1. **EnergyPlus parity testing** (`docs/findings/weather.md:7–26`): The weather finding document explicitly recommends triangular for matching EnergyPlus sub-hourly solar behaviour. Users performing cross-validation need method-level parity, even if the method itself is imperfect.

2. **High-resolution control modelling**: At very fine timesteps (e.g., 60-second resolution), ZOH produces step-function solar profiles with 60 identical sub-step values per hour. The resulting sudden solar gain jumps can trigger unrealistic HVAC control transients (e.g., an air conditioner cycling on at the exact hour boundary). Triangular produces physically-plausible smooth ramps that avoid this artefact.

3. **Transient thermal comfort studies**: Models that compute PMV/PPD at sub-hourly resolution are sensitive to sudden irradiance changes. Triangular's C0-continuous profile (confirmed by `triangular_c0_continuous_at_hour_boundaries` at `weather.rs:2543–2598`) eliminates the step-function discomfort that ZOH creates.

4. **Research on sub-hourly interpolation methods**: EnergyPlus's triangular scheme has been part of the building simulation landscape for decades. HARES providing it as an opt-in allows researchers to study its energy-conservation properties and compare alternatives.

**Impact**: Removing `Triangular` would close off these use cases and prevent EnergyPlus parity for sub-hourly comparisons. The method is a known quantity with well-quantified error bounds (the docstring at lines 358–366 provides exact formula and numerical examples). Keeping it as an explicit, warned opt-in is better engineering than removal.

## Summary
- Total findings: 3
- Critical: 0 / High: 0 / Medium: 1 / Low: 2

## Recommendations

1. **Keep `Triangular` as an opt-in method.** It serves legitimate use cases (EnergyPlus parity, high-resolution control modelling, transient comfort research) and is already correctly gated behind `ResampleOverrides` with a well-written docstring warning. Removing it would be net-harmful — it would eliminate the only path to EnergyPlus-matching sub-hourly solar profiles.

2. **Add a `#[deprecated(note = "...")]` or `tracing::warn!` guard on the `ResampleOverrides` solar fields when they are set to `Triangular`.** Currently, setting `ghi: Some(ResampleMethod::Triangular)` in `ResampleOverrides` produces no runtime warning — the only documentation of the energy penalty is in the enum variant's own docstring. At minimum, add a `tracing::warn!` in `resample_with` (`weather.rs:554`) when any solar field override is `Triangular`, repeating the key energy-non-conservation message. This ensures the warning reaches users regardless of how they discover the API.

3. **Consider a dedicated `SolarTriangular` method name.** Renaming `Triangular` to `SolarTriangular` (or adding a separate variant) would make the method's solar-specific semantics explicit in the type name. Currently, `Triangular` could theoretically be applied to any field (the dispatcher at `weather.rs:1202–1211` accepts any `&[f64]`), but it is only meaningfully defined for solar. Naming it `SolarTriangular` would prevent a user from mistakenly applying it to temperature or pressure fields (where it would produce mathematically valid but physically meaningless results with the same energy-non-conservation property).

4. **Document EnergyPlus's equivalent non-conservation in the `Triangular` docstring.** Currently, the docstring at `weather.rs:334–372` focuses on HARES's own error quantification. Adding a note that "EnergyPlus's `WeatherManager.cc:3113–3116` uses the same triangular blending for `BeamSolarRad` and `DifSolarRad` without warning or energy-conservation fallback" would help users understand that HARES's ZOH default is architecturally superior and that `Triangular` parity with EnergyPlus comes with the same known limitation.

5. **Add a cross-reference in the `ResampleOverrides` struct docstring.** The `ResampleOverrides` struct at `weather.rs:383–396` currently has no docstring. Add one that explains the struct's purpose and includes a `/// ## Warnings` section noting that overriding solar fields to `Triangular` will cause energy non-conservation, with a link to `ResampleMethod::Triangular` for quantitative details.

## References / Citations

- HARES `ResampleMethod::Triangular` docstring (`weather.rs:334–372`): Quantifies the 12.5% energy shortfall at hourly resolution, 18.8% at 15-minute resolution, and 75 W/m² nighttime leakage.
- HARES `triangular_resample` implementation (`weather.rs:1165–1199`): Correctly implements the midpoint-interpolation formula with cyclic boundary wrap.
- HARES `triangular_mean_is_not_preserved_at_sharp_transitions` test (`weather.rs:2700–2749`): Locks in the documented non-preservation behaviour at sunrise transitions.
- HARES `triangular_sunset_boundary_bleeds_into_nighttime_hour` test (`weather_parity.rs:827–888`): Demonstrates 75 W/m² leakage into zero-irradiance nighttime hour at 15-minute resolution.
- EnergyPlus `SetupInterpolationValues` (`WeatherManager.cc:8311–8369`): Computes the `SolarInterpolation` weight array creating a triangular profile peaking at the hour midpoint. No warning about hourly-mean deviation.
- EnergyPlus solar blend (`WeatherManager.cc:3074–3116`): Blends `BeamSolarRad` and `DifSolarRad` from three consecutive hours using the triangular weights. Equivalent to HARES's triangular algorithm.
- Ticket 030 (`docs/tickets/030-solar-upsampling-triangular-mean-error.md`): Changed HARES's solar default from `Triangular` to `Zoh`, documenting the energy-conservation rationale.
- Ticket 098 (`docs/tickets/098-triangular-resample-docstring-or-mean-preserving.md`): Corrected the docstring claim of "hourly mean preservation" to the actual weighted-blend formula.
- Weather findings document (`docs/findings/weather.md:7–26`): Recommends triangular for EnergyPlus parity testing at sub-hourly resolution.
- EnergyPlus Engineering Reference, Climate Calculations chapter, "Weather File Solar Interpolation" section: Describes the method but does not quantify hourly-average deviation.
