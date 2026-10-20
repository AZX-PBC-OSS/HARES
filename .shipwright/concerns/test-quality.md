---
description: "Whether an individual test is worth keeping: does it assert observable behaviour or internal structure, does it survive a behaviour-preserving refactor, does it die when the feature dies, and does the code need a seam rather than another mock"
triggers:
  file_globs:
    - "**/*.test.ts"
    - "**/*.test.tsx"
    - "**/*.test.js"
    - "**/*.test.jsx"
    - "**/*.spec.ts"
    - "**/*.spec.tsx"
    - "**/*.spec.js"
    - "**/*.spec.jsx"
    - "**/test_*.py"
    - "**/*_test.py"
    - "**/*_test.go"
    - "**/*_spec.rb"
    - "**/*Test.java"
    - "**/*Tests.cs"
---

# Test Quality

**Write as few tests as needed to fully exercise the code.** More tests is not better; more confidence is. Every test is permanent maintenance cost, so only write one if you can answer clearly: **what real failure would I catch if this test were removed?** If the answer is "nothing new," the test is performative — remove or rewrite it. Where several tests each double every collaborator, one test over the real path usually replaces all of them and proves more; which level a given behaviour belongs at is the testing concern's question, not this one's.

## Discover how this project tests

Before judging or writing a test, read the tests that already exist next to the code being changed: what they assert on, what they keep real, what they substitute, and what test infrastructure (factories, fakes, helpers) the project already provides. Follow those patterns rather than introducing a parallel style. Where the existing tests themselves show the defects below, say so — but fixing the whole suite is its own work, not silent scope creep on a feature ticket.

## Test behavior, not implementation

A test that knows about internals is not just bad style — it actively lies. It fails on safe refactors (no bug introduced, test breaks) and passes on real bugs (behavior wrong, internals unchanged). Both failure modes destroy trust in the suite.

Tests verify what the code does from the outside: inputs, outputs, side effects, visible state changes, emitted events. Tests do NOT verify how the code achieves it internally.

Two checkable questions bracket the failure modes. Both are decidable — apply them to a specific test and you get a yes or a no, not a judgement call:

> **A correct refactor that preserves behaviour must not break the test.** If it does, the test was asserting implementation.

> **If this feature were deleted from production, which test goes red?** A test that survives deletion of the feature it covers proves nothing.

The first catches brittleness: the test is welded to a structure rather than a contract, so it fails on safe changes. The second catches theatre: the test passes whether or not the behaviour exists, usually because it constructs and drives its own collaborators rather than the production path. Run both against every test you write — a good test survives a behaviour-preserving refactor and dies when the behaviour does.

