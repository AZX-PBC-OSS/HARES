---
description: "Wiring and reachability: code that is written but never called, options and config fields with no production reader, deferred work that hides instead of failing, abstractions duplicated because an existing one wasn't found"
---

# Wiring and Reachability

Applies to any change that adds code, parameters, options, or configuration. The failure this concern targets is a whole feature that type-checks, lints, passes its tests, and never runs — because nothing in production ever reaches it. This defect class is invisible to every automated gate, so it must be caught by the implementer and the reviewer, deliberately.

It does NOT apply to code whose only purpose is to be a library surface for external consumers, or to a change that is purely a modification of an already-reachable path.

## Discover how this project composes itself

Before adding anything, find where the project assembles its parts: the entry points, the place dependencies are constructed and handed to the things that use them, where commands or routes or handlers are registered, where configuration is read and mapped onto behaviour. Read enough to know which of these your change must appear in for it to run at all. Follow the project's existing composition style — if collaborators are passed in at a single construction site, pass yours in there too; if they are looked up from a registry, register yours. Do not introduce a second composition mechanism alongside the one that exists.

## Every symbol you add must be reachable

For each function, class, method, module, field, parameter, option, and configuration key your change introduces, you must be able to name one of three things:

1. **The production call site** — the specific place, outside of tests, from which it is reached, transitively, from an entry point the system actually runs.
2. **Itself as an entry point** — it is the command, handler, route, or process the runtime invokes directly.
3. **An explicit, loud unfinished state** — see below.

If none of the three holds, the code is dead. Dead code that looks finished is worse than absent code: it makes the feature appear delivered, so nobody comes back for it.

## Ask the forward question, not only the backward one

The familiar completeness question is backward-looking: *"I changed this — did I update its callers?"* Tooling answers that one for you. The question tooling cannot answer is forward-looking:

> **"I declared this. What reads it?"**

Apply it to everything declared, not just to functions. A new command-line option with no code that acts on its value. A configuration field parsed into a structure nobody consults. A constructor parameter that is stored and never used. A field defaulted to an absent/empty value, guarded everywhere it is read by a check for absence, and never actually supplied at the one place production constructs the object — every guard passes, the feature never activates, and nothing anywhere reports a problem. Tests do not catch this, because a test constructs the object itself and supplies the value, exercising a state production never reaches — the seam rule in the test-quality concern covers the injected-dependency form of the same defect.

The mirror of the forward question is producer/consumer completeness: a value that is produced and never consumed is dead; a consumer wired to a producer that is never invoked is dead in the same way. Both ends must exist, and both must be on a path from a real entry point.

## Completing your own wiring is never out of scope

If your change produces something, connecting it to its consumer is part of your change — regardless of which file the consumer lives in and regardless of whether that file was named in the ticket's scope. A ticket's file list describes the expected shape of the work, not a boundary that may be used to leave a feature disconnected.

Deferring the connection to a future ticket is only acceptable when that ticket demonstrably exists and the deferral is recorded where it will be seen. "I did not wire this because the call site was outside my scope" is not a completed ticket; it is an unreported broken build with a green light on it.

## Deferred work must fail loudly

If something genuinely cannot be finished now, the placeholder must be impossible to mistake for a working implementation *at runtime*, not merely at read time. A placeholder that returns a plausible value — an empty collection, a default, a success indicator — is indistinguishable from working code to every caller, every test, and every operator. It will be integrated, exercised, and shipped, and the absence will surface much later as wrong behaviour with no trail back to the omission.

A correct placeholder terminates the operation with an error that names what is unimplemented, and has a test asserting that reaching it fails. That test is the only thing that keeps the placeholder honest: it makes the unfinished state a fact the suite carries, and it turns red the moment someone implements the real thing and forgets to remove it.

## Search before you build

Before introducing an abstraction — a helper, a wrapper, a client, a parser, a retry or validation or formatting utility — search the project for one that already does this. State in the change what you searched for and what you found or did not find. This is not ceremony: when several changes each solve the same underlying problem independently, they diverge, and when the shared choice is wrong it is wrong in every copy at once and must be fixed in every copy at once. Re-deriving a decision that already has a home in the codebase is a defect even when each derivation is individually correct.

If you find no existing home and the decision is one others will need, put it somewhere shared rather than local to your file.

## What NOT to flag

- Public library surface intended for consumers outside the repository.
- Interface members, protocol conformances, or overrides required by a framework contract even where this project does not call them.
- Code reached only through reflection, dynamic dispatch, plugin discovery, or registration by name — verify the mechanism exists, then it is wired.
- Test helpers used only by tests. That is their production.
- Genuinely dead code that predates the change and is outside its scope — note it, do not expand the change to remove it.
- A parameter with a default that production deliberately relies on, where the non-default path is a real supported configuration.

## Footguns

- An optional field that is never assigned at the one construction site production uses, guarded by absence checks that therefore always take the inactive branch.
- Registering something in a table, list, or map that is itself never read.
- Adding a configuration key to the schema and the documentation, and to nothing else.
- Wiring the new path in only one of several equivalent entry points (one command but not the other, one adapter but not its sibling), leaving behaviour that varies by how the system was invoked.
- Wiring a feature behind a flag that has no way to be turned on.
- Assuming a downstream ticket will connect your work. Confirm that ticket exists before relying on it, and say so in the change.
