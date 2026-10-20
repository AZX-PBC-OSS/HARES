# Row 16g — B0 re-check sweep (DE trace bitwise-neutral verification)

- **Started:** 2026-10-04 (epoch 1791120182)
- **Baseline:** `/home/rich/src/HARES/.shipwright/initiatives/I-08/baselines/B0-de-trace.json` (captured at 1d60f478)
- **Consumer:** /home/rich/src/HARES_CALIBRATION_SYSTEM @ i08-consumer (18a8ec3)
- **Engine repo:** /home/rich/src/HARES @ feat/output-results (branch tip moved: fe804b59 → 50a5c261, so 32044ab5, c6889589, 50a5c261 added after fe804b59)
- **Method:** per row — reuse/create `.workstrees/<sha>` worktree; `CARGO_TARGET_DIR=/home/rich/src/HARES/.workstrees/target uv pip install --python .venv/bin/python --no-deps --reinstall-package ochre-next --editable <worktree>`; capture with `HARES_REPO=<worktree> uv run --no-sync python tools/capture_de_trace.py capture`; compare against B0. Sequential, cargo `-j 8` (CARGO_BUILD_JOBS=8).
- **Expectation:** identical for EVERY row (bitwise-neutral rows; OR-18 commit 2 moves fleet only, not the DE trace).

## Running table

