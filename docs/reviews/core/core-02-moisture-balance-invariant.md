# Moisture balance invariant registration and solver coupling gap
**Review ID**: core-02
**Category**: core
**Date**: 2026-05-26

## Files Reviewed
crates/hares-core/src/invariants.rs
crates/hares-envelope/src/humidity_solver.rs

## Vendor/Reference Files Consulted
None

## Findings
### Finding 1: [Severity: high]
**Description**: The moisture balance invariant check is a mathematical tautology when the default 15x moisture buffering multiplier is active — it proves only algebraic consistency of the solver implementation, not physical conservation of moisture mass. The invariant computes `delta_m = dW * rho * V * M` and checks it against `Q_latent * dt / h_fg`. Since the solver computes `dW = Q_latent * dt / (h_fg * rho * V * M)`, the two expressions are algebraically identical and the check passes by construction regardless of whether any moisture is actually conserved. The invariant would only fail if: (a) `h_fg` differs between solver and checker, (b) the humidity ratio was clamped to saturation, or (c) the semi-implicit infiltration path uses different logic. Clamped steps are explicitly skipped (dwelling/mod.rs:3207-3217), so condition (b) is itself excluded.

The thermal invariant (`check_thermal`) is similarly algebraic but is **deferred** (dwelling/mod.rs:3135-3142) with the comment "A zone-air-only balance has a ~6 kW residual because wall-mass energy changes aren't captured," acknowledging that algebra alone is insufficient when storage mechanisms are distributed. The moisture balance has the identical structural limitation — the 15x buffering multiplier represents material moisture storage — yet the moisture check is exercised on every timestep as if it validates physical conservation when it validates only arithmetic.

**Code Location**:
- Invariant check tautology: `dwelling/mod.rs:3223` computes `delta_m = d_w * rho_air * zone.volume_m3 * moisture_mult`
- Solver applies same multiplier: `humidity_solver.rs:306-308` computes `d_w = (Q*dt) / (h_fg * rho * V * M)`
- Thermal invariant deferred: `dwelling/mod.rs:3135-3142`
- Clamped zones skipped: `dwelling/mod.rs:3207-3217`

**Root Cause**: The invariant checker does not independently verify the moisture balance against a first-principles sink/source enumeration. It re-uses the same `moisture_buffering_multiplier` as the solver, making the check self-referential. A proper independent invariant would compute the expected `dW` from the sum of independently tracked moisture sources and sinks (occupant generation rate, dehumidifier condensation rate, infiltration exchange mass) and compare against the solver's `dW`, without relying on the solver's internal `M` parameter.

**Impact**: The moisture balance check provides a false sense of correctness. Any systematic error in the humidity solver's treatment of moisture sources, sinks, or the buffering multiplier itself will pass the check. A regression that doubles or halves the buffering multiplier would pass. A missing moisture source (e.g., plants) would pass as long as the solver's input and output are internally consistent.

---

### Finding 2: [Severity: medium]
**Description**: The thermal invariant (`check_thermal`) exists in `invariants.rs:31-55` but is never called from the engine loop. The `check_invariants` method in `dwelling/mod.rs:3051-3268` exercises five checks in order:
1. Zone/tank temperature bounds (`check_temperatures`)
2. SoC bounds (`check_soc`)
3. Electrical finiteness + balance (`check_electrical`)
4. Humidity payload finiteness
5. Moisture mass balance (`check_moisture`)

The thermal balance skip is explicitly documented (lines 3135-3142) as deferred because wall-mass storage is not tracked. However, the `check_thermal` function takes generic parameters (`q_gains`, `delta_e_storage`, `q_loss`) and could be called with zone-air-only quantities from `component_gains()` and zone temperature deltas today — the ~6 kW residual mentioned in the comment is itself a useful diagnostic. Running it at a higher tolerance (e.g., 10 kW) would catch catastrophic divergence without false positives.

**Code Location**: `dwelling/mod.rs:3135-3142` (deferral comment), `invariants.rs:31-55` (unused `check_thermal`).

