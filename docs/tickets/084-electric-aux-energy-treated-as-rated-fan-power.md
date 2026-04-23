# ElectricAuxiliaryEnergy Converted to Average-Watts Fan Power — Semantic Mismatch

**Severity**: Low
**Priority**: P3
**Status**: Open
**Areas**: hares-io, hares-equipment

## Problem

HPXML `<HeatingSystem>/<ElectricAuxiliaryEnergy>` (kWh/year) represents the annual
fan energy consumption including all part-load variation. HARES converts this to a
constant fan power by dividing by 8760 hours and multiplying by 1000 (W/kW):

```
aux_kwh / 8760.0 * 1000.0  →  average watts
```

This treated as a constant `fan_power_w` for every timestep. In reality,
`ElectricAuxiliaryEnergy` is an annual total; using it as a constant rated power
overstates fan consumption at low load and understates it at high load.

For a furnace that operates 800 h/yr at 300 W and idles at 10 W the rest of the
year, the annual energy is 800 × 300 + 7960 × 10 ≈ 320 kWh. Dividing by 8760 gives
36.5 W average. Applying this as a constant power, the furnace consumes 36.5 × 800 =
29 kWh during operating hours — less than half the actual 240 kWh during operation —
while charging 36.5 W during all non-operating hours as if the fan never stops.

## Evidence

`resolve_hvac.rs:1316–1320`:

```
if let Some(aux_kwh) = child_f64(heating, "ElectricAuxiliaryEnergy") {
    params.insert(
        "auxiliary_power_w".to_string(),
        json!(aux_kwh / 8760.0 * 1000.0),
    );
}
```

`resolve_hvac.rs:477–483`:

```
fn fan_power_from_params(params: &Map<String, Value>) -> Option<f64> {
    params
        .get("fan_power_w")
        .and_then(Value::as_f64)
        .or_else(|| params.get("auxiliary_power_w").and_then(Value::as_f64))
}
```

The `auxiliary_power_w` is used as `fan_power_w` — a rated, continuous, per-step
value — in the furnace equipment model.

## OCHRE Cross-check

OCHRE applies the same approximation: `ElectricAuxiliaryEnergy` is divided by hours
to yield an "average" fan power. OCHRE acknowledges this is an approximation but
accepts it for HPXML-based residential simulations where explicit fan curves are
absent. This is an accepted OCHRE limitation, not a target.

## Required Behavior

Two options (in priority order):

**Option A (preferred):** If `<extension><FanPowerWattsPerCFM>` or
`<extension><FanPowerWatts>` is present, use those (already done). When only
`ElectricAuxiliaryEnergy` is available, convert to `aux_fan_energy_kwh_yr` and store
separately. In the furnace step, compute fan power from actual airflow rate × fan
efficiency rather than treating the annual total as a constant rate.

**Option B (acceptable near-term):** Document the approximation explicitly in the
code. Add a note that the computed average watts will overstate fan energy during
off-hours if the fan has standby draw, and understate during operation relative to
rated conditions. No code change; documentation only.

The preferred approach is Option A when airflow data is available; Option B is
acceptable until fan curve data is available.

## Citation

- HPXML 4.x schema §HeatingSystem/ElectricAuxiliaryEnergy: "Annual auxiliary
  electricity consumption of the heating system fan" (kWh/year)
- ANSI/RESNET 301-2022 §4.2.2.1: blower fan power is a function of airflow and
  static pressure; not a constant average rate
- EnergyPlus Engineering Reference §16.4.3: fan power modelled as a function of
  mass flow rate and fan curve coefficients

## Annual kWh Impact Rank

**Low.** The annual fan energy total is preserved (average × 8760 = original kWh).
The error is a timing/distribution issue, not a magnitude issue, so annual kWh
bias is near zero. Hourly profiles are affected.

## Approach

At `resolve_hvac.rs:1316–1320`, add an inline comment to the conversion:

```
// ElectricAuxiliaryEnergy (kWh/yr) is divided by 8760 h to yield an average
// watts value. This is an approximation: real fan power is load-dependent.
// The annual energy total is preserved; per-timestep distribution is not.
// Prefer FanPowerWattsPerCFM or FanPowerWatts extension fields when available.
```

At `resolve_hvac.rs:477`, update the `fan_power_from_params` doc comment to
explicitly document the preference order.

## Definition of Done

- [ ] Comment at `resolve_hvac.rs:1317` explains the average-watts approximation and its limitation
- [ ] `fan_power_from_params` doc comment at `resolve_hvac.rs:477` documents preference order: `FanPowerWattsPerCFM` > `FanPowerWatts` > `ElectricAuxiliaryEnergy`-average
- [ ] No change to existing numeric conversion (approximation accepted per OCHRE precedent)

## Verification

This is a documentation-only change. No new tests required; existing tests at `resolve_hvac.rs` for fan power continue to pass unchanged.
