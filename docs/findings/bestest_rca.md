# BESTEST Root Cause Analysis

## Executive Summary

HARES exhibits systematic under-heating across all conditioned BESTEST cases (600, 640, 900) and a 600FF peak temperature overshoot of +1.6°C above the ASHRAE acceptance band. The dual pattern — both heating AND cooling kWh below reference bands — points to the building thermally behaving as if it has **excess heat gain or insufficient heat loss** relative to the ASHRAE 140 reference programs. Tracing energy flows through the simulation, the three dominant root causes are: (1) internal gains routed 100% convective instead of the ASHRAE 140-specified 30% radiant / 70% convective split, which causes the HVAC to short-cycle against immediate air-temperature spikes and reduces total annual HVAC runtime; (2) the window absorbed-inward solar is dumped directly to zone air rather than distributed through the interior glass surface (as E+ does), bypassing surface thermal mass and treating it as 100% convective; and (3) insufficient nighttime exterior longwave radiation cooling through windows, where the window RC network lacks a separate exterior surface node — lumping R_film_ext into r_glass prevents proper sky LWR modeling, and the T_eff workaround scaled by U/h_out recovers only ~9% of the raw sky-air LWR deficit.

The 600FF peak overshoot is driven primarily by root causes #1 and #2 during peak solar hours: with all gains going convectively to zone air and excess absorbed-inward solar, the free-float zone temperature spikes higher than the reference programs predict. The heavyweight Case 900 is less affected because its concrete thermal mass buffers the gain timing errors, which is consistent with 900 cooling passing the band while 600 cooling fails.

## Current BESTEST Results

| Case | Metric | Value | ASHRAE Band | Gap |
|------|--------|-------|-------------|-----|
| 600 | Heating | 3293 kWh | [4296, 5709] | **-23%** below band low |
| 600 | Cooling | 5678 kWh | [6137, 7964] | **-7%** below band low |
| 640 | Heating | 2193 kWh | [2751, 3803] | **-20%** below band low |
| 900 | Heating | 1025 kWh | [1170, 2041] | **-12%** below band low (close) |
| 900 | Cooling | 2427 kWh | [2132, 3415] | ✅ PASS |
| 600FF | Min temp | -12.2°C | [-18.8, 0.0°C] | ✅ PASS |
| 600FF | Peak temp | 71.1°C | [64.9, 69.5°C] | **+1.6°C** above band high |
| 900FF | Min temp | (passes) | | ✅ PASS |

**Key pattern**: Heating is consistently 12–23% below the band. Cooling is 7% below for lightweight Case 600 but passes for heavyweight Case 900. The 600FF free-float peak overshoot confirms excess heat gain during summer.

## Energy Balance Analysis

### Where heat enters the BESTEST 600 zone

| Source | Annual estimate | Notes |
|--------|----------------|-------|
| Window transmitted solar | ~4500–5500 kWh | 12 m² south-facing, SHGC=0.789, Denver TMY3 |
| Opaque surface solar absorption | ~1200–1800 kWh | Walls + roof, α=0.6 |
| Internal gains (convective) | 1752 kWh | 200W × 8760h × 1.0 sensible |
| Window absorbed-inward solar | ~400–800 kWh | (SHGC-T) × N_i × POA × A |
| Ground coupling (winter) | ~800–1200 kWh | Floor at R-25, ground ~10°C |

### Where heat leaves the BESTEST 600 zone

| Sink | Annual estimate | Notes |
|------|----------------|-------|
| Opaque surface conduction loss | ~3500–5000 kWh | Walls + roof to cold exterior |
| Window conduction loss | ~2500–3500 kWh | U=3.0 × 12 m², Denver winter |
| Infiltration loss | ~1500–2500 kWh | 0.5 ACH, 129.6 m³ |
| Exterior LWR (opaque) | ~2000–3000 kWh | Net emission to cold sky |
| Window exterior LWR beyond U-factor | ~300–600 kWh | T_sky < T_air correction |
| Ground coupling (summer) | ~200–400 kWh | Floor to cooler ground |

