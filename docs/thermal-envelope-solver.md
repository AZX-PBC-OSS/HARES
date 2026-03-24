# Thermal Envelope Solver

Implementation details of the RC thermal network, state-space integration,
and per-timestep heat balance resolution in `hares-envelope`.

---

## Module Layout

```
hares-envelope/src/
├── thermal_solver/
│   ├── mod.rs           ThermalSolver struct, DomainSolver impl, resolve() dispatch
│   ├── config.rs        Config types, EnvelopeComponentGains, wiring indices
│   ├── initialization.rs Steady-state initialization from outdoor conditions
│   ├── stepping.rs      resolve_internal() CN step, semi-implicit infiltration coupling
│   ├── longwave.rs      Exterior iterative + interior linearized LWR application
│   ├── ports.rs         build_input_vector(): outdoor, solar, LWR, ports, infiltration
│   ├── solar.rs         Window SHGC + IAM, opaque solar injection
│   └── infiltration.rs  InfiltrationCoupling: conductance + forcing for semi-implicit
├── boundary_rc.rs       Building geometry → RC network assembly
├── rc_network.rs        RCNetwork graph → Kirchhoff A_c/B_ext matrices
├── state_space.rs       Crank-Nicolson implicit model, matrix exponential, stepping
└── longwave_radiation.rs  4-component EnergyPlus exterior LWR, interior linearized LWR

hares-physics/src/
├── infiltration.rs      ASHRAE wind-stack, ELA, ACH, natural ventilation
├── solar.rs             EnergyPlus glazing curves, IAM modifier
├── film_coefficients.rs TARP interior + DOE-2 exterior convection
├── air_properties.rs    Moist air density (kg_da/m³), psychrometrics
└── psychrometrics.rs    Humidity ratio, latent heat
```

---

## RC Network Construction

**Entry point**: `assemble_building_rc(boundaries, n_zones, zone_capacitances)`

The building envelope is modeled as a lumped-parameter RC network where each
material layer becomes a resistance-capacitance node, and zones are air nodes
with thermal mass.

### Node Types

| Node             | ID range         | Capacitance                           |
|------------------|------------------|---------------------------------------|
| Zone air         | 1..n_zones       | ρ·V·c_p × 7 (interior mass mult)     |
| Material layer   | 1000+            | ρ_mat·c_p_mat·thickness·area          |
| Outdoor (ext)    | u32::MAX − 1     | External driving node (no capacitance)|
| Ground (ext)     | u32::MAX         | External driving node (no capacitance)|

Minimum capacitance floor: 1000 J/K (prevents near-singularity).

### Boundary Processing

Each boundary connects two zones (or a zone to exterior) through its material
layers:

```
Zone air ──[R_film_int]── Layer 1 ──[R_12]── Layer 2 ──[R_23]── Layer 3 ──[R_film_ext]── Outdoor
             C_zone         C_1                C_2                C_3
```

- **Precomputed RC** (OCHRE LUT): Used when available; bypasses raw material
  properties
- **Material layers**: Layer resistance `R = thickness / (k · area)`, capacitance
  `C = ρ·c_p·thickness·area`
- **Same-zone boundaries** (party walls): Keep inner half of layers only. For
  odd layer counts, the middle layer's capacitance is halved
- **Fallback**: No layers → aggregate R-value: `R = fallback_r / area`

Every zone air node is guaranteed at least one resistance connection (falls back
to outdoor if isolated).

### Matrix Assembly

`RCNetwork::build_matrices()` performs Kirchhoff nodal analysis:

For each internal node *i* with capacitance C_i connected to neighbors via
resistances R_ij:

- **A_c[i,i]** = −Σ(1 / (R_ij · C_i)) for all neighbors j
- **A_c[i,j]** = 1 / (R_ij · C_i) for internal neighbor j
- **B_ext[i,k]** = 1 / (R_ij · C_i) for external neighbor j at column k

