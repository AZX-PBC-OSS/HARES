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

---

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match
  - `resolve_hvac.rs:1316–1320`: **confirmed** — `ElectricAuxiliaryEnergy` is extracted via `child_f64` and the expression `aux_kwh / 8760.0 * 1000.0` is inserted as `"auxiliary_power_w"` at **lines 1316–1320** exactly as cited.
  - `resolve_hvac.rs:477–483`: **confirmed** — `fan_power_from_params` is defined with a doc comment at line 477, implements the `fan_power_w` → `auxiliary_power_w` fallback chain at lines 479–482 exactly as cited.
- [x] Described logic matches current implementation — the fallback from `fan_power_w` to `auxiliary_power_w` is present across all 7 equipment builders (lines 531, 577, 630, 675, 789, 946, 1059). The `fan_power_w` field is passed directly to each equipment config unchanged.
- [x] Bug is still present and not yet fixed — no comment at line 1317 documents the approximation; no `aux_fan_energy_kwh_yr` field exists.
- [x] OCHRE cross-check: **diverges — intentionally** (see detail below)
- [x] EnergyPlus cross-check: **partially relevant** (see detail below)

#### OCHRE cross-check detail

OCHRE `vendors/OCHRE/ochre/utils/hpxml.py:887–892`:

```python
# Get auxiliary power (fans, pumps, etc.) air flow rate
hvac_ext = hvac.get("extension", {})
if name == "Boiler":
    # Note: ResStock assumes 2080 hours/year, see hvac.rb line 1754 (get_default_boiler_eae)
    # see also ANSI/RESNET/ICC 301-2019 Equation 4.4-5
    aux_power = hvac.get("ElectricAuxiliaryEnergy", 0) / 2080 * 1000  # converts kWh/year to W
elif "FanPowerWattsPerCFM" in hvac_ext:
    ...
else:
    aux_power = hvac_ext.get("FanPowerWatts", 0)
```

Key differences from HARES:

1. **OCHRE uses 2080 h/yr, not 8760 h/yr, for Boiler** — OCHRE's comment cites ANSI/RESNET/ICC 301-2019 Equation 4.4-5 and the ResStock convention that boiler auxiliaries (pumps, controls) run for 2080 h/yr, not continuously.
2. **OCHRE only applies `ElectricAuxiliaryEnergy` to Boilers** — for furnaces and heat pumps OCHRE reads `FanPowerWattsPerCFM` or `FanPowerWatts` from the `<extension>` block; it does not fall back to `ElectricAuxiliaryEnergy`. HARES, by contrast, applies the `ElectricAuxiliaryEnergy / 8760 * 1000` path to _all_ heating system types (gas furnace, electric furnace, boilers, heat pump heater).
3. **The divisor matters for boilers** — for the most common use case (boilers), OCHRE divides by 2080 (yielding a pump/control power figure ~4.2× higher than HARES's 8760 divisor). For furnaces OCHRE does not use `ElectricAuxiliaryEnergy` at all.

The ticket's claim that "OCHRE applies the same approximation: ElectricAuxiliaryEnergy is divided by hours to yield an 'average' fan power" is **partially incorrect**: OCHRE uses a different divisor (2080, not 8760), applies it only to boilers (not furnaces), and explicitly attributes the 2080 figure to a RESNET standard equation. HARES diverges from OCHRE in a way that understates auxiliary power for boilers by a factor of ~4.2.

#### EnergyPlus cross-check detail

The ticket cites "EnergyPlus Engineering Reference §16.4.3: fan power modelled as a function of mass flow rate and fan curve coefficients." This is **substantively correct but the section number is inaccurate**:

- The EnergyPlus Engineering Reference does not use explicit section numbers like "16.4.3" in its online HTML documentation. The chapter is titled simply "Air System Fans" (source: https://bigladdersoftware.com/epx/docs/22-2/engineering-reference/air-system-fans.html). A search for "16.4.3" in EnergyPlus documentation returns no exact match for that designation.
- The EnergyPlus fan model for `Fan:VariableVolume` uses a 4th-order polynomial relating flow fraction to power fraction: `fpl = c1 + c2·fflow + c3·fflow² + c4·fflow³ + c5·fflow⁴`, then `Q̇tot = fpl · ṁdesign · ΔP / (εtot · ρair)`.
- For `Fan:ConstantVolume` and `Fan:OnOff`, power is `Q̇tot = ṁ · ΔP / (εtot · ρair)` scaled by actual mass flow, not a constant rated value.
- The substantive point — that EnergyPlus models fan power as load-dependent rather than a constant average rate — is accurate. The section number "§16.4.3" appears to be an approximation of a table-of-contents position rather than a URL-addressable section anchor, or may refer to a specific PDF page layout that does not appear in the HTML version.

### Web-Verified Citations

**Citation 1**: "HPXML 4.x schema §HeatingSystem/ElectricAuxiliaryEnergy: 'Annual auxiliary electricity consumption of the heating system fan' (kWh/year)"

- **Source found**: HPXML Data Dictionary v4.2.0 at https://hpxml.nlr.gov/datadictionary/4.2.0/Building/BuildingDetails/Systems/HVAC/HVACPlant/HeatingSystem/ElectricAuxiliaryEnergy
- **Quoted passage**: "The average annual auxiliary electrical energy consumption for, e.g., a gas furnace or boiler, in kilowatt-hours per year." Units: kWh/year, Data Type: double, Min Occurrences: 0 (optional).
- **Verdict**: **Confirmed with minor wording difference** — the official description says "average annual auxiliary electrical energy consumption" not "Annual auxiliary electricity consumption of the heating system fan". The units (kWh/year) and the annual-total nature of the field are confirmed.

**Citation 2**: "ANSI/RESNET 301-2022 §4.2.2.1: blower fan power is a function of airflow and static pressure; not a constant average rate"

- **Source found**: ANSI/RESNET/ICC 301-2022 standard PDF at https://www.resnet.us/wp-content/uploads/ANSIRESNETICC301-2022_resnetpblshd.pdf (PDF is image-scanned; direct text extraction failed). Secondary source: search results showing that section 4.2.2 of RESNET 301 covers "Dwelling Unit Mechanical Ventilation System fan power" with the statement "Blower Fan wattage shall be calculated by multiplying the fan specific fan power efficiency by the larger of the heating and cooling flowrates." Addendum Addendum C-2024 also treats fan power as a function of flow rate.
- **Quoted passage** (from search result excerpt): "Blower Fan wattage shall be calculated by multiplying the fan specific fan power efficiency by the larger of the heating and cooling flowrates."
- **Verdict**: **Partially confirmed** — the substance (fan power is flow-dependent, not a constant average) is consistent with RESNET 301. However, §4.2.2 in RESNET 301-2022 specifically addresses **mechanical ventilation** fans, not HVAC blower fans for heating/cooling delivery. Section 4.2.2.1 as cited by the ticket may not precisely correspond to a blower fan power subsection; the RESNET standard's boiler auxiliary energy equations appear in a different section (OCHRE cites Equation 4.4-5 for boilers). The standard is not publicly machine-readable so the exact section number cannot be verified.
- **Verdict**: **Partially correct** — substantive point is sound but the specific section reference (§4.2.2.1) is likely imprecise.

**Citation 3**: "EnergyPlus Engineering Reference §16.4.3: fan power modelled as a function of mass flow rate and fan curve coefficients"

- **Source found**: EnergyPlus 22.2 Engineering Reference, Air System Fans chapter at https://bigladdersoftware.com/epx/docs/22-2/engineering-reference/air-system-fans.html; also EnergyPlus 9.6 version at https://bigladdersoftware.com/epx/docs/9-6/engineering-reference/air-system-fans.html
- **Quoted passage**: "fpl = c1 + c2·fflow + c3·f²flow + c4·f³flow + c5·f⁴flow; Q̇tot = fpl·ṁdesign·ΔP/(εtot·ρair)" (variable-speed fan). For constant-volume fans: "Q̇tot = ṁ·ΔP/(εtot·ρair)" with actual mass flow as input.
- **Verdict**: **Partially correct** — the substantive claim (EnergyPlus models fan power as a function of flow, not a constant average) is confirmed by direct web fetch. The section number "§16.4.3" does not appear as a named anchor in the HTML Engineering Reference; the chapter appears to sit in a numbered position in a PDF table of contents (search results show "16.4.3 Fan Energy Index" as a possible subsection), but the fan-power-as-function-of-flow content is in the "Simulation" subsection, not "16.4.3". Citation is substantively valid but section number is inaccurate.

### Additional Finding Not in Ticket: Divisor Error for Boilers

The audit uncovered a more material issue not mentioned in the ticket:

HARES divides `ElectricAuxiliaryEnergy` by **8760** for all heating systems. OCHRE (and by implication ANSI/RESNET/ICC 301-2019 Equation 4.4-5 via ResStock) divides by **2080** for boilers, reflecting that boiler auxiliary loads (pumps, controls) operate during heating-season hours only, not year-round. The factor-of-4.2× discrepancy means HARES underestimates boiler auxiliary power roughly fourfold relative to OCHRE's RESNET-derived approach.

For furnaces, OCHRE does not use `ElectricAuxiliaryEnergy` at all (falling back to `FanPowerWattsPerCFM`/`FanPowerWatts`), so the 8760-divisor issue is less material for furnace fan power.

### Legitimacy

- **Verdict**: **Partially Legitimate**
- **Rationale**: The core semantic issue is real and confirmed: `ElectricAuxiliaryEnergy` (kWh/year) is an annual total that HARES converts to a flat average rate by dividing by 8760 h, then applies as a constant per-timestep rated power. The HPXML Data Dictionary confirms the field is "average annual auxiliary electrical energy consumption … in kilowatt-hours per year," not a rated continuous power (verified via direct web fetch of the HPXML Data Dictionary v4.2.0). EnergyPlus models fan power as load-dependent (confirmed by direct web fetch of the EnergyPlus Engineering Reference). However, the ticket has two inaccuracies: (1) it asserts that OCHRE applies the same 8760-divisor approximation — OCHRE actually uses 2080 h/yr for boilers and does not use `ElectricAuxiliaryEnergy` for furnaces at all; (2) the RESNET and EnergyPlus section numbers are imprecise. More importantly, the 8760-vs-2080 divisor issue is a correctness defect for boilers that is more severe than the ticket's "timing/distribution only, annual total preserved" framing implies — for boilers the annual total is itself wrong (HARES produces ~4.2× lower auxiliary power than RESNET intent). The ticket's proposed fix (Option B: documentation-only comment) is appropriate for furnaces but **insufficient for boilers** where the divisor should be 2080, not 8760.

### Proposed Fix Summary

**Do not implement** (audit only). The minimal fix has two separate concerns:

1. **For furnaces (timing/distribution issue only)**: Add an inline comment at `resolve_hvac.rs:1317` explaining the average-watts approximation and its limitation (annual total preserved, per-timestep profile distorted). This matches the ticket's Option B.
2. **For boilers (divisor correctness issue)**: Change the divisor from 8760 to 2080 when the heating system type is `Boiler`, aligning with OCHRE's RESNET-cited approach. This is a new finding not addressed by the ticket; it should be tracked as a separate sub-issue or the ticket should be elevated to include it.

The `fan_power_from_params` doc comment update at line 477 (Option B documentation) is straightforward and correct as described.

### Test Written

The existing test at `resolve_hvac.rs:4088` (`electric_auxiliary_energy_kwh_per_year_converts_to_watts`) already encodes the current behavior (876 kWh/yr → 100.0 W via `/8760*1000`). A regression test is not written for the timing/distribution issue because the ticket's accepted resolution is a documentation-only change (Option B) — the numeric result is intentionally preserved.

For the newly-identified boiler divisor issue, a failing test demonstrating the 8760-vs-2080 mismatch would be:

**File**: `crates/hares-io/src/hpxml/resolve_hvac.rs` (within `#[cfg(test)] mod tests`)
**Test name**: `boiler_electric_auxiliary_energy_should_use_2080_hour_divisor`
**What it tests**: Given `ElectricAuxiliaryEnergy = 2080` kWh/yr for a Boiler type heating system, `auxiliary_power_w` should equal 1000.0 W (2080/2080*1000), not 237.4 W (2080/8760*1000). This test will fail under the current implementation and serve as a red flag for the boiler divisor defect.

```rust
// Demonstrates that the boiler divisor should be 2080 h/yr (RESNET/OCHRE convention),
// not 8760 h/yr (current HARES behavior). This test FAILS under current code.
#[test]
fn boiler_electric_auxiliary_energy_should_use_2080_hour_divisor() {
    let xml = r#"
        <HPXML>
          <Building>
            <BuildingDetails>
              <Systems>
                <HVAC>
                  <HVACPlant>
                    <HeatingSystem>
                      <SystemIdentifier id="htg1"/>
                      <HeatingSystemType><Boiler><BoilerType>hot water</BoilerType></Boiler></HeatingSystemType>
                      <HeatingSystemFuel>natural gas</HeatingSystemFuel>
                      <ElectricAuxiliaryEnergy>2080</ElectricAuxiliaryEnergy>
                      <FractionHeatLoadServed>1.0</FractionHeatLoadServed>
                    </HeatingSystem>
                  </HVACPlant>
                </HVAC>
              </Systems>
            </BuildingDetails>
          </Building>
        </HPXML>
    "#;
    // Parse and resolve
    // Expected: 2080 kWh/yr / 2080 h/yr * 1000 W/kW = 1000.0 W
    // Current (bug): 2080 / 8760 * 1000 ≈ 237.4 W
    // (test infrastructure omitted — adapt to resolve_hvac test helpers)
    let expected_w = 1000.0_f64; // per OCHRE/RESNET 301-2019 Eq. 4.4-5
    let actual_w: f64 = 2080.0 / 8760.0 * 1000.0; // current HARES behavior
    assert!(
        (actual_w - expected_w).abs() < 1.0,
        "Boiler ElectricAuxiliaryEnergy=2080 kWh/yr should yield {expected_w} W \
         (2080 h/yr divisor per OCHRE/RESNET), got {actual_w:.1} W (8760 h/yr divisor)"
    );
}
```

**Status**: Test body written above as a specification; not inserted into production test suite because the audit scope prohibits production code changes and this test **intentionally fails** to flag the boiler defect. A follow-up ticket should be opened for the boiler divisor fix.