**Root Cause**: The invariant checker was designed with per-domain methods but the thermal domain's wall-storage complexity was not resolved before the check was wired into the engine loop. The moisture check followed a simpler path because the humidity solver's 15x buffering multiplier collapses storage into a single scalar.

**Impact**: Detached thermal invariant means multi-node RC model divergence (e.g., from sign-flipped infiltration, incorrect conductance wiring, or B-matrix mis-ranking) goes undetected during integration. Only final temperature bounds catch gross excursions, not gradual energy drift.

---

### Finding 3: [Severity: medium]
**Description**: Condensation mass on cold surfaces is discarded from the moisture balance rather than tracked as an observable sink term. When `w_new` exceeds `w_sat` in the humidity solver (`humidity_solver.rs:283`), the humidity ratio is clamped to `w_sat`. The invariant check then skips the zone (`dwelling/mod.rs:3207-3217`) because the clamped value is either at zero or at saturation, meaning the mass of water vapor that condensed is silently removed from the balance with no record. In a residential simulation, window condensation during cold nights and bathroom mirror condensation after showers are physically real moisture sinks that should appear in the moisture inventory.

**Code Location**: `humidity_solver.rs:282-283` (clamping), `dwelling/mod.rs:3207-3217` (skipping clamped zones in invariant).

**Root Cause**: The humidity solver treats saturation as a hard bound but does not accumulate the condensed mass into a sink term that the invariant checker could validate. The condensation mass is `(w_raw - w_clamped) * rho * V` and should be added to a per-step condensation accumulator.

**Impact**: Moisture mass is silently destroyed in condensation events. The unaccounted mass can be significant in humid climates with cold surfaces (e.g., single-pane windows at 0°C outdoor temperature with indoor dew point at 12°C). Over a daily cycle, the missing condensation mass can accumulate to grams of water — far exceeding the 1e-6 kg invariant tolerance.

---

### Finding 4: [Severity: medium]
**Description**: Solver coupling is strictly sequential (thermal first, then moisture) with no iterative coupling between domains within a timestep. The engine loop at `dwelling/mod.rs:2553-2588` runs:
1. `thermal_solver.integrate()` — updates zone temperatures, emits latent/infiltration payload
2. `humidity_solver.resolve()` — consumes thermal payload, computes new humidity ratios
3. `electrical_solver.resolve()` — consumes updated state
4. `fluid_solver.resolve()` — consumes updated state
5. `custom_domain_solvers` — each in series

The humidity solver reads the thermal domain's latent gain and infiltration data from `env.custom_domains` (set by `upsert_domain_ref` at line 2564) and uses semi-implicit infiltration coupling for unconditional stability. However, the updated humidity ratios are not fed back to the thermal solver in the same timestep to correct the latent heat contribution that the thermal solver assumed (which was based on the previous step's humidity ratios from `PortSlots`). This means the thermal solver's infiltration latent load for step `n` is computed using step `n-1` humidity ratios, not step `n` ratios.