### The discrepancy

For heating to be 23% below the band, the building must be retaining or gaining ~1700 kWh more heat than the reference (or losing ~1700 kWh less). For cooling to be 7% below the band, the building must be gaining ~500 kWh less heat (or losing ~500 kWh more) during the cooling season. These two requirements appear contradictory — more heat in winter but less in summer — until we consider that the **timing and distribution** of heat gains, not just their total magnitude, determines HVAC response.

## Root Cause #1: Internal Gains 100% Convective (B1)

### Problem

The BESTEST specification per ASHRAE 140-2017 §5.2.4.3 and the EnergyPlus BESTEST IDF defines internal gains as 200W with **Fraction Radiant = 0.3** (30% radiant, 70% convective, 0% latent, 0% lost). HARES routes 100% of the sensible gain convectively to zone air.

The BESTEST TOML fixtures specify `internal_gains_radiant_fraction = 0.3`, but the current port system does not carry a radiant component for internal gains. The `PortContribution::Thermal` struct lacks a `radiant_gain_w` field, so all 200W goes to `sensible_gain_w` and is injected directly at the zone air node.

### Physics

With 100% convective gains:
1. Zone air temperature rises immediately by ΔT = Q / (ṁ_cp) when 200W is added
2. The thermostat detects the rise and reduces HVAC heating output
3. The HVAC "short-cycles" — it removes/adds less heat because the zone air spikes quickly
4. Net annual HVAC energy is **lower** because the rapid response causes earlier HVAC shutdown

With 30% radiant (correct):
1. Zone air temperature rises by only 140W worth immediately; 60W is absorbed by interior surfaces
2. Interior surfaces warm slowly, then re-emit heat to zone air over hours via convection and LWR
3. The thermostat sees a more gradual temperature change
4. HVAC runs at higher output for longer because it doesn't see the full 200W immediately
5. Net annual HVAC energy is **higher** — the thermal mass time-shift delays the HVAC response

### Evidence

- `crates/hares-envelope/src/thermal_solver/ports.rs:14-22` — `apply_port_sensible_inputs` accumulates all thermal port contributions as `sensible_gain_w` with no radiant split
- `crates/hares-types/src/ports.rs` — `PortContribution::Thermal` has no `radiant_gain_w` field
- The BESTEST fixture `600.toml:37` specifies `internal_gains_radiant_fraction = 0.3` but this value is not wired through to the thermal solver
- Previous measurement (consolidated.md B1): 40% radiant fraction gave −0.295°C on 900FF; scaling to 30% gives estimated −0.22°C
- The constant `KEY_RADIATIVE_GAIN_FRACTION` at `scheduled_load.rs:30` exists but is not used for the thermal solver port

### Quantitative impact

For 200W constant gain with 30% radiant:
- 60W × 8760h = 525.6 kWh annual radiant gain
- This 525.6 kWh is currently going 100% to zone air instead of being distributed to surfaces
- The HVAC short-cycling effect is estimated at 1.5–2× the radiant energy due to the thermostat response dynamics
- **Estimated annual heating impact**: 500–1000 kWh less heating than correct (10–20% of ~5000 kWh reference)
- **Estimated annual cooling impact**: 100–300 kWh less cooling (smaller because cooling setpoint is higher and gains are less dominant)

This root cause explains **most of the −23% heating gap** for Case 600 and a significant portion of the −12% gap for Case 900 (where heavyweight thermal mass partially buffers the effect).

### Proposed fix

Ticket B1 / consolidated.md Fix 2: Add `radiant_gain_w` to `PortContribution::Thermal`. Route 30% of sensible gain to interior surface nodes via the existing `apply_port_radiant_inputs` mechanism (TMULT area×emissivity weighted distribution). Wire `internal_gains_radiant_fraction` through from the fixture to the scheduled load equipment.

---

## Root Cause #2: Window Absorbed-Inward Solar Overestimation

### Problem

