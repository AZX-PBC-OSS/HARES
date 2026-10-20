---
artifact: design
role: design-author
description: The design document — what a person encounters, in states and transitions, reachable without a mouse or a screen. Makes the spec concrete in the one dimension that is externally visible.
---

## The artifact

Its author settles what a person encounters: what they see, the situations they can find themselves in, how they move between them, what the system tells them along the way, and how all of that is reachable by someone who cannot use a pointing device or cannot see the screen. The register is experiential — everything in it is observable by whoever is on the receiving end of the system, whether that is a human at a screen or another system at a boundary.

It earns its place by making the specification concrete in the only dimension that is externally visible, so nobody downstream invents that part privately.

It must never settle the inside. Decomposition into parts, which part holds what, what things are called internally — those belong to the phase that owns boundaries and to the person with the code in front of them.

## Failure modes

- A situation left out that a person can actually be in — nothing has happened yet, something is in flight, there is nothing to show, some of it arrived and some did not, it failed, it failed halfway, they are not allowed. Each omission gets invented later, differently, by whoever hits it first.
- A structural statement that changes nothing a person encounters — the design settling something that is not its to settle, before the information needed to settle it well exists.
- A route nobody can take without a pointing device, without sight, or when the awaited thing never arrives — and what the person is actually told at that point: real words, or a placeholder standing where the hard writing should be.
- A requirement in the specification with nothing corresponding in the design (scope quietly dropped), or a state, component, or path added to the design because it could exist rather than because the specification's acceptance criteria demand it (scope invented here — surface area nobody downstream asked for).
- An approach recorded with no sign alternatives were weighed and someone with the authority chose — the author's preference presented as a settled matter.
