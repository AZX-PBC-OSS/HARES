# PR readiness: feat/output-results into initial-implementation

State at writing: 2026-10-08, engine head `62ba224b`, consumer head
i08-consumer at the pin over `62ba224b`. The local
`initial-implementation` branch is already fast-forwarded to
`62ba224b` (verified an ancestor relationship: a fast-forward, no
merge commit). Push + the GitHub PR remain the operator's actions.

## What this branch is

The I-08 initiative's first phase, complete: the baseline repair, the
measurement and golden infrastructure, the profiling and memory
discipline, every ledgered silent default, skip and discarded error
closed, the physics divergences from the reference triaged and
registered, and the test corpus hardened to prove it. 299 commits
from `1d60f478`; 95 of 95 in-scope schedule rows done (the two
deferred-by-design rows land with their consumers: see Open items).

## Proofs at the tip (all run this week, all green)

- fmt; clippy --workspace --all-targets --all-features -D warnings
- nextest: 7595 default / 7657 observe / 7527 release / 7604 dst
- goldens: 7 fixtures bitwise identical in both profiles
- BESTEST: 12/12 with drift locks re-measured, causes recorded
- pytest: 1582 passed, 5 xfailed (every one a strict, seeded,
  re-measured declared divergence), 0 skipped (the skip census empty)
- pyright 0/0 (pythonVersion 3.12), the doc gate clean, the advisories
  clean, the OCHRE comparison CI job green
- B0: re-baselined at the tip's physics, identical across runs, and
  identical through the consumer's own pinned-tree build
- The consumer's full gate: 346 passed / 0 failed / 7 environment
  skips; ruff clean

## What improved (the roll-up has every delta with its cause)

- Physics: the trace's worst 6 h zone-temperature divergence from OCHRE
  2.41 to 0.71 C and still falling (the capacitance multiplier, the
  zone geometry, the diffuse loss, the dispatch setpoint landing); the
  parity corpus MAEs roughly halved (cz2a 0.39 to 0.25, resstock 0.60
  to 0.24); the 24 h window's cooling and Other end uses exact; water
  heating's 7x mystery was OCHRE's dropped draw column (D-017); the
  ASHP's cold-weather COP was 16% optimistic (D-014, the RESNET anchor
  model); 31 registered divergences, every one cited and pinned.
- Performance: hot-path allocations 7,603,732 to 666,009 per run
  (-91.2%); the 30-day bench -10%; the profiling overhead 2.75x to
  ~1.0x; step time ~34.6 us/step.
- Correctness: no known silent default, skip, dropped error or
  uncounted rejection remains in the parse, resolve, assembly, step or
  output paths; every engine rejection is a typed error naming its
  subject; the checkpoint schema is versioned and gated at v13; the
  fleet reports every dwelling's construction failure by name.

## Open items (recorded, none silent)

- The BESTEST 600/640 sit below the ASHRAE convergence bands (a
  documented, drift-locked residual; the remaining trace and attic
  candidates are registered as open with their measured bounds).
- The conditioned-attic winter bias (-1.11 C after the floor-film
  experiment ruled the coupling down to ~0.14 C): the cause remains
  unestablished (D-031 + the OD-56 record).
- OD-62 (a non-ideal unit's mode override sticks after its event),
  OD-63 (from_resstock's zip paths), OD-64 (the blueprint's
  defaults-degradation policy), the cz5a peak question, the MSHP
  default curves (D-020), B-16/17's wet-appliance slice.
- The five BESTEST fixtures' 0-ACH dependence was corrected to the
  OS-HPXML rule (the fixtures declare the measurement the fallback
  implied); the sweep's STOP is resolved mechanically.

## The operator's checklist for the PR

1. PUBLISH the local vendors/OCHRE fork commit 58f39d8 (the HPWH
   nominal-COP read fix) to the fork remote: until then the PR's CI
   cannot materialize the submodule.
2. Push: git push origin feat/output-results initial-implementation
   (the branch + the ffed base), then open feat/output-results into
   initial-implementation. Row 99a records the CI run.
3. The helics upstream report (OD-59: their wheel's libzmq declares no
   libstdc++; our canary pins it).