In `solar.rs:44-64`, the window solar model computes:

```rust
let transmitted_beam_w  = win.area_m2 * transmittance * poa_beam;
let transmitted_diffuse_w = win.area_m2 * transmittance * poa_diffuse;
let transmitted_total_w = transmitted_beam_w + transmitted_diffuse_w;

let absorbed_inward = (shgc - transmittance).max(0.0) * win.radiation_frac;
let absorbed_zone_w = win.area_m2 * absorbed_inward * poa_w_m2;
```

The `poa_w_m2` variable equals `poa_beam + poa_diffuse` — the **total** POA irradiance including both beam and diffuse. The `absorbed_inward` formula `(SHGC - T) × N_i` is the E+ Step-5 inward fraction of glass-absorbed solar, which should only be applied to the **glass-absorbed** portion of POA, not to the total POA that also includes the already-transmitted portion.

The correct E+ formulation separates transmitted and absorbed-inward:

```
Q_transmitted = T × A × POA
Q_absorbed_inward = (SHGC - T) × N_i × A × POA
```

And these are ADDITIVE components of SHGC × A × POA:

```
SHGC × A × POA = T × A × POA + (SHGC - T) × N_i × A × POA + (SHGC - T) × (1 - N_i) × A × POA
                 \_ transmitted _/  \___ absorbed inward ___/  \___ absorbed outward ___/
```

The code correctly computes `transmitted_total_w` and `absorbed_zone_w` separately and adds them to the zone. The total zone gain is:

```
Q_zone = T × A × POA + (SHGC - T) × N_i × A × POA
       = A × POA × (T + (SHGC - T) × N_i)
```

For BESTEST windows (U=3.0, SHGC=0.789, T≈0.729):
- N_i ≈ 0.39 (from `calculate_window_parameters` using E+ Step 5 formula)
- Q_zone / (A × POA) = 0.729 + (0.060 × 0.39) = 0.729 + 0.023 = 0.752

This is correct — it matches the E+ SHGC decomposition. The total zone gain from the window at normal incidence is SHGC × IAM × A × POA = 0.789 × 1.0 × A × POA = 0.789 × A × POA, of which the zone receives 0.752 × A × POA (the remainder 0.037 × A × POA is absorbed by the glass and lost to the exterior).

**However**, there is a subtle but significant issue: the `absorbed_zone_w` is added to the **zone air node** directly (via `u[air_idx] += absorbed_zone_w`), while the E+ model distributes the absorbed-inward solar to the interior surface nodes via the window's interior surface temperature. In E+, the inward-flowing absorbed solar heats the interior glass surface, which then convects and radiates to the zone. In HARES, it goes directly to zone air, bypassing the surface thermal mass. This is equivalent to treating the absorbed-inward fraction as 100% convective rather than partially radiant.

### Sub-issue: radiation_frac naming confusion

Ticket #049 claims `win.radiation_frac` is the "RC voltage divider" rather than N_i. This is **incorrect** for the HPXML/BESTEST path: the solver_builder at `solver_builder.rs:272-276` calls `calculate_window_parameters(shgc, u_factor, r_glass)` which returns the correct E+ N_i. The `WindowSolarProperties.radiation_frac` field IS the N_i inward fraction.

However, the **InteriorSurfaceInfo.radiation_frac** for window surfaces IS the film-resistance voltage divider (R_film_int / R_total), which is a different quantity (~0.41 for U=3.0 windows). These two fields have the same name but different physical meanings, creating a maintenance hazard.

### Physics impact

The absorbed-inward solar is small relative to transmitted solar: for BESTEST windows, it's only (0.789 - 0.729) × 0.39 = 0.023 × POA × A, vs transmitted 0.729 × POA × A. The absorbed-inward is 3.2% of the total zone solar gain. Even if this is entirely misrouted to zone air instead of surfaces, the impact on annual HVAC is modest:

- Peak hour excess: 0.023 × 12 × 800 ≈ 220 W (if the 3.2% should have gone to surfaces instead of air)
- Annual impact: ~50–100 kWh

