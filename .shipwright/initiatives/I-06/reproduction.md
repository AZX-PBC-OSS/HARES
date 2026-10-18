---
reproduction_outcome: confirmed
reason: >-
  Both regression tests were run on the pre-fix tree and fail with the
  expected messages: the structural test observes 12 equipment collapsed
  into 1 equipment_core entry (the root cause), and the behavioral test
  runs the ticket scenario verbatim and observes 7 charge-days with daily
  trip cancellations (the reported symptom).
failing_tests:
  - command: cargo test -p hares-core --test equipment_id_regressions equipment_core_has_one_entry_per_equipment
    path: crates/hares-core/tests/equipment_id_regressions.rs
    name: equipment_core_has_one_entry_per_equipment
  - command: cargo test -p hares-core --test equipment_id_regressions nightly_strategy_keeps_ev_charged_over_long_horizon
    path: crates/hares-core/tests/equipment_id_regressions.rs
    name: nightly_strategy_keeps_ev_charged_over_long_horizon
---

# Reproduction: EV under Nightly strategy stops charging after the first week

## Root cause being reproduced (from `rca.md`)

No code path in a real dwelling build assigns unique `EquipmentId`s. Every
equipment constructor defaults its descriptor id to 0 when the config lacks
`equipment_id`, nothing in the assembly path populates that key, so every
equipment instance in a dwelling built through `Dwelling::from_config` is
`EquipmentId(0)`. Every `EquipmentId`-keyed map then collapses to a single
entry — the end-of-step core-output snapshot keeps only whichever equipment
was inserted last (MELs in this fixture) — and observation-driven actors
read the wrong equipment's state. The EV driver never sees its own SOC, its
belief decays ~0.13/day, every trip is eventually cancelled as "out of
range" against a phantom empty pack while the physical pack sits at its
charge target.

## The reproduction tests

New file: `crates/hares-core/tests/equipment_id_regressions.rs` — two tests
that pin the bug from opposite ends. Both build the ticket dwelling through
the **real public assembly path** (`Dwelling::from_config` on ResStock
fixture bldg0000007 with a 75 kWh / 11.5 kW Level-2 EV injected into
`<Systems>`), not hand-built equipment maps, because the RCA established
that unit tests injecting ids manually never exercise the assembly path
where the bug lives.

### 1. `equipment_core_has_one_entry_per_equipment` — the rule

Builds the dwelling, runs one public `step()` (so the end-of-step snapshot
populates `equipment_core` — `from_config` alone does not populate it), then
asserts:

> `equipment_core.len() == dwelling.equipment().len()`

i.e. every equipment instance must be individually addressable in the
`EquipmentId`-keyed core-output map. This is pitched at the **rule**, not
the reported path:

- It fails for an id collision in **any** equipment type in the fixture
  (water heater, EV, washer, dryer, dishwasher, refrigerator, range, two
  lighting circuits, ceiling fan, TV, MELs — 12 constructors), so a future
  equipment constructor that reintroduces a defaulted id fails this test
  even if the EV is untouched.
- It is **fix-agnostic**. The RCA offers two fix designs — assign unique
  `EquipmentId`s, or re-key `equipment_core` by name. Both yield one entry
  per equipment, so this test passes under either. (The diagnose phase's
  probe assertion "every equipment must have a distinct `EquipmentId`"
  encodes only the first design and would fail after a name-rekey fix; the
  cardinality invariant is the contract every consumer actually relies on,
  so it is what the gate asserts.)

### 2. `nightly_strategy_keeps_ev_charged_over_long_horizon` — the symptom

Runs the ticket scenario verbatim through `SimulationEngine::run`: the
injected-EV dwelling with a `Nightly{22:00–06:00, target 0.90}` charging
strategy override, 45 days at 900 s, seed 0, then reads the run's CSV
output and asserts the post-fix contract:

- **≥ 30 of 45 days show a real charging session** (> 1 kWh drawn by the EV
  equipment that day; a nightly top-up is ~9.8 kWh against a 75 kWh pack,
  parasitic draw is far below 1 kWh).
- **`drive_cancelled` never exceeds 0** — a nightly-topped 67.5 kWh pack
  never runs out of range for a ~9.8 kWh daily trip.

The 45-day horizon is load-bearing: the RCA showed observation belief drift
is slow (~0.13 SOC/day), so the existing short-horizon EV tests pass while
this long run fails — the exact coverage gap the ticket called out. Total
runtime is ~2 s.

## Verified failures (run on the pre-fix tree)

Both frontmatter gate commands were run individually. Each exits with code
**101** (the Rust test-harness failure exit) and reports `1 failed; 0 passed`:

```
$ cargo test -p hares-core --test equipment_id_regressions equipment_core_has_one_entry_per_equipment
# exit code 101
test equipment_core_has_one_entry_per_equipment ... FAILED
thread 'equipment_core_has_one_entry_per_equipment' panicked at
  crates/hares-core/tests/equipment_id_regressions.rs:135:5:
assertion `left == right` failed: equipment_core must hold one entry per
equipment instance so EquipmentId-keyed lookups address the right equipment;
12 equipment collapsed into 1 entries — equipment identity collides in the
dwelling assembly path
  left: 1
 right: 12
test result: FAILED. 0 passed; 1 failed; 0 ignored

$ cargo test -p hares-core --test equipment_id_regressions nightly_strategy_keeps_ev_charged_over_long_horizon
# exit code 101
test nightly_strategy_keeps_ev_charged_over_long_horizon ... FAILED
thread 'nightly_strategy_keeps_ev_charged_over_long_horizon' panicked at
  crates/hares-core/tests/equipment_id_regressions.rs:179:5:
Nightly(22:00-06:00, target 0.90) must top the pack up on most of 45 days,
got 7 charge-days
test result: FAILED. 0 passed; 1 failed; 0 ignored
```

The numbers match the RCA's diagnostic run exactly (12 equipment / 1 distinct
id; 7 charge-days; drive_cancelled climbing to 32). No production code was
modified: `git status --porcelain` shows only the untracked test files
(`equipment_id_regressions.rs` and the diagnose phase's `i06_ev_nightly_diag.rs`).

Gates on the new file: `cargo fmt --check -p hares-core` clean;
`cargo clippy -p hares-core --tests -- -D warnings` clean.

## Note for the fix phase

`crates/hares-core/tests/i06_ev_nightly_diag.rs` is the diagnose phase's
uncommitted diagnostic harness (per its RCA scope note, it was left for this
phase to formalise). It is superseded by
`equipment_id_regressions.rs` and should be **removed** when the fix lands.
In particular its `i06_equipment_id_probe` asserts distinct `EquipmentId`s
specifically — valid only under the unique-id fix design, and it would fail
post-fix under the name-rekey alternative. Deleting it requires user
approval under this repo's operating rules, so it is left in place and
flagged here rather than removed during reproduction.
