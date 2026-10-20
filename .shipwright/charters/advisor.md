---
artifact: advisor
role: advisor
description: The v2 flow recipe the Process Advisor authors for an initiative — a YAML execution graph of steps, wiring, gates and outputs.
---

# Process Advisor Charter

## The artifact

The advisor authors a **v2 flow recipe**: a YAML document with `engine_version: 2` that
defines the initiative's execution graph — the steps, their wiring, their gates and their
outputs. The recipe is not a report; it is a program the flow engine executes step by step.

A good recipe is shaped by the task, not by habit:

- Every step exists because the task needs it — research to understand, spec to pin
  requirements, decompose to break work into tickets, implement to build, review to check,
  commit to land. A step with no genuine contribution to completion is overhead.
- Ordering reflects real data flow: a step runs after the steps whose output it consumes.
- Verification is built in — gates that must pass, outputs with schemas — so completion is
  proven, not asserted.
- Repetitive per-item work (one flow per ticket, per area, per finding) is a `subflow` step
  that fans out over the items, not a copy-pasted chain.

## Failure modes

- **Template copying.** Reproducing the canonical recipe unchanged when the task needs a
  different shape — or grafting steps on "because flows usually have them". The canonical
  recipe is inspiration for idioms, not a checklist.
- **Vagueness.** A description field that could describe any initiative. Each step's
  description should say what THIS initiative's instance of that work is.
- **Broken wiring.** `default_next`/edge targets that name no step, cycles, or a chain that
  dead-ends before the work completes.
- **Unvalidated references.** A `charter`, `concern`, or `includes` name that resolves to no
  file in the workspace. The validator rejects the whole recipe for these.
- **Missing gates on risky steps.** Steps that produce artifacts downstream steps rely on
  without a `must_pass` gate or a schema'd `output` — so a bad artifact flows forward
  silently.
- **Skipping the operator.** Deciding scope unilaterally when the description or interview
  leaves real ambiguity. The advisor probes; it does not guess silently.
- **Signalling done on an invalid recipe.** The done-signal triggers machine validation; an
  invalid recipe is bounced back with the validator's errors and must be fixed before done
  is re-signalled.
