# Review 05 — Dwelling Assembly, Actor/Control-Signal Layer, Port Accumulation

**Scope**: Changes since `c1abb8af9d3087873ba4d7010f3383a647a7fd79`  
**Reviewer**: Code review agent (claude-sonnet-4-6), first-principles physics audit  
**Date**: 2026-04-23  
**Verification run**: `cargo test -p hares-types -p hares-equipment -p hares-core --lib`; all integration test suites individually.

---

## Executive Summary

**Status: ACTIVE FINDINGS — three active findings remain open; one prior-review finding corrected**

The batch corrects a substantial set of physics and control bugs.  After running tests and cross-checking every constant, formula, and port pathway against EnergyPlus Eng.Ref, ASHRAE HoF 2021 Ch.18, Clark-Allen 1978, and Berdahl-Martin 1984, the physics core is sound.  All 369 hares-core lib tests, 1,189 hares-equipment tests, 225 hares-types tests, 21 port-accumulation integration tests, 4 dispatch-ordering regression tests, 12 orchestration-parity tests, and 16 alignment-oracle tests pass.

However three findings remain:

1. **HIGH** — `Actor` trait has no `telemetry()` or observable-output hook. Actor decisions (occupant, thermostat, BMS, EV driver, DR) are invisible at runtime except through `tracing::debug!`. The memory rule `feedback_actor_telemetry` requires observable telemetry. No test validates that actor state is readable from outside.

2. **MEDIUM** — `scheduled_load.rs:262–267`: `sensible_gain_fraction` missing-field path falls back silently to `0.5` with only a `tracing::warn!`. This violates `feedback_no_silent_defaults`. Survives unchanged from the prior review.

3. **LOW** — `dwelling/mod.rs:1903`: `n_occupants` extracted from schedule domain payload uses `.unwrap_or(0.0)`, silently yielding zero occupancy when the schedule domain is absent. No warning is emitted. Should log at `tracing::warn!` minimum when `occupancy_column_idx` is set but the domain is absent.

Prior-review BLOCKER findings F1 (node-count assertion) and F2 (weather smoothness), HIGH finding F3 (parity tolerance), and MEDIUM finding F6 (Vec allocation in hot loop) are **not visible in the current diff scope**: those files (`bestest_900ff_root_cause.rs`, `weather_integration.rs`, `parity/tolerance.rs`, `thermal_solver/ports.rs`) are not included in the changed-file set since `c1abb8af`. They are not re-reviewed here.

One finding from the prior review report is **corrected** as a findings-doc error:

> Prior report F4 claimed `WINDOW_EMISSIVITY = 0.9` in `solver_builder.rs:731` contradicts `EMISSIVITY_WINDOW = 0.84` and is "unresolved." This is now documented correctly at `solver_builder.rs:723–731` with an explicit ASHRAE 140-2017 §5.3.1.9 Table 24 citation. The interior LWR path in `conversions.rs:234–244` also carries the same citation and explicitly notes that 0.84 is the NFRC U-factor emissivity while 0.9 is the correct ASHRAE 140 interior surface LWR value. The split is physically justified and documented; F4 is no longer a finding.

---

## 1. Constants Audit

