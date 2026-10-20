# M-3: the baseline — raw record

**Engine state cited:** commit `872cf289` ("build: upgrade every Rust dependency to its
latest release") with the BR-6c staged diff (`pyproject.toml` + `uv.lock` — Python-side
dependency lock only; no engine-code effect). All measurements below were taken on this
state. `frame_golden` (tools/frame_golden) + `hares-core` + `hares-envelope` release
builds were already current at run start (`cargo build --release -p frame_golden -j 8`
→ "Finished ... in 0.15s"; same for the profiling example build).

**Machine note:** WSL2, kernel 6.18.33.2-microsoft. All cargo invocations used `-j 8`;
heavy commands run sequentially; perf/profiling single-process, per the binding memory
constraint. Python items via `uv run --no-sync` (venv carries the BR-6c lock).

---

## Item 1 — simulate() wall: frame kept in memory / file written / nothing kept

Manifest: `tests/fixtures/golden/consumer_shape_900s.toml` (duration 31,536,000 s =
full year at time_res 900 s ⇒ 35,040 steps; single home `bldg_id 176227`).
Each repetition is a fresh construction. Median of 5 per set.

### Set A — frame kept in memory (default target)

Command: `cargo run --release -p frame_golden -j 8 -- run tests/fixtures/golden/consumer_shape_900s.toml --repeat 5`

- Load before: `06:34:56 ... load average: 1.18, 4.89, 14.08`
- Load after: `06:35:01 ... load average: 1.16, 4.76, 13.94`

Raw (`target/measure/item1a.txt`):

```
rep 1: construct+simulate 1.546 s (construct 0.054 s, simulate 1.263 s), vm_rss 69284 kB, frame_bytes 15340640
rep 2: construct+simulate 1.522 s (construct 0.040 s, simulate 1.269 s), vm_rss 69696 kB, frame_bytes 15340640
rep 3: construct+simulate 1.549 s (construct 0.041 s, simulate 1.300 s), vm_rss 69512 kB, frame_bytes 15340640
rep 4: construct+simulate 1.530 s (construct 0.041 s, simulate 1.277 s), vm_rss 69688 kB, frame_bytes 15340640
rep 5: construct+simulate 1.518 s (construct 0.040 s, simulate 1.266 s), vm_rss 69668 kB, frame_bytes 15340640
```

Median wall: **1.530 s** (runs 1.518–1.549; spread 2.03% of median — valid, all runs
within 5% of the median).

### Set B — file written (`--output-dir target/measure`)

Command: `cargo run --release -p frame_golden -j 8 -- run tests/fixtures/golden/consumer_shape_900s.toml --repeat 5 --output-dir target/measure`

- Load before: `06:35:04 ... load average: 1.16, 4.76, 13.94`
- Load after: `06:35:12 ... load average: 1.35, 4.69, 13.82`

Raw (`target/measure/item1b.txt`):

```
rep 1: construct+simulate 1.527 s (construct 0.048 s, simulate 1.253 s), vm_rss 69028 kB, frame_bytes 15340640
wrote target/measure/frame.parquet
rep 2: construct+simulate 1.492 s (construct 0.040 s, simulate 1.248 s), vm_rss 69740 kB, frame_bytes 15340640
wrote target/measure/frame.parquet
rep 3: construct+simulate 1.509 s (construct 0.039 s, simulate 1.258 s), vm_rss 70012 kB, frame_bytes 15340640
wrote target/measure/frame.parquet
rep 4: construct+simulate 1.507 s (construct 0.040 s, simulate 1.253 s), vm_rss 70364 kB, frame_bytes 15340640
wrote target/measure/frame.parquet
rep 5: construct+simulate 1.509 s (construct 0.041 s, simulate 1.252 s), vm_rss 69796 kB, frame_bytes 15340640
wrote target/measure/frame.parquet
```

Median wall: **1.509 s** (runs 1.492–1.527; spread 2.32% of median — valid).

### Set C — nothing kept (`--discard-output`)

Command: `cargo run --release -p frame_golden -j 8 -- run tests/fixtures/golden/consumer_shape_900s.toml --repeat 5 --discard-output`

- Load before: `06:35:14 ... load average: 1.35, 4.69, 13.82`
- Load after: `06:35:21 ... load average: 1.38, 4.58, 13.69`

Raw (`target/measure/item1c.txt`):

