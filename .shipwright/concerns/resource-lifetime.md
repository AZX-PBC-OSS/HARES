---
description: "Resource lifetime: listeners, subscriptions, timers, open handles, connections, and spawned processes that are acquired and never released, or released only on the success path. Applies to any change that registers a callback or event listener, starts a timer or interval, opens a file/socket/connection, spawns a process, subscribes to a watcher, or adds an entry to a long-lived collection keyed per session, per request, or per connection."
triggers:
  content_patterns:
    - "addEventListener\\("
    - "\\.on\\(['\"]"
    - "setInterval\\("
    - "new AbortController\\("
    - "\\.subscribe\\("
    - "\\.watch\\("
---

# Resource Lifetime

One question drives it:

> **This acquired something — a listener, a timer, a handle, a process. Where, on every path out of this code, does it get released?**

Not the happy path alone. Every path: the one that returns early, the one that throws, the one that gets cancelled mid-flight, and the one where the whole process is shutting down while this was still open.

Applies to anything that hands back a "you own this now" value: a subscription or listener registration, a timer or interval, a file handle or descriptor, a network connection or pooled resource, a spawned process, a lock, a temporary file or directory, a watcher on the filesystem or on some other resource. It does not apply to values the language reclaims on its own with no explicit release step — an ordinary in-memory object with no external handle needs nothing here.

This defect class is invisible at the point of failure for the same reason a race is: the acquisition looks correct at its call site, review passes it, and a short-lived test process exits before the leak has anywhere to accumulate. It shows up later, in a long-running process, as memory or handle count climbing with no single stack trace to blame — one of the harder classes to diagnose after the fact, and one of the cheapest to prevent by construction.

## Discover how this project already manages resource lifetime

