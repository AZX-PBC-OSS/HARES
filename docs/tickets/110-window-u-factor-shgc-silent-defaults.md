# Window U-Factor and SHGC Silent Defaults Inject Single-Pane Aluminum Performance

**Severity**: Low
**Priority**: P3
**Status**: Open
**Areas**: hares-core/dwelling, hares-envelope

## Problem

`crates/hares-core/src/dwelling/solver_builder.rs:264-265` uses `win.u_factor_w_m2_k.unwrap_or(5.0)` and `win.shgc.unwrap_or(0.4)` to silently inject window thermal performance defaults when the input schema does not supply them. The values 5.0 W/(m²·K) and 0.4 correspond to single-pane aluminum-framed windows from the 1970s — they are catastrophically worse than any modern double- or triple-glazed window with thermal-break frames. Window thermal performance is one of the largest envelope load drivers in residential construction (typically 25-40% of envelope conductance even though windows are only ~15% of envelope area), so a silent default here produces large unannounced bias in heating and cooling load predictions.

Additionally, this violates `feedback_no_silent_defaults`: missing or invalid input must error loudly, not be silently substituted.

## Current Behavior

`crates/hares-core/src/dwelling/solver_builder.rs:264-265`:
```rust
let u_factor = win.u_factor_w_m2_k.unwrap_or(5.0);
let shgc = win.shgc.unwrap_or(0.4);
```

When either field is `None`:
- U-factor defaults to 5.0 W/(m²·K) — single-pane aluminum-framed; modern code-compliant double-glazed values are 1.4-2.0 W/(m²·K)
- SHGC defaults to 0.4 — typical low-e double-glazed; consistent only by coincidence with modern construction
- No `tracing::warn!`, no error, no diagnostic

A user supplying an HPXML or TOML configuration that omits these fields gets a building modelled with windows ~3× worse than what they likely have, with no indication anything went wrong.

## Required Behavior

1. Both `u_factor_w_m2_k` and `shgc` must error loudly at construction time when absent. Preferred: `DwellingError::MissingWindowProperty { window_id, property: &'static str }`.
2. If a default is desired for synthetic test fixtures, it must be supplied at the input layer (TOML/HPXML resolver) with a tracing log line explaining what default was applied and why, rather than at the solver builder.
3. The HPXML resolver path should already supply NFRC-rated values from `<UFactor>` and `<SHGC>` HPXML elements; verify those paths set the field and never emit `None` for windows that exist in the HPXML.
4. The TOML config path should require these fields in the schema (no `Option<>` wrapper).

## Approach

1. Audit `win.u_factor_w_m2_k` and `win.shgc` field types — change from `Option<f64>` to `f64` if the upstream schema can be tightened.
2. If the field must remain `Option<f64>` (e.g. for partial / overrideable configs), add construction-time validation that rejects `None` with a loud error.
3. Update the HPXML resolver to set the field unconditionally from `<UFactor>` and `<SHGC>` elements; if the HPXML omits them, raise `HpxmlError::MissingRequiredField` with a citation to HPXML spec §6.5 "Windows".
4. Update the TOML schema to require the fields; remove the silent default.
5. Add a unit test asserting a config with absent window fields fails to construct.

## Definition of Done

- [ ] `unwrap_or(5.0)` and `unwrap_or(0.4)` removed from `solver_builder.rs:264-265`
- [ ] Window u-factor and SHGC are required fields at the dwelling-construction layer
- [ ] HPXML resolver errors loudly if `<UFactor>` or `<SHGC>` are missing for any `<Window>` element
- [ ] TOML config schema requires the fields (or errors loudly if absent)
- [ ] Unit test asserts construction failure when fields are absent
- [ ] Existing fixtures that relied on the silent defaults are updated to supply explicit values

## Verification

```bash
cargo test -p hares-core dwelling
cargo test -p hares-io hpxml
cargo test -p hares-envelope thermal_solver
```

## References

- NFRC 100-2020 *Procedure for Determining Fenestration Product U-Factors* — defines the U-factor measurement method that HPXML `<UFactor>` cites.
- NFRC 200-2020 *Procedure for Determining Fenestration Product Solar Heat Gain Coefficient and Visible Transmittance* — SHGC measurement method.
- ASHRAE Handbook of Fundamentals 2021 Ch. 15 *Fenestration*, Table 4 — typical residential window U-factor and SHGC ranges by glazing type.
- HPXML Specification v4.x §6.5 "Windows" — `UFactor` and `SHGC` element definitions.
- IECC 2021 §R402.4 — code-mandated maximum U-factors by climate zone (range 0.30-0.40 Btu/(h·ft²·°F) ≈ 1.7-2.3 W/(m²·K)).

