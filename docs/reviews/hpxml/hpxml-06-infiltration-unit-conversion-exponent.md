# Infiltration ACHnatural/CFMnatural conversion uses hardcoded exponent
**Review ID**: hpxml-06
**Category**: hpxml
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-io/src/hpxml/building.rs` (lines 2174–2331: `parse_air_leakage_cfm50`, `parse_air_leakage_ach50`; lines 225–235: `Building` struct fields; lines 5207–5297: CFMnatural/ACHnatural tests)
- `crates/hares-physics/src/infiltration.rs` (lines 54–81: `ach_nat_to_ach50`, `NATURAL_TO_50PA_EXPONENT`)

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/utils/envelope.py` (line 490: assert-only-ACH; lines 533–534: `n_i=0.65`)
- `vendors/OCHRE/ochre/utils/hpxml.py` (lines 502–519: `parse_indoor_infiltration`)

## Findings

### Finding 1: [Severity: low] — Function name `ach_nat_to_ach50` is misleading when applied to CFM

**Description**: The function `ach_nat_to_ach50()` is used for both ACHnatural → ACH50 and CFMnatural → CFM50 conversions (building.rs:2205, 2234, 2284, 2315). Its name implies ACH-only applicability, but the underlying power-law formula applies to any volumetric flow rate unit equally.

**Code Location**:
- `crates/hares-physics/src/infiltration.rs:79`: function definition with ACH-specific name
- `crates/hares-io/src/hpxml/building.rs:2205`: `ach_nat_to_ach50(v, NATURAL_TO_50PA_EXPONENT)` called with a CFM value
- `crates/hares-io/src/hpxml/building.rs:2234`: same call for HPXML 4.x inline form
- `crates/hares-io/src/hpxml/building.rs:2178–2179`: doc comment acknowledges the naming mismatch: "(the same power-law conversion applies to CFM rates; it depends only on the pressure ratio and flow exponent)"

