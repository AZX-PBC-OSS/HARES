# Room AC vs central AC differences: SHR, fan power, control logic
**Review ID**: equip-hvac-13
**Category**: equipment-hvac
**Date**: 2026-05-26

## Files Reviewed
crates/hares-equipment/src/hvac/air_conditioner.rs crates/hares-equipment/src/hvac/ac_config.rs crates/hares-equipment/src/hvac/cooling_config.rs

## Vendor/Reference Files Consulted
vendors/OCHRE/ochre/Equipment/HVAC.py

## Findings
### Finding 1: [Severity: medium]
**Description**: `CentralAirConditionerConfig` fields `fan_power_w` and `fan_power_w_per_cfm` are never consumed during equipment initialization. The typed config struct stores these values but `init_from_typed()` (air_conditioner.rs:531-618) does not read them or propagate them to `EquipmentConfig` extras. Meanwhile, `HvacEquipment::init()` (hvac_core.rs:467-471) reads fan power from config extras key-value pairs only (`"fan_power_w_per_m3_s"`, `"fan_power_w_per_cfm"`). As a result, any fan power values set on the typed `CentralAirConditionerConfig` are silently ignored at simulation time.

In contrast, the heat pump heater's typed config `fan_power_w` field is explicitly read and applied at `heat_pump/heater.rs:647` and `heat_pump/cooler.rs:153`, which is the correct pattern.

**Code Location**: `cooling_config.rs:39-42` (field declarations), `air_conditioner.rs:531-618` (init_from_typed for central AC — no fan_power processing), `hvac_core.rs:467-471` (where fan_power_w_per_m3_s is actually read from extras).

**Root Cause**: The typed config structs were added as a schema layer but `CentralAirConditionerConfig`'s fan power fields were never wired into the init path as config extras. Only the heat pump code path writes typed fan power fields into extras.

**Impact**: Users who configure central AC fan power via the typed config API see it silently discarded. The equipment runs with the default fan power of ~773 W/(m³/s) regardless of what was configured. Test code that sets `fan_power_w: Some(0.0)` on `CentralAirConditionerConfig` (e.g., air_conditioner.rs:2284, 2500, 2583, 2621) has no actual effect on fan power — those tests pass because the default fan power is used, but they mask the broken wiring.

### Finding 2: [Severity: low]
**Description**: `RoomAcConfig` (cooling_config.rs:277-341) does not declare `fan_power_w` or `fan_power_w_per_cfm` fields, unlike `CentralAirConditionerConfig` which has both (cooling_config.rs:39-42). While the room AC EER/CEER efficiency metric includes fan power at the AHRI rating point, users may still want to customize fan power for specific room AC models, for airflow-defect studies (HPXML supports `AirflowDefectRatio` on room ACs), or when the EIR has been back-calculated to exclude fan power. OCHRE accepts `"Rated Auxiliary Power (W)"` for RoomAC identically to AirConditioner (see HVAC.py:148 and test_hvac.py:95, 122).

**Code Location**: `cooling_config.rs:277-341` (struct RoomAcConfig — no fan_power fields).

**Root Cause**: Fan power fields were omitted from RoomAcConfig during the typed config migration. The heat pump common config includes `fan_power_w` / `fan_power_w_per_cfm` (`heat_pump_config.rs:55,61`), and the HP cooler init path reads and applies them (`heat_pump/cooler.rs:153`). `RoomAcConfig` missed this treatment.

**Impact**: Room AC users cannot customize fan power through the typed config API. Combined with Finding 1 that central AC can declare but not use fan power either, the typed config layer is incomplete for cooling fan power across the board.

### Finding 3: [Severity: low]
**Description**: SHR defaults for room AC and central AC are identical at 0.75 (air_conditioner.rs:527, 601). OCHRE defaults SHR to 1.0 if unspecified (HVAC.py:117-119). While 0.75 is physically reasonable as an AHRI-rated SHR for both equipment types, HPXML convention typically assumes a slightly higher default SHR for room ACs (0.80) vs central AC (0.75) because room AC airflows are lower, reducing latent removal. This is a minor calibration discrepancy.

**Code Location**: `air_conditioner.rs:527` (room AC SHR default), `air_conditioner.rs:601` (central AC SHR default).

**Root Cause**: Same default value used for both equipment types without differentiation.

**Impact**: Minor overestimation of latent cooling for room ACs when SHR is not specified in configuration.

### Finding 4: [Severity: —]
**Description**: Room AC correctly adopts the ideal thermostat target via `CoolingCore::ideal_target()` (air_conditioner.rs:1469-1479). Both `AirConditioner::ideal_target()` (line 279) and `RoomAC::ideal_target()` (line 330) delegate to the same `self.core.ideal_target()`. This reports the active cooling setpoint adjusted for demand response offset, matching the OCHRE protocol where the envelope solver uses the zone temperature and setpoint directly through `solve_ideal_capacity()` (HVAC.py:411-429). No issue found.

**Code Location**: `air_conditioner.rs:279-281`, `air_conditioner.rs:330-332`, `air_conditioner.rs:1469-1479`.

## Summary
- Total findings: 3 (plus 1 confirmatory)
- Critical / High / Medium / Low: 0 / 0 / 1 / 2

## Recommendations
1. **Wire central AC fan power from typed config into config extras** — In `CoolingCore::init_from_typed` for the central AC branch, read `cfg.fan_power_w` and `cfg.fan_power_w_per_cfm` and write them into `config.extras` (or derive `fan_power_w_per_m3_s` directly). Follow the pattern used in `heat_pump/cooler.rs:153` where `hp_cfg.common.fan_power_w_per_cfm` is written to extras.
2. **Add fan_power fields to RoomAcConfig** — Add `fan_power_w: Option<f64>` and `fan_power_w_per_cfm: Option<f64>` to `RoomAcConfig` for consistency with `CentralAirConditionerConfig` and the heat pump common config, then wire them into the room AC init path.
3. Consider adopting distinct default SHR values for room AC (0.80) vs central AC (0.75) to align with HPXML/ResStock conventions, or document that 0.75 is the universal default.

## References / Citations
- OCHRE HVAC.py:85 (HVAC.hvac_mult for cooling), lines 170-178 (Room AC duct DSE=1), lines 1080-1086 (RoomAC class), lines 414-429 (solve_ideal_capacity), lines 543-554 (delivered heat and power calculation)
- HARES air_conditioner.rs:494-509 (room AC init with duct_dse=1 and speed restriction)
- HARES air_conditioner.rs:516-618 (init_from_typed for room vs central AC)
- HARES hvac_core.rs:467-471 (fan power read from config extras)
- HARES cooling_config.rs:39-42 (central AC fan_power_w fields — dead)
- HARES cooling_config.rs:277-341 (room AC config — missing fan_power fields)
- ANSI/AHRI 210/240-2023: room AC CEER includes fan power; SEER for central AC excludes duct/air handler power but may include indoor fan for packaged units
