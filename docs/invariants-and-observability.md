# Invariants & Observability

How HARES validates physics correctness at runtime and provides deep
introspection for debugging simulation state.

---

## Invariant Checking

### Compilation Model

All invariant checks live in `hares-core/src/invariants.rs` behind a
compile-time gate:

```rust
#[cfg(any(debug_assertions, feature = "check_invariants"))]
```

| Build                                        | Checks active? |
|----------------------------------------------|----------------|
| `cargo build` / `cargo test`                 | Yes            |
| `cargo build --release`                      | No (zero cost) |
| `cargo build --release -F check_invariants`  | Yes            |

When inactive, every `check_*` method compiles to `Ok(())` — the compiler
eliminates the body entirely.

### Available Checks (InvariantChecker)

These are the conservation-law checks defined in `InvariantChecker`. Not all
are wired into `Dwelling::check_invariants` yet — the table notes which are
currently active.

| Check                     | Equation                                            | Tolerance                          | Fatal? | Wired? |
|---------------------------|-----------------------------------------------------|------------------------------------|--------|--------|
| `thermal_balance`         | \|Σ Q_gain − ΔE_storage − Q_loss\| < tol           | max(1.0 W, 1e-6 × gross_flux)     | Yes    | No     |
| `electrical_balance`      | \|P_grid + Σ P_equipment\| < tol                    | 0.001 kW                          | Yes    | Yes    |
| `moisture_balance`        | \|Δm_water − Σ(Q_latent·dt / h_fg)\| < tol         | 1e-6 kg                           | Yes    | No     |
| `zone_temperature_bounds` | T_zone ∈ [−50, 80] °C, finite                      | —                                  | Yes    | Yes    |
| `tank_temperature_bounds` | T_tank ∈ [0, 100] °C, finite                       | —                                  | Yes    | No     |
| `soc_bounds`              | SoC ∈ [0, 1], accumulated error < 0.001            | —                                  | No     | No     |

**Thermal tolerance** uses gross flux (sum of absolute values of all gain terms)
for the relative component, not net sum — a balanced system with large opposed
fluxes still has floating-point error proportional to gross magnitude. The 1.0 W
floor prevents division-by-zero when gains are near zero.

**Moisture** uses h_fg = 2,501,000 J/kg (latent heat of vaporisation at 0 °C).

### Active Checks in Dwelling::check_invariants

These fire every timestep after solver resolution but before port zeroing:

| Check                      | What it validates                                              |
|----------------------------|----------------------------------------------------------------|
| `timestep_dt`              | dt > 0 and finite                                              |
| `zone_temperature_bounds`  | All zone temps from thermal_update within [−50, 80] °C        |
| `electrical_net_finite`    | `electrical_solver.net_active_kw()` is finite                  |
| `electrical_balance`       | Solver net matches port accumulation: \|solver + ports\| < 0.001 kW |
| `humidity_payload_finite`  | Every value in humidity domain payload is finite               |

### Error Reporting

All fatal violations produce:

```rust
HaresError::InvariantViolation {
    check_name: String,   // e.g. "thermal_balance"
    value: f64,           // actual residual or offending value
    tolerance: f64,       // threshold exceeded (0.0 for bounds checks)
}
```

This halts the dwelling simulation with a descriptive error — no silent
corruption.

### Ordering Contract

```
Equipment::step()          ← accumulates into PortSlots
ThermalSolver::resolve()   ← reads ports, produces DomainUpdate
check_invariants()         ← reads solver net + port accumulators
ports.zero()               ← resets accumulators for next step
```

`check_invariants()` reads both `self.electrical_solver.net_active_kw()` and
`self.ports.electrical.net_active_kw()` to compare them. It must run before
`ports.zero()` clears the accumulator side of that comparison.

---

## Observer System (Zero-Cost Debugging)

### Feature Gate

The entire observer module is gated by `#[cfg(feature = "observe")]`. When
disabled, every observation site in `run_timestep` compiles away — zero runtime
overhead in production builds.

### Data Model

A `StepSnapshot` captures the complete state at every phase boundary of a single
timestep. All phase fields are `Option<T>` — they are populated incrementally
as each phase completes:

```
StepSnapshot
├── step_index: u64
├── timestamp: DateTime<FixedOffset>
└── phases: PhaseSnapshots
    ├── post_environment:          Option<EnvironmentCapture>
    ├── post_nonthermal_equipment: Option<EquipmentPhaseCapture>
    ├── post_thermal_equipment:    Option<EquipmentPhaseCapture>
    ├── post_solvers:              Option<SolverCapture>
    └── post_zone_update:          Option<ZoneUpdateCapture>
```

#### EnvironmentCapture

Captured after environment update (weather load + schedule advance). Gives you
the starting conditions before equipment runs.

- Outdoor conditions: `outdoor_temp_c`, `ghi_w_m2`, `wind_speed_m_s`, `mains_temp_c`
- Zone state vectors: `zone_temps_c`, `zone_humidity_ratios` (both `Vec<(ZoneId, f64)>`)

#### EquipmentPhaseCapture

Captured twice — once after independent+electrical equipment, once after thermal
equipment. Contains per-equipment observations and the accumulated port state
after the phase.

Each `EquipmentObservation` includes:
- Identity: `name`, `equipment_type`, `end_use`
- `telemetry` — equipment-specific operating state (mode, capacity, COP, etc.)
- `port_declarations` — what ports this equipment registered at init
- `contribution` — what this equipment added to ports during its `step()` (the delta)
- `pre_step_ports` — snapshot of accumulators visible to this equipment before it stepped

#### How Contributions Are Computed

