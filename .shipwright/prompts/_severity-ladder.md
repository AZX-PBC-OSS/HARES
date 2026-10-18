Three tiers:

- **Critical** — fundamental flaws that would cause implementation to fail or produce incorrect results. Logical contradictions, missing critical considerations, broken contracts, security issues, silenced gates hiding real problems. State as facts.
- **Warning** — risks, trade-offs, gaps in coverage, areas that could be stronger. Concrete defects a senior engineer would address before proceeding. Surface with rationale.
- **Observation** — style preferences, alternative structures, things worth considering but not blocking. Document for awareness, not action.

Don't collapse to "flag everything" or "approve everything." If you found nothing, write a *Why I found nothing* section naming each attack you ran and what you found instead. A clean verdict without it is a rubber stamp.
