---
description: "Test strategy for a change: choosing the level a behaviour is tested at, what to double and what to keep real, determinism and flake resistance, coverage judgement, and what not to test at all — stack-agnostic; framework syntax belongs to the project's tool-specific concern"
---

# Concern: Testing

Applies when a change adds, modifies, or reviews test code, or touches behaviour
that has none. It covers **which tests a change should have and at what level**.
Whether an individual test is worth keeping — behaviour versus implementation,
refactor survival, the seam rule — belongs to the test-quality concern.

This document is language- and framework-independent: it states outcomes, and
applies whatever the project's stack. Code blocks illustrate a rule; they are not
a mandate to use that language or library. Translate the shape into whatever the
project already uses, and never introduce a new test tool in a feature ticket.

## References

- The Practical Test Pyramid (Fowler): https://martinfowler.com/articles/practical-test-pyramid.html

## Choosing the level

Test each behaviour at the **cheapest level that gives real confidence in it**, and
never push a genuine integration risk down into mocks. That is the whole rule; a
target distribution across levels is not, and a suite shaped to hit a ratio is
shaped by the wrong thing.

- **Unit** — logic that is decidable from its inputs: rules, transformations,
  parsing, error classification. Fast, no real I/O. This is where most *behaviour*
  is cheapest to pin down, which is why suites usually end up with many of them —
  a consequence, not a quota.
- **Integration** — behaviour that only exists when parts are connected: a handler
  against a real store, a service against a real cache, configuration reaching the
  code that acts on it. If the risk is in the connection, a unit test with a
  double cannot see it, however many you write.
- **End-to-end** — the real system driven from the outside, through the entry point
  production uses. Slowest and most fragile, so reserve it for the journeys whose
  breakage matters most — but do not let its cost push a wiring risk down a level.

The complementary question — whether *anything* in the suite exercises the real
composition — is the integration-testing concern's.

## Test philosophy

**Test observable behaviour, not implementation details.** A test that breaks when
you rename an internal function, without changing behaviour, is a bad test.

**Tests are documentation.** A failing test should say exactly what contract was
violated.

**One concept per test.** A test asserting ten things tells you something broke,
not what the intended behaviour was.

**Tests are independent and order-agnostic.** Shared mutable state between tests
produces flakes and ordering dependencies.

## Test doubles — use the right one

| Double | Definition | Use when |
|---|---|---|
| **Stub** | Returns a fixed canned value | You need a dependency to return specific data |
| **Mock** | Records calls; verifies interactions | The interaction itself is the observable effect |
| **Spy** | Wraps real implementation; records calls | You want real behaviour plus call verification |
| **Fake** | Lightweight working implementation | In-memory store, filesystem, message sender |
| **Dummy** | Satisfies a parameter; never used | Filling required args a test doesn't exercise |

Double only at **I/O and process boundaries**: network, database, filesystem,
clocks, randomness, external processes. Never double an internal collaborator
within the same module — drive it through the public entry point instead.

**Prefer fakes over mocks for repositories and data stores.** A fake asserts the
contract (correct data in, correct data out) rather than the call sequence, and
survives refactoring.

**Double the transport, not the library on top of it.** Mock the network at the
HTTP boundary rather than stubbing the internals of a data-fetching or client
library; stubbing the library leaves its own behaviour — caching, retries,
serialisation — untested and re-implemented by the test.

## Structure — arrange / act / assert

Set up state, perform one action, assert the observable outcome, with the three
phases visibly separated. If the arrange section dwarfs the other two, it wants
extracting into a shared factory or fixture.

## Determinism — clocks, randomness, external state

**Inject time, randomness, and ID generation** — never call the ambient clock,
random source, or ID generator directly inside business logic. Inject without a
production default, so the wiring is visible at every construction site: a
parameter defaulting to the real collaborator is supplied only by tests, which
leaves the dependency ambient in production while the suite stays green. (The
test-quality concern calls this the seam rule and covers it in full.)

```typescript
interface Clock { now(): number; }
class SystemClock implements Clock { now() { return Date.now(); } }
class FixedClock  implements Clock { constructor(private ts: number) {} now() { return this.ts; } }

// Production: new ReportService(new SystemClock())
// Test:       new ReportService(new FixedClock(1700000000000))
```

## HTTP-facing behaviour

Where the change exposes an HTTP surface, drive it through a real client against
the real application object rather than calling the handler function directly, and
assert status code *and* response shape — a status-only assertion passes when the
handler returns the wrong data. Cover the error statuses, not just the success one.

## Property-based testing

Where a function has an invariant that must hold across a whole input domain —
commutativity, round-trip encode/decode, output always matching a shape — assert
the invariant over generated inputs rather than three hand-picked examples. Use
whichever generator library the project already has, and reach for this only where
the invariant, not the example, is the specification.

## Coverage judgement

Coverage is a **lower bound**, not a goal. Full line coverage means every line ran,
not that any edge case was tested.

