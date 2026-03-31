---
id: PARITY-025
title: Tune gas furnace fan heat and DSE interaction to match OCHRE
kind: fix
depends_on: [PARITY-022, PARITY-023]
files_to_touch:
  - crates/hares-equipment/src/hvac/furnace.rs
references:
  - vendors/OCHRE/ochre/Equipment/HVAC.py (lines 540-582)
verification:
  - cargo test -p hares-core --test parity parity_outputs
---

## Background/Context

Fan heat was added to gross capacity before DSE application (matching OCHRE's
`delivered_heat = heat_gain * shr + fan_power` then `* DSE`). This improved
total site energy (0.39% PASS) but regressed zone temp MAE from 0.086 to
0.231°C.

After PARITY-022 (duct leakage) and PARITY-023 (ideal_target) are fixed, the
furnace thermal balance will change significantly. This ticket re-tunes the fan
heat interaction against OCHRE reference with correct DSE and thermostat cycling.

## Work to Do

- [ ] After PARITY-022 and PARITY-023 are done, re-run parity for gas furnace
- [ ] Compare HARES furnace telemetry (fan_kw, thermal_output_w, fuel_input_w)
      step-by-step against OCHRE reference CSV
- [ ] Verify fan heat is applied at the right point (pre-DSE matches OCHRE)
- [ ] Check if space_fraction is applied correctly (OCHRE line 556-560)
- [ ] Adjust if needed — target zone temp MAE < 0.1°C and HVAC energy < 5%

## Files to Touch

- `crates/hares-equipment/src/hvac/furnace.rs`: Fan heat and DSE interaction

## Measures of Success

- [ ] `cz2a_gas_furnace_ac_res_wh` zone temp MAE < 0.1°C
- [ ] `cz2a_gas_furnace_ac_res_wh` HVAC energy deviation < 5%
- [ ] `cz2a_gas_furnace_ac_res_wh` total site energy deviation < 1%

## Verification

- [ ] `cargo test -p hares-core --test parity parity_outputs` — gas furnace metrics improve
