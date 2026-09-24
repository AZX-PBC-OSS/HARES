# Room AC SHR Always None From HPXML — Latent Cooling Always Zero

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-io, hares-equipment

## Problem

`try_build_room_ac_config` in `resolve_hvac.rs` hard-codes `shr: None` (line 891),
ignoring any SHR that HPXML may provide. The `CoolingSystem` loop does read
`<SensibleHeatFraction>` into `params["shr"]` (line 1403–1405), but the Room AC
typed-config builder does not pick it up. With `shr: None`, the room AC equipment
model defaults to SHR = 1.0 (all sensible, no latent removal), so the unit never
dehumidifies the zone regardless of how humid the air is.

Room air conditioners are particularly important for latent load because they are
often operated in high-humidity climates (Southeast, Gulf Coast) where latent removal
is a primary function. SHR for room ACs typically ranges from 0.75 to 0.90.

## Evidence

`resolve_hvac.rs:891`:

```
shr: None,
```

`resolve_hvac.rs:1403–1405`:

```
if let Some(shr) = child_f64(cooling, "SensibleHeatFraction") {
    params.insert("shr".to_string(), json!(shr));
}
```

The `params` map may contain `"shr"` from the HPXML element, but `try_build_room_ac_config`
does not call `params.get("shr").and_then(Value::as_f64)`.

## OCHRE Cross-check

OCHRE reads `SensibleHeatFraction` for room AC and passes it to the coil physics
as the steady-state SHR. OCHRE does not hard-code `shr = None` for room ACs.

## Required Behavior

In `try_build_room_ac_config`, read SHR from the params map. The `RoomAcConfig.shr`
field already exists; only the resolver assignment at line 891 is wrong.

## Approach

Change location: `resolve_hvac.rs:891`. Replace:
```
shr: None,
```
with:
```
shr: params.get("shr").and_then(Value::as_f64),
```

No other change is required. The `CoolingSystem` loop already reads
`<SensibleHeatFraction>` into `params["shr"]` at lines 1403–1405 before calling
`try_build_room_ac_config`.

Cross-reference: ticket 022 (Room AC ideal target) addresses the thermostat control
strategy for room AC. That is a separate defect from this SHR wiring gap. Do not merge.

## Citation

- HPXML 4.x schema §CoolingSystem/SensibleHeatFraction (hpxml.nrel.gov)
- AHRI 310/380-2017 §6.2: room AC rated SHR measured at 80°F DB/67°F WB
- EnergyPlus Engineering Reference §16.1.1: SHR partitions total cooling capacity
  into sensible and latent components; SHR = 1.0 implies zero latent removal


## Annual kWh Impact Rank

**Medium.** In humid climates, latent removal accounts for 20–40% of total cooling
load. With SHR = 1.0 (no latent), the room AC never removes moisture, the humidity
rises unchecked, and the thermostat sees no latent load reduction — causing the unit
to run longer than it should to achieve sensible comfort. Annual cooling kWh can be
overstated by 10–20% in humid climates.

## Definition of Done

- [ ] `resolve_hvac.rs:891`: `shr: None` replaced with `shr: params.get("shr").and_then(Value::as_f64)`
- [ ] Test: HPXML room AC with `<SensibleHeatFraction>0.82</SensibleHeatFraction>` → `RoomAcConfig.shr = Some(0.82)`
- [ ] Test: HPXML room AC without `<SensibleHeatFraction>` → `RoomAcConfig.shr = None`

## Verification

```bash
cargo test -p hares-io -- resolve_hvac::tests::room_ac_shr
```

Add a fixture in `crates/hares-io/tests/fixtures/` with a room AC element including `<SensibleHeatFraction>0.82</SensibleHeatFraction>`. Assert the resulting `RoomAcConfig.shr == Some(0.82)`.

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match (corrected: `shr: None` is at line 891 in `resolve_hvac.rs`, confirmed by direct read)
- [x] Described logic matches current implementation — `try_build_room_ac_config` (line 856) hard-codes `shr: None` at line 891; the `CoolingSystem` loop reads `SensibleHeatFraction` into `params["shr"]` at lines 1403–1405 for all `CoolingSystem` elements including Room AC; `try_build_room_ac_config` is called at line 1436 with those `params` but never reads `params.get("shr")`
- [x] OCHRE cross-check: **diverges from OCHRE** — `vendors/OCHRE/ochre/utils/hpxml.py` lines 878–885 read `SensibleHeatFraction` (via `hvac.get("SensibleHeatFraction")`) for all non-heat-pump cooling equipment, which includes room ACs, and emit it as `"SHR (-)"` in the output dict (line 908). OCHRE does not hard-code `shr = None` for room ACs. HARES diverges here accidentally.
- [x] EnergyPlus cross-check: **consistent with EnergyPlus principles** — EnergyPlus Engineering Reference (Coils chapter, "Single-Speed Electric DX Air Cooling Coil" section) states: *"Sensible/latent capacity splits are determined by the rated sensible heat ratio (SHR) and the apparatus dewpoint (ADP)/bypass factor (BF) approach"* and confirms SHR = 1.0 means all cooling is sensible with zero latent removal (dry-coil condition). The `ZoneHVAC:WindowAirConditioner` EnergyPlus model relies on an underlying DX coil that accepts SHR. The HARES default `cfg.shr.unwrap_or(0.75)` (air_conditioner.rs line 538) would be correct if `cfg.shr` were wired; the wire is missing.

### Web-Verified Citations