**Concretely:** renaming a private function, splitting one module into two with the same combined behaviour, switching a `for` loop to a `map`, or rewording a routine log message — none of these are behaviour, and a passing suite must not move when any of them happen. If a test breaks on one of these changes, it was pinned to the *shape* of the code, not what the code does for its caller. (This is distinct from asserting that a specific error-level log line fires on a genuine failure path — that log line, when the project's logging convention makes it part of the contract, is itself observable behaviour, and its own tests are legitimate. The line drawn here is routine/informational logging whose wording is an implementation detail versus a log emission that a caller or operator is documented to depend on.) A suite that has to change every time the implementation does, without any change in what the code promises its callers, is a second implementation to maintain — it stops paying for itself and starts costing more than it protects.

Red flags:
- Asserting a specific internal function was called (call-count assertions, `toHaveBeenCalledWith` against private helpers)
- Reaching into private state via exported-for-test hatches
- Snapshots used as a proxy for behavior — they break on any refactor, even behavior-preserving ones, and pass silently when behavior is wrong but output format unchanged
- The test setup mirrors the implementation structure so closely that changing the implementation requires rewriting the test even when behavior is identical

The public API's observable behavior is the test surface. Nothing else.

## Mocks only at real boundaries

Mocks are for genuine cross-boundary effects: network, filesystem (when the real one is too expensive), external process spawning, platform APIs the test environment cannot provide. Internal collaborators within the same module or package are not boundaries — exercise both through the public entry point.

Over-mocking is the primary source of "tests pass, feature broken." If a function calls its own helpers, the test should drive the public entry point and verify the outcome, not mock the helpers.

**A double whose surface is hand-listed is a contract nothing verifies** — when the real thing is renamed, gains a member, or changes signature, the stand-in keeps answering for the old shape and the suite stays green, so derive a double's surface from the real thing wherever the tooling allows it.

Spying on internal collaborators to verify call sequences is a smell. Reserve spy/assertion-on-call for genuine cross-boundary effects (e.g. a logger was called with a specific error-level message in a failure path).

## Design for testability

When code is hard to test without mocking internals, the code needs seams — not more mocks. Patterns that make behavior testing natural:

- **Accept dependencies as parameters** — callers provide the real thing in production, tests provide fakes at boundaries
- **Keep side effects at module boundaries** — pure core, impure shell
- **Define clean contracts** — typed parameters, return types, event shapes so the public surface is the test surface
- **Interface-based boundaries** — test against the interface, swap implementations

> **A seam with a production default is not a seam — it is a hidden dependency plus a test-only parameter.** When a dependency is injected, remove the default so the production wiring is visible at the call site. Anything supplied exclusively by tests is dead in production.

This is the failure mode that "accept dependencies as parameters" invites if applied carelessly. A parameter that defaults to the real production collaborator is never passed by any production caller, so the only caller that supplies it is the test. The dependency is still ambient in production, the injection point is exercised by nothing real, and the suite is green either way. The same applies to an optional collaborator field guarded at every use site: if the single production construction site never assigns it, the guarded behaviour is permanently inert while its tests — which supply the collaborator themselves — pass. Type-correct, lint-clean, green, and dead.

```python
# The trap — a seam with a production default. No production call site passes
# `generator`, so the only caller supplying it is the test.
def create_token(generator: Callable[[int], str] = secrets.token_hex) -> str:
    return generator(32)

# Correct — no default. Every construction site names its dependency, so the wiring
# is visible in the diff and a missing wire is a hard error, not a silent fallback.
def create_token(generator: Callable[[int], str]) -> str:
    return generator(32)
```

The defect is a collaborator whose *only* non-default supplier is a test. A default that production genuinely relies on — where the non-default path is a real supported configuration — is not this. When reviewing, locate the **production** construction site for each injected dependency; if only tests construct it, the feature does not exist in production.

When a test can only verify behavior by asserting which internal helper was called, the code's structure is fighting the test. Restructure so the observable contract is enough.

## Every test earns its place

Coverage percentage is not the goal — confidence is. Signs a test is not earning its place:
- The test only works if the code is structured a specific way internally
- Removing the test changes nothing about the failure-detection capability of the suite
- The test duplicates an assertion already covered by a broader integration test
- The test asserts a trivially true condition (e.g. object construction, property assignment) with no failure path
- Deleting the production feature the test covers would leave the test green — it builds and drives its own version of the behaviour rather than the production path

## Failure paths and edge cases

Happy-path-only tests give false confidence. Every behavioral path has a test — including failure paths, error conditions, boundary values, and edge cases the code claims to handle. If the code has a `catch` or an `if` for an error condition, that path needs a test.

## Test-infrastructure quality

Where the suite uses shared infrastructure (containers, databases, brokers), the quality of the infrastructure setup itself is a finding surface.

- **A container that starts per test instead of per session is a slow suite, not a thorough one.** A database container that takes 5 seconds to start, started for each of 200 tests, adds 17 minutes for zero additional confidence. Session-scoped shared setup with per-test isolation (transaction rollback, namespace reset) tests the same thing in seconds.
- **A fixture that depends on external state the test does not control is flaky by construction.** A database row seeded by a previous CI run, a port a previous process grabbed, a temp directory with a hardcoded name — these are ordering and concurrency bugs in the infrastructure, not in the code under test. The fixture must create and own its state, or the test is borrowing trouble.
- **A teardown that does not run on failure leaves the suite polluted.** If the test errors before its cleanup block, the next test inherits the leftover state. Use framework guarantees (fixture teardown, `finally`, `addFinalizer`) rather than manual cleanup at the end of the test body — the framework runs teardown even on assertion failure.
- **A container that is never cleaned up accumulates across runs.** A CI runner that starts a container per job and never removes it fills the disk. Label containers so they can be identified and garbage-collected, or remove them in a session-scoped teardown.

## What NOT to flag

- A test that doubles a genuine external boundary — network, filesystem, clock, spawned process. That is the intended shape, however many appear.
- A default parameter production itself relies on, where the non-default path is a real supported configuration. The defect is a collaborator whose *only* non-default supplier is a test.
- Assertions on a cross-boundary effect (a specific error-level log line, a request actually issued) — those are observable behaviour, not internals.
- Snapshots of something whose exact serialised form *is* the contract, such as a wire format or a generated file.
- Absence of a test for a path with no failure mode of its own: a trivial accessor, generated code, framework glue.
- Several tests over the same function where each pins a genuinely different behaviour. Duplication is repeated *assertions*, not repeated subjects.

## Wrestling-with-tests signal

Watch for: iterative patches to the same test file, implementations peppered with narrow special cases matching test inputs one-to-one, assertions loosened, tests marked `skip`, expected values changed to match output. These indicate the author is driving tests to green rather than solving the problem — a critical finding.
