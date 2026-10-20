---
description: "Correctness when more than one thing can run at once, or a sequence can be interrupted partway: check-then-act races, related writes with no all-or-nothing boundary, retried work that double-applies, and state shared between concurrent flows — the language-independent reasoning, not any particular engine's transaction, locking or isolation mechanics"
---

# Concurrency and Atomicity

Applies when a change writes state that anything else can also write, performs a sequence whose steps must not be seen half-done, runs unattended or on a schedule, can be retried, or shares anything mutable between requests, jobs, processes, or machines.

The defect class this concern targets is the one the other two defences do not see. A race reads correctly line by line, so review passes it. It only fails on a particular interleaving, so a serial test suite passes it too. It surfaces in production, intermittently, under load, as data that cannot be explained by any single execution of the code. That is what makes this worth a deliberate pass rather than leaving it to attention.

One question drives it:

> **What else can be happening between these two lines, and what happens if the process stops between them?**

## Discover how this project already coordinates

Before adding or judging anything, find how this project handles concurrent access already: where it groups writes into a unit that commits or rolls back together, whether it uses locks, queues, single-writer ownership, or leaves it to the store's constraints; how its background or scheduled work is triggered and what happens when a run overlaps the previous one; whether retries exist and whether the things they retry were built to tolerate it. Read a comparable existing operation and follow its shape. A change that introduces a second coordination mechanism alongside the one that exists creates the exact gap this concern is about — two things guarding the same state, neither aware of the other.

If the project has no mechanism and the change genuinely needs one, say so as a design question. Do not invent a coordination primitive inside a feature ticket.

## First establish that concurrency is possible

This concern is dogmatic in the wrong hands, so the applicability question comes before every rule below. Ask what actually reaches this state:

- Can two invocations overlap in time — two requests, two workers, two scheduled runs, two processes, a user double-submitting, a retry arriving while the original is still running?
- Can execution be interrupted between steps — a crash, a deployment, a killed container, a lost connection?
- Is the state durable and shared, or local to one invocation and discarded at the end of it?

If nothing overlaps and nothing outlives the call, a read-then-write is just a read-then-write and needs no protection. Say which of these applies when raising a finding; a finding that does not name the concurrent path is speculation and will teach the reader to ignore the next one.

Note that "single-threaded" is not the same as "not concurrent". A single-threaded process that yields at every await point, or one instance of a service that is deployed twice, or a script run from two terminals, all interleave.

## Check-then-act: the gap between deciding and doing

The shape is: read a value, decide something on the basis of it, then act on that decision. The decision is only valid if the value has not changed by the time the action lands, and nothing in that sequence guarantees it. It appears as verify-then-use, exists-then-create, read-a-balance-then-decrement, check-permission-then-open, is-it-free-then-claim-it, has-it-already-run-then-run-it.

Two overlapping flows both read the state that permits the action, both conclude they may proceed, and both proceed. Neither reports an error. The result is a double-spend, a duplicate record, a claim granted twice, or an operation performed against something that stopped satisfying the condition after the check.

**The fix is to make the decision and the action a single indivisible step, never to shorten the window between them.** A narrower window is a rarer bug, not a fixed one — and rarer is worse, because it survives testing and appears later at higher volume. What indivisible looks like depends on the layer:

- Express the condition inside the write itself, so the store applies it at the moment of writing, and check whether the write actually took effect. A write that silently affects nothing has told you the condition failed — ignoring its result throws away the entire protection.
- Let the store enforce the invariant, so no code path can violate it regardless of what any caller believes. A uniqueness or validity rule enforced where the data lives holds against callers that have not been written yet.
- Where the decision genuinely cannot be expressed as a condition on the write, take exclusive access for the duration of the read and the write together, and hold it across both.
- Where the operation is a claim on something, attempt the claim and handle refusal, rather than asking whether the claim would succeed and then making it.

Security-relevant variants of this shape — validating a path, permission, or identity and then acting on something that may have changed in between — are called out in the security concern too; the reasoning here is why the gap exists, that one is about what an attacker does with it.

## All-or-nothing boundaries for related writes

When several writes must all take effect or none of them, they need a boundary that guarantees exactly that. Without one, an interruption anywhere in the middle leaves state that no successful run and no failed run could produce: a record with no counterpart, a total that no longer matches its parts, a job marked complete whose output was never written. This is not only a concurrency problem — a crash between two writes does it with no concurrency at all.

Three failure modes are subtler than the missing boundary itself, and worth checking specifically:

- **A boundary that spans something it cannot undo.** Sending a message, charging a card, calling another service, writing a file — none of these roll back when the surrounding unit aborts. Keep irreversible effects outside the boundary, and make the state that records them recoverable: either the effect happens after the state commits and is retried until it does, or it is performed idempotently so a repeat is harmless. A boundary held open across a slow external call is also holding whatever it locked for that entire duration.
- **Announcing work before or after it exists.** Publishing an event, enqueueing follow-up work, or notifying a caller *before* the state it describes has committed produces consumers that look for something not there yet — or never, if the commit then fails. Doing it strictly after the commit, with no guarantee the announcement happens, loses the notification when the process dies in between. Which risk is acceptable is a design decision; making it accidentally is the defect. Name which one the code chose.
- **A "boundary" whose parts do not share one.** Steps that run against separate connections, sessions, or stores are not one unit however they are grouped in the source. This is easy to introduce accidentally by moving a step into a helper that acquires its own connection, and it looks correct in every diff.