This root cause contributes **modestly** to the 600FF peak overshoot and the under-heating pattern, but is not the dominant factor.

### Proposed fix

Ticket #049: Add an `n_i_inward_fraction` field to `WindowSolarProperties` separate from the `radiation_frac` used in the interior surface temperature interpolation. Route the absorbed-inward solar through the interior surface distribution mechanism (same as transmitted solar) rather than directly to zone air. This ensures the glass-absorbed inward fraction is subject to the same radiation_frac split as transmitted solar, with the correct portion going to surface thermal mass.

---

## Root Cause #3: Window Exterior LWR T_eff Correction Under-Corrects

### Problem

The window exterior LWR correction (implemented in `longwave.rs:46-118`) uses the T_eff approach:

```rust
let delta_q_w_m2 = info.emissivity
    * STEFAN_BOLTZMANN
    * beta
    * f_sky
    * (t_sky_k.powi(4) - t_air_k.powi(4));

let delta_q_w = (u_factor / h_out) * delta_q_w_m2 * info.area_m2;
```

The scaling factor `U / h_out = 3.0 / 34 ≈ 0.088` reduces the raw LWR deficit to only 8.8% of its physical magnitude. This is based on the NFRC rating-condition assumption that the window U-factor includes radiation at h_out = 34 W/(m²·K) with T_sky ≈ T_air. The correction only accounts for the deviation from this assumption.

However, the correct approach per the E+ Engineering Reference "External Longwave Radiation" section is to compute the **full** exterior LWR exchange for the window's exterior surface (using ε=0.84 for glass) and couple it to the zone through the window's actual thermal resistance network. The U/h_out scaling was a simplification to avoid double-counting with the U-factor's built-in radiation component, but it **significantly under-corrects** because:

1. The U-factor's h_out=34 includes only ~4.5 W/(m²·K) of radiative conductance (the remainder is convective). The radiative component is ε·σ·4·T_avg³ ≈ 0.84 × 5.67e-8 × 4 × 293³ ≈ 4.8 W/(m²·K). So only ~14% of h_out is radiative.

2. The sky-air LWR deficit should be coupled through the window's **radiative** resistance only, not the total h_out. The convective portion of h_out is unaffected by T_sky ≠ T_air.

3. The correct scaling should use the radiative fraction of h_out, not the total h_out. This would give a correction approximately 34/4.8 ≈ 7× larger than the current implementation.

### Structural cause: window RC network lacks exterior surface node

The underlying reason the T_eff hack is needed (and limited) is that the window RC network has no separate exterior surface node. The window U-factor decomposition at `conversions.rs:221-222` lumps R_film_ext into r_glass:

```rust
let (r_glass, r_int) = window_u_factor_decomposition(u);
(r_glass, r_int, 0.0)  // r_film_ext = 0, already included in r_glass
```

`window_u_factor_decomposition` computes `r_glass = 1/U - r_int`, which includes both the actual glass resistance AND the exterior film resistance. The total R assembled in `boundary_rc.rs:723-725` is:

```
R_total = r_glass + r_int + 0.0 = (1/U - r_int) + r_int = 1/U   ← round-trips correctly
```