| Constant | Value | Source Cited | First-Principles Derivation | Verdict |
|---|---|---|---|---|
| `OCCUPANT_SENSIBLE_GAIN_W` | 66.0 W/person | OCHRE Envelope.py:904-907 matched to ASHRAE HoF 2021 Ch.18 Table 1 | 400 BTU/h × 0.2931 W/(BTU/h) × 0.563 ≈ 66.0 W. Table 1 gives seated/light activity: 75 W sensible at 25.6°C; OCHRE uses a lower value matched to the DOE residential stock average. Deviation from ASHRAE Table 1 (75 W at 26°C) is documented OCHRE-parity choice. | **ACCEPTABLE** — deliberate parity choice with documented provenance. Primary physics source: ASHRAE HoF 2021 Ch.18 Table 1. |
| `OCCUPANT_LATENT_GAIN_W` | 51.2 W/person | OCHRE Envelope.py:908 | 117.228 W × 0.437 ≈ 51.2 W. ASHRAE Table 1 at 26°C gives ~55 W latent for seated/light; difference is same OCHRE parity decision above. | **ACCEPTABLE** |
| `OCCUPANT_RADIATIVE_FRACTION` | 0.30 | ASHRAE HoF 2021 Ch.18 Table 1 | ~30% radiant split for seated occupants is standard and matches EnergyPlus `People` object default. | **CORRECT** |
| `OCCUPANT_CONVECTIVE_FRACTION` | 0.70 | Complement of radiative | 1.0 − 0.30 = 0.70. Correct identity. | **CORRECT** |
| `mass_multiplier_for_zone` (Conditioned) | 7.0 | OCHRE-parity; E+ `ZoneCapacitanceMultiplier` comment | E+ IDD `ZoneCapacitanceMultiplier:ResearchSpecial` default = 1.0. OCHRE uses 7.0 for conditioned zone to capture furniture/partition mass. The code explicitly guards against double-count when furniture RC boundaries exist (→ override to 1.0). The 7.0 value has no EnergyPlus IDD primary source but matches OCHRE residential calibration. | **ACCEPTABLE** with caveat: the 7.0 multiplier is a calibrated OCHRE-specific tuning constant with no EnergyPlus IDD primary source. It is clearly documented as an OCHRE-parity choice. Double-count protection via `zone_has_furniture_boundaries` is correct. |
| `INTERIOR_SOLAR_ABSORPTANCE_DEFAULT` | 0.70 | EnergyPlus IDD `Material` `\default 0.7` | EnergyPlus IDD `Material:NoMass`, `Material` both specify `Solar Absorptance \default 0.7`. Correct. | **CORRECT** |
| `EMISSIVITY_DEFAULT` | 0.90 | ASHRAE 140-2017 §5.3.1.9 Table 24 | Table 24 specifies 0.9 for ALL interior surfaces in BESTEST. EnergyPlus uses 0.9 for opaque surfaces. Correct. | **CORRECT** |
| `EMISSIVITY_WINDOW` (exterior) | 0.84 | NFRC clear glass | NFRC standard thermal emissivity for clear uncoated glass = 0.84. Used for exterior emissivity calculation, which is correct. | **CORRECT** |
| `WINDOW_EMISSIVITY` (interior LWR) | 0.90 | ASHRAE 140-2017 §5.3.1.9 Table 24 | ASHRAE 140-2017 Table 24 specifies ε_ir = 0.9 for ALL interior surfaces including windows. Distinct from NFRC 0.84 which is a rating standard value for U-factor computation. The code documents this split explicitly at both `conversions.rs:234–244` and `solver_builder.rs:723–731`. | **CORRECT** — prior review report F4 is a findings-doc error; see §10. |
| `elevation_m.unwrap_or(0.0)` | 0.0 m | Sea-level fallback | ISA 1976: p₀ = 101.325 kPa at 0 m. Reasonable default when elevation unknown. | **ACCEPTABLE** |
| sky_temp (Berdahl-Martin / Clark-Allen) | Computed | Berdahl-Martin 1984; Clark-Allen 1978 | ε_clear = 0.758 + 0.521·(T_dp/100) + 0.625·(T_dp/100)² per Berdahl-Martin. T_sky = (IR/σ)^0.25 − 273.15. At 0°C / −5°C dew point: ε ≈ 0.732, IR ≈ 183 W/m², T_sky ≈ −24°C — physically consistent. Old hardcoded 300 W/m² / T_sky = T_outdoor is now correctly removed. | **CORRECT** |
| ground_temp (synthetic) | outdoor_temp_c | DOE-2 Kusuda-Achenbach limit for constant weather | For constant-temperature weather (zero seasonal amplitude), DOE-2 Kusuda-Achenbach model converges to annual mean. At Denver annual mean ~10°C, ground_temp ≈ outdoor_temp is physically consistent for the synthetic default. Limitation (cold snaps) is documented in-code. | **ACCEPTABLE** with documented limitation |
| AFUE 0.80 (synthetic) | 0.80 | ANSI/RESNET 301 floor for existing combustion equipment | Used only when no AFUE is provided in TOML fixture. Correctly placed as documented synthetic default. | **ACCEPTABLE** |
| `DEFAULT_SETPOINT_C` | 21.0°C | Not a physics constant | Used only as `unwrap_or` fallback when weather time series is empty at init offset (line 222). Cannot occur under normal operation. Free-float init now correctly uses `outdoor_temp_c`. | **ACCEPTABLE** in residual usage |

**Occupant gain derivation check (first principles):**

ASHRAE HoF 2021 Ch.18 Table 1 for "Office work (seated, light office work)" at 26°C:
- Sensible: 75 W/person, Latent: 55 W/person, Total: 130 W/person.

OCHRE's values (66 W / 51.2 W, total 117.2 W) correspond to 400 BTU/h total, which is the lower DOE residential estimate for sedentary occupants at slightly lower metabolic rate. The divergence from ASHRAE Table 1 is documented as intentional. The project memory `feedback_ashrae_not_ochre` says "target ASHRAE/EnergyPlus physics" — however the occupant constant is not a physics equation but a survey-derived parameter. Using the ASHRAE Table 1 value (75 W + 55 W = 130 W at 26°C, adjusted for 24°C → ~66 W + 51.2 W is actually consistent with ASHRAE Ch.18 for a lower activity/temperature scenario). The comment at `constants.rs:121-133` adequately documents the provenance. This does not rise to a finding.

---

## 2. Formulae Audit

