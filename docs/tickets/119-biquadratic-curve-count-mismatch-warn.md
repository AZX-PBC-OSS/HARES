# Mismatched Biquadratic Cap/EIR Curve Counts Should Warn, Not Silent-Fill

**Severity**: Low
**Priority**: P3
**Status**: Open
**Areas**: hares-equipment/hvac/hvac_core

## Problem

`crates/hares-equipment/src/hvac/hvac_core.rs:427-434` silently fills mismatched biquadratic capacity vs EIR curve counts with unity polynomials. If a multi-speed unit has, say, 4 capacity curves and 3 EIR curves, the resolver pads the missing EIR curve with `[1, 0, 0, 0, 0, 0]` (the identity biquadratic). The unit operates as if EIR is constant 1.0 at one speed step — physically impossible for any real heat pump or AC.

## Current Behavior

`crates/hares-equipment/src/hvac/hvac_core.rs:427-434`:
```rust
while eir_curves.len() < cap_curves.len() {
    eir_curves.push(IDENTITY_BIQUADRATIC);  // silent fill
}
```

No warning. The resulting unit silently has wrong EIR for one or more speed steps.

## Required Behavior

1. Emit `tracing::warn!` listing the equipment ID, the count mismatch, and the speed step(s) being filled with identity curves.
2. Optionally elevate to `tracing::error!` if the user has opted into strict mode.
3. Document the silent-fill behaviour in the function doc comment so it is discoverable.
4. If the typed config exposes a strict-mode flag, default to error rather than warn for new users.

## Approach

1. Open `crates/hares-equipment/src/hvac/hvac_core.rs:427-434` and add `tracing::warn!` immediately before the fill loop.
2. Include the equipment ID, the cap/EIR count mismatch, and the specific speed steps being filled.
3. Add a doc comment on the surrounding function describing the silent-fill behaviour.
4. Add a unit test asserting the warning fires when curve counts mismatch.
5. Consider whether to elevate to error — discuss in the ticket with the team.

## Definition of Done

- [ ] `tracing::warn!` emitted on mismatched curve counts
- [ ] Log includes equipment ID, count mismatch, and speed step indices
- [ ] Function doc comment describes the silent-fill behaviour
- [ ] Unit test asserts the warning fires
- [ ] No `_ =>` catch-all elsewhere in the curve-loading code

## Verification

```bash
cargo test -p hares-equipment hvac_core
```

## References

- AHRI Standard 210/240-2023 — multi-speed heat pumps have one capacity curve and one EIR curve per speed step; counts must match.
- EnergyPlus I/O Reference `Coil:Cooling:DX:MultiSpeed` — distinct capacity and EIR curves per stage; identity is invalid for real equipment.
- Project policy `feedback_no_silent_defaults.md`.

## Related Tickets

- 002-ideal-hvac-biquadratic-fallback
- 003-tighten-biquadratic-default-bounds
- 010-default-biquadratic-performance-curves

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] **Referenced line numbers partially match (corrected location noted below)**
  - The ticket cites `hvac_core.rs:427-434` as a `while eir_curves.len() < cap_curves.len()` loop with `eir_curves.push(IDENTITY_BIQUADRATIC)`. The current code at those lines uses a `for i in 0..n_stages` pattern instead, with `.get(i).copied().unwrap_or(DEFAULT_BIQUADRATIC_COEFFS)` on both cap and eir. The semantic behavior — silently filling the shorter list with identity — is identical. The loop form changed (a `for` replaces `while`/`push`), but the line range (426-437) is accurate and the bug is present.
  - Corrected: fill logic is at lines 426-437, not 427-434. `IDENTITY_BIQUADRATIC` in the ticket corresponds to `DEFAULT_BIQUADRATIC_COEFFS` (`[1.0, 0.0, 0.0, 0.0, 0.0, 0.0]`) in the code.

- [x] **Described logic matches current implementation**
  - `hvac_core.rs:424`: `let n_stages = cap_curves.len().max(eir_curves.len());`
  - `hvac_core.rs:430`: `.unwrap_or(DEFAULT_BIQUADRATIC_COEFFS)` silently fills missing cap entries.
  - `hvac_core.rs:434`: `.unwrap_or(DEFAULT_BIQUADRATIC_COEFFS)` silently fills missing eir entries.
  - No `tracing::warn!` or `tracing::error!` is emitted. Bug is present and unaddressed.

