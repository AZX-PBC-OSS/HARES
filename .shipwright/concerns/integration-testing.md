---
description: "Whether the suite as a whole ever reaches the system through the entry point production uses: self-assembled dependency graphs that never exercise real composition, wiring and registration no test executes, doubles standing in for parts inside the system"
triggers:
  file_globs:
    - "**/tests/integration/**/*"
    - "**/test/integration/**/*"
    - "**/integration-tests/**/*"
    - "**/*.integration.test.ts"
    - "**/*.integration.test.tsx"
    - "**/*.integration.test.js"
    - "**/*.integration.spec.ts"
    - "**/test_integration_*.py"
---

# Integration Testing

Applies when a change adds behaviour that only works if several parts are connected correctly — anything with a composition step, an entry point, a boundary crossing, or configuration that must reach the code that acts on it.

This concern is narrow on purpose. What makes a single test good — behaviour over implementation, boundary-only doubles, earning its place, and the seam rule — is covered by the test-quality concern and is not restated here. Apply its two bracketing questions ("if this feature were deleted, which test goes red?" and "would a behaviour-preserving refactor break this test?") to the suite as a whole, and the question this concern exists for follows:

> **Is there any test that would have caught the parts never being connected?**

A suite can be uniformly excellent by every per-test standard and still answer no.

## Discover how this project reaches its own system

Find the tests, if any, that drive this system from the outside — the ones that start the real entry point, load real configuration, and use whatever the system is actually invoked as. Note how they set up their environment, what they substitute and what they keep real, and what the project already treats as an external boundary. Extend that layer rather than inventing a parallel one. If no such test exists anywhere in the project, that is the finding, and it is a significant one.

## A self-assembled dependency graph proves everything except the wiring

The most common shape of a passing suite over a broken system: the test constructs the objects, supplies their collaborators, and calls the thing under test. Every unit behaves correctly. The composition that production performs is never executed, so a component that production never connects, or connects with a value the test always supplied itself, is indistinguishable from a working one.

This is not a flaw in those tests — it is what unit tests are for. It is a flaw in a suite where *nothing else* exists. At least one test must reach the system through the same entry point production uses, and let the real composition happen: the real construction site, the real configuration load, the real registration of handlers or commands. Anything the test builds for itself is a part of the system that test is not covering.

This includes end-to-end tests. An end-to-end test that assembles its own graph before driving it is a large unit test — it is the slowest kind of test and it has skipped the only thing it was uniquely able to check.

## Doubles belong at true external boundaries only

A double is warranted only where the real thing is outside the system's control: a remote service, a third-party interface, a clock, a source of randomness, hardware. Everything inside the boundary is part of what the test is meant to exercise; replacing an internal collaborator removes exactly the connection an integration test exists to verify.

Where a double is warranted, prefer a real collaborator, then a working fake against the same contract, then a fixture, then a stub — in that order. A stub returns what the test says; a fake behaves.

## What NOT to flag

- Unit tests that use doubles, existing alongside integration coverage. That is the intended shape.
- Absence of an end-to-end test for a change with no composition step and no boundary crossing.
- Doubles at genuine external boundaries, however many — the boundary count is the design's, not the test's.
- Test-only construction helpers and factories. Those are test infrastructure, not production seams.
- Demanding a slow integration test for a case a fast one already covers at the same fidelity. Integration coverage is about reaching the real path once, not about repeating every case there.

## Footguns

- An end-to-end test that starts at the layer just below the entry point, skipping the one place composition happens.
- A test environment that loads a different configuration path than production, so a misconfigured production path passes.
- Substituting the component under test's own collaborators, leaving the test asserting only that the doubles were set up correctly.
- A fixture that supplies a value production never assigns — the test exercises a state the running system cannot reach.
- Registering the feature explicitly in the test setup, where production is supposed to register it automatically. The registration step is the thing most likely to be missing.
- Coverage measurement reporting the wiring as covered because a unit test executed the same lines by a different route.
- Marking an integration test as skipped or conditional on an environment that continuous integration does not provide — a test that never runs is not coverage.
