# No Capacity Autosizing When HPXML Omits HeatingCapacity / CoolingCapacity

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-io, hares-equipment

## Problem

When HPXML omits `<HeatingCapacity>` or `<CoolingCapacity>`, HARES has no autosizing
path. For `GasFurnaceConfig`, `capacity_w` is a required field; the resolver reads it
via `child_f64(heating, "HeatingCapacity")` and if absent, the typed-config builder
returns `Ok(None)` — meaning the furnace is silently skipped and the building has no
heating. For heat pumps, the config receives `None` for both heating and cooling
capacities, and `HeatPumpHeaterCore` falls back to `DEFAULT_HEATING_CAPACITY_W =
10_000 W` — a fixed default that may be wildly wrong for the building.

HPXML 4.x supports capacity autosizing (via `<AutoSized>true</AutoSized>` or by
simply omitting capacity). ResStock-generated HPXML files for autosized configurations
omit capacity elements, expecting the simulation tool to size using ACCA Manual J/S
load procedures. HARES silently substitutes 10 kW for every autosized heat pump.

## Evidence

`resolve_hvac.rs:521–523` (gas furnace):

```
let Some(capacity_w) = params.get("heating_capacity_w").and_then(Value::as_f64) else {
    return Ok(None);
};
```

Returns `None` (no equipment spec built) when capacity is absent.

`heat_pump/constants.rs:55–56`:

```
pub const DEFAULT_HEATING_CAPACITY_W: f64 = 10_000.0;
pub const DEFAULT_BACKUP_CAPACITY_W: f64 = 5_000.0;
```

`heat_pump/heater.rs` (init path) uses these constants when `heating_capacity_w` is
`None` in the typed config. No warning is emitted.

`resolve_hvac.rs:2429`:

```
hvac_capacity_w: None,
```

The `Building` struct has an `hvac_capacity_w` field that is never populated from
HPXML (it is always `None`), so even a building-level autosizing hint is unavailable.

## OCHRE Cross-check

OCHRE `hpxml.py` reads `AutosizingFactor` and `AutosizingLimits` and provides a
placeholder autosizing path. When capacity is truly absent, OCHRE raises an error
rather than silently substituting a default.

## Required Behavior

**Phase 1 (error on missing capacity — this ticket):** When a heating or cooling system
has no `<HeatingCapacity>` or `<CoolingCapacity>` and no autosizing signal, emit a loud
`HpxmlError::MissingField` rather than silently skipping the equipment or using a
10 kW default.

**Phase 2 (autosizing — separate ticket):** Implement a Manual J/ACCA-S autosizing path
using the building's design heating load and cooling load. This requires the envelope
thermal model to compute design-day loads and belongs in a dedicated follow-on ticket.

## Approach

**Gas furnace** (`resolve_hvac.rs:521–523`): Replace `return Ok(None)` with:
```
return Err(HpxmlError::MissingField {
    path: "HeatingSystem/HeatingCapacity",
    system_kind: "Gas Furnace",
    system_id: name.to_string(),
    reason: "HeatingCapacity is required; autosizing is not yet implemented",
});
```

**Heat pump** (`hares-equipment/src/hvac/heat_pump/heater.rs:508`): Replace the silent
`DEFAULT_HEATING_CAPACITY_W` fallback with:
```
.ok_or_else(|| EquipmentError::MissingField("heating_capacity_w required for HeatPumpHeater"))?
```
or emit a `tracing::error!` immediately before the fallback so the 10 kW default is
never silent. The error path is preferred.

Similarly for `DEFAULT_BACKUP_CAPACITY_W` at `heater.rs:330/569` when no backup capacity
is provided in the typed config.

## Citation

- HPXML 4.x schema §HeatingSystem/HeatingCapacity and HeatPump/HeatingCapacity:
  field is optional when `<AutosizingFactor>` is present (hpxml.nrel.gov)
- ACCA Manual S-2017: equipment sizing based on design heating/cooling loads
- ACCA Manual J-2016: residential load calculation (design conditions)
- EnergyPlus Engineering Reference §16.7: autosizing of unitary systems

## Annual kWh Impact Rank

**High** (when triggered). Autosized buildings silently receive a 10 kW heat pump
regardless of actual load. A correctly sized 25 kW ASHP for a large home would be
modelled at 40% capacity — causing the ER backup to run continuously, dramatically
overstating resistance heat consumption. Alternatively, a correctly sized 5 kW unit
for a small home would be modelled at 200% capacity — understating ER consumption
and overstating HP COP due to lower part-load operation.

## Definition of Done

- [ ] `try_build_gas_furnace_config` at `resolve_hvac.rs:521–523` returns `HpxmlError::MissingField` (not `Ok(None)`) when `heating_capacity_w` is absent
- [ ] `heater.rs:508` does not silently use `DEFAULT_HEATING_CAPACITY_W`; emits error or `tracing::error!` immediately before any fallback
- [ ] `Building.hvac_capacity_w` at `resolve_hvac.rs:2429` remains `None` (unchanged); a comment notes it is reserved for autosizing Phase 2
- [ ] Test: HPXML furnace fragment without `<HeatingCapacity>` → `Err` containing "HeatingCapacity" field name
- [ ] Test: HPXML heat pump fragment without `<HeatingCapacity>` → same error shape
- [ ] Test: HPXML furnace with valid `<HeatingCapacity>` continues to parse correctly

## Verification

```bash
cargo test -p hares-io -- resolve_hvac::tests::missing_capacity
cargo test -p hares-equipment -- heat_pump::heater::missing_capacity
```
