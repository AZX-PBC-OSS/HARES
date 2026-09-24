# Heating-Side SHR Always 1.0 (Zero Latent)

**Severity**: Low
**Priority**: P3
**Status**: Open
**Areas**: hares-equipment/hvac/heat_pump, hares-equipment/hvac

## Problem

HP heating is modeled as 100% sensible (`latent_gain_w = 0.0`). The only legitimate zone latent source during DX heating is defrost moisture recovery. Understanding the physics correctly requires distinguishing three regimes:

### Physics

**During normal heating (no defrost)**: The outdoor DX coil is the condenser operating below the outdoor air dewpoint, condensing moisture from outdoor air onto the outdoor coil fins. This condensate stays on the outdoor coil and drains or freezes outdoors. None of it enters the supply airstream or the zone. `latent_gain_w = 0.0` during normal heating is physically correct.

**During reverse-cycle defrost**: The refrigerant circuit reverses, making the outdoor coil the evaporator and the indoor coil the condenser. Frost on the outdoor coil melts; the meltwater drains outdoors as liquid. The indoor coil, now acting as a condenser, heats supply air without any moisture addition. The melted frost from the outdoor coil does NOT travel through the refrigerant circuit or enter the indoor supply stream. Per EnergyPlus Engineering Reference §16.5 "DX Heating Coil Model": defrost moisture treatment does not include condensate carry-over from the outdoor to indoor coil; any zone latent effect during defrost must be computed from the supply air conditions during reverse-cycle operation, not from outdoor coil drainage.

**After defrost recovery (transient)**: Once reverse-cycle defrost ends and normal heating resumes, residual moisture on the indoor coil surface (from the brief period when it acted as an evaporator) can evaporate into the supply air. This is the only path by which moisture enters the supply airstream in heating mode. The effect is small and transient.

The magnitude of the zone latent effect from defrost is bounded by: for a defrost event of duration `t` seconds with reverse-cycle power `P` W and heating SHR during defrost `SHR_defrost`, `latent_gain = P * (1 - SHR_defrost) * t`. For a 1 kW reverse-cycle defrost at `SHR_defrost = 0.95`, latent = 50 W per defrost minute — a small but non-zero zone load.

## Current Behavior

1. **Heating thermal output has zero latent component** — `heater.rs:696-701`:
   ```rust
   if step.thermal_output_w > 0.0 {
       self.hvac.write_zone_thermal_contributions(
           ports,
           step.thermal_output_w,
           0.0,  // <-- latent_gain_w is always zero
           ThermalCategory::HvacHeating,
       )?;
   }
   ```
   The `0.0` literal for `latent_gain_w` is hardcoded for all heating output.

2. **Cooling-side SHR model exists** — `coil_physics.rs:214-286` implements a full psychrometric SHR calculation with bypass factor, ADP iteration, and the Henderson-Rengarajan latent degradation model. This is used for the cooling coil but has no heating-side equivalent.

3. **Cooling latent degradation** — `coil_physics.rs:96-194`: `effective_shr_with_latent_degradation` handles part-load cycling effects on latent removal for cooling. No analogous model exists for heating.

4. **HARES heating SHR field** — `HvacEquipment.shr` (line 157) exists but is set to `1.0` at construction (`hvac_core.rs:275`) and is only used for cooling-side calculations.

## Required Behavior

Heating latent output should correctly reflect the three-regime physics described above:

1. **During normal heating** — `latent_gain_w = 0.0`. The outdoor coil condensate stays outdoors. This is physically correct and matches the behavior of EnergyPlus `Coil:Heating:DX:SingleSpeed` with no defrost active.

2. **During reverse-cycle defrost** — residual moisture on the indoor coil surface can evaporate into supply air as the coil transitions between condenser and evaporator roles. The latent effect is:
   ```
   latent_gain_w = q_defrost_w * (1 - SHR_defrost) * time_fraction
   ```
   where `SHR_defrost` is the fraction of defrost power delivered as sensible heat. The sign is positive (small moisture addition to zone), not negative. The melted outdoor coil frost does NOT enter the indoor stream.

