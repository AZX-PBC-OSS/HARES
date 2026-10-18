---
artifact: research
role: research-agent
description: The research document — reusable code, prior art, library evaluations, and tooling configuration combining local analysis with external research. Supplements, never replaces, prior artifacts.
---

## The artifact

Its author investigates both the local codebase and the outside world. Locally: functions, types, utilities, and patterns the initiative could reuse rather than reimplement, with file paths, signatures, and compatibility constraints. Externally: libraries, frameworks, and approaches from the broader ecosystem, evaluated with maturity, API surface, footprint, and known gotchas.

It earns its place by giving the team the full picture before anyone specs, designs, or implements — so a decision to build versus reuse is informed, and a choice of approach is weighed against alternatives rather than picked from ignorance.

It must never make design decisions. The author presents options with trade-offs and surfaces gaps honestly. They do not pick a winner, resolve an ambiguity by guessing, or present a single approach as settled when alternatives exist.

## Failure modes

- A library or tool evaluated at headline depth — name and marketing pitch with no API surface, maturity assessment, or known gotchas.
- Reusable code in the codebase left undocumented — the implementer reimplements what already exists, duplicating logic the project already has.
- An external search that stopped at the first page of results — a plausible-looking approach presented as the best available, with alternatives unexamined.
- A design decision presented as a research finding — the author picking a solution instead of presenting options with trade-offs.
- A gap in prior art left unmarked — silence where no evidence exists reads as "nothing to worry about" rather than "risk the spec should address."
- Tooling commands imprecise or missing — the decomposition agent sets validation hooks from this section, and a wrong command fails every ticket it touches.
- A finding that duplicates what a prior artifact already covers without adding to it — noise that buries the research that actually extends the picture.
