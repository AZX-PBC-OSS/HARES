# I-08 context

## Origin

HARES#28: full-fidelity run output is gated on a disk write. A consumer that
only needs results in memory pays for a file it never reads, and turning the
file off truncates the returned frame to two columns and disables metrics.

## Who is affected

- **Calibration system** (`~/src/HARES_CALIBRATION_SYSTEM`): about 500
  full-year, 900 s evaluations of one home per fit. `simulate()` is about 99%
  of each evaluation's time. The loss reads one column. Today every run
  writes a 3.4 to 4 MB Parquet file nobody opens, and the health gate reads
  counters that are only filled in `observe` builds, so it always sees zero.
- **Fleet and ResStock studies** (10^3 to 10^4 homes): memory grows with homes
  times steps times columns; flagged homes with zeroed metrics count as
  successes.
- **RL and co-simulation** (stepped runs): flush and close happen only in
  `simulate()`, so a stepped run loses its last partial chunk and its billing
  finalisation.
- **Researchers**: output selection is by integer verbosity only, units are
  lost at the Python boundary, and not-applicable values are written as 0.0.

## Measured baseline (release, one full year at 900 s, verbosity 2)

| Quantity | Value |
|---|---|
| `simulate()` wall, output on | 1.21 s (bldg 8590), 1.077 s (bldg 46363) |
| `simulate()` wall, output off | 1.03 to 1.04 s |
| Output pipeline share | about 140 to 170 ms per evaluation (12 to 14%) |
| Per-step telemetry `HashMap` clone | about 15 to 18% of `simulate()` |
| Exogenous forcing update | about 7.9% of `simulate()` |
| Retained batches per run | about 9.5 MB |
| Python IPC conversion | about 6.5 ms per frame; Arrow C stream about 2.7 ms rechunked |

These figures are re-measured at the start of implementation on the working
branch; every optimization records its own before and after.

## Issues and optimizations identified (every item is a ledger entry in plan.md)

Engine runtime and memory:

1. **Per-step telemetry `HashMap` clone.** `mod.rs:6436` clones the telemetry
   map every step (derived `Clone` makes `clone_from` a full clone; a second
   site at `mod.rs:4924`), plus a per-step `HashSet` rebuild at `mod.rs:6389`,
   despite a comment claiming slot reuse. About 15 to 18% of `simulate()`
   (about 190 ms per run), the largest single engine item.
2. **Exogenous forcing recomputed per dwelling.** `update_in_place`
   (`environment.rs:524-700`: solar position, Perez irradiance per surface,
   psychrometrics, mains temperature, schedule-row gather) is 7.9% of
   `simulate()`. Azimuth and PV capacity feed it, so the surface-independent
   forcing is shared through the parsed weather, keyed on the time grid and
   site, and per-surface values never cross geometries.
3. **Parse once per home.** Every evaluation re-parses HPXML, schedule,
   weather and defaults; `DwellingBlueprint::build(self)` consumes the
   blueprint (`blueprint.rs:225`) and `from_parts` mutates `weather.meta` in
   place. Shared immutable parsed inputs remove the repeat work.
4. **Envelope solar and incidence recomputation; dead coupled factorization.**
   The coupled (M + D) LU path (`state_space.rs:695-708`,
   `stepping.rs:682-685, 723-725`) is unreachable because the implicit matrix
   is always the identity; it is removed as dead code. `m_scratch` is
   reallocated each call; the solar slot map is rebuilt twice per step
   (`thermal_solver/mod.rs:1560-1568`).
5. **Thermal input preparation.** `prepare_inputs_inner`
   (`stepping.rs:668`, called from `mod.rs:5439`) clones
   `exterior_surface_temps` per call and `build_input_vector` returns a fresh
   `HashMap` twice per step; `solve_ideal_capacity_for_target` is 6.8% of
   `simulate()`; the phase is about 14%. Profile-directed.
6. **Threads instead of processes.** 26 worker processes hold about 4.7 GB;
   threads sharing one set of parsed inputs need about 1 GB. Needs explicit
   `Send`/`Sync` assertions and the process-global warn-once and counter
   statics made per-run.
7. **Unused checkpoint.** Python `initialize()` serialises a full JSON
   checkpoint on every evaluation that nothing reads (`py_dwelling.rs:661-676`).
8. **Output pipeline cost.** `record_step` about 95 ms and the final flush
   about 9 ms per run at verbosity 2; 13 per-step string-hash column lookups
   in `record_step`; batches cloned unconditionally in `simulate()`,
   `results()` and `metrics()` (`py_dwelling.rs:742-746, 1484, 1492`); IPC
   conversion 6.5 ms versus 2.7 ms for the Arrow C stream.

Output and telemetry reporting:

9. **HARES#28.** Full output requires a disk write (the file is created
   eagerly); turning it off truncates the frame to two columns and makes
   `metrics()` raise.
10. **Health and telemetry only in `observe` builds.** The health columns
    exist in every build but are filled only under `observe`, so shipped
    wheels report zeros; the NaN counter can never be non-zero in a returned
    result.
11. **Stepped runs lose output.** Flush, close and billing finalisation run
    only in `simulate()`.
12. **Diagnostics CSV ignores the output switch** and opens at verbosity 4 or
    higher.
13. **Fleet memory and status.** Fleet memory grows with homes times steps
    times columns; flagged homes with zeroed metrics count as successes; any
    column mismatch empties the aggregate.
14. **Time as strings in three formats**; `Time` becomes a naive local Arrow
    timestamp.
15. **Profiler (HARES#31).** The water-heater timer is never incremented;
    there is no flush/encode timer; `/proc/self/status` is read four times
    per step; about 29% of step time is unattributed; the allocation counter
    is a stub.

Calibration consumer (`~/src/HARES_CALIBRATION_SYSTEM`):

16. **Migrate every call site** to the new output API; `Runner.scratch` and
    the dead Parquet file go; health comes from the engine, not frame
    columns; `contract()` asserts the schema version instead of a live probe.
17. **Worker pools sized to the population** for lone fits (one idle-tail
    wave per generation today); the inner-pool shrink in `scale.py` is dead
    code because `workers` is defaulted first.
18. **Duplicate parent runs.** Nominal is simulated at `fit.py:315` and again
    as `X[0]`; the warm start at 317 and again as `X[1]`; "best" is
    recomputed at 356 (about 3.5 to 6 s per fit). The missing precondition:
    the probe equals nominal only when no ABSOLUTE factor is free.
19. **Fit path computes only what the objective reads**: skip `metrics()` and
    equipment metadata (about 7.4 ms per evaluation); remove the scratch
    output (6.2 GB across `scratch/fitrun`, one directory per worker
    process; deletion only with operator confirmation).
20. **Per-evaluation provenance and replay.** Record the parameter vector,
    source hashes, injector version, engine build and seed per evaluation so
    any evaluation can be replayed with full output.
21. **Surrogate search trial.** Compare DE with surrogate and screening
    methods on the production cells; adoption gated on fit quality, report
    only.

Defects found along the way (fixed as separate ledger entries): the full
confirmed defect table is in `review.md`.

## Goal

Output and telemetry reporting that users pay for only when they ask for it,
correct in every run mode, with bounded memory; the measured runtime overhead
in `simulate()` removed; no change to the physics from any optimization. The
defects found while doing this are fixed as separate entries.
