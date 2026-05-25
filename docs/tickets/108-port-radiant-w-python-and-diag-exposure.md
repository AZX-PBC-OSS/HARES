# Expose `port_radiant_w` in Python `post_solvers` and Diagnostic CSV

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-python, hares-core/diagnostics

## Problem

`port_radiant_w` is missing from two observable surfaces:

1. Python `post_solvers` dict at `crates/hares-python/src/py_dwelling.rs:1939` — only `port_convective_w` is exposed.
2. `EnvelopeDiag.port_radiant_w` field at `crates/hares-core/src/diagnostics.rs:40` — the diagnostic struct has no such field, and the diagnostic CSV omits the column.

Without these, downstream analysis cannot distinguish convective port contributions from radiant ones. The radiant-vs-convective split is precisely what determines surface MRT, surface-air heat exchange, and the BESTEST 900FF diagnostic comparison. Exposing only the convective component is a half-measure.

## Current Behavior

`crates/hares-python/src/py_dwelling.rs:1939`:
```rust
post_solvers.set_item("port_convective_w", ...)?;
// no "port_radiant_w" entry
```

`crates/hares-core/src/diagnostics.rs:40`:
```rust
pub struct EnvelopeDiag {
    pub port_convective_w: f64,
    // no port_radiant_w
    ...
}
```

CSV output omits `port_radiant_w`; Python users cannot read the value.

## Required Behavior

1. Add `port_radiant_w: f64` field to `EnvelopeDiag`.
2. Populate it in the diagnostic-emission path (whatever step writes `EnvelopeDiag` records each timestep).
3. Add the column to the diagnostic CSV header and row writer.
4. Add `post_solvers.set_item("port_radiant_w", ...)?;` in `py_dwelling.rs:1939`.
5. Update any Python tests / fixtures consuming `post_solvers` to assert the new key is present.

## Approach

1. Open `crates/hares-core/src/diagnostics.rs:40` and add the `port_radiant_w` field.
2. Identify the population site (likely in `Dwelling::run_timestep` or the diagnostic emission helper). Wire the radiant-port total there.
3. Open `crates/hares-python/src/py_dwelling.rs:1939` and add the `set_item` call.
4. Update CSV header/row writer to include the new column.
5. Add Rust unit test asserting `EnvelopeDiag` populates `port_radiant_w` correctly for a representative dwelling.
6. Add Python integration test asserting the dict has the new key with the expected value.

## Definition of Done

- [ ] `EnvelopeDiag.port_radiant_w` field added at `crates/hares-core/src/diagnostics.rs:40`
- [ ] Field populated in diagnostic emission path
- [ ] CSV header and row writer include `port_radiant_w` column
- [ ] Python `post_solvers["port_radiant_w"]` exposed
- [ ] Rust unit test asserts field population
- [ ] Python integration test asserts dict key and value
- [ ] No `port_radiant_w` callsite uses a missing-field workaround

## Verification

```bash
cargo test -p hares-core diagnostics
cargo test -p hares-python
uv run pytest python/tests/test_diagnostics.py
```

## References

- HARES `crates/hares-envelope/src/thermal_solver/ports.rs` — radiant port path.
- ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.2 "Internal Heat Gains" — convective and radiant components must be reported separately for analysis.

## Related Tickets

