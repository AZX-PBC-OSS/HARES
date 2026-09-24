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

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation
- [x] Referenced line numbers still match (corrected locations noted below)
- [x] Described logic matches current implementation
- [x] OCHRE cross-check result: diverges — with file+line evidence
- [x] EnergyPlus cross-check result: N/A (EnergyPlus §16.7 addresses commercial unitary autosizing, not the HPXML-to-typed-config layer; see note below)

**Line number corrections:**

| Ticket citation | Actual location |
|---|---|
| `resolve_hvac.rs:521–523` gas furnace `Ok(None)` | Confirmed at lines 521–522 |
| `heat_pump/constants.rs:55–56` | Confirmed at lines 55–57 |
| `heater.rs:508` silent fallback | Confirmed at line 473 (`init_from_typed`) and line 508 (fan-power sizing fallback) |
| `heater.rs:330/569` backup fallback | Confirmed at lines 330 and 572 |
| `resolve_hvac.rs:2429` `hvac_capacity_w: None` | Confirmed at line 2429 |

**Gas furnace silent-skip nuance:** The ticket states the furnace is "silently skipped." The actual behaviour is subtler: `try_build_gas_furnace_config` returns `Ok(None)` (line 522), setting `spec.typed_config = None`. The spec IS still pushed to the equipment list (line 1372). When the solver later calls `equipment_config_from_spec`, it builds a raw config from `params`. Since `heating_capacity_w` is absent from params, downstream `init` will fail — but with an opaque equipment-init error rather than a meaningful `HpxmlError::MissingField`. The effect is the same (no working furnace) but the error message is indirect and can be confusing. The ticket's prescription (return `Err(HpxmlError::MissingField)` immediately) is still the correct fix.

**ASHP silent-default:** `try_build_heat_pump_heater_config` (line 936) accepts `heating_capacity_w = None` and passes it straight into `HeatPumpHeaterConfig`. At `init_from_typed` line 473, `vec![DEFAULT_HEATING_CAPACITY_W]` (10 000 W) is substituted with no log output. The test suite confirms parse succeeds silently.

**OCHRE cross-check (`vendors/OCHRE/ochre/utils/hpxml.py:847`):**
```python
capacity = convert(hvac[f"{hvac_type}Capacity"], "Btu/hour", "W")
```
OCHRE uses direct dict access `hvac[key]` (not `.get(key)`). If `HeatingCapacity` is absent, Python raises `KeyError`, which OCHRE surfaces as an unhandled exception — equivalent to a loud error. OCHRE does **not** silently default capacity; HARES diverges by passing `None` through to a 10 kW fallback (heat pump) or an indirect raw-config init failure (furnace). The divergence is **accidental** — HARES should match OCHRE's loud-failure behaviour.

No `AutosizingFactor` or `AutosizingLimits` fields were found anywhere in the OCHRE codebase. The ticket's claim that "OCHRE reads `AutosizingFactor` and `AutosizingLimits`" was **not verified** in the vendored source. The relevant OCHRE autosizing path was not found.

### Web-Verified Citations

**Citation 1**: HPXML 4.x schema — HeatingCapacity optional when `<AutosizingFactor>` present  
**Source found**: HPXML Data Dictionary v4.0.0 and v4.2.0 at hpxml.nlr.gov  
**Quoted passage**: "Output heating capacity; typically the nameplate capacity at 47F. Min Occurrences: 0, Max Occurrences: 1" (both `HeatPump/HeatingCapacity` and `HeatingSystem/HeatingCapacity`)  
**Verdict**: **Partially correct**. `HeatingCapacity` is genuinely optional (min occurrences = 0) in both HPXML 4.0 and 4.2. However, no `AutosizingFactor` field was found in the HPXML data dictionary pages for `HeatPump` — the ticket's specific claim that capacity is "optional when `<AutosizingFactor>` is present" could not be confirmed from the schema. The field is simply optional unconditionally. OpenStudio-HPXML uses `-1` as the autosizing sentinel value rather than a separate `AutosizingFactor` element.

**Citation 2**: ACCA Manual S-2017 — equipment sizing based on design heating/cooling loads  
**Source found**: ACCA.org Manual S product page; multiple secondary summaries (contractingbusiness.com, basc.pnnl.gov)  
**Quoted passage**: "Manual S instructs designers how to select equipment which meets the application requirements (heating, sensible cooling, and latent cooling) at the design conditions that were used for calculating the loads." (contractingbusiness.com summary)  
**Verdict**: **Confirmed** — Manual S is the correct reference for equipment selection based on Manual J loads. The citation is accurate in substance.

