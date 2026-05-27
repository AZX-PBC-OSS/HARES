# HARES Team Constitution

This document defines the engineering standards for HARES. Every agent reads it before touching code. These are not suggestions — they are the bar.

---

## Architecture

HARES is a 10-crate workspace with a strict dependency DAG. Understand it before adding anything:

```
hares-types → hares-physics, hares-control
hares-envelope → types, physics
hares-equipment → types, physics, control
hares-io → types, physics, control, envelope, equipment
hares-core → all of the above + tariff
hares-fleet → core, io, types
hares-python → core, fleet, control, equipment, types, io, tariff
```

**The DAG is law.** No crate may import from a crate above it in the dependency order. Physics belongs in `hares-physics`. I/O parsing belongs in `hares-io`. Equipment models belong in `hares-equipment`. Envelope RC/state-space belongs in `hares-envelope`. Orchestration belongs in `hares-core`. Do not reach across these boundaries.

The central pattern is **Environment / Port / Solver decoupling**: equipment reads `&EnvironmentState` (immutable), writes to `PortSlots` (accumulator bus), and solvers consume port totals. No equipment references another. No solver references equipment internals.

---

## Code quality

### DRY — one home for shared logic

If a constant, formula, or algorithm appears in more than one place, it lives in exactly one place. Shared physical constants belong in `hares-physics::constants`. Shared psychrometric functions belong in `hares-physics::psychrometrics`. Shared conversion factors are not re-derived inline — they are imported.

### Type safety

Use Rust's type system to make illegal states unrepresentable:
- Prefer `Result<T, E>` with typed, discriminated error enums over `Option` with silent fallback.
- Never `unwrap()` or `expect()` in production paths without a documented invariant at the call site.
- Never use `f64` for quantities where the unit matters at a boundary — use newtypes or at minimum a doc comment naming the unit.
- SI units internally, always. Unit conversions happen only at I/O boundaries (`hares-io`). Label every conversion with the source and target unit.
- The direct HARES API surface — constructors, crate public interfaces, type fields — accepts and returns only SI quantities. No caller should need to know or guess which unit system a value is in.

### Idiomatic Rust

Match the conventions of the surrounding crate:
- Error types: follow the existing typed error enum pattern in the crate; do not introduce `anyhow` or `Box<dyn Error>` in library code.
- Logging: use `tracing::{debug!, info!, warn!, error!}` — not `println!`. Use structured fields (`field = value`), not string interpolation.
- Iterators: prefer iterator chains over manual index loops where it aids clarity. Never collect unnecessarily.
- Pattern matching: use exhaustive `match` — no `_ =>` wildcards that silently swallow unhandled variants.

### No broken windows

Every file you touch must leave the codebase strictly better than you found it:
- No new `clippy` warnings. If you encounter an existing warning in code you are modifying, fix it.
- No `#[allow(...)]` without a comment naming the specific constraint that makes the suppression necessary.
- No commented-out code. Delete it.
- No `TODO`, `HACK`, `FIXME`, or `workaround` left in production code. Either fix it now or open a ticket.
- No `#[ignore]` on tests unless there is a documented, tracked reason.

### File size

Files above ~2000 lines are a signal that a module is doing too much. When you encounter this, consider whether logic can be extracted into a focused submodule with a clear responsibility. Do not be dogmatic — a 2200-line file with a single coherent concern is fine; a 1500-line file with four unrelated concerns is not. Judge by cohesion, not line count.

### Fix the class of bug, not the instance

Every ticket identifies a specific instance of a problem. The fix must address the entire class. Before writing code, ask: does this same issue appear anywhere else?

- A missing port on a dehumidifier → audit every equipment type that moves moisture.
- A hardcoded wrong constant → grep the workspace for that value.
- A missing unit guard on an HPXML field → audit every numeric field with an adjacent unit element in the same parser.
- A silent `_ =>` wildcard on an enum → find every `match` on that enum type.

Fix all instances in one go. A fix that corrects one call site while leaving identical bugs elsewhere is not a fix.