- [x] **OCHRE cross-check result: DIVERGES — OCHRE raises a hard error; HARES silently fills**
  - OCHRE `DynamicHVAC.initialize_biquad_params()` at `vendors/OCHRE/ochre/Equipment/HVAC.py:814-818`:
    ```python
    if len(biquad_params.columns) != self.n_speeds:
        raise OCHREException(
            f"Number of speeds ({self.n_speeds}) does not match number of biquadratic "
            f"equations ({len(biquad_params.columns)})"
        )
    ```
  - Additionally, the base `HVAC.__init__()` at `HVAC.py:158-163` raises on mismatched list lengths:
    ```python
    for speed_list in [self.capacity_list, self.eir_list, self.fan_power_list]:
        if len(speed_list) - 1 != self.n_speeds:
            raise OCHREException(
                f"Number of speeds ({self.n_speeds}) does not match length of list ({len(speed_list) - 1})"
            )
    ```
  - OCHRE treats a cap/EIR curve count mismatch as a **fatal error** (raises exception), never silently fills. HARES silently fills. The divergence is **accidental** — not intentional.

- [x] **EnergyPlus cross-check result: DIVERGES — EnergyPlus defines cap and EIR as required per-speed fields; silent identity fill is not a valid EnergyPlus concept**
  - Source: EnergyPlus V8-7-0 IDD (`vendors/EnergyPlus/idd/versions/V8-7-0-Energy+.idd`, lines 43865-43888), read directly from the vendored IDD file.
  - For `Coil:Cooling:DX:MultiSpeed` Speed 1:
    - `A13, \field Speed 1 Total Cooling Capacity Function of Temperature Curve Name` — `\required-field`; `\object-list BiquadraticCurves`; `curve = a + b*wb + c*wb**2 + d*edb + e*edb**2 + f*wb*edb`
    - `A15, \field Speed 1 Energy Input Ratio Function of Temperature Curve Name` — `\required-field`; `\object-list BiquadraticCurves`
  - For Speed 2 (IDD lines 44021, 44037):
    - `A19, \field Speed 2 Total Cooling Capacity Function of Temperature Curve Name` — `\required-field`
    - `A21, \field Speed 2 Energy Input Ratio Function of Temperature Curve Name` — `\required-field`
  - For Speed 3 (IDD lines 44174, 44188): `A25` (cap) and `A27` (EIR) — **not marked `\required-field`** (optional, because Speed 3 and 4 are only relevant if `Number of Speeds >= 3`). This implies EnergyPlus enforces that the number of speeds declared equals the number of curve sets specified — no identity fill is permitted.
  - EnergyPlus requires exactly one cap biquadratic and one EIR biquadratic **per declared speed stage**. An input with 4 cap curves and 3 EIR curves for a 4-speed coil would be a schema validation error in EnergyPlus. There is no identity-fill fallback in the EnergyPlus model.

### Web-Verified Citations

**Citation 1: AHRI Standard 210/240-2023 — multi-speed heat pumps have one capacity curve and one EIR curve per speed step; counts must match**
- **Source found**: EnergyPlus V8-7-0 IDD file at `vendors/EnergyPlus/idd/versions/V8-7-0-Energy+.idd:43865–44044` (read directly); AHRI 210/240-2023 PDF blocked (HTTP 403). Web search results from `ahrinet.org`, `basc.pnnl.gov`, `federalregister.gov`.
- **Quoted passage** (from EnergyPlus IDD, the simulation standard encoding the AHRI physics model):
  > `A13, \field Speed 1 Total Cooling Capacity Function of Temperature Curve Name` → `\required-field` → `\object-list BiquadraticCurves`
  > `A15, \field Speed 1 Energy Input Ratio Function of Temperature Curve Name` → `\required-field` → `\object-list BiquadraticCurves`
  > `A19, \field Speed 2 Total Cooling Capacity Function of Temperature Curve Name` → `\required-field`
  > `A21, \field Speed 2 Energy Input Ratio Function of Temperature Curve Name` → `\required-field`

  From web search results regarding AHRI 210/240-2023: the standard defines test conditions at each speed stage (AFull/BFull for full speed, BLow/FLow/GLow/ILow for minimum speed), establishing that both capacity and efficiency (COP/EIR) must be characterised per speed step. The standard does not define biquadratic curves directly; those are the EnergyPlus simulation encoding of the AHRI test data.
- **Verdict**: **Partially confirmed**. The AHRI 210/240-2023 PDF could not be directly fetched (HTTP 403). The claim that multi-speed heat pumps require one capacity test and one EIR test per speed step is physically self-evident and reflected in the EnergyPlus IDD (one required cap and one required EIR biquadratic per speed). The specific claim that "counts must match" is confirmed by EnergyPlus IDD required-field annotations and by OCHRE's hard error on count mismatches. Direct AHRI PDF confirmation was not possible.

**Citation 2: EnergyPlus I/O Reference `Coil:Cooling:DX:MultiSpeed` — distinct capacity and EIR curves per stage; identity is invalid for real equipment**
- **Source found**: `vendors/EnergyPlus/idd/versions/V8-7-0-Energy+.idd` (read directly from vendored file); BigLadder Software EnergyPlus documentation pages (multiple versions fetched — all redirect to the new `Coil:Cooling:DX` structure which replaced MultiSpeed in 9.3+). The vendored IDD file is the authoritative source.
- **Quoted passage**:
  - For Speed 1 EIR (IDD line 43881): `A15, \field Speed 1 Energy Input Ratio Function of Temperature Curve Name` → `\required-field` → `\object-list BiquadraticCurves` → `\note curve = a + b*wb + c*wb**2 + d*edb + e*edb**2 + f*wb*edb`
  - Each declared speed requires its own cap and EIR biquadratic. No identity-fill fallback exists in the EnergyPlus schema.
