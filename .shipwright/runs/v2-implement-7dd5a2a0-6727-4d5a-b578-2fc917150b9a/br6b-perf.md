# BR-6b dependency upgrade: physics-simulation performance and peak-memory effect

Method mirrors OR-5 (`.shipwright/initiatives/I-08/ledger/02-measurement.md`, entry OR-5):
the profiling example `profile_sim` (`cargo run --release -p hares-core --features profiling
--example profile_sim -j 8 -- 45 3` = three 45-day runs at 900 s per repetition set, fresh
construction each run, one untimed warm-up first), wrapped in `/usr/bin/time -v` for max RSS.
All cargo commands at `-j 8`, run strictly sequentially.

- AFTER = current tree, branch `feat/output-results`, commit `872cf289` ("build: upgrade
  every Rust dependency to its latest release"), `CARGO_TARGET_DIR=target`.
- BEFORE = parent `39488581` ("refactor: raise rust-version to 1.89 and collapse nested ifs
  into let chains"), pre-upgrade dependencies, worktree `.workstrees/br6b-parent` with the
  isolated target dir `/home/rich/src/HARES/.workstrees/target-br6b-parent`.
- The example prints one total wall per rep set (no per-run times), so the per-set metric is
  the example's total wall; it also prints per-phase means per run (incl. the `other` bucket).
- 5% noise band per OR-5: under 5% = no measurable change.

Protocol deviation, documented: rep set 1 on each side ran a build inside the same
`/usr/bin/time -v` wrapper (AFTER rebuilt 25 changed-dependency crates; BEFORE was a full
cold build of 219 crates), so rep set 1's max RSS is rustc-inflated and unusable for the
RSS comparison. OR-5's noise rule ("a measurement set is valid only when its runs lie
within 5% of their median; otherwise it is re-run") therefore forced one supplemental warm
rep set (set 3) per side. RSS deltas are computed from the warm (uncontaminated) runs;
wall/phase numbers come from the example itself and are unaffected.

## Summary table

Wall is the example's total for 3 x 45-day runs; phases are means per run in ms.

| Side | Rep set | Wall (ms) | envelope_solve | schedule_load | hvac | io | other | water_heater | max RSS (MB) |
|---|---|---|---|---|---|---|---|---|---|
| BEFORE `39488581` | 1 (cold build) | 888.78 | 26.2 | 13.7 | 10.2 | 0.0 | 208.7 | 0.0 | 307.27 (rustc-inflated) |
| BEFORE `39488581` | 2 (warm) | 859.28 | 25.0 | 13.0 | 9.5 | 0.0 | 204.1 | 0.0 | 77.66 |
| BEFORE `39488581` | 3 (warm, supplemental) | 883.05 | 26.5 | 13.7 | 10.2 | 0.0 | 209.4 | 0.0 | 77.71 |
| AFTER `872cf289` | 1 (25-crate rebuild) | 874.45 | 25.4 | 13.3 | 10.1 | 0.0 | 206.7 | 0.0 | 280.14 (rustc-inflated) |
| AFTER `872cf289` | 2 (warm) | 868.09 | 25.2 | 13.2 | 9.7 | 0.0 | 204.5 | 0.0 | 72.70 |
| AFTER `872cf289` | 3 (warm, supplemental) | 909.42 | 27.8 | 14.7 | 11.1 | 0.0 | 212.8 | 0.0 | 72.46 |

Per-side averages (protocol two-set, and including the supplemental set):

| Metric | BEFORE avg (2-set / 3-set) | AFTER avg (2-set / 3-set) | Delta |
|---|---|---|---|
| Wall (ms) | 874.03 / 877.04 | 871.27 / 883.99 | -0.32% / +0.79% |
| `other` (ms/run) | 206.4 / 207.4 | 205.6 / 208.0 | -0.39% / +0.29% |
| Max RSS, warm runs only (MB) | 77.69 | 72.58 | **-6.57%** (-5.11 MB) |

Rep-set spread for the primary metrics: wall within-side spread 2.9-5.8% across sets
(AFTER set 3 ran hot), RSS within-side spread 0.3% (AFTER) and 0.06% (BEFORE) across warm
runs — the RSS signal is far tighter than the 6.6% between-side gap.

## Deltas and verdict

- **Wall: no measurable change.** -0.32% on the protocol two-set average (+0.79% including
  the supplemental hot set); both far inside the 5% noise band. The `other` phase (the
  bucket OR-5 watched) is likewise unchanged: -0.39% / +0.29%.
- **Peak RSS: measurable, reproducible DECREASE of ~6.6%** (77.69 -> 72.58 MB on warm runs;
  both warm samples per side within 0.3% of each other). This is over the 5% band, so it is
  flagged prominently — but it is a memory improvement, not a regression.
- **Verdict sentence:** the BR-6b dependency upgrade leaves simulation wall time unchanged
  (under 1% in every pairing, inside the 5% noise band) while reproducibly reducing the
  example's peak RSS by about 6.6% (77.7 MB to 72.6 MB) — no performance regression; the
  one over-band signal is a favorable peak-memory change.

## Raw outputs

Each run: full verbatim log as captured (`/usr/bin/time -v` wraps `cargo run`; the
`Elapsed` line covers any compilation plus the run, the `wall`/`ms/run` lines are the
example's own timings of the 3 simulation runs plus warm-up).

### AFTER rep set 1 — commit 872cf289, CARGO_TARGET_DIR=target (25 crates rebuilt, 1m 07s)

```
   Compiling num-bigint v0.5.1
   Compiling simba v0.10.2
   Compiling hares-tariff v0.1.0 (/home/rich/src/HARES/crates/hares-tariff)
   Compiling tinytemplate v1.2.1
   Compiling criterion v0.8.2
   Compiling arrow-buffer v59.3.0
   Compiling arrow-data v59.3.0
   Compiling nalgebra v0.35.0
   Compiling arrow-array v59.3.0
   Compiling arrow-select v59.3.0
   Compiling arrow-row v59.3.0
   Compiling arrow-arith v59.3.0
   Compiling hares-envelope v0.1.0 (/home/rich/src/HARES/crates/hares-envelope)
   Compiling arrow-ord v59.3.0
   Compiling arrow-ipc v59.3.0
   Compiling arrow-string v59.3.0
   Compiling parquet v59.3.0
   Compiling arrow-cast v59.3.0
   Compiling arrow-csv v59.3.0
   Compiling arrow-json v59.3.0
   Compiling arrow v59.3.0
   Compiling hares-equipment v0.1.0 (/home/rich/src/HARES/crates/hares-equipment)
   Compiling hares-io v0.1.0 (/home/rich/src/HARES/crates/hares-io)
   Compiling hares-core v0.1.0 (/home/rich/src/HARES/crates/hares-core)
   Compiling hares-fleet v0.1.0 (/home/rich/src/HARES/crates/hares-fleet)
    Finished `release` profile [optimized] target(s) in 1m 07s
     Running `target/release/examples/profile_sim 45 3`
wall 874.45ms for 3 x 45-day runs (plus one warm-up); per-phase means per run:
  envelope_solve      25.4 ms/run
  schedule_load       13.3 ms/run
  hvac                10.1 ms/run
  io                   0.0 ms/run
  other              206.7 ms/run
  water_heater         0.0 ms/run
	Command being timed: "cargo run --release -p hares-core --features profiling --example profile_sim -j 8 -- 45 3"
	User time (seconds): 1.85
	System time (seconds): 1.64
	Percent of CPU this job got: 5%
	Elapsed (wall clock) time (h:mm:ss or m:ss): 1:06.29
	Maximum resident set size (kbytes): 286860
	Major (requiring I/O) page faults: 16
	Minor (reclaiming a frame) page faults: 137532
	Voluntary context switches: 10242
	Involuntary context switches: 29
	Swaps: 0
	File system inputs: 3816
	File system outputs: 53688
	Page size (bytes): 4096
	Exit status: 0
```

### AFTER rep set 2 — warm build (0.08s)

```
    Finished `release` profile [optimized] target(s) in 0.08s
     Running `target/release/examples/profile_sim 45 3`
wall 868.09ms for 3 x 45-day runs (plus one warm-up); per-phase means per run:
  envelope_solve      25.2 ms/run
  schedule_load       13.2 ms/run
  hvac                 9.7 ms/run
  io                   0.0 ms/run
  other              204.5 ms/run
  water_heater         0.0 ms/run
	Command being timed: "cargo run --release -p hares-core --features profiling --example profile_sim -j 8 -- 45 3"
	User time (seconds): 0.49
	System time (seconds): 0.72
	Percent of CPU this job got: 96%
	Elapsed (wall clock) time (h:mm:ss or m:ss): 0:01.26
	Maximum resident set size (kbytes): 74444
	Minor (reclaiming a frame) page faults: 31092
	Exit status: 0
```

### AFTER rep set 3 (supplemental) — warm build (0.11s)

```
    Finished `release` profile [optimized] target(s) in 0.11s
     Running `target/release/examples/profile_sim 45 3`
wall 909.42ms for 3 x 45-day runs (plus one warm-up); per-phase means per run:
  envelope_solve      27.8 ms/run
  schedule_load       14.7 ms/run
  hvac                11.1 ms/run
  io                   0.0 ms/run
  other              212.8 ms/run
  water_heater         0.0 ms/run
	User time (seconds): 0.59
	System time (seconds): 0.69
	Percent of CPU this job got: 95%
	Elapsed (wall clock) time (h:mm:ss or m:ss): 0:01.35
	Maximum resident set size (kbytes): 74200
	Exit status: 0
```

### BEFORE rep set 1 — commit 39488581, worktree .workstrees/br6b-parent,
### CARGO_TARGET_DIR=/home/rich/src/HARES/.workstrees/target-br6b-parent (cold build,
### 219 crates, 1m 16s; crate list elided, full log kept at
### /tmp/opencode/br6b-before-run1.log)

```
   [219 Compiling lines: proc-macro2 ... arrow v58.4.0, hares-types/-control/-tariff/
    -physics/-envelope/-equipment/-io/-core/-fleet from .workstrees/br6b-parent]
   Compiling hares-fleet v0.1.0 (/home/rich/src/HARES/.workstrees/br6b-parent/crates/hares-fleet)
    Finished `release` profile [optimized] target(s) in 1m 16s
     Running `/home/rich/src/HARES/.workstrees/target-br6b-parent/release/examples/profile_sim 45 3`
wall 888.78ms for 3 x 45-day runs (plus one warm-up); per-phase means per run:
  envelope_solve      26.2 ms/run
  schedule_load       13.7 ms/run
  hvac                10.2 ms/run
  io                   0.0 ms/run
  other              208.7 ms/run
  water_heater         0.0 ms/run
	Command being timed: "cargo run --release -p hares-core --features profiling --example profile_sim -j 8 -- 45 3"
	User time (seconds): 10.27
	System time (seconds): 10.53
	Percent of CPU this job got: 27%
	Elapsed (wall clock) time (h:mm:ss or m:ss): 1:15.54
	Maximum resident set size (kbytes): 314640
	Major (requiring I/O) page faults: 74
	Minor (reclaiming a frame) page faults: 1290412
	Voluntary context switches: 135930
	Involuntary context switches: 1788
	Exit status: 0
```

### BEFORE rep set 2 — warm build (0.13s)

```
    Finished `release` profile [optimized] target(s) in 0.13s
     Running `/home/rich/src/HARES/.workstrees/target-br6b-parent/release/examples/profile_sim 45 3`
wall 859.28ms for 3 x 45-day runs (plus one warm-up); per-phase means per run:
  envelope_solve      25.0 ms/run
  schedule_load       13.0 ms/run
  hvac                 9.5 ms/run
  io                   0.0 ms/run
  other              204.1 ms/run
  water_heater         0.0 ms/run
	User time (seconds): 0.47
	System time (seconds): 0.75
	Percent of CPU this job got: 94%
	Elapsed (wall clock) time (h:mm:ss or m:ss): 0:01.31
	Maximum resident set size (kbytes): 79528
	Exit status: 0
```

### BEFORE rep set 3 (supplemental) — warm build (0.10s)

```
    Finished `release` profile [optimized] target(s) in 0.10s
     Running `/home/rich/src/HARES/.workstrees/target-br6b-parent/release/examples/profile_sim 45 3`
wall 883.05ms for 3 x 45-day runs (plus one warm-up); per-phase means per run:
  envelope_solve      26.5 ms/run
  schedule_load       13.7 ms/run
  hvac                10.2 ms/run
  io                   0.0 ms/run
  other              209.4 ms/run
  water_heater         0.0 ms/run
	User time (seconds): 0.49
	System time (seconds): 0.75
	Percent of CPU this job got: 96%
	Elapsed (wall clock) time (h:mm:ss or m:ss): 0:01.30
	Maximum resident set size (kbytes): 79572
	Exit status: 0
```

## Host and cleanup notes

- Host: WSL2 Linux x86_64, 28 cores, 64 GB RAM; all runs sequential, `-j 8`; nothing
  committed; no tracked file modified (this report is under untracked `.shipwright/runs/`).
- Worktree `.workstrees/br6b-parent` removed after measurement
  (`git worktree remove .workstrees/br6b-parent --force`).
- NOT removed: the isolated target dir
  `/home/rich/src/HARES/.workstrees/target-br6b-parent` (850 MB) — the session harness
  blocked `rm -rf`; remove manually to reclaim the disk.
