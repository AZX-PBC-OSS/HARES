# I-09 Charter: HARES as the reference residential (and small-commercial) energy simulator

## Mission

HARES becomes the most powerful, performant, efficient, robust, reliable and
ergonomic residential energy simulator available to academia, research
groups, grid operators and utility analytics teams: the engine of record for
co-simulation with power-flow solvers, reinforcement learning and
Monte Carlo studies at scale, feeding utility ML tools that deliver grid
equity, stability and reliability.

## Who it serves

- Academic researchers and national-lab groups who must cite, reproduce and
  extend the physics (validation suites, provenance, energy closure).
- DR/RL practitioners (the gym): fast episodes, honest observation spaces,
  stochastic realism, typed failures.
- Grid operations and planning studies: panel-to-feeder behavior, DER
  trip/ride-through, voltages, reactive power, islanding, Monte Carlo
  scenarios at fleet scale, co-simulation with power-flow solvers.
- Utility ML/analytics teams: the calibration system's fit paths, scenario
  lakes, reproducible re-baselines.

## Success criteria (each measured, each a ledger row family)

1. **Correctness**: the validation suites (ASHRAE 140/BESTEST pairs, ISO
   13786, run-level energy closure, OCHRE parity records) green and
   re-run per numeric row; every physics divergence registered with its
   cause; conservation invariants always on.
2. **Performance**: the step loop allocation-free end to end; the year wall
   and the fleet high-water tracked in the measurement log; every
   regression gated by a bench; the next 10x items owned by rows.
3. **Robustness**: no silent fallback anywhere (typed errors or counted
   events); parsers fuzzed; the stochastic lifecycle pinned end to end;
   checkpoints bitwise.
4. **Ergonomics**: time-to-first-simulation minutes; the contributor docs
   (lifecycle, roster, output pipeline) true; a changelog and version
   contract; the gym's spaces derived from the catalogue.
5. **Flexibility**: custom equipment, schedules (filed, generated,
   stochastic), actors and tariffs all first-class; co-simulation
   interfaces maintained; Monte Carlo at fleet scale a documented path.

## Non-goals

- Becoming EnergyPlus: HARES cites and tracks the E+ references where they
  define correctness; it does not reimplement E+ scope beyond residential
  and small-commercial.
- Commercial licensing questions; the license posture is the operator's.
- Any breaking rewrite of the landed output/replay surfaces: they are
  pinned by B1 from row 97 on.

## Method (inherited from I-08)

Every row: RED first, one defect or capability per commit, goldens
deliberately re-captured with causes, canonical gates (fmt, clippy
-D warnings, nextest --workspace --all-features, pytest -m '', pyright,
the goldens bitwise), independent review and red team by fresh agents
before any fold, measurements with honest methodology in the measurement
log, the ledger always current.

## Status

- I-08 closes its numeric tail (rows 94c-94j) and the hardening fan-out
  waves 1-5 (its rows 102+).
- I-09 owns the capability program beyond that: co-simulation, Monte
  Carlo at scale, the RL/ML surfaces, small-commercial, the contributor
  platform. Its schedule seeds from the six-domain reviews and the three
  surveys (docs/superpowers/plans/2026-10-17-hares-hardening-*), the
  planning agents' proposals, and the operator's vision statement
  (2026-10-17).