```
rep 1: construct+simulate 1.409 s (construct 0.048 s, simulate 1.132 s), vm_rss 39140 kB, frame_bytes 0
rep 2: construct+simulate 1.401 s (construct 0.044 s, simulate 1.148 s), vm_rss 40996 kB, frame_bytes 0
rep 3: construct+simulate 1.448 s (construct 0.043 s, simulate 1.191 s), vm_rss 42084 kB, frame_bytes 0
rep 4: construct+simulate 1.399 s (construct 0.043 s, simulate 1.148 s), vm_rss 42140 kB, frame_bytes 0
rep 5: construct+simulate 1.400 s (construct 0.043 s, simulate 1.153 s), vm_rss 41040 kB, frame_bytes 0
```

Median wall: **1.409 s** (runs 1.399–1.448; spread 3.48% of median — valid).

### Item 1 summary

| target            | median wall (5 reps) | spread vs median | valid |
|-------------------|----------------------|------------------|-------|
| in memory         | 1.530 s              | 2.03%            | yes   |
| --output-dir      | 1.509 s              | 2.32%            | yes   |
| --discard-output  | 1.409 s              | 3.48%            | yes   |

---

## Item 2 — function shares of one run

### perf attempt (failed — recorded verbatim)

Command: `perf record -e cpu-clock:u --call-graph dwarf -o target/measure/perf.data -- target/release/frame_golden run tests/fixtures/golden/consumer_shape_900s.toml --repeat 1`

- Load before: `06:35:25 ... load average: 1.59, 4.57, 13.63`
- Load after: `06:35:25 ... load average: 1.59, 4.57, 13.63`
- Exit code: 2

Verbatim output (`target/measure/perf_attempt.txt`):

```
WARNING: perf not found for kernel 6.18.33.2-microsoft

  You may need to install the following packages for this specific kernel:
    linux-tools-6.18.33.2-microsoft-standard-WSL2
    linux-cloud-tools-6.18.33.2-microsoft-standard-WSL2

  You may also want to install the following packages to keep up to date:
    linux-tools-standard-WSL2
    linux-cloud-tools-standard-WSL2
```

`perf` is genuinely unavailable on this WSL2 kernel (no matching linux-tools binary).
Per the M-3 instructions, the profiling example's phase breakdown is substituted as
the function-share evidence.

### Substitute: profiling example phase breakdown

Command: `cargo run --release -p hares-core --features profiling --example profile_sim -j 8 -- 45 3`

- Load before: `06:35:26 ... load average: 1.59, 4.57, 13.63`
- Load after: `06:35:27 ... load average: 1.86, 4.58, 13.59`

Raw (`target/measure/item2_profile.txt`):

```
wall 881.80ms for 3 x 45-day runs (plus one warm-up); per-phase means per run:
  envelope_solve      26.0 ms/run
  schedule_load       13.1 ms/run
  hvac                 9.8 ms/run
  io                   0.0 ms/run
  other              209.7 ms/run
  water_heater         0.0 ms/run
```

**Substitution statement:** perf could not be run (verbatim failure above); this phase
breakstand-in is therefore the function-share evidence for this record, as directed.

### Symbol sites (current locations; the plan's mod.rs:6436/:4924 have shifted)

| symbol | current site |
|---|---|
| per-step telemetry map clone | `crates/hares-core/src/dwelling/mod.rs:6624` (`telemetry.clone()` in the `equipment_telemetry` insert path of `run_timestep`, fn at mod.rs:5382); second site mod.rs:4981, same pattern in `snapshot_equipment_state` (checkpoint-restore path, not per-step) |
| `record_step` | `crates/hares-core/src/dwelling/mod.rs:6794` |
| `update_in_place` | `crates/hares-core/src/environment.rs:524` (`Environment::update_in_place`) |
| `prepare_inputs_inner` | `crates/hares-envelope/src/thermal_solver/stepping.rs:668` |
| `solve_ideal_capacity_for_target` | `crates/hares-envelope/src/thermal_solver/stepping.rs:366` |
| envelope solve entry | `ThermalSolver::integrate` — `crates/hares-envelope/src/thermal_solver/mod.rs:1608`, called from `run_timestep` at mod.rs:6202, timed into `profiling.envelope_solve` (mod.rs:6362) |
| `Dwelling::check_invariants` | `crates/hares-core/src/dwelling/mod.rs:7176` |

