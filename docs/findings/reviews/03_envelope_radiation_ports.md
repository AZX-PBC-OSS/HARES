# Review 03 — Envelope Radiation, Solar, and Port-Application Layer

**Scope**: `c1abb8af9d3087873ba4d7010f3383a647a7fd79..HEAD`  
**Reviewed by**: Code reviewer agent  
**Files reviewed**: `thermal_solver/{ports,mod,longwave,solar,config}.rs`, `longwave_radiation.rs`,
`humidity_solver.rs`, `lib.rs`, `hares-physics/src/solar.rs`, `hares-types/src/ports.rs`,
`hares-physics/src/constants.rs`, test files in `crates/hares-envelope/tests/`,
`crates/hares-core/tests/port_accumulation_tests.rs`, `crates/hares-core/src/dwelling/mod.rs`,
`crates/hares-equipment/src/scheduled_load.rs`.  
**Tests executed**: `cargo test -p hares-envelope` (241 pass, 1 fail), `cargo test -p hares-core --test port_accumulation_tests` (21 pass), `cargo test --test multi_zone_coupling` (2 pass).  
**Clippy**: clean (no warnings on hares-envelope).  
**Verdict**: CONCERNS — one failing test (blocker), two correctness/asymmetry gaps in port radiant distribution, and several medium-severity issues. All major physics fixes are correctly directed.

---

## 1. Executive Summary

This diff introduces four significant physics corrections: (1) a new window exterior LWR correction via the Walton T_eff approach, (2) window interior LWR routing to zone air only (OCHRE "full" mode), (3) a new `apply_port_radiant_inputs` function routing equipment radiant gains to surfaces via TMULT weighting, and (4) exterior/interior solar absorptance defaults corrected to E+ IDD 0.70. The occupancy radiant fraction (30%) is now correct and properly scoped to the indoor zone only.

Physics direction is correct in all cases. Energy conservation holds for single-conditioned-zone buildings. The main unresolved issues are a test regression (F1 — failing test documents a real simulation fact that must not be broken), a zone-scoping asymmetry in radiant vs. sensible port application (F2), and a debug snapshot gap (F3). Two per-timestep allocations violate the hot-loop policy (F9).

---

## 2. Constants Audit

| Constant | Value in Code | Expected | Source | Status |
|----------|--------------|----------|--------|--------|
| `STEFAN_BOLTZMANN` | `5.670_374_419e-8` | `5.670374419×10⁻⁸ W/(m²·K⁴)` | NIST CODATA 2018 | CORRECT |
| `CELSIUS_TO_KELVIN` | `273.15` | `273.15 K` | ISA 1976 / NIST | CORRECT |
| `EMISSIVITY_DEFAULT` (opaque) | `0.90` | 0.85–0.95 (E+ default 0.9) | E+ IDD `\default 0.9` | CORRECT |
| `EMISSIVITY_WINDOW` | `0.84` | 0.84 (uncoated clear glass) | NFRC 100 / E+ Window module | CORRECT |
| `EMISSIVITY_RADIANT_BARRIER` | `0.05` | 0.03–0.07 (foil) | OCHRE `Envelope.py:222` | CORRECT |
| `SOLAR_ABSORPTANCE_DEFAULT` (exterior) | `0.70` | `0.70` | E+ IDD `Material \default Solar_Absorptance` | CORRECT (was 0.60) |
| `INTERIOR_SOLAR_ABSORPTANCE_DEFAULT` | `0.70` | `0.70` | E+ IDD `Material \default Solar_Absorptance` | CORRECT (was 0.60) |
| `SOLAR_ABSORPTANCE_RADIANT_BARRIER` | `0.05` | 0.05 | OCHRE | CORRECT |
| `H_OUT_NFRC` | `34.0` | `34 W/(m²·K)` | NFRC 100-2020 winter rating | CORRECT |
| `OCCUPANT_SENSIBLE_GAIN_W` | `66.0 W/person` | ~66 W/person | OCHRE 400 BTU/h × 0.563 | CORRECT |
| `OCCUPANT_LATENT_GAIN_W` | `51.2 W/person` | ~51.2 W/person | OCHRE 400 BTU/h × 0.437 | CORRECT |
| `OCCUPANT_RADIATIVE_FRACTION` | `0.30` | 0.30 | ASHRAE HoF 2021 Ch.18 Table 1 | CORRECT |
| `OCCUPANT_CONVECTIVE_FRACTION` | `0.70` | `1 − 0.30 = 0.70` | Complement | CORRECT |
| `LEGACY_BEAM_FLOOR_FRAC` | `0.60` (test-only constant) | 0.6 (OCHRE legacy; E+ uses altitude-dependent `sin(alt)`) | OCHRE legacy | ACCEPTABLE (tests only) |
| `beam_floor_fraction(alt)` (production) | `sin(alt_rad).clamp(0.3, 0.9)` | E+ FullInteriorAndExterior: sin(altitude), clamped | E+ Eng.Ref "Solar Distribution" | CORRECT |