Equipment contributions are derived by diffing `PortSlots` before and after
each equipment's `step()`. The diff logic in `observer_capture::diff_ports`
matches ports by identity (`ZoneId` for thermal, `(LoopId, FluidType)` for
fluid) rather than positional index, and suppresses noise below thresholds:

- Thermal: `f64::EPSILON` (~2.2e-16 W)
- Fluid flow: 1e-6 kg/s

For fluid ports, individual equipment supply/return temperatures are
back-calculated from the flow-weighted mean accumulator:

```
T_supply_equip = (T̄_after · ṁ_after − T̄_before · ṁ_before) / Δṁ
```

This avoids requiring equipment to report temperatures separately.

#### SolverCapture

Captured after all four domain solvers run. Contains:
- `thermal_update`, `humidity_update`, `electrical_update`, `fluid_update` — all `DomainUpdate`
- `envelope_gains: EnvelopeComponentGains` — component-level gain breakdown

The `EnvelopeComponentGains` struct breaks down every thermal input to the
envelope solver:

| Field                    | Unit | Description                                  |
|--------------------------|------|----------------------------------------------|
| `window_solar_w`         | W    | SHGC × IAM × area × POA                     |
| `opaque_solar_lwr_w`     | W    | Exterior surface solar + LWR combined        |
| `interior_lwr_w`         | W    | Interior longwave radiation exchange         |
| `infiltration_w`         | W    | Infiltration sensible (indoor zone)          |
| `ventilation_w`          | W    | Forced mechanical ventilation sensible       |
| `natural_ventilation_w`  | W    | Natural ventilation sensible                 |
| `port_sensible_w`        | W    | Total equipment port sensible (HVAC + loads) |
| `hvac_heating_w`         | W    | HVAC heating contribution                    |
| `hvac_cooling_w`         | W    | HVAC cooling contribution                    |
| `internal_gain_w`        | W    | Appliances, lighting, occupancy              |
| `jacket_loss_w`          | W    | Equipment shell losses (water heater, etc.)  |
| `duct_loss_w`            | W    | Duct distribution losses                     |
| `infiltration_by_zone`   | W    | Per-zone infiltration breakdown              |
| `interior_lwr_by_zone`   | W    | Per-zone interior LWR net heat gains         |

#### ZoneUpdateCapture

Captured after thermal + humidity updates are applied to zones.
Final `zone_temps_c` and `zone_humidity_ratios` (both `Vec<(ZoneId, f64)>`).

### Dwelling API

| Method                 | On         | Description                          |
|------------------------|------------|--------------------------------------|
| `enable_observer(n)`   | `Dwelling` | Activate observer ring buffer with capacity n |
| `drain_observations()` | `Dwelling` | Extract and clear all snapshots      |
| `observer_buffer()`    | `Dwelling` | Borrow buffer for read-only access (`Option<&ObserverBuffer>`) |

`ObserverBuffer` itself is a FIFO ring buffer (`VecDeque<StepSnapshot>`) that
evicts the oldest snapshot when at capacity. It exposes `last()`, `snapshots()`,
`len()`, `is_empty()`, and `drain()`.

---

## Diagnostic CSV Output

Enabled by `output_verbosity >= 4` in `SimulationConfig`. Writes one row per
timestep via `hares-core/src/diagnostics.rs`.

### Columns

Per-timestep: `step`, `timestamp_s`, `outdoor_temp_c`, `electrical_net_kw`

Per-zone (repeated for each zone): `zoneN_temp_c`, `zoneN_thermal_gain_w`,
`zoneN_latent_gain_w`

Extended fields (populated when available):
- Per-equipment: name, mode, electric_kw, sensible_gain_w
- Envelope breakdown: window_solar, opaque_solar_lwr, interior_lwr,
  infiltration_by_zone, internal_gain, port_sensible

---

## Debugging Workflows

### Thermal Runaway

1. Enable diagnostic CSV (`output_verbosity: 4`)
2. Plot `zone1_temp_c` vs `outdoor_temp_c` — look for divergence
3. Enable observer, drain snapshots around the divergence timestep
4. Inspect `SolverCapture.envelope_gains` — which component is unbounded?
5. Check `EquipmentContribution.thermal` — is HVAC fighting itself?

### Equipment Not Firing

1. Enable observer, inspect `EquipmentObservation.telemetry` for the equipment
2. Check `contribution` — all zeros means `step()` ran but produced no output
3. Check `pre_step_ports` — is a prerequisite (e.g., fluid flow) missing?
4. Inspect control dispatch: was a `DispatchRequest` generated for this equipment?

### Energy Balance Violation

1. The `InvariantViolation` error names the failing check and reports the
   residual and tolerance
2. Enable observer to capture the failing timestep
3. For `electrical_balance`: sum all equipment `electrical_load_kw` +
   `electrical_gen_kw` contributions and compare with solver net

### Humidity Drift

1. Compare `EnvironmentCapture.zone_humidity_ratios` (start of step) with
   `ZoneUpdateCapture.zone_humidity_ratios` (end of step)
2. Inspect latent gains in equipment contributions
3. Check infiltration moisture via `humidity_update`

---

## Source Files

| File | Purpose |
|------|---------|
| `hares-core/src/invariants.rs` | InvariantChecker — conservation law checks |
| `hares-core/src/observer.rs` | Observer types and ObserverBuffer |
| `hares-core/src/observer_capture.rs` | Capture functions and port diffing |
| `hares-core/src/diagnostics.rs` | Diagnostic CSV output |
| `hares-core/src/dwelling/mod.rs` | Integration: run_timestep observation sites, check_invariants |
| `hares-envelope/src/thermal_solver/config.rs` | EnvelopeComponentGains |
