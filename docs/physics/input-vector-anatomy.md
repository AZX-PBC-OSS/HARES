# Input Vector (`u`) Anatomy

The state-space step is `x_next = A_d·x + B_d·u` plus semi-implicit
couplings. `u` is assembled fresh every timestep in
`thermal_solver/mod.rs::build_input_vector`; its column layout is fixed at
construction in `hares-core::dwelling::solver_builder`:

```
[ 0 .. n_ext )                        environmental driving inputs
                                      (ground-node temps…, outdoor air temp)
[ n_ext .. +n_ext_surface_inputs )    one column per exterior boundary WITH
                                      RC layers (outer_wiring): opaque
                                      skin flux injections (solar, LWR),
                                      gain 1/C_outer_node
[ … .. +n_int_surface_inputs )        one column per conditioned-interior
                                      boundary with RC layers
                                      (inner_wiring): interior solar/LWR
                                      splits, gain 1/C_inner_node
[ zone_input_offset .. +n_zones )     per-zone sensible heat: infiltration,
                                      ventilation, HVAC/ports, window solar
                                      air share, interior-LWR air residual,
                                      gain 1/C_zone_air
```

Writers per column class (all in `thermal_solver/`):

| Columns | Writer | Contents |
|---|---|---|
| environmental | `apply_outdoor_inputs` | driving temperatures (outdoor, per-depth Kusuda ground) |
| ext-surface | `apply_exterior_solar_inputs` (rad_frac==0 surfaces), `apply_exterior_longwave_inputs_iterative` (rad_frac>0 surfaces: `(solar+q_lwr)·rad_frac`) | outside-face fluxes; see `docs/physics/exterior-surface-balance.md` |
| int-surface | solar distribution deposits (`solar.rs`), interior LWR (ScriptF mode), radiant port distribution | interior flux splits via `radiation_frac` |
| zone sensible | ports/infiltration/ventilation accumulators, window solar air share, window LWR correction, no-node boundary fallbacks | direct zone-air watts |

**Sharing rule:** a column belongs to a *node*, not a surface. Multiple
surfaces injecting into the same node legitimately share its column —
injections are additive (`u[col] += q`), so the sum is the correct total
flux into that node. Surfaces without RC layers (windows, fallback-R
boundaries) have no dedicated column and share the zone sensible column.
`ThermalSolverConfig::validate` checks the structure at construction;
duplicate *dedicated* columns are the hard-error class (double
registration), shared zone columns are legal.

**Do not measure fluxes by differencing `u.iter().sum()`.** That pattern
(four full-vector sums per step, removed 2026-09-11) could not see
coupling-routed fluxes and produced the misattributed diagnostics behind
I-02. Every `apply_*` function returns the exact watts it injected; use
the return values.

**Semi-implicit couplings bypass `u` entirely.** Infiltration, the
linearized exterior LWR branch (rad_frac==0), and per-step TARP convection
corrections travel through `coupling_buf` (diagonal damping + forcing),
not through input columns. See `stepping.rs::build_coupling`.