### No shortcuts, no shims

Implement fixes completely:
- If a type change requires updating 50 call sites, update all 50.
- No backwards-compatibility re-exports or aliases introduced to avoid propagating a change.
- No wrapper types used solely to paper over a type mismatch rather than fixing the mismatch.
- No partial fixes that correct one call site while leaving others wrong.
- No weakening of assertions (wider tolerances, `assert!` instead of `assert_eq!`) to make a test pass rather than fixing the code.

### No lazy deferrals — fix it now

When you encounter a related or adjacent issue while implementing a ticket, **fix it**. Do not open a ticket and move on. Do not leave a `// TODO`. Do not write "out of scope for this ticket." The issue is in front of you, you understand it, and the fix is doable. That is the moment to fix it — not a hypothetical future moment when someone else re-discovers it.

The broken-window pattern this rule targets: an agent notices something wrong in a file it is already editing, decides it is "adjacent" rather than "core" to the ticket, leaves it broken, and considers the ticket done. That is not done. The codebase is worse than when you started in a way that you could have prevented with one additional edit.

**The test:** if you can describe the fix in one sentence, you can implement it. Describing the bug in a comment or a TODO and not fixing it is the same as leaving it broken — you just added noise.

When a related issue genuinely requires work beyond the current ticket's scope (different crate, different subsystem, non-trivial architectural change), document the dependency concretely and create the tracking ticket *before* marking the current ticket done. Vague "future work" with no ticket ID is an abandoned item.

### No untracked deferrals

Work that is doable with the current infrastructure must not be deferred to vague "future work" or "a later ticket." A deferred item with no tracking ticket is an abandoned item.

This applies specifically to three patterns that are forbidden:

1. **The half-fix.** Parsing, storing, and validating a value, but never applying it to correct the simulation — e.g. emitting a warning for a wrong curve but not scaling it, detecting a mismatched count but silently padding with identity, or hardcoding a well-known constant and leaving a comment saying "should be configurable later." If the fix is doable, finish it. The hard part is finding the bug; the fix is the line of code that makes the numbers correct.

2. **The placeholder default.** Using a fixed value where a dynamic computation is possible — e.g. hardcoding terrain class to `Suburban` when the site metadata is available, defaulting slab thickness to 0.1 m when material layers are in scope, or freezing a coefficient at init-time when the per-step recomputation function already exists in the crate. A `// TODO: make configurable later` comment above a hardcoded value is not documentation — it is an admission that the work was abandoned.

3. **The performative phase split.** Implementing Phase 1 (loud error) and deferring Phase 2 (the actual fix) to "a dedicated follow-on ticket" that is never created. If both phases are doable, implement both. If Phase 2 genuinely depends on infrastructure not yet built (a new crate, a geometry subsystem, a matrix parameterisation mechanism), document the dependency explicitly and create the tracking ticket before marking Phase 1 done.

**The rule:** any `### Known Limitations` entry that describes fixable, in-scope work without a concrete ticket ID — or any comment containing "TODO", "FIXME", "later", "future pass", "subsequent ticket", or "deferred" without a ticket reference — must either be fixed now or accompanied by a ticket created in `.shipwright/initiatives/I-01/tickets/` before the parent ticket reaches `status: done`.

A ticket that does work but stops before the line of code that makes the simulation produce correct numbers is not done. It is performative.

---

## Trust nothing, verify everything

Tickets, audit sections, and cited line numbers are starting points — not ground truth. Before implementing anything:

- **Read the actual code** at the cited locations. Confirm the bug exists as described. If it has already been fixed or never existed, stop and document that.
- **Verify every citation against the primary source.** A ticket that cites EnergyPlus §3.5 or ASHRAE Ch.18 §31 may have the section number wrong, the formula paraphrased incorrectly, or the constant value mis-transcribed. Read `docs/eplus/` for the narrative, `vendors/EnergyPlus/src/` for the actual implementation, and `vendors/OCHRE/` as a cross-check. For HPXML field structure, element names, unit vocabularies, and cardinality, use `docs/hpxml/` — see below. Implement from the correct source, not from the ticket's summary of it. Document what you found in the ticket's `## Implementation Notes`.
- **Read callers, tests, and the surrounding module** before touching anything. A fix that is locally correct but architecturally wrong creates more problems than it solves.
- **Do not trust `unwrap()`-heavy or poorly-tested code** just because it is pre-existing. If you find it in files you are modifying, fix it.

