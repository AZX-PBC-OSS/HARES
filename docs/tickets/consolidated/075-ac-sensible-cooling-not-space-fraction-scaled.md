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

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-22

### Code Confirmation

- [x] Referenced line numbers still match (verified):
  - `air_conditioner.rs:791–795`: `compressor_kw`, `fan_kw`, `sensible_cooling_w`, `latent_cooling_w` all multiplied by `thermal_ratio * effective_load`, **no `space_fraction` applied** — matches ticket exactly
  - `air_conditioner.rs:801–806`: `write_zone_thermal_contributions(ports, -sensible_cooling_w + fan_heat_w, -latent_cooling_w, ...)` — unscaled — matches ticket
  - `air_conditioner.rs:832–833`: `let electric_kw = (compressor_kw + fan_kw + self.crankcase_heater_kw) * self.hvac.space_fraction;` — `space_fraction` on electrical only — matches ticket
  - `humidity_solver.rs:116–122`: `latent_gain_w` accumulated from thermal ports for all zones — matches ticket description

- [x] Described logic matches current implementation: the asymmetry is real — the thermal port write is unscaled while the electrical port IS scaled by `space_fraction`.

- [x] **OCHRE cross-check result: DIVERGES from ticket's interpretation — critical evidence that HARES's current behavior matches OCHRE's zone-model architecture**

  OCHRE `HVAC.py` lines 540–566 (read verbatim from `vendors/OCHRE/ochre/Equipment/HVAC.py`):
  ```python
  self.delivered_heat = heat_gain * self.shr + self.fan_power  # line 543
  self.sensible_gain = self.delivered_heat                      # line 544
  self.latent_gain = heat_gain * (1 - self.shr)                # line 545

  # reduce delivered heat (only for results) and power output based on space fraction
  # Note: sensible/latent gains to envelope are not updated            ← line 556 comment
  self.delivered_heat *= self.space_fraction                    # line 558
  self.electric_kw *= self.space_fraction                       # line 559
  self.fan_power *= self.space_fraction                         # line 560

  def add_gains_to_zone(self):
      for zone, fraction in self.zone_fractions.items():
          zone.hvac_sens_gain += self.sensible_gain * fraction  # line 565 — UNSCALED
          zone.hvac_latent_gain += self.latent_gain * fraction  # line 566 — UNSCALED
  ```

  OCHRE's explicit comment at line 556 states: "Note: sensible/latent gains to envelope are not updated." `add_gains_to_zone()` uses the **unscaled** `sensible_gain` and `latent_gain` fields. Line 595 (`results[...] = self.latent_gain * self.space_fraction`) is a **results-reporting** output, not a zone-thermal contribution — it is written to the output dictionary for logging only, not to the zone air node.

  The ticket's OCHRE citation (line 595) is therefore a **results-reporting line**, not the zone-thermal contribution line. HARES's current behavior — not scaling the thermal zone port by `space_fraction` — precisely matches OCHRE's zone-model architecture.

- [x] **EnergyPlus cross-check result: No §3.1.2 exists; citation is incorrect; no space_fraction in zone air heat balance**

  Source fetched: https://bigladdersoftware.com/epx/docs/25-1/engineering-reference/basis-for-the-zone-and-air-system-integration.html and https://bigladdersoftware.com/epx/docs/9-6/engineering-reference/basis-for-the-zone-and-air-system-integration.html

  Quoted passage — zone air heat balance (verbatim):
  > "Cz dTz/dt = Σ Q̇i + Σ hi Ai (Tsi − Tz) + Σ ṁi Cp (Tzi − Tz) + ṁinf Cp (T∞ − Tz) + Q̇sys"
  > "Q̇sys = ṁsys Cp (Tsup − Tz)"

  The EnergyPlus Engineering Reference uses hierarchical section titles, not numeric section numbers. There is no §3.1.2 in any version of the Engineering Reference (multiple versions confirmed via BigLadder Software). The relevant section is titled "Basis for the Zone and Air System Integration." No `space_fraction` term or "conditioned space fraction" scaling multiplier appears in the zone air heat balance formulation. EnergyPlus v23+ introduced `SpaceHVAC:ZoneEquipmentSplitter` for sub-zone space splitting, but this is an entirely different concept from the single-zone `space_fraction` discussed here.

### Web-Verified Citations

**Citation 1**: OCHRE `HVAC.py` line 558: `delivered_heat *= space_fraction` (and line 595: `latent_gain *= space_fraction`)
- **Source found**: `vendors/OCHRE/ochre/Equipment/HVAC.py` (read directly from local submodule); confirmed against GitHub at https://github.com/NREL/OCHRE/blob/main/ochre/Equipment/HVAC.py (WebFetch confirmed lines 556–566 verbatim)
- **Quoted passage**: Lines 556–566 (verbatim above in OCHRE cross-check section)
- **Verdict**: **Incorrect as cited** — the ticket correctly quotes lines 558/595 but critically misidentifies their role. Line 558 (`delivered_heat *= space_fraction`) modifies a results-only variable; line 595 is a results-reporting line. Neither modifies the actual zone air-node thermal contributions (`sensible_gain`, `latent_gain`). The OCHRE comment at line 556 explicitly prohibits this interpretation: "Note: sensible/latent gains to envelope are not updated." The `add_gains_to_zone()` method at lines 563–566 uses the **unscaled** fields.

