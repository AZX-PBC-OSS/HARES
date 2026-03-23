# Thermal Envelope Solver

Implementation details of the RC thermal network, state-space integration,
and per-timestep heat balance resolution in `hares-envelope`.

---

## Module Layout

```
hares-envelope/src/
├── thermal_solver/
│   ├── mod.rs           ThermalSolver struct, resolve() flow, component gains
│   ├── config.rs        Config types, EnvelopeComponentGains, wiring indices
│   ├── solar.rs         Window SHGC + IAM, opaque solar injection
│   └── infiltration.rs  Infiltration/ventilation integration into u vector
├── boundary_rc.rs       Building geometry → RC network assembly
├── rc_network.rs        RCNetwork graph → Kirchhoff A_c/B_ext matrices
├── state_space.rs       Discretization (ZOH/Van Loan), matrix exponential, stepping
└── longwave_radiation.rs  Exterior iterative LWR, interior linearized LWR

hares-physics/src/
├── infiltration.rs      ASHRAE wind-stack, ELA, ACH, natural ventilation
├── solar.rs             EnergyPlus glazing curves, IAM modifier
├── film_coefficients.rs TARP interior + DOE-2 exterior convection
├── air_properties.rs    Moist air density, psychrometrics
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

## State-Space Discretization

**Entry point**: `discretize_auto(a_c, b_c, dt)`

Converts continuous-time `dx/dt = A_c·x + B_c·u` to discrete
`x[k+1] = A_d·x[k] + B_d·u[k]`.

### Primary Path: ZOH (Zero-Order Hold)

Used when A_c is well-conditioned (rcond > 1e-12):

```
A_d = exp(A_c · dt)
B_d = A_c⁻¹ · (A_d − I) · B_c
```

**Matrix exponential** uses 13th-order Padé scaling-and-squaring:
- Scale factor s = ⌈log₂(‖A_c·dt‖₁ / θ₁₃)⌉ where θ₁₃ ≈ 5.37192
- Repeated squaring recovers exp(A_c·dt) from exp(A_c·dt / 2^s)

B_d computed via LU factorization of A_c (avoids explicit inverse).

### Fallback: Van Loan

Used when A_c is singular or near-singular:

```
         ┌ A_c  B_c ┐
expm(dt· │          │ )
         └  0    0  ┘
