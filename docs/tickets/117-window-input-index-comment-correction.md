# Correct Comment About Windows `input_index` in Radiant Distribution

**Severity**: Low
**Priority**: P3
**Status**: Open
**Areas**: hares-envelope/thermal_solver/ports

## Problem

The comment at `crates/hares-envelope/src/thermal_solver/ports.rs:135-137` claims "Windows (input_index=None)". This is wrong: `crates/hares-core/src/dwelling/solver_builder.rs:818` always sets `input_index: Some(zone_air_idx)` for windows. Window exclusion from the radiant distribution actually works because `solar_absorptance = 0.0` causes the radiant weight to be zero — the `input_index` is set, it's just contributing zero weight.

The comment is misleading. A future contributor reading "Windows (input_index=None)" will look for an `Option::None` branch that does not exist and may introduce a wrong mental model when modifying the code.

## Current Behavior

`crates/hares-envelope/src/thermal_solver/ports.rs:135-137`:
```rust
// Windows (input_index=None) are excluded from radiant distribution
```

`crates/hares-core/src/dwelling/solver_builder.rs:818`:
```rust
input_index: Some(zone_air_idx),  // always set, even for windows
```

The actual exclusion mechanism is `solar_absorptance = 0.0` for windows, producing zero weight in the distribution loop.

## Required Behavior

Update the comment to reflect what the code actually does:

```rust
// Windows have solar_absorptance = 0.0 set at construction (solver_builder.rs),
// so they receive zero weight in the radiant distribution. The input_index field
// is always Some(zone_air_idx); the zero weight is the exclusion mechanism.
```

No code change needed. This is a documentation correction.

## Approach

1. Open `crates/hares-envelope/src/thermal_solver/ports.rs:135-137`.
2. Replace the misleading comment with the corrected text above.
3. Verify the assertion by inspection of `solver_builder.rs:818` — the comment must match the code.

## Definition of Done

- [ ] Comment at `ports.rs:135-137` corrected
- [ ] Comment cites the actual exclusion mechanism (`solar_absorptance = 0.0`)
- [ ] Comment cross-references `solver_builder.rs:818` for verifiability

## Verification

```bash
cargo test -p hares-envelope ports   # no behaviour change; sanity check
```

## References

- HARES `crates/hares-envelope/src/thermal_solver/ports.rs` — distribution loop.
- HARES `crates/hares-core/src/dwelling/solver_builder.rs:818` — window port construction.
- Project policy `feedback_no_useless_comments.md` — comments must accurately describe what the code does.

## Related Tickets

- 091-port-radiant-inputs-all-zones (related radiant-port work)
- 049-window-solar-shgc-vs-transmittance-absorbed-inward

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match (with correction noted below)
- [x] Described logic matches current implementation
- [x] OCHRE cross-check result: **diverges intentionally** — OCHRE uses `h_idx = None` to mark surfaces without RC nodes; HARES chose `Option<usize>` in the struct but never sets it to `None` in production
- [x] EnergyPlus cross-check result: **N/A for this ticket** — EnergyPlus excludes windows from its radiant distribution loop via opaque-surface range bounds (`firstSurfOpaque..lastSurfOpaque`), not via zero absorptance or a `None` index; HARES's `solar_absorptance = 0.0` approach achieves the same functional result through a different mechanism

**Corrected line locations:**

The ticket cites `ports.rs:135-137`. The actual current location of the misleading comment spans two locations in the same function:

- `ports.rs:122-124` (doc-comment of `distribute_radiant_solar_surfaces`): _"Windows have `input_index: None` and are excluded from the TMULT weighting."_ — also incorrect.
- `ports.rs:135-136` (inline comment): _"Windows (input_index=None) can't absorb radiant gain into an RC node; skip them from the TMULT weighting."_ — the inline comment cited by the ticket.

Both locations contain the same factual error. The ticket's proposed fix text targets only the inline comment; the doc-comment at line 122-124 contains the same wrong claim and should also be corrected.

**Solver_builder.rs line:**

The ticket cites `solver_builder.rs:818`. Current code at line 818 reads:
```rust
input_index: Some(s.input_index),
```
This maps `InteriorSurfaceInfo.input_index: usize` → `InteriorSolarSurfaceInfo.input_index: Option<usize>` using `Some(...)` unconditionally. The line number matches. `InteriorSolarSurfaceInfo` is only constructed in one place in the entire codebase (`solver_builder.rs:817`), and it always uses `Some(...)`.

**No production code path ever sets `input_index: None`.** Grepping the entire `crates/` tree for `input_index.*None` returns zero matches. The `is_some()` guard at `ports.rs:137` and the `if let Some(idx)` at `ports.rs:151` are dead-branch code.

**Window exclusion mechanism confirmed (solver_builder.rs:699-768):**

For any boundary where `is_window == true`, the builder sets:
```rust
solar_absorptance: 0.0,   // line 739
```
This gives windows zero weight in the distribution: `s.area_m2 * s.solar_absorptance = 0.0`, which prevents any allocation to the window's `input_index` regardless of whether it is `Some` or `None`.

### OCHRE Cross-Reference

**File:** `vendors/OCHRE/ochre/Models/Envelope.py`