Output: continuous-time `(A_c, B_ext)` matrices plus index maps:
- `zone_state_rows`: ZoneId → state vector row
- `layer_info`: boundary → outermost layer node (for solar/LWR injection)
- `outdoor_col`: B_ext column for outdoor temperature

---

## State-Space Model (Crank-Nicolson Implicit)

**Entry point**: `StateSpaceModel::from_continuous(a_c, b_c, dt, output_mapping)`

The continuous-time RC system `dx/dt = A_c·x + B_c·u` is discretized using the
Crank-Nicolson (trapezoidal) implicit scheme:

```
(I − dt/2·A_c) · x[k+1] = (I + dt/2·A_c) · x[k] + dt·B_c · u[k]
      M                          N                     B_eff
```

This is A-stable: any continuous system with Re(λ) ≤ 0 produces a stable
discrete system regardless of timestep size. This eliminates the overshoot that
the previous explicit ZOH solver exhibited under large infiltration loads.

### Pre-Computed Matrices

At initialization, three matrices are built and stored:
- **M** = I − dt/2·A_c (implicit half, LU-factored)
- **N** = I + dt/2·A_c (explicit half)
- **B_eff** = dt·B_c (scaled input)

The LU factorization of M is computed once. Each timestep step solves
`M·x[k+1] = N·x[k] + B_eff·u[k]` via forward/back substitution — no per-step
factorization.

### Zero-Allocation Stepping

`step_into(x, u, buf)` uses a caller-owned buffer:
```rust
buf = N·x           // gemv, zero-alloc
buf += B_eff·u      // gemv accumulate
M_lu.solve_mut(buf) // in-place triangular solve
```

`ThermalSolver` owns `rhs_buf` and swaps it with `x` after each step.

### Discrete-Path Compatibility

`StateSpaceModel::from_discrete(A_d, B_d, C, D)` sets M = I so the solve
degenerates to `x[k+1] = A_d·x + B_d·u` for pre-discretized models.

### Stability Verification

For small systems (n ≤ 20), eigenvalues of the equivalent A_d = M⁻¹·N are
checked:
- Continuous: all Re(λ) < 0
- Discrete: all |λ| ≤ 1 + 1e-10
- Exception: singular A_c with marginally-stable A_d is permitted

For larger systems, a Gershgorin bound on the continuous A_c is used — since
CN is A-stable, continuous stability implies discrete stability.

### Steady State

`steady_state(u)` solves for the equilibrium state under constant input:
- Continuous path: `x_ss = −A_c⁻¹·B_c·u`
- Discrete path: `x_ss = (I − A_d)⁻¹·B_d·u`

Used for initialization (computing initial state from outdoor conditions).

### Matrix Exponential (Utility)

`matrix_exp()` is retained for boundary RC assembly and returns `Result` (not
panic). Uses 13th-order Padé scaling-and-squaring (θ₁₃ ≈ 5.37192).

---

## Per-Timestep Resolve Flow

`ThermalSolver::resolve(ports, env, dt)` delegates to `resolve_internal()` in
`stepping.rs`. The resolve is split into two stages: input vector construction
(`build_input_vector()` in `ports.rs`) and CN stepping with semi-implicit
infiltration coupling (`resolve_internal()` in `stepping.rs`). All buffers are
pre-allocated — zero per-step heap allocation.

### Input Application Order

`build_input_vector()` writes to u via pre-computed index maps in
`StateSpaceWiring`:

```
u = 0
 ↓
apply_outdoor_inputs          T_outdoor → boundary driving inputs
 ↓
apply_solar_inputs            Window SHGC × IAM × POA → zone inputs
 ↓
apply_exterior_solar_inputs   Opaque absorptance × irradiance (non-iterative path)
 ↓
apply_exterior_longwave_inputs_iterative
                              Iterative surface temp solver with heavy-ball damping
                              Couples solar + LWR to RC node via film resistance
 ↓
apply_interior_longwave_inputs
                              Linearized multi-surface radiation exchange
 ↓
apply_port_sensible_inputs    Equipment sensible gains from PortSlots
 ↓
apply_infiltration_and_ventilation
                              Returns InfiltrationCoupling per zone (h_inf, T_forcing)
                              Latent loads → separate HashMap
 ↓                            ┌─────────────────────────────────────────────┐
resolve_internal()            │ Build semi-implicit coupling tuples         │
                              │ Build coupled LU (M + D) if infiltration    │
                              │ Ideal HVAC solve (coupled or uncoupled)     │
                              │ CN step: M⁻¹(N·x + B_eff·u + forcing)      │
                              │ Cache coupled LU + couplings for next step  │
                              └─────────────────────────────────────────────┘
 ↓
y_next = C·x_next + D·u      Output extraction (zone temperatures)
```

### Outdoor Inputs

For each boundary connected to outdoor: `u[idx] = env.weather.outdoor_temp_c`.
Ground-connected boundaries use `env.weather.ground_temp_c`.

### Window Solar Gains

For each window surface in `env.weather.solar_irradiance`:

1. Select EnergyPlus glazing curve from U-factor and SHGC via
   `GlazingCurve::from_u_shgc()` (6 variants: A, Bdcd, D, E, F, J — curves
   B/C/D are collapsed)
2. Compute angle-of-incidence modifier:
   - Beam: `iam_beam = window_iam(θ, curve)` — degree-4 polynomial (Horner's
     method), normalized by normal-incidence transmittance. Returns 0 for
     θ ≥ π/2.
   - Diffuse: `iam_diffuse = curve.diffuse_iam()` — hemispherical average
3. POA irradiance: `poa = direct·iam_beam + diffuse·iam_diffuse`
   (ground-reflected is excluded for windows by design)
4. Decompose SHGC:
   - Transmitted: `T_sol · area · poa`
   - Absorbed inward: `(SHGC − T_sol) · area · poa`
5. Inject total to u vector at zone input index

### Opaque Solar Gains

For each `ExteriorSurfaceInfo` (non-window):
- `q_solar = absorptance · area · (direct + diffuse + reflected)`
- **Non-iterative path** (rad_frac ≤ 0): injected directly to u
- **Iterative path** (rad_frac > 0): fed into LWR iteration below

### Exterior Longwave Radiation

Two paths depending on film resistance coupling:

**Simple** (rad_frac ≤ 0): Single call to `exterior_longwave_w()` using the
EnergyPlus 4-component model (Engineering Reference §External Longwave
Radiation):

```
Q_lw = ε·σ·A · [F_gnd·(T_air⁴ − T_surf⁴)
              + β·F_sky·(T_sky⁴ − T_surf⁴)
              + (1−β)·F_sky·(T_air⁴ − T_surf⁴)]
```

Three radiation source terms:
- **Ground hemisphere**: F_gnd × (T_air⁴ − T_surf⁴), where T_ground = T_air per E+ standard
- **True sky** (cold): β × F_sky × (T_sky⁴ − T_surf⁴)
- **Near-horizon air** (warm): (1−β) × F_sky × (T_air⁴ − T_surf⁴)

View factors (linear, not raised to 1.5):
- F_sky = 0.5·(1 + cos φ), where φ is surface tilt from horizontal
- F_gnd = 1 − F_sky
- β = √F_sky (splits sky hemisphere into true-sky and near-horizon)

**Iterative** (rad_frac > 0): Couples surface temperature to RC node:

1. Estimate surface temp: `T_surf = rad_frac·T_node + (1−rad_frac)·T_ext`
2. Iterate (n_iter = ⌈dt/300⌉ sub-iterations):
   - Compute net LWR at current T_surf
   - Solve: `T_new = T_surf_init + (solar + q_lw)·R_film`
   - Clamp step to ±2°C (stability)
   - Heavy-ball update: `T = T + 0.5·(T_new − T) + 0.1·(T − T_prev)`
   - Converge when ΔT < 0.01°C