## Domain alignment

Every implementation decision that touches code, formulas, algorithms, defaults, or units must be verified against the authoritative sources before the work is considered done. This is not optional and is not deferred to review.

### What must be verified

- **Formulas and equations.** Any formula or equation must be traced to a primary source — ASHRAE standard chapter and equation number, EnergyPlus Engineering Reference section, or HPXML specification section. Paraphrased or transcribed equations in tickets, comments, or prior code are starting points, not authority. Verify the equation against the actual document in `docs/eplus/` or `vendors/EnergyPlus/src/`; if the document and the source disagree, the source wins. Every formula in code must carry an inline citation to section and equation number.

- **Algorithms and models.** Any simulation algorithm or sub-model (e.g. AIM-2 infiltration, RC thermal network, psychrometric calculation, equipment capacity curve) must be checked against the EnergyPlus source (`vendors/EnergyPlus/src/`) for implementation correctness and against OCHRE (`vendors/OCHRE/`) as a cross-check. Where HARES diverges from either reference, document the divergence explicitly with a rationale comment.

- **Defaults and coefficients.** Any default value, fallback, or coefficient must have a named source. A default is not a guess — it is a specification value, a standard-mandated value, or a documented engineering choice. If you cannot name the source of a default, it must not exist in the code. Defaults derived from HPXML must be verified against `docs/hpxml/hpxml-elements.md` and `docs/hpxml/hpxml-data-types.md`; defaults derived from EnergyPlus must be verified against the Engineering Reference section or source code for that model.

- **Units.** Every unit used in a formula, constructor, or I/O boundary must be verified against the relevant standard or specification. HPXML units must match the unit vocabulary in `docs/hpxml/hpxml-units.md`; EnergyPlus quantities must match the units used in `vendors/EnergyPlus/src/` for the corresponding model. SI unit assumptions at internal boundaries must be confirmed — do not assume a quantity is in SI because it "looks right." Check the definition.

- **Enumerated values and vocabularies.** String enumerations from HPXML must be verified against `docs/hpxml/hpxml-enumerations.md`. Do not add or accept enumeration members by inference; the specification is the only authority.

### How to verify

1. For EnergyPlus: read the relevant section of `docs/eplus/` for the narrative, then confirm the formula or coefficient against `vendors/EnergyPlus/src/` for the implementation. `vendors/EnergyPlus/tst/` contains test cases that validate expected numeric results — use them.
2. For OCHRE: read `vendors/OCHRE/` as a cross-check, not a specification. Where OCHRE deviates from ASHRAE or EnergyPlus without justification, treat that as a bug in OCHRE and implement correctly in HARES.
3. For HPXML: grep `docs/hpxml/hpxml-elements.md` for the element path, `docs/hpxml/hpxml-units.md` for its unit enum and allowed values, and `docs/hpxml/hpxml-enumerations.md` for string fields. These files are grep-friendly by design.
4. For ASHRAE: cite the standard, edition, chapter, and equation number in the comment. If the standard text is not locally available, state the citation explicitly and flag the item for review rather than proceeding on memory.

### Citation format

Every formula, constant, default, and algorithm in code must carry an inline source citation. The citation must name the document, edition or version, section or chapter, and equation or table number where applicable. Vague citations ("per ASHRAE" or "from EnergyPlus") are not acceptable — they cannot be verified.

Acceptable:
```rust
// ASHRAE HoF 2021 Ch.1 Eq.30: h_fg = 2501 kJ/kg at 0°C reference temperature.
// EnergyPlus ERM 24.1 §5.3.2 Eq.16: Q_inf = ρ·V̇·c_p·(T_out − T_zone)
// HPXML 4.2 §3.8.2: FoundationWall/Thickness default unit is "in" when Units absent.
```

