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
