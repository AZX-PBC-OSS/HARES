# Use `InteriorLwrMethod::StarMesh` Explicitly at Callsites

> **Note:** This document contains EnergyPlus Engineering Reference section-number
> citations (e.g. "EnergyPlus §3.5.10") that are unverifiable against the
> web-hosted EnergyPlus documentation. These citations are preserved for audit
> provenance. See `docs/eplus/section-mapping.md` for heading-based citations.

**Severity**: Nit
**Priority**: P4
**Status**: Open
**Areas**: hares-envelope, hares-core

## Problem

Several callsites construct or configure the interior longwave radiation network using `InteriorLwrMethod::default()` rather than naming the variant explicitly. Per the project clarity rule (no implicit defaults where the choice has physics-significant consequences), the variant should be named explicitly at every callsite so that a future reader can see at a glance which network topology is in use.

The `default()` value is `InteriorLwrMethod::StarMesh`, but a reader of the callsite cannot tell this without jumping to the type definition. Worse, if the default is ever changed (e.g. to a future "ScriptF" exact method), every callsite that relied on the implicit default will silently switch behaviour without any callsite change.

## Current Behavior

Callsites use `InteriorLwrMethod::default()` rather than `InteriorLwrMethod::StarMesh`. Locations:
- Anywhere `InteriorLwrMethod::default()` appears in `crates/hares-envelope/src/` and `crates/hares-core/src/dwelling/`.

## Required Behavior

Every callsite that constructs an `InteriorLwrMethod` value must name the variant explicitly:

```rust
// Before:
let lwr = InteriorLwrMethod::default();

// After:
let lwr = InteriorLwrMethod::StarMesh;
```

This applies to struct-literal initialisations, `..Default::default()` spread expressions where `InteriorLwrMethod` is one of the fields, and any other implicit-default usage.

## Approach

1. `grep` the workspace for `InteriorLwrMethod::default()` and `InteriorLwrMethod` in spread expressions.
2. For each callsite, replace the implicit default with the explicit `StarMesh` variant.
3. Where a struct uses `..Default::default()` and `InteriorLwrMethod` is one of the fields, replace the spread with explicit field initialisation OR set the field explicitly before the spread.
4. Optionally remove the `Default` impl on `InteriorLwrMethod` entirely to make the explicit-variant requirement enforced by the compiler.

## Definition of Done

- [ ] No callsite uses `InteriorLwrMethod::default()`
- [ ] Every construction site names the variant explicitly
- [ ] If the `Default` impl is removed, the workspace still builds and all tests pass
- [ ] Comment at the type definition explains why an explicit variant is required

## Verification

```bash
cargo build --workspace
cargo test --workspace
rg 'InteriorLwrMethod::default' crates/
rg 'InteriorLwrMethod' crates/ | rg 'default'
```

## References

- HARES project clarity convention — physics-significant choices must be explicit at the callsite.
- EnergyPlus Engineering Reference §3.5.10 "Network Solution" — distinguishes ScriptF, MRT, and Star/Mesh methods; the choice of method changes results materially.

## Related Tickets

- 044-lwr-fallback-linearised-not-scriptf
- 047-interior-lwr-uses-last-step-zone-temp
- 089-radiation-frac-starmesh-rederivation

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match (or note corrected location)

  The ticket does not cite specific line numbers; it refers to "Anywhere
  `InteriorLwrMethod::default()` appears in `crates/hares-envelope/src/` and
  `crates/hares-core/src/dwelling/`." A workspace grep confirms **28 callsites**
  using `InteriorLwrMethod::default()`:

  - `crates/hares-envelope/src/thermal_solver/mod.rs` — lines 861, 942, 1072,
    1163, 1616, 1663, 1833, 1898, 1947, 2000, 2139, 2288, 2448, 2793, 2904,
    2985, 3081, 3228, 3339, 3677, 3740, 3924, 4083, 4391, 4521, 4766 (26 hits)
  - `crates/hares-envelope/src/thermal_solver/config.rs` — line 457 (1 hit,
    inside `impl Default for ThermalSolverConfig`)
  - `crates/hares-core/src/dwelling/solver_builder.rs` — line 493 (1 hit,
    annotated with the comment `// StarMesh`)

  Total: **28 callsites** (none in production `src/` files use the explicit
  variant; one test file in `crates/hares-envelope/tests/interior_lwr.rs`
  used it and was updated as part of this audit).