- **Verdict**: **Confirmed**. The IDD establishes that each speed requires a distinct cap biquadratic and a distinct EIR biquadratic. Using identity `[1,0,0,0,0,0]` for a missing EIR curve produces physically impossible constant-EIR behaviour and is not a valid EnergyPlus input pattern.

**Citation 3: Project policy `feedback_no_silent_defaults.md`**
- **Source found**: File not found at any path under `docs/` or repo root (`Glob` for `**/feedback_no_silent_defaults*` returned no results). The policy reference appears to be a naming convention, not an actual file.
- **Verdict**: **Cannot verify** — the cited file does not exist. The ticket's underlying policy intent (warn/error instead of silently filling unphysical defaults) is validated by the OCHRE and EnergyPlus evidence and by the project's broader pattern (see ticket 093, 102, 103 which all mandate loud errors for silent defaults). The missing file does not undermine the ticket's legitimacy.

### Legitimacy

- **Verdict**: **Legitimate**

- **Rationale**: The bug is real and confirmed by direct code inspection. `hvac_core.rs:424-438` computes `n_stages = cap_curves.len().max(eir_curves.len())` and then silently fills any missing cap or EIR entry with `DEFAULT_BIQUADRATIC_COEFFS` (`[1.0, 0.0, 0.0, 0.0, 0.0, 0.0]`) via `unwrap_or`. No `tracing::warn!` is emitted. The EnergyPlus IDD (vendored at `V8-7-0-Energy+.idd:43865-44044`) marks `Speed N Total Cooling Capacity Function of Temperature Curve Name` and `Speed N Energy Input Ratio Function of Temperature Curve Name` as `\required-field` for each declared speed stage, making it clear that distinct cap and EIR biquadratic curves are mandatory per speed — there is no identity-fill concept in EnergyPlus. OCHRE raises a hard exception on any count mismatch (`HVAC.py:814-818`). The cited line numbers are off by 1-2 lines (fill logic at 426-437, not 427-434) and the constant name is `DEFAULT_BIQUADRATIC_COEFFS` rather than `IDENTITY_BIQUADRATIC`, but these are minor naming differences. The policy reference (`feedback_no_silent_defaults.md`) does not exist as a file, but the principle is consistently applied across the project. One minor note: the fill applies bidirectionally (missing cap is also filled with identity), not just missing EIR — the ticket only calls out the EIR case; the implementation fills both. This does not change the legitimacy of the core complaint.

### Proposed Fix Summary

1. Before the `for i in 0..n_stages` loop at `hvac_core.rs:426`, add a count check:
   ```rust
   if cap_curves.len() != eir_curves.len() {
       tracing::warn!(
           cap_count = cap_curves.len(),
           eir_count = eir_curves.len(),
           "biquadratic curve count mismatch for {:?}: {} capacity curves vs {} EIR curves; \
            missing curves filled with identity [1,0,0,0,0,0] — speed stages {:?} will have \
            physically incorrect EIR",
           self.equipment_type,
           cap_curves.len(),
           eir_curves.len(),
           (cap_curves.len().min(eir_curves.len())..n_stages).collect::<Vec<_>>(),
       );
   }
   ```
2. Add a doc comment on the surrounding block (lines 409-440) describing the silent-fill behaviour.
3. Optionally: if a strict-mode flag exists in the config, elevate to `tracing::error!`.
4. Do NOT change the fill behaviour itself (that is a separate decision for the team).

**Do NOT implement this fix** — this audit section only.

### Test Written

- **File**: `crates/hares-equipment/src/hvac/hvac_core.rs` (in-module `#[cfg(test)]` block, after the existing ticket 010 tests at line 3207)
- **Test name**: `ticket_119_mismatched_curve_counts_silently_filled_with_identity`
- **What it tests**: Configures an `AshpHeatPumpOnly` with 2 capacity biquadratic curves and 1 EIR biquadratic curve. After `init()`, asserts:
  1. `biquadratic_coeffs.len() == 4` (2 speeds × 2 curves interleaved)
  2. Speed-0 cap and EIR entries match the explicitly provided coefficients
  3. Speed-1 cap entry matches the explicitly provided second cap curve
  4. Speed-1 EIR entry equals `DEFAULT_BIQUADRATIC_COEFFS` (identity), confirming the silent fill
- **Current status**: Test passes (confirming the bug exists — silent fill occurs without warning). After the fix, the warning should be observable via a tracing subscriber in an enhanced version of this test.