- 091-port-radiant-inputs-all-zones (radiant path correctness)
- 092-zone-sensible-breakdown-debug-includes-radiant (related debug breakdown)
- 110-port-sensible-vs-convective-naming (related naming clarity)

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] **py_dwelling.rs line 1939** — confirmed. Line 1939 is `d.set_item("port_convective_w", gains.port_convective_w)?;` with no subsequent `port_radiant_w` entry before the `dict.set_item("post_solvers", d)?;` call at line 1941. The `gains` binding is `&solvers.envelope_gains` which is `EnvelopeComponentGains` — that struct *does* have `port_radiant_w` (config.rs:510) but it is never forwarded to the Python dict.
- [x] **diagnostics.rs line 40** — confirmed. `EnvelopeDiag` ends at line 41 with `port_convective_w: f64` as its final field. No `port_radiant_w` field is present. The full field list is: `window_solar_w`, `opaque_solar_lwr_w`, `interior_lwr_w`, `infiltration_by_zone`, `internal_gain_w`, `port_convective_w`.
- [x] **`port_radiant_w` already tracked upstream** — `EnvelopeComponentGains` (hares-envelope/src/thermal_solver/config.rs:510) carries `pub port_radiant_w: f64` and it is populated at mod.rs:585 from `port_radiant_indoor_w`. The data exists at the solver level; it simply is not forwarded to `EnvelopeDiag`.
- [x] **CSV writer** — `write_header` and `write_row` in diagnostics.rs do not write any `EnvelopeDiag` fields to the CSV at all (neither `port_convective_w` nor `port_radiant_w`). The ticket's claim about the CSV omitting the column is technically correct but slightly understates the situation: the entire `EnvelopeDiag` struct is currently absent from the CSV, not just `port_radiant_w`.
- [x] **OCHRE cross-check** — OCHRE (`vendors/OCHRE/ochre/Equipment/Equipment.py:80`) has an explicit `# FUTURE: separate convection and radiation, move radiation gains to the surfaces around the zone` comment. OCHRE currently combines radiant and convective into a single `sensible_gain` field and does NOT separately track or report `port_radiant_w`. HARES has gone further than OCHRE by implementing the separation at the solver level (`apply_port_radiant_inputs` in ports.rs) and recording `port_radiant_w` in `EnvelopeComponentGains`. The missing piece is forwarding that value into the diagnostic/Python surface.
- [x] **EnergyPlus cross-check** — EnergyPlus Engineering Reference (Zone Internal Gains, v25.2) confirms: "Convective gains are instantaneous additions of heat to the zone air" while "Radiant gains are distributed on the surfaces of the zone, where they are first absorbed and then released back into the room according to the surface heat balances." EnergyPlus exposes separate output variables `OtherEquipment Radiant Heating Rate [W]` and `OtherEquipment Convective Heating Rate [W]` (Input/Output Reference v8.4). HARES's `apply_port_radiant_inputs` function mirrors the EnergyPlus TMULT surface-distribution method (as noted in the inline doc at ports.rs:28). The ticket's claim about BESTEST 900FF diagnostic comparison requiring the radiant/convective split is consistent with EnergyPlus practice.

### Web-Verified Citations

**Citation 1**: ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.2 "Internal Heat Gains" — convective and radiant components must be reported separately for analysis.

- **Source found**: ASHRAE Handbook Online (F17/F21 Chapter 18 — "Nonresidential Cooling and Heating Load Calculations"), table of contents confirmed via https://handbook.ashrae.org/Handbooks/F17/IP/f17_ch18/f17_ch18_ip.aspx
- **Quoted passage**: From the chapter structure retrieved: Section 2 is titled "INTERNAL HEAT GAINS" with subsections 2.1 People, 2.2 Lighting, 2.3 Electric Motors, 2.4 Appliances. The lighting section states: "The **space fraction** in the table is the fraction of lighting heat gain that goes to the room … The **radiative fraction** is the radiative part of the lighting heat gain that goes to the room. The convective fraction of the lighting heat gain that goes to the room is 1 – the radiative fraction." This confirms the chapter discusses the convective/radiant split of internal heat gains.
- **Verdict**: **Partially correct** — the citation correctly identifies that ASHRAE HoF Ch. 18 covers internal heat gains and their convective/radiant split. However, the relevant section is numbered §18.2 only in the context of the chapter's own internal numbering ("Section 2 — Internal Heat Gains"), not a formal §18.2 of the Handbook. The 2021 edition chapter structure cannot be confirmed directly (only F17 was accessible online), but the content of the cited section is real. The citation's claim that "convective and radiant components must be reported separately for analysis" is directionally supported — the chapter prescribes the radiant fraction as an input to load calculations — but the chapter does not explicitly mandate *diagnostic reporting* of separate components; it mandates their distinction for load calculation purposes. The spirit of the citation is valid even if the mandatory-reporting framing slightly overstates what the Handbook requires.
- **Key note**: Chapter 18 of ASHRAE HoF covers **nonresidential** loads. A residential simulation (HARES) would more precisely cite **Chapter 17** ("Residential Cooling and Heating Load Calculations") or EnergyPlus directly for the same principle. This is a minor citation imprecision; the underlying principle is correct.

