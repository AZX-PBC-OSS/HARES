# Rename `port_sensible_w` to `port_convective_w` for Naming Consistency

**Severity**: Nit
**Priority**: P4
**Status**: Open
**Areas**: hares-envelope, hares-core, hares-python

## Problem

The port-input bookkeeping uses `port_sensible_w` for the convective-only portion of equipment heat injection and `internal_gain_w` for the full sensible total (convective + radiant). The names are asymmetric and misleading: a reader reasonably expects `port_sensible_w` to be the *sensible* total, not the *convective* portion of it. This creates a footgun when comparing port telemetry against the dwelling-level sensible balance — any user adding `port_sensible_w` and `port_radiant_w` together expecting "sensible total" will double-count, because `port_sensible_w` is already the convective part only.

## Current Behavior

Across the workspace:
- `port_sensible_w` is the convective-only port input — see `crates/hares-envelope/src/thermal_solver/ports.rs` and `crates/hares-python/src/py_dwelling.rs`.
- `port_radiant_w` is the radiant port input.
- `internal_gain_w` is the full sensible total (convective + radiant) used in zone-level energy bookkeeping.

The `_sensible_` token in `port_sensible_w` overlaps semantically with the `_sensible_` interpretation used in `internal_gain_w` and elsewhere in the codebase.

## Required Behavior

Rename `port_sensible_w` to `port_convective_w` everywhere:

1. Rust struct fields, function names, and local bindings.
2. Python-binding dict keys (`post_solvers["port_sensible_w"]` → `post_solvers["port_convective_w"]`).
3. Diagnostic CSV column names.
4. Test assertions that reference the old name.
5. Documentation comments that reference the old name.

This is a hard rename — no alias, no backward-compat shim (per project memory `feedback_no_backward_compat` — greenfield codebase).

## Approach

1. `grep` workspace for `port_sensible_w` and enumerate every callsite.
2. Run an automated rename using the Edit tool's `replace_all` parameter against each file.
3. Update Python binding tests and CSV-output snapshot tests.
4. Update any markdown documentation that references the old name.
5. Verify the workspace builds and all tests pass after rename.
6. Coordinate with ticket 108 (port_radiant_w Python and Diag exposure) to ensure the new symmetric naming (`port_convective_w` + `port_radiant_w`) appears together in the Python dict and the diagnostic CSV.

## Definition of Done

- [ ] `port_sensible_w` renamed to `port_convective_w` in all Rust source and tests
- [ ] Python binding dict keys updated
- [ ] Diagnostic CSV column header updated
- [ ] No remaining references to `port_sensible_w` in the workspace (except possibly migration notes in CHANGELOG / release notes)
- [ ] All tests pass after rename
- [ ] Doc comments distinguish convective port (`port_convective_w`), radiant port (`port_radiant_w`), and the full sensible total (`internal_gain_w`)

## Verification

```bash
cargo build --workspace
cargo test --workspace
rg port_sensible_w crates/
```

## References

- ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.1 "Internal Heat Gain Components" — defines sensible heat as the sum of convective and radiant components; "convective" and "radiant" are the canonical sub-components, not "sensible" and "radiant".
- EnergyPlus Engineering Reference §3.5 "Inside Surface Heat Balance" — uses `q_conv` and `q_rad` (convection / radiation) as the explicit sub-components, never `q_sensible` for the convective part alone.

## Related Tickets

- 108-port-radiant-w-python-and-diag-exposure (paired exposure work; coordinate naming)

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] **Referenced file `crates/hares-envelope/src/thermal_solver/ports.rs`** — file exists at the cited path; however the ticket conflates two files. `ports.rs` contains the port-application *logic* (`apply_port_sensible_inputs`, `apply_port_radiant_inputs`) but the struct field `port_sensible_w: f64` is declared in `crates/hares-envelope/src/thermal_solver/config.rs:507` (inside `EnvelopeComponentGains`). The second citation (`crates/hares-python/src/py_dwelling.rs`) is correct: line 1939 is `d.set_item("port_sensible_w", gains.port_sensible_w)?;`.
- [x] **Described logic matches current implementation** — confirmed. `port_sensible_w` is populated at `mod.rs:584` as `port_sensible_indoor_w` (the convective-only indoor-zone sum from equipment ports). `port_radiant_w` is the companion radiant sum. The doc-comment on `config.rs:505–507` explicitly reads: *"Total convective sensible gains from all equipment ports (HVAC + appliances) [W]. Only the convective portion that goes directly to zone air."* The name vs. the comment are in tension — the name says "sensible", the comment says "convective only".
- [x] **Bug still present** — `port_sensible_w` name unrenamed as of HEAD. No alias or backward-compat shim exists. The field also appears in `hares-core/src/diagnostics.rs:40` (`EnvelopeDiag`) and at `hares-python/src/py_dwelling.rs:1939`. Total callsite count from workspace grep: 8 distinct locations across Rust source, tests, Python tests, documentation, and BESTEST runner.
- [x] **OCHRE cross-check** — OCHRE explicitly names the convective portion `"Convective Gain Fraction (-)"` in its parameter dictionaries (e.g. `vendors/OCHRE/ochre/utils/hpxml.py:1566`). OCHRE's `Equipment.py:83–85` computes `sensible_gain_fraction = Convective_Gain_Fraction + Radiative_Gain_Fraction`, confirming that in OCHRE terminology "sensible" means convective + radiant, never convective alone. HARES's `port_sensible_w` (convective only) contradicts OCHRE's convention. The divergence is accidental — OCHRE never uses "sensible" for the convective sub-component alone.
- [x] **EnergyPlus cross-check** — The EnergyPlus Engineering Reference "Zone Internal Gains" section (confirmed via WebFetch of `bigladdersoftware.com/epx/docs/25-2/engineering-reference/zone-internal-gains.html` and the 8.9 version) states: *"The total heat gain is comprised of convective, radiant and latent gains in various proportions from these sources."* and *"Convective gains are instantaneous additions of heat to the zone air."* EnergyPlus consistently uses `QConv` (not `QSensible`) for the convective sub-component and never labels the convective-only portion "sensible." The ticket's EnergyPlus citation is directionally correct.

