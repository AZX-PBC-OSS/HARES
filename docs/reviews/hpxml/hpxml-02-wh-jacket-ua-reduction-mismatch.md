# Water heater jacket R-value UA reduction timing mismatch vs OCHRE
**Review ID**: hpxml-02
**Category**: hpxml
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-io/src/hpxml/resolve_water_heater.rs` — HPXML parsing; stores `jacket_r_value_m2_k_w` and un-reduced `ua_w_per_k`
- `crates/hares-io/src/hpxml/water_heater_ua.rs` — UA derivation from EF/UEF (no jacket correction)
- `crates/hares-equipment/src/water_heater/mod.rs` — `apply_jacket_r_value()` shared function
- `crates/hares-equipment/src/water_heater/tank.rs` — `StratifiedTank` constructor (consumes reduced UA)
- `crates/hares-equipment/src/water_heater/gas.rs` — GasWH `init_typed()` applies jacket reduction
- `crates/hares-equipment/src/water_heater/resistance.rs` — ResistanceWH `init_typed()` applies jacket reduction
- `crates/hares-equipment/src/water_heater/heat_pump_wh.rs` — HeatPumpWH `init_typed()` applies jacket reduction

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/utils/hpxml.py` (lines 1131-1139): `parse_water_heater()` — jacket UA reduction at parse time
- `vendors/OCHRE/ochre/Equipment/WaterHeater.py` — WH equipment model (receives already-reduced UA from hpxml.py)

## Findings

### Finding 1: [Severity: medium]
**Description**: HARES applies jacket UA reduction at equipment initialization rather than at parse time, but uses a different physics model than OCHRE, producing materially different UA values. The jacket reduction **is** applied before the energy balance, so standby losses are not silently overestimated — but the reduction magnitude differs substantially from the reference implementation.

**Code Location**:
- OCHRE parse-time reduction: `vendors/OCHRE/ochre/utils/hpxml.py:1131-1139`
- HARES equipment-init reduction: `crates/hares-equipment/src/water_heater/mod.rs:162-177` (`apply_jacket_r_value`)
- HARES config storage of un-reduced UA: `crates/hares-io/src/hpxml/resolve_water_heater.rs:89` (sets `ua_w_per_k` from `ua_from_energy_factor` without jacket correction)

**Root Cause**: OCHRE models the jacket as added in series with the tank wall *skin* only, computes the UA reduction as a fraction of the wall-skin UA, and subtracts that from the *total* tank UA. HARES models the jacket as added in series with the *entire* tank thermal resistance (`1/new_ua = 1/ua_base + jacket_r / lateral_area`). Both models reduce UA before the energy balance runs, but the two formulas are not mathematically equivalent:

- OCHRE (hpxml.py:1131-1139):
  ```
  u_pre_skin = 1 / (jacket_thickness * 5.0 + 1/1.3 + 1/52.8)  # U-value of tank wall in IP
  ΔUA = jacket_r_ip / (1/u_pre_skin + jacket_r_ip) * u_pre_skin * lateral_area_ft2
  ua -= ΔUA
  ```
  OCHRE computes the jacket's contribution to the lateral-wall conduction path only, using default skin properties (R5/inch, air-film coefficients), then subtracts from total UA.

- HARES (mod.rs:162-177):
  ```
  r_total = 1/ua_base + jacket_r_si / lateral_area_m2
  new_ua = 1 / r_total
  ```
  HARES treats the jacket as additional series resistance to the entire tank UA.

**Impact**: For a typical 50-gallon electric water heater (ua ≈ 1.16 W/K, jacket R-10), OCHRE produces `new_ua ≈ 0.16 W/K` while HARES produces `new_ua ≈ 0.56 W/K` — a factor of ~3.5 difference with a substantial jacket installed. Without a jacket, both implementations agree (both use the same Burch & Erickson / Maguire & Roberts UA derivation). The HARES series-resistance model is methodologically cleaner (consistent series-resistance physics), but the OCHRE model is significantly more aggressive in reducing UA. This means HARES will predict **higher standby losses** than OCHRE for tank water heaters with jacket insulation specified in HPXML.

**Edge cases and guard conditions**:
- When `jacket_r_value_m2_k_w` is `None` or `≤ 0`: HARES returns `ua_base` unchanged (`mod.rs:168-172`). Correct.
- When `ua_base ≤ 0`: returns `ua_base` unchanged. Correct.
- When `ua_w_per_k` is `None` in the config (UA derivation failed): falls back to `DEFAULT_UA_W_PER_K` (2.0 W/K) which still passes through `apply_jacket_r_value`. The jacket reduction still works with the fallback value.
- The stored `ua_w_per_k` field on the config structs (`GasWaterHeaterConfig`, etc.) holds the **un-reduced** value. It is only read once in `init_typed()` where it's immediately passed through `apply_jacket_r_value()`. No other code path reads it — verified by grep showing only 3 call sites, all in `init_typed()`.

### Finding 2: [Severity: low]
**Description**: HARES `apply_jacket_r_value()` uses only the lateral (cylindrical side) area for the jacket resistance calculation, matching the physical reality that jackets wrap the tank sides. However, this means the same jacket R-value produces different effective UA reductions for different tank aspect ratios (same volume, different height/diameter). OCHRE would exhibit this behavior too. Not a bug per se, but worth documenting.

**Code Location**: `crates/hares-equipment/src/water_heater/mod.rs:174`

**Impact**: For a given jacket R-value and tank volume, a taller/narrower tank sees a smaller UA reduction than a shorter/wider tank because the lateral area differs. This is physically correct.

## Summary
- **Total findings**: 2
- **Critical**: 0
- **High**: 0
- **Medium**: 1
- **Low**: 1

## Recommendations
1. **Align jacket UA reduction methodology with OCHRE if matching OCHRE's numerical output is desired.** The current HARES series-resistance approach is physically cleaner, but diverges significantly from OCHRE. Consider: (a) adopt OCHRE's skin-based formula for numerical parity, or (b) document the deliberate methodological improvement and validate against measured standby loss data.

2. **Store the jacket-reduced UA in the config structs** rather than the un-reduced value, to avoid any risk of the un-reduced value being read by future code paths. Alternatively, rename `ua_w_per_k` to `ua_w_per_k_from_ef` to clarify that it excludes jacket correction, and add a separate computed `ua_w_per_k_effective` field for the jacket-reduced value.

3. **Add a comparison test** that reproduces OCHRE's specific jacket-reduction formula against a known HPXML input and compares the resulting UA to HARES's output, with a documented expectation of the difference.

## References / Citations
- OCHRE `parse_water_heater()` jacket reduction: `vendors/OCHRE/ochre/utils/hpxml.py:1131-1139`
- HARES `apply_jacket_r_value()`: `crates/hares-equipment/src/water_heater/mod.rs:162-177`
- HARES EF→UA derivation: `crates/hares-io/src/hpxml/water_heater_ua.rs:74-199`
- Burch & Erickson 2004: http://www.nrel.gov/docs/gen/fy04/36035.pdf
- Maguire & Roberts 2020: https://www.ashrae.org/file%20library/conferences/specialty%20conferences/2020%20building%20performance/papers/d-bsc20-c039.pdf
- OCHRE jacket thickness defaults: 1" for electric WH with EF<0.7, 2" otherwise (`hpxml.py:1134`)
- HARES jacket R-value IP→SI conversion: `r_value_ip_to_si` (1 hr·ft²·°F/Btu = 0.176110184 m²·K/W)
