# Zone temperature update after thermal solver: flow to EnvironmentState before equipment step
**Review ID**: coredeep-05
**Category**: core-deep
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-core/src/environment.rs`
- `crates/hares-core/src/dwelling/mod.rs`
- `crates/hares-core/src/dwelling/conversions.rs`
- `crates/hares-core/src/dwelling/solver_builder.rs`
- `crates/hares-envelope/src/thermal_solver/mod.rs`
- `crates/hares-envelope/src/thermal_solver/config.rs`
- `crates/hares-types/src/environment.rs`
- `crates/hares-types/src/domain_solver.rs`
- `crates/hares-equipment/src/hvac/thermostat.rs`
- `crates/hares-equipment/src/hvac/helpers.rs`

## Vendor/Reference Files Consulted
None

## Findings

### Finding 1: [Severity: medium]
**Description**: Zone temperatures are written to two independent storage locations in `EnvironmentState` via separate code paths that could silently diverge. The thermal solver output lands in both `env.zones[].temperature_c` (written by `apply_thermal_update_to_zones`) and `env.custom_domains[THERMAL].zone_temperatures_c` (stored by `upsert_domain_ref`). Equipment thermostats read `env.zones[]` via `lookup_zone_temp` (`thermostat.rs:165`), while humidity and other domain solvers read `custom_domains[THERMAL]`. If `apply_thermal_update_to_zones` silently skips a zone (see Finding 2), equipment would use stale N-1 temperatures while the humidity solver uses the newly solved values, producing an inconsistent load computation.
**Code Location**:
  - Thermal update to `env.zones[]`: `dwelling/mod.rs:2557` calls `apply_thermal_update_to_zones` defined at `conversions.rs:470-476`
  - Thermal domain storage: `dwelling/mod.rs:2564` calls `upsert_domain_ref` defined at `hares-types/src/environment.rs:375-385`
  - Equipment read path: `hvac/thermostat.rs:163-169` (`lookup_zone_temp` reads `env.zones.iter().find()`) and `hvac/helpers.rs:61-69` (`lookup_zone` likewise)
  - Humidity solver reads `env.custom_domains` (containing the full `DomainUpdate`)
**Root Cause**: The N-1 temperature in `env.zones[]` and N temperature in `custom_domains` are derived from the same `self.thermal_update_buf`, but applied through independent mechanisms without a cross-check. A `clone_from` vs linear-search divergence in an edge case (e.g., zone added post-construction) would not be caught.
**Impact**: Under normal operation, both locations are consistent because they derive from the same `thermal_update_buf` applied in adjacent statements. However, the lack of a guard or assertion means a regression in either path would silently produce conflicting temperatures used by different subsystems.

### Finding 2: [Severity: low]
**Description**: `apply_thermal_update_to_zones` silently discards temperatures for zones whose `ZoneId` is not found in `env.zones`. The `find()` closure at `conversions.rs:472` returns `None` for unmatched zones, and the temperature is skipped without any diagnostic. This is a correctness hazard: if the thermal solver's `zone_temps_buf` ever contained a `ZoneId` absent from `env.zones` (e.g., from a stale solver rebuilt with different zone configuration), the mismatch would be invisible.
**Code Location**: `crates/hares-core/src/dwelling/conversions.rs:470-476`
**Root Cause**: `apply_thermal_update_to_zones` uses `if let Some(zone) = env.zones.iter_mut().find(|z| z.id == zone_id)` which silently ignores the `None` branch rather than logging a warning or bubbling an error.
**Impact**: Currently mitigated by the invariant that the thermal solver's `zone_output_indices` is built from the same `env.zones` vector at construction (`solver_builder.rs:639-647`). However, this is not enforced by the type system and would not survive a future feature that adds/removes zones dynamically.

### Finding 3: [Severity: low]
**Description**: `ThermalSolver::format_domain_update` at `thermal_solver/mod.rs:842-846` contains a silent fallback path: if a `ZoneId` in `zone_temps_buf` is not found in `wiring.zone_output_indices`, the zone's temperature in the output `DomainUpdate` retains the initial `indoor_temp_c` value set at solver construction (`thermal_solver/mod.rs:485-490`). This is functionally identical to a NaN or erroneous temperature from the numerical integration, but carries no diagnostic.
**Code Location**: `crates/hares-envelope/src/thermal_solver/mod.rs:842-846`
**Root Cause**: The `if let Some(&output_idx)` guard on line 843 silently preserves the pre-existing initial temperature when the lookup fails. Since `zone_temps_buf` is built from `wiring.zone_output_indices.keys()` (line 486), this branch is dead code in the current implementation, but it masks any future regression where the wiring is rebuilt after `zone_temps_buf` construction.
**Impact**: Not currently reachable, but constitutes a latent correctness risk.

### Finding 4: [Severity: low]
**Description**: `EnvironmentManager::feed_zones` is a no-op when called with an empty slice (`environment.rs:465: if !zones.is_empty()`). The intent is to retain the previous zone state buffer when the caller has no updated zones to provide. This is correct on the hot path (`run_timestep` always feeds non-empty zone data from `latest_env.zones`) but is counterintuitive to new developers who might expect `feed_zones(&[])` to clear the internal buffer. The 38 existing test calls that pass `&[]` to `update()` rely on this behavior.
**Code Location**: `crates/hares-core/src/environment.rs:464-469`
**Root Cause**: The clear-and-extend pattern is guarded by an `is_empty()` check with no explicit documentation of the "retain previous state" semantic in the function signature or doc comment.
**Impact**: No current bug, but the semantic is easy to misunderstand. A future contributor calling `feed_zones(&[])` expecting zone state to be cleared to empty would introduce stale-state bugs.

## Summary

The zone temperature update path from the thermal solver to `EnvironmentState` is **correctly ordered and complete** for the current architecture:

- **(a) All conditioned zones are updated**: The solver builder (`solver_builder.rs:639`) iterates all `env.zones` and assigns each a state row, output index, and sensible input index. `format_domain_update` (`thermal_solver/mod.rs:882-884`) populates `zone_temperatures_c` from all entries in `zone_temps_buf`, which is built from all keys in `zone_output_indices` — every conditioned zone has an entry.
- **(b) Unconditioned zones are also updated**: Garage, attic, foundation, and crawlspace zones are included in `env.zones` alongside conditioned zones. The solver builder's `for (zone_idx, zone) in env.zones.iter().enumerate()` loop at line 639 does not filter by `zone_type`, so unconditioned zones receive state rows and output indices identically to conditioned zones. Their temperatures are updated by the state-space step and written to `env.zones[]`.
- **(c) The update is atomic relative to equipment reads**: Equipment's `update_control()` and `step()` execute in Steps 1c–3a of `run_timestep`, reading `latest_env.zones[]` which contains the **previous timestep's** post-solver temperatures (N-1). The thermal solver's `integrate()` and the subsequent `apply_thermal_update_to_zones` occur at Step 4, AFTER all equipment has completed. Equipment sees N's solved temperatures on the **next** timestep. There is no interleaving of reads and writes within a single step.
- **(d) Zone temperature array index mapping is consistent**: The thermal solver's `zone_output_indices` maps each `ZoneId` to an output index matching its position in `env.zones` (`solver_builder.rs:643`). Equipment uses `find()` by `ZoneId` (`hvac/helpers.rs:63`) rather than positional indexing, so order differences are irrelevant. The `apply_thermal_update_to_zones` function also uses `find()` by `ZoneId` (`conversions.rs:472`), providing immunity to reordering.
- **Zone ID 0 (None/outdoor) does not leak**: Zone IDs are assigned 1-based in `initial_zones` (`environment.rs:898, 945`). `ZoneId(0)` appears only in a types-crate unit test (`hares-types/tests/type_interop.rs:93`), not in production. Outdoor temperature is handled via separate `B_c` matrix columns (`solver_builder.rs:648-651`) and never appears in `zone_state_rows` or `zone_output_indices`.

The four low-severity findings describe robustness gaps that would not produce incorrect results under the current implementation but could mask regressions introduced by future architectural changes (e.g., dynamic zone addition, solver reconfiguration mid-simulation).

- Total findings: 4
- Critical: 0 / High: 0 / Medium: 1 / Low: 3

## Recommendations

1. Add a `debug_assert!` or `tracing::warn!` in `apply_thermal_update_to_zones` when a `ZoneId` from the thermal solver is not found in `env.zones`, making zone-mapping bugs visible in debug builds.

2. Add a similar diagnostic in `format_domain_update` for the "zone not in zone_output_indices" branch, so the dead-code invariant is checked at runtime.

3. Consider a `debug_assert!` in `run_timestep` after the thermal solver step verifying that every zone in `env.zones` has a corresponding entry in `thermal_update_buf.zone_temperatures_c`, confirming all zones were updated.

4. Document the `feed_zones` "retains prior state on empty input" semantic in its doc comment to prevent future misunderstanding.

## References / Citations

- `dwelling/mod.rs:2232-2239`: Step 1 — environment update with prior-step zone temperatures
- `dwelling/mod.rs:2326-2330`: Step 1c — equipment `update_control()` reads temperatures
- `dwelling/mod.rs:2422-2455`: Step 3a — equipment `step()` executes
- `dwelling/mod.rs:2553-2564`: Step 4 — thermal solver integration and temperature application
- `environment.rs:464-469`: `feed_zones` implementation
- `environment.rs:588-590`: Zone state copy in `update_in_place`
- `environment.rs:877-955`: `initial_zones` — ZoneId = idx+1, no ZoneId(0) produced
- `conversions.rs:470-476`: `apply_thermal_update_to_zones`
- `solver_builder.rs:639-647`: Solver wiring — all zones receive output indices
- `thermal_solver/mod.rs:485-490`: `zone_temps_buf` initialisation from `zone_output_indices.keys()`
- `thermal_solver/mod.rs:842-846`: `format_domain_update` temperature extraction from output vector
- `thermal_solver/mod.rs:882-884`: `zone_temperatures_c` population for `DomainUpdate`
- `hvac/thermostat.rs:163-169`: `lookup_zone_temp` reads from `env.zones[]`
- `hvac/helpers.rs:61-69`: `lookup_zone` reads from `env.zones[]`
- `hares-types/src/environment.rs:375-385`: `upsert_domain_ref` stores `DomainUpdate` in `custom_domains`