The mechanics — how a boundary is opened, what an isolation level guarantees, which failures are retriable and how locks are requested — belong to whichever data-store concern applies to this project. This concern owns only whether a boundary is needed and where it belongs.

## Retries and idempotency

Anything that can be retried will be retried, and often not by you: a client resends, a queue redelivers, a scheduler fires twice, an operator re-runs a failed job, an infrastructure layer repeats a request whose response was lost. Delivery guarantees in practice are almost always at-least-once, so the safe assumption is that every handler runs more than once with the same input.

The question for any operation reachable by a retry: **if this runs twice with identical input, is the end state the same as running it once?** Setting a value is; adding to a value is not. Creating a record keyed on something the caller supplies is; creating one with a fresh key each time is not. Where an operation is not naturally repeat-safe, it needs an identity carried by the request that lets a repeat be recognised and turned into a no-op returning the original outcome — not a fresh attempt.

Retries and visibility are linked. A retry that succeeds silently hides a dependency degrading toward total failure, and hides the multiplied cost of the attempts it absorbed; a conflict error swallowed and retried forever hides that two flows are fighting over the same state. Whether that is visible is the observability concern's question — but the concurrency angle is that an invisible retry also means nobody finds out the operation was not repeat-safe until the damage is in the data.

## Shared mutable state

Ask of anything reachable from more than one flow of control: what happens if two of them touch it at once, and what state is it left in if one of them stops halfway? Candidates are the things that outlive a single call — a module-level or process-level cache, a singleton or client holding connection or session state, a value on a class rather than an instance, an accumulating buffer, a file or directory at a fixed path, an environment variable, a fixture everyone assumes is read-only.

Two shapes cause most of it. A value mutated in place while another flow is reading it, which sees a state that was never intentionally written. And a multi-step update to a shared structure that is observable between its steps, so a concurrent reader sees it half-updated. Both disappear if the shared thing is not mutated: replace it wholesale, keep per-flow copies, or give one owner sole write access and let everyone else ask it.

Global configuration mutated at runtime deserves specific suspicion — it is shared by definition, usually written from one place that assumes it is alone, and read from everywhere.

## A serial suite proves nothing about concurrent behaviour

Tests that pass one at a time, in a fixed order, in one process tell you nothing about a system that does none of those things. Two mechanisms make the class visible: randomised ordering exposes tests that depend on state a previous test left behind, and parallel execution exposes tests that depend on exclusive access to something shared. Where the project's test runner offers those, use them — the pytest concern covers the tooling in detail for that ecosystem; the same two mechanisms exist under other runners.

The finding to raise here is a step further than test hygiene, though: when the code under review has a genuine concurrent path, ask whether any test exercises more than one flow of control against it at all. Usually none does, and the reason the race survived review is that nothing was ever going to catch it. Where an invariant is enforced by the store rather than by application code, a test that drives the real store proves the invariant holds against every caller — including the ones a test double would have let through.

## What NOT to flag

- A read-then-write over state that only this invocation can reach, discarded when it returns.
- Code with a single writer by construction — one process, one scheduled run that cannot overlap, an ownership model that assigns each item exactly one handler. State the reason once; do not re-raise it per site.
- Append-only or write-once state, where a concurrent second write is either impossible or harmless.
- A missing all-or-nothing boundary around writes that are genuinely independent, where a partial outcome is a valid state.
- Idempotency on an operation nothing retries and whose caller cannot repeat.
- A deliberate, documented tolerance for a rare conflict where the cost of losing it is understood and lower than the cost of preventing it — that is a design decision, not a defect.
- Defensive locking on data that is never mutated after construction.
- Any race that requires a privileged local attacker to arrange, in a context that grants them nothing they did not already have.

A finding names the two flows that can interleave, the interleaving that breaks it, and the resulting state. Without those three it is a style preference and will be treated as one.

## Footguns

- Checking whether something exists, then creating it — where creating it is where the uniqueness rule should have lived.
- Reading a count, quota, balance, or seat total, deciding in application code, then writing the new value back. Grouping the read and the write into one unit does not fix it if the decision still happens in between.
- Issuing a conditional write and never checking whether it applied — all the protection, none of the enforcement.
- Holding exclusive access across a network call, so its latency becomes everyone else's wait, and its timeout becomes a stuck lock.
- Acquiring two exclusive things in different orders in different places — each site is correct alone, and together they can wedge.
- Releasing an exclusive claim only on the success path, so any failure leaks it permanently.
- A cached value populated on first use with no protection against two flows populating it at once — usually harmless, occasionally not, and always worth knowing which.
- Making a decision on state read before an await, suspension, or yield point, and acting on it after — the value was fresh when read and is not now.
- A background or scheduled task with no guard against overlapping the previous run when that run is slow.
- Deriving uniqueness from a timestamp, a counter held in memory, or a general-purpose random source, where two flows can produce the same value.
- Temporary paths, ports, or fixture names that are fixed rather than per-run, so two concurrent executions collide — in production and in the test suite alike.
- "It has never happened" as evidence of safety on a path whose volume has not yet been high enough to make it happen.
