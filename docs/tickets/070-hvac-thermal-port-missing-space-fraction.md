# HVAC Thermal Port Missing space_fraction Scaling (All Affected Equipment)

**Severity**: High
**Priority**: P1
**Status**: Open
**Areas**: hares-equipment
**Consolidates**: 075-ac-sensible-cooling-not-space-fraction-scaled.md (moved to consolidated/)

## Problem

Five HVAC equipment files apply `space_fraction` to electrical/fuel ports but deliver
the full unscaled gross thermal capacity to the zone thermal port. When
`space_fraction < 1.0` (multi-equipment systems where one unit serves a fraction
of load), the zone receives more sensible heat (or more latent extraction for
cooling) than the equipment actually delivers, breaking the energy balance between
electrical consumption and zone heat delivery.

OCHRE `HVAC.py` line 558 scales `delivered_heat *= self.space_fraction` and line 559
scales `electric_kw *= self.space_fraction` in the same statement block, maintaining
energy balance. HARES omits `space_fraction` from the thermal path in all equipment
types enumerated below.

## Affected Equipment and Current Behavior

### Electric furnace (`furnace.rs:158–183`)

`furnace.rs:159`: `let sf = self.hvac.space_fraction;`
`furnace.rs:161`: `let fan_kw = (self.fan_power_w * duty) / 1_000.0 * sf;` — sf applied to fan
`furnace.rs:160`: `let gross_capacity_w = self.rated_capacity_w * duty;` — no sf
`furnace.rs:174`: `let total_sensible_w = gross_capacity_w + fan_heat_w;` — sf-scaled fan + unscaled capacity

`gross_capacity_w` is unscaled; `fan_heat_w` is sf-scaled (via `fan_kw`). `total_sensible_w`
mixes both and is written to the thermal port at `furnace.rs:177–183` without applying `sf`
to the gross term.

### Gas furnace (`furnace.rs:371–408`)

Same pattern: `gross_capacity_w = self.rated_capacity_w * duty` at `furnace.rs:375` (no sf).
`fan_heat_w = fan_kw * 1000.0` at `furnace.rs:398` (sf-scaled via line 376).
`total_sensible_w = gross_capacity_w + fan_heat_w` at `furnace.rs:399` — mixed.
Written to thermal port at `furnace.rs:402–407` without correcting the gross term.

### Baseboard (`baseboard.rs:133–147`)

`baseboard.rs:133`: `let thermal_output_w = self.rated_capacity_w * duty;` — no sf
`baseboard.rs:134`: `let electric_kw = thermal_output_w * self.eir / 1_000.0 * self.hvac.space_fraction;` — sf on electrical
`baseboard.rs:141–146`: `write_zone_thermal_contributions(ports, thermal_output_w, ...)` — unscaled thermal written

### Air conditioner (`air_conditioner.rs:791–838`)

`air_conditioner.rs:794`: `sensible_cooling_w *= thermal_ratio * effective_load;` — no sf
`air_conditioner.rs:795`: `latent_cooling_w *= thermal_ratio * effective_load;` — no sf
`air_conditioner.rs:800–806`: thermal port write at full scale for both sensible and latent
`air_conditioner.rs:832–833`: `let electric_kw = (compressor_kw + fan_kw + self.crankcase_heater_kw) * self.hvac.space_fraction;` — sf on electrical only

The unscaled `latent_cooling_w` written to the thermal port also reaches the humidity
solver. With `space_fraction = 0.5` the solver removes twice the intended moisture,
driving zone humidity artificially low.

### Heat pump heater (`heat_pump/heater.rs:695–718`)

`heater.rs:695–701`: `write_zone_thermal_contributions(ports, step.thermal_output_w, ...)` — no sf
`heater.rs:703`: `let scaled_electric_kw = step.electric_kw * self.hvac.space_fraction;` — sf on electrical only

`step.thermal_output_w` includes `hp_capacity_w + er_capacity_w + fan_power_w`
(computed at line 1065); none of these are scaled by `space_fraction` before the
thermal port write.

### Boiler (electric and gas) — CORRECT, no change needed