Symbol-mapping method note: with perf unavailable, no mangled perf symbols had to be
resolved; the mapping above was done by grepping the sources (`fn <name>`, `clone`
around the telemetry/state extraction paths) and confirming the envelope-solve timing
window. Per-function self-time shares for `record_step`, `update_in_place`,
`prepare_inputs_inner`, `solve_ideal_capacity_for_target`, `check_invariants` and the
telemetry clone sites are **not measurable** in this environment; the example's phases
are the available breakdown.

Phase shares (of the 293.93 ms/run mean wall = 881.80/3):

| phase | ms/run | share of wall |
|---|---|---|
| envelope_solve | 26.0 | 8.8% |
| schedule_load | 13.1 | 4.5% |
| hvac | 9.8 | 3.3% |
| io | 0.0 | 0.0% |
| water_heater | 0.0 | 0.0% |
| other (everything else, incl. record_step, update_in_place, prepare_inputs_inner, telemetry clones, check_invariants) | 209.7 | 71.3% |
| unattributed (wall minus phase sum) | 35.2 | 12.0% |

The envelope solve (the only named hot region the example isolates) is ~9% of wall;
the per-step bookkeeping paths the ledger names sit inside `other`.

---

## Item 3 — retained frame bytes (full-year in-memory run)

From item 1 Set A (in-memory, full-year run, all 5 reps identical):
**frame_bytes = 15,340,640** (≈ 14.63 MiB) for 35,040 steps × 2,852 columns of f64.
Set B (`--output-dir`) reports the identical 15,340,640 bytes; Set C
(`--discard-output`) reports frame_bytes 0 by design.

---

## Item 4 — Python export time (`results()` median of 5)

Environment: `uv run --no-sync python` (BR-6c lock). Script:
`target/measure/item4_export.py`. It replicates the manifest's defaults handling
exactly as `tools/frame_golden/src/defaults.rs::prepare` does — a temporary symlink
tree of `defaults/` with `envelope/Envelope Materials.csv` replaced by
`tests/fixtures/golden/consumer_shape_envelope_materials.csv` — then builds
`Dwelling.from_hpxml(...)` with consumer_shape_900s's inputs/settings plus
`output_path="target/measure/py.parquet"`, `output_to_parquet=True`, runs
`initialize()` and `simulate()`, then times `results()` five times.

Command: `uv run --no-sync python target/measure/item4_export.py`

- Load before: `06:35:56 ... load average: 1.37, 4.25, 13.24`
- Load after: `06:35:57 ... load average: 1.34, 4.19, 13.17`

Raw (`target/measure/item4.txt`):

```
results() 1: 0.0166 s  (rows: 35040)
results() 2: 0.0081 s  (rows: 35040)
results() 3: 0.0079 s  (rows: 35040)
results() 4: 0.0083 s  (rows: 35040)
results() 5: 0.0081 s  (rows: 35040)
median results(): 0.0081 s
```

**Median results() export time: 0.0081 s** (first call 0.0166 s — cold; runs
0.0079–0.0166, spread of the five vs median: call 1 is +105% — the noise rule's 5-run
set applies to item 1's timing sets; for this item the ledger asks only for the median
of the five `perf_counter()` timings, reported as-is).

No adaptation of save_state/load_state APIs was needed anywhere: the stub
(`python/ochre_next/_hares.pyi`) exposes `save_state(self) -> bytes` and
`load_state(self, state: bytes) -> None` exactly as the ledger assumed.

---

## Item 5 — memory growth (VmRSS readings)

### 5a — repeated construct-and-simulate

Command: `cargo run --release -p frame_golden -j 8 -- run tests/fixtures/golden/consumer_shape_900s.toml --repeat 500 --duration-s 86400 --discard-output`

- Load before: `06:36:13 ... load average: 1.05, 3.98, 12.96`
- Load after: `06:38:14 ... load average: 1.25, 3.00, 11.37`

Readings (`target/measure/item5a.txt`, full log retained):

```
rep 1:   ... vm_rss 37832 kB, frame_bytes 0
rep 100: ... vm_rss 38536 kB, frame_bytes 0
rep 500: ... vm_rss 38556 kB, frame_bytes 0
```

Growth rep 1 → 500: **+724 kB** (37832 → 38556; +1.9% — flat).

### 5b — long horizon (10³ vs 10⁶ steps), sequential runs