3. Persist converged T_surf as warm-start for next timestep
4. Inject: `u[idx] += (solar + q_lw) · rad_frac`

### Interior Longwave Radiation

For each zone with ≥2 interior surfaces:

1. Estimate surface temps: `T_surf_i = rad_frac_i·T_node_i + (1−rad_frac_i)·T_zone`
2. Emissivity-area weighted mean radiant temperature:
   `T_mrt = Σ(ε_i·A_i·T_i) / Σ(ε_i·A_i)`
3. Linearized radiative coefficient: `h_r = 4·ε·σ·T_zone³` (linearized at zone
   air temperature)
4. Per-surface net flux: `Q_i = h_r·A_i·(T_mrt − T_i)`
5. Inject each Q_i to u at the surface's input index

### Equipment Sensible Gains

For each `PortSlots.thermal` entry: `u[zone_idx] += sensible_gain_w`

The `ThermalAccumulator` tracks gains by category in a fixed-size stack array
`[f64; 5]`: HvacHeating, HvacCooling, InternalGain, JacketLoss, DuctLoss.
These feed `EnvelopeComponentGains` but are summed for the u vector.

### Infiltration and Ventilation

**Infiltration models** (per-zone, selected at init):

| Model | Formula | Notes |
|-------|---------|-------|
| ASHRAE wind-stack | Q = √[(c_s·\|ΔT\|^n)² + (c_w·(shelter·v)^(2n))²] | n ∈ [0.5, 0.7]; default 0.65 |
| ELA | Q = ela · √(stack·\|ΔT\| + wind·v²) | Per ASHRAE 62.2; API takes ela in m² (SI) |
| ACH | Q = ach · volume / 3600 | Simplest; 0.3–0.7 typical |

**Natural ventilation** (operable windows):
- Gating: suppressed if w_out ≥ max_humidity, T_zone ≤ T_out, T_zone ≤ T_base,
  or open_area ≤ 0
- `Q_nat = area_cm² · √(stack·|ΔT| + wind·v²)`, capped at 20 ACH

**Combination with mechanical ventilation**:
- Balanced (ERV/HRV): `q_sens = (q_inf + q_nat) + q_forced·(1 − SRE)`
- Unbalanced: `q = √[(q_inf + q_nat)² + q_forced²]`

**Load calculation** (dry-air basis):
- Density: `ρ = moist_air_density_kg_m3(P, T_out, w_out)` — inverts ASHRAE HOF
  2021 Ch.1 Eq.28 specific volume `v = R_da·T·(1+W/ε)/p`, yielding kg_da/m³
  (dry air mass per unit volume, not total moist air mass)
- Mass flow: `ṁ = ρ · q` [kg_da/s]
- Sensible: `Q_s = ṁ · c_p_da · (T_out − T_zone)` [W]
- Latent: `Q_l = ṁ · H_fg · (w_out − w_zone)` → returned separately for
  humidity solver

**Semi-implicit treatment**: Infiltration is not injected directly into the u
vector. Instead, `apply_infiltration_and_ventilation()` returns per-zone
`InfiltrationCoupling` structs containing `h_inf_w_k` (sensible conductance
[W/K]) and `t_forcing_c` (outdoor driving temperature). In `resolve_internal()`,
the temperature-dependent term `−h_inf·T_zone` is moved to the implicit (M)
side of the CN system following EnergyPlus Engineering Reference §13.3. This
guarantees monotonic, oscillation-free convergence even when the infiltration
time constant is much smaller than the timestep.

The coupling modifies M's diagonal per-step:
```
d = h_inf × B_eff[(state, input)]
M_coupled = M + diag(d)
forcing = h_inf × T_out × B_eff + d × x[k]  (compensation for N-side)
```

