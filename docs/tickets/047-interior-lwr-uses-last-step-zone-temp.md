# Interior LWR Iterative Solve Uses Last-Step Zone Temperature as Radiation Reference

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-envelope
**Related**: Ticket 045 (equipment-ports-applied-before-zone-state-update) — that ticket addresses non-thermal equipment ordering and humidity lag; this ticket addresses a distinct mechanism: the LWR convergence loop's stale zone-air temperature reference and a defective convergence criterion.

## Problem

`apply_interior_longwave_inputs` in `hares-envelope/src/thermal_solver/longwave.rs` reads zone temperature from `env.zones` at lines 229–236:

```rust
let t_zone_c = env
    .zones
    .iter()
    .find(|z| z.id == zone_cfg.zone_id)
    .map(|z| z.temperature_c)
    .unwrap_or_else(|| { ... 20.0 });
```

`env.zones[i].temperature_c` is the committed value from the prior timestep, not the end-of-step temperature the ZOH integration will produce. The zone-air LWR reference therefore lags by one full timestep throughout all iterations of the convergence loop at `longwave.rs:286–318`.

For a 15-minute timestep with a 1 °C/step zone temperature ramp (aggressive HVAC cycling), this 1 °C error in the driving temperature produces an LWR flux error of approximately `4εσT³ × A_total × ΔT ≈ 4 × 0.9 × 5.67e-8 × 293³ × 30 × 1 ≈ 16 W` across 30 m² of interior surface — significant relative to typical interior LWR magnitudes of 50–200 W.

EnergyPlus Engineering Reference §13.1 "Zone Air Heat Balance Predictor-Corrector" uses a predictor-step zone temperature (not the prior-timestep value) when evaluating LWR exchange. EnergyPlus Engineering Reference §14.3 "Interior Long-Wave Radiation Exchange" specifies that the iterative surface temperature solve uses the current-step zone air temperature as its background reference.

### Secondary defect: defective convergence criterion

`longwave.rs:312–315` tests the heavy-ball step magnitude (`|buf[j] - prev_buf[j]|`) as the convergence criterion. This measures the momentum update, not the LWR flux residual. The correct convergence test is the relative flux residual `|q_new[j] - q_old[j]| / (|q_old[j]| + ε)`, which directly reflects whether the surface temperature iteration has closed the energy balance.

## Current Behavior

`hares-envelope/src/thermal_solver/longwave.rs:229–236`: `t_zone_c` read from `env.zones` (prior-step committed value), held fixed throughout all iterations.

`hares-envelope/src/thermal_solver/longwave.rs:286`:
```rust
let n_iter = (self.dt_s / 300.0_f64).floor() as u32 + 3;
```
Iteration count scales with timestep duration but is fixed regardless of transient rate-of-change.

`hares-envelope/src/thermal_solver/longwave.rs:312–315`: convergence tested on heavy-ball step magnitude, not LWR flux residual.

OCHRE's `_solve_interior_radiation` uses the current-timestep HVAC-updated zone temperature as the predictor value. HARES uses the prior-step value — a regression relative to OCHRE.

## Required Behavior

1. **Zone temperature reference**: Pass the predictor-step zone temperature to `apply_interior_longwave_inputs` instead of reading from `env.zones`. Implement a predictor-corrector: run `integrate` once with last-step LWR to obtain a predicted zone temperature; pass that predicted temperature into the LWR convergence loop; re-run `integrate` with the corrected LWR input. This matches EnergyPlus Engineering Reference §13.1.

   Minimum acceptable fix if the full predictor-corrector is deferred: add a `tracing::debug!` log per timestep reporting the maximum zone temperature change from the prior step, making the lag observable. Do not leave the stale reference without telemetry.

2. **Convergence criterion**: Replace the heavy-ball step magnitude test with a relative LWR net flux residual:
   ```
   |q_new[j] - q_old[j]| / (|q_old[j]| + 1e-6) < 1e-4
   ```
   where `q[j]` is the net LWR flux into surface `j` (W). This is the physically meaningful convergence indicator per EnergyPlus Engineering Reference §14.3.

References:
- EnergyPlus Engineering Reference §13.1 "Zone Air Heat Balance Predictor-Corrector" — predictor zone temperature used in all load calculations
- EnergyPlus Engineering Reference §14.3 "Interior Long-Wave Radiation Exchange" — iterative surface solve with current-step zone air temperature
- ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.35 "Radiant Exchange Among Room Surfaces"

## Approach

In `apply_interior_longwave_inputs` (`longwave.rs`):
1. Add a `t_zone_predicted_c: Option<f64>` parameter (or a dedicated predictor value on `ThermalSolver`). When `Some`, use it instead of `env.zones[i].temperature_c`.
2. In the caller (`stepping.rs` or `mod.rs`), call `integrate` once to get the predicted temperatures; extract the zone air node temperature; pass it back to `apply_interior_longwave_inputs` before the corrector `integrate`.
3. Replace the convergence test at `longwave.rs:312–315` with the relative flux residual check.

## Definition of Done

