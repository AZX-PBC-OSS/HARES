# Decision Log

## dl-024d6b80-6f17-45fa-bb74-522362331885

```yaml
event_id: dl-024d6b80-6f17-45fa-bb74-522362331885
timestamp: 2026-09-10T20:40:10.860Z
actor: operator
action: flow_recipe_selected
target: Z:/home/rich/src/HARES/.shipwright/flows/fix.yaml
recipe_sha256: 10902c4131024a9024ccef1cbb96af994f1550e48c95c88d0b3fa4d00e5aed6f
reason: workspace_override
```


## dl-fix-1c3fbfaf-power-setpoint-signed

```yaml
event_id: dl-fix-1c3fbfaf-power-setpoint-signed
actor: operator (direction), fix agent (implementation)
action: api_contract_corrected
target: crates/hares-python/src/py_control.rs PyControlSignal::power_setpoint
reason: >
  Wrapper rejected negative active_power_kw while the core ControlSignal
  contract is signed (positive = charge, negative = discharge; V2G/V2H
  strategies and battery Rust tests command negative setpoints). Three
  Python tests (battery min_soc discharge, gym out-of-bounds actions,
  helics battery control) were red at pristine HEAD because of it.
  Operator directed "no broken windows / you fix it" during the fix run.
  Wrapper now validates finiteness only; semantic limits remain the
  receiving equipment's. Wrong-contract tests corrected to pin the signed
  semantics.
scope: I-01 fix run v2-fix-1c3fbfaf-0c08-49c7-a591-410ff8c160f7
```

## dl-fix-1c3fbfaf-parity-corpus-gate

```yaml
event_id: dl-fix-1c3fbfaf-parity-corpus-gate
actor: fix agent
action: escalate
target: tests/parity/mod.rs parity_outputs_against_reference_corpus gate
reason: >
  The must-pass gate `cargo test --workspace` fails on two
  parity_outputs_against_reference_corpus tests. Root cause is NOT the
  I-01 change (failure signatures bit-identical at pristine HEAD
  c2e12206 with the same fixtures). Cause: untracked, provenance-less
  reference_output.parquet files (in-flight parity work, absent from
  git ls-files) activate committed tight-tolerance comparisons, and the
  engine genuinely deviates on the references' 60-row cold-start
  1-hour windows: conditioned-zone temperature MAE 0.68-1.69 C
  (allowed 0.6) on 10/11 fixtures, short-window HVAC energy 44-206%
  (allowed 25-48), while the one 24-hour fixture passes temperature and
  energy (short-window transient dominates the failures). Adjacent
  engine defect fixed during triage: engine.run misreported every
  default-config (retain_batches=false) run as "zero-step simulation"
  because it used flushed_batches() — documented to be always empty
  unless retained — as the ran-at-all signal; now uses the step count
  (see I-01 Implementation Notes, Deviations).
decision_needed: >
  Are the untracked reference parquets authoritative for the current
  engine (then: physics work to close the short-window temperature/
  HVAC-energy gaps belongs to the parity ticket), or provisional
  (then: the fixture owner regenerates or recalibrates them)? Fixing
  either side from this ticket would decide the parity owner's
  in-flight work; loosening the committed tolerances or ignoring the
  test would silence a check reporting real deviations.
scope: I-01 fix run v2-fix-1c3fbfaf-0c08-49c7-a591-410ff8c160f7
```


## Done-challenge produced changes — 2026-09-11T05:20:14.166Z

- **Phase:** coverage-triage
- **Run:** I-01-coverage-triage-e5ca1253-c630-4dc1-9861-bfb8125cbe3b

## Done-challenge produced changes — 2026-09-11T16:31:21.148Z

- **Phase:** red-team
- **Run:** I-01-red-team-54dea811-32a8-4f03-9293-537fdae77dc7

## Done-challenge produced changes — 2026-09-11T17:16:32.046Z

- **Phase:** fix
- **Run:** I-01-fix-afcd30f3-d630-4625-a1ab-92f867c68fea

## Done-challenge produced changes — 2026-09-11T18:53:50.822Z

- **Phase:** red-team
- **Run:** I-01-red-team-e8cc8dbd-5258-49b8-8c85-59fbd38032aa