**Citation 2 (implicit)**: EnergyPlus TMULT radiant distribution method, referenced in ports.rs inline doc but not explicitly cited in the ticket.

- **Source found**: EnergyPlus Engineering Reference, Zone Internal Gains: https://bigladdersoftware.com/epx/docs/25-2/engineering-reference/zone-internal-gains.html
- **Quoted passage**: "Radiant heat is absorbed by the inside surfaces of the zone according to an area times long-wave absorptance weighting scheme" (the TMULT method). EnergyPlus I/O Reference documents output variables `OtherEquipment Radiant Heating Rate [W]` and `OtherEquipment Convective Heating Rate [W]` as standard diagnostic outputs (v8.4 I/O Reference, group Internal Gains).
- **Verdict**: **Confirmed** — the EnergyPlus precedent for separating and reporting radiant vs. convective equipment gains is well-established. HARES's implementation mirrors this approach; the gap is only at the diagnostic-exposure layer.

### Legitimacy

- **Verdict**: **Legitimate**
- **Rationale**: All code claims in the ticket are verified against the current source. `EnvelopeDiag` (diagnostics.rs:29–41) has no `port_radiant_w` field and `py_dwelling.rs:1939` does not emit the value to Python — these are confirmed absent. The value is computed and available one layer up in `EnvelopeComponentGains` (config.rs:510, populated at mod.rs:585). OCHRE does not implement the radiant/convective split at all, so there is no OCHRE baseline to break parity with; HARES has independently implemented the separation correctly in the solver but stopped short of surfacing it diagnostically. EnergyPlus confirms that reporting both components is standard practice. The ASHRAE citation is slightly imprecise (should reference Ch. 17 for residential, and the chapter section numbering is informal) but the underlying principle is correct. The ticket's scope is accurate and the fix is straightforward.

### Proposed Fix Summary

1. Add `pub port_radiant_w: f64` to `EnvelopeDiag` in `crates/hares-core/src/diagnostics.rs` after line 40.
2. Populate the new field wherever `EnvelopeDiag` is constructed — the value to use is `EnvelopeComponentGains::port_radiant_w`, which is already computed by the thermal solver.
3. Add `"port_radiant_w"` to `write_header` and `write_row` in diagnostics.rs (noting that neither function currently emits any `EnvelopeDiag` fields; this is a broader gap to address).
4. Add `d.set_item("port_radiant_w", gains.port_radiant_w)?;` in `py_dwelling.rs` at line 1940 (after `port_convective_w`).
5. Update any Python tests that assert the full key set of `post_solvers`.

Note: The CSV gap is broader than the ticket states — currently `write_header`/`write_row` do not write *any* `EnvelopeDiag` fields. The ticket's scoping of the issue to `port_radiant_w` is still valid, but the implementer should be aware that wiring `port_radiant_w` into the CSV requires also wiring the other `EnvelopeDiag` fields, or at minimum ensuring those are handled consistently.

### Test Written

- **File**: `crates/hares-core/tests/ticket_108_port_radiant_diag.rs`
- **What it tests**: Attempts to construct `EnvelopeDiag { port_radiant_w: 300.0, .. }` — fails to compile with `E0560: struct EnvelopeDiag has no field named 'port_radiant_w'`, directly demonstrating the missing-field bug. A second test (`envelope_component_gains_has_port_radiant_w`) verifies that `EnvelopeComponentGains` already carries the value and that both halves of the split sum correctly. The first test will begin compiling (and passing) only after the field is added, making it a clean compile-gated regression guard.