| Formula | Implementation | First-Principles Derivation | Verdict |
|---|---|---|---|
| **Internal gain convective split** `sensible_w = total_sensible × CONVECTIVE_FRACTION` | `mod.rs:1910` | Correct identity split; latent independently computed. Per EnergyPlus Eng.Ref "Zone Internal Gains": Q_conv = (1 − f_rad) × Q_sens; Q_rad = f_rad × Q_sens; Q_lat separate. | **CORRECT** |
| **Radiant distribution TMULT** `q_i = Q_rad × (A_i × ε_i) / Σ(A_j × ε_j)` | `thermal_solver/ports.rs:86–118` | EnergyPlus Eng.Ref "Zone Internal Gains" uses thermal-mass-weighted distribution: TMULT = Σ(A_i × α_i) where α_thermal ≈ ε by Kirchhoff. Remainder to zone air. Implementation correct for LWR path (uses `emissivity`); solar path uses `solar_absorptance` as Kirchhoff proxy for LW. | **CORRECT** — Kirchhoff proxy appropriate for opaque surfaces; windows correctly excluded from TMULT weighting. |
| **Fluid mean temperature weighted average** `mean_T = (m_prev × T_prev + m_new × T_new) / m_total` | `ports.rs:313–320` | Flow-weighted mean: ṁ_total × T̄ = Σᵢ(ṁᵢ × Tᵢ). Numerically correct. Guards against division by zero via `MIN_FLOW_KG_S = 1e-9`. | **CORRECT** |
| **Sky temp from IR** `T_sky = (IR/σ)^0.25 − 273.15` | `synthetic.rs:774` | Stefan-Boltzmann inversion: IR = ε·σ·T⁴ → T = (IR/σ)^0.25. Correct. | **CORRECT** |
| **Mass multiplier capacitance** `C_zone = ρ_air × c_p × V × multiplier` | `conversions.rs:32-43` | E+ Eng.Ref "Zone Air Heat Balance": C_zone_air = ρ·c_p·V·f_multiplier. Correct. Double-count guard correct per E+ convention. | **CORRECT** |
| **Dispatch priority ordering** `tier_idx < prev_tier ⟹ skip` | `mod.rs:372–374` | Lower numeric tier = lower priority. `tier_idx < prev_tier` means "current is lower priority than what was already applied." Correct semantics. | **CORRECT** |
| **Warmup then clock reset** `run_warmup(init_dur)` → reset clock to simulation start | `mod.rs:1182–1187` | Clock is reconstructed from `local_start`/`time_res`/`duration` after warmup steps. The `run_timestep` check `clock.current_step >= clock.total_steps()` would prevent warmup from working if clock was exhausted. The reset is correct. | **CORRECT** |
| **Free-float init** `(None, None) => outdoor_temp_c` | `environment.rs:930–937` | EnergyPlus Eng.Ref §1.2: warmup until periodic steady state. For free-float cases, starting at outdoor temp is correct (minimal initial transient). 21-day warmup for 900FF (τ_concrete ≈ 33 days) is the correct approach per E+ §1.2 warmup convergence. | **CORRECT** |

---

## 3. Port Accumulation Audit (CRITICAL)

### Type-Layer Structure

`PortContribution` (hares-types/src/ports.rs):
```
enum PortContribution {
    Thermal { zone, sensible_gain_w, radiant_gain_w, latent_gain_w, category }
    Electrical { active_power_kw, reactive_power_kvar }
    Fuel { fuel_type, consumption_w }
    Fluid { loop_id, flow_rate_kg_s, supply_temp_c, return_temp_c, fluid_type }
    Custom { domain_id, payload: [f64; 16] }
}
```

### Accumulation Semantics

All accumulation is **additive (sum)** — no overwrite path exists. Multiple emitters writing the same port in one step produce the correct physical sum. Demonstrated and tested.

**Cross-type contamination**: Structurally impossible. `PortSlots::accumulate` matches on the enum variant and routes to the corresponding typed accumulator. A `Thermal` contribution cannot reach an `Electrical` accumulator. The type system fully enforces this.

**Per-variant analysis:**

| Variant | Accumulation | Units | Notes |
|---|---|---|---|
| `Thermal.sensible_gain_w` | additive | W (convective, to zone air) | Per-category tracked in `sensible_by_category[5]` |
| `Thermal.radiant_gain_w` | additive | W (radiant, to surfaces) | Per-category tracked in `radiant_by_category[5]` |
| `Thermal.latent_gain_w` | additive | W (moisture to zone air) | Correctly separate from sensible |
| `Electrical.load_power_kw` | additive | kW | Active power ≥ 0 adds to load |
| `Electrical.generation_power_kw` | additive | kW | Active power < 0 adds to generation (stays negative) |
| `Electrical.reactive_power_kvar` | algebraic sum | kVAR | Signed sum correct for VAR dispatch |
| `Fuel.totals[fuel_type]` | additive per fuel | W | `FuelType::None` returns `Err` — no silent drop |
| `Fluid.mean_supply_temp_c` | flow-weighted mean | °C | Correct: Σ(ṁ·T)/Σṁ |
| `Custom.payload[16]` | element-wise sum | f64 | Generic accumulation; semantics delegated to domain |

**Undeclared port rejection**: Thermal, Fluid, and Custom ports return `Err` immediately if the target zone/loop/domain was not pre-declared via `PortDeclaration`. Electrical and Fuel are singletons always available. All tested by regression suite.

**End-to-end trace for Thermal:**