## Related Tickets

- 036-window-exterior-film-hardcoded-zero (related window thermal model issue)
- 049-window-solar-shgc-vs-transmittance-absorbed-inward (SHGC interpretation in solver)
- 102-thermal-solver-init-indoor-zone-loud-error (same loud-error pattern)
- 114-scheduled-load-sensible-fraction-loud-error (same loud-error pattern)

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match — `solver_builder.rs:264-265` confirmed present:
  ```rust
  let u_factor = win.u_factor_w_m2_k.unwrap_or(5.0);   // line 264
  let base_shgc = win.shgc.unwrap_or(0.4);              // line 265
  ```
- [x] Described logic matches current implementation — both `unwrap_or` calls are active with the values stated in the ticket.
- [x] The struct field types are confirmed `Option<f64>` at `crates/hares-io/src/hpxml/building.rs:137-138`:
  ```rust
  pub u_factor_w_m2_k: Option<f64>,
  pub shgc: Option<f64>,
  ```
- [x] OCHRE cross-check result: **Diverges intentionally at the parsing layer; unclear at the solver layer.**
  - OCHRE's `ochre/utils/hpxml.py:209-214` extracts UFactor and SHGC from HPXML using `bd_data.get("UFactor")` and `bd_data.get("SHGC")` — both may return `None` if the element is absent.
  - OCHRE's `ochre/utils/envelope.py:97-99` then does `window_shgc = window_data.get("SHGC (-)") * window_data.get("Shading Fraction (-)")` without guarding against `None`, which would raise a Python `TypeError` at runtime — OCHRE does not silently default either.
  - OCHRE's `extract_structure.py:209-210` uses `find_float(win, "UFactor") or 0.0` — a 0.0 fallback in a one-off extraction utility, not in the simulation path.
  - **Conclusion**: OCHRE does not silently default window U-factor or SHGC in its simulation path. HARES diverges from OCHRE by applying a silent 5.0 / 0.4 fallback in `solver_builder.rs`.
- [x] EnergyPlus cross-check result: **N/A for this specific bug.** The ticket is about silent input defaults (an input-validation concern), not an EnergyPlus algorithm divergence. EnergyPlus requires window thermal properties to be specified; it has no analogous silent default.

### Web-Verified Citations

