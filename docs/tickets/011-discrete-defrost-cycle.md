# Discrete Defrost Cycle Modeling

**Severity**: High
**Priority**: P1
**Status**: Open
**Areas**: hares-equipment/hvac/heat_pump, hares-equipment/hvac

## Problem

HARES computes a continuous `defrost_time_fraction` each timestep but never transitions the equipment into a discrete defrost state. EnergyPlus models explicit defrost ON/OFF cycles with distinct capacity/EIR behavior during each phase. The continuous approach:

- **Averages the penalty** across the timestep, underestimating peak power draw during defrost (reverse-cycle defrost draws compressor power at heating-mode EIR or worse)
- **Overestimates average capacity** during frost accumulation — the continuous model assumes capacity degrades smoothly, but real coils maintain near-rated output until frost triggers a discrete defrost cycle
- **Cannot model recovery periods** — after a defrost cycle ends, there is a ~1 minute transient where capacity ramps back up; HARES has no concept of this

The `defrost_accumulator_s` field exists in `HeatPumpHeaterCore` (line 96) and is incremented during defrost steps (line 743), but it is never used to trigger a discrete state transition — it only tracks total defrost time for state serialization.

## Current Behavior

1. **Defrost evaluation is stateless** — `defrost.rs:112-233`: `evaluate_defrost` is a pure function that takes current conditions and returns a `DefrostResult`. It has no memory of previous frost accumulation or defrost cycle state.

2. **Continuous time_fraction** — `defrost.rs:137-138` (OnDemand):
   ```rust
   let time_fraction = (1.0 / (1.0 + DEFROST_TIME_FRACTION_NUMERATOR / delta_omega)).clamp(0.0, 1.0);
   ```
   This gives the *fraction* of time in defrost, not an ON/OFF decision.

3. **Capacity/EIR are averaged** — `heater.rs:987-1016`: when `defrost.active`, capacity and power are multiplied by continuous multipliers:
   ```rust
   hp_capacity_w = (hp_capacity_w * defrost.capacity_multiplier - defrost.q_defrost_w).max(0.0) * crf;
   hp_electric_w = hp_electric_w * defrost.power_multiplier + defrost.extra_power_w;
   ```

4. **No defrost state machine** — `defrost_active` in `HeatPumpHeaterCore` (line 94) is a boolean set from the `DefrostResult.active` field, which is `true` whenever OAT < 4.4445°C. It does not track frost accumulation vs. defrosting vs. recovery.

5. **OCHRE comparison** — OCHRE `HVAC.py:1112-1166` also uses a continuous defrost model with averaged penalties. Both OCHRE and HARES share this simplification relative to EnergyPlus.

6. **EnergyPlus discrete model** — E+ `Coil:Heating:DX` objects model explicit defrost cycles:
   - Defrost cycle duration: typically 3–5 minutes (configurable via `Defrost Time Period`)
   - Maximum defrost time: 10 minutes
   - Recovery period: ~1 minute after defrost ends
   - During Defrosting: capacity = 0 (or reduced), EIR distinct from normal heating
   - The `defrost_time_fraction` formula `dtf = 1 / (1 + 0.01446/delta_omega)` determines what *fraction* of time is spent in defrost cycles, which maps to cycle frequency in the discrete model

## Required Behavior

A discrete defrost model must:

1. **Track frost accumulation** — the existing `defrost_accumulator_s` field tracks seconds of defrost-active time, which is a proxy for frost buildup. The time_fraction from `evaluate_defrost` determines the rate of frost accumulation.

2. **Trigger discrete defrost cycles** — when accumulated frost exceeds a threshold, the equipment transitions to a `Defrosting` state with distinct capacity/EIR behavior.

3. **Model defrost cycle duration** — defrost cycles last a fixed duration (default: ~3–5 minutes, configurable).

4. **Respect maximum defrost time** — no single defrost cycle exceeds 10 minutes.