**Citation 3**: ACCA Manual J-2016 — residential load calculation  
**Source found**: ACCA.org Manual J product page; ANSI/ACCA 2 Manual J-2016  
**Quoted passage**: "Manual J8 determines your specific home's heating and cooling needs based on where your home is located (Weather location), which direction your home faces (Orientation), the insulation R-values in your floor, ceiling and walls and how humid your climate is." (load-calculations.com summary)  
**Verdict**: **Confirmed** — Manual J 8th Edition is the correct ANSI standard for residential load calculation. The citation is accurate.

**Citation 4**: EnergyPlus Engineering Reference §16.7 — autosizing of unitary systems  
**Source found**: bigladdersoftware.com EnergyPlus 25.1 Engineering Reference, Component Sizing chapter  
**Quoted passage**: "In EnergyPlus each HVAC component sizes itself. Each component module contains a sizing subroutine. When a component is called for the first time in a simulation, it reads in its user specified input data and then calls the sizing subroutine. This routine checks the autosizable input fields for missing data and calculates the data when needed."  
**Verdict**: **Partially correct**. EnergyPlus does have autosizing procedures for unitary systems, and the Component Sizing chapter covers this. However, EnergyPlus §16.7 is a commercial HVAC autosizing path (design-day simulation driven) that is not directly analogous to HPXML's capacity-absent case. The HARES issue is in the HPXML parsing layer, not in a simulation-time autosizing loop. The citation is directionally valid but the section number may not correspond precisely to the residential/unitary autosizing content.

### Legitimacy
**Verdict**: **Legitimate** (with one minor inaccuracy noted)

**Rationale**: Both core bugs are real and confirmed by code inspection and failing tests. (1) `try_build_gas_furnace_config` at `resolve_hvac.rs:521–522` returns `Ok(None)` when `heating_capacity_w` is absent, yielding a `typed_config = None` spec that will produce an opaque init error downstream rather than the required `HpxmlError::MissingField`. (2) `try_build_heat_pump_heater_config` passes `heating_capacity_w: None` into `HeatPumpHeaterConfig`, and `init_from_typed` at `heater.rs:473` silently substitutes `DEFAULT_HEATING_CAPACITY_W = 10_000 W` with no log output. Both behaviours were confirmed by writing four regression tests: the two missing-capacity tests **fail** with the current code (parse succeeds instead of erroring), while the two happy-path tests pass. The OCHRE divergence is real: OCHRE's direct dict access raises `KeyError` on missing capacity rather than defaulting. The one minor inaccuracy is the OCHRE `AutosizingFactor`/`AutosizingLimits` claim — no such OCHRE code was found in the vendored source — but this does not affect the core legitimacy of the ticket.

### Proposed Fix Summary
**Gas furnace** (`resolve_hvac.rs:521–522`): Replace the `let Some(capacity_w) ... return Ok(None)` guard with `return Err(HpxmlError::MissingField { path: "HeatingSystem/HeatingCapacity", system_kind: "Gas Furnace", system_id: name.to_string(), reason: "…" })`. Apply the same change to `try_build_gas_boiler_config` at line 620–622 for consistency.

**ASHP/MSHP heat pump** (`heater.rs:473`): Replace `vec![DEFAULT_HEATING_CAPACITY_W]` with an error returned from `init_from_typed`; e.g. `return Err(HaresError::Equipment("heating_capacity_w required for HeatPumpHeater; autosizing not yet implemented".into()))`. The `DEFAULT_HEATING_CAPACITY_W` constant and the backup-capacity fallbacks at lines 330/572 require the same treatment.

Do **not** change `resolve_hvac.rs:2429` (`hvac_capacity_w: None`) — this field is reserved for Phase 2 autosizing and is correctly `None` at this stage.

### Test Written
- **File**: `crates/hares-io/tests/silent_default_regressions.rs` (appended, lines 473–579)
- **What it tests**:
  - `gas_furnace_missing_heating_capacity_errors`: furnace without `<HeatingCapacity>` must return `HpxmlError::MissingField` with path `"HeatingSystem/HeatingCapacity"` and kind `"Gas Furnace"`. **Currently FAILS** (parse succeeds — bug confirmed).
  - `gas_furnace_with_heating_capacity_and_afue_resolves`: furnace with both fields resolves cleanly. **Currently PASSES**.
  - `ashp_missing_heating_capacity_errors`: ASHP heat pump without `<HeatingCapacity>` must return `HpxmlError::MissingField` with path `"HeatPump/HeatingCapacity"` and kind `"ASHP Heater"`. **Currently FAILS** (parse succeeds — bug confirmed).
  - `ashp_with_heating_capacity_resolves`: ASHP with explicit `<HeatingCapacity>` resolves cleanly. **Currently PASSES**.
