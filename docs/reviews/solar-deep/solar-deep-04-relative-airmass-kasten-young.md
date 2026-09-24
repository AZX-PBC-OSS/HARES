# Relative airmass formula (Kasten-Young 1989 vs secant) — check validity range near horizon

**Review ID**: solar-deep-04
**Category**: solar-deep
**Date**: 2026-05-26

## Files Reviewed

- `crates/hares-physics/src/solar.rs:255-279` — `perez_sky_diffuse` (simple secant airmass)
- `crates/hares-physics/src/solar.rs:291-296` — `relative_airmass` (Kasten-Young implementation)
- `crates/hares-physics/src/solar.rs:443` — `perez_tilted_irradiance` (calls `relative_airmass`)
- `crates/hares-physics/src/solar.rs:206` — `clear_sky_irradiance` (calls `relative_airmass`)

## Vendor/Reference Files Consulted

- `vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc:4024-4048` — EnergyPlus Kasten-Young airmass implementation
- `vendors/OCHRE/ochre/utils/envelope.py:145` — OCHRE delegates to `pvlib.atmosphere.get_relative_airmass`

## Findings

### Finding 1: [Severity: medium]

**Description**: Two different airmass formulas are used within the same module — simple secant (`1/cos(z_rad)`) in `perez_sky_diffuse` and Kasten-Young (1989) in `relative_airmass` (called by `perez_tilted_irradiance` and `clear_sky_irradiance`). Both vendor references (EnergyPlus and OCHRE/pvlib) use Kasten-Young consistently throughout their irradiance models.

**Code Location**:
- Simple secant: `solar.rs:260` — `let am = 1.0 / zenith_rad.cos();`
- Kasten-Young: `solar.rs:293-296` — `relative_airmass()` function
- K-Y call site: `solar.rs:443` — `let am = relative_airmass(solar_zenith_deg);`

**Root Cause**: A code comment at `solar.rs:259` states the secant is *"spec-mandated; avoids Kasten-Young refraction correction that is irrelevant to the brightness coefficient delta."* However, no specification cite or literature rationale is given for this claim. The Perez et al. (1990) paper uses optical airmass `m` in the delta calculation (`Δ = m × I_d / I_on`) without prescribing a specific airmass approximation; the standard interpretation in both pvlib and EnergyPlus is Kasten-Young.

**Impact**: At moderate zenith angles the difference is negligible (e.g., at `z=60°`, sec=2.000 and K-Y=1.994, ~0.3% error). At high zenith approaching the Perez fallback limit (`z=87°`), sec=19.1 and K-Y=15.2, a ~25% relative difference. This propagates linearly into the sky brightness delta and then into Perez coefficients `f1` and `f2`. However, absolute irradiance values are small near the horizon (the module already falls back to isotropic at `z>87°` via `PEREZ_ZENITH_LIMIT_DEG`), so the practical energy impact is modest. The inconsistency is primarily a correctness concern — a single site should use either K-Y everywhere or secant everywhere, not both.

---

### Finding 2: [Severity: low]

**Description**: The Kasten-Young formula in `relative_airmass` is parameterised unconventionally in terms of zenith rather than the standard altitude-based form, with the constant `96.07995` derived inline from `6.07995 + 90`. This is algebraically equivalent to the canonical form but makes literature cross-referencing harder than necessary.

**Code Location**: `solar.rs:295`

**Root Cause**: The code expresses the formula as:
```
1.0 / (cos(zenith_rad) + 0.50572 * (96.07995 - zenith_deg)^(-1.6364))
```

The canonical Kasten-Young (1989) form, as preserved in EnergyPlus and ASHRAE HoF 2009 Eq. 16, is:
```
AM = 1 / (sin(altitude_deg) + 0.50572 * (altitude_deg + 6.07995)^(-1.6364))
```

The transformation `altitude_deg = 90 - zenith_deg` yields the zenith-based form, making `96.07995 = 6.07995 + 90`. The two forms produce identical numerical results. EnergyPlus at `WeatherManager.cc:4045` uses the altitude-based form with the explicit constant `6.07995 + SunAltD`, which is directly traceable to the source paper. The HARES form hides this derivation.

**Impact**: No numerical impact. Reduced auditability — a reviewer unfamiliar with the transformation must verify the algebra before confirming correctness.

---

### Finding 3: [Severity: low]

**Description**: The zenith clamp at `89.9°` produces a limit airmass of ~36.4, which is ~1.8% lower than the EnergyPlus clamped return value of `37.07837343`. EnergyPlus uses a `CosZen <= 0.001` threshold (zenith ≈ 89.94°) with a hardcoded plateau value derived from an Excel evaluation at the boundary. HARES simply caps the input and recomputes the formula, producing a slightly lower value.

