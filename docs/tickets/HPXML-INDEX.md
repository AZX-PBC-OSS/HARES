# HPXML Gap Tickets — Dependency Summary

Parse additional HPXML config values into equipment params, using HPXML element names
as canonical config keys (IO layer converts imperial→SI, equipment stores SI).

## Dependency Graph

```
Tier 0 — Foundation:
  HPXML-000  Align ALL equipment config keys with HPXML element names
             - Rename ~40 keys in HPXML parser (equipment.rs) to HPXML PascalCase
             - Update ~15 equipment models to accept HPXML-named keys
             - Remove OCHRE-legacy aliases ("Heat Pump Lockout Temperature (C)" etc.)
             - Move battery RTE sqrt to equipment side
             - Convert WH volume/capacity to SI at parse time

Tier 1 — All depend on HPXML-000, parallelizable within safe sets:

  Category A — Fully wired (HPXML parse only):
    HPXML-001  Dehumidifier: DehumidistatSetpoint, FractionDehumidificationLoadServed
    HPXML-002  Battery: UsableCapacity → min/max_soc, NominalVoltage, Location
    HPXML-004  HPWH: HPWHOperatingMode
    HPXML-006  PV: SystemLossesFraction, Tracking
    HPXML-009  HVAC: CrankcaseHeaterPowerWatts, FanMotorType
    HPXML-010  HVAC: AirflowDefectRatio, ChargeDefectRatio
    HPXML-012  HP: PanHeaterPowerWatts, BackupType, BackupHeatingActiveDuringDefrost

  Category B — Small equipment change + HPXML parse:
    HPXML-003  Gas pilot light (add PilotLight field to furnace/boiler)
    HPXML-005  Setback profile synthesis + HeatingSeason/CoolingSeason dates
    HPXML-016  Design airflow: HeatingDesignAirflowCFM, CoolingDesignAirflowCFM

Tier 2 — Backlogged (needs new model infrastructure):
    HPXML-007  WH: StandbyLoss, UsageBin
    HPXML-008  HP: HeatingCapacity17F → curve anchoring
    HPXML-011  Ventilation: HoursInOperation → schedule generation
    HPXML-013  Ducts: DuctSurfaceArea, DuctEffectiveRValue, supply/return leakage split
    HPXML-014  HP: HeatingDetailedPerformanceData → table interpolation
    HPXML-015  HPWH: HPWHDucting → zone coupling
```

## Naming Convention (established by HPXML-000)

- **All config keys:** `snake_case_of_hpxml_name_with_si_unit`
  (e.g., `CompressorLockoutTemperature` → `"compressor_lockout_temp_c"`)
- **HARES-only params (no HPXML equivalent):** Same convention
  (e.g., `"cell_resistance_ohm"`, `"min_soc"`, `"noct_c"`)
- **Values:** Always SI units. IO layer converts imperial→SI at parse time.
- **No OCHRE legacy:** Remove human-readable string keys like `"Heat Pump Lockout Temperature (C)"`

## Safe Parallel Sets (no function overlap within equipment.rs)

- Set A: HPXML-001, HPXML-002, HPXML-004, HPXML-006
- Set B: HPXML-003, HPXML-005, HPXML-009, HPXML-010, HPXML-012, HPXML-016

## Recommended Implementation Order

1. **HPXML-000** — Equipment-side key alignment (must go first)
2. **Category A batch** — 001, 002, 004, 006, 009, 010, 012 (all parallel after 000)
3. **Category B batch** — 003, 005, 016 (parallel after 000)
4. **Backlog** — 007, 008, 011, 013, 014, 015
