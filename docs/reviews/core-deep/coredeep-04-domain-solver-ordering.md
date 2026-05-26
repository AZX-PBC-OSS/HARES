# Domain solver execution order: thermal→electrical→humidity→fluid, coupling errors
**Review ID**: coredeep-04
**Category**: core-deep
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-core/src/dwelling/mod.rs` — `run_timestep()` solver execution (lines 2209–2815), `apply_thermal_update_to_zones`/`apply_humidity_update_to_zones` (conversions.rs:470–509)
- `crates/hares-envelope/src/thermal_solver/mod.rs` — `integrate()` + `resolve()` (lines 954–985)
- `crates/hares-envelope/src/humidity_solver.rs` — `resolve()` (lines 104–159)
- `crates/hares-envelope/src/electrical_solver.rs` — `resolve()` (lines 106–133)
- `crates/hares-envelope/src/fluid_solver.rs` — `resolve()` (lines 111–178)
- `crates/hares-core/src/dwelling/conversions.rs` — `apply_thermal_update_to_zones` (line 470), `apply_humidity_update_to_zones` (line 478)

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/SimulationManager.cc` — `ManageHeatBalance` re-simulation logic (lines 2910–3009), calling tree documentation (lines 2921–2950)
- `vendors/EnergyPlus/src/EnergyPlus/ZoneTempPredictorCorrector.hh` — EnergyPlus's Predictor-Corrector iteration interface
- `vendors/EnergyPlus/src/EnergyPlus/HeatBalanceAirManager.hh` / `HeatBalanceSurfaceManager.hh` — surface-to-air coupling paths

## Findings

### Finding 1: Solver execution order swaps electrical and humidity vs prescribed order [Severity: medium]
**Description**: The prescribed solver execution order is thermal → electrical → humidity → fluid. The actual execution order in `run_timestep()` is thermal → humidity → electrical → fluid. Electrical and humidity positions are swapped.

**Code Location**: `crates/hares-core/src/dwelling/mod.rs:2577–2594`

```rust
// Step 4 actual order (lines 2553-2594):
self.thermal_solver.integrate(...);                          // line 2553
apply_thermal_update_to_zones(...);                          // line 2557
self.latest_env.upsert_domain_ref(&self.thermal_update_buf); // line 2564

self.humidity_solver.resolve(...);      // line 2577 — runs 2nd
self.electrical_solver.resolve(...);    // line 2583 — runs 3rd
self.fluid_solver.resolve(...);         // line 2589 — runs 4th
```

**Root Cause**: The ordering appears to be an implementation artifact rather than a reasoned design decision. The three non-thermal solvers are called in a single block with no explicit ordering commentary in the soruce code (lines 2577–2594). The `humidity_solver.resolve` is simply listed first in this block.

**Impact**: Currently **benign** because of the specific data dependencies:
- The **electrical solver** (`electrical_solver.rs:106–133`) only reads `ports.electrical` and `env.grid.voltage_pu`. It does NOT read zone temperatures, thermal domain data, or humidity data from `env.custom_domains`. Temperature-dependent load calculations (heat pump COP, battery efficiency, conductor resistance) happen in equipment `.step()` (Step 3a/3b, lines 2412–2517) and are deposited on `ports.electrical` BEFORE the electrical solver runs. So the electrical solver's results are independent of solver ordering.
- The **humidity solver** (`humidity_solver.rs:104–150`) reads the THERMAL domain from `env.custom_domains` (line 122) — which is already upserted at line 2564 before humidity runs. It does NOT read any data produced by the electrical or fluid solvers. So it is also independent of ordering relative to electrical/fluid.

However, this ordering mismatch is **fragile**: if either solver gains cross-coupling logic (e.g., humidity accounting for latent heat from electrical equipment loads, or electrical reading humidity for evaporative cooler power), it will silently use stale data from the wrong solver.