**Root Cause**: The function was designed with ACH in mind (matching OCHRE's `ach` parameter naming), but the power law `Q_50 = Q_nat × (50/4)^n` is unit-agnostic — it applies directly to any flow unit (CFM, ACH, m³/s, L/s) because multiplication by volume is commutative across the conversion chain. The function parameter is correctly named `q_nat` (generic), but the function name itself implies ACH-only usage.

**Impact**: No numerical error. The conversion is mathematically correct regardless of flow-rate unit. The name may confuse future maintainers who assume CFMnatural conversion requires volume normalisation first (it does not — converting CFMnatural → ACHnatural → ACH50 → CFM50 through volume is algebraically identical to direct power-law application on CFM). Consider renaming to `flow_nat_to_flow50` or adding a type-level distinction.

---

### Finding 2: [Severity: low] — Natural-to-50Pa exponent is hardcoded at parse time with no user override

**Description**: The conversion from ACHnatural/CFMnatural to 50 Pa equivalents uses `NATURAL_TO_50PA_EXPONENT` (0.65) as a hardcoded constant at HPXML parse time (building.rs:2205, 2234, 2284, 2315). The runtime AIM-2 infiltration model (`Aim2Params::n_i`) allows configurable values in [0.5, 0.7], but this parse-time conversion is locked to 0.65 with no mechanism to propagate a user-specified or HPXML-provided exponent.

**Code Location**:
- `crates/hares-physics/src/infiltration.rs:58`: `pub const NATURAL_TO_50PA_EXPONENT: f64 = N_I_DEFAULT;` (constant 0.65)
- `crates/hares-io/src/hpxml/building.rs:2205, 2234, 2284, 2315`: four call sites, all use `NATURAL_TO_50PA_EXPONENT` with no user-override path
- `crates/hares-core/src/dwelling/solver_builder.rs:948`: ELA→CFM conversion also uses `N_I_DEFAULT` hardcoded (separate from parse-time conversion)

**Root Cause**: The `Building` struct stores only the converted `infiltration_ach50` / `infiltration_cfm50` values, not the raw natural values alongside a configured exponent. An HPXML `<extension>` could carry a custom `n` exponent, but the current parse pipeline has no field for it.

**Impact**: Users who know their building's actual flow exponent from a multi-point blower-door test cannot use it for the natural-to-50Pa conversion. The hardcoded 0.65 is a reasonable default per ASHRAE 119 / ASTM E779 for typical residential construction, so the practical error is bounded. Typical worst-case: using 0.65 instead of 0.50 introduces ~15% error in the converted value (ratio: `12.5^0.5 / 12.5^0.65 = 3.536 / 5.164 = 0.685`).

---

### Finding 3: [Severity: informational] — OCHRE does not handle CFMnatural or ACHnatural at all

**Description**: OCHRE's `calculate_ashrae_infiltration_params` in `envelope.py:490` contains an assertion: `assert indoor_inf["BuildingAirLeakage"]["UnitofMeasure"] in ["ACH"]`. It rejects both `ACHnatural` and `CFMnatural` inputs entirely. HARES provides CFMnatural/ACHnatural parsing as additional functionality beyond the OCHRE baseline. There is no vendor reference implementation to compare the CFMnatural conversion against.

**Code Location**:
- `vendors/OCHRE/ochre/utils/envelope.py:490`: `assert ... in ["ACH"]  # only allow ACH50, not ACHnatural`
- `crates/hares-io/src/hpxml/building.rs:2174–2250`: HARES `parse_air_leakage_cfm50` — handles CFM, CFM50, and CFMnatural
- `crates/hares-io/src/hpxml/building.rs:2255–2331`: HARES `parse_air_leakage_ach50` — handles ACH, ACH50, and ACHnatural

**Impact**: HARES correctly accepts a wider range of HPXML inputs than OCHRE. The conversion approach is physics-based and self-consistent. Verification relies on the internal unit tests at building.rs:5207–5297, which confirm the expected numerical output.

---

### Verification: Mathematical correctness of direct power-law CFM conversion

The concern raised — that CFMnatural should be normalised by volume to ACH before applying the pressure exponent — is not a mathematical error. The power-law relationship:

```
Q = C × ΔP^n
```

applies directly to volumetric flow rate Q regardless of whether flow is expressed in CFM, ACH, m³/s, etc. Converting through ACH as an intermediate step is algebraically equivalent:

```
CFM50 = volume × ACH50 / 60
      = volume × [ACHnat × (50/4)^n] / 60
      = [volume × ACHnat / 60] × (50/4)^n
      = CFMnat × (50/4)^n
```

The volume factor cancels. The HARES approach (direct power-law on CFM) produces identical results to a hypothetical ACH-intermediated path, with the advantage of not requiring a known volume at parse time.

## Summary
- **Total findings**: 3
- **Critical**: 0
- **High**: 0
- **Medium**: 0
- **Low**: 2
- **Informational**: 1

## Recommendations

1. **Rename `ach_nat_to_ach50`** to a unit-agnostic name (e.g., `flow_nat_to_flow50` or `pressure_convert_flow_natural_to_50pa`) to avoid confusion when the function is applied to CFM values. The existing alias pattern (`ach_nat_to_ach50` → delegate to renamed function) could preserve backward compatibility.

2. **Consider making the natural-to-50Pa exponent configurable** by storing user-specified `n` values in the `Building` struct, perhaps reading from an HPXML `<extension>` field. Fall back to `NATURAL_TO_50PA_EXPONENT` (0.65) when unspecified.

3. **Document the conversion physics** prominently near the `parse_air_leakage_cfm50` function (building.rs:2177–2179) explaining why volume normalisation is not required, since the question arises naturally from the function name.

## References / Citations

- ASHRAE Standard 119-2015: "Air Leakage Performance for Detached Single-Family Residential Buildings" — Section 5.3, power-law flow exponent default n = 0.65 for residential construction.
- ASTM E779-19: "Standard Test Method for Determining Air Leakage Rate by Fan Pressurization" — power-law model Q = C × ΔP^n.
- Walker, I.S. & Wilson, D.J. (1998): "Field Validation of Algebraic Equations for Stack and Wind Driven Air Infiltration Calculations," *HVAC&R Research* 4(2). — Flow exponent origin for AIM-2 model.
- OCHRE source: `vendors/OCHRE/ochre/utils/envelope.py:488–605` — only accepts ACH50; hardcodes `n_i = 0.65` at line 534.
