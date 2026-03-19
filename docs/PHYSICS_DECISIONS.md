# Physics Decisions

## 2026-03-18: Recoverable RC Stability Failure Path

- Context: The architecture example in `docs/architecture/01-sim-core-and-solver.md`
  illustrates stability checks with `assert!`.
- Decision: `hares-envelope::state_space::eigenvalue_check` returns
  `Result<StabilityResult, StabilityResult>` and construction maps instability to
  `StateSpaceError::UnstableSystem`, rather than panicking.
- Rationale: Callers (test harnesses, config validation tools, and orchestrator
  constructors) need a recoverable error that can report which parameterization
  failed stability checks. A panic at construction time would prevent graceful
  reporting and handling.

## 2026-03-18: Dry Air Gas Constant = 287.058 J/(kg·K)

- Context: EnergyPlus `PsyRhoAirFnPbTdbW` and OCHRE use 287.0. The NIST-derived
  value is R / M_air = 8314.462 / 28.9647 ≈ 287.058.
- Decision: `DRY_AIR_GAS_CONSTANT` in `hares-physics::air_properties` is set to
  `287.058` for improved precision over the EnergyPlus/OCHRE approximation.
- Rationale: The ~0.02% improvement is free and aligns with the NIST reference
  value. Test tolerances are set to accommodate the small divergence from OCHRE.

## 2026-03-18: `uom` Boundary Policy for `hares-physics`

- Decision:
  - Use `uom` quantity types at public crate boundaries where unit confusion risk is high.
  - Keep tight inner-loop kernels on raw `f64` when unit is explicit in naming/docs (`*_pa`, `*_c`, etc.).
  - Provide typed wrapper/helper functions in `crates/hares-physics/src/units.rs` and boundary wrappers in physics modules.

- Rationale:
  - Boundary typing prevents cross-crate unit mistakes at compile time.
  - Raw inner kernels keep implementations simple and avoid ergonomic overhead in deeply nested numeric code.

- Immediate application:
  - Added public quantity aliases and conversion helpers in `hares-physics::units`.
  - All public psychrometrics functions have typed `uom` wrappers: `saturation_pressure`, `humidity_ratio_from_tdp_typed`, `humidity_ratio_from_twb_typed`, `relative_humidity_typed`, `wet_bulb_from_humidity_ratio_typed`, `dew_point_typed`. `moist_air_enthalpy_typed` accepts typed `Temperature` but returns raw `f64` because `uom` has no `SpecificEnthalpy` (J/kg) quantity type; similarly `moist_air_density` and `dry_air_density` return raw `f64` (no density type alias in the crate).
  - All public air_properties functions have typed wrappers: `standard_pressure`, `moist_air_density`, `dry_air_density`.
  - Infiltration functions with clear unit semantics have typed wrappers: `ach_infiltration_typed`, `terrain_wind_speed_typed`, `terrain_wind_speed_for_class_typed`.
  - Biquadratic and solar functions are excluded — biquadratic operates on dimensionless coefficients, solar uses mixed-unit angles where `uom` adds friction without safety benefit.
  - `ashrae_wind_stack` and `ela_infiltration` are excluded — their coefficients have complex compound units that would reduce clarity.

- Non-goals:
  - Do not introduce `uom` into `hares-types`.
  - Do not convert every internal local variable in existing kernels to `uom` in this phase.

## 2026-03-18: HVAC Airflow, Sentinel Setpoints, and Ideal-Capacity Switching

- Context: HVAC common logic in `hares-equipment::hvac::common` is aligning with the architecture appendix and HARES-020 acceptance criteria.
- Decision:
  - Default `airflow_cfm_per_ton` is set to `375` (instead of OCHRE's `312`) and scales by HPXML `AirflowDefectRatio` when present.
  - ResStock `No Space Heating` / `No Space Cooling` schedule sentinels are mapped to effective setpoints of `-999°C` / `+999°C` instead of being silently dropped.
  - Thermostat control uses ideal-capacity mode when `time_res >= 5 minutes` or `use_ideal_capacity` is explicitly enabled.
- Rationale:
  - `375 CFM/ton` is consistent with ACCA Manual S minimum practice and avoids inheriting a likely ResStock calibration constant without explicit opt-in.
  - Preserving schedule sentinel semantics ensures explicit schedule intent (disable heating/cooling) is represented deterministically in simulation control.
  - Coarse-timestep ideal-capacity behavior avoids timestep-induced cycling artifacts and matches OCHRE's coarse-step switching behavior.
- Impact note:
  - Changing airflow from `312` to `375 CFM/ton` can change delivered capacity, fan power, latent/sensible split, and therefore whole-building energy totals versus OCHRE-calibrated baselines.

## 2026-03-18: HPXML Efficiency Normalization for SEER2 and HSPF2

- Context: HPXML fixtures and tools may provide either legacy (`SEER`, `HSPF`) or newer (`SEER2`, `HSPF2`) efficiency units.
- Decision:
  - Convert `SEER2` to `SEER` using `SEER = SEER2 / 0.95`.
  - Convert `HSPF2` to `HSPF` using `HSPF = HSPF2 / 0.95`.
  - Preserve other supported unit strings (`SEER`, `EER`, `EER2`, `HSPF`, `AFUE`, `Percent`, `COP`) without conversion.
- Rationale:
  - This keeps normalized equipment parameters consistent for downstream models while accepting all HPXML 4.0 efficiency unit variants.
  - The 0.95 factor is an explicit approximation chosen for cross-input compatibility and deterministic behavior.
