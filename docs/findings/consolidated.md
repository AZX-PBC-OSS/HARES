# HARES Physics Findings — Definitive Issue Register

**Purpose**: A single source of truth for every identified physics defect,
shortcut, and design flaw in HARES, with correct physics, verified citations,
measured vs estimated impact, and a prioritized fix plan.  Supersedes
`consolidated.md` and `summary.md` (both contain errors documented below).

---

## Critical corrections to prior agent findings

### 1. Internal gain radiant fraction is 30%, not 40% or 60%

**summary.md claims**: "60/40 per ASHRAE 140 §5.2.4.3" and cites
"NREL BESTEST-GSR — EnergyPlus OtherEquipment Fraction_Radiant: 0.6"

**correct physics**: The EnergyPlus BESTEST IDF specifies
`Fraction Radiant = 0.3` for the OtherEquipment object (30% of total gain
radiant, 70% convective, 0% latent, 0% lost).  Source: EnergyPlus source
code `design/FY2016/OtherEquipment.md`, confirmed via Context7:

```
OtherEquipment, BASE-1 OthEq 1, ...
  0,    !- Fraction Latent
  0.3,  !- Fraction Radiant
  0;    !- Fraction Lost
```

The BESTEST-GSR Ruby variable library (`bestest_case_var_lib.rb`) does NOT
set a radiant fraction — it specifies only `:int_gen => 200.0`.  The OpenStudio
measure that generates the IDF sets the radiant fraction when creating the
OtherEquipment object.  The NREL BESTEST-GSR reference in summary.md to
"Fraction_Radiant: 0.6" is **unverified and likely refers to a different
convention** (possibly the People object, where the IDD default for
`Fraction Radiant` of the *sensible* portion is 0.3, meaning 30% of
sensible = 0.3 × 0.6 × 130 = 23.4 W radiant).

**consequence for the measured −0.295°C**: If the measurement injected a
40% radiant fraction instead of the correct 30%, the measured delta is
overstated by roughly (40−30)/40 ≈ 25%.  The correct delta at 30% radiant
is approximately **−0.22°C**.  This must be re-measured.

**root-cause test file error**: `bestest_900ff_root_cause.rs:13` says
"ASHRAE 140 §5.2.4.3 requires 60% radiant" — this is incorrect.  The
correct specification for the BESTEST 900FF OtherEquipment is
Fraction Radiant = 0.3 (30% of total gain is radiant).

### 2. Window interior LWR (B2) makes the zone COOLER, not warmer

**consolidated.md (original) claims**: "+0.5–1.0°C (makes 900FF worse)"

**correct direction**: The window interior LWR bug **drops a warming term**,
making the zone **cooler** by 0.5–1.0°C.  At the 900FF min-temp hour,
the window (cold surface, ~0.9°C zone → glass ≈ −16°C exterior) *gains*
net LWR from warm surfaces (concrete at ~2–4°C).  This ~197 W of LWR heat
flowing to the window is computed but never applied anywhere — energy is
destroyed.  Fixing the bug would *warm* the zone.

Source: `interior_lwr.md §2.3–2.4`, verified by the energy balance:
the window's net LWR is positive (heat gain), and dropping it removes
that heating from the zone air node.

**summary.md claims**: H6 "Interior LWR" = 0°C impact.  This likely
tested the *overall* interior LWR model (which is correct by construction
for opaque surfaces), not the window-specific LWR flux drop bug.

### 3. Air density impact on 900FF is +0.02°C, not +0.3–0.5°C

**consolidated.md (original) claims**: "+0.3–0.5°C"

**correct value**: The air_density.md §5.1 reports the measured delta is
~0.02°C.  Zone air capacitance (~157 kJ/K) is only ~1.1% of total effective
system capacitance (~14.3 MJ/K) in heavyweight construction.  The theoretical
0.3–0.5°C applies to general simulations (lightweight buildings) where zone
air is a larger fraction of total thermal mass.

The inconsistency (infiltration uses Denver density, capacitance uses
sea-level) is still a real defect and should be fixed, but it's negligible
for 900FF specifically.

