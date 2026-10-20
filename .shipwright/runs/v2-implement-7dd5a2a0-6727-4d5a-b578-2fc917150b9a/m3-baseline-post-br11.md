# M-3 re-measurement: the baseline after BR-11 — raw record

**Engine state cited:** commit `8b09ac4a` ("fix: a golden manifest with no initialization
duration runs no warm-up") — the BR-11 head, working tree clean (no staged diff; BR-6c
is committed as `75a182a0`). All measurements below were taken on this state.
`frame_golden` + `hares-core` (profiling example) release builds were made current at
run start (`cargo build --release -p frame_golden -j 8` → "Finished ... in 3.17s" —
recompiled because HEAD moved to `8b09ac4a` which touches `tools/frame_golden`;
profiling example "Finished ... in 0.16s").

**Why a re-measurement:** the first M-3 record
(`m3-baseline.md`, engine `872cf289` + BR-6c staged) measured
`consumer_shape_900s` with a two-day warm-up inside the run, because the golden
tool mapped the manifest's `initialization_duration_s = 0` to a warm-up regardless
of value. BR-11 corrected the tool to run no warm-up at 0, matching the consumer's
call. Per the BR-11 ledger row, M-3 is re-measured at `8b09ac4a`; **the baseline
that stands is this record**.

**Method:** identical to the first record — same commands, medians of 5, the same
noise rule (a set is valid when all five runs lie within 5% of their median;
otherwise re-run; a before/after difference under 5% is "no measurable change"),
`uptime` load averages before/after each block, perf single-process.

**Machine note:** WSL2, kernel 6.18.33.2-microsoft. All cargo invocations used
`-j 8`; heavy commands run sequentially; single-process measurements. Python items
via `uv run --no-sync`. Measurement start 10:14 UTC, after waiting for a host load
spike (60+ on 1-min) to settle; 1-min load during the sets was 0.9–1.7, comparable
to the first record's 1.0–1.9 (the 5/15-min figures in this record are elevated by
the spike's tail; the 1-min figures are the comparable ones).

---

## Item 1 — simulate() wall: frame kept in memory / file written / nothing kept

Manifest: `tests/fixtures/golden/consumer_shape_900s.toml` (duration 31,536,000 s =
full year at time_res 900 s ⇒ 35,040 steps; single home `bldg_id 176227`;
`initialization_duration_s = 0` — **no warm-up, post-BR-11**). Each repetition is a
fresh construction. Median of 5 per set.

### Set A — frame kept in memory (default target)

Command: `cargo run --release -p frame_golden -j 8 -- run tests/fixtures/golden/consumer_shape_900s.toml --repeat 5`

- Load before: `10:14:28 ... load average: 1.26, 32.30, 33.13`
- Load after: `10:14:36 ... load average: 1.22, 31.27, 32.78`

Raw (`target/measure/item1a2.txt`):

```
rep 1: construct+simulate 1.553 s (construct 0.044 s, simulate 1.285 s), vm_rss 69188 kB, frame_bytes 15340640
rep 2: construct+simulate 1.539 s (construct 0.034 s, simulate 1.300 s), vm_rss 69284 kB, frame_bytes 15340640
rep 3: construct+simulate 1.514 s (construct 0.035 s, simulate 1.281 s), vm_rss 69508 kB, frame_bytes 15340640
rep 4: construct+simulate 1.531 s (construct 0.035 s, simulate 1.284 s), vm_rss 69560 kB, frame_bytes 15340640
rep 5: construct+simulate 1.533 s (construct 0.035 s, simulate 1.298 s), vm_rss 69668 kB, frame_bytes 15340640
```

Median wall: **1.533 s** (runs 1.514–1.553; spread 2.54% of median — valid, all runs
within 5% of the median).

### Set B — file written (`--output-dir target/measure`)

Command: `cargo run --release -p frame_golden -j 8 -- run tests/fixtures/golden/consumer_shape_900s.toml --repeat 5 --output-dir target/measure`

