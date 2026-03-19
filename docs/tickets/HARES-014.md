---
id: HARES-014
title: "hares-envelope — DomainSolver Trait and Thermal Solver"
kind: implement
depends_on: [HARES-012, HARES-013, HARES-003, HARES-007, HARES-008]
files_to_touch:
  - crates/hares-types/src/domain_solver.rs
  - crates/hares-types/src/lib.rs
  - crates/hares-envelope/src/thermal_solver.rs
  - crates/hares-envelope/src/lib.rs
references:
  - docs/architecture/01-sim-core-and-solver.md
verification:
  - cargo check -p hares-types
  - cargo check -p hares-envelope
  - cargo test -p hares-envelope
  - cargo clippy -p hares-types -- -D warnings
  - cargo clippy -p hares-envelope -- -D warnings
---

## Background/Context

Step 4 of the timestep execution flow ("Envelope Resolution") is structured as a set of domain solvers — one per physics domain — each implementing a common `DomainSolver` trait. This ticket defines that trait and delivers the first and most complex implementation: `ThermalSolver`, which owns the RC state-space model and integrates zone temperatures forward each timestep. The `DomainSolver` trait is the primary extension point for future physics domains without touching the core engine loop.

`DomainSolver` and `DomainId` live in `hares-types` (not `hares-envelope`) for the same reason `Equipment` lives there: custom solver crates must be able to implement the trait without depending on `hares-envelope`. Placing it in `hares-envelope` would create an inversion — downstream crates that provide custom solvers would need to depend on a heavyweight envelope crate just to implement a marker trait.

## Work to Do

- [ ] In `hares-types/src/domain_solver.rs`: define `DomainUpdate` struct with fields: `domain_id: DomainId`, `zone_temperatures_c: Vec<(ZoneId, f64)>`, `custom_payload: Option<Vec<f64>>`. ThermalSolver uses `custom_payload` to pass infiltration latent gains to HumiditySolver: it encodes the values as `[(zone_id_as_f64, latent_w), ...]` pairs (alternating f64 elements) so HumiditySolver can extract them without coupling to ThermalSolver types.
- [ ] In `hares-types/src/domain_solver.rs`: define `DomainSolver` trait:
  ```rust
  pub trait DomainSolver: Send + Sync {
      fn domain_id(&self) -> DomainId;
      fn resolve(
          &mut self,
          ports: &PortSlots,
          env: &EnvironmentState,
          dt: Duration,
      ) -> DomainUpdate;
  }
  ```
  Note: the `DomainSolver` trait contains ONLY `domain_id()` and `resolve()`. HVAC-specific methods like `solve_ideal_capacity` do NOT belong on this trait — they live on concrete solver implementations (e.g. `ThermalSolver`). This keeps the trait domain-agnostic and object-safe.
- [ ] In `hares-types/src/domain_solver.rs`: import `DomainId` from `hares-types/src/equipment.rs` (defined in HARES-001 as `DomainId(pub u16)` newtype); do NOT redefine it. Define constants for built-in solver domains using the newtype constructor: `pub const THERMAL: DomainId = DomainId(0)`, `pub const ELECTRICAL: DomainId = DomainId(1)`, `pub const HUMIDITY: DomainId = DomainId(2)`, `pub const FLUID: DomainId = DomainId(3)`
- [ ] Re-export `DomainSolver`, `DomainUpdate`, `DomainId` from `hares-types/src/lib.rs`
- [ ] The `DomainSolver::resolve` signature is: `fn resolve(&mut self, ports: &PortSlots, env: &EnvironmentState, dt: Duration) -> DomainUpdate`. This signature is fixed and must NOT be extended with HVAC-specific parameters. For ideal HVAC capacity, add a concrete method on `ThermalSolver` (NOT on the `DomainSolver` trait): `pub fn solve_ideal_capacity(&self, env: &EnvironmentState, zone: ZoneId) -> f64` that returns the heat rate (W) needed to maintain the zone setpoint. This is a `ThermalSolver`-only concern and must not pollute the general-purpose trait.
- [ ] In `thermal_solver.rs`: define `InfiltrationMethod` enum:
  ```rust
  pub enum InfiltrationMethod {
      AshraeWindStack {
          c_s: f64,
          c_w: f64,
          shielding_coeff: f64,
          n_stories: u8,
      },
      Ela {
          ela_m2: f64,
          stack_coeff: f64,
          wind_coeff: f64,
      },
      Ach {
          ach: f64,
      },
  }
  ```
