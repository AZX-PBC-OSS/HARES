---
passing_commands:
  - command: cargo fmt --check --all
  - command: cargo clippy --workspace --all-targets --all-features -j 8 -- -D warnings
  - command: cargo nextest run --workspace --no-fail-fast --build-jobs 8
  - command: cargo nextest run --workspace --no-fail-fast --build-jobs 8 -F observe
  - command: cargo nextest run --workspace --no-fail-fast --build-jobs 8 -F dst
  - command: cargo nextest run --workspace --no-fail-fast --build-jobs 8 -F profiling
  - command: cargo nextest run -p hares-core --no-fail-fast --build-jobs 8 -F observe,profiling
  - command: cargo nextest run --workspace --release --no-fail-fast --build-jobs 8
  - command: cargo bench --workspace --no-run -j 8
  - command: cargo run --release -p frame_golden -- compare-all
  - command: cargo run --release -p frame_golden -F dst -- compare-all
  - command: uv run --locked --all-extras --all-groups maturin develop --release --uv
  - command: uv run --all-extras --all-groups --no-sync pytest -m "" -rs -p no:cacheprovider
  - command: (consumer repo, engine under test) HARES_REPO=/home/rich/src/HARES uv run --no-sync python -m calibration.tests.run_all
  - command: (consumer repo) HARES_REPO=/home/rich/src/HARES uv run --no-sync ruff check
  - command: (consumer repo) HARES_REPO=/home/rich/src/HARES uv run --no-sync python tools/capture_de_trace.py compare .shipwright/initiatives/I-08/baselines/B0-de-trace.json <capture>
---

# Implementation: I-08 rows 0 to 21b (through OR-18)

## What was done

The I-08 ledger's rows 0 to 21b, executed row by row per
`ledger/00-schedule.md` and verified against the verification block.
Rows 0 to 18 carried a review and red-team pass before commit. From row
19 on, each commit is audited and red-teamed independently after it
lands, and its findings are fixed as entries or follow-up commits: OR-11's
three unresolved rows became OR-18 and its roster defect OR-19; SL-7,
RT-3 and RT-3a had no defect of their own (their audits found the
pre-existing BR-21 and RT-9); OR-18 landed as one commit instead of its
four and is under audit and red team. OR-19 (row 21c) is committed on
branch `i08-fixes` (`9ef347e0`, follow-up `ffa9051f`) and under review;
independent audits of rows 19 and 20 are recorded in
`verification/audit-OR-11-b8195c83.md` and
`verification/audit-SL-7-7b7a6a7d.md`, and the red team against both is
recorded in `verification/redteam-OR-11-SL-7.md`. The research behind
OR-18's fix for OR-11's stopped census rows is in
`verification/OR-11-stopped-rows-research.md`.

Engine commits on `feat/output-results` (nothing pushed):