Before adding or judging anything, find out what the project already uses: its scoped-cleanup construct (a `with`/context-manager block, a `defer`, a `using`/`IDisposable`, a component lifecycle's cleanup return, a try/finally convention), its idiom for disposing subscriptions and timers, and whether it has any leak-detection tooling already wired in (a handle-count assertion in tests, a runtime warning promoted to an error, a linter rule for missing cleanup). Read a comparable existing acquisition and follow its shape. Introducing a second cleanup convention alongside the one already in use is itself a source of leaks — the two are rarely both remembered.

If the project has no leak-detection tooling and the change genuinely needs it, flag the gap. Do not set up new tooling inside a feature ticket.

## Acquisition and release are a pair

Every acquisition names its release before it names anything else. When reviewing or writing the code that acquires a resource, the first thing to establish is not what the resource does — it is which line releases it, and whether that line runs no matter how the surrounding code exits.

Enumerate the exits explicitly:

- **Success** — the obvious path, and the only one most code is written to actually reach.
- **Error** — an exception, a rejected promise, a non-zero exit, a returned error value partway through. If acquisition happens before something that can fail, and release happens only after that something succeeds, the failure path leaks.
- **Cancellation** — a caller abandons the operation, a request is aborted, a context is cancelled. The resource was acquired on behalf of work that will now never finish; nothing about "the work stopped" implies the resource was released.
- **Shutdown** — the process or component that owns the resource is torn down while the resource is still live. A per-window listener, a per-connection subscription, a per-session timer: each needs an owner-level teardown hook that sweeps whatever the owner is still holding, not just a happy-path cleanup at the end of one operation.

The common failure is not a missing release — it is a release that exists but is reachable from only one of these four paths. A timer cleared after a successful await and never before a failing one; a subscription torn down on normal exit but not on a forced kill; a listener removed when a component unmounts cleanly but not when it errors during mount. Each of these passes a review that only reads the success path, because the success path is correct.

## Prefer the language's own scoped-cleanup construct

Wherever the language or framework offers a construct that ties release to scope — so leaving the scope releases the resource whether by return, by throw, or by the block simply ending — prefer it to a hand-written release call. A hand-written release is only as good as every path in the function remembering to reach it; a scoped construct cannot be bypassed by an early return or an exception, because the runtime itself calls it on the way out. This is one idea with many names across ecosystems: a context manager, a `defer`, a `using`/`IDisposable` block, a try/finally, a component lifecycle's cleanup return, an RAII destructor. Use whichever one the project's language provides rather than re-deriving the same guarantee by hand.

Where the construct genuinely does not fit — the resource must outlive the scope that created it, for instance — the release still needs a single, unambiguous owner and a stated answer to "who calls this, and on which of the four exits."

## Long-lived collections are leaks in disguise

A map, cache, or registry keyed by session, connection, request, or subscriber is a resource in its own right, even though no single entry looks like one. The defect is symmetric with the one above: something adds an entry per event, and nothing removes it when the thing the entry represents goes away. Each entry alone is small; the collection is unbounded, and unbounded growth in a long-running process is exactly the same failure as a leaked handle, just diffused across many small allocations instead of one large one.

Ask, for any collection keyed by something that has a lifecycle of its own: what removes an entry when its key's lifecycle ends, and does that removal run on the same four exits — success, error, cancellation, shutdown — as any other release. A cache with no eviction policy and no upper bound is a leak that has not been named one yet.

## Make the runtime's own leak signal fail the build, not print to a log

Many runtimes already warn about exactly this — a listener count crossing a threshold, a process exiting with handles still open, a resource finalized by the garbage collector instead of by explicit release. Where the project's runtime offers a signal like this, treat a warning that only reaches a log nobody reads as equivalent to no signal at all. Where the tooling allows it, promote that warning to a test failure or a build failure so the leak is caught the day it's introduced, not months later in production telemetry. If no such signal exists in this ecosystem or this project has not wired it up, say so as a gap rather than leaving it implicit.

## Testing the invariant

The direct test of "does this release what it acquires" is cheap and concrete: acquire and release the same thing N times in a loop, and assert that whatever measures the resource — a listener count, an open-handle count, a process table, a connection-pool size — returns to its starting value afterward. This catches a leak that a single acquire/release test would miss, because a leak of one unit per call is invisible in a test that only calls it once.

Where the release path differs by exit (success vs. error vs. cancellation), test each exit separately with the same before/after count assertion — a test that only exercises the success path proves nothing about the error path's cleanup, which is usually where the leak actually lives.

## What NOT to flag

- A resource whose owner is the whole process and whose "release" is process exit — a global registered once at startup with no per-unit growth is not a leak.
- Values the language reclaims automatically with no explicit release step — ordinary objects, most primitives, anything with no external handle.
- A collection keyed by something that is itself bounded and small by construction (a fixed set of feature flags, a handful of named singletons) — unbounded growth is the defect, not the presence of a map.
- A short-lived script or one-shot CLI invocation whose process exit is itself the cleanup — the four-exits reasoning matters most for anything long-running or serving more than one unit of work.
- Test-only fixtures that leak within a single test run but are torn down between runs by the test framework itself, where the leak cannot accumulate across real usage.

A finding names the specific acquisition, the exit path that skips its release, and what accumulates as a result. Without those three it is a style preference.

## Footguns

- A timer or interval created before an `await` (or any point that can throw or reject), cleared only on the branch where that `await` succeeds.
- A subscription or listener torn down in a "normal exit" handler but not in a "killed", "timed out", or "app is quitting" handler.
- An event listener registered per-instance (per window, per connection, per component) on a target that outlives the instance, with no matching removal when the instance goes away.
- A cancellation token or `AbortController` triggered on the success path only — cancellation itself never releases what it was supposed to cancel.
- A `finally`-equivalent that releases the wrong variable — reassigned inside a retry loop, so the block releases the last attempt's resource and leaks every earlier one.
- A pool or cache entry acquired but never returned to the pool on an exception, so the pool silently shrinks until nothing is left to hand out.
- A scoped-cleanup construct wrapping only part of the code that uses the resource, so a failure outside the wrapped section still skips it.
- Reliance on a garbage collector or finalizer as the release mechanism for something that holds an OS-level handle (file descriptor, socket, process) — finalization timing is not guaranteed and is not a substitute for explicit release.
