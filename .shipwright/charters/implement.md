---
artifact: implement
role: implementer
description: A production code change that fully implements a ticket — wired end to end in both directions, tested behaviourally, verified against the project's gates.
---

## The artifact

Working code that solves the problem the ticket describes, not the nearest easier one. The ticket describes the destination, not the road — a better approach found along the way is taken and recorded under `### Deviations`, not silently substituted or rejected on principle.

Every parameter, option, configuration field, flag, event, hook, and optional dependency this change declares needs both ends: the production code that supplies it and the production code that reads it. A required argument breaks its callers loudly; an optional one that nothing ever supplies breaks nothing, tests green, and ships doing nothing.

Tests prove behaviour a real caller depends on, not the test's own setup. A suppression, weakened assertion, or routed-around safety net is allowed only with a call-site `Why:` naming the specific mechanic that forces it — never "temporary," never unexplained. A limitation is a property of *working* code; code that does not do what the ticket asked is unfinished, not limited.

## Failure modes

- A declared option, field, flag, or hook with a default supplier and no real caller — dead surface that reads as finished because every check is green.
- A changed contract with a stale caller, a broken reference, or dead code left behind.
- A test that constructs the object *and* supplies the collaborator production never supplies — it proves its own setup, not the code. Ask: if this feature were deleted from production, which test goes red? Would a behaviour-preserving refactor break this test?
- A suppression, skip, broad catch, or weakened assertion with no `Why:`, a vague one, or one whose named constraint is not actually real.
- An optional parameter that exists so existing call sites need not be touched — the wiring left undone, not a design decision.
- A unit that grew because the ticket named it, rather than because the work demanded it — or one that should have grown and did not.
- A failure path indistinguishable from success, or a cause a handler discards, broadens, or strips before it reaches the person who has to act on it.
- Configurability, an abstraction layer, or hand-rolled machinery built past what the ticket's stated outcome requires — a general solution to a problem nobody has yet, or new code duplicating a library or pattern the codebase already uses for it.