- [ ] In `thermal_solver.rs`: define `ThermalSolverConfig` struct with explicit fields:
  ```rust
  pub struct ThermalSolverConfig {
      /// Maps each zone to its index in the state vector `x`
      pub zone_state_indices: HashMap<ZoneId, usize>,
      /// Maps each zone to its index in the output vector `y = C*x + D*u`
      pub zone_output_indices: HashMap<ZoneId, usize>,
      /// Maps each zone to its column index in `B_d` for sensible heat injection
      pub zone_sensible_input_indices: HashMap<ZoneId, usize>,
      /// Column indices in `B_d` for outdoor temperature inputs
      pub outdoor_temp_input_indices: Vec<usize>,
      /// Column indices in `B_d` for indoor temperature inputs
      pub indoor_temp_input_indices: Vec<usize>,
      /// Maps surface IDs to their column indices in `B_d` for solar gain inputs
      pub solar_input_indices: HashMap<u32, usize>,
      /// Per-zone setpoints for ideal HVAC capacity solve (°C)
      pub ideal_setpoints_c: HashMap<ZoneId, f64>,
      /// Infiltration calculation method for this building
      pub infiltration: InfiltrationMethod,
      /// Mechanical ventilation flow rate (m³/s); 0.0 if none
      pub ventilation_flow_m3_s: f64,
      /// Ventilation configuration (per-zone flows and supply conditions)
      pub ventilation: VentilationConfig,
      /// Zones where ideal HVAC capacity should be computed (solver
      /// back-calculates the exact heat injection to hit setpoint).
      /// This is HVAC-specific config, NOT part of the DomainSolver trait.
      pub ideal_hvac_zones: Vec<ZoneId>,
  }
  ```
- [ ] In `thermal_solver.rs`: define `ThermalSolver` struct implementing `DomainSolver`:
  - Owns a `StateSpaceModel` (from HARES-012)
  - Owns current RC state vector `x: DVector<f64>`
  - `domain_id()` returns `THERMAL`
  - The `ideal_hvac_zones` list comes from the `ThermalSolverConfig` field (set at construction time), NOT passed to `resolve()`. The `DomainSolver::resolve` signature stays as-is (4 params: `&mut self, ports, env, dt`). The thermal solver does NOT introspect control signals to determine ideal mode; `hares-core` configures `ideal_hvac_zones` on `ThermalSolverConfig` at construction. If the set of ideal zones needs to change at runtime, provide a `ThermalSolver::set_ideal_hvac_zones(&mut self, zones: Vec<ZoneId>)` method — this is a concrete method, not a trait method.
  - `resolve()` steps:
    1. Build input vector `u` from: per-zone accumulated `sensible_gain_w` in `PortSlots`, infiltration sensible gain (call hares-physics infiltration functions using `env.weather` and zone config), ventilation sensible gain, outdoor temperature, solar gains per surface. Infiltration air density is correctly computed at outdoor temperature — incoming air has outdoor properties per ASHRAE Fundamentals. After computing infiltration flow rate and its sensible contribution, also compute the infiltration latent gain (using the infiltration flow rate and indoor/outdoor humidity ratio difference) and include it in `DomainUpdate.custom_payload` keyed by `ZoneId` so HumiditySolver can consume it in the same timestep. (`DomainSolver::resolve` receives `&PortSlots`, so it must not mutate slots directly.)
    2. Call `StateSpaceModel::step(&self.x, &u)` to advance state
    3. For each zone in `ideal_hvac_zones`, call `StateSpaceModel::solve_for_input` to find the exact heat injection required to hit setpoint, substitute into `u`, re-run step
    4. Update `self.x` to the new state
    5. Extract zone temperatures from `y = C * x + D * u`
    6. Return `DomainUpdate` with updated zone temperatures
