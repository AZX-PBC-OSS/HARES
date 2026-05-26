# Full-system energy balance not in public telemetry
**Review ID**: envelope-03
**Category**: envelope
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-envelope/src/thermal_solver/mod.rs`
- `crates/hares-envelope/src/thermal_solver/stepping.rs`
- `crates/hares-core/src/telemetry.rs`

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Models/Envelope.py`
- `vendors/EnergyPlus/src/EnergyPlus/ZoneTempPredictorCorrector.cc`

## Findings

### Finding 1: Per-zone energy balance residual computed but discarded before public telemetry [Severity: high]

**Description**: The per-zone energy balance residual `|C·ΔT/dt − q_port|` is computed in `integrate_inner` at `stepping.rs:534-548`, stored in `self.energy_balance_residuals: HashMap<ZoneId, f64>`, serialized into the thermal domain's `custom_payload` at `mod.rs:870-875` as the 5th element of each 5-float chunk, and then explicitly discarded with a `let _residual` binding at `dwelling/mod.rs:3182`. The `DwellingTelemetry` struct in `telemetry.rs:11-28` has no residual field, and `to_observation_vec` at `telemetry.rs:32-108` has no key path to retrieve it. As a result, any user — whether operating the building model, running a reinforcement learning policy, or validating simulation results — has zero programmatic access to the heat balance closure check per timestep.

**Code Location**:
- Computation: `crates/hares-envelope/src/thermal_solver/stepping.rs:534-548`
- Storage in solver: `crates/hares-envelope/src/thermal_solver/mod.rs:134`
- Custom payload serialization: `crates/hares-envelope/src/thermal_solver/mod.rs:870-875`
- Explicit discard: `crates/hares-core/src/dwelling/mod.rs:3182` (`let _residual = quint[4];`)
- Public API gap: `crates/hares-core/src/telemetry.rs:11-28` (no `energy_balance_residuals` field)
- Observation path gap: `crates/hares-core/src/telemetry.rs:32-108` (no `zone_energy_balance[...]` key)

**Root Cause**: The residual was added to the custom_payload format for observability (as documented in the comment at `dwelling/mod.rs:3174-3176`), but the downstream consumer in the dwelling was only interested in the moisture coupling fields (quint elements 1–3). The residual was extended into the payload with explicit intent ("carries the per-step energy balance residual for observability") but never plumbed through to the telemetry struct. There is no intermediate `to_observation_vec` key for `zone_energy_balance[<name>]`.

**Impact**: Users cannot verify energy conservation without instrumenting the solver internals. This defeats the purpose of computing the residual in the first place. The only observable signal the residual has is a `tracing::warn!` at `stepping.rs:559-568` when it exceeds 5 kW, but that threshold is calibrated for "gross port-wiring errors" and fires only on extreme transients. For RL-based control, energy balance verification is critical (if a policy sees coherent-looking but non-conservative telemetry, it can learn to exploit the imbalance). For validation users, comparing HARES against EnergyPlus or ASHRAE 140 requires per-timestep closure quality metrics.

### Finding 2: Full-system stored energy diagnostic is debug-only [Severity: medium]

**Description**: The full-system stored energy change rate (sum over all thermal node capacitances) is computed at `stepping.rs:589-600` but is only emitted via `tracing::debug!(stored_energy_w, ...)` — a tracing call that does nothing in release builds unless the `debug` level is explicitly enabled. There is no path for this quantity to reach `DwellingTelemetry`, `to_observation_vec`, or even the `custom_payload`. This is less severe than Finding 1 because the ZOH discretization conserves energy by construction, but for multi-node RC models during transients, the stored-energy rate is the very quantity that reconciles wall-mass redistribution with the zone air balance.

**Code Location**: `crates/hares-envelope/src/thermal_solver/stepping.rs:589-600`

**Root Cause**: The full-system diagnostic was added as a development/debugging aid without consideration for public telemetry. It is explicitly documented as "a diagnostic — not an assertion or correction" (stepping.rs:580), but that comment addresses its role in the solver, not its observability value for users.