**Citation 2**: EnergyPlus Engineering Reference, Zone Air Heat Balance §3.1.2 — partial-load operation scales all outputs proportionally
- **Source found**: https://bigladdersoftware.com/epx/docs/9-6/engineering-reference/basis-for-the-zone-and-air-system-integration.html (WebFetch); https://bigladdersoftware.com/epx/docs/25-1/engineering-reference/basis-for-the-zone-and-air-system-integration.html (WebFetch); EnergyPlus Engineering Reference Table of Contents (WebFetch of https://bigladdersoftware.com/epx/docs/9-0/engineering-reference/)
- **Quoted passage**: "Cz dTz/dt = Σ Q̇i + ... + Q̇sys" and "Q̇sys = ṁsys Cp (Tsup − Tz)"
- **Verdict**: **Incorrect** — (a) Section §3.1.2 does not exist in any EnergyPlus Engineering Reference version; the document uses heading hierarchy, not numeric subsections. (b) The relevant section title is "Basis for the Zone and Air System Integration," not "Zone Air Heat Balance." (c) The zone air heat balance equation contains no `space_fraction` multiplier. (d) EnergyPlus does not scale individual HVAC output components (sensible, latent, electrical) by a `space_fraction` in the classical zone model; the `Q̇sys` term is the actual delivered air-node output computed from supply air conditions.

**Citation 3** (from ticket 070, carried forward to this audit): OCHRE `HVAC.py` line 595 — `latent_gain *= space_fraction` for humidity solver
- **Source found**: `vendors/OCHRE/ochre/Equipment/HVAC.py` line 595 (read directly)
- **Quoted passage**: `results[f"{self.end_use} Latent Gains (W)"] = self.latent_gain * self.space_fraction`
- **Verdict**: **Incorrect as cited** — line 595 is inside `generate_results()`, which builds an output/logging dictionary. It does NOT modify `self.latent_gain` (the field used by `add_gains_to_zone()`). This line scales the *reported* latent for result display only. The humidity solver in OCHRE reads from `zone.hvac_latent_gain` (populated by `add_gains_to_zone()` at line 566 using the **unscaled** `self.latent_gain`).

### Legitimacy

- **Verdict**: **Not Legitimate**

- **Rationale**: The code asymmetry described is real — HARES scales the electrical port by `space_fraction` while leaving the thermal zone port unscaled. However, this is the **correct behavior**, not a bug. OCHRE's source (the reference implementation) contains an explicit comment at HVAC.py line 556 — "Note: sensible/latent gains to envelope are not updated" — and its `add_gains_to_zone()` method (lines 563–566) writes **unscaled** sensible/latent to the zone air node. The three citations in the ticket all misrepresent their sources: (1) the OCHRE "line 558" citation quotes a results-only modification, omitting the explicit adjacent comment; (2) the OCHRE "line 595" citation quotes a logging-only output, not a zone-air-node write; (3) the EnergyPlus §3.1.2 citation references a section that does not exist and describes a scaling mechanism absent from the EnergyPlus zone air heat balance formulation. The correct mental model for `space_fraction` (HPXML `FractionCoolingLoadServed`) is: it is an accounting/sizing parameter representing what fraction of a building's total HVAC load this unit serves, not a runtime energy delivery multiplier that scales zone thermal contributions. HARES correctly applies it to electrical power reporting and leaves zone air node contributions unscaled — matching OCHRE's deliberate design. This ticket is also marked "Superseded by: Ticket 070," whose audit reached the same "Not Legitimate" verdict.

### Proposed Fix Summary

No production fix required — the current behavior is architecturally correct per OCHRE's explicit design decision. If a policy decision is made to diverge from OCHRE (scaling the zone thermal port by `space_fraction`), the minimal change is:
1. After the `thermal_ratio * effective_load` multipliers in `air_conditioner.rs:791–795`, also multiply `sensible_cooling_w`, `latent_cooling_w`, `compressor_kw`, and `fan_kw` by `self.hvac.space_fraction`.
2. Remove the separate `* self.hvac.space_fraction` on the electrical port line (currently line 833) to avoid double-application.

### Test Written

- File: `crates/hares-equipment/tests/hvac_tests.rs` (appended after ticket-070 tests)
- Functions:
  - `ticket_075_ac_thermal_port_equals_gross_at_default_sf` — verifies that at `fraction_load_served=1.0`, the AC sensible thermal port is strongly negative (≥50% of rated capacity), documenting the baseline for this audit.
  - `ticket_075_ac_electrical_halved_thermal_unscaled_at_half_sf` — verifies that at `fraction_load_served=0.5`: (a) the electrical port IS halved (current correct behavior, ratio ≈ 0.5 ± 0.02), and (b) the sensible thermal port is NOT halved (ratio ≈ 1.0 ± 0.05), matching OCHRE's design. These tests pass under the current implementation and will FAIL if ticket-075 is accepted and implemented, serving as change-detectors.