- Load before: `10:14:41 ... load average: 1.20, 30.76, 32.61`
- Load after: `10:14:49 ... load average: 1.27, 30.28, 32.45`

Raw (`target/measure/item1b2.txt`):

```
rep 1: construct+simulate 1.604 s (construct 0.044 s, simulate 1.340 s), vm_rss 68836 kB, frame_bytes 15340640
wrote target/measure/frame.parquet
rep 2: construct+simulate 1.552 s (construct 0.035 s, simulate 1.323 s), vm_rss 69800 kB, frame_bytes 15340640
wrote target/measure/frame.parquet
rep 3: construct+simulate 1.559 s (construct 0.034 s, simulate 1.325 s), vm_rss 69680 kB, frame_bytes 15340640
wrote target/measure/frame.parquet
rep 4: construct+simulate 1.559 s (construct 0.034 s, simulate 1.323 s), vm_rss 68908 kB, frame_bytes 15340640
wrote target/measure/frame.parquet
rep 5: construct+simulate 1.574 s (construct 0.034 s, simulate 1.334 s), vm_rss 70224 kB, frame_bytes 15340640
wrote target/measure/frame.parquet
```

Median wall: **1.559 s** (runs 1.552–1.604; spread 3.34% of median — valid).

### Set C — nothing kept (`--discard-output`)

Command: `cargo run --release -p frame_golden -j 8 -- run tests/fixtures/golden/consumer_shape_900s.toml --repeat 5 --discard-output`

Two attempts were discarded under the noise rule (spread over 5% of median) and the
set re-run, per the ledger; the third set is the recorded one.

Attempt 1 (`target/measure/item1c2.txt`, discarded — spread 5.61%): before
`10:14:52 ... 1.24, 29.80, 32.28`, after `10:14:59 ... 1.21, 28.85, 31.94`;
runs 1.427/1.484/1.432/1.415/1.404, median 1.427.

Attempt 2 (`target/measure/item1c2_rerun.txt`, discarded — spread 5.19%): before
`10:15:13 ... 1.10, 27.92, 31.60`, after `10:15:17 ... 1.16, 27.05, 31.28`;
runs 1.465/1.484/1.500/1.460/1.424, median 1.465.

Attempt 3 — recorded (`target/measure/item1c2_rerun2.txt`):

- Load before: `10:15:25 ... load average: 0.98, 26.15, 30.95`
- Load after: `10:15:32 ... load average: 0.98, 25.74, 30.79`

```
rep 1: construct+simulate 1.426 s (construct 0.043 s, simulate 1.163 s), vm_rss 39104 kB, frame_bytes 0
rep 2: construct+simulate 1.411 s (construct 0.038 s, simulate 1.167 s), vm_rss 41772 kB, frame_bytes 0
rep 3: construct+simulate 1.458 s (construct 0.040 s, simulate 1.208 s), vm_rss 41500 kB, frame_bytes 0
rep 4: construct+simulate 1.454 s (construct 0.039 s, simulate 1.202 s), vm_rss 41500 kB, frame_bytes 0
rep 5: construct+simulate 1.433 s (construct 0.039 s, simulate 1.182 s), vm_rss 41940 kB, frame_bytes 0
```

Median wall: **1.433 s** (runs 1.411–1.458; spread 3.28% of median — valid).

### Item 1 summary

| target            | median wall (5 reps) | spread vs median | valid |
|-------------------|----------------------|------------------|-------|
| in memory         | 1.533 s              | 2.54%            | yes   |
| --output-dir      | 1.559 s              | 3.34%            | yes   |
| --discard-output  | 1.433 s              | 3.28% (set 3)    | yes   |

---

## Item 2 — function shares of one run

### perf attempt (failed — recorded verbatim)

Command: `perf record -e cpu-clock:u --call-graph dwarf -o target/measure/perf.data -- target/release/frame_golden run tests/fixtures/golden/consumer_shape_900s.toml --repeat 1`

- Load before: `10:15:38 ... load average: 0.90, 25.31, 30.62`
- Load after: `10:15:38 ... load average: 0.90, 25.31, 30.62`
- Exit code: 2

