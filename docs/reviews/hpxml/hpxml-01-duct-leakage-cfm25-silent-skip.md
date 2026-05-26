# HPXML duct leakage CFM25 units silently skipped
**Review ID**: hpxml-01
**Category**: hpxml
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-io/src/hpxml/building.rs`
- `crates/hares-io/src/hpxml/equipment.rs`

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/utils/hpxml.py`
- `vendors/OCHRE/ochre/utils/equipment.py`

## Findings

### Finding 1: [Severity: high] CFM25 duct leakage silently dropped — zero leakage fed into DSE

**Description**:
When `<DuctLeakageMeasurement>` uses `<Units>CFM25</Units>` (volumetric flow at 25 Pa, e.g. 100 CFM), the parser logs a `tracing::warn` and produces `None` for the `leakage_fraction` field. Downstream, `compute_duct_dse_params` applies `unwrap_or(0.0)` (`resolve_hvac.rs:191`), silently zeroing out duct leakage in the ASHRAE 152 DSE calculation. A user providing a compliant HPXML file with CFM25 leakage gets an energy model with zero duct losses — no error, no diagnostic surfaced to the CLI or API caller.

**Code Location**: `crates/hares-io/src/hpxml/building.rs:1840-1848`

```rust
let fraction = match units.as_str() {
    "percent" => Some(value / 100.0),
    "fraction" => Some(value),
    _ => {
        tracing::warn!(
            units = %units,
            "unsupported duct leakage unit (cannot convert to fraction without fan flow); skipping"
        );
        None
    }
};
```

**Root Cause**:
The `DuctSystem` struct (`building.rs:197-204`) stores only `leakage_fraction: Option<f64>`, with no field for raw CFM25 values. At parse time, fan flow is not yet available (equipment resolution happens later in `resolve_hvac.rs`). The parser cannot convert CFM25 → fraction without knowing the system airflow.

The comment on line 1846 correctly identifies this dependency ("cannot convert to fraction without fan flow"), but the resolution is a silent skip rather than a deferred conversion or hard error.

By contrast, when fan flow IS available later — in `compute_duct_config` (`resolve_hvac.rs:381-384`) — the code already computes `fan_flow_m3_s` from explicit airflow or nominal CFM/ton. A conversion `CFM25_leakage / fan_flow_m3_s_equivalent` is straightforward arithmetic but never performed because the raw CFM25 value was discarded during parsing.

```rust
// resolve_hvac.rs:381-384 — fan flow IS available here
let cfm_per_ton = if is_heating { 350.0_f64 } else { 400.0_f64 };
let fan_flow_m3_s = explicit_airflow_m3_s_per_w
    .map(|airflow| capacity_w * airflow)
    .unwrap_or_else(|| capacity_w * (cfm_per_ton * CFM_TO_M3_S / W_PER_TON));
```

**Impact**:
- HPXML files using CFM25 duct leakage (a valid HPXML unit) produce DSE values reflecting zero duct loss — the model is silently optimistic.
- The user receives no error; `tracing::warn` is invisible unless the tracing subscriber is configured to capture warnings in production.
- OCHRE also does not convert CFM25 — it `assert`s `Units == "Percent"` at `hpxml.py:997`, which would crash loudly. HARES's behavior is worse in practice: OCHRE's crash prevents incorrect simulation, while HARES's silent skip allows incorrect simulation to proceed.

## Summary
- **Total findings**: 1
- **Critical**: 0 / **High**: 1 / **Medium**: 0 / **Low**: 0

## Recommendations

1. **Short-term (error instead of silent skip)**: Replace the `tracing::warn` branch in `building.rs:1840-1848` with a `tracing::error` (or return a `MissingField` / `ParseError` error) that fails parsing when CFM25 is detected. This matches OCHRE's "fail loudly" approach and prevents incorrect DSE from propagating silently. Include a user-facing message like: "CFM25 duct leakage detected. Use Percent or Fraction units, or supply airflow data so CFM25 can be converted."

2. **Long-term (defer CFM25→fraction conversion)**: Extend `DuctSystem` with an `leakage_cfm25: Option<f64>` field. Store raw CFM25 values during parsing. In `compute_duct_config` (`resolve_hvac.rs:332`), after `fan_flow_m3_s` is computed, convert CFM25 to fraction: `fraction = cfm25_cfm / fan_flow_cfm`. This properly handles CFM25 inputs and avoids the fan-flow availability problem.

3. **Documentation**: Document duct leakage unit expectations in the HARES user guide, noting that CFM25 is not supported and Percent is the recommended unit.

## References / Citations
- HPXML specification allows `CFM25`, `CFM50`, `Percent`, and `Fraction` as `<Units>` values for `<DuctLeakage>`
- OCHRE behaviour: `vendors/OCHRE/ochre/utils/hpxml.py:997` — asserts `Units == "Percent"` or value == 0 (fails loudly on CFM25)
- OCHRE DSE computation: `vendors/OCHRE/ochre/utils/equipment.py:161-465` — uses `fan_flow * supply_nom_leakage` to compute absolute cfm leakage from fractional leakage, demonstrating that conversion in the opposite direction (CFM25 / fan_flow = fraction) is the intended transformation
- HARES DSE computation: `crates/hares-io/src/hpxml/resolve_hvac.rs:332-419` — fan flow is available and used only for DSE calculation, not for leakage unit conversion
- HARES parser: `crates/hares-io/src/hpxml/building.rs:1820-1860` — duct leakage measurement parsing loop
- Test confirming the skip: `building.rs:5048-5102` (`duct_leakage_cfm25_rejected`) — verifies `leakage_fraction: None` from CFM25 input