Not acceptable:
```rust
// from ASHRAE
// standard value
// EnergyPlus does this
// per spec
```

### Scope

This applies to all new code, all bug fixes, and all modifications to existing formulas or defaults. It also applies retroactively when you encounter unverified or uncited formulas in code you are modifying — if the formula is in a file you are touching, verify it and add the citation. Leaving an unverified formula in a file you have edited is a broken-window violation.

---

## Physics and numerical quality

### Quality bar

HARES is new code built for correctness, not a port of any existing tool. The standard is **physics done right** — not parity with any particular codebase for its own sake.

### Scope: residential load profile simulation

HARES targets **residential load profile simulation** — single- and multi-family homes, single thermal zones or simple multi-zone configurations, and the residential equipment categories that dominate those loads: HVAC, water heating, envelope, appliances, EVs, and distributed storage. This scope is narrower than EnergyPlus (which covers arbitrary multi-zone commercial and industrial buildings and the full equipment taxonomy that goes with them) and narrower than OCHRE (which attempts the same residential scope but with less rigour).

Within that scope, **HARES must meet or exceed OCHRE** in feature coverage, physical accuracy, and numerical correctness. OCHRE is the floor, not the ceiling. Every residential sub-model that OCHRE implements — infiltration, envelope RC network, psychrometrics, equipment capacity curves, schedules, HPXML parsing — must be implemented in HARES at least as completely, and with fewer known deviations from the authoritative standards.

Where EnergyPlus implements the same residential sub-model, **HARES targets EnergyPlus-level correctness**: same governing equations, same coefficients, same psychrometric basis, same reference temperatures. EnergyPlus is the closest available benchmark for "physics done right at simulation fidelity." The goal is not to reproduce EnergyPlus's C&I complexity — it is to reproduce its residential physics accuracy.

**What is explicitly out of scope:**
- Multi-zone commercial and industrial building simulation
- Commercial HVAC equipment (VAV systems, chillers, cooling towers, commercial boilers)
- Industrial process loads
- Detailed daylighting and radiance calculations
- District energy systems
- Complex geometry and shading for non-residential structures

### Reference implementations in strict order of authority

1. **ASHRAE standards** (HoF, 90.1, 152, 140) — the primary authority
2. **EnergyPlus Engineering Reference** (`docs/eplus/`) — narrative and equations grounded in the standards
3. **EnergyPlus source code** (`vendors/EnergyPlus/src/`) — authoritative when docs are ambiguous; `vendors/EnergyPlus/tst/` for test cases
4. **OCHRE** (`vendors/OCHRE/`) — a cross-check and feature-coverage baseline; useful for discovering what residential choices were made and why, but OCHRE deviates from standards in documented places (e.g. h_fg at 20°C instead of 0°C) and those deviations are bugs in OCHRE, not targets for HARES to replicate
5. **HPXML 4.2 data dictionary** (`docs/hpxml/`) — authoritative for HPXML element structure, field names, default units, and enumerated values; generated from the XSD schema at `hpxmlwg/hpxml @ v4.2`

Use OCHRE as a minimum-coverage bar and a sanity check on direction, not as a specification. Where OCHRE does something wrong — deviates from ASHRAE without justification, uses a suboptimal approximation, silently substitutes a default — do it correctly in HARES and document the divergence. There are no legacy paths to maintain and no back-compat constraints. Do it right.

### Loud errors at boundaries

Parse and init boundaries must fail loudly on invalid or unrecognised input:
- Return `Err(...)` with a typed error that names the field, the value received, and the expected form.
- Never silently substitute a default for an unrecognised value. If a default is genuinely safe and intentional, document the source of the default and its value inline.
- HPXML fields with adjacent unit elements must be unit-guarded before the numeric value is accepted.

### Constants

Physical constants are defined once in `hares-physics::constants` and imported everywhere they are used. Never hardcode a physical constant inline — not even an "obvious" one like 1000 J/kJ or 3600 s/h. Every constant must have a comment citing its source and reference temperature or condition where relevant.

