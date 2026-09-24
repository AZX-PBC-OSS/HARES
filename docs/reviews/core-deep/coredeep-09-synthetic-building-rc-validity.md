# Synthetic building RC network validity: physically plausible geometry, materials, specs
**Review ID**: coredeep-09
**Category**: core-deep
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-core/src/dwelling/synthetic.rs` (1227 lines) — primary review target
- `crates/hares-core/src/dwelling/conversions.rs` (1625 lines) — Building → BoundaryInput conversion
- `crates/hares-core/src/dwelling/mod.rs` (6041 lines) — Dwelling constructor, `from_toml_config`
- `crates/hares-core/src/dwelling/solver_builder.rs` (2047 lines) — RC assembly → ThermalSolver
- `crates/hares-core/src/dwelling/autosize.rs` (894 lines) — equipment autosizing
- `crates/hares-core/src/rng.rs` (58 lines) — deterministic RNG derivation
- `crates/hares-envelope/src/boundary_rc.rs` (3326 lines) — RC network assembly, zone capacitance
- `crates/hares-envelope/src/rc_network.rs` (907 lines) — RC graph validation
- `crates/hares-envelope/src/state_space.rs` (1758 lines) — discretization, eigenvalue stability
- `crates/hares-io/src/hpxml/validation.rs` (710 lines) — WWR and window property validation
- `tests/fixtures/bestest/600.toml` — representative BESTEST fixture
- `benches/common.rs` — benchmark synthetic TOML construction

## Vendor/Reference Files Consulted
None

## Findings

### Finding 1: [Severity: high] Window area is not deducted from host wall area
**Description**: When windows are attached to a wall via `attached_to_wall_id`, the synthetic builder materializes each window as a separate `Boundary` (lines 693-718) but does NOT subtract the window area from the host wall's `area_m2`. In the BESTEST 600 fixture, the south wall has area 9.6 m² but has two windows of 6.0 m² each (12 m² total), yielding a per-wall window-to-wall ratio of 125% — physically impossible.
**Code Location**: `synthetic.rs:693-718` (window-as-boundary push) and `synthetic.rs:589-668` (boundary loop — no area reduction)
**Root Cause**: The `attached_to_wall_id` field on `SyntheticWindowConfig` is written into the `Window.attached_to_wall_id` field but is never consumed for geometry reconciliation. The HPXML validation layer (`validation.rs:287-311`) checks aggregate WWR only, and the 12/63.6 ≈ 0.19 aggregate ratio passes the validation range [0.02, 0.40], masking the per-wall anomaly.
**Impact**: The total envelope surface area is overestimated by the window area (12 m² excess in BESTEST 600 — a 7.5% envelope area error). The thermal solver treats the wall and window as parallel heat-flow paths in the same surface area, effectively double-counting the window area for both opaque conduction and glazing solar gains. This inflates overall UA and produces biased heating/cooling loads in every BESTEST benchmark.

### Finding 2: [Severity: high] Default synthetic building has an incomplete thermal envelope
**Description**: When no `[[boundaries]]` are specified in the TOML, `build_synthetic_building` constructs exactly ONE boundary — a single vertical wall of `wall_area_m2` (default 120 m²) connecting conditioned zone to outdoor (lines 643-668). There is no roof, no floor, and no other wall surface. The zone connects to outdoor through a single surface whose area is independently configured and bears no relationship to the zone volume or floor area. This is an open, non-physical thermal envelope.
**Code Location**: `synthetic.rs:643-668`
**Root Cause**: The default branch (lines 643-668) is a convenience path used by benchmarks (`benches/common.rs:139-175`) and some tests. It constructs the minimal valid `Building` struct — a single wall boundary — without enforcing envelope closure. The `floor_area_m2` and `wall_area_m2` are independently configurable and may bear no geometric relationship (e.g., floor_area=48 m² and wall_area=120 m² implies a wall 2.5× the floor area, which for a cube-like dwelling should be closer to 1.0×).
**Impact**: Benchmarks built on this default path use physically inconsistent geometry, producing thermal dynamics that do not represent any real building. Downstream RC network construction processes this as a valid input, creating an RC graph with a single resistive path and no wall thermal mass (since `material_layers` is empty). This is not flagged at any validation stage.

### Finding 3: [Severity: high] Default fallback path has zero wall thermal mass
**Description**: The default synthetic boundary (lines 643-668) has `material_layers: Vec::new()` combined with `assembly_r_value_m2_k_w: Some(wall_r_value)`. In the envelope conversion pipeline (`conversions.rs:210-213`), "when the boundary has explicit material layers ... skip LUT" — but for empty material layers, it falls through to the envelope LUT lookup at lines 214-265. For synthetic fixtures that use explicit TOML material layers (like BESTEST 600), these layers ARE used (lines 210-213 of conversions.rs). However, for the default single-wall path, material_layers is empty, so the fallback goes to the envelope LUT, which is `DefaultsStore::empty()` in many test/benchmark paths — resulting in `precomputed_rc = Vec::new()`. At `boundary_rc.rs:743`, this triggers the single-resistance fallback path with zero capacitance nodes. The only thermal mass in the system is the zone air capacitance multiplied by the interior mass multiplier (7.0).
**Code Location**: `synthetic.rs:653` (`material_layers: Vec::new()`); `conversions.rs:210-265`; `boundary_rc.rs:743-859`
**Root Cause**: The synthetic default path sets no material layers. The wall has R-value but no C-value (thermal capacitance). While the zone air has capacitance via `derive_zone_capacitances` (including the 7× interior mass multiplier), real walls contribute 50-200 kJ/(K·m²) of additional thermal storage that critically damps temperature swings. Without it, the thermal response is unrealistically fast — a pure first-order RC decay rather than the higher-order response of a real building.
**Impact**: Synthetic benchmarks underestimate thermal inertia, producing faster temperature responses than real buildings. This biases all timing-dependent metrics (peak load timing, demand response valuation, thermal comfort exceedance hours). In extreme cases, a building with no wall thermal mass and a small zone volume could produce eigenvalues very close to the unit circle, causing near-instability (see Finding 7).

### Finding 4: [Severity: medium] Heating capacity default is not proportional to floor area
**Description**: The default heating capacity is `30.0 kBTU/h` = 30,000 BTU/h ≈ 8,793 W (line 331-332). This is a fixed scalar, independent of floor area, volume, insulation level, or climate zone. The capacity is written as `hvac_capacity_w: Some(...)` (line 772), which means the `Building` struct carries an explicit capacity — and the autosizing logic at `autosize.rs:119-121` is completely bypassed for all synthetic buildings.
**Code Location**: `synthetic.rs:331-332` (default capacity); `synthetic.rs:772` (capacity wired as `Some`, bypassing autosizing)
**Root Cause**: `hvac_capacity_w: Some(...)` signals to `solver_builder.rs:1124` that a user-supplied capacity exists, so the autosizing path is skipped. The synthetic generator always supplies a capacity (never `None`), meaning NO synthetic building benefits from the physically-grounded autosizing logic that uses design temperatures, building UA, and oversizing factors from ACCA Manual S.
**Impact**: A 48 m² BESTEST dwelling and a 200 m² synthetic dwelling both get 30 kBTU/h. This is acceptable for the 48 m² BESTEST case (~625 BTU/h per m² = slightly oversized, 1.4× typical rule-of-thumb), but for larger dwellings the system is undersized, and for smaller ones it's oversized. In cooling mode (line 458), the cooling capacity is hardcoded to the SAME value as the heating capacity, which is contrary to standard practice where cooling and heating are independently sized. This affects all synthetic benchmarks that use the default capacity.

### Finding 5: [Severity: medium] Material property ranges are not validated in the synthetic builder
**Description**: The TOML deserialization accepts any `f64` values for `wall_r_value_m2_k_w`, `shgc`, `u_factor_w_m2_k`, `density_kg_m3`, `specific_heat_j_kg_k`, and `conductivity_w_m_k`. None of these are validated for physical plausibility at parse time or at building construction time:
- `wall_r_value_m2_k_w` can be 0.0, negative, or NaN → produces infinite or undefined conductance
- `shgc` can be > 1.0 or < 0.0 → violates the second law of thermodynamics
- `u_factor_w_m2_k` can be 0.0 or negative → infinite or negative resistance
- `density_kg_m3` defaults to 0.0 (line 218) → zero thermal capacitance per layer
- `specific_heat_j_kg_k` defaults to 0.0 (line 227) → zero thermal capacitance per layer
**Code Location**: `synthetic.rs:70-71` (MaterialsConfig), `synthetic.rs:232-241` (WindowConfig), `synthetic.rs:205-229` (MaterialLayer defaults)
**Root Cause**: The TOML schema has no validation constraints. Downstream, the RC network constructor (`rc_network.rs:64-68`) rejects non-positive capacitances and resistances, and the HPXML validation layer (`validation.rs:329-338`) requires U-factor and SHGC on windows. However, the synthetic builder's own material property ranges are unchecked, so users can specify physically impossible buildings that fail late at RC construction with cryptic errors (e.g., `NonPositiveResistance` or `NonPositiveCapacitance`) rather than early with contextual guidance.
**Impact**: Silent creation of buildings with zero or near-zero thermal mass layers (when `density_kg_m3` or `specific_heat_j_kg_k` are omitted). Physically impossible SHGC values (>1.0) pass through to solar gain calculations, inflating cooling loads. An R-value of 0.0 would produce a `NonPositiveResistance` error only at RC network construction time, with no reference back to the TOML source.

### Finding 6: [Severity: medium] `ceiling_height_m` is never set for synthetic buildings
**Description**: The `Building` struct's `ceiling_height_m` field is always `None` in synthetic construction (line 783). Downstream code in `solver_builder.rs:955` defaults to 2.5 m. However, the actual ceiling height implied by the BESTEST 600 fixture is `zone_volume_m3 / floor_area_m2 = 129.6 / 48.0 = 2.7 m`. Any calculation using the default 2.5 m (e.g., natural ventilation effective area scaling, duct height assumptions) will be off by ~8% for this case.
**Code Location**: `synthetic.rs:783` (`ceiling_height_m: None`)
**Root Cause**: `ceiling_height_m` is never computed from `zone_volume_m3 / floor_area_m2` in the synthetic path, despite both values being available. Other synthetic paths (e.g., `conversions.rs:817-818`) similarly set it to `None`.
**Impact**: Minor dimensional errors in duct sizing, natural ventilation, and infiltration stack-effect calculations. The primary thermal capacitance (zone air mass) uses the correct volume via `conditioned_volume_m3`, so the core thermal dynamics are unaffected. The risk is low but the fix is trivial.

### Finding 7: [Severity: low] Eigenvalue stability check exists but no synthetic-level guard
**Description**: The RC network → state-space pipeline correctly checks stability: `StateSpaceModel::from_continuous()` at `state_space.rs:299-312` rejects unstable systems via Gershgorin bounds, returning `StateSpaceError::UnstableSystem`. The RC network constructor at `rc_network.rs:59-137` validates non-positive capacitance/resistance. However, there is NO stability guard at the synthetic building generation level — an implausible building (R=0 walls, zero mass, infinite UA) can be constructed and will only fail later during solver construction with an error that does not reference the TOML source.
**Code Location**: `state_space.rs:299-312` (existing stability guard); absence of guard in `synthetic.rs:327-793`
**Root Cause**: Separation of concerns — the synthetic builder produces a `Building` (a pure data structure), and the envelope pipeline performs validation. The existing validation is correct and robust (Gershgorin is conservative, and the fallback eigenvalue check at `state_space.rs:1019-1058` via `eigenvalue_check` uses full complex eigenvalues). The gap is only in error locality.
**Impact**: Low — unstable synthetic buildings are caught, just not at the most user-friendly point. The existing guard is sound: Gershgorin bound ≥ 1.0 + 1e-10 with a non-singular A_c produces a hard `Err`, and near-unity eigenvalues at `NEAR_UNITY_EIGENVALUE_THRESHOLD` (0.99) produce `tracing::warn!` diagnostics.

### Finding 8: [Severity: low] Floor area fallback uses hardcoded 2.5 m ceiling height
**Description**: When `floor_area_m2 <= 0.0`, the floor area is derived as `zone_volume_m3 / 2.5` (line 336). The constant 2.5 m is reasonable for residential buildings, but it differs from the DEFAULT_HEIGHT_M constant in `boundary_rc.rs:24` and the `unwrap_or(2.5)` fallback in `solver_builder.rs`. If these constants ever diverge, inconsistencies would be introduced.
**Code Location**: `synthetic.rs:336` (hardcoded `2.5`)
**Root Cause**: Magic number. `boundary_rc.rs` defines `DEFAULT_HEIGHT_M: f64 = 2.5` at line 24, which should be reused here via `use hares_envelope::boundary_rc::DEFAULT_HEIGHT_M`.
**Impact**: Low — the constants are currently equal. If someone changes the DEFAULT_HEIGHT_M value (e.g., to 2.4 m to match IECC standard assumptions), the synthetic builder would silently diverge.

### Finding 9: [Severity: low] `master_seed` is captured but not used for building geometry generation
**Description**: The `master_seed` field (line 162) is deserialized from TOML and passed through to `SimulationConfig.master_seed` (line 870 of mod.rs), where it seeds the per-dwelling RNG via `derive_dwelling_rng` (line 931 of mod.rs). This RNG is used for environment/schedule resampling — NOT for building geometry generation. The building geometry is purely deterministic from the TOML config. While this is correct (building geometry should be deterministic), the naming of `master_seed` under `output` (rather than `simulation`) and its placement in `SyntheticOutputConfig` is misleading — it suggests the seed controls output randomness when it actually controls the environment's stochastic resampling.
**Code Location**: `synthetic.rs:162` (field in SyntheticOutputConfig); `mod.rs:931` (actual usage)
**Root Cause**: The seed exists in `SyntheticOutputConfig` alongside output format/chunk settings, but its primary consumer is the simulation engine's environment initialization, not output generation.
**Impact**: Low. Building generation IS deterministic — benchmarks ARE reproducible — but the configuration schema could confuse users who think the seed affects building properties. The seed correctly ensures that two runs with the same `master_seed` and TOML config produce identical simulation outputs.

## Summary
- Total findings: 9
- Critical: 0
- High: 3 (window area not deducted from walls, incomplete default envelope, zero wall thermal mass in default path)
- Medium: 3 (heating capacity not proportional to floor area, unvalidated material properties, ceiling_height_m never set)
- Low: 3 (stability guard at wrong abstraction layer, magic 2.5 constant, seed configuration semantics)

## Recommendations

1. **Deduct window area from host wall area** in `build_synthetic_building`: after the window loop (line 688), iterate over boundaries and subtract each window's area from the boundary whose `id` matches `window.attached_to_wall_id`. Emit a warning if a window's area exceeds its host wall area.

2. **Validate the default single-wall path**: either require `[[boundaries]]` for any non-trivial test/benchmark, or auto-generate six faces (four walls, roof, floor) from floor area and ceiling height for the default path. At minimum, add a `tracing::warn!` when the default incomplete envelope is used.

3. **Add thermal mass to the default path**: when `material_layers` is empty, populate at least one capacitance-bearing layer using the `wall_r_value_m2_k_w`, default material properties (concrete: ρ=2400, c_p=880), and a typical thickness derived from `R = d/k`. This gives the default building physically plausible thermal inertia.

4. **Make equipment capacity proportional to floor area**: derive default `heating_capacity_kbtu_h` from `floor_area_m2 * typical_load_w_per_m2 / 293.07` (e.g., 50 W/m² for moderate climate → 48 m² × 50 = 2,400 W ≈ 8.2 kBTU/h). Or, better, set `hvac_capacity_w: None` in the synthetic builder so the autosizing logic is exercised, computing capacity from actual building UA and design temperatures. This would bring synthetic benchmarks into alignment with HPXML-building behavior.

5. **Validate material properties at parse time**: add range checks in the synthetic config parse or in `build_synthetic_building`:
   - `wall_r_value_m2_k_w > 0.0` (reject 0 or negative)
   - `0.0 < shgc <= 1.0`
   - `u_factor_w_m2_k > 0.0`
   - `conductivity_w_m_k > 0.0` for any material layer with thickness > 0
   - `density_kg_m3 >= 0.0` and `specific_heat_j_kg_k >= 0.0`

6. **Set `ceiling_height_m` from volume / floor area**: add `ceiling_height_m: Some(config.geometry.zone_volume_m3 / floor_area)` at line 783.

7. **Reuse `DEFAULT_HEIGHT_M` constant**: replace the magic `2.5` at line 336 with `hares_envelope::boundary_rc::DEFAULT_HEIGHT_M`.

## References / Citations
- ASHRAE HoF 2021 §1.8 Eq.28 — ideal gas law for zone air density (used in `derive_zone_capacitances` at `boundary_rc.rs:397-426`)
- ASHRAE 90.1-2019 §6.4.3.1.1 — default indoor design setpoints (used in autosize.rs:27-28)
- ACCA Manual S-2017 §4 — equipment oversizing factors (used in autosize.rs:32-33)
- EnergyPlus Engineering Reference, Sky Radiation Modeling — Berdahl-Martin sky emissivity (used in synthetic.rs:820-835)
- Kusuda & Achenbach (1965), ASHRAE Trans. 71(1):61-74 — deep ground temperature model (referenced in synthetic.rs:103-108)
- Incropera & DeWitt, Fundamentals of Heat and Mass Transfer §5.8 — diurnal penetration depth (used in boundary_rc.rs:57-81)
- ISO 13786:2007 §6.2 — dynamic thermal characteristics, diffusion-length criterion (referenced in boundary_rc.rs:75-76)
- EnergyPlus InputOutputRef, ZoneCapacitanceMultiplier — mutual exclusivity with InternalMass objects (referenced in conversions.rs:29-33)
- ASHRAE HoF 2021 Ch. 18.31 — F-factor perimeter method for slab-on-grade (used in conversions.rs:158-165)