### Finding 2: One-step-lag for non-thermal solver cross-coupling within a timestep [Severity: medium]
**Description**: Within a single timestep, only the thermal solver's results are committed to `latest_env` before subsequent solvers run. The outputs of humidity, electrical, and fluid solvers are buffered in `DomainUpdate` structs and only upserted to `latest_env.custom_domains` after ALL non-thermal solvers have completed (lines 2609–2612). This means no non-thermal solver can consume another non-thermal solver's current-step output.

**Code Location**: `crates/hares-core/src/dwelling/mod.rs:2553–2612`

The feedback sequence is:

| Line(s) | Action | `latest_env` state after |
|---------|--------|------------------------|
| 2553–2564 | Thermal `integrate` → `apply_thermal_update_to_zones` → `upsert_domain_ref(THERMAL)` | Zones updated, THERMAL domain in custom_domains |
| 2577–2582 | Humidity `resolve` → output to `humidity_update_buf` | No change to latest_env |
| 2583–2588 | Electrical `resolve` → output to `electrical_update_buf` | No change to latest_env |
| 2589–2594 | Fluid `resolve` → output to `fluid_update_buf` | No change to latest_env |
| 2596 | `apply_humidity_update_to_zones` | Zones get humidity_ratio, RH, wet_bulb_c |
| 2609–2612 | `upsert_domain_ref` for humidity, electrical, fluid | All domains now in custom_domains |

This is a **commit-at-end** pattern: thermal results are committed eagerly (line 2557/2564), but all other domain results are committed at the end of Step 4. The thermal-update-eager pattern is correct and necessary because the humidity solver needs same-step zone temperatures from the thermal solver.

**Impact**: Currently mitigated by the narrow data dependencies noted in Finding 1. However, this design means:
- A heat pump water heater (HPWH) cannot see the same-step electrical panel load when deciding whether it can operate without overloading the panel — the electrical summary is one step stale (line 2253: `self.latest_env.electrical = self.prior_electrical_summary.clone()`).
- If a future coupling were added (e.g., humidity solver consuming electrical latent gains, electrical solver consuming humidity-driven fan power), it would silently operate with previous-step data, producing a systematic one-timestep phase lag in coupled dynamics.

### Finding 3: Thermal-to-humidity coupling path is correct and well-structured [Severity: low]
**Description**: The thermal solver produces a `DomainUpdate` that carries both zone temperatures and a custom payload with per-zone latent heat, infiltration mass flow rates, and outdoor humidity ratios. The humidity solver reads this domain data from `env.custom_domains` (line 122 of humidity_solver.rs) and uses it for semi-implicit moisture balance. This coupling is correctly sequenced: thermal runs first (line 2553), its update is upserted before humidity runs (line 2564), and the humidity solver reads it (line 2577).

**Code Location**:
- Thermal produces coupling data: `thermal_solver/integrate_inner` → payload format `[zone_id, q_latent_w, m_dot_inf_kg_s, w_outdoor, energy_balance_residual_w]`
- Thermal domain upserted: `mod.rs:2564`
- Humidity reads thermal domain: `humidity_solver.rs:122–149`

**Impact**: This is the intended design and works correctly. The moisture balance invariant check (`mod.rs:3046–3268`, `check_invariants`) validates this coupling path using the semi-implicit infiltration formulation.

### Finding 4: Electrical solver does not read zone temperatures or thermal domain data [Severity: low]
**Description**: The electrical solver (`electrical_solver.rs:106–133`) performs only ZIP load correction (multiplying port-accumulated load by a voltage-dependent polynomial) and net power summation. It does not read `env.zones`, `env.weather`, or `env.custom_domains` for any domain. Temperature-dependent loads (heat pump COP, battery efficiency, conductor resistance changes) are handled by individual equipment `.step()` methods during Steps 3a/3b (lines 2412–2517), which write their electrical power to `ports.electrical` before the electrical solver runs.

**Impact**: This is a design choice that keeps the electrical solver simple. It means the electrical solver is effectively a post-processor rather than a physics solver. However, it also means there is no system-level check for temperature-dependent electrical effects — individual equipment is responsible for self-consistency. A heat pump that computes COP using the zone temperature at equipment-step time (which is the temperature BEFORE thermal solver integration has run for the current step) may use a slightly stale temperature, though in practice the equipment step thermal stage (Step 3a) runs with the same `latest_env` as the thermal solver and writes to ports that the thermal solver then reads.