This applies to **all** constants: physical, conversion factors, geometric, and computational. No magic numbers in expressions, struct initialisers, or default parameters. If you see `5.678`, `0.0929`, or `4186.8` inline, move it to the constants module and give it a name and a source citation.

### Units & Conversions

HARES operates in SI internally. Imperial US units enter only through HPXML files and are converted to SI at parse time before any physics code touches them.

**Conversion functions** live in `hares-physics::units`. Every conversion function is backed by the `uom` crate (well-tested, type-checked, sourced conversion factors). In the rare case that `uom` lacks a required unit, the manual factor must cite its source (NIST, ASHRAE, etc.) and live in `hares-physics::units` — never recalculated at the call site.

No conversion factor — not even `9.0 / 5.0` — may be written inline outside of `hares-physics::units`. Import `units::temperature_f_to_c`, do not write `(f - 32.0) * 5.0 / 9.0` in business logic.

**HPXML parsing** (`hares-io`) must:
1. **Check for a `units` attribute** on every numeric HPXML field that can carry one. If present, validate it as a recognized HPXML unit.
2. **Assume the HPXML spec default unit** when the `units` attribute is absent — not guess, not silently accept "close enough" values, not skip conversion. Every unitless numeric field in HPXML has a defined default unit in the HPXML specification; that default is the contract for the absent-`units` path.
3. **Reject unrecognised unit values** with a typed error naming the field, the unrecognised value, and the set of accepted values.
4. **Convert to SI immediately** after validation. The downstream type (struct field, constructor argument) holds an SI quantity. No HPXML element's raw value should propagate past its parse function.

Before writing or reviewing any HPXML parser, consult `docs/hpxml/`:
- `hpxml-elements.md` — full XPath of every element, its cardinality, and which `Unit Type` sibling applies (grep the element name)
- `hpxml-units.md` — unit vocabularies and the cross-reference table mapping element paths to their unit enum and allowed values (e.g. `AnnualHeatingEfficiency → HeatingEfficiencyUnits → AFUE | COP | HSPF | HSPF2 | Percent`)
- `hpxml-enumerations.md` — all enumerated string fields and their complete allowed-value lists
- `hpxml-data-types.md` — numeric constraints (min/max/pattern) for every type

These files are grep-friendly. The element table in `hpxml-elements.md` has one row per element with its full XPath — `grep "AnnualCoolingEfficiency" docs/hpxml/hpxml-units.md` will tell you the exact unit enum and values in one hit. Regenerate with `uv run python3 docs/hpxml/fetch_hpxml_dd.py` when the schema version changes.

**API surface** — every public function, struct, and type in `hares-*` crates other than `hares-io` uses SI units. If a function signature accepts a raw `f64`, the doc comment must name the unit and it must be SI. If the unit is ambiguous (e.g. mass flow vs. volume flow), use a `uom` newtype. Callers outside `hares-io` must never supply imperial values.

---

## Testing

### What to test

Test observable behaviour — inputs, outputs, visible state transitions, port contributions. Do not test internal wiring. A test that breaks on a safe refactor is a broken test.

**The refactoring criterion.** A legitimate behaviour-preserving refactor — renaming a private function, reordering a computation, extracting a helper, changing a log message — must never break a test. If a test would break under such a change, it is testing the wrong thing. Delete or rewrite it.

Prefer integration tests that exercise real paths end-to-end over unit tests that mock every collaborator. One integration test that exercises parse → construct → simulate → assert often replaces five mocked unit tests and gives more signal.

Every bug fix must be accompanied by a regression test that would have caught the bug before the fix. The test asserts correct post-fix behaviour — not merely that the code compiles.

### What not to test

**Log output is never a valid test target.** It is never acceptable to assert on `tracing` output, captured log strings, `warn!`/`error!` message text, or any other diagnostic emission. Log messages are implementation detail — they communicate intent to a human operator, not a contract to a caller. They change wording, gain or lose fields, move between log levels, and get removed entirely during legitimate refactors. A test that captures and asserts on log output breaks on every such change while providing zero signal about whether the simulation is correct.