```

A_d is extracted from the top-left block, B_d from the top-right block of the
result matrix.

### Stability Verification

For small systems (n ≤ 20), eigenvalues are checked:
- Continuous: all Re(λ) < 0
- Discrete: all |λ| ≤ 1 + 1e-10
- Exception: singular A_c with marginally-stable A_d is permitted

For larger systems, a Gershgorin bound on A_d is used instead of full
eigenvalue decomposition.

---

## Per-Timestep Resolve Flow

`ThermalSolver::resolve(ports, env, dt)` builds the input vector u, steps the
state, and extracts zone temperatures. The u vector is pre-allocated and
zeroed in-place each step (no allocation).

### Input Application Order

Each phase writes to u via pre-computed index maps in `StateSpaceWiring`:

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
                              Wind-stack/ELA/ACH + natural ventilation + HRV/ERV
                              Sensible → u vector; latent → separate return
 ↓
ideal HVAC solve              Back-calculate capacity to hold setpoint (if configured)
 ↓
x_next = A_d·x + B_d·u       State integration
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

**Simple** (rad_frac ≤ 0): Single call to `exterior_longwave_w()`:
```
Q_lw = ε·σ·A · [(f_gnd + (1−β)·f_sky)·(T_air⁴ − T_surf⁴) + β·f_sky·(T_sky⁴ − T_surf⁴)]
```
- f_sky: sky view factor (function of tilt)
- β: horizon bias factor (enhances near-horizon air radiation)
- f_gnd = 1 − f_sky

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

**Load calculation**:
- Mass flow: `ṁ = ρ_da(P, T_out, w_out) · q` [kg_da/s]
- Sensible: `Q_s = ṁ · c_p · (T_out − T_zone)` → injected to u
- Latent: `Q_l = ṁ · H_fg · (w_out − w_zone)` → returned separately for
  humidity solver

### Ideal HVAC Capacity

For zones configured with ideal HVAC, the solver back-calculates the input
power needed to hold the setpoint after one state-space step:

```
y_target = C·A_d·x + (C·B_d[:,idx] + D[out,idx])·u[idx] + background
```

Rearranged: `u[idx] = (y_target − background) / coeff`

Uses `last_u` (previous timestep) for the background estimate — one-step-stale
duty cycle matching OCHRE behavior.

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

## Planned Improvements (THERMAL Tickets)

The following tickets track improvements to the thermal solver, ordered by
dependency chain.

### THERMAL-001: Dry Air Density for Infiltration

Infiltration mass flow currently uses moist air density directly, overestimating
by ~0.5–1%. Fix: divide by (1 + W) to get dry-air mass flow, since sensible and
latent loads are formulated per kg dry air.

### THERMAL-002: EnergyPlus 4-Component Exterior LWR

Replace the current 2-component sky/ground LWR model with EnergyPlus's
4-component formulation using linear view factors and a β split coefficient.
Removes the `powf(1.5)` sky view factor exponent. Adds `beta: f64` to
`ExteriorSurface`.

### THERMAL-003: Pre-Refactor Cleanup

Structural cleanup before the Crank-Nicolson refactor (all behavior-preserving):
- Split `resolve_internal()` into phases
- `matrix_exp()` returns `Result` instead of panicking
- Configurable indoor zone (removes hardcoded `ZoneId(1)`)
- Deduplicate LWR functions
- Separate mutable surface temps from config
- Depends on: THERMAL-002

### THERMAL-004: Pre-Refactor Test Coverage

Fill coverage gaps before the solver rewrite:
- Multi-zone thermal coupling (2-zone wall heat transfer)
- Interior LWR energy balance (4-surface zone)
- 24h energy conservation (with and without HVAC)
- Depends on: THERMAL-003

### THERMAL-005: Crank-Nicolson Implicit Solver

Replace the explicit `x[k+1] = A_d·x + B_d·u` step with an implicit
Crank-Nicolson scheme that eliminates overshoot under large infiltration loads:

```
(I − dt/2·A_c)·x[k+1] = (I + dt/2·A_c)·x[k] + dt·B_c·u[k]
```

Pre-computes M = (I − dt/2·A_c) and its LU factorization at init. Zero-allocation
`step_into()` uses pre-allocated work buffers. Removes `SolverMethod` enum —
implicit-only.
- Depends on: THERMAL-002, THERMAL-003, THERMAL-004

### THERMAL-006: Analytical Validation

Synthetic box tests against known analytical solutions:
- 1R1C exponential decay, steady-state, solar step response
- Extreme ACH stability (50 ACH — would blow up explicit solver)
- Implicit vs explicit agreement for stable cases
- Zero-allocation verification
- Depends on: THERMAL-005

### THERMAL-007: 48-Hour OCHRE Trace Comparison

Per-timestep comparison against OCHRE reference output over 48 hours.
Thresholds: zone temp < 0.1°C divergence for first 6h, cumulative < 0.5°C at
48h.
- Depends on: THERMAL-001, THERMAL-002, THERMAL-005

### THERMAL-008: 7-Day Parity Benchmark

Tighten OCHRE parity tolerances to ASHRAE 140-2023 levels:
- HVAC heating: 15% (was 123%)
- Schedule-driven: 2%
- Performance guard: >30k steps/sec
- 1h parity gate: all equipment within 5%
- Depends on: THERMAL-007

### Dependency Chain

```
THERMAL-001 (dry air density) ─────────────────────┐
THERMAL-002 (4-component LWR) ──┬──────────────────┤
                                ↓                   ↓
THERMAL-003 (cleanup) ──→ THERMAL-004 (tests) ──→ THERMAL-005 (Crank-Nicolson)
                                                    ↓
                                               THERMAL-006 (analytical)
                                                    ↓
                                               THERMAL-007 (48h trace) ──→ THERMAL-008 (7-day parity)
```