**Citation 1**: HPXML 4.x schema §CoolingSystem/SensibleHeatFraction (hpxml.nrel.gov)
- **Source found**: OCHRE source `vendors/OCHRE/ochre/utils/hpxml.py` lines 883–885 confirms `SensibleHeatFraction` exists as a HPXML element for non-heat-pump cooling systems (room ACs included); OpenStudio-HPXML workflow inputs documentation (openstudio-hpxml.readthedocs.io) confirms the field exists for cooling systems
- **Quoted passage**: From OCHRE `hpxml.py` line 885: `shr = hvac.get("SensibleHeatFraction")` — the field is read without any guard that restricts it to a non-room-AC type. OCHRE treats Room AC as just another non-heat-pump cooling system for this purpose.
- **Verdict**: Confirmed — `SensibleHeatFraction` is a valid HPXML element for `CoolingSystem`, applicable to room air conditioners.

**Citation 2**: AHRI 310/380-2017 §6.2: room AC rated SHR measured at 80°F DB/67°F WB
- **Source found**: Multiple AHRI standard listings confirm AHRI 310/380 is "Standard for Packaged Terminal Air-Conditioners and Heat Pumps" — it explicitly covers *packaged terminal* units (PTACs), which are commercial through-wall units, **not residential window/portable room air conditioners**.
- **Quoted passage**: Per AHRI search results: AHRI 310/380 "does not apply to room air-conditioners/heat pumps, as defined in CAN/CSA-C368.1." The correct standard for residential room (window) air conditioners is **ANSI/ASHRAE Standard 16**, "Method of Testing for Rating Room Air Conditioners, Packaged Terminal Air Conditioners, and Packaged Terminal Heat Pumps for Cooling and Heating Capacity." AHRI 370 is a **sound performance rating** standard for large outdoor equipment — it has nothing to do with SHR test conditions.
- **Verdict**: **Incorrect** — AHRI 310/380 covers PTACs, not residential window/room ACs. AHRI 370 is a sound standard, not a cooling performance standard. The correct test procedure reference is ANSI/ASHRAE 16 (or AHRI 210/240 for unitary ACs). The stated test conditions (80°F DB / 67°F WB) are correct standard indoor AHRI test conditions, but the standard number cited is wrong. This citation error does not affect the correctness of the described bug, only the accuracy of the supporting documentation.

**Citation 3**: EnergyPlus Engineering Reference §16.1.1: SHR partitions total cooling capacity into sensible and latent components; SHR = 1.0 implies zero latent removal
- **Source found**: EnergyPlus Engineering Reference, Coils chapter — Big Ladder Software mirror at bigladdersoftware.com/epx/docs/8-1/engineering-reference/page-078.html
- **Quoted passage**: *"Sensible/latent capacity splits are determined by the rated sensible heat ratio (SHR) and the apparatus dewpoint (ADP)/bypass factor (BF) approach... If the model determines that the cooling coil is dry (ωin < ωADP)... the humidity ratio remains unchanged (ωout = ωin), indicating zero latent capacity."* When SHR = 1.0 the coil is dry and provides zero latent removal.
- **Verdict**: **Partially correct** — the substance (SHR partitions sensible/latent; SHR=1.0 → zero latent) is confirmed. However, the EnergyPlus Engineering Reference for coils does **not** use a "§16.1.1" section numbering; the section is titled "Single-Speed Electric DX Air Cooling Coil" without a numeric hierarchical designator at that level. The section number claimed cannot be verified.

### Legitimacy

- **Verdict**: **Partially Legitimate**
- **Rationale**: The core bug is real and confirmed by direct code inspection: `try_build_room_ac_config` at line 891 hard-codes `shr: None`, the `params` map already contains `"shr"` loaded from `<SensibleHeatFraction>` at lines 1403–1405, and `try_build_room_ac_config` never reads `params.get("shr")`. OCHRE independently reads `SensibleHeatFraction` for room ACs without restriction (`hpxml.py` line 885), confirming the omission is unintentional. The equipment model defaults to `0.75` (not `1.0` as the ticket claims) when `shr` is `None` (air_conditioner.rs line 538: `cfg.shr.unwrap_or(0.75)`), so the severity claim ("unit never dehumidifies") is overstated — the default SHR of 0.75 provides significant latent removal even when the HPXML value is dropped. The real loss is the inability to use the HPXML-specified value. Two citations are inaccurate (wrong AHRI standard number; unverifiable EnergyPlus section number), but these errors do not affect the bug description itself.

### Proposed Fix Summary

In `try_build_room_ac_config` (`resolve_hvac.rs` line 891), replace the hard-coded `shr: None` with `shr: params.get("shr").and_then(Value::as_f64)`. No other change is required; the `CoolingSystem` loop already loads `<SensibleHeatFraction>` into `params["shr"]` before calling `try_build_room_ac_config`. Do NOT modify any `crates/*/src/` production file as part of this audit.

### Test Written

- File: `crates/hares-io/src/hpxml/resolve_hvac.rs` (inside existing `#[cfg(test)]` module)
- Tests added:
  - `room_ac_builder_propagates_shr_from_params` — passes `params["shr"] = 0.82` and asserts `cfg.shr == Some(0.82)`. **Currently FAILS**, demonstrating the bug.
  - `room_ac_builder_shr_is_none_when_not_in_params` — omits `"shr"` from params and asserts `cfg.shr == None`. **Currently PASSES**.
- Run with: `cargo test -p hares-io --lib -- tests::room_ac_builder_propagates_shr_from_params tests::room_ac_builder_shr_is_none_when_not_in_params`