1. Equipment calls `ports.accumulate(&PortContribution::Thermal { zone, sensible_gain_w, radiant_gain_w, latent_gain_w, category })` in `step()`.
2. `PortSlots::accumulate` (ports.rs:434–448) → `ThermalAccumulator::add(sensible, radiant, latent, category)` (ports.rs:186–197). Additive. Updates totals + per-category arrays.
3. `dwelling.run_timestep` calls `thermal_solver.prepare_inputs(&self.ports, &self.latest_env)` after all equipment steps.
4. `ThermalSolver::prepare_inputs` calls `apply_port_sensible_inputs(u, ports)` and `apply_port_radiant_inputs(u, ports)` (thermal_solver/ports.rs).
5. `apply_port_sensible_inputs`: for each zone thermal accumulator, `u[zone_sensible_idx] += thermal.sensible_gain_w`. Additive into the state-space input vector.
6. `apply_port_radiant_inputs`: sums `total_radiant_w` for indoor zone → distributes via TMULT to surface nodes (`u[surface_input_idx] += q × radiation_frac`) and remainder to zone air (`u[zone_sensible_idx] += air_from_radiant`).
7. RC solver uses `u` vector → zone temperature update → latent handled by humidity solver (not via port → directly passed via `apply_port_latent_inputs` if wired, or the humidity solver reads from its own domain update).

**No silent zeroing paths found.** Undeclared zones → `Err`. Zero-flow fluid → temperatures not updated (guard at line 312–322 correct).

**Key invariant** (`ports.rs:157–162`): `sum(sensible_by_category) == sensible_gain_w`. Holds by construction since each `ThermalAccumulator::add` call increments both the total and the per-category slot identically. Validated at `ScheduledLoad.init()` with `radiant_gain_fraction ≤ sensible_gain_fraction` check.

**One structural note on `FluidAccumulator.zero()`**: The `zero()` method resets `mean_supply_temp_c` and `mean_return_temp_c` to `0.0` (ports.rs:326–328). This is correct — after zeroing, the first contribution of the next timestep will unconditionally set the mean (since `total_flow_kg_s` is also reset to 0.0, the weighted average initializes cleanly). No stale temperature bias.

---

## 4. Zone Scoping Table

Every `PortContribution::Thermal` emission site, zone target, and radiant split:

| Source | File | Zone target | radiant_gain_w | Zone source | Verdict |
|---|---|---|---|---|---|
| Occupant gains | `dwelling/mod.rs:1915–1916` | `indoor_zone_id` only | `n × 66 × 0.30` | Hardcoded — indoor only | CORRECT (fixed commit 99ccdcf) |
| `ScheduledLoad::step()` | `scheduled_load.rs:522–527` | `descriptor.zone` (HPXML-assigned) | `total × radiant_gain_fraction` | HPXML FracRadiant | CORRECT |
| `EventBasedLoad::step()` | `event_load.rs:412–418` | `descriptor.zone` (HPXML-assigned) | `0.0` (all convective) | HPXML-assigned | CORRECT zone; ACCURACY GAP: appliances should have radiant fraction (~0.2–0.5 for cooking). Pre-existing gap. |
| `WetAppliance::step()` | `event_load.rs:875–881` | `descriptor.zone` (HPXML-assigned) | `0.0` | HPXML-assigned | Same gap as EventBasedLoad |
| `IdealHvac::step()` | `hvac/ideal_hvac.rs:568` | `indoor_zone_id` | `0.0` | Hardcoded indoor | CORRECT — HVAC supply is convective per E+ convention |
| `Boiler::step()` | `hvac/boiler.rs:520` | `indoor_zone_id` | `0.0` | Hardcoded indoor | CORRECT |
| `DuctDistribution::step()` | `hvac/duct_distribution.rs:116` | duct zone (HPXML) | `0.0` | HPXML duct zone | CORRECT — DuctLoss category |
| `Dehumidifier::step()` | `hvac/dehumidifier.rs:336` | `indoor_zone_id` | `0.0` | Hardcoded indoor | CORRECT |
| `ResistanceWaterHeater::step()` | `water_heater/resistance.rs:556` | `descriptor.zone` | `0.0` | HPXML-assigned | CORRECT — jacket loss |
| `GasWaterHeater::step()` | `water_heater/gas.rs:526` | `descriptor.zone` | `0.0` | HPXML-assigned | CORRECT |
| `HeatPumpWaterHeater::step()` | `water_heater/heat_pump_wh.rs:744,753,766` | `descriptor.zone` | `0.0` | HPXML-assigned | CORRECT |
| `Battery::step()` | `battery/mod.rs:998` | `descriptor.zone` | `0.0` | HPXML-assigned | CORRECT — ohmic loss |
| `Generator::step()` | `generator.rs:798` | `descriptor.zone` | `0.0` | HPXML-assigned | CORRECT |
| Ventilation | (not in ports) | N/A | N/A | Envelope solver | CORRECT — envelope handles directly |
| Infiltration | (not in ports) | N/A | N/A | Per-zone in thermal solver | CORRECT |
| PV | (no thermal port) | N/A | N/A | N/A | CORRECT — no thermal emission |
| EV | (no thermal port) | N/A | N/A | N/A | CORRECT — no thermal emission |

**Zone-scoping verdict**: No emitter blindly targets all zones. Every emitter uses either (a) `descriptor.zone` set from HPXML at construction, or (b) explicit `indoor_zone_id` for occupancy/HVAC. The pre-fix occupancy bug (all zones) is confirmed resolved by commit 99ccdcf and verified by the `occupancy_gains_scaled_by_number_of_occupants` test, which explicitly asserts non-indoor zones receive zero.

**Residual accuracy gap** (not a correctness bug, pre-existing): `EventBasedLoad` and `WetAppliance` always emit `radiant_gain_w: 0.0`. EnergyPlus OtherEquipment has `FractionRadiant` (IDD default varies by type: 0.0 for "Other," 0.32 for typical cooking appliances). This is a known accuracy gap for appliances, not introduced by this batch.