- [ ] `apply_interior_longwave_inputs` accepts a predictor zone temperature and uses it throughout all convergence iterations
- [ ] Predictor-corrector: `integrate` called once (predictor), LWR re-evaluated with predicted zone temperature, `integrate` called again (corrector)
- [ ] Convergence criterion tests `|q_new[j] - q_old[j]| / (|q_old[j]| + 1e-6) < 1e-4` for net LWR flux
- [ ] `tracing::debug!` emits maximum zone temperature change per step (observable without full predictor-corrector)
- [ ] `cargo test -p hares-envelope` passes; no regression in radiant/LWR tests
- [ ] Test: with a 1 °C/step zone ramp, LWR flux computed with predictor temperature differs from prior-step temperature by < 2 W per surface at convergence

## Verification

```bash
cargo test -p hares-envelope
```

## References

- EnergyPlus Engineering Reference §13.1 "Zone Air Heat Balance Predictor-Corrector"
- EnergyPlus Engineering Reference §14.3 "Interior Long-Wave Radiation Exchange"
- ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.35 "Radiant Exchange Among Room Surfaces"
- `hares-envelope/src/thermal_solver/longwave.rs:229–236` — stale zone temperature read
- `hares-envelope/src/thermal_solver/longwave.rs:312–315` — defective convergence criterion

