# Simulation Core & Solver

## Timestep Execution Flow

The simulation loop follows OCHRE's proven single-pass approach. Equipment runs in
dependency order; the envelope solves last with all gains accumulated. This is the same
causality ordering OCHRE uses — we're reimplementing it in Rust, not changing it.

```
FOR each timestep:

  1. ADVANCE CLOCK
     └─ Increment current_time, load weather/schedule for this step

  2. DISTRIBUTE CONTROL SIGNALS
     ├─ HELICS: receive publications from co-sim federates (if connected)
     ├─ RL Gym: receive action from agent (if gym mode)
     └─ Deliver typed ControlSignal values to target equipment by ID

  3. EQUIPMENT UPDATE (sequential, dependency-ordered)
     For each equipment in dependency order:
       a. Read current environment (previous-step zone temps, current weather)
       b. Run internal control (thermostat, SOC tracking) or apply external control
       c. Calculate power and heat (biquadratic curves, efficiency models)
       d. Write port contributions (thermal gains, electrical power, fuel use)

  4. ENVELOPE RESOLUTION
     ├─ Sum thermal port contributions per zone
     ├─ Sum electrical port contributions per bus
     ├─ Solve envelope: x[k+1] = A·x[k] + B·u[k]  (RC state-space)
     ├─ Update humidity: moisture mass balance
     └─ Update environment state (zone temps, humidity)

  5. RECORD RESULTS
     ├─ Append to output buffers (Arrow RecordBatch)
     ├─ HELICS: publish outputs to co-sim (if connected)
     └─ RL Gym: return observation to agent (if gym mode)
```

## Coupling Strategy

**v1 implements sequential coupling only** — matching OCHRE exactly. Equipment runs in
dependency order; each equipment sees upstream equipment's current-step output but the
previous timestep's zone temperatures. This is OCHRE's approach and it works well for
residential simulation at 1-minute timestep.

The one-step temperature lag is negligible for thermal systems with time constants of
30+ minutes. For ideal-capacity HVAC, the lag is compensated by inverting the RC model
to find the exact heat injection needed to reach setpoint — this works because the RC
system is linear and the HVAC has unlimited capacity in ideal mode.

### Equipment Dependency Order

Matches OCHRE's `Dwelling._sort_sub_simulators()`:

```
Stage 1:  Scheduled loads, PV, event-based loads
          (no thermal feedback needed, run first)

Stage 2:  Battery, EV, Generator
          (electrical only, may need PV output for self-consumption)

Stage 3:  HVAC, Water Heater
          (thermal coupling — need accumulated loads from stages 1-2)

Stage 4:  Envelope solver + humidity
          (resolves all thermal/moisture contributions into next state)
```