A per-step LU factorization of M_coupled is built once and reused for both
the ideal HVAC solve and the CN step. The coupled LU is cached for the
next-step ideal HVAC back-calculation.

### Ideal HVAC Capacity

For zones configured with ideal HVAC, the solver back-calculates the input
power needed to hold the setpoint after one CN step. Two paths:

- **Coupled** (infiltration active): `solve_for_scalar_input_coupled()` uses
  the per-step M_coupled LU factorization
- **Uncoupled**: `solve_for_output_input()` uses the base M LU

Both solve a single linear equation for the HVAC input index that drives the
zone output to the setpoint.

`solve_ideal_capacity()` is called by equipment *before* `resolve()` builds the
current-step inputs, using `last_u`, `last_coupling`, and `last_coupled_lu`
(previous timestep). The estimate is therefore one-step stale — matching OCHRE
behavior. Zero allocation: the coupled LU was cached at the end of the previous
`resolve()` call.

### EnvelopeComponentGains

Populated by tracking u-vector sums before/after each application phase:

| Field                  | Source                                        |
|------------------------|-----------------------------------------------|
| `window_solar_w`       | Δu from `apply_solar_inputs()`                |
| `opaque_solar_lwr_w`   | Δu from opaque solar + iterative LWR          |
| `interior_lwr_w`       | Δu from `apply_interior_longwave_inputs()`    |
| `infiltration_w`       | Indoor-zone value from infiltration return map |
| `ventilation_w`        | Forced ventilation sensible                   |
| `natural_ventilation_w`| Natural ventilation sensible                  |
| `port_sensible_w`      | Sum of equipment port contributions           |
| `hvac_heating_w`       | HvacHeating category from thermal accumulator |
| `hvac_cooling_w`       | HvacCooling category from thermal accumulator |
| `internal_gain_w`      | InternalGain category from thermal accumulator|
| `jacket_loss_w`        | JacketLoss category from thermal accumulator  |
| `duct_loss_w`          | DuctLoss category from thermal accumulator    |
| `infiltration_by_zone` | Per-zone infiltration from return map         |
| `interior_lwr_by_zone` | Per-zone interior LWR from accumulation       |

Accessible via `solver.component_gains()`.

---

## Source Files

| File | Purpose |
|------|---------|
| `hares-envelope/src/thermal_solver/mod.rs` | ThermalSolver struct, DomainSolver impl |
| `hares-envelope/src/thermal_solver/config.rs` | Config types, EnvelopeComponentGains, wiring |
| `hares-envelope/src/thermal_solver/stepping.rs` | resolve_internal(), semi-implicit CN step |
| `hares-envelope/src/thermal_solver/ports.rs` | build_input_vector(): all input application |
| `hares-envelope/src/thermal_solver/longwave.rs` | Exterior iterative + interior LWR application |
| `hares-envelope/src/thermal_solver/solar.rs` | Window SHGC + IAM, opaque solar |
| `hares-envelope/src/thermal_solver/infiltration.rs` | InfiltrationCoupling computation |
| `hares-envelope/src/thermal_solver/initialization.rs` | Steady-state init from outdoor conditions |
| `hares-envelope/src/boundary_rc.rs` | Building geometry → RC network |
| `hares-envelope/src/rc_network.rs` | Kirchhoff A_c/B_ext matrix assembly |
| `hares-envelope/src/state_space.rs` | Crank-Nicolson model, matrix_exp, stepping |
| `hares-envelope/src/longwave_radiation.rs` | 4-component exterior LWR, interior linearized |
| `hares-physics/src/infiltration.rs` | ASHRAE wind-stack, ELA, ACH, natural vent |
| `hares-physics/src/solar.rs` | EnergyPlus glazing curves, Perez tilted irradiance |
| `hares-physics/src/air_properties.rs` | Dry air density (ASHRAE HOF 2021) |
