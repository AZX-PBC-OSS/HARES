---
artifact: decompose
role: decompose-author
description: The ticket set — an assignment board of implementation tickets, each citing prior artifacts rather than copying them, each with an observable done-condition.
---

## The artifact

A ticket must be understandable on its own and must cite everything else rather than copy it. A reader who wants the full reasoning follows the citation; a reader who wants to start work does not need to. That is the whole self-containment rule — copying the spec into a ticket does not make it self-contained, it makes a second copy that drifts the moment the spec changes.

A ticket says what to achieve, not exactly how. Names, signatures, file layout, internal structure, and the shape of the tests are decisions made with the code in front of you — decisions a ticket writer would be making blind. Every seam needs an owner: where two pieces of work meet, exactly one ticket owns the definition and the others cite it. The whole needs an assembly: one ticket proves the pieces work once connected, through the real system, never through a stand-in built for the purpose.

It must never design instead of specify: fixed names, signatures, message text, internal helpers, module layout, or a file manifest read as a permission boundary. It must never carry the plan's own vocabulary — section numbers, phase indices, stage labels — into the product as identifiers.

## Failure modes

- A ticket that fixes a name, signature, parameter list, or internal helper the code does not have yet — a decision made blind that the implementation will later contradict.
- A file manifest, in the body or a stray `scope` field, read by the implementer as a permission boundary rather than a starting point.
- Precision about a shape too far downstream to be known yet — prediction wearing a requirement's clothes.
- A promise one ticket makes and another relies on, with no ticket owning the definition, or two tickets that will each decide the same thing independently.
- No ticket that assembles the parts and proves the whole works through the real system — every part can pass individually and the whole still not work.
- A requirement traceable to no ticket, or a ticket traceable to no requirement.
- A done-condition that counts files, names a function, or restates the goal instead of describing behaviour observable from outside the change.
- A ticket scoped wider than the design and architecture actually call for — infrastructure invented at this stage rather than demanded upstream.
