---
description: "Performance: avoidable latency, unnecessary re-renders/re-computation, resource leaks, N+1 patterns, unbounded growth"
triggers:
  file_globs:
    - "**/benchmarks/**/*"
    - "**/*.bench.ts"
    - "**/*.bench.tsx"
    - "**/*.bench.js"
    - "**/*.benchmark.py"
    - "**/bench_*.py"
---

# Performance

This concern applies when a change touches code paths that affect observed latency, resource consumption, or scalability — data fetching, rendering, computation over collections, I/O, caching, and startup/shutdown paths. It does NOT apply to trivially-fast code with no users, no data, and no runtime cost worth measuring. Don't micro-optimize; do catch architectural performance problems.

## Discover the project's performance tooling

Before reviewing or implementing, check the project's configuration for performance tooling: profiling or benchmarking scripts, bundle analysis, performance testing frameworks, and any established performance budgets or CI gates. Read how existing code handles performance-sensitive paths (pagination, virtualization, lazy loading, memoization patterns). If the project has established patterns for hot paths, follow them. If the project has performance budgets or CI gates, ensure the change stays within them. If the project has no performance tooling, flag that as a gap — but do not set up profiling or benchmarking infrastructure yourself. That is project infrastructure, not per-ticket work.

## Measure before asserting

Performance findings should be grounded: "this code does X per item in the collection, which at scale N means Y." Avoid speculative findings ("this might be slow") without a mechanism. If the code is on a path with bounded input and the cost is trivial, it's not a performance concern.

## Common patterns to flag

### N+1 in loops

Fetching, querying, or doing I/O per item inside a loop. The per-item cost is typically small in isolation but compounds linearly. Flag the pattern and suggest batching.

### Unnecessary re-computation or re-rendering

Repeating expensive work on every invocation when the inputs haven't changed. In UI frameworks: re-rendering a subtree when props/state haven't changed (missing memoization or selector). In data processing: re-deriving a value that could be cached or computed incrementally. The fix is usually framework-appropriate memoization, incremental computation, or structural sharing — not hand-rolled caches.

### Unbounded growth

Data structures that grow without limit: caches without eviction, event listeners that are never removed, intervals that are never cleared, file handles or connections that are never released. These are not "slow now" problems — they are "degrades over time until failure" problems. Flag them as warnings even if the current scale makes them invisible.

### Blocking the main thread / event loop

Synchronous I/O, heavy computation, or large synchronous operations on paths that must stay responsive (UI threads, request handlers, IPC handlers). These cause visible jank or timeout failures. The fix: move work off the critical path (background threads, worker processes, async I/O, streaming) or break it into smaller chunks.

### Payload size and serialization cost

Transferring more data than the consumer needs. Selecting all columns when the UI shows three. Serializing large objects across process boundaries. Returning full documents when a summary suffices. The cost shows up as latency on the transfer path and GC pressure on the receiving end.

### Lazy loading opportunities

Loading a heavy module or resource eagerly when it's only needed conditionally (a settings screen, a rarely-used dialog, a large dependency for one code path). Code splitting, dynamic imports, and deferred loading reduce startup cost and memory footprint. Flag when a large dependency is imported at the top level but only used in a conditional path.

## What NOT to flag

- Micro-optimizations that save nanoseconds on a path that runs once.
- Theoretical performance concerns without a concrete mechanism or scale.
- Adding caching where the cost of the uncached path is trivial and the cache adds complexity.
- "This could be faster" without "and here's why it matters at the expected scale."

Performance findings must identify: the specific mechanism, the expected scale, and the user-visible impact. Without those three, it's speculation, not a finding.

## Footguns

- Adding a cache without an eviction policy — unbounded growth in disguise.
- Memoizing a function whose inputs are always new references (e.g. inline objects/arrays) — the cache never hits and adds overhead.
- Debouncing/throttling a user-facing action without considering whether the user needs immediate feedback.
- "Optimizing" a path that isn't the bottleneck — the actual bottleneck is elsewhere and now the code is more complex for no gain.
- Premature code splitting that fragments a small bundle — the overhead of additional requests exceeds the size savings.
