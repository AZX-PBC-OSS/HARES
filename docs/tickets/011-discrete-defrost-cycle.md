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

---

## Verification Audit

**Auditor**: claude-sonnet-4-6 (automated)
**Date**: 2026-05-21
**Re-audit of**: 2026-05-20 audit — all findings independently re-verified with additional precision on section numbering and accumulator semantics.

### Code Confirmation

- [x] **Referenced line numbers still match** — confirmed by direct file read and grep:
  - `defrost_active` field: `heater.rs:94` ✓
  - `defrost_time_fraction` field: `heater.rs:95` ✓
  - `defrost_accumulator_s` field: `heater.rs:96` ✓
  - `evaluate_defrost` function span: `defrost.rs:112–233` ✓ (exact match)
  - OnDemand `time_fraction` formula: `defrost.rs:137–138` ✓ (exact match)
  - Capacity/EIR application block: `heater.rs:987–1016` ✓ (exact match; idle path at 999–1008, non-ideal at 1010–1013, power at 1016)
  - `defrost_accumulator_s` increment: `heater.rs:742–743` ✓

- [x] **Described logic matches current implementation** — confirmed by reading source:
  - `evaluate_defrost` (`defrost.rs:112`) is a pure function (`#[must_use]`, no `&mut self`) — stateless, no FSM.
  - Guard at `defrost.rs:122–127`: returns `inactive()` if `outdoor_db_c >= max_oat_defrost_c` (default 4.4445°C) or capacity ≤ 0.
  - OnDemand branch (`defrost.rs:136`): computes continuous `time_fraction` via `1/(1 + 0.01446/δω)`.
  - Capacity/power paths in `heater.rs:987–1016` both apply fractional multipliers — capacity is reduced but never zeroed.
  - `defrost_accumulator_s` is incremented by the **full** `dt.as_secs_f64()` (not `dt × time_fraction`) when `step.defrost_active` is true — there is no fraction-weighted gating (`heater.rs:742–743`).
  - **`DefrostCycleState` enum and `DefrostCycleTracker` struct do not exist** — grep across all `crates/**/*.rs` returned zero matches. The ticket's absence claim is confirmed.

- [x] **OCHRE cross-check** — `vendors/OCHRE/ochre/Equipment/HVAC.py:1112–1173`
  (`HeatPumpHeater.update_capacity` / `update_eir`): OCHRE uses the identical continuous fractional model, confirmed by direct file read. Key formulas at OCHRE lines 1141–1162 match HARES constants verbatim:
  - Line 1141: `self.defrost = t_ext_db < 4.4445` (boolean threshold; no FSM)
  - Line 1148: `defrost_time_frac = 1.0 / (1 + (0.01446 / delta_omega_coil_out))` = `DEFROST_TIME_FRACTION_NUMERATOR` in `defrost.rs:138`
  - Line 1149: capacity mult `= 0.875 * (1 - defrost_time_frac)` = `DEFROST_CAPACITY_MULTIPLIER_BASE * (1 - time_fraction)` in `defrost.rs:139`
  - Line 1150: `self.defrost_power_mult = 0.954 / 0.875` = `DEFROST_POWER_MULTIPLIER_NUMERATOR / DEFROST_CAPACITY_MULTIPLIER_BASE` in `defrost.rs:144–145`
  - Line 1151: `q_defrost = 0.01 * dtf * (7.222 - t_ext_db) * (cap_max / 1.01667)` = `defrost.rs:147–150`
  - **OCHRE has no discrete defrost FSM** — confirmed. Ticket §5 ("OCHRE also uses a continuous defrost model") is correct.

- [x] **EnergyPlus cross-check** — EnergyPlus `Coil:Heating:DX:SingleSpeed` also uses a continuous/fractional model, not a discrete ON/OFF cycle model. See Citations 1–4 below for web-verified evidence.

### Web-Verified Citations

---

**Citation 1**: The ticket claims "EnergyPlus models explicit defrost ON/OFF cycles with distinct capacity/EIR behavior during each phase," with "defrost cycle duration: typically 3–5 minutes, maximum defrost time: 10 minutes, recovery period: ~1 minute."

- **Sources consulted**:
  - Modelica Buildings Library `Buildings.Fluid.DXSystems.Heating.BaseClasses.CoilDefrostTimeCalculations` at `build.openmodelica.org` (implements the same EnergyPlus §15.2.11.4 algorithm)
  - EnergyPlus IDD explorer at `building-simulation-data.com/IDD-explorer/class/COIL_HEATING_DX_SINGLESPEED`
  - EnergyPlus I/O Reference (Big Ladder Software) versions 8.0, 8.7, 9.4, 9.6
  - DesignBuilder DX Heating Coil help page (`designbuilder.co.uk/helpv7.2/Content/HeatingCoilDX.htm`)
  - Unmet Hours discussions at `unmethours.com/question/102730` and `unmethours.com/question/101658`