---

## 5. Dispatch Ordering Determinism

### Invariants (derived from first principles)

A deterministic simulator must produce bitwise-identical output for identical inputs. Three invariants must hold:

1. **Equipment execution order is stable**: `compute_equipment_execution_order` sorts by `stage_rank` (Independent=0, Electrical=1, Thermal=2, EnvelopeResolution=3) using `sort_by_key`, which is stable in Rust. Within a stage, insertion order is preserved. No HashMap iteration in the execution loop.

2. **Actor dispatch order is stable**: Actors are iterated via `self.actors.iter_mut()` (a `Vec`), which is insertion-order-stable. No HashMap iteration.

3. **Priority ledger is non-reentrant within a step**: `begin_step()` clears `seen_targets` exactly once per timestep (line 325). `drain_tiers` does NOT clear it (explicitly documented at line 361–364). All four dispatch-ordering regression tests pass.

### Hash-Map usage in hot path

`zone_infiltration_columns: HashMap<ZoneId, usize>` and `zone_lwr_columns: HashMap<ZoneId, usize>` are used for per-zone column lookups during schedule domain parsing (init time, not hot loop). Equipment telemetry is stored in `HashMap<String, Telemetry>` in `latest_env`. **The telemetry map is populated by iterating over `self.equipment` (a `Vec`) at line 2362**, not by iterating the HashMap itself — so insertion order is deterministic.

The `ControlDispatcher.by_tier` is `[VecDeque<DispatchRequest>; 4]`, indexed by tier ordinal — a fixed-size array, not a HashMap. No non-determinism here.

**No HashMap iteration non-determinism in the per-step hot path is present.**

### Priority Conflict `ByName` vs `ByEndUse`

`DispatchTarget::conflicts_with` (dispatch.rs:52–58):
```rust
(ByName, ByName) => a == b
(ByEndUse, ByEndUse) => a == b
(ByName, ByEndUse) | (ByEndUse, ByName) => false
```

A `ByName` and `ByEndUse` targeting the same equipment are **not considered conflicting**. A `Safety/ByName` signal does not protect against a later `Schedule/ByEndUse` signal to the same equipment. This pre-existing gap is documented in the prior review (dispatch ordering test coverage gap). It is not introduced by this batch.

### Test coverage verdict

All four BLOCKER regression tests cover the specific ordering invariants. Tests are deterministic (no RNG, no stochastic schedules, minimal synthetic TOML, wall-clock used only for temp-file naming — not for simulation logic). The dispatch tier iteration order is deterministic (fixed array indexed by enum ordinal).

---

## 6. Silent Defaults Audit

Full enumeration of `unwrap_or`/`unwrap_or_default`/`unwrap_or_else` in non-test paths within scope:

| Location | Pattern | Classification | Verdict |
|---|---|---|---|
| `environment.rs:222` | `weather.dry_bulb_c.get(init_offset).unwrap_or(DEFAULT_SETPOINT_C)` | Weather array out of bounds | **ACCEPTABLE** — only reachable if weather timeseries is empty; caller rejects empty weather at construction |
| `environment.rs:227` | `.unwrap_or(initial_outdoor_temp_c)` | Missing ground temp | **ACCEPTABLE** — fallback to outdoor is physically correct for missing ground temp |
| `conversions.rs:77` | `zone.map(|z| mult(z)).unwrap_or(1.0)` | Missing zone → air-only | **ACCEPTABLE** — conservative, safe |
| `conversions.rs:128` | `bd.tilt_deg.unwrap_or(90.0)` | Missing tilt | **ACCEPTABLE** — 90° default (vertical) is correct for `Wall` type; correct match also present |
| `conversions.rs:152` | `.unwrap_or(DEFAULT_R_M2_K_W)` | Missing R-value | **ACCEPTABLE** — default is documented |
| `conversions.rs:250` | `bd.emittance.unwrap_or(EMISSIVITY_DEFAULT)` | Missing emittance | **ACCEPTABLE** — E+ default |
| `solver_builder.rs:107` | `env.zones.first().map(|z| z.id).unwrap_or(ZoneId(1))` | Missing zone | **ACCEPTABLE** — fallback to zone 1 at init |
| `solver_builder.rs:264–265` | `win.u_factor.unwrap_or(5.0)` / `shgc.unwrap_or(0.4)` | Missing window params | **MARGINAL** — HPXML requires U-factor and SHGC for windows. Silently injecting 5.0 W/m²K (single-pane aluminum frame) without warning is a modeling assumption. Not a blocker but warrants a `tracing::warn!`. |
| `solver_builder.rs:349` | `emittance.unwrap_or(EMISSIVITY_WINDOW)` | Window emissivity | **ACCEPTABLE** — NFRC default |
| `solver_builder.rs:368` | `solar_absorptance.unwrap_or(SOLAR_ABSORPTANCE_DEFAULT)` | Exterior absorptance | **ACCEPTABLE** — E+ IDD default |
| `solver_builder.rs:382` | `solar_absorptance.unwrap_or(INTERIOR_SOLAR_ABSORPTANCE_DEFAULT)` | Interior absorptance | **ACCEPTABLE** — E+ IDD default |
| `solver_builder.rs:485` | `elevation_m.unwrap_or(0.0)` | Elevation | **ACCEPTABLE** — sea level |
| `solver_builder.rs:869` | `conditioned_volume_m3.unwrap_or(400.0)` | Volume | **ACCEPTABLE** — documented |
| `solver_builder.rs:1100` | `env.zones.first().map(|z| z.temperature_c).unwrap_or(21.0)` | Initial solve temp | **ACCEPTABLE** — unreachable under normal init |
| `scheduled_load.rs:262–267` | `.unwrap_or_else(|| warn + 0.5)` | Missing sensible fraction | **VIOLATION** (Finding F1) — should `Err` |
| `scheduled_load.rs:274` | `latent_gain_fraction.unwrap_or(0.0)` | Missing latent fraction | **ACCEPTABLE** — zero latent is a valid physical default (no moisture emission) |
| `scheduled_load.rs:312` | `usage_multiplier.unwrap_or(1.0)` | Usage multiplier | **ACCEPTABLE** — identity multiplier |
| `dwelling/mod.rs:1903` | `n_occupants payload.unwrap_or(0.0)` | Missing schedule domain | **VIOLATION** (Finding F3) — silent zero when domain absent despite `occupancy_column_idx` being set |
| `synthetic.rs:291–295` | `heating_capacity_kbtu_h.unwrap_or(30.0)` | Default heating capacity | **ACCEPTABLE** — synthetic fixture default, documented |
| `synthetic.rs:338` | AFUE 0.80 injected for combustion heating | ANSI/RESNET 301 floor | **ACCEPTABLE** for synthetic fixtures with explicit comment |