**Citation 1**: "ASHRAE Handbook of Fundamentals 2021 Ch. 15 *Fenestration*, Table 4 — typical residential window U-factor and SHGC ranges by glazing type"
- **Source found**: ASHRAE Handbook of Fundamentals 2021, Chapter 15 (SI version), Table 4 — fetched directly from `handbook.ashrae.org`
- **Quoted passage** (Table 4, Single Glazing section, ID #1 — 3.2 mm glass):
  > "Single Glazing / 1 / 3.2 mm glass / Center of Glass: 5.91 / Edge of Glass: 5.91 / Aluminum Without Thermal Break (Fixed): 6.38 W/(m²·K)"
  >
  > "Double Glazing, e = 0.20 on surface 2 or 3 / 13 mm argon space (ID #19): Center of Glass 1.70 W/(m²·K) ... Fixed, Wood/Vinyl: 1.98 W/(m²·K)"
  >
  > "Triple Glazing, e = 0.10 on surfaces 2 and 5, 13 mm argon (ID #43): Center of Glass 0.80 W/(m²·K)"
- **Verdict**: **Confirmed.** The 5.0 W/(m²·K) default is consistent with single-pane glass-only performance (Table 4 IDs 1–3 show centre-of-glass values 5.00–5.91 W/(m²·K)). Modern double-glazed windows range 1.4–3.5 W/(m²·K) depending on frame; modern triple-glazed go below 1.5 W/(m²·K). The ticket's claim that 5.0 W/(m²·K) is "catastrophically worse than any modern double- or triple-glazed window" is confirmed by ASHRAE Table 4.
- **Additional**: ASHRAE Table 4 notes state (note 8): "U-factors in this table were determined using NFRC 100-91. They have not been updated to the current rating methodology in NFRC 100 (2014a)." The values for single-pane glass remain valid reference points.

---

**Citation 2**: "NFRC 100-2020 *Procedure for Determining Fenestration Product U-Factors* — defines the U-factor measurement method that HPXML `<UFactor>` cites"
- **Source found**: ANSI/NFRC 100-2020, confirmed via Intertek's standards page and NFRC community portal at `nfrccommunity.org/page/TD`
- **Quoted passage** (from Intertek's NFRC 100 page):
  > "This procedure utilizes computer simulation software to determine overall product thermal transmittance (U-Factor), including frame and glass elements. Two-dimensional finite element software is used to calculate the U-Factors of individual elements of the window. Frame, Edge and Center-of-Glass (COG) U-Factors are calculated using the WINDOW and THERM software programs developed by Lawrence Berkeley National Laboratory (LBNL). The ANSI/NFRC 100 procedure calculates U-Factors for winter conditions of 70ºF (21ºC) interior temperature and 0ºF (-18ºC) exterior temperature."
- **Verdict**: **Confirmed.** NFRC 100 defines the whole-product U-factor measurement procedure. HPXML `<UFactor>` is specified in NFRC 100 units (Btu/(h·ft²·°F) for US-customary). This corroborates that HPXML `<UFactor>` refers to the NFRC-rated whole-product value.

---

**Citation 3**: "NFRC 200-2020 *Procedure for Determining Fenestration Product Solar Heat Gain Coefficient and Visible Transmittance* — SHGC measurement method"
- **Source found**: Referenced alongside NFRC 100 on the NFRC community portal technical documents page (`nfrccommunity.org/page/TD`).
- **Quoted passage**: Not directly fetchable as free text (standard is paywalled), but confirmed as the standard governing SHGC measurement alongside NFRC 100.
- **Verdict**: **Partially confirmed.** The existence and purpose of NFRC 200-2020 is confirmed; the specific 2020 edition number could not be independently quoted from a free source. The ticket's reference to it as the SHGC measurement method is accurate.

---

**Citation 4**: "HPXML Specification v4.x §6.5 'Windows' — `UFactor` and `SHGC` element definitions"
- **Source found**: OpenStudio-HPXML workflow inputs documentation at `openstudio-hpxml.readthedocs.io/en/latest/workflow_inputs.html` (the authoritative HPXML v4 implementation reference)
- **Quoted passage** (HPXML Windows section):
  > "Each window or glass door area is entered as a /HPXML/Building/BuildingDetails/Enclosure/Windows/Window."
  > | Element | Type | Units | Constraints | Required | Default | Notes |
  > |---|---|---|---|---|---|---|
  > | UFactor and/or GlassLayers | double or string | Btu/F-ft2-hr | > 0 or See [215] | **Yes** | — | Full-assembly NFRC U-factor or glass layers description |
  > | SHGC and/or GlassLayers | double or string | — | > 0, < 1 | **Yes** | — | Full-assembly NFRC solar heat gain coefficient or glass layers description |
  >
  > "If UFactor and SHGC are not provided and GlassLayers is not 'glass block', additional information is entered in Window [FrameType, GlassType, GasFill] ... UFactor/SHGC Lookup If UFactor and SHGC are not provided, they are defaulted as follows: single-pane, Aluminum/Metal, false, clear/reflective → UFactor: **1.27** Btu/(h·ft²·°F), SHGC: **0.75**"
- **Verdict**: **Confirmed with important nuance.** The HPXML spec requires either `UFactor`+`SHGC` or `GlassLayers`+`FrameType` (which triggers a lookup table to derive them). The spec does NOT permit a window with no thermal properties at all — HPXML requires these fields. Importantly, the HPXML lookup table's default for single-pane aluminium (no thermal break, clear glass) is **1.27 Btu/(h·ft²·°F) = 7.21 W/(m²·K)**, not 5.0 W/(m²·K). The ticket's description of the bug is accurate: when HPXML omits `<UFactor>` entirely (i.e., an invalid/incomplete HPXML), HARES silently uses 5.0 rather than rejecting the input.
- **Section reference**: The ticket cites "§6.5 Windows" — the OpenStudio-HPXML docs do not use the same section numbering as the formal HPXML schema XSD, but the element definitions are consistent with the HPXML 4.0 schema (confirmed by the `schemaVersion="4.0"` in test fixtures).

---

**Citation 5**: "IECC 2021 §R402.4 — code-mandated maximum U-factors by climate zone (range 0.30–0.40 Btu/(h·ft²·°F) ≈ 1.7–2.3 W/(m²·K))"
- **Source found**: PNNL/Building America Solution Center table at `basc.pnnl.gov/information/table-maximum-fenestration-u-factor-requirements-new-homes-listed-2009-2021-iecc-and`
- **Quoted passage** (2021 IECC column from Table 1):
  > "Climate Zone 1: NR [no requirement listed]; CZ 2: NR; CZ 3: NR; CZ 4 except Marine: 0.30; CZ 5 and Marine 4: 0.30; CZ 6: 0.30; CZ 7 and 8: 0.30"
  >
  > "Table adapted from Table 402.1.1 in the 2009 and 2012 IECC, Table R402.1.2 in the 2015, 2018 IECC, and Table 402.1.3 in the 2021 IECC."
- **Verdict**: **Partially correct.** The ticket cites "§R402.4" but the correct table is **Table 402.1.3** in the 2021 IECC (not §R402.4). Section §R402.4 in the IECC covers air leakage, not fenestration U-factors. The fenestration U-factor requirements are in **Table R402.1.2** (2015/2018 IECC) or **Table 402.1.3** (2021 IECC). The numerical values cited in the ticket (0.30–0.40 Btu/(h·ft²·°F) ≈ 1.7–2.3 W/(m²·K)) are **correct** for the IECC's range across climate zones, and the 2021 IECC mandates max 0.30 for CZ 4–8. The section number is wrong but the numerical claim is accurate.

### Legitimacy

- **Verdict**: **Legitimate** (with one minor citation correction)
- **Rationale**: The bug is real and confirmed. `crates/hares-core/src/dwelling/solver_builder.rs:264-265` uses `unwrap_or(5.0)` and `unwrap_or(0.4)` as silent defaults when a window's U-factor or SHGC is `None`. ASHRAE HoF 2021 Ch. 15 Table 4 (fetched directly) confirms that 5.0 W/(m²·K) corresponds to 1970s single-pane glass-only performance — nearly 3× worse than the IECC 2021 maximum of 1.70 W/(m²·K) for Climate Zones 4–8 (confirmed via PNNL/BASC data). The HPXML v4 specification (confirmed via openstudio-hpxml.readthedocs.io) requires UFactor or equivalent GlassLayers input — it does not permit silently defaulting this value. OCHRE does not silently default in its simulation path either. The ticket's section citation for the IECC (§R402.4) is slightly wrong — the correct reference is Table 402.1.3 in the 2021 IECC — but all numerical values are correct. The severity claim (catastrophic bias in heating/cooling load predictions) is physically justified by the ~3× gap between the default and any modern code-compliant window.

### Proposed Fix Summary

1. **`crates/hares-core/src/dwelling/solver_builder.rs:264-265`**: Replace `unwrap_or(5.0)` and `unwrap_or(0.4)` with a result that returns `DwellingError::MissingWindowProperty { window_id: win.id.clone(), property: "u_factor_w_m2_k" }` (and the analogous error for SHGC). The surrounding `win_match.map(|win| { ... })` closure must become a fallible closure and propagate the error via `?`.
2. **`crates/hares-io/src/hpxml/building.rs:960-961`**: The IO layer already correctly parses and propagates `None` — no change needed there. If the spec requires UFactor to be present, add a validation pass in `validate_building_ranges` or a dedicated `validate_window_properties` that returns `HpxmlError::MissingRequiredField` for windows with neither `<UFactor>` nor `<GlassLayers>`.
3. **Synthetic fixtures** (`crates/hares-core/src/dwelling/synthetic.rs:196-197`): `SyntheticWindowConfig` already uses `u_factor_w_m2_k: f64` (not `Option<f64>`) — so synthetic tests are unaffected by the fix.
4. **TOML fixtures** that rely on the silent default (currently none identified as missing these fields): if any exist, they must be updated to supply explicit values.

### Test Written

- **File**: `crates/hares-core/tests/ticket_110_window_u_factor_shgc_silent_defaults.rs`
- **What it tests**:
  1. `hpxml_parser_returns_none_u_factor_when_element_absent` — confirms the IO layer already correctly returns `None` (not a default) when `<UFactor>` is absent. **Passes now; must continue to pass after fix.**
  2. `hpxml_parser_returns_none_shgc_when_element_absent` — same for SHGC. **Passes now; must continue to pass after fix.**
  3. `hpxml_parser_converts_u_factor_from_imperial_to_si` — confirms that 0.30 Btu/(h·ft²·°F) → ~1.703 W/(m²·K) conversion is correct. **Passes now; must continue to pass after fix.**
  4. `current_behaviour_none_u_factor_does_not_error_at_io_layer_bug_ticket_110` — documents that the IO layer correctly produces `None` (the bug is downstream). Contains inline `TODO` showing what the solver-layer test should look like after the fix. **Passes now; must be REPLACED after fix.**
  5. `documents_unreasonable_single_pane_default_values_that_ticket_110_must_remove` — computes the 5.0 W/(m²·K) default vs IECC 2021 CZ5-8 maximum (1.703 W/(m²·K)) and asserts the default is >2.5× the code limit. **Passes now; must be REMOVED after fix (the `unwrap_or(5.0)` will no longer exist).**

  All 5 tests pass in the current (pre-fix) state: `test result: ok. 5 passed; 0 failed; 0 ignored`.