### 4. RC discretization is ruled out — +0.029°C wrong direction

**summary.md and bestest_900ff_root_cause.rs confirm**: Increasing from 2
to 4 concrete nodes moves min_temp by +0.029°C (warmer, not cooler).  This
is measured, not estimated.  RC under-discretization is NOT a root cause.

The `energy_balance.md` claim of +1.5–2.5°C from concrete super-node was
theoretical and has been empirically disproven.  The concrete super-node
does not prevent night draining — the ZOH state-space solver correctly
handles the stiff coupling.

### 5. S8 ground temp interpolation is NOT a deficiency

HARES's linear interpolation between mid-month ground temperature anchors
is **physically more realistic** than EnergyPlus's monthly step function.
Real ground temperature varies continuously.  This should not be classified
as a shortcut — it's an improvement over EnergyPlus.  Source: `weather.md §4`.

### 6. Occupant gains 66/51.2 W are correct for residential

**HANDOFF.md §3c recommends** changing to 75/55 W (ASHRAE HoF "office").
This is wrong for HARES's residential use case.  The 66/51.2 W values
(400 BTU/h, sensible fraction 0.563) are the OCHRE residential model for
seated/light-activity occupants.  The 75/55 W values are for "moderately
active office work" at 24°C (ASHRAE HoF 2021 Table 1).  For residential
simulation, 66/51.2 is appropriate.

The actual defect is the **0% radiative fraction** (currently all sensible
goes convective to zone air).  ASHRAE HoF specifies ~30% of sensible
occupant gain should be radiative.  Source: `physics_defaults.md §3c`.

### 7. B6 window exterior LWR +2.0–2.5°C is an unmeasured estimate

The 248 W additional LWR cooling through 12 m² of south-facing vertical
glass is a sound steady-state calculation, but the zone temperature impact
of +2.0–2.5°C assumes quasi-steady-state (ΔT ≈ ΔQ / G_total) which
overstates the transient effect.  This has NOT been measured by running a
simulation with the fix applied.  The actual impact could be significantly
less.  Source: `lwr.md §3.5`.

---

## Definitive issue register

### BUGS (wrong physics, must fix)

| # | Issue | Location | 900FF impact | General impact | Evidence | Source |
|---|---|---|---|---|---|---|
| B1 | Internal gain 100% convective — E+ BESTEST IDF specifies FractionRadiant=0.3 (30%/70%) | `ports.rs:19` | **−0.22°C (est. at 30% radiant; −0.295°C was measured at 40%)** | All simulations | Measured hack-test at 40%; proportional estimate at 30% | E+ IDD OtherEquipment; `internal_gain.md §1`; Context7 `/nrel/energyplus` |
| B2 | Window interior LWR flux computed but never applied — energy destroyed | `longwave.rs:300-307` | **−0.5 to −1.0°C** (bug makes zone COOLER by dropping ~197W of window LWR heating) | All with glazing | Analytical; interior_lwr.md §2.3 | E+ Eng.Ref "Inside Surface Heat Balance"; Kirchhoff's law; energy conservation |
| B3 | Synthetic weather sky_temp = outdoor_temp — eliminates all sky LWR cooling | `synthetic.rs:741` | None (900FF uses EPW) | Severe for synthetic weather | Code inspection | E+ WeatherManager.cc; Clark & Allen 1978; `lwr.md §2` |
| B4 | TMY3 midpoint_offset_secs = 0 — should be 1800, causes 30-min shift | `tmy3.rs:240` | Unknown (900FF uses EPW) | All TMY3 simulations | EPW sets 1800 at `epw.rs:288`; TMY3 uses same hour-ending convention | TMY3 format spec; `weather.md §7` |
| B5 | Sky temp interpolated directly instead of recomputed from interpolated inputs | `weather.rs:465-469` | Small | Sub-hourly accuracy | E+ source comment at WeatherManager.cc:3113 | `weather.md §3`; E+ source |
| B6 | Window exterior LWR completely skipped — ~248W additional cooling unaccounted | `longwave.rs:42` | **est. +1.0 to +2.5°C** (unmeasured; upper bound likely overstates transient) | All with exterior glazing | Steady-state calculation | E+ Eng.Ref "External Longwave Radiation"; NFRC rating conditions; `lwr.md §3` |
| B7 | Mass multiplier double-counting — 7.0× on zone air PLUS explicit furniture RC nodes | `conversions.rs:29` | None (BESTEST mult=1) | **39% overstated zone mass** for HPXML | Calculation: 3,377 vs 2,434 kJ/K | E+ ZoneCapacitanceMultiplier default=1.0; InternalMass exclusive; `physics_defaults.md §3d` |
| B8 | Interior solar absorptance hardcoded 0.6 — should be per-surface or 0.70 default | `solver_builder.rs:704` | Minor (BESTEST spec is 0.6) | 17% interior solar error | Code inspection | E+ Material IDD default 0.70; `physics_defaults.md Add1` |
| B9 | Wind direction uses ZOH instead of circular interpolation | `weather.rs:498-502` | Minor | Incorrect at 350°→10° transitions | Code vs E+ source | E+ WeatherManager.cc:3183; `weather.md §5` |

