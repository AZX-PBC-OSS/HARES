# Decision Log

## dl-b3708677-7666-4b7c-9093-6bd7e176b6cf

```yaml
event_id: dl-b3708677-7666-4b7c-9093-6bd7e176b6cf
timestamp: 2026-09-12T06:58:10.028Z
actor: operator
action: flow_recipe_selected
target: Z:/home/rich/src/HARES/.shipwright/flows/fix.yaml
recipe_sha256: 10902c4131024a9024ccef1cbb96af994f1550e48c95c88d0b3fa4d00e5aed6f
reason: workspace_override
```

## dl-8f4c21a3-6f2e-4b18-9d47-2c5a90e1b7d2

```yaml
event_id: dl-8f4c21a3-6f2e-4b18-9d47-2c5a90e1b7d2
timestamp: 2026-09-12T16:58:48.000Z
actor: fix
action: prior_artifact_corrected
target: rca.md (passing_commands)
what_was_wrong: >-
  rca.md listed "cargo nextest run --release --no-fail-fast --profile ci
  -p hares-core" as a passing command. As written it fails on HEAD
  (c2e12206) and always has: dwelling::tests::run_timestep_panics_on_unhealthy_actor
  asserts a panic that only exists under debug_assertions or the
  check_invariants feature, both compiled out of a bare --release run.
  CI (.github/workflows/ci.yml:136) runs this suite with -F check_invariants.
what_it_says_now: >-
  "cargo nextest run --release -F check_invariants --no-fail-fast
  --profile ci -p hares-core" — verified 1076/1076 passing on the fixed tree.
documents_citing_old_version:
  - rca.md frontmatter only; reproduction.md cites no passing_commands.
note: >-
  Escalated to the operator (I-02 owns the decision): three python tests are
  red on HEAD — test_py_equipment_mutation.py::test_min_soc_prevents_deep_discharge,
  test_gym_env.py::test_vec_gym_out_of_bounds_actions_no_nan and
  test_helics_integration.py::test_control_signal_via_helics. All three share
  one root: I-02 commit 2523f162 (T-0536) added validate_non_negative to
  PyControlSignal::power_setpoint (crates/hares-python/src/py_control.rs:33),
  pinned by its own new test test_power_setpoint_rejects_negative_kw, while
  (a) the equipment contract still accepts signed setpoints — battery
  apply_control_unchecked stores power_setpoint_kw with no sign check and
  discharges on negative (crates/hares-equipment/src/battery/mod.rs:1778) —
  (b) the actor-level twin PySignal::power_setpoint still accepts negative
  (crates/hares-python/src/py_actor.rs:703), and (c) the three red tests
  depend on negative setpoints to command or prove discharge; the helics
  test's comment documents the physics reason only a discharge setpoint can
  prove delivery (charge is blocked by min_charge_temp_c lockout in its
  scenario). Removing the validation contradicts T-0536's pinned contract;
  keeping it leaves direct-control battery discharge uncommandable (no
  discharge signal exists in the PyControlSignal surface). Both remedies
  decide I-02's API contract, so this is left to the operator/I-02 rather
  than resolved inside I-03. Separately fixed in I-03's round (broken gate
  surfaced by verification, one-line, matches the dwelling pattern):
  hares-fleet aggregation.rs used debug_assert! inside a check_invariants-
  gated block, leaving the documented invariant inert under the CI release
  command; now assert!, and
  aggregation::tests::build_aggregate_batch_debug_assert_catches_invalid_weight
  passes under --release -F check_invariants.
```



## Done-challenge produced changes — 2026-09-12T17:15:23.471Z

- **Phase:** fix
- **Run:** I-03-fix-9e2eca7d-804e-44cd-9a1d-cb6ca5a1b179

## Done-challenge produced changes — 2026-09-12T17:22:10.342Z

- **Phase:** round-out-coverage
- **Run:** I-03-round-out-coverage-5c1108cc-577b-495e-97e3-515fbdd4b40b

## Done-challenge produced changes — 2026-09-12T17:46:29.903Z

- **Phase:** red-team
- **Run:** I-03-red-team-8d16fb63-26df-49b7-9f8f-bc2cf6a1d1fb

## Done-challenge produced changes — 2026-09-12T18:05:18.342Z

- **Phase:** fix
- **Run:** I-03-fix-f8c0f6ef-4e92-4187-90df-26f54d2dde19

## Done-challenge produced changes — 2026-09-12T19:07:04.063Z

- **Phase:** red-team
- **Run:** I-03-red-team-20f05491-18ad-424a-87db-fc8daf6bd906## dl-82242cc9-8e56-4579-abdd-90760caae52f

```yaml
event_id: dl-82242cc9-8e56-4579-abdd-90760caae52f
timestamp: 2026-09-12T19:18:15.996Z
actor: operator
action: operator_overrode_gate_bypass
target: red-team
reason: operator bypassed command gates after reviewing failures
```



## Done-challenge produced changes — 2026-09-12T19:23:55.341Z

- **Phase:** red-team
- **Run:** I-03-red-team-6b4da4f9-8204-4d91-ae73-f55af331ac66## dl-f147cbe4-8c7f-46fe-9dbb-1a5cc8bfc886

```yaml
event_id: dl-f147cbe4-8c7f-46fe-9dbb-1a5cc8bfc886
timestamp: 2026-09-12T20:39:11.514Z
actor: operator
action: operator_overrode_gate_bypass
target: red-team
reason: operator bypassed command gates after reviewing failures
```

## dl-c14bf58c-38eb-48b5-8b96-01b5c4e8645e

```yaml
event_id: dl-c14bf58c-38eb-48b5-8b96-01b5c4e8645e
timestamp: 2026-09-12T21:29:34.086Z
actor: operator
action: operator_overrode_gate_bypass
target: red-team
reason: operator bypassed command gates after reviewing failures
```

## dl-5cf11309-5ac1-416d-b58b-dd53aa75aff4

```yaml
event_id: dl-5cf11309-5ac1-416d-b58b-dd53aa75aff4
timestamp: 2026-09-12T22:45:36.030Z
actor: operator
action: operator_overrode_gate_bypass
target: red-team
reason: operator bypassed command gates after reviewing failures
```

## dl-c6d035ae-376a-4365-856b-6173d2fa53f3

```yaml
event_id: dl-c6d035ae-376a-4365-856b-6173d2fa53f3
timestamp: 2026-09-12T22:50:41.177Z
actor: operator
action: operator_overrode_gate_bypass
target: red-team
reason: operator bypassed command gates after reviewing failures
```



## Done-challenge produced changes — 2026-09-13T00:29:58.297Z

- **Phase:** fix
- **Run:** I-03-fix-28a80e7d-fbf5-4212-a2cd-b4c7c106c630

## Done-challenge produced changes — 2026-09-13T01:44:54.759Z

- **Phase:** review
- **Run:** I-03-review-67b5e9f1-8626-4657-ac5e-cb8055606f54

## Done-challenge produced changes — 2026-09-13T02:03:45.599Z

- **Phase:** retrospective
- **Run:** I-03-retrospective-21659977-1ad1-4483-9504-bf610f1b5e81