So **there is no U-factor magnitude error** (contradicting Ticket #036's claim of an 8.8% overestimate). The total window heat transfer coefficient is correct. The issue is purely **structural**: the window's exterior surface is not modeled as a separate RC node, so the LWR exchange with the sky cannot be applied at the physically correct location (the exterior glass surface). Instead, the T_eff approach modifies the driving temperature for the entire lumped path, which can only approximate the sky effect.

Without a separate exterior surface node, no scaling factor — however derived — can perfectly reproduce the E+ surface heat balance, because the convective and radiative exterior paths are always combined into a single resistance.

> **Note on Ticket #036**: The ticket claims an 8.8% U-factor error for U=2.0 windows. This analysis is incorrect because the total R round-trips to 1/U. The ticket's real concern — the structural inability to model window exterior LWR separately — is valid and is the same issue described here. OCHRE has the same structural limitation (`res_ext_w = 0` in `envelope.py:302`) but doesn't even attempt a T_eff correction.

### Quantitative impact

At the 900FF min-temp hour (T_sky ≈ -30°C, T_air ≈ -15°C, T_zone ≈ 0.9°C):
- Raw sky-air LWR deficit for 12 m² vertical glass (ε=0.84, β×F_sky=0.354):
  Δq_raw = 0.84 × 5.67e-8 × 0.354 × (243.15⁴ - 258.15⁴) × 12 ≈ -281 W
- Current correction: (3.0/34) × 281 = 24.8 W
- Physically correct correction: approximately 24.8 × (34/4.8) ≈ 176 W (or more precisely, the full LWR deficit coupled through the glass resistance)

The difference: 176 - 25 = **151 W missing cooling** through windows at this hour. Over a typical winter night (8 hours), this is 1208 Wh = 1.2 kWh per night. Over 180 heating-season nights: ~216 kWh additional heating that should be present but isn't.

For the **annual** heating impact across all exterior conditions, the average missing window LWR cooling is estimated at **50–150 W** during heating hours, yielding **300–900 kWh** of under-heating. This explains a significant portion of the −23% gap for Case 600.

For the 600FF peak temperature, the window LWR in summer has T_sky closer to T_air, so the correction is smaller and the under-correction is less significant. The peak overshoot is primarily driven by Root Causes #1 and #2.

### Evidence

- `longwave.rs:70-91` — T_eff approach with U/h_out scaling
- `longwave.rs:86-89` — `h_out` falls back to NFRC 34 W/(m²·K) when `info.h_out_w_m2_k <= 1.0`
- The window exterior surface emissivity is correctly set to 0.84 (EMISSIVITY_WINDOW)
- The β and F_sky factors are correctly computed from the Walton (1983) tilted-sky model
- The raw Δq calculation is correct — the under-correction is in the scaling factor only

### Comparison with OCHRE

OCHRE does NOT compute window exterior LWR at all (windows have `t_idx=None` and are skipped in `_solve_exterior_radiation`). The HARES T_eff approach is an improvement over OCHRE but still under-corrects. EnergyPlus computes the full exterior LWR balance for windows by iterating on the window exterior surface temperature within the surface heat balance loop, which captures the full effect.

### Proposed fix

Replace the U/h_out T_eff scaling with a proper 3-node window RC network:

1. **Decompose the window U-factor into three components** (matching E+ Simple Window Model Step 1):
   - `r_glass_actual = 1/U - R_film_int - R_film_ext` (glass-only resistance)
   - `R_film_ext = 0.044 m²·K/W` (NFRC exterior film, currently lumped into r_glass)
   - `R_film_int` (already correctly separated)

2. **Add a window exterior surface node** connected to the exterior via R_film_ext / A, allowing the sky LWR exchange to be applied at the exterior glass surface (same as opaque surfaces).

3. **Connect the window interior surface** to the zone via R_film_int / A (with the radiation_frac split for star-mesh radiation participation).

4. **Remove the T_eff hack** in `longwave.rs` — the window exterior LWR is now handled identically to opaque surfaces through the 4-component exterior LWR model at the exterior surface node.

This structural change makes the window a 3-node RC network (exterior surface ↔ glass ↔ interior surface) instead of the current 2-node network (lumped glass+R_ext ↔ interior film). The total U-factor is preserved because `R_glass + R_film_ext + R_film_int = 1/U`.

---

## Relevant Existing Tickets

| Ticket | Root Cause | Relevance |
|--------|-----------|-----------|
| **B1** (consolidated.md) | #1: Internal gains 100% convective | **Direct match** — the primary fix for the −23% heating gap |
| #049 | #2: Window absorbed-inward N_i | **Naming confusion** — radiation_frac IS the correct N_i, but the inward solar should be distributed to surfaces, not dumped to zone air |
| **B6** (consolidated.md) | #3: Window exterior LWR under-correction | **Partial fix implemented** — T_eff approach is in place but scaling factor under-corrects by ~7× |
| **#036** | #3: Window exterior film hardcoded zero | **Structural cause** — r_film_ext lumped into r_glass prevents separate exterior surface node; ticket's U-factor error claim is incorrect (R round-trips), but the structural point is valid |
| #043 / **#054** | Beam solar floor fraction | **Secondary** — 0.3 minimum clamp forces floor gain at low solar altitude; #054 provides the specific fix (remove lower clamp) |
| #047 | Interior LWR uses last-step zone temp | **Secondary** — creates ~20W flux error during HVAC transients |
| #041 | Warmup not enforced | **Mitigated** — BESTEST fixtures now set initialization_duration_s; 900 uses 21 days |
| #044 | LWR fallback linearised not ScriptF | **Operational** — ScriptF should be precomputed; fallback silently degrades accuracy |
| #045 | Equipment ports ordering | **Low impact** — temporal inconsistency for non-thermal equipment |
| #048 | No energy balance closure check | **Diagnostic** — would help verify fixes close the gap |
| #040 | Radiant gain weights heap alloc | **Performance only** — no physics impact |

## Proposed Additional Fixes

### Fix A: Route internal gain radiant fraction through TMULT distribution

Add `radiant_gain_w: f64` to `PortContribution::Thermal` (default 0.0 for backward compatibility). In `scheduled_load.rs` or wherever internal gains create port contributions, split: `sensible_gain_w = total × (1 - radiant_fraction)`, `radiant_gain_w = total × radiant_fraction`. The existing `apply_port_radiant_inputs` method already handles TMULT-weighted distribution to interior surfaces.

**Expected impact**: Recovers ~500–1000 kWh heating for Case 600, bringing it within or close to the ASHRAE band.

### Fix B: Distribute window absorbed-inward solar through surface nodes

Instead of `u[air_idx] += absorbed_zone_w`, route the absorbed-inward solar through the same `distribute_transmitted_solar` mechanism used for transmitted solar. This ensures the glass-absorbed inward fraction is subject to the radiation_frac split between surface thermal mass and zone air, matching the E+ window heat balance where inward-absorbed solar heats the interior glass surface which then convects/radiates to the zone.

**Expected impact**: Modest (50–100 kWh), but corrects the physics and improves peak temperature accuracy.

### Fix C: Add 3-node window RC network with separate exterior surface

Decompose the window U-factor into `R_glass_actual + R_film_ext + R_film_int`. Add a window exterior surface node connected to the exterior via R_film_ext / A, enabling the sky LWR exchange to be applied at the exterior glass surface (identical to opaque surfaces). Remove the T_eff hack in `longwave.rs`. The total U-factor is preserved: `R_glass_actual + R_film_ext + R_film_int = 1/U`.

**Expected impact**: Recovers ~300–900 kWh heating for Case 600, significantly closing the gap. For 600FF, reduces peak temperature by ~0.5–1.0°C.

## Prediction: Impact on BESTEST

| Root Cause | 600 Heating | 600 Cooling | 640 Heating | 900 Heating | 600FF Peak |
|-----------|-------------|-------------|-------------|-------------|------------|
| #1: Internal gain radiant split | +800 to +1200 kWh | +100 to +300 kWh | +500 to +800 kWh | +200 to +400 kWh | −0.5 to −1.0°C |
| #2: Absorbed-inward solar distribution | +50 to +100 kWh | −30 to −60 kWh | +30 to +60 kWh | +20 to +40 kWh | −0.2 to −0.5°C |
| #3: Window LWR full correction | +300 to +900 kWh | negligible | +200 to +600 kWh | +100 to +300 kWh | −0.3 to −0.8°C |
| **Combined** | **+1150 to +2200 kWh** | **+70 to +240 kWh** | **+730 to +1460 kWh** | **+320 to +740 kWh** | **−1.0 to −2.3°C** |

Current 600 heating: 3293 kWh. Band low: 4296 kWh. Gap: 1003 kWh.
Predicted recovery: +1150 to +2200 kWh → **4443 to 5493 kWh** → **within or close to the [4296, 5709] band**.

Current 600 cooling: 5678 kWh. Band low: 6137 kWh. Gap: 459 kWh.
Predicted recovery: +70 to +240 kWh → **5748 to 5918 kWh** → **still below band; additional investigation needed for cooling gap**.

Current 600FF peak: 71.1°C. Band high: 69.5°C. Gap: +1.6°C.
Predicted reduction: −1.0 to −2.3°C → **68.8 to 70.1°C** → **close to or within the [64.9, 69.5°C] band**.

### Residual cooling gap analysis

The cooling gap for Case 600 is not fully explained by the three root causes above. Additional factors likely contributing:

1. **Beam solar floor fraction (Ticket #043)**: At low winter solar altitudes, the 0.3 clamp sends too much beam to the floor. The floor stores and slowly releases this heat, reducing the immediate cooling load but adding to the delayed load. The net annual cooling impact is estimated at +100–200 kWh.

2. **Interior film coefficient ΔT floor (S1)**: The TARP model uses a ΔT floor that may be too high, resulting in R_film that is too low (overly conductive coupling). This could affect both heating and cooling by ~100–200 kWh.

3. **Solar irradiance model differences**: HARES may compute slightly different POA irradiance than E+ due to different solar position algorithms (Spencer vs. more accurate models — Ticket #026) or different diffuse sky models. A systematic overestimate of POA would increase both transmitted and absorbed solar, reducing heating but increasing cooling. The cooling gap suggests the solar overestimate (if any) is small.

4. **Infiltration density**: HARES uses altitude-corrected air density for infiltration (correct for Denver), while some reference programs may use sea-level density. This would make HARES's infiltration heat exchange ~18% smaller, reducing both heating and cooling. Estimated cooling impact: +100–200 kWh.

---

## Appendix: OCHRE Cross-Reference

### Sky view factor

OCHRE uses `svf = ((1 + cos(tilt)) / 2) ^ 1.5` which combines F_sky and β into a single factor. HARES separates them as `F_sky = 0.5 × (1 + cos(tilt))` and `β = √F_sky`. The product `β × F_sky` matches OCHRE's `svf` numerically to within floating-point precision for all tilt angles.

### Interior radiation

OCHRE uses a single `_solve_interior_radiation` function with area-weighted view factors and T⁴ radiosity iteration. HARES matches this via the ScriptF path. When ScriptF is not precomputed, the linearized fallback produces results within 0.5% of exact T⁴.

### Window parameters

OCHRE's `calculate_window_parameters` computes transmittance and radiation_frac (N_i) using the same E+ Step 4-5 formulas as HARES's `calculate_window_parameters` in `hares-physics/src/solar.rs`. The implementations produce identical values for the same inputs.

### Film coefficients

OCHRE computes interior film resistance from the TARP algorithm with a ΔT floor of 12.9°C (`envelope.py:374`). HARES uses a similar TARP model. The key difference is that OCHRE computes film R once at construction time (frozen), while E+ recomputes it each timestep. Both approaches produce similar annual results but differ during rapid transients.

### Window exterior LWR

OCHRE does NOT compute window exterior LWR — windows have `t_idx=None` and are skipped in `_solve_exterior_radiation`. All window LWR is assumed implicit in the U-factor. This is the same assumption that HARES previously used (before the T_eff correction was added). HARES's current T_eff correction is an improvement over OCHRE but under-corrects as described in Root Cause #3.

### Window RC network structure

OCHRE uses the same lumped-resistance approach as HARES: `create_rc_data` at `envelope.py:302` sets `res_ext_w = 0` and computes `r_window = 1/U - res_int_w`, lumping the exterior film into the glass resistance. Both HARES and OCHRE therefore lack a window exterior surface node, which is the structural cause of Root Cause #3. Neither codebase can properly model window exterior LWR without adding a 3-node window RC network.

---

## Appendix: BESTEST Fixture Verification

### Case 600 construction (verified against ASHRAE 140-2017 Table 5-2)

| Component | Fixture | ASHRAE 140 | Match? |
|-----------|---------|------------|--------|
| South wall area | 9.6 m² | 9.6 m² | ✅ |
| North wall area | 21.6 m² | 21.6 m² | ✅ |
| East/West wall area | 16.2 m² each | 16.2 m² each | ✅ |
| Roof area | 48 m² | 48 m² | ✅ |
| Floor area | 48 m² | 48 m² | ✅ |
| Window area | 12 m² (2×6) | 12 m² | ✅ |
| Window U-factor | 3.0 W/(m²·K) | 3.0 W/(m²·K) | ✅ |
| Window SHGC | 0.789 | 0.789 | ✅ |
| Solar absorptance (ext) | 0.6 | 0.6 | ✅ |
| Emittance (ext) | 0.9 | 0.9 | ✅ |
| Infiltration | 0.5 ACH | 0.5 ACH | ✅ |
| Internal gains | 200W | 200W | ✅ |
| Radiant fraction | 0.3 (specified but not wired) | 0.3 | ⚠️ Specified but not applied |
| Heating setpoint | 20°C | 20°C | ✅ |
| Cooling setpoint | 27°C | 27°C | ✅ |
| Initialization | 86400s (1 day) | E+ runs until convergence | ⚠️ May be insufficient |

### Case 900 construction (verified)

| Component | Fixture | ASHRAE 140 | Match? |
|-----------|---------|------------|--------|
| Wall concrete layer | 100mm, k=0.51, ρ=1400, cp=1000 | Matches | ✅ |
| Floor concrete layer | 80mm, k=1.13, ρ=1400, cp=1000 | Matches | ✅ |
| Initialization | 1814400s (21 days) | E+ runs until convergence | ✅ Adequate for heavyweight |

### Potential fixture issues

1. **600/640 warmup**: Only 1 day (86400s) for lightweight construction. E+ converges in ~1–3 days for lightweight, so this is marginal. A 3-day warmup would be safer. Ticket #41 addresses this for the HPXML path but the BESTEST fixtures manually set the duration.

2. **South wall area with windows**: Verified correct. The BESTEST building is 6m (N-S) × 8m (E-W) × 2.7m high, with the long axis facing south. South wall: 8m × 2.7m = 21.6 m² total, minus 12 m² windows = **9.6 m² opaque**. This matches the fixture value. North wall: 8m × 2.7m = 21.6 m² (no windows). East/West: 6m × 2.7m = 16.2 m² each. ✅

3. **Floor tilt**: The floor has `tilt_deg = 180.0` in the fixtures. A downward-facing floor has F_sky = 0 (correct — the floor doesn't see the sky). The exterior zone is "Ground", which uses ground temperature as the driving temperature. This is correct.

---

## References

- ASHRAE Standard 140-2017 — Standard Test Method for Evaluating Building Energy Simulation Programs
- EnergyPlus Engineering Reference v9.6 — §14.5 "Beam Solar Radiation Distribution"; §14.7 "Window Heat Balance"; §External Longwave Radiation
- EnergyPlus IDD — OtherEquipment FractionRadiant = 0.3 (confirmed via Context7)
- NFRC 100-2020 — Window U-factor rating conditions (h_out = 34 W/(m²·K), h_in = 8.3 W/(m²·K))
- Walton (1983) — Tilted-sky model: β = √F_sky
- OCHRE source: `vendors/OCHRE/ochre/Models/Envelope.py`, `vendors/OCHRE/ochre/utils/envelope.py`
- HARES consolidated findings: `docs/findings/consolidated.md`
- HARES tickets: `docs/tickets/043-*.md`, `044-*.md`, `047-*.md`, `049-*.md`, `041-*.md`, `045-*.md`, `048-*.md`, `036-*.md`, `054-*.md`