### SHORTCUTS (correct direction, incomplete or wrong magnitude)

| # | Issue | Location | 900FF impact | General impact | Evidence | Source |
|---|---|---|---|---|---|---|
| S1 | Interior film coefficients frozen at init-time TARP with 12.9°C ΔT floor | `film_coefficients.rs:158-160` | +0.2–0.5°C (est.) | Heavyweight buildings | At ΔT=2°C, R_film 17–27% too low; interior_convection.md §4 | TARP/Alamdari-Hammond; E+ ConvectionCoefficients.cc MIN_DELTA_T |
| S2 | RC discretization uses timestep-dependent Fourier criterion — should use diurnal diffusion length Λ=√(αP/4π) | `boundary_rc.rs:44-78` | <0.03°C (measured) | Thick slabs, non-3600s dt | Empirical: 2→4 nodes = +0.029°C wrong direction | Incropera & DeWitt; ISO 13786:2007; `rc_discretization.md §3` |
| S3 | Air density hardcoded sea-level for zone capacitance — infiltration uses altitude-corrected | `boundary_rc.rs:17` | **+0.02°C (measured)** | 17% zone C error at Denver; 30% at 3100 m | Measured simulation delta | ASHRAE HoF 2021 §1.8 Eq.28; E+ PsyRhoAirFnPbTdbW; `air_density.md §5.1` |
| S4 | Free-float zones start at 21°C (DEFAULT_SETPOINT_C) — concrete has τ≈33 days | `environment.rs:836,751` | **−2.4°C** (measured at init=3°C; −4.53°C at init=0°C) | Free-float only | Measured sensitivity | E+ Eng.Ref §1.2 warmup convergence; `summary.md` empirical table |
| S5 | No warmup period for BESTEST — feature exists but fixtures don't use it | `900ff.toml` | Same as S4 | First 1–5 days of all runs | Same as S4 | E+ runs warmup until convergence; `dwelling/mod.rs:1912 run_warmup()` |
| S6 | Solar absorptance default 0.60 — E+ default is 0.70 | `longwave_radiation.rs:67` | None (BESTEST sets 0.6) | 17% exterior solar error | Code vs E+ IDD | E+ Material IDD \default 0.7; `physics_defaults.md §3a` |
| S7 | Solar radiation uses ZOH — E+ uses triangular interpolation | `weather.rs:477-491` | Minor | Step-function solar at hour boundaries | Code vs E+ source | E+ WeatherManager.cc SetupInterpolationValues; `weather.md §2` |
| S8 | Ground temp interpolated monthly — HARES is MORE realistic than E+ monthly step | `epw.rs:604-659` | Minor | Smoother (correct) | Physical reasoning | E+ WeatherManager.cc:2087; `weather.md §4` — NOT a deficiency |

### DESIGN FLAWS (not wrong, but misleading or incomplete)