**v1.1+ design space** (not implemented, but architecture doesn't block):
- Parallel (Jacobi) coupling: all equipment reads frozen previous-step state
- Iterative coupling: equipment + envelope loop until convergence

These would require double-buffering and convergence checks. The architecture supports
adding them later because equipment already communicates only through typed ports — not
by directly mutating shared state.

### Domain Solver Extensibility

The envelope resolution step (step 4 in the execution flow) is internally structured
as a set of domain solvers — one per physics domain:

```rust
pub type DomainId = u16;

pub trait DomainSolver: Send + Sync {
    fn domain_id(&self) -> DomainId;
    fn resolve(
        &mut self,
        ports: &PortSlots,          // accumulated contributions from all equipment
        env: &EnvironmentState,
        dt: Duration,
    ) -> DomainUpdate;             // delta to apply to EnvironmentState
}
```

Built-in solvers:
- **ThermalSolver**: Sum thermal contributions per zone → RC envelope state-space step
- **ElectricalSolver**: Sum power per bus, apply voltage-dependent (ZIP) model
- **HumiditySolver**: Latent gains → moisture mass balance
- **FluidSolver**: Loop flow/temperature balance (when hydronic equipment present)

The trait is the extension point for future physics domains (CO2 concentration, wall
vapor diffusion, multi-zone airflow) without modifying the core engine loop.

In v1, the built-in solvers are called directly (no dynamic dispatch overhead). The
trait definition exists to ensure the architecture supports pluggable domains when
needed — user-registered solvers handle custom domains via the same interface.

## RC Envelope Solver

Discrete-time state-space model, identical to OCHRE's `StateSpaceModel`:

```
x[k+1] = A_d · x[k] + B_d · u[k]
y[k]   = C · x[k] + D · u[k]
```

Where:
- `x` = state vector (node temperatures across envelope)
- `u` = input vector (outdoor temp, solar gains, internal gains, HVAC output)
- `A_d, B_d` = discretized system matrices (computed once at init via ZOH)
- `C, D` = output matrices (zone temperatures from state)

Discretization uses zero-order hold (ZOH): `A_d = exp(A_c · dt)`, `B_d = A_c⁻¹(A_d - I)B_c`.
This is the same approach OCHRE uses via `scipy.signal.cont2discrete`.

**Singular A_c fallback**: When `A_c` is singular (degenerate RC networks), the matrix
inverse does not exist. OCHRE handles this via the Van Loan augmented matrix method
(`StateSpaceModel.py:261-264`): compute `expm` of the block matrix `[[A_c, B_c], [0, 0]] * dt`
and extract `A_d`, `B_d` from the result. The Rust implementation must reproduce both
paths.

**RC network construction** follows OCHRE's `RCModel`: surfaces contribute R-C nodes
organized by boundary type, with star-mesh reduction for multi-surface zones.

### Stability

At initialization, two-stage validation ensures the RC network is stable:

```rust
// Stage 1: Continuous-time stability (necessary condition)
// A correctly constructed RC network (all R > 0, all C > 0) has all eigenvalues
// of A_c with negative real parts.
let eigenvalues_c = compute_eigenvalues(&a_continuous);
assert!(eigenvalues_c.iter().all(|λ| λ.re < 0.0),
    "Envelope RC matrix has non-negative eigenvalue: unstable system");

// Stage 2: Discrete-time stability after ZOH discretization (sufficient condition)
// A thin wall node with very small R×C may have |λ_d| near 1.0 at 60s timestep,
// causing oscillation that the continuous check misses.
let a_discrete = matrix_exp(&a_continuous * dt);
let eigenvalues_d = compute_eigenvalues(&a_discrete);
assert!(eigenvalues_d.iter().all(|λ_d| λ_d.norm() < 1.0),
    "Discretized envelope matrix has eigenvalue outside unit circle");

// Warning for near-unity eigenvalues (oscillatory but bounded)
for λ_d in &eigenvalues_d {
    if λ_d.norm() > 0.99 {
        warn!("Near-unity discrete eigenvalue |λ_d|={:.4}: time constant may be \
               too fast for timestep {}s, expect slow oscillation", λ_d.norm(), dt);
    }
}
```

Additionally validate:
- All R, C values are positive (catch construction errors before eigenvalue computation)
- Matrix condition number is reasonable (catch near-singular networks)

## Airflow & Moisture (Improved Over OCHRE)

**Infiltration** (three methods, same as OCHRE):
- ASHRAE wind+stack: quadrature combination
- ELA (Effective Leakage Area): single coefficient
- ACH: fixed air changes per hour (fallback)

**Fixes over OCHRE** (see [Appendix](appendix-physics-improvements.md) for verified equations):
- Air density computed from site altitude and outdoor temperature via ASHRAE/ISA formula
  (OCHRE hardcodes sea-level `0.0765 lb/ft³` — 18% error at Denver). Elevation from
  HPXML `Building/Site/Elevation` → EPW header → hard error (no silent zero default).
- Terrain category, shielding coefficient, and wind exposure derived from HPXML
  `Site/SiteType` and `Site/ShieldingofHome` using ASHRAE HOF Ch.16 terrain tables
  and AIM-2 shelter class lookup (OCHRE hardcodes suburban assumptions)

**Ventilation** (same as OCHRE):
- Balanced mechanical with HRV/ERV (sensible + latent recovery)
- Unbalanced mechanical
- Natural ventilation via operable windows
- Combined: `Q_eff = √(Q_nat² + Q_forced²)` (OCHRE's heuristic, matches ASHRAE 62.2)

**Humidity** (improved over OCHRE):
- Indoor zone moisture mass balance
- Moisture buffering multiplier
- Latent gains from HVAC, infiltration/ventilation, appliances/occupancy
- Dehumidifier equipment type (OCHRE gap — acknowledged but never implemented)

## Configuration

```toml
[simulation]
start_time = "2019-05-05T12:00:00"
duration = "30d"                     # arbitrary: hours, days, weeks, or full year
time_res = "60s"                     # 1-min default; also supports 15-min, 5-min, etc.
# coupling_mode = "sequential"      # only option in v1
```

**Timestep**: Any uniform duration that divides evenly into the simulation period.
Common values are 60s (1-min, OCHRE default) and 900s (15-min, common for fleet/grid
studies). Shorter timesteps improve temporal resolution but increase computation and
output size proportionally.

**Simulation period**: Arbitrary start time and duration, bounded only by the weather
and schedule file coverage. A 1-year, 1-min simulation produces 525,600 timesteps;
output is streamed to disk via Arrow RecordBatch flushing (see `06-input-output.md`)
to avoid unbounded memory growth.

No multi-rate time stepping in v1. All equipment and envelope run at the same `time_res`.
