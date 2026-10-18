# I-08 checkpoint roll-up, row 24o

- Date: 2026-10-07 (measurements taken 2026-10-07, 23:20-23:55 local).
- Tip measured: `41935191` (`feat/output-results`, "test: the fleet's
  all-failed error carries its causes, pinned").
- Comparison base: `1d60f478` (local `initial-implementation`, the Task 0
  head; verified an ancestor of the tip: `git merge-base --is-ancestor`).
- Method: cargo `-j 4`, sequential lines. The main tree's tracked files
  were not modified; one scratch worktree (`.workstrees/rollup-rt1base`
  at `31732dc2`, RT-1's parent) was created for the RT-1 before-side
  measurement and removed with its `target` after. No base run at
  `1d60f478` was possible or needed (see 2 and 5).
- Rule: every number below is a measurement taken for this report or a
  citation to its ledger record (file and, where it exists, line).
  Nothing is inferred.

## 1. The comparison base and the schedule's rows

- Commits: `git rev-list --count 1d60f478..41935191` = **299**.
- Schedule rows (`ledger/00-schedule.md`): **195** table rows total, of
  which **100** are rows after the checkpoint (rows 25 to 101a, the
  planned output, Python, fleet, runtime and closing work: not started,
  out of scope). Rows in scope (0 through 24o): **95**.
  - **69** carry a terminal state as recorded (committed 47, folded
    16, recorded 3, landed 2, done 1).
  - Row 16g (the B0 re-check) is done: "the per-SHA sweep ran ...
    rows 1 to 29 through the tip: all identical" (00-schedule.md:181).
  - **22** more landed during the drain since the handoff
    (`db5efe80` -> `41935191`, 213 commits), verified by their ledger
    States and landing commits: 21m OD-34, 21n OD-35, 21o OD-36,
    21p OD-37, 21q OD-38, 21r OD-39, 21t OD-41, 21v OD-43, 21w OD-44,
    21x OD-48, 21z OD-50, 21za OD-51, 21zb OD-52, 21zc BR-25,
    21zd OD-53, 21ze OD-54, 21zf OD-55, 21zg OD-56 (partial),
    21zh OD-57 (partial), 21y OD-49, 22 RT-1, 24g OD-19. Two of the 22
    are partial, not complete (OD-56, OD-57; see 7).
  - **2** rows in scope are open: 16a1 (BR-20, the toolchain pin) and
    24h (OD-21, the attribute/time-zone pins; its three named
    acceptance tests do not exist in the tree at the tip). See 7.
  - The remaining row is 24o itself (this checkpoint).
- Bookkeeping deviation: the schedule's State column was last
  back-filled at the handoff; the drain's landings live in
  `02-measurement.md`'s log and the `11-other-defects.md` State
  entries, not in the State column. The 92-done count above is the
  recorded state plus the verified landings, not the State column
  alone.

## 2. The golden deltas, base to tip

**The base has no goldens.** At `1d60f478`,
`git ls-tree 1d60f478 -- tests/fixtures/golden/` lists only
`.gitkeep`, and `tools/` is empty (no `frame_golden`); M-0's block
record reads "[golden] exit 101: no crate frame_golden"
(`02-measurement.md` M-0, item 1). The tasking's
`git show 1d60f478:tests/fixtures/golden/full_year_hourly.golden.json`
therefore cannot run. The BEFORE values below are read from git at
**`38398bd4`**, the earliest committed golden state whose per-fixture
metrics the ledger records (the OD-30 delta table's base,
`02-measurement.md` "OD-30, 2026-10-05"). Every row between
`1d60f478` and `38398bd4` was bitwise-neutral by its own record (each
of 00-schedule.md rows 0-20 and 16a-16h, and 02-measurement.md's
per-row records), except BR-11's re-capture of `consumer_shape_900s`
(gross 8842.86 -> 8843.67 kWh, the warm-up correction,
`02-measurement.md` BR-11): the BEFORE side is the corrected (cold
start) value. The AFTER values are read from the committed goldens at
`41935191`. Deltas between the two are the arithmetic of the values
shown; the cause families follow the table.

BEFORE = `38398bd4`, AFTER = `41935191` (kWh / kW as labelled;
sources: the two golden.json trees).

| Fixture (home) | Metric | Before | After | Change |
|---|---|---|---|---|
| consumer_shape_900s | gross kWh | 8843.67 | 9149.08 | +3.45% |
| consumer_shape_900s | peak import kW | 3.047 | 3.666 | +20.33% |
| consumer_shape_900s | hvac_heating kWh | 47.40 | 92.74 | +95.66% |
| consumer_shape_900s | hvac_cooling kWh | 1115.74 | 1139.00 | +2.09% |
| consumer_shape_900s | peak hvac_cooling kW | 1.177 | 2.489 | +111.41% |
| full_year_hourly | gross kWh | 9338.21 | 9491.87 | +1.65% |
| full_year_hourly | peak import kW | 5.779 | 6.004 | +3.88% |
| full_year_hourly | hvac_heating kWh | 342.17 | 369.81 | +8.08% |
| full_year_hourly | hvac_cooling kWh | 503.93 | 493.59 | -2.05% |
| full_year_hourly | water_heating kWh | 3760.95 | 3799.16 | +1.02% |
| full_year_900s_dst | gross kWh | 12366.67 | 12945.74 | +4.68% |
| full_year_900s_dst | peak import kW | 24.292 | 24.231 | -0.25% |
| full_year_900s_dst | hvac_heating kWh | 6755.61 | 7230.81 | +7.03% |
| full_year_900s_dst | hvac_cooling kWh | 459.99 | 467.37 | +1.60% |
| full_year_900s_dst | peak hvac_cooling kW | 0.779 | 1.054 | +35.34% |
| leap_year_february_900s | gross kWh | 220.66 | 240.51 | +9.00% |
| leap_year_february_900s | water_heating kWh | 4.23 | 5.86 | +38.60% |
| weather_wrap_900s | gross kWh | 290.58 | 307.74 | +5.91% |
| weather_wrap_900s | water_heating kWh | 3.64 | 4.78 | +31.45% |
| weather_wrap_900s | peak import kW | 21.923 | 21.364 | -2.55% |
| month_boundary_billing_3600s | gross kWh | 354.25 | 373.12 | +5.33% |
| month_boundary_billing_3600s | peak import kW | 5.689 | 5.389 | -5.28% |
| fleet_mixed_offset (home 0) | gross kWh | 120.14 | 98.65 | -17.89% |
| fleet_mixed_offset (home 0) | other kWh | 52.52 | 24.54 | -53.28% |
| fleet_mixed_offset (home 1) | gross kWh | 204.91 | 205.68 | +0.38% |
| fleet_mixed_offset (home 1) | hvac_heating kWh | 89.96 | 88.12 | -2.04% |
| fleet_mixed_offset (home 2) | gross kWh | 167.83 | 176.91 | +5.41% |
| fleet_mixed_offset (home 2) | hvac_heating kWh | 3.58 | 4.89 | +36.60% |

Lighting rises 10.07% on every BEopt-derived fixture (1059.34 ->
1166.00 on the full-year pair) and Other falls by the same absolute
energy: the Lighting aggregate now holds exterior lighting (OD-47,
`0f785f81`; "the Lighting column rises, the Other column falls by the
same share, energy conserved between the buckets",
`11-other-defects.md` OD-47 State).

Gas therms: the golden format stores the gas columns as bit digests,
not therms values; the ledger's recorded gas-furnace trajectory is
89.17 -> 114.99 -> 120.75 therm at the fix-or18c fold (the OS-HPXML
geometry and leakage family; `plan-critique.md` fold register, "OR-18
review items + OD-31"). At the tip the gas columns are pinned bitwise
by the digests since the last re-capture ([golden] identical, both
feature sets, fresh run this session).

Cause families (each cited to its fold record; the fine per-leaf
attribution is in the named records):

1. Appliance gains reach their zone, split per OS-HPXML; replay by the
   run's own schedule rows; the radiant/visible split (OD-30,
   `9a32e928`, `a71f0af8`, `e2785ed1`, `320f85c2`): the per-fixture
   deltas are the OD-30 table (`02-measurement.md` lines 1597-1649);
   the replay-date causes per fixture (cooking/other/water-heating
   moves) at lines 1659-1679.
2. Zone capacitance multiplier x7, OS-HPXML zone geometry and leakage,
   gable volume, terrain factor, material roughness (OR-18c / OD-31 /
   OD-53, D-011, D-010; re-capture commits `e2a1024c`,
   `132ed237`..`e903016e`): the equivalent battery's consequence for
   the consumer is recorded in `02-measurement.md` "OR-18c" (bldg0176227
   0.2521 -> 1.7644 kWh/K).
3. The ideal dispatch's setpoint landing and the boiler EBM window
   (`0eaf4dbc`, CHECKPOINT_VERSION 13 -> 14): consumer_shape_900s
   hvac_cooling 1115.3 -> 1118.9, hvac_heating 95.1 -> 94.7 kWh, and
   per-fixture deltas in the commit message.
4. Diffuse short-wave leaves back out through the windows (OD-44,
   `df784f23`, D-016): gross moved <= 0.005%, cooling down where
   cooling runs, heating up slightly (commit message).
5. Initial zone humidity from the weather; door zones from the
   attached wall (`31732dc2`): every golden moved from the first block
   (per-fixture difference counts in the commit message).
6. Appliance energies take the occupant-adjusted bedroom count
   (OD-51's bedroom fix, `44ee8bfe`): closes the 24 h window's Other
   and cooling gaps (OD-51 State).
7. ASHP default curves per the RESNET anchor model (OD-54,
   `d3d676af`, re-captures `77fe396d`, `74ab9ba8`); heat-pump goldens.
8. D-017/D-018 registration and the through-glass column split
   (`2c0c7da3`, re-capture `ba1ddf15` for the dst fixture).

The cycling fix (`1846db41`) and OD-39's basement merge
(`440f028a`) left the six goldens bitwise (commit messages); their
physics shows in the 60 s parity corpus and the oracles instead.

Current state: `[golden]` identical in both feature sets at
`41935191` (fresh run, this session; 6 default + 1 dst, all seven).

## 3. BESTEST: the drift locks

Lock values, bands and the recorded history: `tests/bestest/mod.rs:106-140`
(re-measured 2026-10-07 at the combined tip; the comments carry each
lock's cause). Fresh run this session (`cargo nextest run -p hares-core
--release --test bestest`): **12/12 passed**, every out-of-band metric
on its lock (DRIFT-LOCK-OK), every in-band metric strict.

Out of band, locked at the measured baselines (value; ASHRAE band;
distance to the nearest edge):

| Case | Metric | Value | Band | Distance |
|---|---|---|---|---|
| 600 | annual heating | 3122.278 | [4296, 5709] | -1173.7 |
| 600 | annual cooling | 5542.032 | [6137, 7964] | -595.0 |
| 640 | annual heating | 2086.165 | [2751, 3803] | -664.8 |
| 900 | annual heating | 1105.795 | [1170, 2041] | -64.2 |
| 600FF | peak zone temp | 73.210 | [64.9, 69.5] | +3.71 |
| 900FF | peak zone temp | 45.963 | [41.6, 44.8] | +1.16 |
| 900FF | min zone temp | 3.179 | [-6.4, -1.6] | +4.78 |

In band, strict: 900 annual cooling 2583.128 in [2132, 3415];
600FF min zone temp -11.950 in [-18.8, 0.0] (both fresh, this
session). The residuals are the documented state the locks pin against
silent drift, not conformance claims (`mod.rs:54-59`).

Movement causes (the locks' comments and the fold records):

- The ideal dispatch's setpoint landing (`0eaf4dbc`): 600 heating
  3222.6 -> 3100.9, 600 cooling 6134.0 -> 5737.7, 640 heating
  2144.3 -> 2065.8, 900 heating 948.2 -> 1061.2 ("toward its ASHRAE
  band, distance -222 to -109").
- The diffuse short-wave window loss (OD-44, `df784f23`): 900FF peak
  46.61 -> 45.96, min 3.44 -> 3.18 (the lock's comment: the peak
  -0.65 K, the min -0.26 K), 600 cooling to 5905.8 in the same
  record.
- The freefloat through-glass flux for 600FF: the peak 73.73 ->
  73.21 (-0.52 K, -0.70%), inside the lock's 1% tolerance; recorded
  at `e5ab430c`.
- The cycling fold (`1846db41`) recorded the locks untouched ("the
  BESTEST is unchanged, the ideal path at 3600 s, 12/12, the drift
  baselines untouched"). No lock movement is recorded for the
  capacitance/geometry (OR-18c) or gain-split (OD-30) folds; the
  current locks' recorded causes are the dispatch landing and the
  diffuse loss (`mod.rs:109-126`).

## 4. OCHRE parity

**The twelve parity cases pass.** OD-47's record: "the strict xfail
heals: the mark is off, the case passes within 1e-9, all twelve parity
cases pass" (`11-other-defects.md` OD-47 State). Fresh block run this
session: pytest **1582 passed, 5 xfailed, 0 skipped, 0 failed**
(the census empty and identical to the ledger's census file).

The five strict xfails at the tip: the two thermal-trace temperature
cases (below) and the 24 h window's Heating (+4.83%, the D-012
envelope class), Water Heating (12.2x, D-017) and Total (their sum)
(`11-other-defects.md` OD-51 State; the pairs quoted in
`tests/python/test_thermal_trace.py` / the parity suite's records
test).

**The 24 h window's end-use closings** (OD-51 State, 2026-10-07): the
spec's cooling 2x and Other +18% gaps closed exactly - cooling
0.950 = 0.950, Other 7.4094 = 7.4094 (worst member 1.9e-14) - by the
bedroom fix (HARES read the raw `NumberofBedrooms` where OCHRE applies
the occupant-adjusted count, `ochre/utils/hpxml.py:791-800` applied at
:1664). B-16/B-17 own about 0.35 kWh of the 24 h water-heating gap.

**Corpus MAE trajectory** (conditioned-zone MAE, the 60 s corpus;
record at the OD-38 fold `1846db41` -> fresh at the tip, this
session's `--test regression --success-output` run, 19/19 passed):

| Fixture | At the OD-38 fold | At the tip |
|---|---|---|
| cz2a_gas_furnace_ac_res_wh | 0.393 -> 0.252 | 0.2734 |
| cz6b_resistance_res_wh | 0.345 -> 0.251 | 0.2663 |
| resstock_bldg0112631_24h | 0.599 -> 0.240 | 0.3432 (band 0.63) |
| cz4a_ashp_hpwh (ideal path) | 0.355 -> 0.355 | 0.4586 |
| cz5a_minisplit_gas_wh | - | 0.2796 |
| cz2a_pv_ev | - | 0.3674 |

(The later physics folds - the basement merge, the diffuse loss, the
humidity init - moved the corpus after OD-38's record; the tip values
are the fresh measurement. The tasked starting point "cz2a 0.454"
appears in no ledger record found; the earliest recorded corpus start
is 0.393 at `1846db41`'s parent. See the deviations section.)

- cz4a: the comparison was made fair and the vendored fork's
  `hp_cop` defect fixed; residual 5.26% HVAC / 4.89% site, inside the
  default bands (OD-61 State, 2026-10-06).
- cz5a's peak question: the duct remainder closed (1.17% site); the
  residual is the recovery minute's maximum draw, HARES 17.84 kW
  against OCHRE's 14.81 (energies 1.2% apart), band 21.5 with the
  measured cause; which side's maximum draw the reference tools
  support is not established - the open question lives at
  `tests/parity/mod.rs:47-56` (OD-61 State).
- resstock_bldg0112631_24h peak: 25.4% on a 25.7% lock; the code's
  comment at the tip states the cause "is not yet established"
  (`tests/parity/mod.rs:71-74`), classed with cz6b as D-012's fitted
  RC network in OD-61's record; the two statements do not fully agree
  (see deviations).

**Thermal-trace records trajectory** (seeded, against the vendored
OCHRE):

1. OD-50's seeded record (2026-10-05): first-6 h worst 2.41 C,
   48 h mean 0.82 C, overnight worst 4.72 / mean 0.88, window solar
   1.305x OCHRE, 48 h heating 39.3 vs 47.1 kWh
   (`11-other-defects.md` OD-50 Problem).
2. The capacitance/geometry physics "roughly halved every trace
   metric" (`fd615fed`).
3. The ideal dispatch fix (`0eaf4dbc`) + re-measure (`a61e5502`):
   worst 6 h 1.3243 -> 0.8749, 48 h mean 0.3938 -> 0.4582, heating
   30.06 -> 42.56 vs 47.05 (was 36% under, now 9.5% under).
4. At the tip (OD-50 State 2026-10-07; the records hold in
   `tests/python/test_thermal_trace.py:333-340`): worst 6 h
   **0.7066** (threshold 0.1), 48 h mean **0.3697** (threshold 0.5),
   overnight 1.3031 / 0.3672, solar ratio **0.9991** (worst 0.96%),
   heating 41.553 vs OCHRE's 47.052 (48 h -11.7%, day-1 +4.8%). The
   window-solar record was a HARES output-column bug, fixed
   (`e2b7565d`); the step-1 infiltration lead does not reproduce
   seeded (0.7% at step 1). The remaining divergence is the envelope
   thermal response, D-012's class: the two temperature cases stay
   strict xfails. Net: 2.41 -> 0.7066 (-71%).

## 5. Performance

**The base cannot run the profiling surface.** At `1d60f478` there is
no `frame_golden` and no profiling feature (`tools/` empty,
`git ls-tree`; the profiling phases arrived with PR-1..PR-6, rows
14-18, after the base). The baseline is therefore the M-3-era record,
and the "after" is a fresh measurement.

Baseline (M-3, re-measured at `8b09ac4a`, the valid baseline;
`02-measurement.md` M-3 2026-10-02 record): simulate() wall medians
1.533 s (in memory) / 1.559 s (file written) / 1.433 s (nothing kept)
on `consumer_shape_900s` (35,040 steps); phase shares envelope 9.1%,
schedule 4.7%, hvac 3.5%, `other` 70.3%; telemetry retention about
**0.11 kB/step** (+111,328 kB over 10^6 steps); retained frame
15,340,640 B.

At the tip (fresh, this session, `frame_golden run -F profiling
consumer_shape_900s --repeat 3 --discard-output`, release, `-j 4`):

- step_total **1.213 / 1.214 / 1.256 s** (median 1.214 s; about
  34.6 microseconds per step); simulate() wall 1.216 / 1.217 /
  1.258 s.
- Phases (the partition sums to step_total in exact Duration
  arithmetic): envelope 39%, equipment 14%, environment 12%,
  **state_snapshot 18%**, control 9%, ideal_capacity 5%, invariants
  2%, dispatch 1%, accounting 1%, output 0%, actors 0%.
- memory_high_water_kb **147,580 -> 159,712** (the HWM read once per
  summary, monotone within the process).
- hot_path_allocations **666,009** across 35,040 steps
  (hot_path_alloc_violations 35,040: the recorder still allocates;
  the output-reporting rows that own it are after the checkpoint).

Same-session A/B this session (the only noise-safe kind here; the
ledger's own rule: absolute wall drifts between sessions, M-3-era
1.53 s vs 1.19 s for the same binary, `02-measurement.md` PR-1):

- RT-1's parent `31732dc2`, measured in the scratch worktree
  (`-F profiling`, same command): step_total 1.154 / 1.181 / 1.181 s
  (median 1.181), phases identical to the tip's (state_snapshot 18%),
  hot_path_allocations **7,603,732**.
- The tip: step_total median 1.214 (+2.8%: within the 5% noise band;
  RT-1 removed allocations, not time) and hot_path_allocations
  **666,009**: **-91.24%** (7,603,732 -> 666,009; the tasking's
  after-number 666,008 has no ledger record and differs by one
  allocation from the fresh measurement; the before-side matches the
  tasking exactly and is now measured, not just tasked).
- OD-38's bench (recorded, same-session at its fold): the
  single-building 30-day median at 60 s **1.125 -> 1.011 s,
  about -10%** (`1846db41` commit message).
- state_snapshot's share: PR-1's era record 8%
  (`02-measurement.md` PR-1 table) -> **18%** fresh (the same 18% at
  RT-1's parent; single-rep shares swing, the partition is exact).

Peak memory: the M-3-era peak-RSS records (72.58 MB warm run,
BR-6b's record) are profiling-example quantities, not the summary
HWM; the summary HWM at the tip is 159,712 kB fresh with no earlier
HWM record to compare against (PR-2's red-team observation
147,496 -> 179,788 kB across reps is the only prior HWM record).
No step-time regression is measurable anywhere in the same-session
pairs.

## 6. The test counts' trajectory

**Base (`1d60f478`), cited from the M-0 record, not re-run.** The
base's own block run is M-0's record at exactly this sha
(`02-measurement.md` M-0): pytest **1377 passed, 9 skipped**; the
[test-debug] line exit 0 (no count recorded); [test-release] 6954 of
6957 passed (the three failures OR-5 healed). A fresh base count
would have required a scratch-tree build to reproduce a number M-0
did not record; the M-0 record is cited instead (the tasking's
else-branch).

**Tip (`41935191`), fresh this session** (all lines of the block,
below in 8):

| Line | Count |
|---|---|
| nextest [test-debug] | **7591** passed, 0 skipped |
| nextest [test-observe] | **7653** passed |
| nextest [test-dst] | 7604 passed |
| nextest [test-profiling] | 7597 passed |
| [test-observe-profiling] | 1568 passed |
| nextest [test-release] | 7545 passed |
| pytest | **1582 passed, 5 xfailed, 0 skipped** |
| pyright | **0 errors, 0 warnings** (exit 0) |
| regression suite | 19/19 |
| BESTEST | 12/12 |
| ASHRAE reference check | PASS |

(The tasking's 7589/7648/1583 match the tree one or two commits
before the tip; the last commits add the fleet-error pins. The fresh
counts above are the tip's.)

Gate inventory at the tip (each with its owner row):

- `[clippy]` x9: one line per profile and feature set the block
  builds (BR-21): all-features, default, dst, profiling, observe,
  observe,profiling, release, release-dst, release-observe. All
  exit 0 fresh.
- `[doc]`: `RUSTDOCFLAGS="-D warnings" cargo doc --workspace
  --all-features --no-deps`, exit 0 fresh (BR-25, `f01347d4`; also a
  CI docs-api job).
- The skip gates: the pytest census (the ledger's census file is
  empty since OD-13; the fresh diff is identical) and CI's OCHRE job
  running `pytest -m ochre -rxX --fail-on-skip` on push and PR
  (headaudit's fix, `.github/workflows/ci.yml:270-294`); the vendored
  OCHRE commit check (`check-ochre-commit.sh`).
- The BESTEST drift locks: in-tree, every out-of-band metric pinned
  to its measured baseline at 1% (3 above).
- B0: the last recorded de-trace check is **identical at
  `db5efe80`** (`02-measurement.md`, rows 24k-24n). The physics folds
  after it moved the engine's numbers per the cited causes (section
  2), so B0 identity is not expected at the tip; the checkpoint's
  consumer gate (gate 4) is the consumer's own run against the
  roll-up tip, not a B0 comparison. No newer B0 record exists.
- `[advisories]` exit 0, `[bench]` exit 0, `[stray]` clean,
  `[writing]` clean, `[tools]` unchanged from M-0 (rustc 1.94.1,
  nextest 0.9.137, cargo-deny 0.20.2, uv 0.12.8).

## 7. Known open items (honest)

1. **BESTEST 600/640 (and 900/600FF/900FF) out-of-band residuals**:
   the documented state, locked against drift (section 3). Not
   conformance; the bands stay the target.
2. **The thermal-trace envelope-response xfail** (2 strict xfails,
   D-012's class): worst 6 h 0.7066 against the 0.1 C threshold
   (section 4).
3. **The conditioned-attic winter bias** (-1.26 C; HARES +0.32
   against OCHRE's +1.56): open in OD-56 (PARTIALLY CLOSED; the
   window-solar candidate was the reporting bug, the ground candidate
   is D-018; no HARES defect established for the attic-floor
   coupling; all oracle bounds unchanged and holding:
   0.773/0.896/1.271 ideal, 0.791/0.899/1.299 dynamic).
4. **The cz5a peak question** (17.84 vs 14.81 kW at the recovery
   minute; band 21.5): which side's maximum draw the reference tools
   support is not established (`tests/parity/mod.rs:47-56`).
5. **The resstock_bldg0112631_24h peak cause**: the code comment says
   not yet established; OD-61's record classes it with D-012 (see
   deviations).
6. **MSHP default curves** (D-020, class unset): the evenly spaced
   default stages at a flat EIR against the reference's per-product
   calibrated stages; a candidate cause beside the mini-split peak
   question (`413c75aa`).
7. **B-16/B-17**: the wet-appliance hot-water volume slice (B-16 the
   confirming measurement, B-17 the fix, ~0.35 kWh of the 24 h
   water-heating gap; rows 94k/94l, the numeric block after the
   checkpoint).
8. **OD-57's leftovers, verified in code at the tip**: the unhandled
   zone-type match still warns and maps to Outdoor
   (`crates/hares-core/src/dwelling/conversions.rs:389-397`), and the
   loop allocator still swallows typed-config errors through
   `.ok()?` (`crates/hares-core/src/dwelling/loop_allocator.rs:826,
   833`). The row is partial, not done.
9. **OD-62** (a non-ideal unit's mode override sticks after its DR
   event), **OD-63** (`Fleet::from_resstock`'s zip paths and unset
   weather FIPS), **OD-64** (the blueprint's empty-defaults
   fallback): recorded, open (`11-other-defects.md`).
10. **OD-59 + operator-owned**: the helics wheel's libstdc++ defect
    (the local conftest mitigation stands until an upstream report;
    the operator sends it); the `CODECOV_TOKEN` secret; any
    publication of the local `vendors/OCHRE` fork commit; every push
    (`handoff-INDEX.md` Operator-owned).
11. **Row 24h (OD-21) is open**: its three acceptance tests
    (attribute/line-end normalization, the malformed-attribute parse
    error, the site-offset lookup pin) do not exist in the tree.
12. **Row 16a1 (BR-20) is open**: no toolchain pin file exists; the
    toolchain versions are recorded per run ([tools]) and pinned in
    CI, which may be the intended shape - the row was never closed
    either way.
13. **The schedule's State column** needs a back-fill for the drain's
    22 landings (section 1); OD-58 and OD-60 also landed in the
    sweep (verified: the v3 `ShieldingofHome` spelling is parsed,
    `building.rs:584`; the constant-power schedule errors on an
    undeterminable power, `3099d2ce`) without State updates.
14. From the handoff's finds, still open: the `[bench]`-only
    compile-smoke suggestion (a run-time panic in a bench goes
    unseen); OD-46's bag channel is FIXED (in the sweep) but the
    M-1-class audit items it bundled were tracked separately.

## 8. The verification block's final state at 41935191

Fresh run, this session, 2026-10-07 23:30-23:55, cargo `-j 4`,
sequential lines, `bash -euo pipefail` semantics per line; raw log:
`ROLLUP-24o-block.log` (beside this report; the run's own artifacts
under `target/` only). The first pass's clippy and golden lines were
invoked malformed by this session (a `-j` after the `--` separator;
`-j 4` given to frame_golden) and are the EXIT 101 / EXIT 1 entries
in the log; the clean re-runs of those 12 lines follow them and are
the ones cited here.

| Line | Exit | Count / result |
|---|---|---|
| [tools] | 0 | rustc 1.94.1, nextest 0.9.137, deny 0.20.2, uv 0.12.8 |
| [fmt] | 0 | clean |
| [clippy] x9 (all profiles) | 0 each | 0 warnings |
| [test-debug] | 0 | 7591 passed |
| [test-observe] | 0 | 7653 passed |
| [test-dst] | 0 | 7604 passed |
| [test-profiling] | 0 | 7597 passed |
| [test-observe-profiling] | 0 | 1568 passed |
| [test-release] | 0 | 7545 passed |
| [bench] (--no-run) | 0 | 14 bench executables built |
| [advisories] | 0 | no ignores |
| [golden] (default) | 0 | 6/6 identical |
| [golden] (-F dst) | 0 | 1/1 identical |
| [python-build] | 0 | maturin develop --release |
| [pytest] | 0 | 1582 passed, 5 xfailed, 0 skipped; census identical |
| [pyright] | 0 | 0 errors |
| [doc] | 0 | clean |
| [stray] | 0 | before == after; TMPDIR empty |
| [writing] | 0 | commit message and added lines clean |
| ASHRAE reference check | PASS | fixture matches the generator |

Gates 1 (the audits of the operator commits and the OR-19 fold), 4
(the consumer gate against the roll-up tip) and 5 (the fast-forward
of `initial-implementation`) are outside this report and remain to
run; this report is gate 3's evidence.

## Deviations: what would not survive scrutiny

1. **The BEFORE golden values are at `38398bd4`, not `1d60f478`.**
   The base predates the golden infrastructure; the tasked
   `git show 1d60f478:...` is impossible (verified). The chain
   "1d60f478 -> 38398bd4 bitwise by each row's record" is a citation
   chain over ~40 row records, not a single bitwise proof.
2. **The tasked corpus start "cz2a 0.454" was not found** in any
   ledger record or commit message; the recorded start is 0.393
   (`1846db41`). The report uses the recorded value.
3. **The tasked tip counts (7589/7648 nextest, 1583/5/0 pytest) do
   not match the tip's fresh counts** (7591/7653, 1582/5/0); they
   match the tree a commit or two earlier. The report uses the fresh
   counts.
4. **RT-1's after-number**: fresh 666,009 vs the tasked 666,008
   (+1 allocation, a later commit's probe boundary). The before-side
   (7,603,732) is now measured fresh at `31732dc2`, matching the
   tasked number.
5. **No base run at `1d60f478` was performed** (nothing to run: no
   goldens, no frame_golden, no profiling feature); the sanctioned
   `.workstrees/rollup-base` was not created. The base's counts are
   the M-0 record's, per the tasking's else-branch; the M-0 record
   has no nextest count for the base (only exit 0).
6. **Cross-session wall comparisons carry the ledger's noise
   caveat**; the M-3-era-vs-tip step-time figures are indicative.
   The same-session pairs (RT-1's A/B, OD-38's bench) are the
   noise-safe ones.
7. **B0 is not identical at the tip by design** (the physics folds
   moved numbers, each with its recorded cause); the last identical
   record is at `db5efe80`. The consumer gate must not be run as a
   B0 comparison.
8. **The resstock peak cause** is stated two ways (the code comment:
   not established; OD-61: classed with D-012). The discrepancy is
   the OD-61 disposition's wording against the band comment at the
   tip; unresolved here.
9. **Two rows above the checkpoint are open** (24h OD-21, 16a1
   BR-20) and two are partial (21zg OD-56, 21zh OD-57), against the
   checkpoint row's "every row above ... done". The roll-up reports
   this; the checkpoint's owner decides.
10. The schedule's State column is not back-filled for the drain
    (section 1); the row accounting here is this report's own
    reconciliation, not the ledger's column.
11. The main tree carries pre-existing untracked artifacts
    (`HeatBalanceSurfaceManager.cc`, `data/examples/freefloat_*`);
    tracked files are untouched and the [stray] before/after gate is
    clean. They predate this session.