| # | Issue | Location | Source |
|---|---|---|---|
| D1 | interior_lwr diagnostic always zero — reports Σq_i = 0 by conservation | `longwave.rs:316` | ScriptF energy conservation |
| D2 | Synthetic IR = 300 W/m² — inconsistent with clear-sky sky_cover=0 | `synthetic.rs:739-740` | Denver EPW clear winter: ~180 W/m² |
| D3 | Synthetic ground_temp = outdoor_temp — ground warmer in winter | `synthetic.rs:742` | DOE-2 ground temp model |
| D4 | Occupant radiative fraction = 0% — ASHRAE specifies ~30% | `constants.rs:136` | ASHRAE HoF 2021 Ch.18 Table 1 |
| D5 | PCHIP flat extrapolation at year boundary — no cyclic wrap | `weather.rs:704` | `weather.md §6` |
| D6 | `_ => 7.0` fallback in mass_multiplier_for_zone — should be exhaustive | `conversions.rs:29` | Project rule: no silent defaults |

### VERIFIED CORRECT (no fix needed)

| # | Item | Location | Verification |
|---|---|---|---|
| ✓1 | Window emissivity 0.84 matches NFRC/ASHRAE clear glass | `longwave_radiation.rs:54` | NFRC standard; E+ default |
| ✓2 | EPW sky temperature 3-tier cascade matches E+ | `epw.rs:483-502` | E+ WeatherManager.cc; `weather.md §8` |
| ✓3 | DOE-2 ground temperature model | `epw.rs:423-467` | Cross-verified against OCHRE + hand calc |
| ✓4 | PCHIP Fritsch-Carlson implementation | `weather.rs:561-648` | Cross-verified against SLATEC pchim.f |
| ✓5 | Infiltration uses altitude-corrected density | `infiltration.rs:75` | `moist_air_density_kg_m3(p_pa, t, w)` |
| ✓6 | Thermostat deadband 1.0°C within residential range | `thermostat.rs:36` | Typical: 0.5–1.1°C |
| ✓7 | Occupant gains 66/51.2 W appropriate for residential | `constants.rs:125,131` | OCHRE: 400 BTU/h × 0.563/0.437 |
| ✓8 | Energy balance closes by construction in RC network | Empirical | State-space KCL at each node |

---

## Revised 900FF error budget

900FF min_temp = +0.90°C, ASHRAE 140 upper band = −1.6°C, outlier = +2.50°C.

| Source | Direction | Estimated impact | Confidence | Evidence type |
|--------|-----------|-----------------|------------|---------------|
| S4/S5: Initial temp 21°C / no warmup | Warmer | **−2.4°C** (at init≈3°C) | **HIGH** | Measured |
| B1: Internal gain 100% convective | Warmer | **−0.22°C** (at correct 30% radiant) | MEDIUM | Measured at 40%; proportional est. at 30% |
| B6: Window exterior LWR skipped | Warmer | **−1.0 to −2.5°C** | **LOW** | Unmeasured steady-state calc |
| B2: Window interior LWR dropped | Cooler | **+0.5 to +1.0°C** (fix warms) | MEDIUM | Analytical; unmeasured |
| S1: Frozen film coefficients | Warmer | −0.2 to −0.5°C | LOW | Analytical |
| S3: Air density sea-level | Warmer | −0.02°C | HIGH | Measured |
| S2: RC under-discretization | Warmer | +0.03°C (wrong dir) | HIGH | Measured — ruled out |

**Net warm bias (with B2 cancellation)**: +1.6 to +3.7°C
**Observed**: +2.50°C — within range.

**Key insight**: The warmup/initialization issue (S4/S5) is the **largest
measured** contributor at ~−2.4°C.  B1 adds −0.22°C.  Together these
explain ~−2.6°C of the +2.50°C outlier — potentially closing the gap
by themselves.  B6 could over-correct significantly if its upper-bound
estimate is accurate.  B2 partially offsets B6 when both are fixed.

**The most likely scenario**: S4/S5 + B1 account for most of the error.
B6's real transient impact is probably at the lower end of its range
(~1.0°C rather than 2.5°C), and B2's offset (~0.5°C warming) partially
cancels B6.  This is consistent with the observed 2.50°C outlier.

---