3. **After defrost recovery (transient)** — residual moisture on the indoor coil evaporates into supply air, producing a small positive latent gain that decays exponentially:
   ```
   latent_gain_w = q_heating * (1 - SHR_heating_rated) * exp(-recovery_elapsed_s / tau)
   ```
   where `tau ≈ 120s`.

4. **Default `SHR_defrost`** — the default must be traceable to a primary source. EnergyPlus example files for `Coil:Heating:DX:SingleSpeed` use the `Defrost Energy Input Ratio Function of Temperature Curve` but do not specify a separate `SHR_defrost` field; the latent split during defrost is implicit in the reverse-cycle power. If NREL ResStock example files (e.g., `options_lookup.tsv` ASHP entries) provide a heating SHR during defrost, that value should be used. If no traceable value is found, state explicitly: "no primary source found; use 1.0 (all defrost power is sensible) as a conservative default, pending data." Do not invent a value.

When defrost is not active and not in recovery:
```
latent_gain_w = 0.0
```

## Approach

1. **Add `heating_shr` config field** to `HeatPumpHeaterConfig` (`heat_pump_config.rs:20`) with default `1.0` (matching OCHRE behavior). The field is for future use; default preserves current physics.

2. **Add `heating_shr` field to `HeatPumpHeaterCore`** — store the resolved value during `init_from_typed`.

3. **Compute `latent_gain_w` in `compute_step`** (`heater.rs:839-1154`):
   - When `defrost_active` and `heating_shr < 1.0` (reverse-cycle defrost):
     ```rust
     // Negative latent = dehumidification during reverse-cycle defrost
     let latent_gain_w = -step.defrost_q_w * (1.0 - self.cooling_shr) * step.defrost_time_fraction;
     ```
   - During defrost recovery (if discrete defrost model is active):
     ```rust
     let latent_gain_w = step.thermal_output_w * (1.0 - self.heating_shr) * (-recovery_elapsed_s / 120.0).exp();
     ```
   - When not in defrost: `latent_gain_w = 0.0`

4. **Pass `latent_gain_w` to `write_zone_thermal_contributions`** in `heater.rs:696-701` instead of the hardcoded `0.0`.

5. **Add `latent_gain_w` to `HeaterStep`** struct so it flows through the step result.

6. **Add telemetry field** `HEATING_LATENT_W` for visibility.

7. **Add `tracing::debug!` when `latent_gain_w > 0.0`** during heating: `"heating latent gain: {latent_gain_w:.1}W during defrost recovery"`.

8. **Add test** verifying that cold-humid conditions with defrost produce negative latent gain during reverse-cycle defrost (dehumidification).

9. **Add HPXML wiring** — `heating_shr` has no standard HPXML source. Default to 1.0 matching OCHRE.

## Definition of Done

- [ ] `heating_shr` field added to `HeatPumpHeaterConfig` with default 1.0 (matching OCHRE)
- [ ] `latent_gain_w` is negative (dehumidification) when reverse-cycle defrost is active and `heating_shr < 1.0`
- [ ] `latent_gain_w` is zero during normal heating (no defrost)
- [ ] `write_zone_thermal_contributions` receives computed `latent_gain_w` instead of `0.0`
- [ ] Heating-only (no defrost) still produces `latent_gain_w = 0.0`
- [ ] Telemetry reports heating latent output
- [ ] Default `heating_shr = 1.0`; field is for future use
- [ ] `tracing::debug!` fires when `latent_gain_w > 0.0` during heating

## Verification

1. **Unit test**: Construct ASHP heater with `heating_shr = 0.95`. Run at OAT = -5°C with humidity. Verify `latent_gain_w < 0.0` during reverse-cycle defrost steps (dehumidification).
2. **Unit test**: Same heater at OAT = 10°C (no defrost). Verify `latent_gain_w = 0.0`.
3. **Unit test**: Heater with `heating_shr = 1.0` (default). Verify `latent_gain_w = 0.0` even during defrost.
4. **Conservation test**: `sensible_gain_w + latent_gain_w` must equal total thermal output (energy balance).
5. **Sign test**: During reverse-cycle defrost, `latent_gain_w` must be negative (dehumidification), not positive.