`boiler.rs:212`: `let thermal_output_w = self.rated_capacity_w * duty * sf;` — sf applied before thermal port.
`boiler.rs:458`: same for gas boiler.
Existing tests `electric_boiler_space_fraction_halves_thermal_and_electrical_output` and
`gas_boiler_space_fraction_halves_thermal_and_fuel_output` at `boiler.rs:1163–1268` pass.

## Required Behavior

All affected equipment must apply `space_fraction` to the gross thermal output
(both sensible and latent) before writing to `PortContribution::Thermal`, consistent
with OCHRE `HVAC.py` line 558. The pattern is:

```
total_sensible_w = (gross_capacity_w + fan_heat_w) * space_fraction
```

where `gross_capacity_w` and `fan_heat_w` derive from unscaled rated values so that
`space_fraction` is applied once to the sum. Electrical and fuel ports must not
receive a second application — they are already correct in the non-furnace equipment.

For the gas and electric furnace, the sf-double-application via `fan_kw` must be
corrected: compute `fan_heat_w` from the unscaled `fan_power_w * duty`, not from
the already-sf-scaled `fan_kw`, then apply `sf` to the sum.

Reference: EnergyPlus Engineering Reference §"Zone Air Heat Balance" — equipment
contributions to the zone air node are the actually-delivered fraction of output,
not gross rated values. ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.2 —
loads on zone air are the delivered fraction only.

## Approach

1. **Electric furnace** (`furnace.rs:155–183`): compute
   `let fan_heat_w = self.fan_power_w * duty;` (unscaled), then
   `let total_sensible_w = (gross_capacity_w + fan_heat_w) * sf;`.
   Remove the existing `fan_kw`-derived `fan_heat_w`. Update the electrical port
   derivation if needed so `electric_kw` remains unaffected.

2. **Gas furnace** (`furnace.rs:371–408`): same correction — derive `fan_heat_w`
   from `self.fan_power_w * duty` (unscaled), apply `sf` to the sum.

3. **Baseboard** (`baseboard.rs:126–148`): multiply `thermal_output_w` by
   `self.hvac.space_fraction` before the `write_zone_thermal_contributions` call.

4. **Air conditioner** (`air_conditioner.rs:786–838`): after the `effective_load`
   and `thermal_ratio` multipliers, multiply `sensible_cooling_w`, `latent_cooling_w`,
   `compressor_kw`, and `fan_kw` by `self.hvac.space_fraction`. Remove the separate
   `* self.hvac.space_fraction` from the electrical port derivation to avoid
   double application.

5. **Heat pump heater** (`heat_pump/heater.rs:693–718`): pass
   `step.thermal_output_w * self.hvac.space_fraction` to
   `write_zone_thermal_contributions`. The `scaled_electric_kw` line is already correct.

## Definition of Done

- [ ] Electric furnace thermal port scaled by `space_fraction` (gross capacity only; no double-application via fan term)
- [ ] Gas furnace thermal port scaled by `space_fraction` with same correction
- [ ] Baseboard thermal port scaled by `space_fraction`
- [ ] Air conditioner sensible and latent thermal ports scaled by `space_fraction`; electrical port not double-scaled
- [ ] Heat pump heater thermal port scaled by `space_fraction`
- [ ] Electrical and fuel port writes in all five files unchanged in net effect
- [ ] Tests for all five equipment types: `space_fraction=0.5` thermal port is exactly half of `space_fraction=1.0`; `space_fraction=0.5` AC latent port is exactly half
- [ ] No existing boiler tests broken

## Verification

```bash
cargo test -p hares-equipment
```

## References

- OCHRE `HVAC.py` lines 558–561: `delivered_heat *= self.space_fraction`, `electric_kw *= self.space_fraction`, `fan_power *= self.space_fraction`, `gas_therms_per_hour *= self.space_fraction`
- EnergyPlus Engineering Reference §"Zone Air Heat Balance": equipment contributions to zone air node are the actual delivered fraction, not gross rated values
- ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.2 "Heat Balance Method": loads on zone air are the delivered fraction only