| Commit | Row | Summary |
|---|---|---|
| `1d60f478` | Task 0 | the local merge of PR #27 with `ebdb1a17`, fast-forwarded onto `feat/output-results` |
| `aa22be6c`, `c88225df` | BR-1 | the observe binding at the battery's daily degradation boundary captures `sum_sq_dod`; the workspace compiles under `-F observe` and `--all-features` |
| `c4ecc7d3` | BR-3 | the nested DST test un-nested to module scope as `spring_forward_day_of_year_at_hourly_boundary_steps`; it runs and passes for the first time |
| `fd23dd80` | BR-2 | clippy clean under every feature set (map_or, &Path, struct literals, the profiling example's cfg gating) |
| `21d64b7b` | BR-9 | `.config/nextest.toml` bounds test parallelism at 8 in the default and ci profiles (the block's concurrent runs OOMed the host at the default 28) |
| `dc033cee`, `469ea955` | OR-5 | health recorded and physics invariants enforced in every build profile, warm-up included |
| `363b4607` | BR-4 | CI runs the block's clippy line and the default, dst, profiling and plain-release test builds |
| `81cc6d85` | BR-7 | tests stop writing `dwelling_*.csv` into the tree; CI gains the stray check |
| `b674a767` | BR-8 | the eleven ignored print-only tests deleted; `#[ignore]` is gone from the tree |
| `979b8773` | M-1 | `tools/frame_golden`: the golden capture and bitwise compare tool |
| `4a450949` | (found, BR-16) | the EV's `soh` binding gated to the builds that read it |
| `7643b4e7` | M-2 | the six golden fixtures and the consumer-shape defaults variant |
| `59b060d6` | (found, BR-17) | the golden round-trip tests name the running feature set |
| `39488581` | BR-6a | `rust-version` raised to 1.89; nested ifs collapsed into let chains |
| `f880a6d7` | (found, BR-18) | a zero-row golden product reads back as a schema-only batch |
| `872cf289` | BR-6b | every Rust dependency upgraded to its latest release (turns `[advisories]` green) |
| `75a182a0` | BR-6c | Python dependencies and maturin upgraded to their latest releases |
| `8b09ac4a` | BR-11 | a golden manifest with `initialization_duration_s = 0` runs no warm-up; `consumer_shape_900s` re-captured |
| `d7f0e381` | PR-1 (fixes SL-9) | profiled phases partition the step so they sum to the step time; the profiling example deleted |
| `21d90588` | PR-2 | the memory high-water mark read once per summary, not four times per step |
| `26626710` | PR-3 | one per-thread counting allocator for hot-path allocations |
| `1d471677` | PR-5 | the profiling binding raises instead of returning zero timings when profiling is not compiled in |
| `afa9a071` | PR-6 | per-actor timings accumulated over the run and reported in the profiling summary |
| `b8195c83` | OR-11 | every `check_invariants` gate classified and enforced in every build profile; the feature deleted |
| `7b7a6a7d` | SL-7 | the unreachable coupled-LU path and identity solves removed from the state-space model |
| `44da330c` | RT-3 | the thermal solver steps without heap allocation |
| `69950a98` | RT-3a | the ideal-capacity solve's shared terms computed once per step |
| `1c18543a` | OR-18 | OR-11's three unresolved census rows closed in one commit: water heaters in unmodelled locations on the OS-HPXML ambient (fleet golden home 1 moves, delta recorded), dehumidifier zone, synthetic envelope closure, IdealHvac end use |
| `fe804b59` | OD-16 | the actuate release signal no longer zeroes the thermostat hysteresis; tolerated step failures reconcile the consistency sums |
| `32044ab5` | BR-13 | golden comparison fails when the defaults tree differs from the capture |
| `c6889589` | BR-14 | tariff files read strictly; golden manifests carry a tariff; the month-boundary billing fixture |
| `50a5c261` | BR-15 | fleet golden manifests state each home's weight and the aggregation resolution |
| `4a1f2c19` | BR-10 | CI runs on the working branch and its pull requests; observe-profiling and golden lines added |
| `3ff4583a` | OD-15 | investigated, no defect: a January run's cooling energy is the crankcase heater; pinned by a test |
| `6646c801`, `aab05da0` | BR-12 | the seasonal ResStock tests compile and run; unbuilt test files deleted; smoke bounds separate loads from PV |
| `f719895b`, `b7ea5d0a`, `b3807202` | OR-18 (fixes) | ambient heat pump wet-bulb, no invented zones, OS-HPXML water heater location default, fleet golden provenance (folded from `fix-or18`) |
| `3005ee1c`, `a4b29269`, `59465557` | OD-29 | each event load on its own random stream with an order-free identity (folded from `fix-od29`) |
| `91815306`, `19ca1069`, `0c0bece9`, `ede151da`, `398d21a6` | BR-21 | every profile and feature set linted; locked gates; OCV tables checked against the generator; unused-import suppressions removed; goldens' defaults digest re-captured |
| `2a39a8ee`, `89eae0ba`, `d09a3b64` | RT-9 | the thermal solver's restore covers its coupling, ventilation recovery and fallback state; checkpoint shape pinned at version 10; atomic load (folded from `fix-rt9`) |

On branch `i08-fixes` (found defects in committed code, brought onto
`feat/output-results` once reviewed and red-teamed):

| Commit | Row | Summary |
|---|---|---|
| `9ef347e0` | OR-19 | equipment and actor roster changes are all-or-nothing |
| `ffa9051f` | OR-19 (follow-up) | assembly through the roster plan; rejected changes consume no equipment id; planning never moves the roster out |

Consumer commits on `i08-consumer` (nothing pushed):

| Commit | Row | Summary |
|---|---|---|
| `824805e` | CN-0 | the bitwise DE-trace capture and compare tool; registry baseline |
| `a6d1345` | CN-16 | the EV symptom named without a cause the code does not establish; a missed detection fails with its own message |
| `f7131d7`, `aec2e3f` | (found, BR-19) | a capture refuses an installed engine from outside the tree its header names |
| `5c651bc` | BR-6d | consumer Python dependencies upgraded to their latest releases |
| `18a8ec3` | CN-17 | the contract check reads the engine tree every gate resolves; the bundle is written where the gate names |

Engine HEAD is `2f85338d` on `feat/output-results` (2026-10-05). On top of `8082592e`, in order: `787487df` (BR-23: the Python package type-checks clean), `901354e0` (OD-12), `699cfad2` (OD-14: pint uncapped), `a7a9253d` (OD-40: autosized heating-only heat pump keeps the core-output contract), `ee7bee85` (OD-42 with OD-45: parameter overrides reach the typed configs), `249912f9` (OD-3: autosized boiler keeps its indirect tank's loop), `13881b48` (OD-6: a missing schedule path is an error), then the OR-19 fold `e5b9ee42` .. `2f85338d` (roster changes plan then install, all or nothing; a rejected add leaves the dwelling untouched; `CHECKPOINT_VERSION` 11). Earlier: (`8082592e`, BR-22: lint suppressions removed by fixing what they hid, 64 of 95 sites, 31 kept with reasons; before it `d9133308`, OD-13: the OCHRE comparison and fleet conversion tests run instead of skipping; the pytest skip census emptied). Fold state per fix branch: `plan-critique.md` "Fold register". BR-21 was fast-forwarded onto `38398bd4` as `91815306`, `19ca1069`, `0c0bece9`, `ede151da` and `398d21a6`. Between `d09a3b64` and `38398bd4`: `39113e2d` (the checkpoint shape test's pinned version follows the fold), `5d66dafc` (the thermal-trace parity tests expect only their divergence assertion) and `38398bd4` (the vendored OCHRE sentinel writes at the column's own resolution). `verification/fold-integrity-5d66dafc.md` proves every folded fix is on the head intact; consumer HEAD is
`5c651bc` on `i08-consumer` (`git -C /home/rich/src/HARES log --oneline`,
`git -C /home/rich/src/HARES_CALIBRATION_SYSTEM log --oneline`). Row 21c
(OR-19) is committed on branch `i08-fixes` and under review; rows 16a to
16h have not started.

Baseline B0 is captured at consumer `aec2e3f` against engine `1d60f478`
(`baselines/B0-de-trace.json`, `B0-losses.json`, `B0-record.json`); the
production set reproduced bitwise identical at every engine row through
OR-11 and SL-7, the last recorded checks at `b8195c83` and `7b7a6a7d`
(`ledger/02-measurement.md`, the OR-11 and SL-7 entries).

## Verification block state at HEAD (`7b7a6a7d`)

Green through row 20 (`ledger/02-measurement.md`'s OR-11 and SL-7
entries): `[fmt]`, `[clippy]`, `[test-debug]`, `[test-observe]`,
`[test-dst]`, `[test-profiling]`, `[test-observe-profiling]`,
`[test-release-invariants]` (release suite 7086 at OR-11),
`[test-release]`, `[bench]`, `[golden]` (both feature sets, every
fixture identical in all three buildable profiles at SL-7),
`[python-build]`, `[pytest]`, `[stray]` (empty), `[writing]`,
`[advisories]` (turned green by BR-6b, row 11, `872cf289`). M-0's
reference record is in `ledger/02-measurement.md`. Independent audits
of OR-11 and SL-7 are `verification/audit-OR-11-b8195c83.md` and
`verification/audit-SL-7-7b7a6a7d.md`; the red team against both is
`verification/redteam-OR-11-SL-7.md`.

### Deviations

- Task 0's PR #27 merge was made locally (the GitHub PR is still open;
  nothing is pushed); the push and the GitHub merge remain operator
  checkpoints (`dl-i08-execution.task0-state`).
- Engine-under-test worktrees live at `.workstrees/` per the operator,
  not `~/src/hares-eut/` (`dl-i08-execution.worktrees`).
- BR-1 needed a formatting commit and OR-5 a gate/doc commit behind it
  (history rewriting was unavailable): `c88225df`, `469ea955`.
- BR-8's commit also re-enables the alignment oracle helper's Parquet
  output (BR-7's default flip had silenced it and failed five fixture
  tests); the hunk-level commit split was unavailable.
- OR-5 un-gated enforcement-data sites the ledger's table did not
  enumerate (the `prev_humidity_ratios` bookkeeping, the thermal-balance
  terms in hares-envelope, the port-core electrical consistency check):
  release runs the checks on the same data debug always used; the
  release parity corpus passes bit-for-bit.
- frame_golden records what the engine's current surface provides:
  fleet covered weights one row per home, fleet health null, the
  defaults digest in sorted relative-path order; the fleet aggregate
  pins today's empty mixed-offset behaviour that FL-4 replaces.
- The pytest skip census's line numbers were refreshed in place (same 9
  skips) after BR-7's edits shifted the import sites.

### Known Limitations

- **What:** `RunHealth` is reachable through `Dwelling::health()` but
  not yet in a run result; the consumer still reads no health columns.
- **Constraint:** `RunResult` is OR-6's row; the consumer reads health
  in CN-1, which depends on it. The ledger's shape is followed in
  order.
- **Tried:** none; surfacing it early would fold two rows' contracts
  into one commit.
- **Resolvable when:** OR-6 lands.

- **What:** the gym env's dict path silently drops `write_output`.
- **Constraint:** pre-existing Python-side behaviour outside this run's
  rows; recorded in `dl-i08-execution.progress` as an instance of
  OD-2's class.
- **Tried:** none; it gates nothing until the consumer migration.
- **Resolvable when:** OD-2 (row 31) lands.
