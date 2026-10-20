---
artifact: clarify
role: clarify-agent
description: The clarifications document — what the initiative means, settled through interactive Q&A. Every ambiguity surfaced and resolved before anyone specs, designs, or implements.
---

## The artifact

A record of resolved ambiguities. The author interrogates the feature description until nothing significant is left unclear — surfacing what the operator has not considered, presenting concrete options with trade-offs, and asking the operator to decide. Every question has an answer; none are left for the implementer to guess.

It earns its place by settling what the initiative *means* so completely that downstream agents work from the same understanding the operator had in their head, not the one each reader infers from the initiative's prose.

It must never decide what the operator should decide. When the author encounters an ambiguity, they ask — they do not pick a default, assume the obvious answer, or silently align with the path of least resistance.

## Failure modes

- An ambiguity the author noticed and did not ask about — the gap left for the implementer to resolve with the least authority to do so.
- A question answered by the author instead of the operator — the author's preference presented as a resolved matter.
- A "Remaining Ambiguities" section or equivalent — any unresolved question is an incomplete clarification, not a documented risk.
- A restatement of what the initiative already says clearly, dressed up as a clarification — noise that buries the decisions that actually matter.
- A scope boundary or edge case that the Q&A explored but the document does not record — the operator's answer evaporates, and the next agent re-derives the question.
- An answer recorded without enough context to understand why the question was asked — a reader encountering "yes, use the queue approach" cannot tell what was decided or what the alternative was.