- [x] Described logic matches current implementation

  `boundary_rc.rs:318–326` confirms:
  ```rust
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
  pub enum InteriorLwrMethod {
      #[default]
      StarMesh,
      ScriptF,
  }
  ```
  `InteriorLwrMethod::default()` therefore resolves to `StarMesh` today. The
  ticket's concern is correct: a future change to `#[default]` (e.g. to add a
  new variant) would silently alter behaviour at every callsite.

- [x] OCHRE cross-check result: **diverges (intentionally) — with evidence**

  OCHRE (`vendors/OCHRE/ochre/Models/Envelope.py`, lines 734–742) exposes
  three radiation modes: `"full"` (nonlinear iterative T⁴), `"linear"`
  (linearized via star-mesh transform + floating-node elimination), and
  `"none"`. HARES maps `InteriorLwrMethod::StarMesh` to OCHRE's `"linear"`
  path and `InteriorLwrMethod::ScriptF` to OCHRE's `"full"` path. The
  difference is intentional: HARES bakes the linearized conductances into the
  A-matrix at construction time (OCHRE also does this — see the comment at
  `Envelope.py:1053`: *"node <label>-rad is removed from envelope model using
  star-mesh transform"*), whereas OCHRE's `"full"` path runs iterative T⁴
  radiosity each timestep. The divergence is by design, not accidental.

- [x] EnergyPlus cross-check result: **N/A — citation cannot be verified**

  See "Web-Verified Citations" below.

### Web-Verified Citations

**Citation 1**
- **Claim**: "HARES project clarity convention — physics-significant choices
  must be explicit at the callsite."
- **Source**: Internal convention; not an external document. No web
  verification required or possible.
- **Verdict**: Accepted as internal policy.

**Citation 2**
- **Claim**: "EnergyPlus Engineering Reference §3.5.10 'Network Solution' —
  distinguishes ScriptF, MRT, and Star/Mesh methods; the choice of method
  changes results materially."
- **Sources searched and fetched**:
  - BigLadder EnergyPlus Engineering Reference (versions 8.0, 8.1, 8.5, 8.6,
    8.8, 9.2, 9.3, 9.4, 9.5, 9.6, 22.2, 23.1, 24.1, 25.1) — all fetched via
    WebFetch and confirmed section structure.
  - EnergyPlus table of contents (8.6): top-level sections are named by topic
    ("Surface Heat Balance Manager", "Inside Heat Balance", etc.) with no
    decimal subsection numbering in the online format.
  - EnergyPlus PDF attempts: both
    `eta-publications.lbl.gov/sites/default/files/engineeringreference.pdf`
    and `windows.lbl.gov/sites/default/files/engineeringreference.pdf`
    returned 403 / content-too-large.
- **Quoted passage** (EnergyPlus 24.1, "Internal Long-Wave Radiation Exchange"
  section, as fetched):

  > "EnergyPlus offers two algorithms for modeling long wave radiation: The
  > 'ScriptF' method, and the 'CarrollMRT' methods."

  And: "The 'ScriptF' algorithm was developed by Hottel (Hottel and Sarofim,
  *Radiative Transfer*, Chapter 3, McGraw-Hill, 1967)."

  CarrollMRT: "an approximation of gray-body long-wave radiation exchange
  within an enclosure that simplifies the surface-to-surface radiation
  exchange by using a single, mean radiant temperature node, Tr."