Verbatim output (`target/measure/perf_attempt2.txt`):

```
WARNING: perf not found for kernel 6.18.33.2-microsoft

  You may need to install the following packages for this specific kernel:
    linux-tools-6.18.33.2-microsoft-standard-WSL2
    linux-cloud-tools-6.18.33.2-microsoft-standard-WSL2

  You may also want to install one of the following packages to keep up to date:
    linux-tools-standard-WSL2
    linux-cloud-tools-standard-WSL2
```

`perf` is genuinely unavailable on this WSL2 kernel (no matching linux-tools binary),
as in the first record. Per the M-3 instructions, the profiling example's phase
breakdown is substituted as the function-share evidence.

### Substitute: profiling example phase breakdown

Command: `cargo run --release -p hares-core --features profiling --example profile_sim -j 8 -- 45 3`

- Load before: `10:15:43 ... load average: 1.39, 25.00, 30.49`
- Load after: `10:15:45 ... load average: 1.39, 25.00, 30.49`

Raw (`target/measure/item2_profile2.txt`):

```
wall 929.19ms for 3 x 45-day runs (plus one warm-up); per-phase means per run:
  envelope_solve      28.1 ms/run
  schedule_load       14.6 ms/run
  hvac                10.9 ms/run
  io                   0.0 ms/run
  other              217.6 ms/run
  water_heater         0.0 ms/run
```

**Substitution statement:** perf could not be run (verbatim failure above); this phase
breakdown stand-in is therefore the function-share evidence for this record, as
directed. (The example's "plus one warm-up" is the example's own internal warm-up
run — unrelated to the manifest warm-up BR-11 removed.)

### Symbol sites (unchanged from the first record; re-verified by grep at `8b09ac4a`)

No engine-code file under `crates/hares-core` or `crates/hares-envelope` changed
between `872cf289` and `8b09ac4a` (BR-11 touched only `tools/frame_golden` and the
fixtures; `75a182a0` only `pyproject.toml`/`uv.lock`), so every site below is the
same line as the first record, re-confirmed at this commit:

| symbol | current site |
|---|---|
| per-step telemetry map clone | `crates/hares-core/src/dwelling/mod.rs:6624` (`telemetry.clone()` in the `equipment_telemetry` insert path of `run_timestep`, fn at mod.rs:5382); second site mod.rs:4981, same pattern in `snapshot_equipment_state` (fn at mod.rs:4945; checkpoint-restore path, not per-step) |
| `record_step` | `crates/hares-core/src/dwelling/mod.rs:6794` |
| `update_in_place` | `crates/hares-core/src/environment.rs:524` (`Environment::update_in_place`) |
| `prepare_inputs_inner` | `crates/hares-envelope/src/thermal_solver/stepping.rs:668` |
| `solve_ideal_capacity_for_target` | `crates/hares-envelope/src/thermal_solver/stepping.rs:366` |
| envelope solve entry | `ThermalSolver::integrate` — `crates/hares-envelope/src/thermal_solver/mod.rs:1608`, called from `run_timestep` at mod.rs:6202's caller chain, timed into `profiling.envelope_solve` (mod.rs:6362) |
| `Dwelling::check_invariants` | `crates/hares-core/src/dwelling/mod.rs:7176` |

Symbol-mapping method note: as in the first record, with perf unavailable no mangled
perf symbols had to be resolved; the mapping was re-verified by grepping the sources
at this commit. Per-function self-time shares for `record_step`, `update_in_place`,
`prepare_inputs_inner`, `solve_ideal_capacity_for_target`, `check_invariants` and the
telemetry clone sites are **not measurable** in this environment; the example's phases
are the available breakdown.

Phase shares (of the 309.73 ms/run mean wall = 929.19/3):

| phase | ms/run | share of wall |
|---|---|---|
| envelope_solve | 28.1 | 9.1% |
| schedule_load | 14.6 | 4.7% |
| hvac | 10.9 | 3.5% |
| io | 0.0 | 0.0% |
| water_heater | 0.0 | 0.0% |
| other (everything else, incl. record_step, update_in_place, prepare_inputs_inner, telemetry clones, check_invariants) | 217.6 | 70.3% |
| unattributed (wall minus phase sum) | 38.5 | 12.4% |

