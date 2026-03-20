# Physics Decisions

## 2026-03-18 — `uom` Boundary Policy for `hares-physics`

- Decision:
  - Use `uom` quantity types at public crate boundaries where unit confusion risk is high.
  - Keep tight inner-loop kernels on raw `f64` when unit is explicit in naming/docs (`*_pa`, `*_c`, etc.).
  - Provide typed wrapper/helper functions in `crates/hares-physics/src/units.rs` and boundary wrappers in physics modules.

- Rationale:
  - Boundary typing prevents cross-crate unit mistakes at compile time.
  - Raw inner kernels keep implementations simple and avoid ergonomic overhead in deeply nested numeric code.

- Immediate application:
  - Added public quantity aliases and conversion helpers in `hares-physics::units`.
  - All public psychrometrics functions have typed `uom` wrappers: `saturation_pressure`, `humidity_ratio_from_tdp_typed`, `humidity_ratio_from_twb_typed`, `relative_humidity_typed`, `wet_bulb_from_humidity_ratio_typed`, `dew_point_typed`, `moist_air_enthalpy_typed`.
  - All public air_properties functions have typed wrappers: `standard_pressure`, `moist_air_density`, `dry_air_density`.
  - Infiltration functions with clear unit semantics have typed wrappers: `ach_infiltration_typed`, `terrain_wind_speed_typed`, `terrain_wind_speed_for_class_typed`.
  - Biquadratic and solar functions are excluded — biquadratic operates on dimensionless coefficients, solar uses mixed-unit angles where `uom` adds friction without safety benefit.
  - `ashrae_wind_stack` and `ela_infiltration` are excluded — their coefficients have complex compound units that would reduce clarity.

- Non-goals:
  - Do not introduce `uom` into `hares-types`.
  - Do not convert every internal local variable in existing kernels to `uom` in this phase.