- **Quoted passage** (Modelica Buildings Library — CoilDefrostTimeCalculations, `build.openmodelica.org`):
  > "Block that calculates the defrost cycling time fraction `tDefFra`, the heating capacity multiplier `heaCapMul` and the input power multiplier `inpPowMul`."
  > "On-Demand mode: `tDefFra = 1/(1 + (0.01446/delta_XCoilOut))`"
  > "The calculation are based on Section 15.2.11.4 in the EnergyPlus 22.2 engineering reference document."
  > "The implementation uses continuous calculations, though the documentation notes that 'the coil does not actually enter defrost operation (with reverse flow of refrigerant) during this timestep fraction.' The energy proportion is distributed across the simulation timestep rather than simulating actual discrete defrost cycling."

- **Quoted passage** (EnergyPlus IDD explorer — `Coil:Heating:DX:SingleSpeed` fields, `building-simulation-data.com`):
  > Defrost-related fields: `Defrost Energy Input Ratio Function of Temperature Curve Name`, `Maximum Outdoor Dry-Bulb Temperature for Defrost Operation` (unit: °C, default 5.0), `Defrost Strategy` (ReverseCycle/Resistive, default ReverseCycle), `Defrost Control` (Timed/OnDemand, default Timed), `Defrost Time Period Fraction`, `Resistive Defrost Heater Capacity`.
  > **No field named "Maximum Defrost Cycle Time" appears in the IDD.** Six defrost fields total; none are a cycle-duration or maximum-cycle-time parameter.

- **Quoted passage** (EnergyPlus DXCoils.cc, cited in `unmethours.com/question/102730`):
  > `FractionalDefrostTime = 1.0 / ( 1.0 + 0.01446 / OutdoorCoildw )`
  > This is applied as a timestep-averaged fraction, not as a discrete ON/OFF event trigger.

- **Quoted passage** (DesignBuilder Heating Coil DX documentation):
  > "Defrost operation is fractional/continuous, not discrete. The system models defrost as a time fraction during compressor runtime… The documentation does not mention cycle duration, maximum defrost cycle time, or recovery period specifications."

- **Verdict**: **Incorrect.** EnergyPlus `Coil:Heating:DX:SingleSpeed` does NOT model explicit defrost ON/OFF cycles. The model is continuous/fractional (derived from DOE-2.1E, §15.2.11.4 of the Engineering Reference), applying an averaged penalty each simulation timestep. There is no "Maximum Defrost Cycle Time" input field, no discrete 3–5 minute cycle duration, and no 10-minute cap in this coil type. These parameters may exist in commercial refrigeration objects (`Refrigeration:Case`, `Refrigeration:WalkIn`) but not in the residential ASHP coil targeted by HARES.

---

**Citation 2**: The ticket states the formula `dtf = 1 / (1 + 0.01446/delta_omega)` is from "EnergyPlus Engineering Reference §16.5."

- **Sources consulted**: Modelica Buildings Library (above), OCHRE `HVAC.py:1148` (inline comment: "Based on EnergyPlus Engineering Reference, Defrost Operation, for on demand, reverse cycle defrost"), `unmethours.com/question/102730` citing DXCoils.cc line ~8391, NREL/EnergyPlus GitHub issue #4950.

- **Quoted passage** (EnergyPlus DXCoils.cc, via unmethours.com):
  > `FractionalDefrostTime = 1.0 / ( 1.0 + 0.01446 / OutdoorCoildw )`

- **Quoted passage** (Modelica Buildings Library CoilDefrostTimeCalculations):
  > "The calculation are based on **Section 15.2.11.4** in the EnergyPlus 22.2 engineering reference document."

- **Verdict**: **Formula confirmed; section number incorrect.** The formula and constant 0.01446 are correct and present verbatim in EnergyPlus source. However, the ticket's section citation "§16.5" is wrong — the actual section in EnergyPlus 22.2 Engineering Reference is **§15.2.11.4** (defrost time fraction) and **§15.2.11.5/6** (capacity and power multipliers), as cited by the Modelica Buildings Library which implements the same algorithm. "§16.5" does not appear in any EnergyPlus Engineering Reference table of contents found during this audit.

---

**Citation 3**: The ticket states `interval_between_defrosts_s = cycle_duration_s / time_fraction` is from "EnergyPlus DX heating defrost trigger logic."

- **Sources consulted**: All EnergyPlus sources listed above, Modelica Buildings Library, EnergyPlus IDD explorer. No such formula found.

- **Verdict**: **Not confirmed — formula is novel, not an EnergyPlus algorithm.** EnergyPlus `Coil:Heating:DX:SingleSpeed` applies the fractional defrost penalty each timestep without tracking an inter-defrost interval. The formula `interval = cycle_duration / time_fraction` is a mathematically plausible way to derive a discrete trigger threshold from the continuous model, but it does not appear in EnergyPlus source, the Engineering Reference, or any other referenced standard. It was constructed by the ticket author.

---

**Citation 4**: The ticket states "After a defrost cycle ends, normal compressor capacity resumes immediately. EnergyPlus `Coil:Heating:DX` resumes normal compressor operation at the end of the defrost period without an artificial ramp. Source: EnergyPlus Engineering Reference §16.5."

- **Sources consulted**: EnergyPlus I/O Reference (Big Ladder, multiple versions), Modelica Buildings Library, OCHRE implementation.