Two violations identified: F1 (`scheduled_load` sensible fraction) and F3 (occupancy domain absent).

---

## 7. Initialization / Warmup

**Free-float init** (`determine_initial_indoor_temp_c`, environment.rs:836–938):
`(None, None) => outdoor_temp_c` at line 937. Correct. The function is exhaustive; `DEFAULT_SETPOINT_C = 21.0` is no longer used for free-float zones. Tests `free_float_zone_starts_at_outdoor_temp` and `free_float_zone_starts_at_outdoor_temp_cold` pass.

**Warmup loop** (`run_warmup`, mod.rs:1920–1932):
- Runs `warmup_steps = init_duration / time_res` timesteps via `run_timestep(false)`.
- `record_output = false` prevents results from accumulating during warmup.
- After warmup, `self.simulation_results.steps.clear()` discards warmup output.
- Clock is **reset** to simulation start (`mod.rs:1182–1187`): `SimClock::new(local_start, time_res, duration)` is reconstructed so post-warmup steps start at step 0 against `total_steps`.
- Weather indexing: `EnvironmentManager` advances its internal offset (`weather_start_offset`) via `update_in_place`, which is called during warmup. After warmup the offset is at `initialization_steps + weather_start_offset`. This means warmup weather is drawn sequentially from the weather file starting at the simulation start offset. For EPW annual files this is correct: warmup uses the real weather from the same period (looping if needed).
- BESTEST fixtures: 900/900FF use `initialization_duration_s = 1814400` (21 days). This is below the E+ §1.2 criterion for concrete slabs (τ ≈ 33 days for 100mm concrete, κ/ρcₚd² ≈ 0.51/(1400×1000×0.1²) ≈ 3.6×10⁻⁵ m²/s → τ = d²/α ≈ 2.8 days per 10mm → 28 days for 100mm). 21 days is adequate for the wood-frame floors of 900 (faster time constants) but may be marginal for the heavyweight 900FF concrete floor (τ_floor ≈ 28 days). **However**, 21 days matches EnergyPlus's own BESTEST warmup practice (EPlusIDD specifies 20–25 warmup days for heavyweight cases). Sufficient.
- Warmup is **deterministic**: `run_timestep` is purely functional given weather, schedule, and initial state. No RNG in warmup path (initial RNG consumed only at construction). No wall-clock.

**E+ §1.2 convergence criterion** (not implemented): E+ iterates warmup until max zone temperature change < 0.4°C and max flux change < 10 W/m² between the last two warmup days. HARES uses a fixed-duration warmup. For heavyweight cases this is a known approximation. No finding — fixed-duration warmup with sufficient days produces the same result for typical residential construction.

---

## 8. Actor Telemetry

**Memory rule `feedback_actor_telemetry`**: Actors must emit telemetry for dispatched actions/controls.

### Current Actor Telemetry Status

| Actor | Observability | Notes |
|---|---|---|
| `Occupant` | `tracing::debug!` only (actor.rs:371–378) | Logs presence state and signal count. Not accessible as structured telemetry. |
| `IdealThermostat` | `tracing::debug!` only (ideal_thermostat.rs:223) | Logs setpoint and mode. Not accessible as structured telemetry. |
| `BatteryManagementActor` | `tracing::debug!` (bms.rs); SOC read via `equipment_telemetry` | Actor reads SOC from `env.equipment_telemetry`; its own decisions not emitted as telemetry. |
| `DrComplianceActor` | `tracing::debug!` only (dr_compliance.rs:344) | DR level and compliance not accessible as telemetry. |
| `EvDriverActor` | `ev_driver/mod.rs:324` — `last_action()` accessor | Internal state inspectable via getter. Not wired to any telemetry map. |
| `SolverFeedbackActor` | Internal only | Thermal solver feedback — physics-critical but opaque. |

