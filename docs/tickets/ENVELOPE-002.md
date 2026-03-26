---
id: ENVELOPE-002
title: Investigate and reconcile solar gain vs OCHRE (beam + diffuse)
kind: investigate
depends_on: []
files_to_touch:
  - crates/hares-physics/src/solar.rs
  - crates/hares-envelope/src/thermal_solver/solar.rs
references:
  - vendors/OCHRE/ochre/utils/envelope.py (line 126)
  - crates/hares-physics/src/solar.rs (diffuse_iam method)
  - tests/python/test_thermal_trace.py (line 375 — "~35% higher than OCHRE")
verification:
  - cargo test -p hares-physics
  - cargo test -p hares-envelope
  - cargo clippy --all-targets -- -D warnings
---

## Background/Context

HARES window solar gains are ~19% higher than OCHRE in the conditioned oracle (winter:
+26.7W mean, +19%). The Python thermal trace test (`test_thermal_trace.py:375`) notes
"~35% higher than OCHRE." The discrepancy has both a diffuse and potentially a beam
component.

### Diffuse path

- **OCHRE**: Applies a blanket `0.854` correction to POA diffuse irradiance *before*
  any IAM, then uses `IAM_diffuse = 1.0`. Net: `diffuse x 0.854`
- **HARES**: Does NOT apply the 0.854 POA correction. Instead uses per-curve
  hemispherical IAMs (Curve A = 0.907, Curve E = 0.855, etc.)
- Delta for Curve A: HARES transmits **6.2% more diffuse solar**

The comment in `solar.rs:512` claims consistency with OCHRE's 0.854, but that's
misleading. The hemispherical IAM values are physically correct per-curve averages;
OCHRE's 0.854 is an empirical correction to pvlib's Perez model output. These are
two different modeling choices.

### Beam path

The 6.2% diffuse delta alone cannot explain a 19-35% total solar excess. Possible
beam-path contributors:
- Different IAM angular response at high incidence angles
- Different SHGC or transmittance values parsed from HPXML
- Different window area
- Different beam/diffuse decomposition from Perez model

Both paths need investigation.

## Work to Do

- [ ] Decompose the solar gain delta into beam vs diffuse contributions:
  - Extract per-step beam and diffuse transmitted solar from HARES observer
  - Compare against OCHRE's `Window Transmitted Solar Gain (W)` column
  - Quantify beam delta and diffuse delta separately
- [ ] Verify window parameters match OCHRE:
  - SHGC, transmittance, area, U-factor from HPXML parsing
  - IAM curve selection (which glazing curve does BEopt_example use?)
- [ ] Research which approach EnergyPlus actually uses for diffuse:
  - Per-curve hemispherical IAM (HARES approach)
  - Blanket POA correction (OCHRE approach)
  - Something else?
- [ ] If OCHRE's 0.854 is more EnergyPlus-accurate:
  - Apply it as a POA correction to `irr.diffuse_w_m2` in `apply_solar_inputs()`
  - Replace per-curve `diffuse_iam()` with 1.0
- [ ] If HARES's per-curve IAM is more physically correct:
  - Document why and accept the diffuse delta as intentional
  - Investigate and fix the beam-path discrepancy instead
- [ ] Fix the misleading comment at `solar.rs:512` regardless of outcome

## Files to Touch

- `crates/hares-physics/src/solar.rs`: Fix misleading comment, potentially adjust diffuse IAM
- `crates/hares-envelope/src/thermal_solver/solar.rs`: Potentially apply POA correction

## Measures of Success

- [ ] Root cause of 19-35% solar excess identified (beam vs diffuse split quantified)
- [ ] Clear documentation of which solar model HARES uses and why
- [ ] If a fix is applied, total window solar matches OCHRE within 5%
- [ ] Misleading comment at `solar.rs:512` corrected

## Verification

- [ ] `cargo test -p hares-physics` passes
- [ ] `cargo test -p hares-envelope` passes
- [ ] `cargo clippy --all-targets -- -D warnings` passes
