# I-09 Schedule

The capability program: co-simulation and grid operations (A), Monte
Carlo at scale (B), reinforcement learning and utility ML tools (C),
small-commercial readiness (D), the contributor platform (E). The rows'
full specs live in `10-axis-a-cosim.md`, `11-axis-b-mc-axis-c-rl.md` and
`12-axis-d-e.md` (the planning round's verified proposals, 2026-10-17).
Sizes: S about a day, M a few days, L about a week. The I-08 rules apply
whole: RED first, one row per commit or a named commit series, goldens
deliberately re-captured with causes, the canonical gates, independent
review and red team before any fold, honest measurements.

State values: `draft` (not started), `in flight` (an implementer active),
`review` (implemented, awaiting the fresh-agent review round), `landed`
(folded with gates), `recorded` (a report/decision row).

Columns: # | axis | row | title | size | spec | depends on | state

| # | axis | row | title | size | spec | depends on | state |
|---|---|---|---|---|---|---|---|
| 1 | E | E2 | the whole-step allocation assertion | S | 12-axis-d-e.md | none | draft |
| 2 | E | E5 | CHANGELOG and the version contract | S | 12-axis-d-e.md | none | draft |
| 3 | E | E6 | the submodule and reproducibility posture | S | 12-axis-d-e.md | none | draft |
| 4 | E | E3 | the checkpoint bench and the CI bench gate set | S | 12-axis-d-e.md | none | draft |
| 5 | A | A8 | the HELICS wheel-workaround watchdog | S | 10-axis-a-cosim.md | none | draft |
| 6 | D | D4 | schedule-driven DCV (occupancy-modulated OA) | S | 12-axis-d-e.md | none | draft |
| 7 | D | D6 | the demand-charge basis pinned and demonstrated | S/M | 12-axis-d-e.md | none | draft |
| 8 | D | D5 | the small-commercial schedule library and holidays | M | 12-axis-d-e.md | none | draft |
| 9 | D | D1 | the density-based premise path | M | 12-axis-d-e.md | none | draft |
| 10 | E | E1 | the property harness, five highest-value properties | M | 12-axis-d-e.md | (4) wants I-08 Task 1.5 | draft |
| 11 | E | E7 | the nextest slow partition | M | 12-axis-d-e.md | none | draft |
| 12 | E | E8 | the docs-truth guard and the sample-config home | M | 12-axis-d-e.md | coordinates with I-08 Task 2.12 | draft |
| 13 | A | A1 | the grid-signal input channel | M | 10-axis-a-cosim.md | I-08 Task 1.6 | draft |
| 14 | A | A3 | the co-simulation contract | M | 10-axis-a-cosim.md | none (names A1's topics) | draft |
| 15 | A | A2 | grid-event scenario scripting | M | 10-axis-a-cosim.md | A1 | draft |
| 16 | C | C6 | gymnasium conformance and the version contract | S | 11-axis-b-mc-axis-c-rl.md | I-08 Task 3.2 | draft |
| 17 | C | C1 | catalogue-derived spaces | M | 11-axis-b-mc-axis-c-rl.md | I-08 Task 3.2 | draft |
| 18 | C | C3 | typed reward components and the RewardSpec | M | 11-axis-b-mc-axis-c-rl.md | I-08 Task 3.2 | draft |
| 19 | C | C5 | the episodes/second contract and the bench | S | 11-axis-b-mc-axis-c-rl.md | C3 | draft |
| 20 | B | B3 | the scenario seed algebra, pinned | S | 11-axis-b-mc-axis-c-rl.md | I-08 Tasks 2.8, 2.9, 1.5 | draft |
| 21 | B | B2 | the scenario spec and sampler | M | 11-axis-b-mc-axis-c-rl.md | I-08 Task 2.9 | draft |
| 22 | B | B1 | stochastic occupancy and behavioral diversity | L | 11-axis-b-mc-axis-c-rl.md | I-08 Tasks 1.3, 1.5, 2.8 | draft |
| 23 | C | C2 | the missing actor archetypes (three actors) | L | 11-axis-b-mc-axis-c-rl.md | I-08 Task 1.9; actor 1 also 4.6 | draft |
| 24 | C | C4 | per-equipment attribution (the multi-agent view) | M | 11-axis-b-mc-axis-c-rl.md | I-08 Tasks 3.2, 3.4 | draft |
| 25 | B | B4 | the scenario lake | M | 11-axis-b-mc-axis-c-rl.md | I-08 Task 4.1; B2 | draft |
| 26 | B | B7 | the one shared evaluation surface | M | 11-axis-b-mc-axis-c-rl.md | I-08 Task 4.5 | draft |
| 27 | B | B5 | the Monte Carlo runner | L | 11-axis-b-mc-axis-c-rl.md | B2, B3, B4; I-08 Task 2.14 | draft |
| 28 | B | B6 | the analysis surface | M | 11-axis-b-mc-axis-c-rl.md | B4 | draft |
| 29 | A | A4 | per-DER trip observability end to end | M | 10-axis-a-cosim.md | I-08 Task 4.6; A3 | draft |
| 30 | A | A5 | load behavior below 0.9 pu | M | 10-axis-a-cosim.md | I-08 Task 4.6 | draft |
| 31 | D | D2 | the packaged single-zone RTU assembly | M | 12-axis-d-e.md | D1 | draft |
| 32 | A | A6 | the named co-sim surface and the pandapower adapter | L | 10-axis-a-cosim.md | A3, A1, A2 | draft |
| 33 | D | D3 | the outdoor-air economizer | M | 12-axis-d-e.md | D2 | draft |
| 34 | A | A7 | the service-entrance electrical model | L | 10-axis-a-cosim.md | A5 | draft |
| 35 | D | D7 | the small-commercial acceptance scenario | M | 12-axis-d-e.md | D1-D6; E1 first | draft |
| 36 | E | E4 | the phase-share re-record | M | 12-axis-d-e.md | I-08 Task 4.2 | draft |
| 37 | E | E9 | the contributor chapters | L | 12-axis-d-e.md | I-08 Tasks 4.1, 4.2; last | draft |