The envelope solve (the only named hot region the example isolates) is ~9% of wall;
the per-step bookkeeping paths the ledger names sit inside `other` — same shape as
the first record.

---

## Item 3 — retained frame bytes (full-year in-memory run)

From item 1 Set A (in-memory, full-year run, all 5 reps identical):
**frame_bytes = 15,340,640** (≈ 14.63 MiB). Set B (`--output-dir`) reports the
identical 15,340,640 bytes; Set C (`--discard-output`) reports frame_bytes 0 by
design. Identical to the first record: BR-11 changed cell **values** (cold start),
not the frame's shape or column count.

---

## Item 4 — Python export time (`results()` median of 5)

Environment: `uv run --no-sync python` (committed lock, incl. `75a182a0`'s Python
dependency upgrades — a known difference from the first record's staged BR-6c lock).
Script: `target/measure/item4_export.py` (unchanged file from the first record). It
replicates the manifest's defaults handling exactly as
`tools/frame_golden/src/defaults.rs::prepare` does — a temporary symlink tree of
`defaults/` with `envelope/Envelope Materials.csv` replaced by
`tests/fixtures/golden/consumer_shape_envelope_materials.csv` — then builds
`Dwelling.from_hpxml(...)` with consumer_shape_900s's inputs/settings plus
`output_path="target/measure/py.parquet"`, `output_to_parquet=True`,
`initialization_duration=0`, runs `initialize()` and `simulate()`, then times
`results()` five times.

Command: `uv run --no-sync python target/measure/item4_export.py`

- Load before: `10:16:01 ... load average: 1.18, 23.43, 29.86`
- Load after: `10:16:02 ... load average: 1.17, 23.06, 29.70`

Raw (`target/measure/item4b.txt`):

```
results() 1: 0.0162 s  (rows: 35040)
results() 2: 0.0086 s  (rows: 35040)
results() 3: 0.0096 s  (rows: 35040)
results() 4: 0.0086 s  (rows: 35040)
results() 5: 0.0088 s  (rows: 35040)
median results(): 0.0088 s
```

**Median results() export time: 0.0088 s** (first call 0.0162 s — cold; as in the
first record, the ledger asks only for the median of the five `perf_counter()`
timings, reported as-is).

Stability check (because the recorded median sits +8.6% above the first record's
0.0081 s, over the 5% line): an immediate re-run
(`target/measure/item4b_stability.txt`) gave 0.0149/0.0079/0.0080/0.0077/0.0076,
**median 0.0079 s** — bracketing the first record's 0.0081 s. The export time is
jitter on a sub-10 ms operation, not a stable shift; recorded as **no stable
measurable change**.

No adaptation of save_state/load_state APIs was needed anywhere (unchanged from the
first record).

---

## Item 5 — memory growth (VmRSS readings)

### 5a — repeated construct-and-simulate

Command: `cargo run --release -p frame_golden -j 8 -- run tests/fixtures/golden/consumer_shape_900s.toml --repeat 500 --duration-s 86400 --discard-output`

- Load before: `10:16:06 ... load average: 1.17, 23.06, 29.70`
- Load after: `10:17:59 ... load average: 1.71, 15.76, 26.17`

Readings (`target/measure/item5a2.txt`, full log retained):

```
rep 1:   ... vm_rss 37772 kB, frame_bytes 0
rep 100: ... vm_rss 38776 kB, frame_bytes 0
rep 500: ... vm_rss 38776 kB, frame_bytes 0
```

Growth rep 1 → 500: **+1,004 kB** (37772 → 38776; +2.7% — flat; same allocator
high-water jitter as the first record's +724 kB).

### 5b — long horizon (10³ vs 10⁶ steps), sequential runs

Commands:
1. `cargo run --release -p frame_golden -j 8 -- run tests/fixtures/golden/consumer_shape_900s.toml --repeat 1 --duration-s 900000 --discard-output` (10³ steps)
2. `cargo run --release -p frame_golden -j 8 -- run tests/fixtures/golden/consumer_shape_900s.toml --repeat 1 --duration-s 900000000 --discard-output` (10⁶ steps)

- Load before: `10:18:03 ... load average: 1.57, 15.50, 26.03`
- Load after: `10:18:35 ... load average: 1.60, 13.96, 25.13`

Readings:

```
10^3 steps: rep 1: construct+simulate 0.296 s ..., vm_rss 37804 kB, frame_bytes 0
10^6 steps: rep 1: construct+simulate 34.977 s ..., vm_rss 149132 kB, frame_bytes 0
```

Growth 10³ → 10⁶ steps: **+111,328 kB** (37804 → 149132 kB) over 999,000 extra
steps, i.e. ≈ 0.11 kB/step — ≈ 114 bytes per step of steady growth (≈ 111 MB over
10⁶ steps), the retained telemetry. Same slope as the first record (+111,352 kB).

### 5c — stepped episodes (Python loop)

Script: `target/measure/item5c_episodes.py` (unchanged file from the first record) —
builds the consumer_shape_900s dwelling with `duration_s=86400`,
`write_output=False` (defaults tree as in item 4), calls `initialize()`, keeps
`state = save_state()`, then 100 × (`load_state(state)`; 96 × `step()`), reading
`VmRSS` from `/proc/self/status` after episodes 1 and 100.

Command: `uv run --no-sync python target/measure/item5c_episodes.py`

- Load before: `10:18:41 ... load average: 1.47, 13.73, 24.99`
- Load after: `10:18:41 ... load average: 1.47, 13.73, 24.99`

Raw (`target/measure/item5c2.txt`):

```
episode 1: VmRSS 80612 kB
episode 100: VmRSS 80620 kB
done
```

Growth episode 1 → 100: **+8 kB** (80612 → 80620; flat — identical to the first
record).

---

## Verification — repeat of item 1's first command set

The first verification attempt (`target/measure/item1a_verify2.txt`, median 1.522 s)
was discarded and re-run under the noise rule (spread 6.77% of median); the second
is the recorded one, per the ledger.

Attempt 1 (discarded): before `10:18:41 ... 1.35, 13.50, 24.86`, after
`10:18:49 ... 1.32, 13.29, 24.73`; runs 1.592/1.513/1.489/1.522/1.522.

Attempt 2 — recorded (`target/measure/item1a_verify2b.txt`):

Command: `cargo run --release -p frame_golden -j 8 -- run tests/fixtures/golden/consumer_shape_900s.toml --repeat 5`

- Load before: `10:18:55 ... load average: 1.12, 12.85, 24.46`
- Load after: `10:19:03 ... load average: 1.11, 12.66, 24.34`

```
rep 1: construct+simulate 1.540 s (construct 0.044 s, simulate 1.276 s), vm_rss 69376 kB, frame_bytes 15340640
rep 2: construct+simulate 1.506 s (construct 0.035 s, simulate 1.276 s), vm_rss 69544 kB, frame_bytes 15340640
rep 3: construct+simulate 1.561 s (construct 0.036 s, simulate 1.334 s), vm_rss 70040 kB, frame_bytes 15340640
rep 4: construct+simulate 1.537 s (construct 0.035 s, simulate 1.314 s), vm_rss 70264 kB, frame_bytes 15340640
rep 5: construct+simulate 1.520 s (construct 0.034 s, simulate 1.288 s), vm_rss 70508 kB, frame_bytes 15340640
```

Verification median: **1.537 s** (runs 1.506–1.561; spread 3.58% of median — valid).

First set's band (±5% of 1.533 s): 1.4564 s – 1.6097 s.
**Verification median 1.537 s lies within the band. PASSES.**
(First median 1.533 s vs verification median 1.537 s: +0.3% — under 5%, recorded as
"no measurable change" per the noise rule.)

---

## Before/after — first M-3 record (`m3-baseline.md`, `872cf289`, warm-up bug present) vs this record (`8b09ac4a`, no warm-up)

**Wall per mode (medians of 5):**

| target | first M-3 | post-BR-11 | delta | verdict |
|---|---|---|---|---|
| in memory | 1.530 s | 1.533 s | +0.003 s (+0.20%) | no measurable change |
| --output-dir | 1.509 s | 1.559 s | +0.050 s (+3.31%) | no measurable change |
| --discard-output | 1.409 s | 1.433 s | +0.024 s (+1.70%) | no measurable change |

**Phase breakdown (profiling-example substitute, single sample each time):**

| phase | first M-3 | post-BR-11 | delta |
|---|---|---|---|
| envelope_solve | 26.0 ms (8.8%) | 28.1 ms (9.1%) | +2.1 ms, +0.3 pp |
| schedule_load | 13.1 ms (4.5%) | 14.6 ms (4.7%) | +1.5 ms, +0.2 pp |
| hvac | 9.8 ms (3.3%) | 10.9 ms (3.5%) | +1.1 ms, +0.2 pp |
| other | 209.7 ms (71.3%) | 217.6 ms (70.3%) | +7.9 ms, −1.0 pp |
| unattributed | 35.2 ms (12.0%) | 38.5 ms (12.4%) | +3.3 ms, +0.4 pp |
| mean wall/run | 293.93 ms | 309.73 ms | +15.8 ms (+5.4%) |

The per-phase shares moved ≤1.0 pp; the example's mean wall (+5.4%) is a
single-sample quantity (no median-of-5 behind it) and is within the example's
run-to-run jitter. Profile shape unchanged: envelope solve ~9% of wall, everything
the ledger names inside `other` (~70%).

**Retained frame bytes:** 15,340,640 → 15,340,640 — delta **0** (BR-11 changes
values, not shape).

**Python `results()` export:** 0.0081 s → 0.0088 s as first-recorded (+8.6%); the
stability re-run in this record (0.0079 s, −2.5% vs the first record) brackets the
old value — **no stable measurable change** (caveat: the Python lock differs between
the records by `75a182a0`'s dependency upgrades).

**VmRSS growth:**

| experiment | first M-3 | post-BR-11 | delta |
|---|---|---|---|
| 5a: 500× construct+simulate (1 day) | +724 kB | +1,004 kB | +280 kB — both flat (<3% of RSS) |
| 5b: 10³ → 10⁶ steps | +111,352 kB | +111,328 kB | −24 kB — same ≈0.11 kB/step slope |
| 5c: 100 stepped episodes | +8 kB | +8 kB | 0 |

**Reading:** removing the two-day warm-up (≈1,200 of 35,040+ warm-up-equivalent
steps, inside `construct`) does not measurably move any M-3 quantity: all three wall
modes, the profile shape, the frame size, the export time and all three memory-growth
signatures are unchanged within the 5% noise rule. The post-BR-11 engine's cost
structure is the first record's; only the numbers the frame **contains** changed
(BR-11's re-captured golden).

---

## Files retained (all untracked, under target/measure/; first record's files untouched)

- `item1a2.txt`, `item1b2.txt`, `item1c2.txt`, `item1c2_rerun.txt`, `item1c2_rerun2.txt` — item 1 raw outputs (set C attempts 1–3 in order)
- `perf_attempt2.txt` — verbatim perf failure
- `item2_profile2.txt` — profiling example substitute output
- `item4b.txt`, `item4b_stability.txt`, `item4_export.py` — item 4 raw output, stability re-run, script
- `item5a2.txt`, `item5b_1k2.txt`, `item5b_1M2.txt`, `item5c2.txt`, `item5c_episodes.py` — item 5 raw outputs + script
- `item1a_verify2.txt`, `item1a_verify2b.txt` — verification attempts 1 (discarded) and 2 (recorded)
- `frame.parquet`, `py.parquet`, `py_defaults/` — artifacts rewritten by the measured runs

**Commit:** none (measurement record only; no tracked file was modified — the
working tree is clean at `8b09ac4a`).