Coverage is worth pursuing on business logic and domain rules, error paths and edge
cases, concurrency/cancellation/error propagation, and security-sensitive paths. It
is not worth pursuing on framework glue, generated code, or trivial accessors.

Where the project's language has a mutation-testing tool, its score is a better
signal than line percentage: a line executed by a test that would still pass if the
line's behaviour changed is not covered in any meaningful sense.

## What NOT to test

- Framework behaviour you didn't write — its request validation, row mapping, route matching
- Trivial accessors with no logic
- Third-party library internals
- Implementation details that don't affect observable output

## Anti-patterns

| Anti-pattern | Correct alternative |
|---|---|
| Sleeping for a fixed duration to wait for async work | Fake clocks; poll with timeout; event-driven waits |
| Shared mutable state between tests | Fresh state per test |
| Asserting call counts on internal helpers | Assert observable output and effects |
| Over-mocking — everything doubled, nothing real exercised | Fakes; doubles only at I/O boundaries |
| Assertions loose enough to pass on wrong values | Assert exact expected values |
| Flaky tests tolerated | Fix the root cause or delete; never ignore |
| No test for error paths | Every error case has a test |
| One giant test per feature | One concept per test |
| Asserting a double was called, but not what happened afterwards | Assert the downstream observable effect |
| Injected dependency whose only non-default supplier is a test | Remove the default; wire it explicitly at every construction site |
| Doubling the framework's internals to intercept a call | Double the process or network boundary underneath it |
| Coverage exclusions added to hide untested code | Test it, or record the gap explicitly where the project tracks gaps |

## Test-infrastructure patterns

Where the suite needs a shared heavyweight resource — a database container, a search engine, a message broker — the way that resource is provisioned and shared across tests determines whether the suite is fast, reliable, and parallelisable. The patterns below are stack-neutral; translate them into whatever test framework the project uses.

### One shared session-scoped container, coordinated by a lock

A container that takes seconds to start should start once per test session, not once per test. Coordinate the start across parallel workers with a file lock: the first worker acquires the lock, starts the container, writes its connection details to a known location, and releases the lock. Subsequent workers acquire the lock, read the details, and connect to the already-running container.

- The lock prevents a thundering herd where every worker starts its own container.
- The shared details file is the contract between the starter and the joiners — write it atomically (temp file + rename) so a reader never sees a partial write.
- A container that is already running (detected by a health check) short-circuits the start — the lock is still acquired to read the details, but no new container is spawned.

### Per-worker database namespaces

Parallel workers sharing one database must not see each other's writes. Assign each worker a logical database namespace — a separate schema, a separate logical database index, or a prefixed table set — derived from the worker's identity (e.g. `worker_id` in pytest-xdist, the process PID).

- A namespace collision between two workers produces interleaved test data and unpredictable failures.
- The namespace is created at worker startup and torn down at worker exit. A worker that crashes without cleanup leaves stale namespaces — the reaper (below) handles that.

### Lazy fixtures so the unit lane never pays for containers

Not every test needs the container. A unit test that exercises pure logic should not wait for a database to start. Gate container-dependent fixtures behind lazy initialization — the container is not started until a test that actually needs it runs.

- A session-scoped fixture that eagerly starts a container at collection time penalises every test run, even `--co` (collect-only) or a single-file run.
- Lazy initialization means the container starts on first use, not at session start. A test run that touches no database-dependent code never starts the container.

### Explicit lifecycle: refcount, reaper disabled, stale-container cleanup

- **Refcount for teardown.** Track how many workers are using the container. The last worker to disconnect tears it down; earlier disconnects leave it running for the remaining workers. A refcount stored alongside the connection details (in the same shared file) survives worker crashes.
- **Reaper disabled or guarded.** Some container runtimes have a reaper that removes idle containers. Disable it for the test container, or guard it with a label the reaper ignores — a reaper that removes the test database mid-session causes cascading failures.
- **Stale-container cleanup.** A container from a previous crashed run can linger and conflict with the new one. At startup, check for a container matching the expected label/name and remove it before starting fresh, or adopt it if it is healthy and the right version.

## Ecosystem-specific guidance

Test-framework syntax, fixture and mock semantics, timer hygiene, and query APIs
belong to the concern for the tool the project actually uses — apply whichever of
those is attached to the ticket, and follow the project's existing setup where none
is. Nothing in this document overrides them; where they differ in detail, the
tool-specific concern is authoritative for its own tool.

## Reviewer checklist

- [ ] New behaviour has tests, covering error paths as well as the happy path
- [ ] Each behaviour is tested at a level that can actually see its failure mode
- [ ] Concurrency, cancellation, and error-propagation paths tested
- [ ] No fixed-duration sleeps — deterministic async patterns used
- [ ] Tests isolated — no shared mutable state, no order dependence
- [ ] Doubles only at I/O boundaries; fakes for complex collaborators
- [ ] Exact expected values asserted — no assertion loose enough to pass on wrong output
- [ ] Clock, randomness, and ID generation injected, with a production wiring site for each
- [ ] Deleting the feature under test would turn at least one of its tests red