**Code Location**:
- HARES: `solar.rs:294` — `let z = zenith_deg.min(89.9);`
- EnergyPlus: `WeatherManager.cc:4037` — `if (CosZen <= 0.001) { AirMass = 37.07837343; }`

**Root Cause**: Minor difference in clamping strategy. HARES computes `min(89.9)` then evaluates the formula, while EnergyPlus returns a constant when `cos(zenith) <= 0.001`. The EnergyPlus approach has the slight advantage of avoiding the `powf` computation in the already-clamped regime and providing a directly verifiable plateau value.

**Impact**: Negligible — the ~1.8% airmass difference at extreme zenith translates to a fraction-of-a-Watt difference in irradiance, since both DNI and DHI approach zero at the horizon. The existing test `relative_airmass_high_zenith_is_large_and_finite` (`solar.rs:1505-1515`) correctly asserts finiteness and monotonicity without depending on exact plateau values.

---

### Finding 4: [Severity: low]

**Description**: The Kasten-Young formula uses geometric (un-refracted) zenith from the Spencer (1971) solar position model, but the original K-Y formulation was designed for apparent (refracted) solar position. Both EnergyPlus and pvlib use apparent zenith as input to the airmass calculation.

**Code Location**:
- Solar position: `solar.rs:88-146` — Spencer declination/EOT model computes geometric position
- Airmass call: `solar.rs:206`, `solar.rs:443` — feeds geometric zenith to `relative_airmass`

**Root Cause**: HARES's solar position model (`solar_position`) computes geometric (true) solar position without atmospheric refraction correction. The K-Y formula's constants were derived from tabulated optical airmass values that account for refraction. Passing geometric instead of apparent zenith introduces a small systematic bias that grows near the horizon: at 1° geometric altitude, refraction adds ~0.4°, changing the effective airmass. However, since HARES clamps at `z=89.9°` (altitude 0.1°), and building energy solar gains at such low elevations are effectively zero, this is not a significant practical concern.

**Impact**: The airmass values for low solar altitudes are slightly too low (underestimating optical path length) compared to implementations that feed apparent zenith to K-Y. Energy impact is negligible because contributions below ~5° altitude are dominated by diffuse sky radiation anyway, and the Perez model falls back to isotropic above 87° zenith.

## Summary

- **Total findings**: 4
- **Critical**: 0
- **High**: 0
- **Medium**: 1
- **Low**: 3

## Recommendations

1. **Unify the airmass formula** (Finding 1): Either replace the simple secant in `perez_sky_diffuse` with a call to `relative_airmass`, or document the specific reference justifying the secant choice for the Perez brightness coefficient. OCHRE/pvlib and EnergyPlus both use K-Y for the delta term, suggesting K-Y is the standard expectation. If secant is kept, add a cite to a specific paper or standard.

2. **Document the zenith-to-altitude transformation** (Finding 2): Add a brief comment in `relative_airmass` noting the equivalence: `(96.07995 - zenith_deg) = (6.07995 + altitude_deg) where altitude = 90 - zenith`. This is a one-line documentation fix.

3. **Consider adopting the EnergyPlus plateau constant** (Finding 3): Replace `zenith_deg.min(89.9)` with a threshold comparison against `cos(89.94°) ≈ 0.001` and return the published limit value `37.078` directly. This would align with the vendor reference and eliminate the slightly divergent plateau. Low priority.

4. **Evaluate refraction correction for solar position** (Finding 4): If HARES is ever extended to model twilight or horizon-sensitive phenomena (e.g., glare analysis, photovoltaic tracker cut-in angles), consider adding atmospheric refraction to the `solar_position` function. Not needed for building energy at current scope.

## References / Citations

- Kasten, F. and Young, T. (1989). "Revised optical air mass tables and approximating formula." *Applied Optics* 28:4735–4738.
- ASHRAE Handbook of Fundamentals (2009), Chapter 14, Eq. 16.
- EnergyPlus Engineering Reference, Weather Manager, `ASHRAETauModel` subroutine (`WeatherManager.cc:4024–4048`).
- pvlib-python `atmosphere.get_relative_airmass`: [https://pvlib-python.readthedocs.io/en/stable/reference/generated/pvlib.atmosphere.get_relative_airmass.html](https://pvlib-python.readthedocs.io/en/stable/reference/generated/pvlib.atmosphere.get_relative_airmass.html)
- Perez, R., Ineichen, P., Seals, R., Michalsky, J., and Stewart, R. (1990). "Modeling daylight availability and irradiance components from direct and global irradiance." *Solar Energy* 44(5):271–289.
- Spencer, J.W. (1971). "Fourier series representation of the position of the sun." *Search* 2(5):172.