5. **Provide distinct capacity/EIR during Defrosting**:
   - **ReverseCycle**: compressor reverses; zone capacity = 0 (no net heating or cooling to zone); EIR follows the defrost EIR curve; defrost heat comes from the indoor coil absorbing heat from the zone
   - **Resistive**: compressor off; resistive heater defrosts outdoor coil; zone heating from resistive element only (if present)

6. **After defrost cycle ends, normal compressor capacity resumes immediately.** There is no zone-capacity-level recovery ramp. EnergyPlus `Coil:Heating:DX` resumes normal compressor operation at the end of the defrost period without an artificial ramp. Source: EnergyPlus Engineering Reference §16.5 "DX Heating Coil Defrost Modeling".

### Key formulas (EnergyPlus reference)

- Defrost time period fraction (OnDemand): `dtf = 1 / (1 + 0.01446 / delta_omega)` — already in `defrost.rs:138`. Source: EnergyPlus Engineering Reference §16.5 (frost-rate based defrost time fraction).
- Inter-defrost interval: `interval_between_defrosts_s = cycle_duration_s / time_fraction`. Defrost fires when `time_since_last_defrost >= interval_between_defrosts_s`. Source: EnergyPlus DX heating defrost trigger logic.
- During Defrosting (ReverseCycle): `Q_zone = -q_defrost`, `P_electric = P_rated * defrost_eir_curve + P_resistive`

## Approach

1. **Add `DefrostCycleState` enum** in `defrost.rs`:
   ```rust
   pub enum DefrostCycleState {
       Accumulating,  // Normal heating, frost building on outdoor coil
       Defrosting,    // Defrost cycle active (reverse-cycle or resistive)
   }
   ```

2. **Add `DefrostCycleTracker` struct** in `defrost.rs`:
   ```rust
   pub struct DefrostCycleTracker {
       pub state: DefrostCycleState,
       pub accumulated_frost_s: f64,   // surrogate for frost thickness
       pub defrost_elapsed_s: f64,     // time in current defrost cycle
       pub cycle_duration_s: f64,      // target defrost cycle duration (default 180s)
       pub max_defrost_duration_s: f64, // hard cap (default 600s)
   }
   ```

3. **Use existing `defrost_accumulator_s`** field in `HeatPumpHeaterCore` (line 96) as the frost accumulation proxy. When `evaluate_defrost` returns `active=true`, increment `accumulated_frost_s` by `dt * time_fraction`.

4. **Add frost threshold** — compute the inter-defrost interval: `interval_between_defrosts_s = cycle_duration_s / time_fraction`. When `time_since_last_defrost >= interval_between_defrosts_s`, transition to Defrosting. The accumulated frost time is `defrost_accumulator_s`, which increments each step by `dt_s * time_fraction` when the compressor is running and OAT < 4.4445°C. Source: EnergyPlus DX heating defrost trigger logic.

5. **During Defrosting state** — modify `compute_step` in `heater.rs`:
   - **ReverseCycle**: The indoor coil becomes the evaporator (absorbing heat from indoor air). HARES models this as zone capacity = 0 (no net heating or cooling to zone). This matches EnergyPlus behavior where the defrost period temporarily halts heating delivery without actively cooling the zone. Some E+ configurations model negative capacity (zone cooling), but the zero-capacity model is the common default for residential ASHPs. `hp_electric_w = P_rated * defrost_eir`; set `defrost_active = true`
   - **Resistive**: `hp_capacity_w = 0.0`; `er_capacity_w += resistive_defrost_capacity_w * time_fraction`

6. **After `cycle_duration_s`** → transition back to `Accumulating`. Normal compressor capacity resumes immediately; no artificial ramp. Source: EnergyPlus Engineering Reference §16.5 "DX Heating Coil Defrost Modeling".

7. **Serialize/deserialize** the `DefrostCycleTracker` state in `HeaterState` for checkpoint/restart.

## HPXML Wiring

New fields and their HPXML sources:

| Config Field | HPXML Source | Default |
|---|---|---|
| `defrost_cycle_duration_s` | Not in HPXML; config-only | 210 (3.5 min) |
| `defrost_max_duration_s` | Not in HPXML; config-only | 600 (10 min) |

The existing HPXML fields `DefrostControl` and `DefrostType` are wired in
ticket 014. The discrete cycle parameters are HARES-specific extensions
with no HPXML source — they come from defaults or manual config.

## Definition of Done

- [ ] `DefrostCycleState` enum and `DefrostCycleTracker` struct implemented in `defrost.rs`
- [ ] Inter-defrost interval derived from `cycle_duration_s / time_fraction`; defrost triggers when `time_since_last_defrost >= interval_between_defrosts_s`
- [ ] ReverseCycle Defrosting: zone capacity = 0 (or negative), distinct EIR behavior
- [ ] Resistive Defrosting: zone capacity from resistive element only
- [ ] Normal compressor capacity resumes immediately after defrost ends (no recovery ramp)
- [ ] Maximum defrost duration enforced (10 min hard cap)
- [ ] State serialization includes `DefrostCycleTracker`
- [ ] Peak power draw during defrost is higher than continuous model's averaged power
- [ ] Average capacity over a full accumulate-defrost cycle matches continuous model within 5%
- [ ] Telemetry key `DEFROST_CYCLE_STATE` added (0=Accumulating, 1=Defrosting)
- [ ] Telemetry key `DEFROST_ACCUMULATED_FROST_S` added
- [ ] Telemetry key `DEFROST_ELAPSED_S` added
- [ ] `tracing::debug!` for each FSM state transition: `"defrost FSM: {old} → {new} at frost={frost_s:.1}s"`
- [ ] Output column `Defrost State (-)` added at v7 when discrete model is active

## Verification

1. **Unit test**: Cold conditions (OAT=-5°C, high humidity). After sufficient frost accumulation (`time_since_last_defrost >= cycle_duration_s / time_fraction`), `DefrostCycleTracker.state` transitions from `Accumulating` to `Defrosting`.
2. **Unit test**: During `Defrosting` with ReverseCycle strategy, `hp_capacity_w` must be 0 (or negative); `hp_electric_w` must be non-zero (compressor running in reverse).
3. **Unit test**: After `cycle_duration_s`, state returns to `Accumulating`; normal capacity resumes immediately (no ramp).
4. **Integration test**: Run 1-hour cold-condition simulation. Compare total heating energy between discrete and continuous models. Difference must be < 5% (same average, different instantaneous profile).
5. **Peak power test**: Maximum instantaneous power during discrete defrost must exceed the continuous model's average power for the same timestep by at least 20% (the averaging effect).

## References

- EnergyPlus Engineering Reference, *Coil:Heating:DX* → *Defrost Operation*
subsection. The discrete defrost model uses `Defrost Time Period Fraction`
to determine cycle frequency and `Maximum Defrost Cycle Time` for cycle
duration. See I/O Reference for `Coil:Heating:DX` fields: `Defrost Strategy`,
`Defrost Control`, `Maximum Defrost Cycle Time`.
- EnergyPlus Input/Output Reference, `Coil:Heating:DX` fields: `Defrost Strategy`, `Defrost Control`, `Defrost Time Period Fraction`
- OCHRE `HVAC.py:1112-1166`: `HeatPumpHeater.update_capacity` — continuous defrost model
- HARES `defrost.rs:112-233`: current continuous `evaluate_defrost` implementation
- HARES `constants.rs:6-32`: defrost constants matching EnergyPlus formulas

## Related Tickets

- [010-default-biquadratic-performance-curves.md](010-default-biquadratic-performance-curves.md) — curves interact with defrost capacity multiplier
- [012-heating-side-shr.md](012-heating-side-shr.md) — latent effects during defrost recovery
- [014-defrost-typed-config.md](014-defrost-typed-config.md) — typed config for defrost parameters