### Finding 5: No iterative convergence loop — compares unfavourably to EnergyPlus's predictor-corrector [Severity: low]
**Description**: EnergyPlus resolves zone heat balance using a Predictor-Corrector iteration pattern (`SimulationManager.cc:2987–3005`):
1. `ManageZoneAirUpdates` with `GetZoneSetPoints` — collect HVAC setpoints
2. `ManageZoneAirUpdates` with `PredictStep` — forward-predict zone temperature
3. `SimHVAC` — simulate HVAC response
4. If demand limiting is active, the entire heat balance (surfaces + air + HVAC) can be re-simulated (lines 2970–3008), iterating until convergence within the timestep.

HARES uses a single-pass sequential approach: each solver runs exactly once per timestep, with no feedback loop for demand-response coupling. The EnergyPlus approach ensures that within-timestep coupling (e.g., HVAC load changing because zone temperature changed because HVAC ran) is resolved to convergence. The HARES approach accepts that equipment-step effects will only be fully reflected in the NEXT timestep.

**Code Location**:
- HARES single-pass: `crates/hares-core/src/dwelling/mod.rs:2553–2594`
- EnergyPlus predictor-corrector: `vendors/EnergyPlus/src/EnergyPlus/SimulationManager.cc:2987–3005`

**Impact**: For residential simulation with typical 1–15 minute timesteps, a single pass is generally sufficient because the dominant thermal time constants (hours for building mass) are much longer than the timestep. However, for systems with fast dynamics (e.g., inverter-driven heat pumps that modulate within seconds, or battery/PV interactions at sub-minute resolution), the lack of iteration could produce observable error. The EnergyPlus approach also naturally handles demand-limiting scenarios where equipment must be throttled to stay within panel capacity — HARES cannot do this within a single timestep because the electrical solver runs after all equipment has already stepped.

## Summary
- **Total findings**: 5
- **Critical**: 0
- **High**: 0
- **Medium**: 2 (Findings 1, 2)
- **Low**: 3 (Findings 3, 4, 5)

## Recommendations
1. **Re-order non-thermal solvers to match the prescribed order** (thermal → electrical → humidity → fluid) in `run_timestep()` lines 2577–2594. While currently electrically benign, matching the documented order prevents future coupling bugs and makes the code self-documenting about intended data flow direction.

2. **Consider committing humidity results before running later solvers** if future couplings require it. Currently `apply_humidity_update_to_zones` runs at line 2596 (after all solvers). Moving it between humidity and electrical would allow electrical to see same-step humidity if that coupling is ever added.

3. **Document the commit-at-end pattern explicitly** in a block comment above the solver block (line 2550), noting which solvers consume which domains' data and that the one-step-lag is an intentional design tradeoff for simplicity over iterative convergence.

4. **Add an integration test** that verifies solver execution order is thermal-first and that thermal domain data is committed before humidity reads it. This would catch any future refactoring that accidentally reorders the solver block.

5. **Consider panel capacity constraint checking post-hoc** rather than pre-emptive: after all equipment steps and the electrical solver run, validate that net power does not exceed panel limits, and flag violations for the next timestep's actors to throttle.

## References / Citations
- EnergyPlus Engineering Reference, "Warmup Convergence" and "Zone Heat Balance Predictor-Corrector" sections
- EnergyPlus `SimulationManager.cc:2910–3009` — `ManageHeatBalance` re-simulation loop with Predictor-Corrector iteration
- `ZoneTempPredictorCorrector.hh` — `GetZoneSetPoints` and `PredictStep` enumeration defining the two-phase iteration
- HARES `humidity_solver.rs:104–150` — humidity solver reads THERMAL domain from `custom_domains`
- HARES `electrical_solver.rs:106–133` — electrical solver only reads ports and grid voltage; no cross-domain consumption
- ASHRAE Handbook of Fundamentals 2021, Chapter 18.4 — residential heat balance coupling requirements