| # | Row | Worktree | Capture | Compare verdict |
|---|-----|----------|---------|-----------------|
| 1 | 1d60f478 (base sanity) | .workstrees/1d60f478 | [b0-1d60f478.json](file:///tmp/opencode/i08/b0-1d60f478.json) | **identical** (exit 0) |
| 2 | 8b09ac4a (cumulative rows 1-12) | .workstrees/8b09ac4a | [b0-8b09ac4a.json](file:///tmp/opencode/i08/b0-8b09ac4a.json) | **identical** (exit 0) |
| 3 | d7f0e381 | .workstrees/d7f0e381 | [b0-d7f0e381.json](file:///tmp/opencode/i08/b0-d7f0e381.json) | **identical** (exit 0) |
| 4 | 21d90588 | .workstrees/21d90588 | [b0-21d90588.json](file:///tmp/opencode/i08/b0-21d90588.json) | **identical** (exit 0) |
| 5 | 26626710 | .workstrees/26626710 | [b0-26626710.json](file:///tmp/opencode/i08/b0-26626710.json) | **identical** (exit 0) |
| 6 | 1d471677 | .workstrees/1d471677 | [b0-1d471677.json](file:///tmp/opencode/i08/b0-1d471677.json) | **identical** (exit 0) |
| 7 | afa9a071 | .workstrees/afa9a071 | [b0-afa9a071.json](file:///tmp/opencode/i08/b0-afa9a071.json) | **identical** (exit 0) |
| 8 | b8195c83 | .workstrees/b8195c83 | [b0-b8195c83.json](file:///tmp/opencode/i08/b0-b8195c83.json) | **identical** (exit 0) |
| 9 | 7b7a6a7d | .workstrees/7b7a6a7d | [b0-7b7a6a7d.json](file:///tmp/opencode/i08/b0-7b7a6a7d.json) | **identical** (exit 0) |
| 10 | 44da330c | .workstrees/44da330c | [b0-44da330c.json](file:///tmp/opencode/i08/b0-44da330c.json) | **identical** (exit 0) |
| 11 | 69950a98 | .workstrees/69950a98 | [b0-69950a98.json](file:///tmp/opencode/i08/b0-69950a98.json) | **identical** (exit 0) |
| 12 | 1c18543a | .workstrees/1c18543a | [b0-1c18543a.json](file:///tmp/opencode/i08/b0-1c18543a.json) | **identical** (exit 0) |
| 13 | fe804b59 | .workstrees/redteam-fe804b59 (reused) | [b0-fe804b59.json](file:///tmp/opencode/i08/b0-fe804b59.json) | **identical** (exit 0) |
| 14 | 32044ab5 (post-fe804b59) | .workstrees/32044ab5 | [b0-32044ab5.json](file:///tmp/opencode/i08/b0-32044ab5.json) | **identical** (exit 0) |
| 15 | c6889589 (post-fe804b59) | .workstrees/c6889589 | [b0-c6889589.json](file:///tmp/opencode/i08/b0-c6889589.json) | **identical** (exit 0) |
| 16 | 50a5c261 (branch tip) | .workstrees/50a5c261 | [b0-50a5c261.json](file:///tmp/opencode/i08/b0-50a5c261.json) | **identical** (exit 0) |

## Verdict

**ALL 16 ROWS IDENTICAL.** The DE trace is bitwise-neutral across every engine row from 8b09ac4a (cumulative rows 1-12) through the branch tip 50a5c261, including the three commits after fe804b59 (32044ab5, c6889589, 50a5c261) and the base sanity row 1d60f478 (the B0 baseline's own commit). No row moved physics for the consumer; no divergence to bisect. The sweep did not stop early — every row ran to compare.

- Build/capture discipline: sequential rows only, `CARGO_BUILD_JOBS=8`, shared `CARGO_TARGET_DIR=/home/rich/src/HARES/.workstrees/target`; installs ~45-50 s each (warm shared target), captures ~2m40s-4m10s each.
- Worktrees: 10 reused (HEADs verified against their shas), 6 newly created (8b09ac4a, d7f0e381, 1c18543a, 32044ab5, c6889589, 50a5c261).
- Capture files kept at /tmp/opencode/i08/b0-<sha>.json for evidence.

---

# Row 16g — B0 re-check sweep, CONTINUATION (commits after 50a5c261)

- **Resumed:** 2026-10-04 (later same day)
- **Consumer:** /home/rich/src/HARES_CALIBRATION_SYSTEM @ i08-consumer (b71dbb4 = 18a8ec3 + doc commit; HEAD moved from 18a8ec3 — the gate works from its tree)
- **Range:** engine commits on feat/output-results after 50a5c261 through tip d09a3b64. The task listed 10 rows, but `git log 50a5c261..d09a3b64` contains **13** commits — the task list omitted **6646c801** (test-only), **aab05da0** (test-only), and **2a39a8ee** (thermal solver coupling-state fix). Per the "include anything newer" instruction and the sweep's purpose (every commit since the last sweep), all 13 were queued; three extras are flagged below.
- **Method:** identical to the first sweep (worktree, editable install with `CARGO_TARGET_DIR=/home/rich/src/HARES/.workstrees/target CARGO_BUILD_JOBS=8`, capture, compare). One correction to the record: the verdict command is the capture script's own `compare` subcommand, which diffs only the `premises` payload and ignores header metadata (engine/consumer commits, .so sha and path, uv.lock sha — those legitimately differ per row). A raw `cmp` of whole files flags header-only lines (18 diff lines for row 17, all header); that is expected, not drift. This matches what "identical (exit 0)" meant in the first sweep.

## Continuation table

| # | Row | Worktree | Capture | Compare verdict |
|---|-----|----------|---------|-----------------|
| 17 | 4a1f2c19 | .workstrees/4a1f2c19 (new) | [b0-4a1f2c19.json](file:///tmp/opencode/i08/b0-4a1f2c19.json) | **identical** (premises compare exit 0) |
| 18 | 3ff4583a | .workstrees/3ff4583a (new) | [b0-3ff4583a.json](file:///tmp/opencode/i08/b0-3ff4583a.json) | **identical** (premises compare exit 0) |
| 19 | 6646c801 (NOT in task list — test-only) | .workstrees/6646c801 (new) | [b0-6646c801.json](file:///tmp/opencode/i08/b0-6646c801.json) | **identical** (premises compare exit 0) |
| 20 | aab05da0 (NOT in task list — test-only) | .workstrees/aab05da0 (new) | [b0-aab05da0.json](file:///tmp/opencode/i08/b0-aab05da0.json) | **identical** (premises compare exit 0) |
| 21 | f719895b | .workstrees/f719895b (new) | [b0-f719895b.json](file:///tmp/opencode/i08/b0-f719895b.json) | **identical** (premises compare exit 0) |
| 22 | b7ea5d0a | .workstrees/b7ea5d0a (new) | [b0-b7ea5d0a.json](file:///tmp/opencode/i08/b0-b7ea5d0a.json) | **identical** (premises compare exit 0) |
| 23 | b3807202 | .workstrees/b3807202 (new) | [b0-b3807202.json](file:///tmp/opencode/i08/b0-b3807202.json) | **identical** (premises compare exit 0) |
| 24 | 3005ee1c | .workstrees/3005ee1c (new) | [b0-3005ee1c.json](file:///tmp/opencode/i08/b0-3005ee1c.json) | **identical** (premises compare exit 0) |
| 25 | **a4b29269** | .workstrees/a4b29269 (new) | **NO OUTPUT — capture failed** ([error log](file:///tmp/opencode/i08/b0-a4b29269-ERROR.log)) | **CAPTURE FAILURE — sweep STOPPED** |
| — | 59465557, 2a39a8ee, 89eae0ba, d09a3b64 | not created | not captured | **NOT SWEPT** (stopped at a4b29269 per protocol) |

## ⚠️ ROW a4b29269 — CAPTURE FAILS (bitwise-neutrality claim violated)

The consumer **cannot capture the DE trace at all** against engine commit **a4b29269** ("fix: require an explicit stream and an order-free identity for event loads"). One premise in the production set is rejected by the engine before any trace is written:

```
_hares.HaresConfigError: dwelling error: event loads 'Clothes Washer' (identity 'ClothesWasher1')
and 'Clothes Washer' (identity 'ClothesWasher1') map to the same random stream;
give each a unique HPXML SystemIdentifier id or instance name
```

- Deterministic: re-ran the capture once; exits 1 both times with the same error ([b0-a4b29269-ERROR.log](file:///tmp/opencode/i08/b0-a4b29269-ERROR.log)). No `b0-a4b29269.json` is produced. (A secondary `PicklingError: Can't pickle _hares.HaresConfigError` is just the worker process failing to ship the engine exception back; the root cause is the `HaresConfigError` above.)
- Provenance verified: venv engine installed from `/home/rich/src/HARES/.workstrees/a4b29269`, `HARES_REPO` pointed at the same tree.
- Window: introduced by a4b29269 (strict stream/identity requirement) immediately after 3005ee1c captured clean; the next commit 59465557 ("the event-load stream registration runs once per equipment") plausibly repairs it, but per stop-on-divergence protocol that was **not verified**.
- Interpretation: regardless of what 59465557 does, the range 3005ee1c..59465557 is NOT bitwise-neutral for the consumer — for one commit the production set does not run. "Every row claims bitwise neutrality" is false at a4b29269 in the strongest sense (hard failure, not a numeric difference).

## Continuation verdict

**STOPPED AT ROW a4b29269 — CAPTURE FAILURE.** Rows 17-24 (4a1f2c19 → 3005ee1c, including the two unlisted test-only commits 6646c801 and aab05da0) are identical to the B0 baseline. Row a4b29269 breaks the consumer's DE-trace capture outright (duplicate event-load stream identity for a paired 'Clothes Washer'). Rows 59465557, 2a39a8ee, 89eae0ba, d09a3b64 (including the branch tip) were **not** swept. Evidence kept: /tmp/opencode/i08/b0-<sha>.json for rows 17-24, /tmp/opencode/i08/b0-a4b29269-ERROR.log for the failure, diff-4a1f2c19.txt (header-only cmp).

- Build/capture discipline unchanged: sequential, `CARGO_BUILD_JOBS=8`, shared target dir; installs ~35-41 s each (warm shared target).
- Worktrees: all 9 created fresh this session (none reused); HEADs verified via `git -C <worktree> rev-parse` against their shas.

---

# Row 16g — B0 re-check sweep, RESUMPTION 2 (from 59465557 through current tip)

- **Resumed:** 2026-10-04 (same day, third session)
- **Context:** the a4b29269 capture failure that stopped the continuation sweep was the DUPLICATE registration block that 59465557 ("fix: the event-load stream registration runs once per equipment") explicitly removed — the failure is repaired; 59465557's B0 identity was verified by hand beforehand. This session re-verifies 59465557 and sweeps everything through the tip.
- **Range:** `git log 59465557^..HEAD` = 4 commits: 59465557, 2a39a8ee, 89eae0ba, d09a3b64. Tip re-checked immediately before row 29 and still d09a3b64 — nothing new arrived during the sweep.
- **Method:** identical to both prior sessions (fresh `.workstrees/<sha>` worktrees, editable install with `CARGO_TARGET_DIR=/home/rich/src/HARES/.workstrees/target CARGO_BUILD_JOBS=8`, capture, `compare` subcommand against B0 premises). Sequential, one worktree at a time.
- **STOP conditions armed:** a paired-appliance self-collision (same 'Clothes Washer'-vs-'Clothes Washer' stream error) would have been reported as a REAL OD-29 identity-rule collision; any other difference would have stopped the sweep. Neither occurred.

## Resumption table

| # | Row | Worktree | Capture | Compare verdict |
|---|-----|----------|---------|-----------------|
| 26 | **59465557** (repairs a4b29269's duplicate-registration failure) | .workstrees/59465557 (new) | [b0-59465557.json](file:///tmp/opencode/i08/b0-59465557.json) | **identical** (premises compare exit 0) |
| 27 | 2a39a8ee (thermal solver coupling-state restore) | .workstrees/2a39a8ee (new) | [b0-2a39a8ee.json](file:///tmp/opencode/i08/b0-2a39a8ee.json) | **identical** (premises compare exit 0) |
| 28 | 89eae0ba (coupled-state flag, checkpoint shape, free-float oracle) | .workstrees/89eae0ba (new) | [b0-89eae0ba.json](file:///tmp/opencode/i08/b0-89eae0ba.json) | **identical** (premises compare exit 0) |
| 29 | d09a3b64 (branch tip: ventilation recovery/fallback checkpoint, atomic load) | .workstrees/d09a3b64 (new) | [b0-d09a3b64.json](file:///tmp/opencode/i08/b0-d09a3b64.json) | **identical** (premises compare exit 0) |

## Resumption verdict

**ALL 4 ROWS IDENTICAL — sweep complete, tip reached.** 59465557 confirms the a4b29269 failure was an artifact of the removed duplicate registration block (no paired-appliance collision exists; the OD-29 identity rule separation holds for the fixture home). The three checkpoint/thermal-solver commits after it (2a39a8ee, 89eae0ba, d09a3b64) are all bitwise-neutral for the consumer's DE trace. Every commit from the last known-good row (3005ee1c, row 24) through the current tip d09a3b64 now has a verified verdict except a4b29269 itself, which remains the one non-capturing row (hard failure, repaired in the next commit; an inherent gap in that single commit's row, not a physics divergence).

- Build/capture discipline: sequential, one worktree at a time, `CARGO_BUILD_JOBS=8`, shared target dir; installs ~35 s each (warm shared target), captures ~2-3 min each.
- Worktrees: all 4 created fresh this session; HEADs verified via `git -C <worktree> rev-parse --short=8` against their shas.
- Capture files kept at /tmp/opencode/i08/b0-<sha>.json for rows 26-29.
