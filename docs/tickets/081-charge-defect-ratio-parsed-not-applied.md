# ChargeDefectRatio Parsed From HPXML But Never Applied in Equipment Physics

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-io, hares-equipment

## Problem

The HPXML resolver reads `<ChargeDefectRatio>` from `CoolingSystem` and `HeatPump`
extension elements and stores the value in the params map under `"charge_defect_ratio"`.
However, no downstream equipment code (heat pump heater, heat pump cooler, or central
AC) reads or applies this value. The refrigerant charge correction that reduces both
capacity and efficiency for undercharged or overcharged systems is never computed.

Refrigerant charge defects of -10% to +10% are common in real installations (ANSI/
RESNET 301 estimates 5–30% of systems have charge defects). ASHRAE 152 and EnergyPlus
both specify how charge defect ratios reduce capacity and raise EIR.

## Evidence

`resolve_hvac.rs:1420–1421` (CoolingSystem path):

```
if let Some(v) = child_f64(ext, "ChargeDefectRatio") {
    params.insert("charge_defect_ratio".to_string(), json!(v));
}
```

`resolve_hvac.rs:1555–1556` (HeatPump path):

```
if let Some(v) = child_f64(ext, "ChargeDefectRatio") {
    params.insert("charge_defect_ratio".to_string(), json!(v));
}
```

No field named `charge_defect_ratio` exists in `CentralAirConditionerConfig`,
`HeatPumpHeaterConfig`, or `HeatPumpCoolerConfig`. Searching
`crates/hares-equipment/src/hvac/` for "charge_defect_ratio" returns no matches.
The value is stored in the params map and discarded during typed-config construction.

Test `hpxml_parity.rs:364` inserts `ChargeDefectRatio = -0.10` and verifies it
appears in the params map, but does not verify it affects equipment output.

## OCHRE Cross-check

OCHRE `Equipment/HVAC.py` applies refrigerant charge corrections from
`hpxml.py:ChargeDefectRatio` via the `update_capacity` method, which scales both
rated capacity and EIR using the correction factors from ANSI/RESNET 301. The
correction is applied at init time to the rated values before curve evaluation.

## Required Behavior

1. Add `charge_defect_ratio: Option<f64>` to `CentralAirConditionerConfig`,
   `HeatPumpHeaterConfig`, and `HeatPumpCoolerConfig` in `hares-equipment/src/hvac/`.
2. Populate from the params map during typed-config construction in `resolve_hvac.rs`
   (`try_build_central_ac_config`, `try_build_heat_pump_heater_config`, `try_build_heat_pump_cooler_config`).
3. In each equipment `init_from_typed`, apply capacity and EIR corrections per ANSI/RESNET/ICC 301-2022
   §4.4.4: for a charge defect ratio `r`:
   - `capacity_correction = 1.0 + Cc * r`
   - `eir_correction = 1.0 + Ce * r`
   where for cooling DX coils Cc ≈ −0.9, Ce ≈ +0.9 (ANSI/RESNET Table 4.4.4.1).
4. Apply corrections multiplicatively to all rated capacity and EIR values (per-speed
   for multi-speed equipment) before storing in `HvacEquipment`.

## Approach

- Config change: add `charge_defect_ratio: Option<f64>` to the three typed config structs.
- Resolver change: in the three typed-config builders, add:
  ```
  charge_defect_ratio: params.get("charge_defect_ratio").and_then(Value::as_f64),
  ```
- Equipment init change: in each `init_from_typed`, if `charge_defect_ratio` is `Some(r)`:
  ```
  let cap_corr = 1.0 + (-0.9) * r;
  let eir_corr = 1.0 + 0.9 * r;
  rated_capacity_w *= cap_corr;
  rated_eir *= eir_corr;
  ```
  The coefficients Cc and Ce should be sourced from ANSI/RESNET 301-2022 Table 4.4.4.1 and
  stored as named constants, not inline literals.

## Citation

- ANSI/RESNET/ICC 301-2022 §4.4.4: refrigerant charge correction factors for
  capacity and efficiency
- EnergyPlus Engineering Reference §16.1.4.1: `RefrigerantChargeDefectRatio`
  effect on DX coil capacity and EIR
- HPXML 4.x schema §extension/ChargeDefectRatio (hpxml.nrel.gov)

## Annual kWh Impact Rank

**Medium.** A -10% charge defect reduces cooling capacity ~9% and raises EIR ~9%.
For a typical 3-ton unit running 800 h/yr, this is ~200 kWh/yr error per affected
unit. ResStock estimates ~20% of units have meaningful charge defects.

## Definition of Done

- [ ] `charge_defect_ratio: Option<f64>` added to `CentralAirConditionerConfig`, `HeatPumpHeaterConfig`, `HeatPumpCoolerConfig`
- [ ] Resolver populates the field from `params["charge_defect_ratio"]` in all three typed-config builders
- [ ] Equipment `init_from_typed` for each type applies capacity/EIR corrections when `Some(r)` is present
- [ ] Correction coefficients stored as named constants (not inline magic numbers)
- [ ] Test: `charge_defect_ratio = -0.10` → rated cooling capacity × 0.91, rated cooling EIR × 0.91 (within 0.1%)
- [ ] Test: `charge_defect_ratio = 0.0` → rated capacity and EIR unchanged
- [ ] Test: `charge_defect_ratio` absent → `None`; no error, no correction applied

## Verification

```bash
cargo test -p hares-io -- resolve_hvac::tests::charge_defect
cargo test -p hares-equipment -- hvac::central_ac
cargo test -p hares-equipment -- hvac::heat_pump
```

The existing test at `hpxml_parity.rs:364` that only checks the params map must be extended to verify the resulting equipment has the corrected rated capacity and EIR.