## References

- EnergyPlus Engineering Reference §16.5 "DX Heating Coil Model": defrost moisture treatment. Specifies that melted/evaporated frost from the outdoor coil does not enter the indoor supply stream; latent effects during defrost arise from indoor coil surface conditions.
- EnergyPlus Engineering Reference, `Coil:Heating:DX:SingleSpeed` — Heating Bypass Factor method for sensible/latent split during defrost recovery
- ASHRAE Handbook of Fundamentals 2021 Ch.23: Heating and Cooling Coils
- HARES `coil_physics.rs:214-286`: cooling-side SHR solver (reference for psychrometric treatment pattern)

## Dependencies

- **Ticket 010 must be applied before verifying this ticket.** Defrost activation in the model is driven by `cap_ratio` falling below a threshold at low OAT. With identity curves, `cap_ratio = 1.0` at all temperatures and defrost never activates in cold weather. Verification tests for this ticket that exercise defrost latent paths are meaningless unless run with real biquadratic curves from ticket 010 in place.

## Related Tickets

- [011-discrete-defrost-cycle.md](011-discrete-defrost-cycle.md) — discrete defrost interacts with latent moisture
- [010-default-biquadratic-performance-curves.md](010-default-biquadratic-performance-curves.md) — see Dependencies above

---

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-20

### Code Confirmation

- [x] **Line numbers match**: `heater.rs:695–701` — the `write_zone_thermal_contributions` call with hardcoded `0.0` for `latent_gain_w` is confirmed at line 699. The `compute_step` function spans lines 839–1154 exactly as stated.
- [x] **Described logic matches current implementation**: `HvacEquipment.shr` is defined at `hvac_core.rs:157` and initialised to `1.0` at `hvac_core.rs:275`. The field is only updated by cooling-side code (`air_conditioner.rs:769`, `air_conditioner.rs:1188`). The heater never reads or updates `shr` for the latent split. `coil_physics.rs:214–286` (`calculate_shr`) and `coil_physics.rs:96–194` (`effective_shr_with_latent_degradation`) are both cooling-only. `ideal_hvac.rs:552–554` explicitly comments "Heating: all sensible, no latent." The bug is present and unambiguous.
- [x] **`HeaterStep` struct** (line 220) already carries `defrost_active` (line 231), `defrost_time_fraction` (line 232), and `defrost_q_w` (line 234), so the data needed for a latent computation is already in the step result.
- [x] **OCHRE cross-check**: MATCHES — `vendors/OCHRE/ochre/Equipment/HVAC.py:458–462`:
  ```python
  def update_shr(self):
      self.coil_input_db = self.zone.temperature
      if self.is_heater:
          return 1   # SHR=1.0 for all heating equipment, all operating modes
  ```
  OCHRE also models defrost only as a capacity/EIR penalty (lines 1139–1165) with no latent moisture effect. HARES and OCHRE are aligned on the current (zero-latent-heating) behaviour; the ticket proposes adding latent treatment that OCHRE does not have.