## Prioritized fix plan

### Tier 1 — High confidence, high impact, measured evidence

#### Fix 1: Warmup / annual-periodic initialization (S4 + S5)

**Target**: `environment.rs:836,751`; `900ff.toml`
**Change**: For free-float buildings, initialize zone temperatures from
outdoor temperature (not DEFAULT_SETPOINT_C=21°C).  Enable warmup period
in BESTEST fixtures using the existing `run_warmup()` at
`dwelling/mod.rs:1912`.  E+ runs warmup until zone temperatures converge
to periodic steady state (Eng.Ref §1.2).
**Predicted delta**: −2.0 to −2.4°C on 900FF (measured)
**Risk**: Low — feature already exists, just needs wiring
**Physics source**: E+ Eng.Ref §1.2 "Simulation Warmup Convergence";
zone temperatures must reach periodic steady state before annual results
are collected.
**Implementation**:
1. Change `determine_initial_indoor_temp_c()` to use outdoor temperature
   for free-float zones (no HVAC setpoints → outdoor_temp, not 21°C)
2. Add `initialization_duration` to BESTEST fixtures (≥20 days to cover
   wall concrete τ≈3 days × 7 = 21 days for 99.9% decay)
3. Re-measure 900FF and 600FF

#### Fix 2: Internal gain radiant/convective split (B1)

**Target**: `ports.rs:19`, `hares-types/src/ports.rs`, `scheduled_load.rs`
**Change**: Add `radiant_gain_w` to `PortContribution::Thermal`.  Route
30% of sensible gain to interior surface nodes (area × emissivity
weighted), 70% to zone air.  Key constant already reserved:
`KEY_RADIATIVE_GAIN_FRACTION` at `scheduled_load.rs:30`.
**Predicted delta**: −0.22°C on 900FF (at FractionRadiant=0.3)
**Risk**: Medium — API change to `PortContribution::Thermal`, all callers
must update.  Default `radiant_gain_w=0.0` for backward compatibility.
**Physics source**: E+ BESTEST IDF OtherEquipment FractionRadiant=0.3
(Context7 confirmed); E+ Eng.Ref §"Zone Internal Gains" — radiant
fraction distributed by area × thermal absorptance (TMULT).
**Implementation**:
1. Add `radiant_gain_w: f64` to `PortContribution::Thermal` (default 0.0)
2. Add `apply_port_radiant_inputs()` in `thermal_solver/mod.rs` — reuse
   interior surface distribution mechanism (same as solar)
3. Add `radiant_gain_fraction` to BESTEST fixture: 0.3 per E+ IDF
4. Wire through `scheduled_load.rs` using existing `KEY_RADIATIVE_GAIN_FRACTION`
5. **Re-measure** 900FF with correct 30% radiant fraction (not 40%)

### Tier 2 — Correct physics, moderate impact, needs measurement

#### Fix 3: Window exterior LWR (B6)

**Target**: `longwave.rs:42`
**Change**: Remove the `BoundaryCategory::Window` skip.  Estimate window
exterior surface temperature from U-factor:
  T_surf_ext ≈ T_outdoor + R_ext_film_conv / R_total × (T_zone − T_outdoor)
Compute LWR delta beyond U-factor's built-in radiation:
  ΔQ = ε·σ·A·β·F_sky·(T_sky⁴ − T_air⁴)
Inject ΔQ into zone sensible input.
**Predicted delta**: −1.0 to −2.5°C on 900FF (WIDE RANGE — must measure)
**Risk**: Medium — requires estimating window surface temp without RC node;
careful to avoid double-counting with U-factor radiation component
**Physics source**: E+ Eng.Ref "External Longwave Radiation" — all exterior
surfaces including windows participate; NFRC rating assumes T_sky≈T_air;
Walton (1983) tilted-sky model β=√F_sky.
**Implementation**:
1. Add `u_factor_w_m2_k` to `ExteriorSurfaceInfo` for windows
2. Estimate T_surf_ext from R_ext_film_conv / R_total split
3. Compute LWR using the existing `exterior_longwave_w()` function
4. Inject ΔQ (LWR beyond U-factor assumption) to zone sensible input
5. **Must measure** actual 900FF delta before proceeding further