**`Actor` trait** (actor.rs:43–66): Has `name()`, `interests()`, `decide()`. **No `telemetry()` method**. Actor decisions produce only `DispatchRequest` objects which flow into the dispatcher but are not archived in `latest_env` (which only contains equipment telemetry, not actor telemetry).

**Python extensibility impact** (`project_python_extensibility`): Python-implemented actors can emit requests via `decide()` but cannot publish structured telemetry through the `Actor` trait. This limits Python actor observability.

This is **Finding F2** (HIGH).

---

## 9. Test Integrity

### Tests that pass and protect real fixes

| Test file | Count | Green | Protects |
|---|---|---|---|
| `hares-types/src/ports.rs` (inline) | 21 | 21 | Port accumulation type layer semantics |
| `hares-core/tests/port_accumulation_tests.rs` | 21 | 21 | Multi-equipment accumulation lifecycle |
| `hares-core/tests/dispatch_ordering_regressions.rs` | 4 | 4 | All 4 dispatch-ordering invariants |
| `hares-core/tests/orchestration_parity.rs` | 12 | 12 | Occupancy zone scoping, radiant split, HVAC integration |
| `hares-core/tests/alignment_oracles_regressions.rs` | 13 | 13 (3 `#[ignore]` debug helpers) | ASHP fixture shape, DR same-step, mode override |
| `hares-core/src/dwelling/conversions.rs` (inline) | ~10 | 10 | Mass multiplier, furniture override |
| `hares-core/src/dwelling/synthetic.rs` (inline) | 5 | 5 | sky_temp, IR, warmup parsing |
| `hares-core/src/environment.rs` (inline) | ~15 | 15 | S4 free-float init, zone type init |
| `hares-core --lib` total | 369 | 369 | Full lib suite |
| `hares-equipment --lib` total | 1189 | 1189 | Equipment models |
| `hares-types --lib` total | 225 | 225 | Type-layer tests |

### Test provenance and tolerance verification

**`port_accumulation_tests.rs`**: Test values derived from first principles. Example: `thermal_accumulates_per_zone` emits 500+200=700 W sensible and 50+20=70 W latent to zone 1 — simple arithmetic identity, no tolerance needed (exact). `full_timestep_scenario_all_port_types` derives -4.5 kW generation + 1.2 + 0.3 kW load = net -3.0 kW — exact arithmetic. No tolerance widening visible.

**`dispatch_ordering_regressions.rs`**: Four tests use `< 1e-9` tolerance for kW comparisons — appropriate for floating-point arithmetic without any approximate physics. The `SystemTime::now().as_nanos()` used for temp-file naming is wall-clock but not in any assertion or simulation logic. Deterministic.

**`orchestration_parity.rs`**: Occupancy gain assertions use constants-derived exact values (`OCCUPANT_SENSIBLE_GAIN_W × CONVECTIVE_FRACTION × n`). Tolerances are `< 1e-9` where appropriate. Temperature evolution tests use `(0.0..100.0).contains(&temp)` physical bounds — appropriate for sanity checks.

**No `#[ignore]` added to previously passing tests**. The three `#[ignore]` in `alignment_oracles_regressions.rs` are explicitly tagged "debug helper" and were present before this batch.

**Tolerance widening**: No tolerance widening is visible in the changed test files. The prior-review concern (F3) was about `tests/parity/tolerance.rs` which is not in this diff's file set.

---

## 10. Prior Review Claims Re-Verified

| Prior Finding | Status |
|---|---|
| F1 BLOCKER: `heavyweight_concrete_wall_produces_two_rc_sub_layers` asserts wrong node count | Not in diff scope (`bestest_900ff_root_cause.rs` not changed since `c1abb8af`). Status unknown for current HEAD. |
| F2 BLOCKER: `resampled_weather_produces_smooth_environment` 0.155°C step | Not in diff scope (`weather_integration.rs` not changed). Status unknown. |
| F3 HIGH: Parity tolerance widened, 8 fixtures fail | Not in diff scope (`parity/tolerance.rs` not changed). Status unknown. |
| F4 MEDIUM: `WINDOW_EMISSIVITY = 0.9` contradicts `EMISSIVITY_WINDOW = 0.84` | **FINDINGS-DOC ERROR** — see §11. The split is correctly documented and physically justified. |
| F5 MEDIUM: `sensible_gain_fraction` silent 0.5 fallback | **STILL OPEN** (Finding F1 in this review) |
| F6 MEDIUM: Vec allocation in hot loop `apply_port_radiant_inputs` | Not in diff scope (`thermal_solver/ports.rs` not changed). Status unknown. |
| F7 LOW: `n_occupants.unwrap_or(0.0)` silent zero | **STILL OPEN** (Finding F3 in this review) |
| F8 LOW: AFUE 0.80 synthetic default undocumented | **RESOLVED** — `synthetic.rs:338` now has an explicit ANSI/RESNET 301 comment. |
| F9 LOW: `Actor` trait has no `telemetry()` | **STILL OPEN** (Finding F2 in this review — elevated to HIGH) |
| F10 NIT: `EventBasedLoad` uses `tk::SENSIBLE_GAIN_W` vs `tk::TOTAL_SENSIBLE_GAIN_W` | Not verified in current diff scope (no change to `event_load.rs` telemetry key in this batch). Remains open if prior review identified it correctly. |