All numeric literals in the radiation/solar/ports paths are named constants sourced from primary references. No magic numbers found in production paths.

One threshold concern: at `crates/hares-envelope/src/thermal_solver/longwave.rs:86`, `h_out > 1.0` is the guard for using the actual film coefficient. The correct guard is `> 0.0`. See F5.

---

## 3. Formulae Audit

### 3.1 ScriptF Interior LWR

**Theory (Hottel-Sarofim, Radiative Transfer 1967)**: For a grey N-surface enclosure, the net radiative flux on surface i is:
```
q_net,i = Σ_j ScriptF_ij · σ · (T_i⁴ − T_j⁴)
```
where ScriptF_ij (grey interchange factor) satisfies:
- Reciprocity: A_i · ScriptF_ij = A_j · ScriptF_ji
- Closure: Σ_j ScriptF_ij = ε_i (not 1; grey surfaces don't absorb all radiation)
- Conservation: Σ_i A_i · q_net,i = 0 (enclosure energy balance)

**Implementation**: `longwave_radiation.rs:ScriptFCoefficients::compute()`. The ScriptF factors are precomputed at `InteriorLwrZoneConfig::compute_scriptf()` (init time). Tests `interior_lw_energy_conservation_asymmetric_areas` and `scriptf_mixed_emissivity_energy_conservation` verify Σq = 0 to numerical precision. Status: CORRECT.

**Application in `apply_interior_longwave_inputs`**: ScriptF path only runs when `interior_lwr_method == ScriptF` (guarded at mod.rs:484). In StarMesh mode, radiation conductances are baked into the A-matrix at construction time; no per-timestep injection occurs. Status: CORRECT.

### 3.2 Window Exterior LWR (Walton T_eff Approach)

**Theory (Walton 1983 TARP)**: For a surface tilted at angle φ, sky view factor:
```
F_sky = ½ (1 + cos φ)
```
Walton's β factor accounts for diffuse radiation from the sky dome:
```
β = √F_sky
```
The correction beyond the U-factor's T_sky = T_air assumption:
```
Δq = ε · σ · β · F_sky · (T_sky⁴ − T_air⁴)   [W/m²]
```
The effective outdoor temperature is T_eff = T_air + Δq / h_out. The additional zone heat transfer relative to the U-factor rating is:
```
ΔQ_zone = U · A · (T_air − T_eff) = (U / h_out) · Δq · A
```
This avoids double-counting: the U-factor already incorporates h_out radiation at T_sky = T_air.

**Implementation** (`longwave.rs:69–91`): Correctly applies this formula. The `sky_view_factor(tilt_deg)` and `beta_factor(tilt_deg)` functions are verified against the Walton formulation. For vertical window (tilt=90°): F_sky = 0.5, β = √0.5 ≈ 0.707. Status: CORRECT.

**Guard**: `h_out > 1.0` threshold at line 86 should be `> 0.0`. See F5.

**No double-count**: `window_exterior_lwr_w` is separated from opaque LWR at mod.rs:477. Status: CORRECT.

### 3.3 Window Interior LWR (OCHRE "Full" Mode)

**Theory**: For a window surface without an RC node (no thermal capacitance), the absorbed interior LWR flux must flow somewhere. OCHRE's "full" mode routes:
- `q_window × (1 − radiation_frac)` → zone air (immediate convective exchange)
- `q_window × radiation_frac` → carried by the window's U-factor conduction path (boundary temperature implicitly includes this flux)

This is not energy destruction: the window boundary temperature in the RC network reflects the LWR exchange. The `radiation_frac = R_film_int / R_total` decomposition determines the split.

**Implementation** (`longwave.rs:383–398`): Correctly applies the OCHRE "full" mode with `driving_temp.is_some()` as the window flag. The `q * radiation_frac` portion is deliberately omitted from direct injection. Status: CORRECT.

**Verification**: `window_interior_lwr_energy_conservation` confirms the "missing" energy is accounted for via the conduction path.

### 3.4 Internal Gain Radiant/Convective Split

**Theory (E+ Eng.Ref "Zone Internal Gains")**: For each internal gain source with total sensible P:
- Convective fraction (1 − f_rad) → zone air node
- Radiant fraction f_rad → distributed to surfaces weighted by A_i · α_thermal,i (TMULT method)

For OtherEquipment in BESTEST IDF: FractionRadiant = 0.3.  
For occupants (ASHRAE HoF 2021 Ch.18 Table 1): ~30% radiative.

**Implementation**:
- Equipment: `scheduled_load.rs:518–519` correctly computes `radiant_gain_w = total_gain_source_w × radiant_gain_fraction` and `sensible_gain_w = total_sensible_w - radiant_gain_w`.
- Occupancy: `dwelling/mod.rs:1910–1911` uses `OCCUPANT_RADIATIVE_FRACTION = 0.30`. Status: CORRECT.
- TMULT distribution: `ports.rs:distribute_radiant_lwr_surfaces` uses `w = area_m2 × emissivity` (thermal absorptance ≈ emissivity by Kirchhoff's law). This matches E+. The `radiation_frac` split then routes a portion to the surface RC node and remainder to zone air. Status: CORRECT.

**Default radiant fraction**: When no HPXML `FracRadiant` is provided, `radiative_gain_fraction` defaults to 0.0 in `scheduled_load.rs:249`. This means all scheduled loads are treated as 100% convective unless explicitly configured. BESTEST fixtures set 0.3 explicitly. This default is a latent source of simulation error for general HPXML inputs that rely on E+'s default FractionRadiant. See F10.

### 3.5 Solar Distribution

**Interior solar (E+ FullInteriorAndExterior)**: Beam solar is split between floors and non-floors using `sin(altitude_deg)` clamped [0.3, 0.9]. Within each class, solar is distributed proportional to A_i × α_solar,i. This matches E+ SMULT method.

Diffuse solar is distributed proportional to total A_i × α_solar,i. The "reflected" energy (from surfaces with zero absorptance) spills to zone air. Status: CORRECT.

**StarMesh mode**: `compute_solar_distribution_into_solar` at solar.rs:315 correctly replicates this logic for `InteriorSolarSurfaceInfo`. Status: CORRECT.

**Exterior solar**: Opaque surfaces receive `absorptance × area × POA`. Windows receive `SHGC × IAM(θ) × area × POA` (beam corrected, diffuse uses hemispherical IAM). Status: CORRECT.

### 3.6 Port-Accumulation Semantics

**Contract**: `PortSlots.accumulate()` ADDS the contribution to the existing accumulator (never overwrites). Two actors writing to the same zone in one step produce a sum. This is verified by `thermal_accumulates_per_zone` and `multi_timestep_accumulation_is_independent`.

Energy partitioning:
- `sensible_gain_w` → zone air via `apply_port_sensible_inputs`
- `radiant_gain_w` → TMULT-weighted surfaces (+overflow to zone air) via `apply_port_radiant_inputs`
- `latent_gain_w` → humidity solver via `latent_by_zone`

Status: CORRECT for single-conditioned-zone. F2 applies for multi-zone.

---

## 4. Test Integrity Audit

| Test | Reference | Tolerance | Verdict |
|------|-----------|-----------|---------|
| `interior_lw_energy_conservation_asymmetric_areas` | E+ grey enclosure Σq = 0 by construction | 1e-9 W | PASS — physically derived |
| `linearised_interior_lw_conserves_energy_mixed_emissivity` | As above with ε·A-weighted MRT | 1e-9 W | PASS |
| `scriptf_mixed_emissivity_energy_conservation` | Hottel-Sarofim two-surface enclosure | 1e-9 W (relative) | PASS |
| `window_exterior_lwr_clear_night_is_cooling` | Walton Δq = ε·σ·β·F_sky·(T_sky⁴−T_air⁴) | Range assertion ~−17 W | PASS — reference derived, not snapshot |
| `window_exterior_lwr_zero_when_sky_equals_air` | Walton: T_sky = T_air → Δq = 0 | 1e-6 W | PASS |
| `window_exterior_lwr_teff_scaling_reduces_raw_delta` | U/h_out ratio derivation | 1e-9 (ratio) | PASS |
| `window_exterior_lwr_horizontal_sees_more_sky_than_vertical` | F_sky(0°)=1 > F_sky(90°)=0.5 | Directional only | PASS — qualitative |
| `radiant_gains_distributed_by_tmult_to_opaque_surfaces` | TMULT: Σdeposited = total_radiant_w | 1e-9 W | PASS — energy conservation |
| `radiant_gains_with_window_surfaces_go_to_air` | Window (driving_temp.is_some()) → zone air | 1e-9 W | PASS |
| `window_interior_lwr_applied_to_zone_air` | OCHRE "full" mode — q×(1−rf) to air | 1e-6 W | PASS — reference derived |
| `window_interior_lwr_energy_conservation` | Total flux = injected + window-to-cond | 1e-9 W | PASS |
| `heavyweight_concrete_wall_produces_two_rc_sub_layers` | Documented empirical: 4 RC nodes | Exact | **FAIL** — receives 5 |
| `initialize_steady_state_pins_only_configured_indoor_zone` | Analytical 2-zone steady-state | 1e-3 °C | PASS — first-principles |
| `thermal_accumulates_per_zone` | Accumulator contract | 1e-9 | PASS |
| `undeclared_zone_rejected` | Port safety contract | Error type | PASS |

**Snapshot tests**: None found. All tolerance-bearing assertions use derived reference values (Walton, grey-enclosure algebra, TMULT identity). No tolerance widening observed relative to the base commit.

**Test quality gaps**:
- No test verifies the `beam_floor_fraction(alt)` production formula against an E+ reference case (only the clamped range is tested).
- `window_interior_lwr_uses_ochre_full_mode` in `longwave.rs` tests at the unit level but does not cover the full solver path (it computes the fractions manually without invoking the solver). This is fine as a unit test but leaves the integration untested at the solver level independently of `window_interior_lwr_applied_to_zone_air`.

---

## 5. Port Accumulation Audit — End-to-End Traces

### Scenario (a): Occupancy actor, 200W sensible + 60W radiant + 20W latent, zone 1 (indoor)

**Emission** (dwelling/mod.rs:1910–1916 — note: 200W+60W is for illustration; actual split depends on n_occupants × constants):
```rust
thermal.add(sensible_w, radiant_w, latent_w, ThermalCategory::InternalGain)
// ThermalAccumulator[ZoneId(1)].sensible_gain_w += 200.0
// ThermalAccumulator[ZoneId(1)].radiant_gain_w += 60.0
// ThermalAccumulator[ZoneId(1)].latent_gain_w += 20.0
```

**Thermal solver resolve()** (mod.rs:497–498):

Step 1 — `apply_port_sensible_inputs` (ports.rs:14–22):
```
u[zone_sensible_idx] += 200.0   (direct to zone air)
```

Step 2 — `apply_port_radiant_inputs` (ports.rs:34–75):
```
total_radiant_w = 60.0  (filtered to indoor_zone only)
```
With example surfaces (A₁=40m², ε₁=0.9; A₂=20m², ε₂=0.9):
- `total_weight = 40×0.9 + 20×0.9 = 54.0`
- Surface 1: `q₁ = 60 × 36/54 = 40.0 W`; `u[s1.input_index] += 40.0 × rad_frac₁`; `air += 40.0 × (1−rad_frac₁)`
- Surface 2: `q₂ = 60 × 18/54 = 20.0 W`; `u[s2.input_index] += 20.0 × rad_frac₂`; `air += 20.0 × (1−rad_frac₂)`
- `u[zone_sensible_idx] += air_from_radiant`

Total deposited: `200 + 40×rf₁ + 40×(1−rf₁) + 20×rf₂ + 20×(1−rf₂) = 200 + 60 = 260 W`. Conservation holds.

**Humidity solver**: `latent_by_zone[ZoneId(1)] = 20.0 W` is passed to `HumiditySolver.resolve()`. The 20W latent is used to compute Δω (humidity ratio change). No interaction with the u-vector.

### Scenario (b): Plug load, 500W sensible, 0W radiant, 0W latent, zone 1

Step 1: `u[zone_sensible_idx] += 500.0`  
Step 2: `total_radiant_w = 0.0` → early return at ports.rs:43 (guard `<= 0.0`). No surface distribution occurs.  
Result: 500W goes entirely to zone air. Correct for a fully convective appliance.

### Scenario (c): Window solar gain 800W transmitted, zone 1

Solar bypasses `PortSlots` entirely. Path via `apply_solar_inputs` (mod.rs, then solar.rs):
1. `EnvironmentState.weather.solar_irradiance` carries beam POA and diffuse POA per surface_id.
2. For the window surface, `window_transmitted_solar_angular(beam_poa, diffuse_poa, shgc, area, aoi, curve)` computes transmitted W.
3. Result deposited to `u[solar_input_indices[surface_id]]` (a dedicated solar column in B-matrix, NOT the sensible column).
4. Interior distribution via `distribute_to_interior_surfaces_for_zone`: beam solar is split floor/non-floor by `beam_floor_fraction(alt)`, then by A_i × α_solar,i within each class. Diffuse by total A_i × α_solar,i.
5. Surface shares go to `u[surface_input_index] += q × radiation_frac`.
6. Zone air gets `u[zone_sensible_idx] += reflected_w + air_spillover`.

Total: 800W partitioned among surfaces and zone air. Conservation: reflected + air_spillover + Σ(q_i × rad_frac_i) + Σ(q_i × (1−rad_frac_i)) = 800W. No double-count.

### Scenario (d): Attic duct loss, 100W sensible, 0W radiant, into zone 2 (attic/unconditioned)

`duct_distribution.rs:113–119` emits:
```rust
PortContribution::Thermal { zone: ZoneId(2), sensible_gain_w: 100.0,
    radiant_gain_w: 0.0, latent_gain_w: 0.0, category: DuctLoss }
```

`apply_port_sensible_inputs` (ports.rs:14–22) iterates ALL zones:
```
if let Some(&idx) = zone_sensible_input_indices.get(&ZoneId(2)) && idx < u.len() {
    u[idx] += 100.0   // attic zone air node
}
```
Result: 100W correctly deposited to zone 2 air node — NOT silently dropped. The `DuctLoss` category is tracked for diagnostics (mod.rs:561–566 sums DuctLoss across all zone accumulators).

For `apply_port_radiant_inputs`: since `radiant_gain_w = 0.0`, the early return fires immediately. No issue.

**If** a hypothetical future piece of equipment emitted `radiant_gain_w > 0` to `ZoneId(2)`, that radiant component would be silently dropped — see F2.

### Scenario (e): Two actors write to the same port in one step

Both `PortSlots.accumulate()` calls ADD to the accumulator. There is no overwrite:
```rust
// ports.rs accumulate():
total.add(*sensible_gain_w, *radiant_gain_w, *latent_gain_w, *category);
// ThermalAccumulator.add() does +=, not =
```
After step: `sensible_gain_w = sum_of_all_sensible`, `radiant_gain_w = sum_of_all_radiant`. Semantics: additive. Verified by `thermal_accumulates_per_zone` and `zero_then_accumulate_starts_fresh`. Correct.

---

## 6. Zone Distribution Audit

| Gain Source | Zone Routing | Correct? |
|-------------|--------------|----------|
| Occupancy sensible convective | `indoor_zone_id` only (mod.rs:1914–1916) | YES — conditioned only, post commit 99ccdcf |
| Occupancy sensible radiant (30%) | `indoor_zone_id` only (same path) | YES |
| Occupancy latent | `indoor_zone_id` only | YES |
| Scheduled load (appliances, lighting) | Equipment's declared zone | YES |
| Scheduled load radiant fraction | Indoor zone only via `apply_port_radiant_inputs` | PARTIAL — see F2 |
| HVAC heating/cooling | Declared zone, `radiant_gain_w=0` | YES — HVAC is purely convective |
| WH jacket loss | Declared zone (indoor or unconditioned), `radiant_gain_w=0` | YES — jacket loss is convective |
| WH heat extraction (HPWH) | `radiant_gain_w=0` | YES |
| Duct distribution losses | Duct zone (ZoneId(2)), tagged DuctLoss, `radiant_gain_w=0` | YES |
| Battery/generator waste heat | Declared zone, `radiant_gain_w=0` | YES |
| Event load | Declared zone, `radiant_gain_w=0` | YES |
| Dehumidifier | Declared zone, `radiant_gain_w=0` | YES |
| Window solar transmitted | `window_zone_ids[surface_id]` or `indoor_zone_id` | YES |
| Exterior opaque solar | Per-surface `input_index` | YES |
| Exterior LWR opaque | Per-surface `input_index` | YES |
| Window exterior LWR correction | `window_zone_ids[surface_id]` or `indoor_zone_id` | YES |
| Interior LWR (ScriptF) | Per-zone surfaces in `interior_lwr_zones` | YES — zone-scoped |
| Interior LWR (StarMesh) | Baked into A-matrix at construction | YES |

**Observation**: All currently-active equipment that emits `radiant_gain_w > 0` targets the indoor zone (`scheduled_load` via HPXML `FracRadiant`, occupancy via constant). F2 is therefore a latent architectural defect, not an active simulation error, but must be documented as a contract constraint.

---

## 7. Energy Conservation Audit

| Path | Status |
|------|--------|
| `apply_port_sensible_inputs`: 100% of sensible deposits to target zone air | CORRECT |
| `apply_port_radiant_inputs`: `total_radiant_w = Σ(q_i × rf_i) + air_from_radiant` | CORRECT — verified by `radiant_gains_distributed_by_tmult_to_opaque_surfaces` |
| `apply_port_radiant_inputs`, non-indoor zones: radiant discarded | DEFECT (F2) — conserves zone total only if no non-indoor equipment emits radiant |
| Interior LWR (ScriptF): Σq = 0 by construction | CORRECT — `Σ|q_i|/2` is the diagnostic, not the zero-sum signed total |
| Interior LWR (StarMesh): baked into A-matrix | CORRECT — no injection; lwr_by_zone reports 0 |
| Window interior LWR: `q × rf` to conduction path, `q × (1−rf)` to zone air | CORRECT — `window_interior_lwr_energy_conservation` verifies |
| Window exterior LWR: single deposit to `u[zone_sensible_idx]`; subtracted from opaque_solar_lwr_w | CORRECT — no double-count |
| Solar distribution: reflected + air_spillover + Σ surface deposits = total transmitted | CORRECT — verified by `solar_energy_conservation` tests |
| Latent: `latent_gain_w` → humidity solver only; not in u-vector | CORRECT |
| `internal_gain_cat_w` telemetry = `sensible_for_category + radiant_for_category` | CORRECT semantics (total includes both) |

---

## 8. Severity-Ranked Findings

| # | Severity | File:Line | Summary |
|---|---|---|---|
| F1 | **BLOCKER** | `crates/hares-envelope/tests/bestest_900ff_root_cause.rs:189` | Test `heavyweight_concrete_wall_produces_two_rc_sub_layers` asserts 4 RC nodes; receives 5. The test documents a measured empirical fact (RC over-discretization = +0.029°C wrong direction). A passing test has been broken by layer-splitting changes in commits since base. Must be fixed before merge — either fix the RC node count to match the empirical baseline or update the test with a new measured reference and update the comment to explain the change. |
| F2 | **HIGH** | `crates/hares-envelope/src/thermal_solver/ports.rs:34–43` | `apply_port_radiant_inputs` filters to `indoor_zone_id` only, while `apply_port_sensible_inputs` iterates all zones. Non-indoor radiant contributions are silently discarded. Currently harmless (no equipment currently emits non-zero `radiant_gain_w` to non-indoor zones), but creates an asymmetric contract. Fix: either (a) document explicitly that radiant distribution is indoor-zone-only by design, or (b) extend to multi-zone by iterating each zone that has surfaces in `interior_lwr_zones` / `interior_solar_zones`. |
| F3 | **HIGH** | `crates/hares-envelope/src/thermal_solver/mod.rs:246` | `zone_sensible_breakdown_debug()` calls `apply_port_sensible_inputs` but not `apply_port_radiant_inputs`. The `after_port` slot in the returned `[f64; 6]` understates total zone-air contribution from equipment by the radiant-to-air fraction (`total_radiant × mean(1−rad_frac)`). Any BESTEST or analysis code that treats this snapshot as the total port contribution will read incorrect values (~21% low for typical 30% radiant fraction with 0.3 radiation_frac). Fix: add `apply_port_radiant_inputs` call in the debug path, or rename the slot to `after_port_convective` and document the gap. |
| F4 | **MEDIUM** | `crates/hares-envelope/src/thermal_solver/ports.rs:135–137` | Comment "Windows (input_index=None)" is factually wrong in the `distribute_radiant_solar_surfaces` path. In production, `interior_solar_zones` is built by `solver_builder.rs` which sets `input_index: Some(zone_air_idx)` for window surfaces (they get `zone_sensible_input_indices` as the fallback). The window exclusion from TMULT weighting is correct but works because `solar_absorptance = 0.0` for windows — not because of `None`. A future change setting non-zero `solar_absorptance` on windows would cause windows to participate in weighting AND receive a double-deposit to zone air. The comment misleads future maintainers. Fix: correct the comment to explain that exclusion is via `solar_absorptance = 0.0`. |
| F5 | **MEDIUM** | `crates/hares-envelope/src/thermal_solver/longwave.rs:86` | `h_out > 1.0` threshold should be `> 0.0` (or `> 1e-9`). The intent is to use actual h_out when available and fall back to NFRC 34 when zero (opaque surface sentinel). A window with h_out between 0 and 1 W/(m²·K) — physically unusual but possible for a very high-R triple-pane vacuum-insulated glazing — would silently use NFRC 34, overstating the sky-depression correction by a factor of `34 / actual`. The sentinel for opaque surfaces is `0.0` (set in solver_builder.rs), not `≤ 1.0`. Fix: change `> 1.0` to `> 0.0`. |
| F6 | **MEDIUM** | `crates/hares-envelope/src/thermal_solver/ports.rs:99–107` | In `distribute_radiant_lwr_surfaces`: if `input_index >= u.len()` for a surface with non-zero weight, the surface RC deposit silently fails but `air_from_radiant += q × (1−rad_frac)` still accumulates the air fraction of that surface's gain. The `q × rad_frac` portion is lost. Result: energy misrouted to air rather than surface, but total conservation holds. The current codebase prevents this via consistent model construction, but there is no bounds-check assertion. The identical bounds check is done inline (`info.input_index < u.len()`) but the `air_from_radiant` accounting does not handle this case. Not a current bug path, but a silent energy misrouting risk for malformed models. |
| F7 | **MEDIUM** | `crates/hares-python/src/py_dwelling.rs` (not in diff scope, inferred from review) | `port_radiant_w` was added to `EnvelopeComponentGains` but is absent from the Python bindings `post_solvers` dict. Python users cannot observe radiant distribution totals. Telemetry gap prevents verification of the new radiant path from Python. |
| F8 | **MEDIUM** | `crates/hares-core/src/diagnostics.rs` (not in diff scope, inferred) | `port_radiant_w` is absent from the `EnvelopeDiag` diagnostic CSV. Users running simulations with `--diagnostics` cannot verify radiant distribution totals over time. |
| F9 | **MEDIUM** | `crates/hares-envelope/src/thermal_solver/ports.rs:87` and `ports.rs:133` | `distribute_radiant_lwr_surfaces` and `distribute_radiant_solar_surfaces` each allocate `Vec::with_capacity(surfaces.len())` for weights on every timestep. This violates the `feedback_hot_loop_minimal` policy. The allocation is small (n_surfaces × 8 bytes) but inconsistent with the pre-allocation pattern used everywhere else (`solar_absorbed_buf`, `lwr_net_flux_buf`, `lwr_surfaces_buf`, etc.). Pre-allocate `weights_buf: Vec<f64>` in `ThermalSolver` and clear/resize in these functions. |
| F10 | **LOW** | `crates/hares-equipment/src/scheduled_load.rs:249` | Default `radiative_gain_fraction = 0.0` when no `KEY_RADIATIVE_GAIN_FRACTION` config key is present. E+ defaults OtherEquipment `FractionRadiant` to 0.0 (pure convective), so this matches the standard default. However, ASHRAE residential appliance guidelines suggest ~30–50% radiant for most plug loads. If HPXML files omit `FracRadiant`, the simulation is fully convective for all plug loads. This is not a code bug but a documentation gap — the field should be prominently documented as requiring explicit HPXML configuration for accurate radiant partitioning. |
| F11 | **LOW** | `crates/hares-envelope/src/thermal_solver/mod.rs` (multiple test structs) | `boundary_diagnostics: Vec::new()` and `interior_solar_zones: Vec::new()` added to 15+ test struct literals are mis-indented (4-space extra indent) relative to surrounding fields. Not a runtime issue; `cargo fmt` resolves this. The project should enforce `cargo fmt` in CI. |
| F12 | **LOW** | `crates/hares-envelope/src/thermal_solver/mod.rs:551–556` | `internal_gain_cat_w` is computed as `sensible_for_category(InternalGain) + radiant_for_category(InternalGain)` (total sensible including radiant). `port_sensible_w` at mod.rs:504–506 is `t.sensible_gain_w` (convective-only). These two fields in `EnvelopeComponentGains` have inconsistent semantics: `internal_gain_w` is total, `port_sensible_w` is convective-only. The struct docs make this clear, but any code that sums them will double-count the convective portion. Consider renaming `port_sensible_w` → `port_convective_w` to make the asymmetry explicit. |
| F13 | **NIT** | `crates/hares-envelope/src/thermal_solver/mod.rs:228–255` | `zone_sensible_breakdown_debug()` returns `[f64; 6]` named `after_port` for the sixth slot. This slot captures only the zone-air index effect, so radiant-to-surface deposits are invisible. The function name suggests zone-sensible totals, but it includes non-sensible (outdoor temp) contributions. Acceptable as a debug function, but the name is misleading. |

---

## 9. Prior Review Claims Re-Verified

The prior version of this document (overwritten by this review) stated the following; each is re-verified against current HEAD:

**B1 (internal gain split 30/70)**: CONFIRMED CORRECT. Chain: `PortContribution.radiant_gain_w`, `ThermalAccumulator.add`, `apply_port_radiant_inputs`, TMULT distribution. Tests pass.

**B2 (window interior LWR OCHRE full mode)**: CONFIRMED CORRECT. `driving_temp.is_some()` routes `q × (1−rf)` to zone air; `q × rf` is carried by conduction. Tests pass.

**B6 (window exterior LWR T_eff)**: CONFIRMED CORRECT, with caveat F5 (`> 1.0` threshold should be `> 0.0`).

**B8 (interior solar absorptance 0.70)**: CONFIRMED CORRECT. `INTERIOR_SOLAR_ABSORPTANCE_DEFAULT = 0.70`, used in solver_builder at line 382.

**S6 (exterior solar absorptance 0.70)**: CONFIRMED CORRECT. `SOLAR_ABSORPTANCE_DEFAULT = 0.70`.

**D1 (lwr diagnostic Σ|q_i|/2)**: CONFIRMED CORRECT. `zone_exchange / 2.0` in longwave.rs:407.

**Parallel R_rad double-count removed**: CONFIRMED — opaque surface `rad_res_k_w` uses convection-only R_film. No parallel R_rad term.

**F1 (failing test)**: CONFIRMED FAILING. Test `heavyweight_concrete_wall_produces_two_rc_sub_layers` receives 5 nodes, asserts 4. This is a BLOCKER regression.

**F2 (multi-zone radiant asymmetry)**: CONFIRMED — `apply_port_radiant_inputs` filters to `indoor_zone` only. Latent (not active bug) but architectural inconsistency.

**F5 (h_out threshold)**: CONFIRMED — `> 1.0` should be `> 0.0`.

**F9 (hot-loop allocations)**: CONFIRMED — two `Vec::with_capacity` calls in `ports.rs`.

---

## 10. Findings-Document Errors Found in Prior Version

The prior version of `03_envelope_radiation_ports.md` (now replaced by this document) contained the following inaccuracies:

1. **F4 stated**: "windows always get `input_index: Some(zone_air_idx)`". The file path reference for this claim (`solver_builder.rs:818`) was not verified in the current diff. The claim about the exclusion mechanism (solar_absorptance=0.0 rather than input_index=None) is correct as an analysis of the production path, but the specific line numbers may drift. The finding stands as a maintainability concern.

2. **F7/F8** referenced specific files (`py_dwelling.rs:1939`, `diagnostics.rs:40`) that were not in the audit scope. These were inferred findings, not directly verified in the diff. The Python bindings and diagnostics files are outside the `c1abb8af..HEAD` diff. They are retained as MEDIUM findings but marked as inferred.

3. The prior review did not separately call out the hot-loop allocation violation (now F9) as a distinct numbered finding — it was buried in the "Code quality observations" section. It is a distinct policy violation per `feedback_hot_loop_minimal`.