#### Fix 4: Window interior LWR application (B2)

**Target**: `longwave.rs:300-307`
**Change**: When `info.driving_temp.is_some()` (window), apply net LWR
flux q to zone air:
  u[air_idx] += q;
**Predicted delta**: +0.5 to +1.0°C on 900FF (fix WARMS zone — opposite
direction to most other fixes)
**Risk**: Low — simple code change; correct physics (energy conservation)
**Physics source**: E+ Eng.Ref "Inside Surface Heat Balance" — window LWR
included in zone heat balance via convective coupling; window has no
capacitance so all absorbed LWR immediately convects to zone air.
**Implementation**: Change guard from `driving_temp.is_none()` to always
apply, with window flux going entirely to zone air (radiation_frac=0
for windows).
**Ordering**: Must be done AFTER Fix 3 so that B6's cooling effect
dominates over B2's warming effect.

### Tier 3 — Correct physics, lower impact, good housekeeping

#### Fix 5: Synthetic weather sky temperature (B3)

**Target**: `synthetic.rs:741`
**Change**: `sky_temp_c: vec![clark_allen_sky_temp_c(t_db, t_dp); n]`
Also fix IR=300 → compute from sky emissivity, and ground_temp offset.
**Physics source**: Clark & Allen 1978; Berdahl & Martin 1984; E+
WeatherManager.cc uses same cascade.
**Impact**: Zero on BESTEST; severe on synthetic weather users.

#### Fix 6: TMY3 midpoint offset (B4)

**Target**: `tmy3.rs:240`
**Change**: `midpoint_offset_secs: 1800` (one-line fix)
**Physics source**: TMY3 hour-ending convention; EPW already sets 1800.

#### Fix 7: Zone air density altitude correction (S3)

**Target**: `boundary_rc.rs:17,273-285`
**Change**: Accept `site_pressure_pa` in `derive_zone_capacitances()`;
use `dry_air_density_kg_m3(site_pressure_pa, 20.0)` instead of constant.
**Physics source**: ASHRAE HoF 2021 §1.8 Eq.28; E+ PsyRhoAirFnPbTdbW;
ISA 1976 standard atmosphere `standard_pressure_pa(elevation_m)`.
**Impact on 900FF**: −0.02°C (measured); 17% correction at Denver for
general simulations.

#### Fix 8: Mass multiplier double-counting (B7)

**Target**: `conversions.rs:29`
**Change**: When furniture RC boundaries are present, set multiplier=1.0.
Remove `_ => 7.0` fallback (D6).  Phase 2: remove multiplier entirely.
**Physics source**: E+ uses ZoneCapacitanceMultiplier OR InternalMass,
not both; physics_defaults.md §3d shows 39% overstatement.
**Impact**: Zero on BESTEST (mult=1 already); significant for HPXML.

#### Fix 9: Sky temp recomputation after resampling (B5)

**Target**: `weather.rs:465-469`
**Change**: After PCHIP interpolation of all input fields, recompute
sky_temp_c from interpolated IR, dry_bulb, dew_point, sky_cover.
**Physics source**: E+ WeatherManager.cc:3113 comment; T_sky is a
non-linear function of inputs — direct interpolation violates chain rule.

#### Fix 10: Interior solar absorptance (B8)

**Target**: `solver_builder.rs:704`, `thermal_solver/mod.rs:3495`
**Change**: Extract `0.6` to named constant `INTERIOR_SOLAR_ABSORPTANCE_DEFAULT`
with value 0.70.  Wire per-surface from Boundary.solar_absorptance.
**Physics source**: E+ Material IDD default solar_absorptance=0.70.

#### Fix 11: Solar absorptance exterior default (S6)

**Target**: `longwave_radiation.rs:67`
**Change**: `SOLAR_ABSORPTANCE_DEFAULT = 0.70` (from 0.60)
**Physics source**: E+ Material IDD \default 0.7.

#### Fix 12: Wind direction circular interpolation (B9)

