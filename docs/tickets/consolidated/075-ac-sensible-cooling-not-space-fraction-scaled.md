# AC Sensible and Latent Cooling Thermal Port Missing space_fraction

**Severity**: High
**Priority**: P1
**Status**: Open
**Areas**: hares-equipment
**Superseded by**: Ticket 070 covers this; this ticket adds specifics for the AC
latent cooling path which ticket 070 does not detail.

## Problem

`AirConditioner` applies `space_fraction` to the electrical port but not to the
thermal port for either sensible or latent cooling. When `space_fraction < 1.0`,
the zone receives the full cooling load but only a fraction of the electrical draw
is billed — producing an impossible COP and breaking the zone energy balance.

A related consequence: the `latent_cooling_w` written to the thermal port is also
not scaled by `space_fraction`, so the humidity solver receives the full latent
extraction rate even though only a fraction of the cooling capacity is being served.
This overcorrects the zone humidity ratio for multi-equipment configurations.

## Current Behavior

`air_conditioner.rs:791-806`:

```rust
sensible_cooling_w *= thermal_ratio * effective_load;
latent_cooling_w *= thermal_ratio * effective_load;
// No space_fraction applied to either ^^

let fan_heat_w = fan_kw * 1000.0;   // fan_kw is also not sf-scaled here
self.hvac.write_zone_thermal_contributions(
    ports,
    -sensible_cooling_w + fan_heat_w,
    -latent_cooling_w,
    ThermalCategory::HvacCooling,
)?;
```

`air_conditioner.rs:832-838`:

```rust
let electric_kw =
    (compressor_kw + fan_kw + self.crankcase_heater_kw) * self.hvac.space_fraction;
// space_fraction applied here — electrical port only
```

The humidity solver (`humidity_solver.rs:116-122`) converts `latent_gain_w`
(negative for cooling) to a humidity ratio decrement. An unscaled `latent_cooling_w`
with `space_fraction = 0.5` removes twice as much moisture from the zone as the
equipment actually delivers, leading to an overcooled humidity ratio.

## OCHRE Cross-check

OCHRE `HVAC.py` line 558: `self.delivered_heat *= self.space_fraction`. The
variable `delivered_heat` includes both sensible and latent components (the sensible
is `heat_gain * shr + fan_power` and the latent is tracked via `latent_gain`).
Line 595: `self.latent_gain * self.space_fraction` is applied when writing results.

## EnergyPlus Reference

EnergyPlus Engineering Reference, Zone Air Heat Balance: the fraction of load
served (`fraction_of_autosized_cooling_capacity`) scales all delivered outputs —
sensible, latent, and electrical — proportionally. No partial scaling of one output
without the others.

## Required Behavior

In `air_conditioner.rs`, after control multipliers are applied and before the
thermal port write:

```rust
let sf = self.hvac.space_fraction;
sensible_cooling_w *= sf;
latent_cooling_w *= sf;
fan_kw *= sf;     // also scaled so fan_heat_w is sf-consistent
```

The electrical port line should derive from the already-sf-scaled values so there
is no double application.

This ticket is closely related to ticket 070 which covers the same defect in
furnaces and the HP heater. The AC-specific detail is the latent path, which
affects the humidity solver's moisture balance.

## Impact

Annual kWh impact rank: **High** (same as ticket 070). Multi-equipment systems
with `space_fraction < 1.0` will have erroneous cooling-side moisture removal.
The humidity error compounds over the simulation day as each timestep extracts
too much moisture, driving the zone humidity artificially low, reducing apparent
latent load, and allowing the cooling coil to deliver more sensible capacity than
intended.

## Approach

1. After the `effective_load` and `thermal_ratio` multipliers are applied, multiply
   `sensible_cooling_w`, `latent_cooling_w`, `compressor_kw`, and `fan_kw` by
   `self.hvac.space_fraction`.
2. Remove the separate `* self.hvac.space_fraction` from the electrical port line
   (it will already be incorporated).
3. Add a test: `space_fraction=0.5` AC step produces exactly half the zone sensible
   cooling, half the latent cooling, and half the electrical draw of `space_fraction=1.0`.

## Definition of Done

- [ ] `sensible_cooling_w` scaled by `space_fraction` before thermal port write
- [ ] `latent_cooling_w` scaled by `space_fraction` before thermal port write
- [ ] `fan_kw` scaled by `space_fraction` before thermal port write
- [ ] Electrical port does not double-apply `space_fraction`
- [ ] Test: `space_fraction=0.5` → thermal port is exactly half of full-scale
- [ ] Humidity solver receives correct (sf-scaled) latent for moisture balance

## References

- OCHRE `HVAC.py` lines 558, 595: `delivered_heat *= space_fraction`,
  `latent_gain *= space_fraction`
- EnergyPlus Engineering Reference, Zone Air Heat Balance §3.1.2: partial-load
  operation scales all outputs proportionally
- `air_conditioner.rs:791-838`: current thermal vs electrical port scaling
- Ticket 070: same class of defect in furnaces and HP heater
