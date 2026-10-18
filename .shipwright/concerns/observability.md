---
description: "Observability and operability: silent failure, failure legibility, unbounded waits and invisible hangs, instrumentation proportionate to blast radius"
triggers:
  content_patterns:
    - "except\\s*:"
    - "except\\s+[\\w.]+\\s+as\\s+_\\s*:"
    - "catch\\s*\\(\\s*_+\\s*\\)"
    - "catch\\s*\\{\\s*\\}"
    - "catch\\s*\\([^)]*\\)\\s*\\{\\s*\\}"
    - "child_process"
    - "subprocess\\."
    - "\\.join\\(\\)"
---

# Observability

One question governs this concern:

> **When this breaks at 3am and someone is woken up, can they find out what broke and why, from what the system already produced?**

Not from what they could add afterwards, not from what they could reproduce in a debugger — from the artefacts the failing run already left behind.

Applies when a change spawns a process, crosses a network or process boundary, writes durable state, runs unattended, or degrades rather than failing outright. It does NOT apply to pure computation over already-validated inputs — a function that transforms data it was handed and returns a value needs no instrumentation, and adding some is noise.

This concern is about whether failures are *findable*. The mechanics of emitting a signal — structure, level discipline, redaction, correlation identifiers — belong to the project's logging conventions; follow those.

## About this concern's content triggers

The `content_patterns` above look for two shapes: a swallowed error (a bare `except`, an empty catch, an exception bound to `_`) and a wait that can hang invisibly (a subprocess spawn, a bare blocking join). They are shape matches, not judgements — a bare `except` immediately followed by a log and a re-raise is fine. They exist so this concern is read when the change plausibly affects what an operator can see when something goes wrong.

## Discover the project's existing signals

Find how this project already reports what it is doing: its logging or tracing setup, its metrics or error-reporting integration, its conventions for naming events and attaching context, where output is written and at what verbosity by default. Read a comparable existing operation and match it — the same event shape, the same context fields, the same level choices. A change that invents its own reporting style is a change nobody will find in the logs.

If the project has no signal mechanism at all, and the code you are adding runs unattended or crosses a boundary, flag the absence as a gap. Do not build logging infrastructure inside a feature ticket.

## Failure must be distinguishable from success

The defining property of a silent failure is that its output is byte-identical to a legitimate one. A lookup that fails and reports "no results" is indistinguishable from a lookup that succeeded and found nothing. A parse that fails and yields an empty structure looks exactly like a valid empty input. Every caller downstream behaves correctly on that value, so the failure never surfaces anywhere — it becomes an absence of behaviour, attributed later to the feature "not working" with no evidence pointing at the real cause.

The rule: a failure path may not produce a value that a success path could also produce, unless the failure is simultaneously reported through a channel that survives — an emitted signal, an error result, a recorded metric. Degrading to an empty or default value is a legitimate design choice; degrading *quietly* is not.

Watch the aggregate, not just the instance. One handler that maps a storage error onto an empty result is a local judgement call. Dozens of them across a subsystem is a subsystem that can be entirely non-functional while every caller reports success and every gate stays green. Concentrated degradation is a structural finding, and it is invisible if you only ever look at one file.

## Failures carry their cause

When you catch a failure and raise, wrap, or report a different one, the original must travel with it. Discarding the underlying cause — keeping only its text, or replacing it with a message of your own — destroys the one artefact that identifies where the failure actually happened. The stack that matters is the one you threw away.

Name the specific occurrence, not just the operation. "Failed to read configuration" is far less useful than the same message carrying which file, which key, and which attempt. At 3am, the difference between those two is the difference between a fix and an investigation.

Handle only what you have a response for. Catching broadly and reporting generically converts a precise, located failure into a vague one; if there is no action you can take, letting it propagate to a place that can act is the better outcome. A handler that exists only to make the failure quieter is a defect.

## Hangs must be visible

A crash is a *good* failure: it is loud, it is located, and it is over. The worst failure is a hang — the system consumes resources, produces nothing, reports nothing, and cannot be diagnosed after the fact because it never ended.

Every wait on something outside the process is bounded: a spawned process, a network call, a lock, a queue read, a retry loop, a poll for a condition. Unbounded means the operation can stop forever with no signal. Exceeding the bound must itself produce a signal that says what was being waited on and for how long — a bound that silently returns a default is a silent failure wearing a timeout's clothing.

Long-running unattended work needs enough periodic signal that a stalled run is distinguishable from a slow one. Retries need to be visible: a loop that recovers silently on the third attempt hides a dependency degrading toward total failure.

## Instrumentation proportionate to blast radius

Instrument according to what it costs to be blind, not uniformly. The things that most need signal are the ones whose failure is expensive and whose behaviour you cannot reconstruct: process boundaries and external invocations, anything that writes durable or irreversible state, anything running unattended where nobody is watching it fail, and every path that degrades instead of failing.

A subsystem that reaches outside the process and emits nothing at all is not a minor gap — it is a component that can only ever be debugged by modifying it first. Conversely, per-iteration signal inside a tight loop over in-memory data buys nothing and drowns the signals that matter. Proportionality cuts both ways.

## Reviewing the aggregate, not just the diff

Some of these defects only exist in the aggregate and cannot be seen one hunk at a time. Before signing off on a change that touches operational code, look across the component: how many failure paths swallow their cause, how many of the outward calls are unbounded, whether whole packages that reach outside the process emit nothing. A per-file review will pass every one of those individually.

## What NOT to flag

- Pure functions over validated inputs. Instrumenting them adds noise, not signal.
- Expected, handled outcomes reported at a routine level — a missing record on a lookup that is defined to allow misses is not a failure.
- Waits that are genuinely bounded by the operating environment rather than by the code, where the bound is real and discoverable.
- Absence of metrics or tracing in a project that has neither. Flag the gap once; do not raise it per file, and do not introduce the infrastructure in a feature ticket.
- Verbosity for its own sake — "this should log more" without naming a failure that would currently be undiagnosable.

Findings under this concern name a concrete failure and what the responder would be missing. Without that, it is a style preference.

## Footguns

- A default or empty value returned from a failure path, where the same value is a legitimate success result.
- Re-raising or wrapping without carrying the original cause — the location of the real failure is gone.
- Catching broadly around a large block: the precise failure becomes "something in here went wrong."
- A timeout whose expiry is handled by continuing with a default, so the wait is bounded but the failure is still silent.
- Spawning a process without bounding its runtime, or without capturing and reporting its error output and exit status — a process that fails and prints why is useless if nothing reads what it printed.
- Retry loops that never report the attempts they absorbed.
- Signals emitted only at a verbosity that is disabled by default in the environment where the failure happens.
- Instrumentation that fires on success and is absent on the failure path — precisely inverted from what is needed.