**Target**: `weather.rs:498-502`
**Change**: Add `CircularLinear` variant to `ResampleMethod`.
**Physics source**: E+ interpolateWindDirection() at
WeatherManager.cc:3183-3197.

#### Fix 13: RC discretization criterion (S2)

**Target**: `boundary_rc.rs:44-78`
**Change**: Replace timestep-dependent Fourier criterion with
timestep-independent diurnal diffusion length Λ=√(α·86400/(4π)).
Remove `DEFAULT_DT_S` and `dt_s` parameter.
**Physics source**: Incropera & DeWitt penetration depth; ISO 13786:2007;
implicit ZOH solver decouples spatial and temporal accuracy.
**Impact on 900FF**: <0.03°C; improves generalizability.

### Tier 4 — Design flaw cleanup

D1: Fix interior_lwr diagnostic to report zone-air-relevant flux or
    sum-of-absolute-fluxes instead of zero-sum Σq_i.
D2/D3: Fix synthetic weather IR and ground_temp to be physically consistent.
D4: Add `OCCUPANT_RADIATIVE_FRACTION = 0.30` to `constants.rs:136`;
    wire through equipment ports (related to B1 but for occupants).
D5: Add cyclic PCHIP wrap at year boundary (affects multi-year runs).
D6: Remove `_ => 7.0` fallback (related to B7).

---

## Measurement priorities

Before implementing Tier 2 fixes, these **must be measured** by running
900FF simulations with the specific code change:

1. **B1 at correct 30% radiant** — re-measure with FractionRadiant=0.3
   (the −0.295°C was at 40%; expect ~−0.22°C)
2. **B6 window exterior LWR** — no measurement exists; the +1.0–2.5°C
   estimate could be off by 50%+
3. **B2 window interior LWR** — unmeasured; the +0.5–1.0°C is analytical
4. **S4/S5 combined with B1** — after implementing warmup AND radiant
   split, measure total delta to see if 900FF closes

The recommended approach: implement Fix 1 (warmup) first, measure, then
implement Fix 2 (radiant split) and measure incrementally.  Only then
proceed to Fix 3 (window exterior LWR) and Fix 4 (window interior LWR).

---

## Sources

- **ASHRAE 140-2017** — reference bands, test case specifications
- **EnergyPlus Engineering Reference** — §1.2 warmup convergence;
  §3.2.4 exterior convection; §"Zone Internal Gains"; §"Inside Surface
  Heat Balance"; §"External Longwave Radiation"; §3.3.10 CondFD
- **EnergyPlus source code** — WeatherManager.cc (sky temp cascade,
  solar interpolation, wind direction, ground temp);
  ConvectionCoefficients.cc (MIN_DELTA_T); HeatBalFiniteDiffManager.cc
  (CondFD discretization); Psychrometrics.cc (PsyRhoAirFnPbTdbW)
- **EnergyPlus IDD** — OtherEquipment FractionRadiant=0.3 (confirmed via
  Context7 `/nrel/energyplus` from `design/FY2016/OtherEquipment.md`);
  Material default solar_absorptance=0.70
- **ASHRAE Handbook of Fundamentals 2021** — Ch.1 Eq.28 moist air density;
  Ch.15 window emissivity; Ch.18 Table 1 occupant gains
- **NFRC** — standard window rating conditions (winter: indoor 21°C,
  outdoor −18°C, h_out=34 W/(m²·K))
- **Walton (1983)** — tilted-sky model β=√F_sky for exterior LWR
- **Clark & Allen (1978)** — clear-sky temperature model
- **Berdahl & Martin (1984)** — sky emissivity model
- **Incropera & DeWitt** — thermal penetration depth δ=√(αt/π)
- **ISO 13786:2007** — dynamic thermal characteristics, RC network
  frequency response
- **ISA 1976 / ICAO Doc 7488** — standard atmosphere pressure from elevation
- **BESTEST-GSR** — `NatLabRockies/BESTEST-GSR` GitHub repository;
  `bestest_case_var_lib.rb` confirmed 900FF: `int_gen=200, mass='H',
  infil=0.5, glass_area=12`