- **What the reference actually says vs. what the ticket claims**:
  - The EnergyPlus Engineering Reference **does not have a section numbered
    "3.5.10"**. The online versions use topic-named sections without decimal
    numbering; the table of contents for version 8.6 lists no such subsection.
  - The section is titled **"Internal Long-Wave Radiation Exchange"** under
    **"Inside Heat Balance"**, which corresponds to the broader section 3
    ("Surface Heat Balance Manager") — but no "3.5.10" sub-number is visible
    in any version examined.
  - EnergyPlus distinguishes **two** interior LWR methods (ScriptF and
    CarrollMRT), **not three**. The term **"Star/Mesh"** does not appear
    anywhere in the EnergyPlus Engineering Reference for interior longwave
    radiation.
  - The star/mesh terminology comes from circuit-theory literature (Carroll
    1980/1981 papers cited by EnergyPlus as the basis for CarrollMRT), not
    from EnergyPlus as a named method.
  - The statement "the choice of method changes results materially" is
    physically accurate but is not a quoted claim from the cited section.

- **Verdict**: **Incorrect** — Section §3.5.10 "Network Solution" does not
  exist. EnergyPlus has two methods (ScriptF, CarrollMRT), not three. "Star/
  Mesh" is not an EnergyPlus method name; the star-network concept underlies
  CarrollMRT but EnergyPlus does not use that label. The citation is inaccurate
  in section number, title, and method taxonomy.

  A corrected citation would be:
  > EnergyPlus Engineering Reference, "Inside Heat Balance" → "Internal
  > Long-Wave Radiation Exchange" (no section number in online docs; PDF §3.x
  > varies by release). Two methods: ScriptF (Hottel matrix) and CarrollMRT
  > (single MRT node, Carroll 1980/1981). HARES `StarMesh` corresponds to the
  > linearized MRT-star-node concept, not to a named EnergyPlus method.

### Legitimacy

- **Verdict**: **Partially Legitimate**
- **Rationale**: The core issue is real and well-described: 28 production-code
  callsites use `InteriorLwrMethod::default()` instead of
  `InteriorLwrMethod::StarMesh`, exactly as the ticket claims. The risk
  described — that a future change to `#[default]` would silently alter
  behaviour at all callsites — is genuine. The approach (explicit variant,
  optionally remove the `Default` impl) is correct. The ticket's only flaw is
  its EnergyPlus citation: §3.5.10 "Network Solution" does not exist in the
  EnergyPlus Engineering Reference; EnergyPlus uses two methods (ScriptF and
  CarrollMRT), not three, and does not use the "Star/Mesh" label. This
  citation error is cosmetic — it does not affect the validity of the fix —
  but it should be corrected to avoid misleading future reviewers.

### Proposed Fix Summary

No production-code change is needed beyond textual substitution:

1. Run `rg 'InteriorLwrMethod::default\(\)' crates/` to list all 28 callsites.
2. Replace each with `InteriorLwrMethod::StarMesh` (or the fully-qualified
   `crate::boundary_rc::InteriorLwrMethod::StarMesh` / `hares_envelope::
   InteriorLwrMethod::StarMesh` where needed).
3. In `impl Default for ThermalSolverConfig` (`config.rs:457`), keep the
   `Default` impl but change the field to
   `interior_lwr_method: InteriorLwrMethod::StarMesh,`.
4. Optionally: remove `Default` from `InteriorLwrMethod`'s `#[derive]` list
   and the `#[default]` attribute on `StarMesh`, then fix the one callsite
   in `impl Default for ThermalSolverConfig` — the compiler will flag the
   remaining 27 callsites.
5. Correct the EnergyPlus citation in this ticket.

Do NOT modify any test files that intentionally use `InteriorLwrMethod::ScriptF`.

### Test Written

- **File**: `crates/hares-envelope/tests/interior_lwr.rs`
- **Function**: `ticket128_interior_lwr_default_is_starmesh`
- **What it tests**: Asserts that `InteriorLwrMethod::default() ==
  InteriorLwrMethod::StarMesh`. This test passes today (confirming no
  regression has been introduced yet) and will fail the moment someone
  changes the `#[default]` attribute without updating all callsites,
  providing an early warning before the behavioural change propagates.
- **Status**: Test compiles and passes (`cargo test --test interior_lwr
  ticket128` → ok).