OCHRE uses a parallel but distinct mechanism. In OCHRE, window boundaries produce no RC capacitors (`n_nodes = 0`, `create_rc_data` returns `([], [r_window])`). The window interior surface's node name is set to the exterior zone label (`ext_zone_label`), so `"H_" + surface.node` does not appear in the input vector — leaving `surface.h_idx = None` (Envelope.py:858-859). When distributing transmitted solar to interior surfaces, OCHRE weights by `area × absorptivity`, where window absorptivity is non-zero (calculated from SHGC; `calculate_window_parameters` in `utils/envelope.py:406-431`). Windows _do_ participate in OCHRE's solar distribution (with non-zero absorptivity), but they _don't_ inject into an RC node (because `h_idx = None`, checked at Envelope.py:1167-1168: `valid = zone._h_idxs >= 0`).

HARES chose a different approach: windows participate in the `InteriorSolarSurfaceInfo` list with `input_index: Some(zone_air_idx)` but are silenced by `solar_absorptance = 0.0`, giving zero weight in the distribution. This is an intentional design simplification — HARES treats absorbed solar at windows as zero for the distribution purpose, consistent with the understanding that window conduction is handled by the U-factor path, not the radiant RC-injection path.

### Web-Verified Citations

The ticket contains no explicit standards citations (ASHRAE, NFRC, ISO, DOE). The only references are to internal HARES source files and project policy (`feedback_no_useless_comments.md`). Therefore no external citation verification is required.

The following EnergyPlus and OCHRE references were checked as part of cross-referencing the underlying physics:

---

**Claim (implicit):** EnergyPlus excludes windows from internal radiant gain distribution.

**Source found:** [EnergyPlus Engineering Reference — Zone Internal Gains (v25.2)](https://bigladdersoftware.com/epx/docs/25-2/engineering-reference/zone-internal-gains.html)

**Quoted passage:** "Long wavelength radiation from all internal sources, such as people, lights and equipment, is combined and then distributed over surfaces. If all surfaces in the room are opaque, the radiation is distributed in proportion to the area*absorptance product of each surface."

**EnergyPlus source code (`HeatBalanceSurfaceManager.cc`):** The distribution loop iterates over `firstSurfOpaque` to `lastSurfOpaque`, explicitly excluding windows via range bounds rather than zero absorptance. Windows are handled separately in the glazing heat-balance (absorbed solar in the glass is added to the glass heat balance, not injected into the opaque-surface distribution).

**Verdict:** Confirmed — EnergyPlus excludes windows from the internal-radiant distribution loop, consistent with the HARES intent. The HARES mechanism (zero `solar_absorptance`) achieves the same result through different means.

---

**Claim (implicit):** OCHRE windows have `input_index = None` (or equivalent) as the exclusion mechanism.

**Source found:** `vendors/OCHRE/ochre/Models/Envelope.py` (local submodule)

**Quoted passage (Envelope.py:858-859):**
```python
if "H_" + surface.node in self.input_names:
    surface.h_idx = self.input_names.index("H_" + surface.node)
```
For windows, `surface.node == self.ext_zone_label` (e.g., "EXT"), so `"H_EXT"` is not an input name → `h_idx` remains `None`. The vectorised guard at line 929-930 maps `None → -1` and line 1167 filters with `valid = zone._h_idxs >= 0`.

**Verdict:** Confirmed — OCHRE does use `None` (sentinel `-1`) as the exclusion mechanism for windows in radiant injection. HARES uses `solar_absorptance = 0.0` instead. Both are valid; the comment in HARES incorrectly describes the OCHRE mechanism as if it were HARES's own.

### Legitimacy

**Verdict:** Legitimate

**Rationale:** The bug is real and the ticket's description is accurate. The comment at `ports.rs:122-124` (doc-string) and `ports.rs:135-136` (inline) both assert "Windows (input_index=None)" as the exclusion mechanism. This is factually wrong: `InteriorSolarSurfaceInfo.input_index` is always `Some(zone_air_idx)` in production because `solver_builder.rs:818` always wraps the value in `Some(...)`, and no other construction site exists. The actual exclusion mechanism is `solar_absorptance = 0.0` assigned at `solver_builder.rs:739`. The misleading comment could cause a future contributor to search for a `None`-setting code path, introduce a bug trying to make windows "correctly" set `input_index = None`, or misunderstand the zero-weight short-circuit as a fallback for a `None` case. The ticket correctly identifies the problem, the correct mechanism, and the minimal fix (comment-only, no logic change). One minor gap: the ticket cites only the inline comment (lines 135-136) but not the identical error in the doc-comment at lines 122-124; both should be corrected.

### Proposed Fix Summary

1. Replace the doc-comment at `ports.rs:122-124` to remove the false `input_index: None` claim and instead state that windows are excluded by their zero `solar_absorptance`.
2. Replace the inline comment at `ports.rs:135-136` with the corrected text proposed in the ticket.
3. No logic changes needed.

The doc-comment correction is an addendum to the ticket's scope; both should be done atomically.

### Test Written

- **File:** `crates/hares-envelope/src/thermal_solver/mod.rs` (within existing `#[cfg(test)]` module)
- **Function:** `ticket_117_window_excluded_via_zero_solar_absorptance_not_none_input_index`
- **What it tests:** Exercises the `distribute_radiant_solar_surfaces` path (StarMesh/solar, via `interior_solar_zones` with empty `interior_lwr_zones`) with a window surface that has `input_index: Some(2)` and `solar_absorptance: 0.0`. Asserts that the window's input index receives zero gain, the opaque wall RC node receives the expected fraction, zone air receives the remainder, and energy is conserved. The test confirms the actual exclusion mechanism is zero absorptance (not `None` index), validating the ticket claim. The test passes as written — this is a _documentation_ bug, not a behaviour bug, so no test can be "failing"; the test instead documents and locks the correct invariant that `input_index` is always `Some` for window solar surfaces.