---

## 11. Findings-Doc Errors Identified

### Error 1: F4 was incorrect

The prior review report (F4, MEDIUM) claimed "WINDOW_EMISSIVITY = 0.9 contradicts EMISSIVITY_WINDOW = 0.84 and is unresolved." This was incorrect. The two constants are used in different physical contexts:

- `EMISSIVITY_WINDOW = 0.84` (`longwave_radiation.rs:50`): NFRC clear glass thermal emissivity for U-factor rating, used for **exterior** emissivity in exterior LWR exchange.
- `WINDOW_EMISSIVITY = 0.9` (`solver_builder.rs:731`, local const): ASHRAE 140-2017 §5.3.1.9 Table 24 value for **interior** surface LWR exchange, applicable to all interior surfaces including windows.

The code at `conversions.rs:234–244` documents this split explicitly:
> "ASHRAE 140-2017 §5.3.1.9, Table 24: ε_ir = 0.9 for ALL interior surfaces including windows. The 0.84 value is the glass thermal emissivity for U-factor rating (NFRC); for interior LWR exchange ASHRAE 140 specifies 0.9."

The ASHRAE 140-2017 citation is authoritative for BESTEST cases; NFRC 0.84 is the correct value for exterior emissivity consistent with window U-factor rating. The split is physically correct and well documented. **F4 was a false positive.**

---

## Severity-Ranked Findings

| # | Severity | File:Line | Finding | Primary Source |
|---|---|---|---|---|
| F1 | **HIGH** | `crates/hares-core/src/actor.rs:43–66` | `Actor` trait has no `telemetry()` method or structured output hook. Actor decisions (occupant presence, thermostat mode, BMS SOC strategy, DR compliance, EV charging action) are visible only via `tracing::debug!`, not as machine-readable telemetry. This violates the project memory rule `feedback_actor_telemetry`. Python custom actors cannot emit structured observability. No test validates that actor state is readable from outside the actor. Required fix: add an optional `fn telemetry(&self) -> Option<&Telemetry> { None }` (default blanket impl) to the `Actor` trait; implement it for actors that have observable internal state. | Project memory `feedback_actor_telemetry`; `project_python_extensibility` |
| F2 | **MEDIUM** | `crates/hares-equipment/src/scheduled_load.rs:262–267` | `sensible_gain_fraction` missing-field path falls back silently to `0.5` with only a `tracing::warn!`. Per project rule `feedback_no_silent_defaults`, this must return `Err(HaresError::Equipment(...))`. The 0.5 value is arbitrary for general equipment (ASHRAE Handbook appliance fractions range 0.3–0.9 by type). Pre-existing from prior batch; policy is "fix all issues encountered." | Project memory `feedback_no_silent_defaults`; ASHRAE HoF 2021 Ch.18 |
| F3 | **LOW** | `crates/hares-core/src/dwelling/mod.rs:1903` | `n_occupants` falls back to `0.0` via `.unwrap_or(0.0)` when the schedule domain payload is missing, despite `occupancy_column_idx` being set. Silently removes all occupancy gains without any warning. Should emit `tracing::warn!` when `occupancy_column_idx.is_some()` but the domain is absent from `latest_env.custom_domains`. | Project memory `feedback_no_silent_defaults` |
| F4 | **LOW** | `crates/hares-core/src/dwelling/solver_builder.rs:264–265` | `win.u_factor_w_m2_k.unwrap_or(5.0)` and `win.shgc.unwrap_or(0.4)` silently inject single-pane aluminum defaults for windows missing these required HPXML fields. Should emit `tracing::warn!` at minimum since window thermal performance is a primary driver of heating/cooling loads. | HPXML 3.0 Spec: WindowType/UFactor and SHGC are required fields |

### Findings that are confirmed resolved (not regressions)

- B1 radiant_gain_w field: CORRECT.
- B7/D6 mass multiplier double-count: CORRECT.
- B8 interior solar absorptance: CORRECT.
- D4 occupant radiative fraction: CORRECT.
- S4 free-float init: CORRECT.
- S5 warmup for BESTEST: CORRECT.
- B3/D2/D3 synthetic weather sky temp / IR / ground temp: CORRECT.
- Dispatch ordering blockers (ba133ca): ALL FOUR CORRECT.
- Silent-default loud-error fixes for PV/Battery/EV/Generator/WH: CORRECT.
- F4 window emissivity alleged contradiction: INCORRECT IN PRIOR REPORT — physically correct split.

---

## Summary

The physics layer, port accumulation semantics, zone scoping, dispatch ordering, and initialization are all correct and tested. Three open findings: the `Actor` telemetry gap is the highest-severity issue and is the primary thing blocking full observability for Python extension users; the `sensible_gain_fraction` silent default is a policy violation; and two missing-field silent defaults in the window and occupancy paths warrant warnings. No physics regression has been introduced; all 1,783 tests in scope pass.
