---
artifact: fix-plan
role: fix-planner
description: A single-artifact fix plan for one small, self-contained, verifiable change — the chosen approach, why it was chosen over the alternatives, and how it will be verified, written before any code changes.
---

## The artifact

A fix plan commits to an approach. It does not restate the root-cause analysis or the reproduction — it reads them, then decides what to do about them and says why that approach beats the alternatives it considered and rejected. The plan targets the confirmed root cause, not the symptom the reproduction test exercises; if the two diverge, it says so and routes back rather than papering over the gap. It is scoped to one small, self-contained, verifiable change — the smallest correct fix, not the most thorough rewrite the root cause could justify.

The plan names its blast radius: which files and call sites the fix touches, what else could be affected, and what stays untouched on purpose. It states how the fix will be verified — the failing test that must go green, the passing commands that must stay green — using the paths and commands the diagnosis and reproduction already established, not ones invented for the plan. It never fixes anything itself; the fix step still owns every line of production code.

A plan is not a design document for infrastructure the root cause doesn't call for, and not a promise about code that doesn't exist yet. It describes the shape of the change at a level a plan can honestly commit to — which function or module changes and what its new behaviour is — without inventing names, signatures, or internal structure the implementer will discover once the code is in front of them.

## Failure modes

- A plan that restates the RCA's narrative instead of choosing an approach — a summary wearing a plan's clothes.
- No alternatives considered, or alternatives listed with no reason the chosen approach beats them.
- Scope creep: fixing more than the root cause requires, folding in adjacent cleanup or hardening the RCA never asked for.
- A plan that targets the reported symptom rather than the RCA's confirmed root cause — the failure mode diagnosis exists to prevent, recurring one phase later.
- No statement of how the fix will be verified — which failing test must pass, which passing commands must stay green — leaving the fix step to invent its own definition of done.
- Predicting exact function names, signatures, or internal structure the code does not have yet — a blind decision the implementer's context will contradict, the failure decompose.md's tickets are built to avoid.
- A blast radius left unstated: files and callers the change touches, named nowhere, so the fix step and the reviewer after it cannot check the change stayed inside it.
- A `plan_outcome: ready` verdict on a plan that itself finds the RCA's root cause wrong or unscopeable — that is `blocked`, not a plan to hand to the fix step anyway.
