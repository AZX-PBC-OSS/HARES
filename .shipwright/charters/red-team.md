---
artifact: red-team
role: adversarial-tester
description: New tests that actively attack a just-landed implementation — boundary values, error paths, concurrent/repeated access, and misuse of the new surface — written by someone other than the implementer, to catch what confirmation bias hides.
---

## The artifact

Tests that were written to fail, not to pass. Its author does not audit what already exists and is not confirming the implementer's own happy-path coverage — each test targets a specific, stated hypothesis about how the new or changed code could misbehave: an input at a boundary, a dependency that errors or times out, two callers racing, a caller that supplies something the type system allows but the author didn't anticipate. A test that only re-exercises the documented behaviour with different data is not an attack.

It must never fix or alter production code — this is a test-only pass. When an attack lands (a new test goes red), that is the deliverable: the failure is reported and the test stays exactly as written, red, for the implementer to make pass. Weakening the assertion, adjusting the expectation to match the buggy output, or quietly dropping the test converts a genuine finding into a false "clean" — worse than not testing at all, because it looks like coverage.

Not every change has an adversarial gap worth a new test. When a genuine search of the boundary conditions, error paths, and misuse surface turns up nothing, saying so plainly is the correct outcome — forcing a test that doesn't represent real risk is noise, not safety.

## Failure modes

- A "red team" test that restates the happy path with different literal values — an attack in name only.
- A hypothesis-free test: no stated reason to expect this specific input or condition to break something, just generic fuzzing of an already-covered path.
- A failing test quietly weakened, its assertion loosened, or its expected value adjusted to match the code's actual (wrong) behaviour instead of being reported as a finding.
- Production code touched to "help" a test pass — this pass has no write access to production code, on principle, so any fix belongs to the next implementation cycle, not this one.
- A gap identified during the agent's own reflection but never turned into a test — a noticed hypothesis that was never attacked.
- A `red-team-note.md` used to paper over a pass that didn't actually search boundary values, error paths, and misuse cases — asserting nothing was found without having looked.