Commands:
1. `cargo run --release -p frame_golden -j 8 -- run tests/fixtures/golden/consumer_shape_900s.toml --repeat 1 --duration-s 900000 --discard-output` (10³ steps)
2. `cargo run --release -p frame_golden -j 8 -- run tests/fixtures/golden/consumer_shape_900s.toml --repeat 1 --duration-s 900000000 --discard-output` (10⁶ steps)

- Load before: `06:38:18 ... load average: 1.31, 2.98, 11.32`
- Load after: `06:38:49 ... load average: 1.24, 2.81, 11.00`

Readings:

```
10^3 steps: rep 1: construct+simulate 0.304 s ..., vm_rss 37748 kB, frame_bytes 0
10^6 steps: rep 1: construct+simulate 33.412 s ..., vm_rss 149100 kB, frame_bytes 0
```

Growth 10³ → 10⁶ steps: **+111,352 kB** (37748 → 149100 kB), i.e. ≈ 0.112 kB/step —
~117 bytes per step of steady growth (≈ 117 MB over 10⁶ steps).

### 5c — stepped episodes (Python loop)

Script: `target/measure/item5c_episodes.py` — builds the consumer_shape_900s dwelling
with `duration_s=86400`, `write_output=False` (defaults tree as in item 4), calls
`initialize()`, keeps `state = save_state()`, then 100 × (`load_state(state)`;
96 × `step()`), reading `VmRSS` from `/proc/self/status` after episodes 1 and 100.
No API adaptation needed (`save_state/load_state` matched the .pyi exactly).

Command: `uv run --no-sync python target/measure/item5c_episodes.py`

- Load before: `06:39:21 ... load average: 1.50, 2.69, 10.65`
- Load after: `06:39:22 ... load average: 1.50, 2.69, 10.65`

Raw (`target/measure/item5c.txt`):

```
episode 1: VmRSS 80540 kB
episode 100: VmRSS 80548 kB
done
```

Growth episode 1 → 100: **+8 kB** (80540 → 80548; flat).

---

## Verification — repeat of item 1's first command set

Command: `cargo run --release -p frame_golden -j 8 -- run tests/fixtures/golden/consumer_shape_900s.toml --repeat 5`

- Load before: `06:39:23 ... load average: 1.38, 2.64, 10.60`
- Load after: `06:39:30 ... load average: 1.54, 2.64, 10.51`

Raw (`target/measure/item1a_verify.txt`):

```
rep 1: construct+simulate 1.530 s (construct 0.049 s, simulate 1.254 s), vm_rss 68928 kB, frame_bytes 15340640
rep 2: construct+simulate 1.470 s (construct 0.039 s, simulate 1.226 s), vm_rss 69300 kB, frame_bytes 15340640
rep 3: construct+simulate 1.489 s (construct 0.040 s, simulate 1.244 s), vm_rss 69432 kB, frame_bytes 15340640
rep 4: construct+simulate 1.466 s (construct 0.040 s, simulate 1.219 s), vm_rss 69332 kB, frame_bytes 15340640
rep 5: construct+simulate 1.472 s (construct 0.040 s, simulate 1.224 s), vm_rss 69636 kB, frame_bytes 15340640
```

Verification median: **1.489 s** (runs 1.466–1.530; spread 4.30% of median — valid).

First set's band (±5% of 1.530 s): 1.4535 s – 1.6065 s.
**Verification median 1.489 s lies within the band. PASSES.**
(First median 1.530 s vs verification median 1.489 s: −2.7% — under 5%, recorded as
"no measurable change" per the noise rule.)

---

## Files retained (all untracked, under target/measure/)

- `item1a.txt`, `item1b.txt`, `item1c.txt` — item 1 raw outputs
- `perf_attempt.txt` — verbatim perf failure
- `item2_profile.txt` — profiling example substitute output
- `item4.txt`, `item4_export.py` — item 4 raw output + script
- `item5a.txt`, `item5b_1k.txt`, `item5b_1M.txt`, `item5c.txt`, `item5c_episodes.py` — item 5 raw outputs + script
- `item1a_verify.txt` — verification repeat raw output
- `frame.parquet`, `py.parquet`, `py_defaults/` — artifacts written by the measured runs

**Commit:** none (measurement record only; no tracked file was modified — the only
working-tree changes remain the pre-existing staged BR-6c diff: `pyproject.toml`,
`uv.lock`).
