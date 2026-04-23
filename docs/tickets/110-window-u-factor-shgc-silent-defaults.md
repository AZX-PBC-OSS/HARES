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