**Impact**: Users cannot distinguish whether a per-zone residual of, say, 3 kW is genuine imbalance or transient storage in wall-mass capacitances. Access to both the per-zone residual and the full-system stored-energy rate would let users cross-validate that `Σ(per-zone residual) ≈ stored_energy_w` minus through-envelope losses, providing confidence in the model's energy closure.

### Finding 3: EnergyPlus exposes per-component heat balance terms; HARES does not [Severity: low]

**Description**: EnergyPlus exposes per-zone heat balance component terms (`SumIntGains`, `SumHADTsurfs`, `SumMCpDTzones`, `SumMCpDtInfil`, `SumMCpDTsystem`, `SumNonAirSystem`, and `CzdTdt`) as `Output:Variable` quantities, and computes the net imbalance `imBalance` as their summation (ZoneTempPredictorCorrector.cc:5662-5663). The imbalance is compared against a dynamic threshold (20% of the quadrature sum of components) and reported via recurring warnings. HARES computes `component_gains: EnvelopeComponentGains` with similar breakdown granularity, but these individual components are not exposed through public telemetry either — only the zone temperature vector.

**Code Location**:
- HARES component breakdown: `crates/hares-envelope/src/thermal_solver/stepping.rs:400-510` (zone energy closure check); `mod.rs:754-810` (component_gains population)
- EnergyPlus comparison: `vendors/EnergyPlus/src/EnergyPlus/ZoneTempPredictorCorrector.cc:5661-5688`

**Impact**: This is lower severity because `component_gains` is indirectly available through `ThermalSolver::component_gains()` (mod.rs:238-240), but that method requires mutable access to the solver, which is not available in typical telemetry-consumption scenarios. Users wanting to compare HARES zone heat balance components against EnergyPlus results would need to fork the dwelling run loop.

## Summary
- Total findings: 3
- Critical: 0
- High: 1 (Finding 1 — energy balance residual computed but discarded)
- Medium: 1 (Finding 2 — full-system stored energy debug-only)
- Low: 1 (Finding 3 — per-component heat balance terms not exposed)

## Recommendations

1. **Add `energy_balance_residual_w` to `DwellingTelemetry`.** Add a `pub energy_balance_residuals: Vec<f64>` field parallel to `zone_temperatures_c` (same zone ordering), populated from the custom_payload quint[4] that is currently discarded at `dwelling/mod.rs:3182`. This is a ~10-line change: add the field to the struct, collect the values during `telemetry()` construction, and update `telemetry.rs` tests.

2. **Extend `to_observation_vec` with a `zone_energy_balance[<name>]` key.** Following the same pattern as `zone_temp[<name>]` and `setpoint_heat[<name>]` in `telemetry.rs:54-59` and `telemetry.rs:61-67`, add a bracket-key parser for zone-level energy balance residual. This lets RL policies select it as an observation channel without changing the flat struct layout.

3. **Promote the full-system stored-energy rate from `tracing::debug!` to a telemetry field.** Add `pub stored_energy_w: f64` to `DwellingTelemetry` and pass it through `custom_payload` (e.g., as a 6th element per zone, or as a single trailing scalar). This gives users the ability to reconcile per-zone residuals against total thermal storage for multi-node RC models.

4. **Consider exposing per-component gains through `to_observation_vec`.** The `EnvelopeComponentGains` struct already carries `window_solar_w`, `opaque_solar_lwr_w`, `infiltration_w`, `hvac_heating_w`, `hvac_cooling_w`, etc. Adding bracket keys like `component_gain[<zone>,solar]` would enable direct comparison against EnergyPlus `Output:Variable` quantities without requiring solver-internal access.

## References / Citations
- EnergyPlus Engineering Reference, "Basis for the Zone and Air System Integration" — heat balance method energy conservation requirement
- ASHRAE HoF 2021 Ch.18 — zone heat balance fundamental requirement
- EnergyPlus `ZoneTempPredictorCorrector.cc:5661-5688` — `imBalance` computation and threshold-based warning
- HARES stepping.rs:512-531 — zone energy balance closure check comment citing EnergyPlus Engineering Reference
- HARES stepping.rs:571-583 — full-system stored energy diagnostic comment citing the same reference
- HARES mod.rs:863-870 — custom_payload format documentation in `format_domain_update`
- HARES dwelling/mod.rs:3174-3176 — custom_payload consumption comment acknowledging residual for observability