---

### Web-Verified Citations

**Citation 1 — ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.1 "Internal Heat Gain Components"**

- **Source found**: ASHRAE.org Table of Contents 2021 ASHRAE Handbook—Fundamentals (`https://www.ashrae.org/technical-resources/ashrae-handbook/table-of-contents-2021-ashrae-handbook-fundamentals`)
- **Quoted passage**: Chapter 18 is titled "Nonresidential Cooling and Heating Load Calculations." No sub-section titled "§18.1 Internal Heat Gain Components" is visible in the public-facing table of contents. The section number "§18.1" and heading "Internal Heat Gain Components" could not be independently verified from publicly accessible sources.
- **Verdict**: **Partially correct.** The substantive claim — that ASHRAE defines sensible heat as convective + radiant, with convective and radiant as the canonical sub-components — is well-supported by multiple ASHRAE-aligned sources and EnergyPlus documentation that implements ASHRAE methods. However, the exact section reference "Ch. 18 §18.1 'Internal Heat Gain Components'" cannot be confirmed from public sources; the ASHRAE handbook requires a paid subscription to read in full.

**Citation 2 — EnergyPlus Engineering Reference §3.5 "Inside Surface Heat Balance"**

- **Source found**: Big Ladder Software EnergyPlus Engineering Reference "Inside Heat Balance" (various versions: `https://bigladdersoftware.com/epx/docs/24-1/engineering-reference/inside-heat-balance.html`; also confirmed `zone-internal-gains.html`)
- **Quoted passage**: From the "Inside Heat Balance" section: *"q′′LWX + q′′SW + q′′LWS + q′′ki + q′′sol + q′′conv = 0"* where `q′′conv` is "Convective heat flux to zone air." From "Zone Internal Gains": *"Convective gains are instantaneous additions of heat to the zone air. Radiant gains are distributed on the surfaces of the zone."* and *"QConv = Convective heat gain rate to zone heat balance."*
- **Verdict**: **Partially correct.** The underlying claim — that EnergyPlus uses `q_conv` and `q_rad` as the explicit sub-components, never using `q_sensible` for the convective part alone — is confirmed by the source. However, the section number "§3.5" could not be verified: the EnergyPlus Engineering Reference page titles its section "Inside Heat Balance" with no visible numeric designation matching "§3.5." The corresponding "Zone Internal Gains" content is a separate page. The spirit of the citation is accurate; the section number is unverifiable.

---

### Legitimacy

- **Verdict**: **Legitimate**
- **Rationale**: The naming bug is real and currently present in HEAD. `port_sensible_w` stores the convective-only portion of equipment port heat, while every other use of "sensible" in the codebase (`internal_gain_w`, `combined_airflow_sensible_w`, OCHRE's `sensible_gain_fraction`, EnergyPlus's treatment) means convective + radiant. This creates a genuine footgun: `port_sensible_w + port_radiant_w` does give the correct full-sensible total from ports, but a reader who assumes `port_sensible_w` already IS the sensible total (the natural reading of the name) would double-count the radiant component. The OCHRE cross-check confirms divergence from established naming convention. The EnergyPlus cross-check confirms that `q_conv` (not `q_sensible`) is the standard label for the convective sub-component. Both ticket citations are directionally correct; only the precise section numbers are unverifiable without paid access to the standards.

---

### Proposed Fix Summary

Rename the field `port_sensible_w` to `port_convective_w` in all locations:
1. `crates/hares-envelope/src/thermal_solver/config.rs:507` — struct field in `EnvelopeComponentGains`
2. `crates/hares-envelope/src/thermal_solver/mod.rs:584` — struct literal initialiser
3. `crates/hares-core/src/diagnostics.rs:40` — field in `EnvelopeDiag`
4. `crates/hares-python/src/py_dwelling.rs:1939` — Python dict key
5. `tests/bestest/mod.rs:318,325` — diagnostic observer format string and field reference
6. `tests/python/test_observer_diagnostics.py:233` and `tests/python/parity_diagnostics.py:123` — Python test key assertions
7. Any doc-comment text that spells out the old name

No backward-compat alias should be added (per project convention). Update ticket #108 coordinate comment to use new name.

---

### Test Written

- **File**: `crates/hares-core/tests/ticket_127_port_sensible_rename.rs`
- **What it tests**: Documents the invariant that `port_sensible_w` holds the convective-only component and that `port_sensible_w + port_radiant_w` is the correct full sensible total. Demonstrates the double-counting footgun the ticket describes. The test will need its field references updated when the rename is completed, serving as an acceptance signal that the rename was applied to `EnvelopeComponentGains`.