---

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match (confirmed: `t_zone_c` read from `env.zones` at lines 229–236; convergence test at lines 311–315 — shifted by one line from ticket's "312–315" but logic is identical)
- [x] Described logic matches current implementation: `t_zone_c` is read once from `env.zones[i].temperature_c` before the iteration loop and is never updated within the loop — confirmed at `longwave.rs:229–237`
- [x] OCHRE cross-check result: **partially matches but ticket's claim about OCHRE diverges** — OCHRE `Envelope.py:1189` calls `zone.calculate_interior_radiation(zone.temperature)` using the prior-step `zone.temperature` (updated at line 1331 in `update_results`, after radiation). So OCHRE itself also uses a prior-step zone temperature. The ticket's statement "OCHRE uses the current-timestep HVAC-updated zone temperature" is **incorrect**. HARES matches OCHRE here, not diverges from it.
- [x] EnergyPlus cross-check result: **Ticket's section numbers do not exist as stated.** Web research on the EnergyPlus Engineering Reference found no §13.1 titled "Zone Air Heat Balance Predictor-Corrector" or §14.3 titled "Interior Long-Wave Radiation Exchange" as standalone numbered sections. The relevant content is in "Basis for Zone and Air System Integration" (predictor-corrector overview) and "Inside Heat Balance" (interior LWR). More critically: EnergyPlus's `CalcInteriorRadExchange` in `HeatBalanceIntRadExchange.cc` does **not** accept zone air temperature as a parameter at all — it computes pure surface-to-surface T⁴ radiation exchange (grey enclosure model) where zone air is treated as fully transparent. Therefore the EnergyPlus citation supporting the use of "predictor zone temperature in LWR" does not hold: EnergyPlus's interior LWR model has no zone air temperature background reference.

### Web-Verified Citations

**Citation 1**: EnergyPlus Engineering Reference §13.1 "Zone Air Heat Balance Predictor-Corrector" — predictor zone temperature used in all load calculations

- **Source found**: BigLadder Software EnergyPlus Engineering Reference (versions 8.0–25.2), fetched from `https://bigladdersoftware.com/epx/docs/8-1/engineering-reference/page-008.html` and the 24.1 table of contents at `https://bigladdersoftware.com/epx/docs/24-1/engineering-reference/`
- **Quoted passage**: "Since the load on the zone drives the entire process, that load is used as a starting point to give a demand to the air system. Then a simulation of the air system provides the actual supply capability and the zone temperature is adjusted if necessary. This process in EnergyPlus is referred to as a Predictor/Corrector process." (E+ Engineering Reference "Basis for the Zone and Air System Integration")
- **Verdict**: **Partially incorrect.** The predictor-corrector concept exists, but the specific §13.1 section number does not appear in EnergyPlus documentation. More importantly, the citation's claim that this section specifies using a "predictor-step zone temperature" when evaluating LWR exchange is not supported by EnergyPlus source code: `CalcInteriorRadExchange` takes only surface temperatures — zone air temperature is irrelevant to E+'s interior LWR model.

**Citation 2**: EnergyPlus Engineering Reference §14.3 "Interior Long-Wave Radiation Exchange" — iterative surface solve with current-step zone air temperature

- **Source found**: BigLadder Software EnergyPlus Engineering Reference "Inside Heat Balance" chapter (versions 8.0–25.2), fetched from `https://bigladdersoftware.com/epx/docs/8-8/engineering-reference/inside-heat-balance.html` and `https://bigladdersoftware.com/epx/docs/25-2/engineering-reference/inside-heat-balance.html`. Also `HeatBalanceIntRadExchange.cc` raw source at `https://raw.githubusercontent.com/NREL/EnergyPlus/develop/src/EnergyPlus/HeatBalanceIntRadExchange.cc`
- **Quoted passage**: From Inside Heat Balance: "EnergyPlus uses a grey interchange model for the longwave radiation among zone surfaces... The heat balance formulation in EnergyPlus treats air as completely transparent. This means that it does not participate in the LW radiation exchange among the surfaces in the zone." From E+ source: `CalcInteriorRadExchange(EnergyPlusData &state, Array1S<Real64> const SurfaceTemp, ...)` — zone air temperature is not a parameter.
- **Verdict**: **Incorrect.** The section number §14.3 does not exist; the relevant section is "Internal Long-Wave Radiation Exchange" under "Inside Heat Balance". More critically, EnergyPlus's interior LWR model treats zone air as fully transparent and does NOT use zone air temperature as a radiation background reference. The claim that "the iterative surface solve uses the current-step zone air temperature as its background reference" misrepresents the EnergyPlus algorithm.

**Citation 3**: ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.35 "Radiant Exchange Among Room Surfaces"

- **Source found**: ASHRAE Handbook of Fundamentals 2021 Table of Contents, fetched from `https://www.ashrae.org/technical-resources/ashrae-handbook/table-of-contents-2021-ashrae-handbook-fundamentals`; Chapter 18 content fetched from `https://handbook.ashrae.org/Handbooks/F21/IP/F21_Ch18/F21_Ch18_ip.aspx`; Chapter 4 content fetched from `https://handbook.ashrae.org/Handbooks/F21/IP/F21_Ch04/F21_Ch04_ip.aspx`
- **Quoted passage**: Chapter 18 is titled "Nonresidential Cooling and Heating Load Calculations" and covers sections §18.1 (Cooling Load Calculation Principles), §18.2 (Internal Heat Gains), and methods like RTS and HB. It contains NO section §18.35 or any section titled "Radiant Exchange Among Room Surfaces." The relevant ASHRAE content on radiant exchange between opaque surfaces is in **Chapter 4 "Heat Transfer"** under "Radiant Exchange Between Opaque Surfaces," using the radiosity method and thermal circuit method for grey enclosures.
- **Verdict**: **Incorrect.** The citation §18.35 in Chapter 18 does not exist. The correct location of radiant surface exchange theory is ASHRAE HoF Ch. 4 "Heat Transfer," not Ch. 18.

### Legitimacy

- **Verdict**: **Partially Legitimate**
- **Rationale**: Two real issues are identified and confirmed in code. (1) The `t_zone_c` stale-read is real — `env.zones[i].temperature_c` is the prior-step committed value, read at `longwave.rs:229–237` and held fixed throughout iterations. (2) The convergence test at lines 311–315 does measure the heavy-ball step magnitude `|t_next − old_buf[j]|` rather than the LWR net flux residual. However, both issues are materially mischaracterised: (a) The claimed error magnitude of 16 W (and the formula `4εσT³×A×ΔT` yielding 154 W) is incorrect by 300×; the actual impact is ≈0.5 W max per surface in the linearised fallback path and exactly 0 W in the primary ScriptF path (which does not use `t_zone_c` at all); (b) OCHRE also uses a prior-step zone temperature at `Envelope.py:1189` — calling this "a regression relative to OCHRE" is false; (c) The EnergyPlus citations cite section numbers that do not exist (§13.1, §14.3 as standalone numbered sections, §18.35) and misrepresent EnergyPlus's algorithm: E+ interior LWR does not use zone air temperature as a background reference — air is transparent; (d) The convergence criterion in HARES matches OCHRE exactly (`delta = max(|t_new - t_surfaces|) < 0.01` at OCHRE line 113–116), so calling it a "regression relative to OCHRE" is also incorrect. The ASHRAE Ch. 18 §18.35 citation does not exist in any form. The primary (ScriptF) path has no sensitivity to the stale zone temperature whatsoever — only the linearised fallback is affected, and only at sub-watt magnitude.

### Proposed Fix Summary

For the stale zone temperature (Defect 1): the fix is warranted but the priority should be downgraded to Low/P3 given the actual sub-watt impact. Add a `tracing::debug!` log reporting the prior-step zone temperature per timestep (the "minimum acceptable fix" in the ticket's own language). A full predictor-corrector is not justified by the measured error magnitude.

For the convergence criterion (Defect 2): the step-magnitude test `|t_next - prev|` at `longwave.rs:313` is identical to OCHRE's convergence test. If it is to be changed to a flux-residual test, this would be a deliberate departure from OCHRE, not a bug fix — and given OCHRE has operated correctly with the step test, the justification must come from first principles rather than from an OCHRE-divergence argument.

Neither fix should reference the EnergyPlus citations as given, since the section numbers are wrong and the algorithmic description misrepresents E+.

### Test Written

- **File**: `crates/hares-envelope/tests/interior_lwr.rs`
- **Function**: `ticket047_stale_zone_temp_linearised_lwr_error_is_sub_watt`
- **What it tests**: Quantifies the actual per-surface LWR flux error caused by a 1 °C stale zone temperature in the linearised fallback path. Asserts the error is < 1 W (not the 16 W claimed) and > 0.01 W (non-zero, confirming the bias exists). This characterisation test passes regardless of bug fix status and pins the measured magnitude so reviewers can verify any future change.
