# Incomplete uom typed wrapper coverage
**Review ID**: types-physics-05
**Category**: types-physics
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/units.rs`
- `crates/hares-equipment/src/ports.rs`

## Additional Files Consulted
- `crates/hares-types/src/ports.rs` — Actual `PortContribution` / `PortSlots` / accumulator definitions
- `crates/hares-envelope/src/thermal_solver/ports.rs` — Solver-side port consumption
- `crates/hares-physics/src/psychrometrics.rs` — Typed-wrapper usage patterns
- `crates/hares-physics/src/air_properties.rs` — Typed-wrapper usage patterns
- `crates/hares-physics/src/constants.rs` — Physical constants with unit-annotated names
- `crates/hares-equipment/src/generator.rs` — Equipment port writing
- `crates/hares-equipment/src/water_heater/tank.rs` — Energy arithmetic patterns
- `crates/hares-equipment/src/water_heater/heat_pump_wh.rs` — Equipment port writing
- `crates/hares-equipment/src/hvac/furnace.rs`, `boiler.rs`, `air_conditioner.rs` — Equipment port writing
- `crates/hares-equipment/src/event_load.rs`, `scheduled_load.rs` — Equipment port writing

## Vendor/Reference Files Consulted
None

## Findings

### Finding 1: Typed uom wrappers are confined to hares-physics; all cross-crate interfaces use bare f64

**Severity**: high
**Description**: The `hares-physics/src/units.rs` module defines strongly-typed `uom` aliases (`Temperature`, `Power`, `Energy`, `Pressure`, `MassFlowRate`, `Length`, `Area`, `Volume`, `Velocity`, `HeatCapacity`) with documented policy that "Public API boundaries between crates should prefer typed uom quantities." However, these typed aliases are **never used outside `hares-physics` itself**. Every cross-crate interface — `PortContribution`, `PortSlots`, `ControlSignal`, `EnvironmentState`, `CoreOutput`, `HvacConfig`, `Equipment::step()`, `DomainSolver::resolve()` — uses bare `f64` with unit-convention suffixes in field/variable names.

**Code Location**:
- Type alias definitions: `crates/hares-physics/src/units.rs:27-36`
- Boundary policy documentation (not enforced): `crates/hares-physics/src/units.rs:1-7`
- PortContribution definition (bare f64): `crates/hares-types/src/ports.rs:57-91`
- PortSlots definition (bare f64): `crates/hares-types/src/ports.rs:427-435`
- Equipment trait signature (bare f64): `crates/hares-equipment/src/lib.rs` (step takes `&mut PortSlots`)
- DomainSolver trait signature: `crates/hares-types/src/domain_solver.rs` (resolve takes `&PortSlots`)
- Actual imports of typed wrappers from hares-physics (only 2 sites, both use plain f64 helpers): `crates/hares-core/src/dwelling/autosize.rs:20` and `crates/hares-io/tests/hvac_wiring_regressions.rs:17` — both import `temperature_f_to_c`/`temperature_c_to_f`, not `Temperature`/`Power` types.

**Root Cause**: The `PortContribution`, `PortSlots`, and accumulator types in `hares-types` were designed with bare `f64` fields. Because `hares-types` is the leaf crate with no internal dependencies, it cannot depend on `hares-physics` (which depends on `uom`). The dependency direction prevents using typed quantities at the shared-interface layer without either moving the port types into `hares-physics` or giving `hares-types` its own `uom` dependency.

**Impact**: The typed-uom investment provides zero compile-time protection at the interface between equipment and solvers. A developer could write `sensible_gain_w: electric_kw` (attaching a kW value to a watt-labeled field) and the compiler would not catch it. The current naming convention (`_w`, `_kw`, `_c`, `_f`) is purely documentary. All equipment models currently perform correct W/kW conversions (verified across furnace, boiler, AC, HP, water heaters, battery, generator, EV, PV), but this is maintained by developer discipline rather than compiler enforcement.

### Finding 2: Mixed unit conventions within `PortContribution` enum (W vs kW)

**Severity**: medium
**Description**: The `PortContribution` enum uses **different SI scales** for different port variants:
- `Thermal { sensible_gain_w: f64, radiant_gain_w: f64, latent_gain_w: f64 }` — **watts**
- `Electrical { active_power_kw: f64, reactive_power_kvar: f64 }` — **kilowatts / kVAR**
- `Fuel { consumption_w: f64 }` — **watts**

This mixing is deliberate and documented in field names, but it creates an inherent risk surface: a developer writing an equipment model must mentally switch between W and kW scales depending on which port variant they are populating. The W→kW conversions are all done inline with `/ 1_000.0` or `* 1_000.0` in every equipment file, each representing a manual conversion point.

**Code Location**:
- PortContribution definition: `crates/hares-types/src/ports.rs:57-91`
- Example conversion point (furnace): `crates/hares-equipment/src/hvac/furnace.rs:174-188` — `electric_kw = (gross_capacity_w * eir) / 1000.0`
- Example conversion point (boiler): `crates/hares-equipment/src/hvac/boiler.rs:224,565` — `electric_kw = thermal_output_w * eir / 1000.0`, then `jacket_loss_w = fuel_input_w + electric_kw * 1e3 - thermal_output_w`
- Example conversion point (HPWH): `crates/hares-equipment/src/water_heater/heat_pump_wh.rs:715` — `active_power_kw: electric_power_w / 1_000.0`

**Root Cause**: The electrical domain conventionally uses kW (grid-scale convention), while thermal and fuel domains use W (building-scale convention). No automated conversion layer exists — each equipment model manually bridges the two.

**Impact**: Each manual `/ 1000.0` or `* 1000.0` is a potential off-by-1000x error. A missed or doubled conversion would produce physically significant errors (reporting 1 kW as 1000 kW or vice versa). All current conversions were manually verified and found correct, but regression risk is high.

### Finding 3: No custom `Add`/`Sub`/`Mul`/`Div` implementations with unit propagation

**Severity**: medium
**Description**: The review instructions ask whether multiplying a `Temperature` and a `HeatCapacity` yields `Energy` (not bare `f64`). The answer is **no** — HARES defines no custom arithmetic trait implementations on the `uom` quantity types. The `uom` crate itself provides `Add`, `Sub`, `Mul`, and `Div` on individual quantity types (e.g., `Temperature + Temperature → Temperature`), but HARES does not implement cross-type arithmetic (e.g., `Temperature * HeatCapacity → Energy`). All such compositions are done with bare `f64` throughout the codebase.

**Code Location**:
- Type aliases: `crates/hares-physics/src/units.rs:27-36` — no trait impls follow
- Typical energy computation (water heater tank): `crates/hares-equipment/src/water_heater/tank.rs:281` — `let mcp = water_density_kg_m3(t_now) * vol * CP_LIQUID_WATER_J_KG_K` — all bare f64
- Temperature rise from power: `crates/hares-equipment/src/hvac/boiler.rs:229` — `return_temp_c + thermal_output_w / (self.flow_rate_kg_s * CP_LIQUID_WATER_J_KG_K)` — bare f64, result is °C but mathematically is `W / (kg/s * J/(kg·K)) → K`
- Tankless demand: `crates/hares-equipment/src/water_heater/tankless.rs:324` — `let demand_w = total_draw_kg_s * CP_LIQUID_WATER_J_KG_K * delta_t_c * duty` — produces W from kg/s × J/(kg·K) × K, only correct because of consistent SI units

**Root Cause**: This is a natural consequence of Finding 1 — since typed wrappers don't cross crate boundaries, there's no opportunity for cross-type arithmetic at the crate interface. Within `hares-physics`, where typed wrappers exist, there are no functions that combine different quantity types (e.g., the psychrometric functions operate on `Temperature` and `Pressure` but never multiply them together).

**Impact**: Developers performing physical computations (e.g., `mass_flow × specific_heat × delta_T → power`) must manually ensure dimensional correctness. A computation like `power_w / cp_j_kg_k` would produce a value in `K·kg/s` rather than `K`, but this kind of error would only be caught by the field name convention, not by the compiler.

### Finding 4: `IDLE_KW_THRESHOLD` constant name is misleading when used with watt-valued variables

**Severity**: low
**Description**: In `crates/hares-equipment/src/generator.rs`, the constant `IDLE_KW_THRESHOLD = 1e-6` (nominally 1e-6 kW = 0.001 W) is compared against variables that are explicitly in **watts**, creating a semantic unit-label mismatch. The numeric value (0.001 W) is functionally equivalent to "near-zero" in both contexts for real generator equipment, so no actual misbehavior occurs. However, the constant name implies kW-scale thinking when the comparison is against W-scale values.

**Code Location**:
- Constant definition: `crates/hares-equipment/src/generator.rs:216` — `const IDLE_KW_THRESHOLD: f64 = 1e-6;`
- Correct usage (kW vs kW): `generator.rs:712`, `741`, `751`, `818` — comparing `target`, `ramp_delta`, `output_kw`, `current_power_kw` against `IDLE_KW_THRESHOLD`
- Incorrect label mismatch (kW constant vs W variable): `generator.rs:793` — `if zone_heat_w > IDLE_KW_THRESHOLD` (zone_heat_w is in W)
- Incorrect label mismatch: `generator.rs:806` — `if q_thermal_w > IDLE_KW_THRESHOLD` (q_thermal_w is in W)
- Test code mismatch: `generator.rs:2259` — `assert!(generator.telemetry().get(tk::FUEL_INPUT_W).unwrap() < IDLE_KW_THRESHOLD);`

**Root Cause**: The constant was likely named for its primary use case (kW-valued variables) without adjusting the name or creating a separate W-valued threshold for the secondary use cases.

**Impact**: If the threshold value were ever changed (e.g., to `1e-3` kW = 1 W for a real idle-power cutoff), the W comparisons would receive a 0.001× smaller threshold than intended. Currently harmless because 1e-6 kW ≈ 0.001 W is effectively zero in both contexts for generators (which operate at kW scale).

### Finding 5: `SpecificHeatCapacity` and `MassDensity` uom types imported but not exposed as public aliases

**Severity**: low
**Description**: The `units.rs` module imports `UomSpecificHeatCapacity` and `UomMassDensity` from `uom` and uses them internally in conversion functions (`specific_heat_btu_lb_f_to_j_kg_k` and `density_lb_ft3_to_kg_m3`), but does not expose them as public type aliases alongside `Temperature`, `Power`, `Energy`, etc. The `HeatCapacity` alias is defined, but `SpecificHeatCapacity` — which is the workhorse unit in water heater and HVAC energy balance equations — is not.

**Code Location**:
- Imported but not aliased: `crates/hares-physics/src/units.rs:13-14` — `SpecificHeatCapacity as UomSpecificHeatCapacity` and `MassDensity as UomMassDensity`
- Only used in conversion functions: `units.rs:153-161`
- HeatCapacity IS aliased: `units.rs:36` — `pub type HeatCapacity = UomHeatCapacity;`
- All actual specific-heat arithmetic uses bare f64 constants: `crates/hares-physics/src/constants.rs:24-28,86` — `CP_DRY_AIR_J_KG_K`, `CP_LIQUID_WATER_J_KG_K`

**Root Cause**: These types were added to support IP→SI conversion functions but were not needed as standalone type aliases because the cross-crate boundary uses bare f64.

**Impact**: Minor. If these types were exposed, they would enable typed interfaces within hares-physics itself (e.g., functions that take `SpecificHeatCapacity` instead of bare `f64`), but since the boundary is f64 regardless, this has no cross-crate effect.

### Finding 6: Typed boundary wrappers convert to/from f64 at the hares-physics boundary — no true type safety propagation

**Severity**: low
**Description**: The `psychrometrics.rs` and `air_properties.rs` modules provide "typed uom boundary" variants of their core functions (e.g., `saturation_pressure(Temperature) → Pressure`, `humidity_ratio_from_twb_typed(Temperature, Temperature, Pressure) → f64`). However, these typed wrappers are thin shells that immediately convert to bare f64 via `temperature_to_celsius()` / `pressure_to_pascal()`, call the bare-f64 kernel, and (for return values that are temperatures or pressures) wrap the f64 result back into a typed quantity. This means the typed interface offers input-validation but does not provide dimensional correctness guarantees beyond the hares-physics function boundary — the caller immediately receives back an f64 that can be misused downstream.

**Code Location**:
- Typed saturation pressure: `crates/hares-physics/src/psychrometrics.rs:65-68` — converts Temperature→f64, calls bare-f64 kernel, wraps result as Pressure
- Typed humidity from wet-bulb: `psychrometrics.rs:102-108` — converts 3 typed inputs to f64, returns bare f64
- Typed wet-bulb (returns Temperature): `psychrometrics.rs:147-153` — converts to f64, calls kernel, wraps result as Temperature
- Typed air density: `crates/hares-physics/src/air_properties.rs:44-46` — converts Pressure+Temperature to f64, returns bare f64 (density lacks a type alias)

**Root Cause**: This is the intended design per the boundary policy in `units.rs:5-6`: "Inner-loop kernels may use raw f64." The typed wrappers are a convenience layer, not a system of type-guaranteed dimensional correctness.

**Impact**: The typed wrappers could give a false sense of security. A developer using `saturation_pressure(my_temp)` and receiving a `Pressure` back might assume the returned value is protected from misuse, but if they pass it through `pressure_to_pascal()` and feed the f64 into an unrelated computation, there is no protection.

## Summary
- Total findings: 6
- Critical: 0 / High: 1 / Medium: 2 / Low: 3

## Recommendations

1. **Move `PortContribution`/`PortSlots` to a common location that can use typed quantities.** The root obstacle is that `hares-types` (leaf crate) cannot depend on `hares-physics` (which depends on `uom`). Options:
   - Add `uom` as a dependency to `hares-types` and use typed quantities in `PortContribution` fields (`sensible_gain_w: Power`, `active_power_kw: Power` scaled to kW).
   - Move port types into `hares-physics` so they can use typed wrappers natively.
   - Create a `hares-units` crate at the dependency-tree root with type aliases and helper functions, separating unit types from physics computations.

2. **Unify the W/kW convention at the PortContribution boundary.** Either use watts for all power-valued fields (and let the electrical solver scale to kW for grid reporting) or create explicit newtype wrappers (`struct Kilowatts(f64)`, `struct Watts(f64)`) that prevent accidental mixing. The latter is simpler and doesn't require a `uom` dependency.

3. **Implement cross-type arithmetic traits** (`Temperature × HeatCapacity → Energy`, `MassFlowRate × SpecificHeatCapacity × Temperature → Power`) if and only if typed quantities are adopted at the interface boundary (Recommendation 1). Without boundary adoption, these trait impls would have no consumer.

4. **Rename `IDLE_KW_THRESHOLD`** to `IDLE_W_THRESHOLD` and set `= 0.001` (or maintain two constants: `IDLE_KW_THRESHOLD` for kW comparisons and `IDLE_W_THRESHOLD` for W comparisons) to eliminate the label mismatch.

5. **Expose `SpecificHeatCapacity` and `MassDensity`** as public type aliases in `units.rs` along with `from_celsius`/`to_celsius`-style conversion helpers, even if they remain unused at boundaries today. This completes the type alias coverage and future-proofs against adoption of typed interfaces.

## References / Citations
- `hares-physics/src/units.rs:1-7` — Documented boundary policy (typed at public boundaries, f64 in inner loops)
- `hares-types/src/ports.rs:57-91` — `PortContribution` enum with mixed W/kW conventions
- `hares-equipment/src/generator.rs:216` — `IDLE_KW_THRESHOLD` definition
- uom crate documentation: <https://docs.rs/uom/latest/uom/>