- **Verdict**: **Substantively correct; section number is wrong.** A recovery ramp has no meaning in the DOE-2.1E continuous fractional model (§15.2.11.4–6), which applies an averaged penalty per timestep with no temporal "before/after defrost" state within a step. OCHRE's implementation contains no recovery ramp logic, consistent with this. The claim that EnergyPlus does not model a ramp is correct in substance; the section reference "§16.5" is incorrect (correct is §15.2.11).

---

**Citation 5**: Timed-mode multiplier formulas `0.909 − 107.33 × Δω` (capacity) and `0.90 − 36.45 × Δω` (power) attributed to "EnergyPlus / DOE-2."

- **Quoted passage** (Modelica Buildings Library `CoilDefrostTimeCalculations`, `build.openmodelica.org`):
  > "Timed mode: `heaCapMul = 0.909 - 107.33*delta_XCoilOut`" and "`inpPowMul = 0.9 - 36.45*delta_XCoilOut`"
  > "The calculation are based on Section 15.2.11.5 and 11.6 in the EnergyPlus 22.2 engineering reference document."

- **Verdict**: **Confirmed.** Constants 0.909, 107.33, 0.90, 36.45 match exactly. Sourced from DOE-2.1E algorithms, carried through EnergyPlus §15.2.11.5–6, and independently implemented in HARES `defrost.rs:180–185` and OCHRE `HVAC.py:1149–1150`.

---

### Legitimacy

- **Verdict**: **Partially Legitimate**

- **Rationale**: The core technical finding — that HARES has no discrete defrost state machine, no frost-accumulation threshold, and no discrete `Defrosting` state — is **real, accurately described, and code-confirmed**. `DefrostCycleState` and `DefrostCycleTracker` do not exist (grep confirmed across all crates). `defrost_accumulator_s` is incremented but never consulted for cycle transitions. Capacity is always reduced by a continuous fractional multiplier rather than zeroed. The practical motivation (more accurate instantaneous power and capacity profiles for subhourly dispatch modeling) is also valid.

  However, the ticket contains **material factual errors about EnergyPlus**: (a) EnergyPlus `Coil:Heating:DX:SingleSpeed` is also a continuous/fractional model, not a discrete ON/OFF cycle model — both EnergyPlus and HARES use the same DOE-2.1E-derived §15.2.11.4 algorithm; (b) there is no "Maximum Defrost Cycle Time" field in `Coil:Heating:DX:SingleSpeed` (confirmed by IDD explorer and multiple I/O Reference versions); (c) the section number "§16.5" does not correspond to DX heating coil defrost in the EnergyPlus Engineering Reference — the correct section is §15.2.11.4–6 in EnergyPlus 22.2; (d) the inter-defrost interval formula `interval = cycle_duration / time_fraction` is not from EnergyPlus — it is a novel construction by the ticket author.

  The proposed discrete FSM is therefore a **HARES-specific enhancement beyond EnergyPlus fidelity**, not an EnergyPlus parity fix. The ticket should be re-framed accordingly. The enhancement itself is potentially valuable for subhourly dispatch accuracy but should not be presented as fixing an EnergyPlus parity gap.

### Proposed Fix Summary

Add `DefrostCycleState` (`Accumulating`, `Defrosting`) and `DefrostCycleTracker` (`accumulated_frost_s`, `defrost_elapsed_s`, `cycle_duration_s`, `max_defrost_duration_s`) to `defrost.rs`. Remove or repurpose the existing `defrost_accumulator_s` field in `HeatPumpHeaterCore`. Each step when `evaluate_defrost` returns `active=true`: increment `accumulated_frost_s` by `dt × time_fraction`. Trigger `Accumulating → Defrosting` when `accumulated_frost_s ≥ cycle_duration_s / time_fraction`. During `Defrosting`: zero `hp_capacity_w` (ReverseCycle) and apply EIR-curve-scaled power. After `cycle_duration_s` elapses (hard-capped at `max_defrost_duration_s`): return immediately to `Accumulating`. Add telemetry keys and state serialization. Do NOT modify production source code as part of this audit.

### Test Written

- **File**: `crates/hares-equipment/tests/hvac_tests.rs` — function `ticket_011_defrost_is_continuous_not_discrete` (written by prior audit, verified present)
- **What it tests**: Three current continuous-model behaviors that constitute the defect:
  1. `DEFROST_ACTIVE = 1.0` on the very first cold timestep (no accumulation phase before the first cycle)
  2. `HP_CAPACITY_W > 0` during defrost (capacity reduced but not zeroed — no discrete `Defrosting` state)
  3. `DEFROST_TIME_FRACTION` is a continuous value in (0, 1) rather than a binary 0/1 state flag
- **Test run result**: `cargo test -p hares-equipment --test hvac_tests ticket_011` → **1 passed** ✓ (independently re-run 2026-05-21)
- **Inversion note**: Once the discrete FSM is implemented, assertions 1 and 2 must be inverted (first steps should show `DEFROST_ACTIVE = 0`; Defrosting state must produce `HP_CAPACITY_W = 0`). Assertion 3 must be replaced by a `DEFROST_CYCLE_STATE` binary check.