**Code Location**: `dwelling/mod.rs:2553-2588` (solver orchestration order), `humidity_solver.rs:122-150` (reads thermal payload from previous step's humidity context).

**Root Cause**: The domain solver architecture (`DomainSolver` trait) does not support iterative coupling. Each solver receives `(ports, env, dt)` and produces a `DomainUpdate` that subsequent solvers read from `custom_domains`. There is no mechanism for a solver to re-execute after a downstream solver has updated the environment. The semi-implicit infiltration treatment in the humidity solver mitigates stability concerns but does not close the latent heat feedback loop within a step.

**Impact**: At coarse timesteps (10-60 minutes), the one-step lag in latent heat coupling introduces a phase error in the humidity-temperature relationship. The effect is small (<1% for dt ≤ 60s at standard infiltration rates) but grows with timestep size and ACH rate. The test at `humidity_solver.rs:1279-1349` explicitly demonstrates that semi-implicit treatment is more accurate than explicit at coarse dt, but still has error relative to the analytical solution — the missing feedback amplifies this.

---

### Finding 5: [Severity: medium]
**Description**: The moisture buffering multiplier of 15x (humidity_solver.rs:23-24, 34) is documented as matching OCHRE's `humidity_cap_mult`, but OCHRE's original value was calibrated for a specific test house with mixed construction (drywall + hardwood floors + upholstered furniture). The multiplier is applied uniformly to all zones regardless of: (a) construction type (lightweight timber vs. masonry), (b) furnishing level (sparsely furnished vs. densely furnished), (c) presence of hygroscopic materials (books, carpets, wood paneling), (d) temperature dependence of sorption isotherms. The comment at line 22 states "15x matches OCHRE's humidity_cap_mult: furniture and building materials absorb moisture, slowing RH swings and preventing dehumidifier cycling." The test at `humidity_solver.rs:662-700` validates only that the multiplier scales dW linearly, not that 15x is physically correct.

**Code Location**: `humidity_solver.rs:23-24`, `humidity_solver.rs:34`, `humidity_solver.rs:757-795` (test pinning default).

**Root Cause**: The moisture buffering model inherits OCHRE's bulk empirical factor without providing a mechanism to tune it per dwelling. The `HumiditySolverConfig` struct exposes `moisture_buffering_multiplier` as a configurable field but no HPXML input maps to it, and no construction-type-based lookup table is provided.

**Impact**: For lightweight construction (timber frame with minimal drywall, no carpets), a 15x multiplier may over-buffer by 5-10x, causing the humidity solver to underestimate RH swings by the same factor. For heavyweight construction (concrete/brick with plaster), 15x may under-buffer. The solver would mispredict dehumidifier cycling frequency and latent cooling loads accordingly.

---

### Finding 6: [Severity: low]
**Description**: The moisture balance tolerance in `invariants.rs:97` is a fixed absolute `1e-6` kg, unlike the thermal balance which uses `max(1.0, 1e-6 * gross_flux)` to scale with problem magnitude (`invariants.rs:44-45`). A 200 m³ zone at standard conditions contains ~240 kg of dry air carrying ~2 kg of water vapor at 50% RH. A `1e-6` kg tolerance corresponds to a humidity ratio precision of `~4e-9` kg/kg, which is near IEEE 754 f64 round-off error for values in the `[0.001, 0.030]` kg/kg range. The tolerance is approximately 3-4 orders of magnitude tighter than physically meaningful.

**Code Location**: `invariants.rs:97` (absolute tolerance), compared to `invariants.rs:44-45` (relative tolerance in thermal check).

**Root Cause**: The moisture invariant hard-codes an absolute tolerance without considering the scale of moisture mass in realistic zones. The thermal invariant's pattern of using `max(absolute_floor, relative * gross_flux)` provides both a baseline centering and scaling with problem magnitude.

**Impact**: In practice the tautology (Finding 1) masks this since the check always passes. If the invariant were reworked to be an independent conservation check, the tolerance would need to be relaxed to something like `max(1e-4, 1e-4 * gross_moisture_mass_kg)` to avoid false positives from floating-point noise.

---

### Finding 7: [Severity: low]
**Description**: The humidity solver does not assert that its received `dt` matches the thermal solver's configured `dt_s`. The thermal solver has debug assertions checking this (`thermal_solver/mod.rs:955-960` and `973-983`), but the humidity solver silently accepts any `Duration`. If a caller accidentally passes a different timestep, the humidity solver would produce silently incorrect results rather than panicking in debug mode.

**Code Location**: `humidity_solver.rs:104-111` (no dt validation in `resolve`), compared to `thermal_solver/mod.rs:955-960` (dt validation in `integrate`).

**Root Cause**: The `DomainSolver::resolve` trait method receives `dt: Duration` but the humidity solver does not validate it against a configured or previously recorded timestep.

**Impact**: Low — the engine loop at `dwelling/mod.rs` passes the same `dt` to all solvers from the simulation clock. A bug would require a new caller path. Adding a debug assertion would be defensive and consistent with the thermal solver's pattern.

---

### Finding 8: [Severity: low]
**Description**: The moisture invariant check uses the hard-coded constant `2_501_000.0` (the same as `H_FG_J_KG` in invariants.rs and `h_fg_j_kg` in HumiditySolverConfig) at `dwelling/mod.rs:3254` and `dwelling/mod.rs:3259`. These are separate from the solver's configurable `h_fg_j_kg` and the `H_FG_J_KG` in invariants.rs. If the constant were ever changed in one location but not others, the moisture check would break silently. A regression test (`humidity_solver.rs:706-716`) validates the two solver constants match, but does not validate the hard-coded constants used in the invariant check body.

**Code Location**: `dwelling/mod.rs:3254`, `dwelling/mod.rs:3259`, `invariants.rs:96`.

**Root Cause**: The latent heat constant is duplicated in at least 4 locations: `hares_physics::constants::LATENT_HEAT_VAPORISATION_0C_KJ_KG`, `HumiditySolverConfig::default().h_fg_j_kg`, `invariants.rs::check_moisture::H_FG_J_KG`, and hard-coded in `dwelling/mod.rs`. There is no single source of truth or compile-time verification that all four agree.

**Impact**: Low — the regression test at `humidity_solver.rs:706-716` and the tautological nature of the check mitigate the risk. If the constant diverged, the test would catch it and the invariant would fail (not silently pass).

## Summary
- Total findings: 8
- Critical: 0 / High: 1 / Medium: 4 / Low: 3

## Recommendations
1. **Independent moisture invariant**: Rework `check_moisture` to independently compute expected moisture mass change from tracked moisture sources and sinks (occupants, dehumidification, infiltration mass exchange, condensation mass), comparing against the solver's `dW * rho * V` (without multiplying by `moisture_buffering_multiplier`). The residual between the two represents the net sorption/desorption by building materials — a physically meaningful quantity that should be monitored and bounded, not eliminated algebraically.
2. **Accumulate condensation mass**: When the humidity solver clamps `w_new` to `w_sat` or zero, compute and store the discarded mass as a condensation or evaporation term. Export it in the humidity `custom_payload` so the invariant checker can include it in the moisture inventory.
3. **Wire the thermal invariant**: Call `check_thermal` with zone-air-only quantities at a coarse tolerance (e.g., 5-10 kW) to catch catastrophic divergence. Gate it behind a higher-level feature flag if wall-storage residuals would false-positive in normal operation.
4. **Consider iterative coupling**: For simulations with high infiltration rates (ACH > 5) and coarse timesteps (dt > 300s), implement an optional fixed-point iteration between thermal and humidity solvers within each timestep. The semi-implicit infiltration treatment already provides the mathematical framework — the iteration would converge the latent feedback.
5. **Make buffering multiplier construction-aware**: Map HPXML construction types (wood frame, masonry, SIP) to appropriate buffering multipliers, or expose the multiplier as a configurable input. A sensitivity analysis on the 15x default should be documented.
6. **Unify h_fg constant**: Use the `hares_physics::constants` value as the single source of truth, referencing it from `invariants.rs`, `dwelling/mod.rs`, and `HumiditySolverConfig` to eliminate the 4-way duplication.
7. **Relative moisture tolerance**: Adopt the thermal invariant's pattern of `max(floor, relative * gross_flux)` for moisture, with a floor of `1e-4` kg and a relative factor of `1e-6` applied to the total moisture mass inventory to prevent false positives.

## References / Citations
- `humidity_solver.rs:21-35`: Moisture buffering multiplier configuration, documentation referencing OCHRE's `humidity_cap_mult`
- `dwelling/mod.rs:3135-3142`: Thermal balance deferral comment acknowledging wall-storage limitation
- `dwelling/mod.rs:3223`: `delta_m` computation in invariant that makes the check self-referential
- `humidity_solver.rs:282-283`: Saturation clamping that discards condensation mass
- `humidity_solver.rs:706-716`: Latent heat constant consistency test between solvers
- `humidity_solver.rs:1279-1349`: Semi-implicit vs. explicit accuracy comparison at coarse timestep
- `invariants.rs:44-45`: Thermal invariant's relative tolerance pattern (recommended for moisture)