- [ ] Wire `eigenvalue_check` call at `ThermalSolver` construction time; it returns `Result<StabilityResult, StabilityResult>` (as defined in HARES-012) — `Ok(StabilityResult)` when both continuous and discrete eigenvalues are stable, `Err(StabilityResult)` when any eigenvalue is unstable. If `Err`, propagate as a construction error (do not panic).
- [ ] Note: the `domain_id` field on `DomainUpdate` is redundant with `solver.domain_id()`. The engine must verify they match at the call site to prevent misrouting.
- [ ] Implement steady-state initialization: set initial `x` so all node temperatures lie between indoor and outdoor temperature, matching OCHRE's `StateSpaceModel.get_initial_state` logic exactly

## Files to Touch

- `crates/hares-types/src/domain_solver.rs`: `DomainSolver` trait, `DomainUpdate` struct, `DomainId` (imported from HARES-001, not redefined) and constants
- `crates/hares-types/src/lib.rs`: re-export `DomainSolver`, `DomainUpdate`, `DomainId`
- `crates/hares-envelope/src/thermal_solver.rs`: `ThermalSolver`, `ThermalSolverConfig`, `InfiltrationMethod`
- `crates/hares-envelope/src/lib.rs`: re-export `ThermalSolver`, `ThermalSolverConfig`, `InfiltrationMethod`

## Measures of Success

- [ ] Free-float single zone with no HVAC ports, `T_init = 20°C`, `T_outdoor = 0°C`: after 60 steps of 60s the zone temperature is strictly less than 20°C and strictly greater than 0°C (drifting toward outdoor)
- [ ] 24-hour sinusoidal outdoor temperature (amplitude 10°C, mean 15°C, period 24h): zone temperature lags outdoor by a measurable phase and has a smaller amplitude — confirms the RC integrates correctly over many timesteps
- [ ] Steady-state initialization golden test: for the reference 3R2C network, run OCHRE's `StateSpaceModel.get_initial_state(T_indoor, T_outdoor)` and capture the output as a constant. Assert that `ThermalSolver`'s initialization produces node temperatures matching those constants to within 1e-6°C. A range-only check (`[min, max]`) is not sufficient.
- [ ] Per-timestep thermal energy balance: in all integration tests assert `|ΣQ_gain - ΔE_storage - Q_loss| < max(1.0, 1e-6 · |ΣQ_gain|)` W per arch doc `07-testing-and-verification.md §Numerical Invariants`. This must be checked at every timestep, not just at the end of the run.
- [ ] `DomainSolver` is object-safe (can be held as `Box<dyn DomainSolver>`)
- [ ] Unstable RC network at construction returns `Err`, not a panic

## Performance Notes
- **P1 — ThermalSolver u_buf**: The input vector `u` is built into a reusable `DVector` field (`u_buf`) via `std::mem::replace`, avoiding a heap allocation every timestep. `last_u.clone_from(&u)` reuses the existing allocation instead of creating a new `DVector`. This is the hottest allocation site in OCHRE's thermal loop.
- **P3 — Ideal HVAC solve**: `solve_for_output_input` inlines scalar algebra directly (one division) instead of cloning `A_d`/`B_d` matrices. The ideal-capacity back-solve runs once per zone per step; eliminating matrix clones removes O(n^2) allocations from the inner loop.

## Verification

- [ ] `cargo check -p hares-types` passes
- [ ] `cargo check -p hares-envelope` passes
- [ ] `cargo test -p hares-envelope` passes
- [ ] `cargo clippy -p hares-types -- -D warnings` passes
- [ ] `cargo clippy -p hares-envelope -- -D warnings` passes
