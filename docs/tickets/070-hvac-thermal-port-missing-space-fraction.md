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

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match (verified):
  - Electric furnace: `sf` at line 159, `gross_capacity_w` unscaled at line 160, `fan_kw` sf-scaled at line 161, `total_sensible_w` mixed at line 174, `write_zone_thermal_contributions` at line 177 — matches ticket exactly
  - Gas furnace: `sf` at line 372, `gross_capacity_w` unscaled at line 375, `fan_kw` sf-scaled at line 376, `total_sensible_w` at line 399, `write_zone_thermal_contributions` at line 402 — matches ticket exactly
  - Baseboard: `thermal_output_w` unscaled at line 133, `electric_kw` sf-scaled at line 134, `write_zone_thermal_contributions` at line 141 — matches ticket exactly
  - Air conditioner: `sensible_cooling_w` unscaled at line 794, `latent_cooling_w` unscaled at line 795, `write_zone_thermal_contributions` at line 801, `electric_kw * space_fraction` at line 832 — matches ticket exactly
  - Heat pump heater: unscaled `step.thermal_output_w` at line 698, `scaled_electric_kw` at line 703 — matches ticket exactly
  - Boiler: `thermal_output_w = self.rated_capacity_w * duty * sf` at line 212 (electric), line 458 (gas) — confirmed correct as ticket states

- [x] Described logic matches current implementation: the asymmetry is real — thermal ports are unscaled while electrical/fuel ports ARE scaled by `space_fraction` in furnace, baseboard, AC, and heat pump heater. Boiler is correctly scaled.

- [x] **OCHRE cross-check result: DIVERGES from ticket's interpretation — with critical evidence that HARES's current behavior matches OCHRE's zone-model architecture**

  OCHRE `HVAC.py` lines 543–565 (read verbatim):
  ```python
  self.delivered_heat = heat_gain * self.shr + self.fan_power  # line 543
  self.sensible_gain = self.delivered_heat                      # line 544
  self.latent_gain = heat_gain * (1 - self.shr)                # line 545

  # reduce delivered heat (only for results) and power output based on space fraction
  # Note: sensible/latent gains to envelope are not updated            ← line 556 comment
  self.delivered_heat *= self.space_fraction                    # line 558
  self.electric_kw *= self.space_fraction                       # line 559
  self.fan_power *= self.space_fraction                         # line 560
  self.gas_therms_per_hour *= self.space_fraction               # line 561

  def add_gains_to_zone(self):
      for zone, fraction in self.zone_fractions.items():
          zone.hvac_sens_gain += self.sensible_gain * fraction  # line 565 — UNSCALED
          zone.hvac_latent_gain += self.latent_gain * fraction  # line 566 — UNSCALED
  ```

  **OCHRE's explicit comment at line 556 states:** "reduce delivered heat (only for results) and power output based on space fraction — Note: sensible/latent gains to envelope are not updated." OCHRE's `add_gains_to_zone` (line 565) uses the **unscaled** `sensible_gain` field. `space_fraction` in OCHRE scales only power-reporting metrics (`delivered_heat`, `electric_kw`, `fan_power`, `gas_therms_per_hour`) and result outputs; it explicitly does **not** scale the thermal contribution to the zone air node.

  OCHRE's `zone_fractions` (line 189) is `{zone: duct_dse * (1 - basement_heat_frac)}` — it represents duct distribution losses, NOT `space_fraction`. The `space_fraction` in OCHRE (sourced from HPXML `FractionHeatLoadServed`, `ochre/utils/hpxml.py` line 848) represents what fraction of the building's total load this unit serves, and OCHRE treats it as a power scaling for accounting purposes only.

  **HARES's current behavior — not scaling the thermal zone port by `space_fraction` — precisely matches OCHRE's zone-model architecture.** The ticket's OCHRE citation is misleading: it quotes lines 558–561 (which scale power metrics) but omits the explicit comment at line 556 and the `add_gains_to_zone` implementation at line 565 that shows the zone thermal contribution is *unscaled*.