- [x] **EnergyPlus cross-check**: PARTIALLY MATCHES — The EnergyPlus DX heating coil `CalcDXHeatingCoil` function is documented to produce **no latent output** (issue #7440 states "the heating coil has no latent/sensible energy, no latent/sensible energy rate"). The bigladdersoftware.com Engineering Reference confirms a "Defrost Operation" subsection exists under "Single-Speed Electric Heat Pump DX Air Heating Coil," and that defrost is modelled via capacity and EIR adjustment factors derived from DOE-2.1E algorithms — not via a supply-air SHR split. No "Heating Bypass Factor" method for defrost is described in any version of the EnergyPlus Engineering Reference that was accessible (8.0, 8.1, 8.4, 8.9, 23.1 checked). The claim in the ticket references section "§16.5" — this section number could not be confirmed (the Engineering Reference uses heading-based rather than numeric section references in recent versions, and the HTML pages were too large for the DX heating section to be fetched in full).

### Web-Verified Citations

**Citation 1**
- **Citation**: "EnergyPlus Engineering Reference §16.5 'DX Heating Coil Model': defrost moisture treatment. Specifies that melted/evaporated frost from the outdoor coil does not enter the indoor supply stream; latent effects during defrost arise from indoor coil surface conditions."
- **Source found**: DesignBuilder v7.2 DX Heating Coil help page (mirrors EnergyPlus Engineering Reference); BigLadder Software EnergyPlus Engineering Reference v8.0–23.1 (Coils chapter, multiple versions); EnergyPlus GitHub issue #7440 ("Heating Coil Calculation uses Cooling-Coil properties")
- **Quoted passage**: From the DesignBuilder/EnergyPlus defrost description: *"If the reverse-cycle strategy is selected, the heating cycle is reversed periodically to provide heat to melt frost accumulated on the outdoor coil."* From GitHub issue #7440: *"the heating coil has no latent/sensible energy, no latent/sensible energy rate."* The BigLadder pages confirm a "Defrost Operation" subsection exists and references DOE-2.1E empirical models, but the section body was not fully accessible.
- **Verdict**: **Partially correct** — The claim that EnergyPlus does not carry frost from the outdoor coil through to the indoor supply stream is consistent with EnergyPlus's treatment (the model uses capacity/EIR adjustment factors, not a moisture pathway). However, the specific "§16.5" section number is **unverified** — no accessible version of the Engineering Reference uses that numeric designation for the DX heating coil section. The claim that EnergyPlus specifies "latent effects during defrost arise from indoor coil surface conditions" is **not confirmed** by any source found; EnergyPlus's actual approach appears to treat defrost as an entirely sensible capacity/energy correction with no latent split at all.

**Citation 2**
- **Citation**: "EnergyPlus Engineering Reference, `Coil:Heating:DX:SingleSpeed` — Heating Bypass Factor method for sensible/latent split during defrost recovery"
- **Source found**: BigLadder Software EnergyPlus Engineering Reference (all versions searched); EnergyPlus GitHub issue #7440
- **Quoted passage**: Issue #7440: *"the heating coil has no latent/sensible energy, no latent/sensible energy rate"*. The Engineering Reference table of contents confirms a "Defrost Operation" subsection exists but its body content (including any bypass-factor discussion) was not reachable via WebFetch due to page size.
- **Verdict**: **Incorrect** — No "Heating Bypass Factor method for sensible/latent split during defrost recovery" is described in any accessible version of the EnergyPlus Engineering Reference. EnergyPlus models DX heating as **all-sensible** (no latent output), with defrost handled via capacity and EIR multipliers. The bypass factor (ADP/BF) approach is explicitly a *cooling* coil technique. The ticket invents a heating analogue that does not exist in EnergyPlus.

**Citation 3**
- **Citation**: "ASHRAE Handbook of Fundamentals 2021 Ch.23: Heating and Cooling Coils"
- **Source found**: ASHRAE.org — Table of Contents 2021 ASHRAE Handbook—Fundamentals (fetched directly); ashraepyramids.org — Table of Contents 2020 ASHRAE Handbook HVAC Systems and Equipment
- **Quoted passage**: From ASHRAE.org 2021 Fundamentals TOC: Chapter 23 is *"Insulation for Mechanical Systems"*. From ashraepyramids.org 2020 S&E TOC: Chapter 23 is *"Air-Cooling and Dehumidifying Coils"*; Chapter 27 is *"Air-Heating Coils"*. ASHRAE.org description of Ch.23 (2020 S&E): *"The chapter discusses sensible heat ratio (SHR) only in the context of cooling/dehumidifying coils."*
- **Verdict**: **Incorrect** — The 2021 *Fundamentals* handbook Chapter 23 is about mechanical insulation, not coils. The relevant coil content is in the 2020 *HVAC Systems and Equipment* handbook: Chapter 23 is Air-Cooling and Dehumidifying Coils (cooling only, not directly relevant to heating SHR) and Chapter 27 is Air-Heating Coils. Neither chapter addresses DX heat pump heating SHR. The ticket's citation is wrong on both the volume name and the chapter number.

**Citation 4**
- **Citation**: "HARES `coil_physics.rs:214-286`: cooling-side SHR solver (reference for psychrometric treatment pattern)"
- **Source found**: `/Users/rich/source/HARES/crates/hares-equipment/src/hvac/coil_physics.rs` (read directly)
- **Quoted passage**: Lines 214–286: `pub(super) fn calculate_shr(db_in_c, w_in, p_kpa, q_kw, flow_m3_s, ao) -> crate::Result<CoilResult>` — full ADP/BF iteration, returns `CoilResult { shr, adp_temp_c, bypass_factor, supply_temp_c }`.
- **Verdict**: **Confirmed** — the function exists at the stated lines and implements exactly what the ticket describes.

### Legitimacy

- **Verdict**: **Partially Legitimate**
- **Rationale**: The core bug is real and clearly present in the code: `heater.rs:699` hardcodes `latent_gain_w = 0.0` for all heating output, and no heating-side SHR field or latent computation exists in `HeatPumpHeaterConfig` or `HeatPumpHeaterCore`. The OCHRE cross-check confirms HARES matches OCHRE on this point (both use SHR=1.0 for heating). The proposed fix of adding a `heating_shr` config field is reasonable. However, several specific claims are inaccurate:
  1. The cited EnergyPlus "§16.5" section number is unverifiable and the claimed "Heating Bypass Factor method for sensible/latent split during defrost recovery" does not appear to exist in EnergyPlus — EnergyPlus models DX heating as all-sensible (confirmed by issue #7440).
  2. The ASHRAE citation is wrong on both the volume (Fundamentals vs. Systems and Equipment) and chapter number (23 in Fundamentals is insulation; coils are in S&E Ch.23/27).
  3. The physics description in the ticket is self-consistent and plausible, but the claim that "latent effects during defrost arise from indoor coil surface conditions" is the ticket author's physical reasoning, not confirmed EnergyPlus documentation. EnergyPlus simply produces no latent output from heating coils in any mode.
  4. The ticket's "Approach" section item 3 contains a sign error: it uses `self.cooling_shr` where it should use `self.heating_shr`.
  The dependency note (Ticket 010 required for defrost to activate with real curves) is correct and important.

### Proposed Fix Summary

Minimum fix to address the stated issue:
1. Add optional `heating_shr: Option<f64>` field to `HeatPumpHeaterConfig` (default `None`; treated as `1.0`). No new HPXML mapping needed — default matches OCHRE.
2. Store resolved `heating_shr: f64` in `HeatPumpHeaterCore` during `init_from_typed`.
3. In `heater.rs` `step()` (lines 695–701), replace hardcoded `0.0` with:
   - `0.0` when not in defrost (correct physics; no change from current)
   - `step.defrost_q_w * defrost_time_fraction * (1.0 - self.heating_shr)` when `defrost_active && self.heating_shr < 1.0` (small positive zone latent gain)
4. Add telemetry key `HEATING_LATENT_W`.
Do NOT implement a "Heating Bypass Factor" analogous to the cooling ADP/BF solver — there is no EnergyPlus precedent for this and it would over-engineer the fix. The sign convention in the ticket's Approach §3 must be corrected: use `self.heating_shr` (not `self.cooling_shr`).

### Test Written

- **File**: `crates/hares-equipment/tests/hvac_tests.rs`
- **Test 1** (`ticket_012_heating_latent_always_zero_during_normal_heating`): Runs ASHP at 7°C OAT (no defrost); asserts `latent_gain_w == 0.0`. This assertion is **physically correct** and should remain passing after the fix.
- **Test 2** (`ticket_012_heating_latent_always_zero_during_defrost`): Runs ASHP at -5°C OAT (defrost active); asserts `latent_gain_w == 0.0` and documents this as the current bug. After the fix, this assertion should **fail** and be updated to `assert!(latent_w >= 0.0)` (with a non-zero check when `heating_shr < 1.0`).
- Both tests compile and pass under the current (buggy) implementation: `cargo test --package hares-equipment --test hvac_tests ticket_012` → 2 passed.
