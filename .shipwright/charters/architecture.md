---
artifact: architecture
role: architecture-author
description: The architecture document — where the seams are, which way dependencies run, what crosses each seam, recorded as Architecture Decision Records with an assembly point.
---

## The artifact

Its author owns where the seams are, which way the dependencies run, and what crosses each seam. This is the one place in the planning chain where naming things is legitimate, and only for one reason: more than one piece of work has to use the same name for the same thing. The register is a record of decisions, each with the tension that forced it, the choice made, what that choice costs, what was rejected and why, and — critically — what the decision settles that several pieces of work must honour identically, and where the single definition of each lives.

It also carries the assembly point: where the parts come together, who does the joining, and the behaviour that proves the connection is real when exercised through the running system rather than through something built to demonstrate it.

It must never say where code lives — that is decided with the code in front of you — or record decisions that are nobody else's business. Its characteristic failure is the opposite of over-reach: the decision left out because it seemed too obvious to write down. Obvious *and* shared must be recorded. Obvious *and* local must not.

## Failure modes

- A decision nobody thought worth recording — how a shared failure is represented, what a shared value means at the edges, the shape of what crosses a seam — that several pieces of work will each re-derive independently and differently.
- A record only one piece of work will ever touch, recorded at the wrong altitude: it spends attention and freezes a choice that should have stayed open until the code was in front of someone.
- A location named ("this will live in X") standing in for a boundary and what crosses it — the cheap version of the work, presented as if the hard part were done.
- A missing assembly point: where the pieces meet, whose work performs the joining, and what observable behaviour demonstrates it. Any one of the three missing means every piece can be built to specification and the whole still not work.
- A decision reasoned from a boundary that does not exist as described — everyone downstream inherits the same false premise.
- A layer, seam, or extensibility point recorded for a piece of work that does not exist yet — an abstraction earning its keep against an imagined future requirement rather than the one actually in front of it.