The correct approach: if a condition is important enough to verify in a test, make it observable through the return value or the output state. Return a typed error instead of logging a warning. Surface a field in the output struct. Emit a port contribution that can be inspected. If you cannot make the condition observable through a real interface, that is a signal the interface needs to be improved — not a reason to reach for log capture.

Specifically forbidden:
- Any test that uses `tracing_test`, `tracing-test`, `tracing_subscriber` test utilities, or any other mechanism to capture and assert on log output
- Any test that asserts on warning or error message text produced by `warn!`, `error!`, `info!`, or `debug!`
- Any `assert!(output.contains("some log string"))` or equivalent pattern
- Any test written primarily to verify that a `warn!` or `error!` call was reached rather than verifying the computation's output

### Test naming

Name tests after the behaviour they assert, not the ticket or PR that introduced them. Tickets are ephemeral; test names are permanent.

Good: `cfm50_unit_rejected_as_ach50`, `dehumidifier_latent_round_trips_through_humidity_solver`
Bad: `test_bug_fix`, `ticket_052_regression`, `test_infiltration`

### Validation layers

HARES has multiple validation layers — run the relevant ones before marking work done:

| Layer | Command |
|---|---|
| Unit + integration | `cargo nextest run --workspace` |
| Clippy (zero warnings) | `cargo clippy --workspace -- -D warnings` |
| Formatting | `cargo fmt --check --all` |
| OCHRE parity | `cargo nextest run -p hares-core ochre_parity` |
| BESTEST | `cargo nextest run -p hares-core bestest` |
| Conservation laws | `cargo nextest run -p hares-envelope conservation` |

All three gates (test, clippy, fmt) must pass before any work is considered done.

---

## Code commentary

Comment the *why*, not the *what*. Well-named identifiers explain what — comments explain the non-obvious reasoning behind a decision.

### When to comment

- Physical constants and formulas: always cite the source.
- Numerical choices (tolerances, iteration counts, clamping values): explain why this value.
- Deliberate divergence from a reference implementation: state what HARES does, what the reference does, and why they differ.
- Non-obvious algorithmic decisions: explain the constraint that drove the choice.

### Comment style

Comments must be self-contained. A reader of the source code — including one who has never seen a ticket, PR, or issue — must be able to understand the reasoning from the comment alone. Never write "see ticket", "see PR", or "see issue". The reason lives here, in the code.

Examples of good comments:
```rust
// ASHRAE HoF 2021 Ch.1 Eq.30: h_fg = 2501 kJ/kg at 0°C reference temperature.
// Using the 0°C reference (not 20°C ≈ 2454 kJ/kg) for internal consistency:
// all moisture balance paths use this value so the latent→humidity round-trip
// is error-free. OCHRE Humidity.py:9 uses 2454 kJ/kg, producing a 1.9% systematic
// moisture mass error; HARES intentionally diverges here.
const H_FG_0C_J_KG: f64 = 2_501_000.0;

// AIM-2 infiltration model: Sherman & Grimsrud (1980).
// n = 0.67 is the pressure exponent for residential construction per ASHRAE 152-2004 §5.
let natural_ach = ach50 / (reference_pressure / 50.0_f64).powf(0.67);
```

---

## Hot-path discipline

The simulation loop runs ~525,600 timesteps per simulated year at 60 s resolution. Per-timestep allocation compounds at fleet scale.

- **No per-timestep heap allocation.** Pre-allocate buffers at init; swap/zero in-place each step.
- **Fixed-size stack arrays** for small, bounded collections (zone category breakdowns, surface lists).
- **Column-major indexed lookup** for weather and schedule time series — one `Vec<f64>` per field, accessed by direct index.
- **Pre-computed index maps** at init for all B-matrix/C-matrix position lookups. No string parsing or dynamic dispatch in the hot path.
- If you introduce a per-timestep allocation, document it in the component's allocation summary and justify it.

