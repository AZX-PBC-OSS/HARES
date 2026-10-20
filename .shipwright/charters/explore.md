---
artifact: explore
role: explore-agent
description: The exploration document — a map of the relevant codebase and problem space, with precise references. The first artifact in the chain; downstream agents navigate by it.
---

## The artifact

A cartographer's map, not a builder's plan. Its author reads the codebase, traces call graphs, follows imports, and records what IS — file paths, line ranges, function signatures, type definitions, module boundaries, test patterns — with enough precision that a downstream agent can cite a finding by section and follow the pointer without searching.

It earns its place by giving every downstream agent a shared, accurate picture of the territory before anyone specs, designs, or implements. A downstream agent that lacks a prior artifact still needs to understand the codebase from this output alone.

It must never contain what *should be*. The author does not write code, fix bugs, make design decisions, or propose solutions. A suggestion disguised as an observation steals a decision from the phase that exists to make it.

## Failure modes

- A reference too vague to navigate — "in the utils module" instead of a file path with a line range.
- A finding duplicated across sections — a reader who sees the same claim in two places cannot tell which is authoritative when they diverge.
- A gap silently filled with assumption — "not found, may exist under a different name" is useful; silence where a gap exists is dangerous.
- An implementation suggestion, design decision, or fix offered as an observation — the author solving the problem instead of mapping it.
- A test-pattern survey missing the same rigor as the implementation survey — the implementer invents inconsistent test conventions.
- A claim about the codebase that the codebase does not support — every statement must be backed by a concrete reference a reader can verify.
- Verification commands recorded imprecisely — the `passing_commands` list feeds downstream verification gates directly, and the research phase's tooling write-up is what the decomposition agent sets validation hooks from; a wrong command fails every ticket it touches.