- [x] **EnergyPlus cross-check result: Does NOT prescribe space_fraction scaling for the classical zone air heat balance**

  Source fetched: https://bigladdersoftware.com/epx/docs/9-6/engineering-reference/basis-for-the-zone-and-air-system-integration.html
  (and https://bigladdersoftware.com/epx/docs/25-2/engineering-reference/basis-for-the-zone-and-air-system-integration.html)

  Quoted passage — zone air heat balance equation (verbatim):
  > "Cz dTz/dt = Σ Q̇i + Σ hi Ai (Tsi − Tz) + Σ ṁi Cp (Tzi − Tz) + ṁinf Cp (T∞ − Tz) + Q̇sys"
  > "Q̇sys = ṁsys Cp (Tsup − Tz)"

  The Q̇sys term represents the actual delivered air system output — the supply air enthalpy relative to zone air conditions. No "space fraction" term appears in this formulation. The concept of space fraction as a zone-thermal scaling multiplier does not appear in the classical EnergyPlus zone air heat balance. It only appears in EnergyPlus v23+ under the **SpaceHVAC** feature (`SpaceHVAC:ZoneEquipmentSplitter`), which distributes multi-zone equipment outputs to sub-zone air nodes by multiplying the space fraction.

  The ticket's citation of "EnergyPlus Engineering Reference §'Zone Air Heat Balance'" is partially accurate (the section concerns delivered output, not rated values), but the actual section title is **"Basis for the Zone and Air System Integration"** (not "Zone Air Heat Balance"), and the EnergyPlus formulation does not support the inference that HVAC equipment should scale thermal zone contributions by a `space_fraction` multiplier outside of the SpaceHVAC context.

### Web-Verified Citations

**Citation 1**: OCHRE `HVAC.py` lines 558–561 scale `delivered_heat *= self.space_fraction`
- **Source found**: `/Users/rich/source/HARES/vendors/OCHRE/ochre/Equipment/HVAC.py` (read directly)
- **Quoted passage**: Lines 556–566 (verbatim above)
- **Verdict**: **Incorrect as cited** — the ticket correctly quotes lines 558–561 but critically omits the comment at line 556 ("Note: sensible/latent gains to envelope are not updated") and the `add_gains_to_zone` method at line 565 which shows zone thermal contributions use the *unscaled* `sensible_gain`. The ticket inverts OCHRE's intent.

**Citation 2**: EnergyPlus Engineering Reference §"Zone Air Heat Balance" — equipment contributions are the actually-delivered fraction
- **Source found**: https://bigladdersoftware.com/epx/docs/9-6/engineering-reference/basis-for-the-zone-and-air-system-integration.html
- **Quoted passage**: "Q̇sys = ṁsys Cp (Tsup − Tz)" — the system output is computed from actual supply air conditions; "Cz dTz/dt = Σ Q̇i + ... + Q̇sys"
- **Verdict**: **Partially correct — but does not support the ticket's specific inference.** The section confirms that Q̇sys is delivered output, not rated capacity. However, the section title is "Basis for the Zone and Air System Integration," not "Zone Air Heat Balance." More importantly, this section contains no concept of a `space_fraction` multiplier applied to equipment thermal contributions. The EnergyPlus SpaceHVAC feature (v23+) does distribute HVAC output by space fractions, but only to sub-zone nodes within a multi-space zone model — not as a scaling multiplier on single-zone equipment output.

**Citation 3**: ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.2 "Heat Balance Method" — loads on zone air are the delivered fraction only
- **Source found**: ASHRAE Handbook is paywalled; table of contents confirmed at https://www.ashrae.org/technical-resources/ashrae-handbook/table-of-contents-2021-ashrae-handbook-fundamentals
- **Quoted passage**: Cannot be obtained — document requires purchase. Chapter 18 is confirmed as "Nonresidential Cooling and Heating Load Calculations." §18.2 section number and exact wording could not be independently verified.
- **Verdict**: **Cannot Verify** — the concept is consistent with standard heat balance theory, but the specific section number (§18.2) and the phrase "delivered fraction only" could not be confirmed from any publicly accessible source. The ASHRAE HoF requires institutional access.

### Legitimacy
- **Verdict**: **Not Legitimate**

- **Rationale**: The code asymmetry described in the ticket is real — HARES does scale electrical/fuel ports by `space_fraction` while leaving thermal zone ports unscaled. However, **this is the correct behavior, not a bug**. OCHRE's own source code at `HVAC.py` lines 556–566 contains an explicit comment documenting this design decision: `space_fraction` is applied to power-reporting metrics only, and "sensible/latent gains to envelope are not updated." OCHRE's `add_gains_to_zone` uses the unscaled `sensible_gain` field for zone thermal contributions. HARES faithfully mirrors this OCHRE architecture. The ticket's OCHRE citation (lines 558–561) is selective and misleading — it quotes the power-scaling lines while omitting the adjacent comment that explicitly prohibits thermal-port scaling. The EnergyPlus zone air heat balance equation (Q̇sys = ṁsys Cp (Tsup − Tz)) does not introduce a space_fraction multiplier on single-zone equipment thermal output. The ASHRAE citation could not be verified. The boiler's `thermal_output_w *= sf` at line 212 may itself be worth examining — since the boiler delivers heat to a hydronic fluid loop rather than directly to zone air, the semantics differ from the direct-air-delivery equipment this ticket targets.

  The correct mental model: `space_fraction` represents "what fraction of the building's total heating/cooling load this specific unit is sized to serve" (HPXML `FractionHeatLoadServed`). It is an accounting and sizing parameter, not a runtime energy delivery multiplier. OCHRE's design reduces reported power consumption by `space_fraction` to reflect that only a fraction of the house's HVAC energy consumption is attributed to this simulation unit, without changing the actual thermal exchange with the zone air node (which serves the full zone regardless of how many units are installed).

### Proposed Fix Summary

No production fix required — the current behavior is architecturally correct per OCHRE's explicit design. If a deliberate policy decision is made to scale thermal ports (diverging from OCHRE), the minimal change would be:
1. Multiply `total_sensible_w` by `sf` in both furnace variants before passing to `write_zone_thermal_contributions`
2. Multiply `thermal_output_w` by `self.hvac.space_fraction` in baseboard before the port write
3. Multiply `sensible_cooling_w`, `latent_cooling_w` by `self.hvac.space_fraction` in AC before the port write and remove the separate `* space_fraction` on the electrical line
4. Multiply `step.thermal_output_w` by `self.hvac.space_fraction` in heat pump heater before the port write

The boiler's existing `* sf` scaling on the fluid port may also warrant review against OCHRE's pattern (boiler delivers to a hydronic loop, not directly to zone air, so the semantics are different from direct-air equipment).

### Test Written

- File: `crates/hares-equipment/tests/hvac_tests.rs` (appended at end)
- Functions:
  - `ticket_070_electric_furnace_thermal_equals_rated_capacity_at_default_sf` — verifies that at `space_fraction=1.0` (default), electric furnace thermal port equals rated capacity and electrical port equals `capacity * EIR / 1000`. Documents baseline behavior; passes under current implementation.
  - `ticket_070_baseboard_thermal_equals_rated_capacity_at_default_sf` — same verification for electric baseboard. Documents baseline behavior; passes under current implementation.
- **Note**: `ElectricFurnaceConfig` and `ElectricBaseboardConfig` lack a `fraction_heating_load_served` field, so `space_fraction < 1.0` cannot be configured in external integration tests. The tests document the `sf=1.0` baseline. If ticket-070 is accepted as a valid policy change, the config structs must first add the field before space_fraction < 1.0 tests can be written in the integration test suite (or tests can be written as internal unit tests within the equipment modules, where `hvac.space_fraction` can be set directly on the concrete type).