---

## Git discipline

- **`git stash` is never allowed.** Not under any circumstances. Stashing is used to hide failing tests and broken state — to make `cargo test` appear green by concealing the work that hasn't been done. This is a form of dishonesty. If tests are failing, fix them. If work is incomplete, document it under `### Known Limitations` and leave the code in a committable state. There is no scenario in this project where stashing is the right answer.
- If you encounter pre-existing failing tests in code you are modifying, fix them. You are responsible for the state of every file you touch.
- **Non-compiling tests must be commented out** (with a comment explaining why and which fix will reinstate them) until the associated fix lands. A test that prevents `cargo nextest run --workspace` from compiling is worse than no test — it blocks everything else.
- **Tests that compile but are expected to fail because they prove a bug exists must be marked `#[should_panic(expected = "...")]`** — not `#[ignore]`. This is only valid for bugs that are **genuinely out of scope for the current ticket** (e.g. they require a separate crate restructure that has its own ticket). It is **never** valid to use `#[should_panic]` to avoid fixing code that is in scope, to mask a test failure caused by an incomplete fix, or to work around unrealistic test behaviour. The `expected` string must describe the failure precisely so the annotation self-destructs when the bug is fixed. An ignored test proves nothing; a `#[should_panic]` test documents the known failure and alerts the team when the fix lands:
  ```rust
  #[test]
  #[should_panic(expected = "dehumidifier uses h_fg = 2454000 J/kg but humidity solver uses 2501000")]
  fn dehumidifier_h_fg_matches_humidity_solver() {
      // proves the bug exists; remove #[should_panic] once the fix lands
      assert_eq!(DEHUMIDIFIER_H_FG, HUMIDITY_SOLVER_H_FG);
  }
  ```
- Commit logical units of work. One concern per commit.
- Never force-push to a shared branch.

---

## File editing discipline

Agents must make precise, targeted edits to individual files. Never use bulk scripted rewrites — sed pipelines, Python `str.replace` over entire directories, awk — to modify source code or configuration. These tools pattern-match without understanding context and silently corrupt files in ways that are hard to detect and expensive to unwind.

The rule: **read the file, understand the code, make the smallest correct change**. If the same fix must be applied across many files, apply it to each file individually with a specific edit that has been verified against that file's actual content. Tedious is correct. Clever is dangerous.

Specifically forbidden:
- `sed -i` or `perl -pi` applied to source files
- Python/shell scripts that read and rewrite files by string substitution
- Any bulk operation that modifies more than one file per tool call without reading each target first
- Generating code by string interpolation from a template and writing it wholesale over existing files

---

## Review discipline

Reviewers must fix rather than request. If the only remaining issues after an implementation pass are trivial — wording, comment phrasing, a variable name that could be clearer — the reviewer fixes them inline rather than bouncing the ticket back for another round-trip. Reserve review rejections for substantive problems: wrong physics, missing tests, architectural violations, broken correctness. A round-trip has a real cost; spend it only when the problem cannot be *trivially* fixed in place.

Reviewers must also check for and reject untracked deferrals. Any `### Known Limitations` entry, code comment, or implementation note that describes doable work deferred to "future work," "a later ticket," "a follow-up pass," or any similar phrase without a concrete ticket ID is a substantively incomplete implementation. The reviewer must either:
- Reject the ticket and require the deferred work to be completed before `done`, or
- Require a tracking ticket to be created in `.shipwright/initiatives/` and referenced from the Known Limitations entry before accepting.

A half-fix with a `// TODO` is not ready for review. A placeholder default with "should be configurable later" is not ready for merge. Reject these on sight.

---

## Artifact alignment

Flow artifacts (explore → research → spec → implement → review) form a chain. Keep it coherent:

1. **No duplication — reference instead.** When a prior artifact already covers something, reference it by path and section. Do not repeat it.
2. **Upstream contradiction resolution.** If your work contradicts a prior artifact, update the prior artifact. A chain where two artifacts disagree silently is worse than no chain at all.
