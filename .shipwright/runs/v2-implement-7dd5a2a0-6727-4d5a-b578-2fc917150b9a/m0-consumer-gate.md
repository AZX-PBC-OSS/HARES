# M0 Consumer Gate — Measurement Run

- Consumer repo: /home/rich/src/HARES_CALIBRATION_SYSTEM (branch i08-consumer, HEAD c71c542)
- Engine under test: /home/rich/src/HARES/.workstrees/1d60f478 (engine commit 1d60f478)
- Date: 2026-09-30

## Step 1: uv sync

```
Resolved 21 packages in 3ms
Uninstalled 1 package in 1ms
 - ochre-next==0.1.0 (from file:///home/rich/src/HARES_CALIBRATION_SYSTEM/HARES)
```

## Step 2: Editable install of ochre-next (release build)

```
Resolved 1 package in 2ms
   Building ochre-next @ file:///home/rich/src/HARES/.workstrees/1d60f478
      Built ochre-next @ file:///home/rich/src/HARES/.workstrees/1d60f478
Prepared 1 package in 1m 28s
Installed 1 package in 2ms
 + ochre-next==0.1.0 (from file:///home/rich/src/HARES/.workstrees/1d60f478)
```

## Step 3: Environment check (dry-run)

```
Would use project environment at: .venv
Resolved 21 packages in 1ms
Would uninstall 1 package
 - ochre-next==0.1.0 (from file:///home/rich/src/HARES/.workstrees/1d60f478)
```

Exactly one "Would uninstall" line, naming ochre-next. ✓ Expected result.

## Step 4: Consumer gate

### run_all

Command: `HARES_REPO=/home/rich/src/HARES/.workstrees/1d60f478 uv run --no-sync python -m calibration.tests.run_all > /tmp/gate.txt` — exit code 0.

**Summary line:** `329 passed, 0 failed, 7 skipped, 336 total`
(plus the note: `NOTE: skipped cases assert NOTHING. A skip is not a pass.`)

**FAIL lines:** none — zero failing cases.

**Skip lines (7):**

| Case | Reason |
|---|---|
| `a_furnace_home_fits_without_proposing_impossible_equipment` | gas-furnace cell definition not available |
| `GridLAB_D_reproduces_the_sweep_when_asked_to_model_the_same_feeder` | gridlabd not installed at /home/rich/software/gridlabd-5.3.0/bin/gridlabd; see docs/GridLABD_Install.md |
| `the_HELICS_federation_and_batch_GridLAB_D_agree_on_the_same_feeder` | gridlabd not installed at /home/rich/software/gridlabd-5.3.0/bin/gridlabd; see docs/GridLABD_Install.md |
| `coupled_mode_puts_the_phases_on_one_conductor_so_they_can_actually_interact` | gridlabd not installed at /home/rich/software/gridlabd-5.3.0/bin/gridlabd; see docs/GridLABD_Install.md |
| `core_loss_is_a_standing_draw_that_does_not_depend_on_the_load` | gridlabd not installed at /home/rich/software/gridlabd-5.3.0/bin/gridlabd; see docs/GridLABD_Install.md |
| `a_voltage_regulator_holds_the_feeder_up_instead_of_letting_it_sag` | gridlabd not installed at /home/rich/software/gridlabd-5.3.0/bin/gridlabd; see docs/GridLABD_Install.md |
| `the_solver_reproduces_a_published_test_feeder` | IEEE 13 model not found at /home/rich/software/gridlabd-5.3.0/share/IEEE_13_Node_Test_Feeder.glm; see docs/GridLABD_Install.md |

### paths.summary()

The prescribed invocation `from calibration.paths import paths; print(paths.summary())` **fails**:

```
ImportError: cannot import name 'paths' from 'calibration.paths' (/home/rich/src/HARES_CALIBRATION_SYSTEM/calibration/paths.py)
```

`calibration/paths.py` has no `paths` object; `summary()` is a module-level function (paths.py:188). Recording the output via the equivalent module-level call `from calibration.paths import summary; print(summary())`:

```
{
  "hares_repo": "/home/rich/src/HARES/.workstrees/1d60f478",
  "engine_so": "/home/rich/src/HARES/.workstrees/1d60f478/python/ochre_next/_hares.cpython-313-x86_64-linux-gnu.so",
  "engine_sha256": "28be65c0835f3622",
  "resstock_cache": "/home/rich/.cache/ochre_next/resstock/2025.1",
  "n_cached_buildings": 59,
  "meta_parquet": "/home/rich/.cache/ochre_next/resstock_meta/upgrade0_2025_1.parquet",
  "n_tmy3_epw": 3144
}
```

### ruff check

Command: `HARES_REPO=/home/rich/src/HARES/.workstrees/1d60f478 uv run --no-sync ruff check`

```
All checks passed!
```

**Exit code: 0**

### build_bundle.py

Command: `HARES_REPO=/home/rich/src/HARES/.workstrees/1d60f478 uv run --no-sync python build_bundle.py`

**Exit code: 1** (NOT GREEN)

VERIFY section:
- MISSING: none — `✓ every registry file present`
- UNTRACKED (3) — present but not registered (add to SOURCE/SOURCE_DIRS or delete):
  - `docs/Red_Team_Record.md`
  - `tools/check_losses.py`
  - `tools/race_gridlabd.py`

Test-count line: `test cases: 329 passed, 0 failed, 7 skipped`
Also reported: `HARES contract drift check: PASS`; wrote `/home/rich/src/HARES_CALIBRATION_SYSTEM_BUNDLE_2026-09-30.zip` (6.1 MB).

The script rewrote the tracked files `INDEX.txt`, `MANIFEST.txt`, `SETUP.txt`. Not committed (per instructions). Diff:

```
$ git -C /home/rich/src/HARES_CALIBRATION_SYSTEM diff --stat
 INDEX.txt    |  35 +++++++--
 MANIFEST.txt | 237 +++++++++++++++++++++++++++++++++++++++++++++++++++++++----
 SETUP.txt    |  45 ++++++++----
 3 files changed, 280 insertions(+), 37 deletions(-)
```

## Step 5: git status

```
$ git -C /home/rich/src/HARES_CALIBRATION_SYSTEM status --porcelain
 M INDEX.txt
 M MANIFEST.txt
 M SETUP.txt
```

**Tree NOT fully clean:** the only dirty entries are the three files `build_bundle.py` itself rewrites (INDEX.txt, MANIFEST.txt, SETUP.txt). No other tracked files were modified; scratch/ and .venv are ignored as expected. Nothing was committed.

## Constraints honoured

- No commits made; no tracked files modified by hand (the three modified files are build_bundle.py's own rewrite, reported above).
- /home/rich/src/HARES not touched except reading the pinned worktree for the editable install and writing this results file under .shipwright/runs/.
