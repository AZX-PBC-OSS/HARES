Interior Surface Convection Coefficient Model: Impact on BESTEST 900FF Minimum Temperature
Executive Summary
Finding: The interior convection coefficient model has a secondary impact (~+0.2–0.5°C) on the 900FF minimum temperature, in the same direction as the primary root cause (internal gains 100% convective, ~+1.5–2.5°C). It is not the dominant cause of the +2.5°C outlier.
HARES does NOT use a constant R_FILM_INTERIOR = 0.12 for all surfaces (the thermal_loss.md doc is outdated). The production code path computes tilt-dependent, zone-dependent R values using the TARP model at initialization time, then bakes these as fixed resistances into the RC network. The problem is that these fixed values diverge from what a fully dynamic TARP model (as in EnergyPlus) would give at the small surface-air ΔT conditions of the minimum-temperature event.
---
1. Current HARES Interior Convection Model — File:Line References
1.1 Production Code Path (NOT the constant 0.12)
The production code path through solver_builder.rs → building_to_boundary_inputs() calls film_resistances() from hares-physics:
File	Line	What
crates/hares-core/src/dwelling/conversions.rs	67	use hares_physics::film_coefficients::{SurfaceRoughness, film_resistances};
crates/hares-core/src/dwelling/conversions.rs	83–91	let (r_film_int, r_film_ext) = film_resistances(tilt_deg, interior_label, exterior_label, avg_wind_m_s, avg_ground_c, avg_ambient_c, SurfaceRoughness::Rough);
crates/hares-core/src/dwelling/conversions.rs	203	r_film_interior_m2_k_w: r_film_int, — baked into BoundaryInput
The film_resistances() function (crates/hares-physics/src/film_coefficients.rs:137–189) implements the full TARP model with tilt-dependent formulas and the DOE-2 exterior model. This produces different R values for walls, floors, and roofs — it is NOT the flat 0.12 constant.
1.2 The Constant 0.12 Is Only Used in Unit Tests
The constant R_FILM_INTERIOR_M2_K_W = 0.12 at boundary_rc.rs:35 is only used in test code within that same file (lines 1036, 1494, 1745, 1799, 1813, 2005, 2043, 2080, 2114) and in the root-cause test file (bestest_900ff_root_cause.rs lines 207, 267, 339, 428, 463). It is not used in the production simulation path.
1.3 Outdated Documentation
The file docs/thermal_loss.md:14 states:
> Interior film resistance | Yes (tilt-dependent) | Yes (constant 0.12 m²·K/W)
This is incorrect — HARES now uses tilt-dependent TARP-based values via film_resistances().
---
2. What HARES Actually Computes for 900FF Boundaries
Using Denver EPW averages (avg_ambient ≈ 10°C, avg_ground ≈ 10°C):
Boundary	Tilt	Interior Zone	Exterior Zone	HARES R_film_int	BESTEST R_film_int
Walls (4×)	90°	Conditioned	Outdoor	0.122	0.12 (vertical)
Roof	0°	Conditioned	Outdoor	0.145	0.106 (upward) / 0.162 (downward)
Floor	180°	Conditioned	Ground	0.115	0.106 (upward) / 0.162 (downward)
Key computation for walls (the dominant heavyweight surface):
delta_t = max(|15 − 20|, 12.9) = 12.9°C  (MIN_DELTA_T floor applied!)
h_conv  = 1.31 × 12.9^(1/3) = 3.075 W/(m²·K)
h_rad   = 4 × 0.9 × 5.67e-8 × 293.15³ = 5.143 W/(m²·K)
h_comb  = 3.075 + 5.143 = 8.218 W/(m²·K)
R_int   = 1/8.218 = 0.1217 m²·K/W
The 12.9°C MIN_DELTA_T floor is from EnergyPlus ConvectionCoefficients.cc and prevents near-zero h at small ΔT. But it means HARES always computes h_conv as if ΔT = 12.9°C, even when the actual surface-air ΔT is much smaller.
---
3. What TARP/Alamdari-Hammond Would Give at Min-Temp Conditions
At the minimum-temperature event (step 944, zone air ≈ 0.9°C), the actual surface-air ΔT for the wall interior concrete surface is approximately 1–3°C (the concrete has been cooling all night but retains some stored heat).
TARP natural convection for vertical surfaces: h = 1.31 × |ΔT|^(1/3)
Actual ΔT (°C)	TARP h_conv	TARP h_combined (+h_rad=5.14)	TARP R_film	HARES R_film (fixed)	HARES overstates heat flow by
1.0	1.31	6.45	0.155	0.122	27%
2.0	1.65	6.79	0.147	0.122	21%
3.0	1.89	7.03	0.142	0.122	17%
5.0	2.24	7.38	0.136	0.122	11%
10.0	2.82	7.97	0.126	0.122	3%
At the min-temp hour with ΔT ≈ 2°C, HARES's fixed R_film of 0.122 overestimates the surface-to-air convective coupling by approximately 21% compared to what a dynamic TARP model would give.
---
4. Direction and Magnitude of Temperature Impact
4.1 Physical Mechanism
In the 900FF heavyweight case, the interior concrete surface of the walls is the dominant thermal mass buffering zone air temperature. The heat flow path is:
Concrete inner node → R_film_int → Zone air node
At the minimum-temperature event:
- Zone air: ~0.9°C (losing heat via infiltration and exterior conduction at ~2000W)
- Concrete inner node: ~2–4°C (still retains daytime heat)
- Heat flows FROM concrete TO zone air through R_film_int
If R_film_int is too low (coupling too strong):
1. More heat flows from concrete to air in each timestep → air is warmer
2. But concrete cools faster → buffering effect is shorter-lived
If R_film_int is too high (coupling too weak):
1. Less heat flows from concrete to air → air is cooler
2. Concrete retains heat longer → buffering lasts longer but delivers heat more slowly
HARES's fixed R = 0.122 is too low at the min-temp event (dynamic TARP gives R ≈ 0.147 at ΔT = 2°C). This means HARES overestimates the heat flow from wall concrete to zone air by ~21%, making zone air warmer than it should be.
4.2 Quantitative Estimate
The wall concrete area is ~51.6 m² (4 walls minus windows). At ΔT ≈ 2°C:
Model	R_eff (film + half-concrete)	Q_wall→air (W)
HARES fixed R_film=0.122	0.122 + 0.049 = 0.171 m²·K/W	51.6 × 2 / 0.171 = 603 W
TARP dynamic R_film=0.147	0.147 + 0.049 = 0.196 m²·K/W	51.6 × 2 / 0.196 = 527 W
Extra heat to zone air: ~76 W from wall concrete alone.
The floor concrete contributes negligibly because the floor is heavily insulated from ground (R ≈ 25 m²·K/W), causing the floor concrete to track zone air temperature closely (ΔT ≈ 0.04°C, heat flow ≈ 16W — insignificant).
The roof is lightweight (gypsum + insulation) with negligible thermal mass, so R_film differences there have minimal impact on zone air temperature.
4.3 Impact on Minimum Temperature
The extra ~76W from walls, over the ~5-hour cooling event leading to the minimum:
- Upper bound (constant ΔT): 76W × 5h × 3600s / 157,000 J/K = +8.7°C — obviously too high; the concrete would cool
- Time-constant correction: wall concrete τ ≈ 3.3 hours; over 5h, ~78% of stored heat is released regardless. The extra heat from lower R_film is a fraction of the total released heat.
- Realistic estimate: +0.2–0.5°C on minimum zone temperature
4.4 Direction of Error
The interior convection coefficient error pushes zone air WARMER — the same direction as the primary root cause (internal gains 100% convective). It does not offset or compete with the primary error; it amplifies it.
Source	Estimated Impact on Min Temp	Direction
Internal gains 100% convective (primary)	+1.5–2.5°C	Too warm ↑
Fixed-vs-dynamic TARP R_film (secondary)	+0.2–0.5°C	Too warm ↑
Zone air capacitance sea-level density (secondary)	+0.05–0.15°C	Too warm ↑
Combined	+1.8–3.2°C	Too warm ↑
---
## 5. Code Changes Needed
### 5.1 Option A: Dynamic TARP in the Thermal Solver (Major refactor)
The RC network uses **linear** state-space matrices (A, B fixed at initialization). Making R_film dynamic would require either:
- Reformulating the state-space matrices at each timestep (expensive, O(n³) matrix exponential)
- Switching to a nonlinear ODE solver (fundamental architecture change)
**Verdict**: Too invasive for a +0.2–0.5°C improvement.
### 5.2 Option B: Use BESTEST Fixed Film Coefficients for 900FF (Targeted fix)
ASHRAE 140 specifies fixed interior film resistances (Table 5-6):
- 0.12 m²·K/W for vertical surfaces
- 0.106 m²·K/W for upward heat flow
- 0.162 m²·K/W for downward heat flow
For the BESTEST cases specifically, we could:
1. Add a `convection_model` field to the boundary fixture (e.g., `"FixedASHRAE140"`)
2. In `building_to_boundary_inputs()`, when this model is specified, use the ASHRAE 140 table values instead of TARP
3. For walls, the difference is negligible (0.12 vs 0.122). For floor and roof, the difference is modest.
**Verdict**: Low-impact for 900FF specifically (walls are the dominant heavyweight surface and R is already close). Not worth implementing for the 900FF fix alone.
### 5.3 Option C: Remove or Lower the MIN_DELTA_T Floor (Moderate fix)
The 12.9°C MIN_DELTA_T floor in `film_coefficients.rs:158` causes R_film to be systematically too low for small-ΔT conditions. Removing or lowering this floor (e.g., to 1.0°C) would:
- Make R_film closer to what dynamic TARP gives at small ΔT
- Risk: at ΔT → 0, h_conv → 0 and R → ∞, causing numerical issues in the RC network
**Implementation**: Change line 158 from `12.9` to a smaller value (e.g., `1.0` or `2.0`) and add a maximum R_film cap to prevent infinite resistance.
**Estimated impact**: Would reduce the min-temp overestimate by ~0.1–0.3°C.
**Verdict**: Minor improvement; not sufficient to bring 900FF into band by itself.
### 5.4 Recommended Priority
The interior convection coefficient is a **secondary** issue. The primary fix is implementing the radiant/convective split for internal gains (Hypothesis #1 from the root cause analysis). The convection coefficient fix should be addressed **after** the primary fix, as part of a comprehensive accuracy improvement.
---
6. How to Verify Empirically
6.1 Diagnostic Test: Compare Fixed vs Dynamic TARP
Create a test that:
1. Runs 900FF with the current fixed R_film values
2. Runs 900FF with R_film values computed for ΔT = 2°C (representative of min-temp conditions)
3. Compares minimum temperatures
The R_film for walls at ΔT = 2°C: 0.147 m²·K/W (vs current 0.122)
Expected result: Step 2 should give a minimum temperature ~0.2–0.5°C lower than Step 1.
6.2 Compare Against OCHRE
Run OCHRE (which uses the same TARP-at-initialization approach) for 900FF and compare minimum temperatures. Since OCHRE uses the same film R computation, the results should be similar, confirming this is an inherited design limitation rather than a HARES-specific bug.
6.3 EnergyPlus Comparison
Run EnergyPlus with the 900FF IDF using the TARP interior convection model (dynamic). Compare the minimum temperature against HARES. The difference attributable to fixed-vs-dynamic R_film should be ~0.2–0.5°C.
---
7. Classification: Shortcut or Bug?
This is a shortcut (design simplification) that becomes a secondary bug in heavyweight free-floating cases:
- Shortcut: The RC network requires fixed resistances for the linear state-space formulation. Computing R_film at initialization and fixing it is the simplest approach compatible with this architecture. The OCHRE reference implementation does the same thing (ochre/utils/envelope.py:342-402).
- Secondary bug: At small surface-air ΔT (the exact conditions of the min-temp event in 900FF), the MIN_DELTA_T = 12.9°C floor makes R_film too low by ~17–27%, overestimating surface-to-air heat transfer by a similar fraction. This contributes ~+0.2–0.5°C to the minimum temperature error.
- Not a primary bug: The wall R_film (0.122) is within 1.4% of the BESTEST specification (0.12) and within 21% of what dynamic TARP would give at the min-temp event. The error magnitude is small relative to the primary root cause (+1.5–2.5°C from internal gains).
---
8. Summary Table
Aspect	Finding
HARES uses constant 0.12?	No — uses TARP at init via film_resistances()
HARES uses dynamic TARP?	No — bakes R_film at init, fixed for simulation
Wall R_film vs BESTEST	0.122 vs 0.12 = +1.4% (negligible)
Wall R_film vs dynamic TARP at ΔT=2°C	0.122 vs 0.147 = −17% (too low)
Floor R_film impact	Negligible (concrete tracks zone air; heavy ground insulation)
Roof R_film impact	Small (lightweight surface, minimal mass)
Impact on 900FF min temp	+0.2–0.5°C (secondary, same direction as primary error)
Classification	Shortcut → secondary bug (inherited from OCHRE)
Fix priority	After primary fix (internal gains radiant split)
Fix approach	Lower/remove MIN_DELTA_T floor, or accept as known limitation