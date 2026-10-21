# Reference state, golden tool and baseline

Every bitwise claim and every performance number in this ledger depends on
three things that do not exist at `ebdb1a17`: a record of the state the
opening sequence starts from, a capture/compare tool with stored fixtures,
and a baseline measured on the upgraded tree. `tests/fixtures/golden/` holds
only `.gitkeep`, and no capture or compare tool exists in the tree.

Order: M-0 right after Task 0; M-1 and M-2 after BR-8 and before BR-6a (the
dependency upgrade is compared against M-2's captures); M-3 after BR-6d and
BR-11 (BR-11 corrects the fixture M-3 measures);
M-4a after the last numeric entry and M-4b after the operator's first push (`plan.md` §3 closing
sequence).

## M-0: record the reference state

**Kind:** initiative work (measurement, no code change).

**Problem:** the opening sequence turns block lines green one by one, the
`[pytest]` line compares skips against a census, and the consumer gate
compares against base failure and skip lists. None of these references is
recorded.

**Change:** at the Task 0 head, with the Task 0 stray files removed, record in
this file's log:

1. The result of each verification-block line (`plan.md` §3), and for each
   failing line the entry that turns it green. A failure that no entry
   covers becomes a baseline-repair entry in `01-baseline-repair.md`, ordered
   before M-1.
2. The pytest skip census: run `[python-build]` and the pytest command of
   `[pytest]`, and write `{ grep '^SKIPPED' target/pytest.log || true; } | sort`
   to `ledger/pytest-skip-census.txt`. Each census line names a file, a line
   and a reason; `[pytest]` compares file, count and reason with the line
   number stripped on both sides (`plan.md` §3 `census()`), so the line
   numbers are for reading. A later entry changes the census only by
   writing the change into the file in its own commit's verification, with
   the reason in the log.
3. The consumer base gate: at `c71c542`, with the engine built from the Task 0
   head through the engine-under-test mechanism (`08-consumer.md`), run the
   consumer gate and record the exact names of every failing and every
   skipped case, the summary line and `paths.summary()`. The committed
   `docs/Test_Report.md` ("325 passed, 3 failed, 7 skipped" of 335) was
   generated on 2026-09-22 against the older pin `7a3366a7`, before the
   engine's equipment-identity fix (`2ee83a86`); the consumer's own record
   of the `0ec757cb` pin (`docs/Red_Team_Record.md:144-155`, the `c71c542`
   commit message) reads "329 passed, 0 failed, 7 skipped, 336 total". The run recorded here is the
   base; `08-consumer.md` CN-16 owns the three failures the stale report
   names. This gate and B0's read the consumer's engine schema
   (`results/engine_schema.json`) written on 2026-09-22; the base on a
   schema regenerated against the engine under test is recorded by
   `08-consumer.md` CN-17.

**Acceptance test:** none (a record). The acceptance is that the three items
are in the log with the commands that produced them and the commit.

**Verification:** re-run item 2 once; the census is identical.

**Commit:** none (`ledger/` is not part of the repository).

**Dependencies:** Task 0.

## M-1: the golden capture and compare tool

**Kind:** initiative work (measurement infrastructure).

**Problem:** nothing captures a run's products and compares two captures
bit for bit, so no entry can prove it is bitwise-neutral or record the delta
a numeric change causes.

**Evidence:** `tests/fixtures/golden/` contains only `.gitkeep`;
`Cargo.toml:2-13` lists no tool crate; `.gitignore:49` ignores `*.parquet`
everywhere except `tests/fixtures/parity/**` (`:54`).

**Change:** add `tools/frame_golden`, a library plus binary crate, listed in
`[workspace] members`, depended on by nothing. Features: `dst` and
`profiling`, each forwarding to the `hares-core` feature of the same name.
Golden references committed to the repository are compact per-column bit
digests plus metrics, never full-year frames; full frames live under
`target/` for local delta inspection (`dl-i08-architecture.golden_storage`).

*Manifest.* Each fixture is `tests/fixtures/golden/<name>.toml`, parsed with
unknown keys rejected:

```toml
kind = "dwelling"              # "dwelling" (exactly one [[home]]) or "fleet" (two or more)
features = []                  # [] or ["dst"]: the exact feature set the fixture runs under
defaults = "defaults"          # repository-relative defaults directory

[defaults_files]               # optional: defaults-relative path -> repository file that replaces it
# "envelope/Envelope Materials.csv" = "tests/fixtures/golden/consumer_shape_envelope_materials.csv"

[simulation]                   # a SimulationConfig table, verbatim
start_time = "2023-01-01T00:00:00-07:00"
duration = 31536000
time_res = 900
output_verbosity = 2
master_seed = 0

[[home]]
bldg_id = 1
hpxml = "tests/fixtures/parity/cz4a_ashp_hpwh/building.xml"
schedule = "tests/fixtures/parity/cz4a_ashp_hpwh/schedule.csv"
weather = "tests/fixtures/parity/cz4a_ashp_hpwh/weather.epw"
initialization_duration_s = 0
overrides = {}                 # the engine's override map, verbatim
```

`[simulation]` keys are checked against `SIMULATION_KEYS`, the
`SimulationConfig` field list (`tools/frame_golden/src/manifest.rs:72-87`).
Until `06-runtime-memory.md` RT-4 the adapter builds a `[defaults_files]`
replacement as a temporary directory of symbolic links to the repository
defaults with the listed files copied in; from RT-4 it passes them as
`DefaultsSource.files`. Every later entry that changes a
config key the manifests use (OR-3 `reports`, OR-8 `metrics = true`, B-2
`warmup = "disabled"`) migrates the manifests in its own commit; the
migration is config-only and every golden is unchanged by it.

*Committed golden.* `capture` writes one file next to the manifest,
`<name>.golden.json`, holding for each product:

- the schema (names, Arrow types, nullability, order) and the row count;
- per column: a SHA-256 over the validity bitmap and the value bits of the
  whole column (floats by `to_bits()` little-endian, so NaN payloads count;
  timestamps and integers by their little-endian bytes; strings by length
  and bytes), and a SHA-256 per block of 96 rows, which locates a first
  difference without the full frame;

and, in full, the metrics product (one row per home, every field of the run
metrics flattened to a dotted field path such as
`"total_energy_kwh.net_energy_kwh"`, as its `f64` bit pattern in hex with
its decimal value beside it for reading, `None` as
`{"bits": null, "value": null}` and enums by variant name, e.g.
`"coverage": "PartialYear"`; `tools/frame_golden/src/golden.rs:87`). The
products are:

- a dwelling: the frame; `billing` when a tariff is configured; `health`,
  the serialised `RunHealth` (the counters and the tagged
  `WarmupOutcome`);
- a fleet: `aggregate` (today's `aggregate_timeseries`); `weights`, one
  row per home holding its sample weight, the stand-in for covered weights
  until `04-fleet.md` FL-1 (`tools/frame_golden/src/adapter.rs:349-370`);
  `homes` (`bldg_id`, energy, peak, weight, status, failed;
  `adapter.rs:372-430`); `health` is `null` (`golden.rs:58-60`).

From `09-numeric-fixes.md` B-14's first commit (row 94c) every manifest
also carries a `resolved` product (`tools/frame_golden/src/resolved.rs`,
new), added under this entry's schema change rule: per equipment in model
order, the equipment's name and, per `ResolvedParameter`, the parameter's
name, `Unit::symbol()` and its value as the 16-digit hex of
`f64::to_bits`. It is written by `frame_golden`'s own JSON writer, not by
serialising `ResolvedModel` (which gains no serde derive), and it covers
equipment parameters only, so a row that touches no equipment parameter
leaves it bitwise. From row 94c on, `compare` and `compare-all` compare
it like any other product, and a row that moves a resolved equipment
parameter re-captures it in its own commit through the numeric entry
procedure above.

The file also records the defaults directory (repository-relative) and a
SHA-256 over every file of the defaults tree the run used, sorted by
relative path (relative path, `0x00`, bytes; the symbolic links of a
replacement tree followed; `tools/frame_golden/src/defaults.rs:71-82`), so
a golden captured against another defaults tree is detectable; the feature
set; `git rev-parse HEAD` and whether the tree was dirty. JSON keys are
sorted and the file is byte-stable for one capture.

*Local full frames.* `capture` and `materialize` also write every product
in full as ZSTD Parquet under `target/golden/<name>/` (never committed;
`target/` is ignored). `materialize <name>` re-runs a manifest, checks that
its digests equal the committed golden, and writes the full products; it is
how a numeric entry, at its parent commit, obtains the reference frames that
`delta` compares against.

*Engine adapter.* `tools/frame_golden/src/adapter.rs` is the only module that
calls the engine. At this commit it runs a dwelling with `write_output: true`,
`retain_batches: true` and an `output_path` inside a `tempfile` directory it
deletes, reads `flushed_batches()`, the run metrics and `Dwelling::health()`
(OR-5 lands before this entry), and runs a fleet on an explicit
single-thread pool. Every later entry that changes how results are obtained
updates this module in that entry's commit (`03-output-reporting.md` OR-3
and OR-6 among them), so every commit compiles and passes `[golden]`.

*Subcommands.*

```
frame_golden capture <name>
frame_golden materialize <name> | materialize-all
frame_golden compare <name> [--columns <c1,c2,...>]
frame_golden compare-all
frame_golden delta <name> [--columns <c1,c2,...>]
frame_golden diff <dir-a> <dir-b>
frame_golden run <manifest-path> [--repeat <k>] [--duration-s <s>]
                                 [--output-dir <dir> | --discard-output]
```

- `compare` re-runs the manifest and compares with the committed golden:
  schema (with `--columns`, only the listed columns, which must exist on
  both sides), row count, then each column's digest; the metrics product
  field by field on bits. On a difference it prints the product, each
  differing column and its first differing 96-row block; when
  `target/golden/<name>/` holds full products whose digests equal the
  committed golden, it also prints the first differing row with both values
  and bit patterns and the count of differing cells per column. It exits
  non-zero on any difference.
- `compare-all` compares every manifest whose `features` equals the running
  binary's feature set exactly, prints the names it compared, and exits
  non-zero if any comparison fails or if none matched. The block runs it
  once without and once with `-F dst`, which together cover every manifest.
- `delta` re-runs and compares against the full products under
  `target/golden/<name>/` (it fails naming `materialize` if they are absent
  or do not match the committed digests) and prints, per differing column:
  differing-cell count, validity changes, maximum absolute and maximum
  relative difference; and every metrics field that changed. It exits zero;
  it is how a numeric entry records its delta before `capture`.
- `diff` prints the same per-column report between two directories of full
  products (M-4b uses it across platforms).
- `run` executes a manifest without comparing and prints, per repetition,
  construction-plus-simulate wall seconds, `VmRSS` in kB after the run (from
  `/proc/self/status`; `unavailable` on other platforms) and the in-memory
  frame's `get_array_memory_size()` bytes. `--output-dir` writes the full
  products as Parquet into that directory; `--discard-output` keeps no frame
  (`write_output: false`, `retain_batches: false`); `--duration-s` overrides
  the manifest's duration. Built with `-F profiling`, it also prints the
  dwelling profiling summary per repetition.

*Numeric entry procedure.* At the parent commit, `materialize-all`; make the
change; `delta` on every fixture (recorded in the log); `capture` on every
fixture, committed in the entry's commit.

*Schema change rule.* An entry that intentionally changes a product's schema
(for example `Time` becoming a timestamp) runs `compare --columns` over the
unchanged columns (must pass), `delta` over the changed ones (recorded), and
re-captures in its own commit.

**Acceptance test:** in `tools/frame_golden/tests/`:

- `digest_of_identical_batches_is_equal`;
- `digest_detects_one_bit_of_one_f64` and reports its column and block;
- `digest_distinguishes_nan_payloads` (two NaNs with different payloads
  differ; identical NaNs match);
- `digest_distinguishes_validity`;
- `digest_is_independent_of_batch_chunking` (one table split into batches of
  1, 7 and 96 rows gives one digest);
- `compare_rejects_schema_mismatch_unless_columns_selected`;
- `compare_reports_first_mismatch_row_from_materialized_frames`;
- `delta_reports_per_column_max_abs_difference`;
- `delta_without_materialized_frames_names_materialize`;
- `manifest_rejects_unknown_key`;
- `manifest_dwelling_kind_requires_exactly_one_home`;
- `compare_all_selects_manifests_by_exact_feature_set`;
- `golden_json_is_byte_stable` (two captures of one run write identical
  bytes);
- `capture_then_compare_round_trips` (a one-day manifest captured into a
  temporary golden directory and compared against itself).

**Verification:** block lines green at this point (all except
`[advisories]`); `[golden]` starts running with M-2.

**Commit:** `feat: add a golden capture and bitwise compare tool`

**Dependencies:** BR-8.

## M-2: golden fixtures

**Kind:** initiative work.

**Problem:** no fixture crosses DST transitions, month boundaries, February
29, the end of a weather file, or matches the calibration consumer's run
shape, so none of the bitwise claims in `03`, `06` and `09`, and none of the
dependency-upgrade deltas, can be checked.

**Change:** add these manifests and capture them on the tree before BR-6a.
All use `master_seed = 0`, `output_verbosity = 2`,
`initialization_duration_s = 0`, `defaults = "defaults"`, no resample
overrides. The committed files are the six manifests, the six
`.golden.json` files and one replacement defaults file; no frame is
committed.

| Name | Inputs | Start (local) | Duration | Step | Features | Rows |
|---|---|---|---|---|---|---|
| `full_year_hourly` | `tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh/` (`building.xml`, `schedule.csv`, `weather.epw`) | `2023-01-01T00:00:00-07:00` | 365 d | 3600 s | `[]` | 8 760 |
| `full_year_900s_dst` | `tests/fixtures/parity/cz4a_ashp_hpwh/`, `civil_timezone = "America/Denver"` | `2023-01-01T00:00:00-07:00` | 365 d | 900 s | `["dst"]` | 35 040 |
| `leap_year_february_900s` | `tests/fixtures/parity/cz4a_ashp_hpwh/` | `2024-02-27T00:00:00-07:00` | 5 d | 900 s | `[]` | 480 |
| `weather_wrap_900s` | `tests/fixtures/parity/cz4a_ashp_hpwh/` | `2023-12-30T00:00:00-07:00` | 4 d | 900 s | `[]` | 384 |
| `consumer_shape_900s` | `tests/fixtures/resstock/2025.1/bldg0176227/` (`home.xml`, `in.schedules.csv`), weather `tests/fixtures/resstock/2025.1/weather/G0600770_2018.csv`, overrides `{"Gas Water Heater" = {ua_w_per_k = 2.0}}`, `[defaults_files]` replacing `envelope/Envelope Materials.csv` with `tests/fixtures/golden/consumer_shape_envelope_materials.csv` | `2018-01-01T00:00:00-08:00` | 365 d | 900 s | `[]` | 35 040 |
| `fleet_mixed_offset` | `bldg0000002` (G0900090, UTC-5), `bldg0000004` (G4803030, UTC-6), `bldg0176227` (G0600770, UTC-8), each with its own `_2018.csv`, weight 1.0 | `2018-01-01T00:00:00-06:00` | 7 d | 3600 s | `[]` | pinned at capture |

Notes on the choices:

- The clock is fixed-offset (`crates/hares-core/src/clock.rs:27-56`): a year at
  900 s has 35 040 rows with or without DST; `dst` changes civil calendar
  fields, not the row count. `full_year_900s_dst` spans the 2023-03-12 and
  2023-11-05 transitions.
- No checked-in weather file holds a Feb 29 (the parity EPWs are TMY3 files
  whose rows come from several source years, none of them a Feb 29; the
  ResStock CSVs are AMY 2018). `leap_year_february_900s` pins
  today's handling of a 2024 run over such a file, which
  `09-numeric-fixes.md` B-1c changes (Feb 29 is skipped, as in EnergyPlus)
  and records.
- `weather_wrap_900s` reads past the last record of an 8 760-row file.
- `consumer_shape_900s` mirrors the consumer's call (`calibration/engine.py`
  `Runner.build_kwargs`, `calibration/grid.py:158` `STUDY_GRID`: 900 s, no
  warm-up; verbosity 2; a ResStock AMY 2018 CSV; a sparse per-equipment
  override map as `calibration/inject.py:148-153` builds it, using the
  consumer's `wh_ua_scale` key `Gas Water Heater`/`ua_w_per_k`,
  `calibration/registry.py:202-203`; and a defaults variant as
  `calibration/defaults.py` writes it, the `Resistance (m^2-K/W)` column of
  `envelope/Envelope Materials.csv` scaled by 1.3 and the
  `Capacitance (kJ/m^2-K)` column by 1.5, the consumer's `opaque_r_scale`
  and `thermal_mass_scale` routes, `calibration/registry.py:161-172`). The
  replacement file is generated once from the repository's
  `defaults/envelope/Envelope Materials.csv` with those two columns scaled
  and every other byte unchanged, and committed.
- `fleet_mixed_offset` spans three UTC offsets; its aggregate row count is the
  one today's fleet path produces, pinned in the golden: zero rows, because
  its three homes declare different variables and today's fleet path then
  empties the aggregate. `04-fleet.md` FL-2 makes it non-empty and FL-4
  changes it by the declared alignment, each recording its delta.

**Acceptance test:** the six manifests and their `.golden.json` files
exist; `frame_golden compare-all` passes in both feature sets immediately
after capture; each dwelling frame has exactly the row count in the table;
`git ls-files tests/fixtures/golden` lists no `.parquet` file.

**Verification:** turns `[golden]` green. Record each committed file's size
and each local full product's size in the log.

**Commit:** `test: capture golden fixtures for DST, leap day, weather wrap, month boundaries and the consumer run shape`

**Dependencies:** M-1.

## M-3: the baseline

**Kind:** initiative work (measurement, no code change).

**Problem:** `context.md`'s baseline table was measured on consumer homes
outside this repository, before the dependency upgrade. Every before/after
number in this ledger needs a baseline anyone can reproduce from the
repository.

**Change:** on the BR-11 head (`8b09ac4a`), release build, record in the log with the
exact command, `uptime` load average before and after, and the commit:

1. `simulate()` wall with the engine's own output (its Parquet file
   written by the engine's writer and its batches retained, today's only
   way to keep results, which is the cost HARES#28 is about) and with
   nothing kept:
   `cargo run --release -p frame_golden -- run tests/fixtures/golden/consumer_shape_900s.toml --repeat 5`
   (the tool's default run mode sets `write_output`, `retain_batches` and
   an engine output file in a temporary directory,
   `tools/frame_golden/src/adapter.rs:146-176`) and the same with
   `--discard-output`. `--output-dir` is not a baseline variant: it adds
   the tool's own writer of its products, not the engine's. Each
   repetition is a fresh construction; report the median of 5.
2. Function shares of one run:
   `perf record -e cpu-clock:u --call-graph dwarf -o target/measure/perf.data -- target/release/frame_golden run tests/fixtures/golden/consumer_shape_900s.toml --repeat 1`,
   then `perf report -i target/measure/perf.data --no-children --sort symbol`.
   Record the shares of: the per-step telemetry map clone
   (`crates/hares-core/src/dwelling/mod.rs:6966`, `:5278`), `record_step`,
   `update_in_place`, `prepare_inputs_inner`,
   `solve_ideal_capacity_for_target`, the envelope solve, and
   `Dwelling::check_invariants`.
3. Retained frame bytes for the full-year run (item 1's in-memory run).
4. Python export time: in the block's environment,
   `uv run --no-sync python -c` a script that builds
   `Dwelling.from_hpxml(...)` with `consumer_shape_900s`'s inputs and
   settings, including its defaults: `defaults_path` names a directory
   under `target/measure/defaults` built as the tool builds it (a tree of
   symlinks to `defaults/` with `envelope/Envelope Materials.csv` replaced
   by `tests/fixtures/golden/consumer_shape_envelope_materials.csv`,
   `tools/frame_golden/src/defaults.rs`), its override
   `{"Gas Water Heater": {"ua_w_per_k": 2.0}}` and no warm-up, plus
   `output_path="target/measure/py.parquet"` and
   `output_to_parquet=True`, runs `initialize()` and `simulate()`, then times
   `dwelling.results()` five times with `time.perf_counter()` and records the
   median.
5. Memory growth, each as `VmRSS` readings:
   - repeated construct-and-simulate: `run ... --repeat 500 --duration-s 86400 --discard-output`,
     readings after repetitions 1, 100 and 500;
   - long horizon: `run ... --repeat 1 --duration-s 900000 --discard-output`
     (10^3 steps) against `--duration-s 900000000` (10^6 steps);
   - stepped episodes: a Python loop that builds the `consumer_shape_900s`
     dwelling with `duration_s=86400` and `write_output=False`, calls
     `initialize()`, keeps `save_state()`, then 100 times calls
     `load_state(state)` and `step()` 96 times, reading `VmRSS` from
     `/proc/self/status` after episodes 1 and 100.

Noise rule: a measurement set is valid when its five runs lie within 5% of
their median; otherwise it is re-run. A before/after difference under 5% is
recorded as "no measurable change".

**Acceptance test:** none (a record). The acceptance is that every item is in
the log with its command line, load average and commit.

**Verification:** repeat item 1 once; the new median lies within the first
set's band.

**Commit:** none (`ledger/` is not part of the repository).

**Dependencies:** BR-6d, M-2, `01-baseline-repair.md` BR-11.

## M-4a and M-4b: cross-platform agreement, measured and published

**Kind:** initiative work (`dl-i08-architecture.reproducibility`,
`cross_platform_measurement`). Two entries: M-4a adds the workflow, M-4b
publishes its result.

**Problem:** bitwise reproducibility is tested per platform (RT-5, FL-1),
but nothing measures whether a macOS arm64 build and a Linux x86_64 build
produce the same numbers, and nothing tells a user how far they differ.

**Change:**

- M-4a: `.github/workflows/reproducibility.yml` (new), triggered by
  `workflow_dispatch` and by a pull request into `initial-implementation`
  (the triggers `01-baseline-repair.md` BR-10 gives CI; no feature-branch
  name appears in a workflow): a matrix of
  `ubuntu-24.04` (x86_64) and `macos-15` (arm64) runners, each with the
  rustc release `01-baseline-repair.md` BR-10 pins for the golden job,
  building `frame_golden` in release (with and without `-F dst`) and running `run
  --output-dir` on every M-2 manifest, then `compare-all`; each job uploads
  its full products and its `compare-all` output as an artifact. The job
  records its runner architecture and `rustc -V`.
- M-4b: after the run, the two artifacts are downloaded and compared with
  `frame_golden diff`. `docs/reproducibility.md` (new) states: what is
  tested bitwise on one platform (processes, thread counts, runs), the
  platforms measured, the commit and workflow run, and per fixture the
  result: identical, or per differing column the differing-cell count and
  the maximum absolute and relative difference, plus every metrics field
  that differs.

**Acceptance test:** the Linux job's `compare-all` passes (the committed
goldens are Linux x86_64 captures); `docs/reproducibility.md` names the
workflow run it was written from and reproduces from the commands it gives.

**Verification:** whole block for each commit.

**Commit:** M-4a `ci: measure golden agreement between Linux x86_64 and macOS arm64`;
M-4b `docs: publish cross-platform agreement of the golden fixtures`.

**Dependencies:** M-4a: M-2 and every numeric entry
(`09-numeric-fixes.md`, so the published numbers are the final engine's).
M-4b: M-4a and the operator's first push of `feat/output-results` with the
pull request into `initial-implementation` (`plan.md` §3 closing
sequence), which runs the workflow.

## Entries that change the golden format

M-1 and M-2 above define the format and fixtures as committed (`979b8773`,
`7643b4e7`). These entries change them, in schedule order:
- `01-baseline-repair.md` BR-11 (`8b09ac4a`): `initialization_duration_s =
  0` runs no warm-up and a positive value warms up; the manifests that
  warmed up carry `604800`, and `consumer_shape_900s` carries `0` and was
  re-captured (log below).
- BR-13: `compare` and `materialize` fail when the run's defaults digest
  differs from the recorded one, naming both; `delta` reports it.
- BR-14: an optional `[tariff]` table (a JSON `ElectricTariff` file, read
  with unknown keys rejected, and an IANA zone) and the fixture `month_boundary_billing_3600s`
  (`tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh/`, start
  `2023-01-25T00:00:00-07:00`, 12 d at 3600 s, tariff
  `tests/fixtures/golden/flat_tariff.json` in `America/Denver`), the first
  golden with a `billing` product.
- BR-15: each fleet home states its `weight`, and a fleet manifest states
  its aggregation `resolution`.
- `04-fleet.md` FL-1: its first commit captures `fleet_same_offset_300s`
  (kind `fleet`, three `bldg0176227` homes on three UTC-8 weather files,
  weights 1.0, 2.5 and 0.5, `resolution = "fifteen_min"`, 2 d at 300 s);
  its second replaces `resolution` with a required `[fleet]` table (a
  `FleetReport`), and the fleet products become `aggregate`,
  `covered_weight`, `homes` and `metrics` (`weights` goes).
- FL-2: `fleet_mixed_offset`'s `aggregate` and `covered_weight` become
  non-empty and its `homes` status values change spelling; re-captured.
- FL-4: the `[fleet]` table gains an optional `alignment`;
  `fleet_mixed_offset` declares `Instant(+00:00)`.
- `09-numeric-fixes.md` B-2: `warmup` in `[simulation]` replaces
  `initialization_duration_s` (`0` becomes `warmup = "disabled"`, `604800`
  the default).
- B-9a: `kind = "resstock_fleet"` and the fixture `resstock_2025_fleet`.
- B-14: its first commit adds the fixtures `der_pv_battery_900s`
  (`base-pv-battery.xml` with a self-consumption `bms_mode` override, 7 d
  at 900 s), the first golden with PV and a battery, and
  `der_pv_battery_ev_900s` (`tests/fixtures/parity/cz6b_pv_battery_ev/`,
  the same override, 7 d at 900 s), the first with an EV, plus
  `tests/fixtures/golden/resolved/<manifest>.json`, the resolved model of
  every golden home, which `resolved_models_are_unit_invariant` reads.

## Records the schedule requires of later rows

Each is appended to the measurement log below by the row named, with the
fields every log row carries:
- Rows 17 and 18 (`07-profiler.md` PR-5 `1d471677` and PR-6 `afa9a071`),
  appended from the implementer's row records before row 19 commits: the
  `[tools]` line and the verification block's result at each commit, each
  record stating that the row's B0 check is the row 16g re-check.
- Row 19 (`03-output-reporting.md` OR-11 `b8195c83`): its Measurement,
  the `[tools]` line and the block's result, with its B0 check stated as
  the row 16g re-check.
- Rows 20 onward (SL-7 `7b7a6a7d`, RT-3 `44da330c`, RT-3a `69950a98` and
  each later engine row): the same, with the B0 check stated as the row
  16g re-check, or as CN-17's gate for the gate engine's row.
- Row 16a (`08-consumer.md` CN-17): the consumer gate on a worktree of
  the gate engine (named) with step 4b, its base skip set on the
  regenerated schema, the DE-trace comparison with B0 (the gate engine
  row's B0 check and BR-6d's acceptance), and the contract-drift census
  over every engine row up to the gate engine.
- Row 16g (the B0 re-check), on the CN-17 consumer: one record per commit,
  in row order, for BR-11 `8b09ac4a` (the cumulative check of rows 1 to
  12, since its engine is row 12's), PR-1 `d7f0e381`, PR-2 `21d90588`,
  PR-3 `26626710`, PR-5 `1d471677`, PR-6 `afa9a071`, OR-11 `b8195c83`,
  SL-7 `7b7a6a7d`, RT-3 `44da330c`, every later engine commit before the
  gate engine, and the gate engine (the capture of CN-17's gate, entered
  here, not repeated): the dry-run
  line of mechanism step 4 and the DE-trace comparison with B0. A
  difference at `8b09ac4a` adds one record per bisection step over the
  engine commits `1d60f478`..`75a182a0` and names the row it finds.
- Row 16h (`11-other-defects.md` OD-15): the triage of the January
  cooling energy.
- Row 21 (`06-runtime-memory.md` RT-3): the ideal-capacity phase's share
  of `simulate()` on `consumer_shape_900s` after the entry, and the
  decision it takes at the 5% line.
- Row 87a (`04-fleet.md` FL-5): the delta of every changed column of the
  acceptance run (the reconciled `Named` selection, the verbosity-8 tier
  plus the three weather channels) on the `fleet_same_offset_300s`
  inputs, and the empty golden delta.
- Row 88 (`09-numeric-fixes.md` B-11): the delta of every `Enum` column of
  the same `Pattern("*")` run, and every golden unchanged.
- Row 94a (`11-other-defects.md` OD-9): the delta of the generated-schedule
  run on `bldg0000002` (from Friday 2018-01-05, 3 d), every variable.
- Rows 94c, 94g and 94h (`09-numeric-fixes.md` B-14): per commit, each
  golden `identical` or its moved columns with the largest relative
  difference and the conversion that explains it, and the EV cold-snap
  run's delta.
- Row 94f (`11-other-defects.md` OD-10's numeric commit): the delta on
  `base.xml` and the two `base-bldgtype-multifamily*` files.
- Row 94i (`11-other-defects.md` OD-20): the triage (which PV attaches to
  which roof at the parent, and why) and the delta on the PV goldens.

## Measurement log

Appended to by M-0, M-3 and every entry with a measurement or a recorded
delta. Each row: date, git commit, entry, quantity, before, after, command,
load average.

### M-0, 2026-09-30, Task 0 head `1d60f478` (feat/output-results)

Load average at write time: 4.10 (1 min). Full command output:
`.shipwright/runs/v2-implement-7dd5a2a0-6727-4d5a-b578-2fc917150b9a/m0-block-results.md`.

Task 0 state on this host: PR #27 was open on origin, so the merge was made
locally (`1d60f478`, a merge of `ebdb1a17` with `fix/data-host-renamed-nlr`);
nothing is pushed. `i08-consumer` was created at `c71c542`. The fourteen
stray `dwelling_*.csv` files of BR-7's evidence list were moved out of the
tree to `target/stray-quarantine/` (the host blocks file deletion; `target/`
is excluded from the stray check). Two root-level copies were overwritten
during the move by same-named `crates/hares-core` copies; all were
git-ignored test artifacts.

Pin provenance: `0ec757cb` (nested engine clone) fetched into this
repository; `tree(0ec757cb)` = `81dc665571b737baab812dbcfe359b6c6b944a48`,
`tree(1d60f478)` = `b48a53cb803b1c2966321588db0d4eae7ce47abd`. The diff is
exactly the `cargo fmt` commit (`89f32822`) reformatting
`crates/hares-core/examples/profile_sim.rs` and
`crates/hares-python/src/py_dwelling.rs`; no semantic difference.

`[tools]` (installed binaries, each installed before this record; read on
2026-10-02 and unchanged since): `rustc 1.94.1 (e408947bf 2026-03-25)`,
`cargo-nextest 0.9.137 (75ddba7e9 2026-05-26)`, `cargo-deny 0.20.2`,
`uv 0.12.8`.

1. Verification-block lines at the Task 0 head:

| Line | Result | Turned green by |
|---|---|---|
| [fmt] | exit 0 (2 s) | green |
| [clippy] | exit 101: `E0425 cannot find value sum_sq_dod` at `crates/hares-equipment/src/battery/mod.rs:1468` | BR-1, then BR-2 |
| [test-debug] | exit 0, 117 s | green |
| [test-observe] | exit 101: the same `E0425` compile error | BR-1 |
| [test-dst] | exit 0, 142 s (the nested test does not run: BR-3) | green (BR-3 un-nests) |
| [test-profiling] | exit 0, 186 s | green |
| [test-observe-profiling] | exit 101: the same `E0425` compile error | BR-1 |
| [test-release-invariants] | exit 0, 90 s | green (deleted by OR-11) |
| [test-release] | exit 100, 6954 of 6957 passed: `dwelling::tests::build_equipment_column_map_still_panics_on_schema_drift_for_known_equipment`, `ev_soc_check_fails_loudly_when_core_output_soc_is_absent`, `ev_capacity_check_fails_loudly_when_ev_telemetry_keys_are_absent` | OR-5 |
| [bench] | exit 0 (3 s, cached) | green |
| [advisories] | exit 1: RUSTSEC-2023-0089 (atomic-polyfill), RUSTSEC-2024-0436 (paste), RUSTSEC-2026-0176 and RUSTSEC-2026-0177 (pyo3), RUSTSEC-2026-0194 and RUSTSEC-2026-0195 (quick-xml) | BR-6b |
| [golden] | exit 101: no crate `frame_golden` | M-1, M-2 |
| [python-build] | exit 0, 41 s | green |
| [pytest] | exit 0: 1377 passed, 9 skipped, 274 s | census written |
| [stray] | the block recreated 12 git-ignored `dwelling_*.csv` files | BR-7 |

No failing line lacks a covering entry.

2. Pytest skip census: the `[python-build]` line then the `[pytest]` line,
`{ grep '^SKIPPED' target/pytest.log || true; } | sort`, written to
`ledger/pytest-skip-census.txt` (9 lines: 3 OCHRE-not-installed, 6
ResStock-fixtures-not-available). Re-run once: identical (verification of
this item).

3. Consumer base gate at `c71c542` against the Task 0 engine head
   (worktree `.workstrees/1d60f478`, engine sha256 `28be65c0835f3622`),
   through the engine-under-test mechanism (`08-consumer.md`; the dry-run
   check printed exactly one "Would uninstall" line, `ochre-next`):

   - `run_all`: `329 passed, 0 failed, 7 skipped, 336 total`. The failing
     set is empty; the skip set is exactly the seven environmental base
     skips (the gas-furnace cell definition case and six GridLAB-D/IEEE 13
     cases). The three failures in the committed `docs/Test_Report.md` do
     not reproduce on the Task 0 engine: they are healed by the engine's
     equipment-identity fix, matching `docs/Red_Team_Record.md:144-155`.
   - `ruff check`: exit 0.
   - `build_bundle.py`: exit 1 (expected while the base skips remain); no
     MISSING file; three UNTRACKED files (`docs/Red_Team_Record.md`,
     `tools/check_losses.py`, `tools/race_gridlabd.py`), which CN-0
     registers; test-count line `test cases: 329 passed, 0 failed,
     7 skipped`; contract-drift check PASS.
   - Paths record (the module function is `calibration.paths.summary()`,
     not a `paths.summary()` method): `hares_repo`
     `/home/rich/src/HARES/.workstrees/1d60f478`, `engine_sha256`
     `28be65c0835f3622`, 59 cached buildings, 3144 TMY3 EPWs,
     ResStock cache `~/.cache/ochre_next/resstock/2025.1`.
   - The three generated files `build_bundle.py` rewrites (`INDEX.txt`,
     `MANIFEST.txt`, `SETUP.txt`) were restored to their committed bytes
     with `git checkout c71c542 -- <file>` after the run; no other
     tracked file changed. From CN-17 (row 16a) on, the gate's zip goes
     to `scratch/i08/<sha>/bundle.zip` through `--out`, so no file beside
     either repository is written or needs restoring.

### B0, 2026-09-30, consumer `aec2e3f` (i08-consumer), engine `1d60f478`

Files in `baselines/`: `B0-de-trace.json` (sha256
`5e230942e9efbc7f2addba8f922921dfe60c2fc913f56d6497665b8971de7581`),
`B0-losses.json` (sha256
`3c2dadc8e8533d4efa4d320e6fa6537b4185256996ed6179545d87d93b14cc4f`),
`B0-record.json`. Engine build sha256 `28be65c0835f3622` (the Task 0 head
worktree). `uv.lock` sha256
`faa299a041f10b62e657b985339fc3708a6f75cd339347ee91ba1678b7dc3c67`.

- DE-trace capture: 4 premises, 174 s wall (69 m 42 s user across the fit's
  workers), peak RSS 6.2 GB. Captured three times (twice at consumer
  `a6d1345`, once at `aec2e3f` after the capture-tool provenance fix);
  `compare` exit 0 across all three: the production set is deterministic
  across processes. The first two captures' headers named the submodule pin
  instead of the engine under test; their premises payloads are bitwise
  identical to the committed capture's, and the committed baseline is the
  third.
- The 13 losses of `tools/check_losses.py`, as `float.hex` in
  `B0-losses.json` (keys `hvac:truth_bldg`); `check_losses.py` re-run at the
  consumer head reports all 13 IDENTICAL (exit 0).
- Gate at the consumer head: `332 passed, 0 failed, 7 skipped, 339 total`;
  the failing set is empty; the skip set is the seven environmental base
  skips. `ruff check` exit 0.

### B0 check, 2026-09-30, rows 1 to 3 (BR-1, BR-3, BR-2, engine `aa22be6c`..`fd23dd80`)

`capture_de_trace.py capture` on the consumer after the three engine rows:
`compare` against `B0-de-trace.json` prints `identical`, exit 0. The three
rows touch only observe-gated code, tests, an example and lint sites, so the
release engine the consumer runs is unchanged; the capture proves it.

### OR-5, 2026-09-30, engine `469ea955` (before: `fd23dd80`)

The cost of the now-unconditional invariant checks. `perf record` is
unavailable on this host (WSL2 exposes no PMU), so the measurement is
wall-plus-phase before and after: the profiling example
(`cargo run --release -p hares-core --features profiling --example
profile_sim -- 45 3`, three 45-day runs at 900 s per repetition set, fresh
construction each run), two rep sets per side, built at each commit in its
own worktree. Before: wall median 829.9 ms per set, `other` phase (the
bucket that absorbs the checks) 199.1 ms/run. After: wall median 860.1 ms,
`other` 203.5 ms/run. The wall difference (3.6%) is under the 5% noise
band: no measurable change at this resolution. The unconditional checks'
share of `simulate()` is bounded above by roughly half a percent of wall.

### M-2, 2026-09-30, commit `7643b4e7`

Committed files and sizes: six manifests (430-995 B each) and six
`.golden.json` files (25 286 B to 1 067 995 B, the 35 040-row fixtures
largest) plus the replacement defaults file
`consumer_shape_envelope_materials.csv` (198 549 B, 1 520 data rows, only
the two scaled columns' bytes differ, values at 10 significant digits).
Local full frames under `target/golden/`: 0.27 to 3.5 MB per fixture. No
Parquet file is committed. Capture wall times: 0.3 to 1.7 s per fixture,
10.3 s for the dst fixture including its recompile. Row counts: 8 760 /
35 040 / 480 / 384 / 35 040, fleet aggregate pinned at 0 rows (the
mixed-offset column-mismatch branch), fleet homes and weights 3 rows
each. `compare-all` passes in both feature sets; the captures were
re-run from a different working directory and reproduced byte-identical
files; a seed-1 tamper and a defaults-replacement-removed variant both
fail `compare` naming the differing columns.

### BR-6a, 2026-10-01, commit `39488581`

Bitwise-neutral, no delta to record: `compare-all` identical in both
feature sets on every M-2 fixture. The refactor raised the workspace
`rust-version` to 1.89 and collapsed 339 nested `if` chains into let
chains across 79 files (2426 insertions, 2468 deletions, net -42
lines). Red-team verification beyond the acceptance gates: every
aligned pure-reindent line was classified, and the eight multi-line
string continuations whose in-string whitespace the mechanical dedent
had altered (seven diagnostic messages in hares-envelope, hares-io,
hares-types) were restored to their pre-refactor bytes, so even the
log-message content is unchanged; the eight pre-existing em dashes on
reindented comment and message lines were rewritten per SL-3; the two
docs stating the old MSRV (README, docs/python.md) were updated.

### BR-6b, 2026-10-01, commits `f880a6d7` + `872cf289`

Numerically inert: `delta` on every M-2 fixture (and the dst fixture)
reports no differences against the committed goldens, so no fixture was
re-captured and no delta columns exist to tabulate. The dependency set:
pyo3/pyo3-build-config 0.29.3, numpy 0.29.0, arrow and parquet 59.3.0
(60 withheld: pyo3-arrow 0.19 needs 59), nalgebra 0.35.0, quick-xml
0.42.0, tzf-rs 2.1.2, criterion 0.8.2, sha2 0.11.0, strum 0.28.0, syn
3.0.6 (dev), postcard 1.1.3 with default features off (`alloc`,
`use-crc` kept), `cargo update` patches for the rest; `heapless`,
`atomic-polyfill` and `paste` are out of the lockfile and `[advisories]`
is green with no ignores (six RUSTSEC advisories cleared). The upgrade
row also found a pre-existing frame_golden defect: a zero-row frame
product (the fleet aggregate) could not be read back for `delta`
("cannot digest an empty batch list"); fixed in `f880a6d7` by keeping
the schema as one empty batch, analytically and live verified digest
identical (the committed zero-row digest is SHA256 of empty).

Physics performance and memory, before `39488581` vs after `872cf289`
(the OR-5 method: profiling example, three 45-day runs at 900 s per
repetition set, two warm rep sets per side, warm-run max RSS): wall
874.0 ms to 871.3 ms per set (-0.32%), `other` phase 206.4 to 205.6
ms/run (-0.39%), both inside the 5% noise band: no change. Peak RSS
77.69 MB to 72.58 MB (-6.6%), reproducible within 0.3% per side and
outside the band in the favorable direction: the upgrade reduced the
simulation's peak memory by about 5 MB per process.

Notes for later rows: quick-xml 0.42 normalizes attribute whitespace
and text EOLs where 0.39 did not (XML 1.0 spec-correct; no committed
fixture contains entities or control whitespace in attributes, so no
golden moves); the tzf-rs data release moved (2026-b-fix1 to
2026-d-fix1) and every golden fixture pins its UTC offset explicitly,
so the tzf path is covered only by the four site_location unit tests:
its integration coverage gap is pre-existing and unchanged. Both notes
are closed by `11-other-defects.md` OD-21 (row 24h).

### BR-6c, 2026-10-01, engine commit `75a182a0` (pyproject.toml + uv.lock)

Every Python dependency's lower bound raised to the latest stable
release and `uv lock --upgrade`: pandas 2.3.3 to 3.0.6 (cap removed),
numba 0.68.0, pvlib 0.16.1, fastparquet 2026.9.0, boto3 1.43.107,
pytz 2026.4, pybamm 26.9.0.0, marimo 0.25.1, plotly 7.1.0, ruff 0.16.10,
NREL-PySAM 7.1.1.post1; maturin stays 1.15.0 with its `<2` cap removed;
32 lock moves, all upgrades, no pre-releases, clean-room
`uv sync --locked` verified. The suite: 1377 passed, 9 skipped, skip
census byte-identical.

Two holds, both evidence-backed: pytest-timeout stays 2.4.0 (2.5.0 is
yanked upstream), and **pint stays capped `<0.25`** (at `>=0.24.4`): the
red team measured pint 0.25.0 removing `inch_H2O_39F` from the default
registry, which the vendored OCHRE infiltration model converts with at
five sites (0.24.4 converts fine, 0.25.2 and 0.26.1 raise
UndefinedUnitError at Dwelling init); with the cap restored the
vendor-exercising tests pass (ochre parity 2 passed, bestest 2 passed).
pandas 3.0.6 held: forced runs of the pandas-touching tests pass, and
the corruption probes (CoW chained assignment, string-dtype round
trips, float equality) found no silent-comparison risk in the tests
that exist.

Process finding recorded for the block's future rows: the default
pytest selection (`-m 'not slow and not ochre'` addopts) never executes
the pandas/pint/pvlib code paths, so a green `-m ""` run cannot support
a "held under the suite" claim for those packages on its own; the
census's three "OCHRE not installed" module skips hide the interop
surface. The red team's forced-run evidence is what validated this row.

### M-3, 2026-10-01, the baseline (engine `872cf289` + BR-6c staged, Python-side only)

Raw record: `.shipwright/runs/v2-implement-7dd5a2a0-6727-4d5a-b578-2fc917150b9a/m3-baseline.md`
(every command line, per-set spreads, and load averages; all five sets
valid under the 5% noise rule).

1. `simulate()` wall for the full-year `consumer_shape_900s` manifest
   (5 fresh constructions per set, medians): in memory 1.530 s, file
   written 1.509 s, nothing kept 1.409 s. The write-and-retain overhead
   is roughly 8% of wall; discarding output saves roughly 8%.
2. `perf` is unavailable on this host (WSL2, no PMU; the software-event
   invocation failed verbatim, recorded). Substitute: the profiling
   example's phase breakdown (three 45-day runs at 900 s): envelope
   solve 8.8%, schedule load 4.5%, hvac 3.3%, `other` 71.3% - the
   bucket holding `record_step`, `update_in_place`,
   `prepare_inputs_inner`, the per-step telemetry clones and
   `check_invariants`. Symbol map for the plan's named sites (the
   plan's line numbers have shifted): telemetry clone at
   `run_timestep` (dwelling/mod.rs:6624; second site :4981),
   `record_step` :6794, `update_in_place` environment.rs:524,
   `prepare_inputs_inner` stepping.rs:668,
   `solve_ideal_capacity_for_target` stepping.rs:366, envelope entry
   `ThermalSolver::integrate`, `check_invariants` dwelling/mod.rs:7176.
   This is the profile the output and telemetry sink rows attack.
3. Retained frame bytes, full-year in-memory run: 15,340,640 bytes
   (about 14.6 MiB), identical across all repetitions.
4. Python `results()` export time: median 0.0081 s for the 35,040-row
   frame (five timed calls, spread under 3%).
5. Memory growth (VmRSS): repeated construct-and-simulate, 500
   repetitions at one day: +724 kB total (flat); long horizon, 10^3
   steps vs 10^6 steps: +111,352 kB over 999,000 extra steps (about
   0.11 kB per step, the retained telemetry); stepped episodes, 100
   load-state-and-96-step cycles: +8 kB (flat).

Verification: item 1's first set repeated; new median 1.489 s (spread
4.30%), inside the first set's band: no measurable change.

### M-3, 2026-10-02, re-measured at `8b09ac4a` (the valid baseline)

Raw record: `.shipwright/runs/v2-implement-7dd5a2a0-6727-4d5a-b578-2fc917150b9a/m3-baseline-post-br11.md`.

Wall medians (5 fresh constructions per set, all sets valid): in memory
1.533 s, file written 1.559 s, nothing kept 1.433 s. Phase shares:
envelope solve 9.1%, schedule load 4.7%, hvac 3.5%, `other` 70.3%
(the bucket holding `record_step`, the per-step telemetry clones,
`update_in_place`, `prepare_inputs_inner`, `check_invariants`),
unattributed 12.4%. Retained frame bytes for the full-year in-memory
run: 15,340,640 bytes. Python `results()` export: median 0.0088 s
(stability re-run 0.0079 s). Memory growth (VmRSS): 500
construct-and-simulate repetitions +1,004 kB (flat); horizon 10^3 to
10^6 steps +111,328 kB (about 0.11 kB per step of retained telemetry);
100 stepped episodes +8 kB (flat). Verification repeat: median 1.537 s
(spread 3.58%), inside the first set's band.

Against the first (pre-BR-11) record: every quantity within the noise
band; removing the two-day warm-up moved no measured number beyond 5%.
The remaining baseline facts this ledger's before/after rows cite:
`other` is about 70% of `simulate()` wall, retained frames are about
14.6 MiB for a full year at 900 s, telemetry retention costs about
0.11 kB per step, and the per-episode loop leaks nothing.

### BR-11, 2026-10-02, commit `8b09ac4a` (the consumer-shape fixture correction)

The golden tool mapped every manifest's `initialization_duration_s` to a
warm-up regardless of value; `0` now maps to none, matching the Python
binding. `consumer_shape_900s` (the manifest that mirrors the consumer's
no-warm-up call) was re-captured by the materialize/delta/capture
procedure. The delta against the old golden: `health.warmup` from
`Ran {converged: true, days_run: 2, ...}` to `Disabled`, and the full
year's numbers moved because the run no longer starts from a two-day
converged state: first-row indoor 19.20 C to 18.34 C (cold start,
heating already active), Total Gas Power row 0 from 0.0074 to 0.1370
therms/hour, 35040 differing cells on the thermally coupled columns and
the stochastic event loads, `Time`/weather/ground-temperature and the
deterministic schedule loads byte-identical, `defaults_digest`
unchanged. Metrics deltas are peak and total shifts of the same order
(peak import 3.0448 to 3.0468 kW; gross consumption 8842.86 to
8843.67 kWh over the year). The other five goldens are byte-unchanged:
the engine ignores the magnitude (a warm-up's replay length is fixed at
0.5-day chunks converged over 25 iterations, `run_warmup_converged`),
so the five manifests' move to 604800 is nominal. The re-captured
golden verifies deterministically (materialize twice, both exit 0). B0
after the commit: identical (no engine change). M-3 was first measured
against the old warm-up shape and is re-measured at this commit; the
valid baseline is the record above.

### OR-11, 2026-10-02, commit `b8195c83` (the check_invariants classification)

Bitwise neutrality held through the entire row: every golden fixture
identical in both feature sets after every crate's worth of ungating,
and the consumer's B0 de-trace identical at the commit. Release
performance held by same-session A/B against a pristine-parent
worktree: 1.441 s to 1.438 s median on `consumer_shape_900s` (-0.2%),
so the roughly 400 checks that became unconditional in release (over
the restructured hot paths: assembly-resolved zone index lists,
precomputed pump node lists, construction-time port wiring and ZIP-sum
validation) cost nothing measurable. The release suite (where those
checks never ran before) passes 7086.

Found along the way, both pre-existing corruption classes: the master
`defaults/envelope/Envelope Materials.csv` had been overwritten by the
golden fixture's scaled copy (R x1.3, C x1.5; the scaling belongs only
to the committed `consumer_shape_envelope_materials.csv` replacement),
restored to HEAD's bytes; and the boilers' init-time port write
(`ports[3]`) landed on the thermal port, not the fluid port, because
`rebuild_thermal_ports` pushes thermal ports last, so the configured
fluid type never reached the declaration (fixed with a by-type port
lookup). The tool's defaults-tree overlay follows symlinks and needed
the same link-breaking the frame_golden capture path already does.

### SL-7, 2026-10-02, commit `7b7a6a7d` (the dead coupled-LU path removed)

Net -592 lines of unreachable code; bitwise identical goldens in all
three buildable profiles (default, dst, profiling) and the consumer's
B0 identical. The red team established the acceptance's strength: the
golden digest hashes f64 to_bits (raw IEEE bytes), and nalgebra's
LU-of-identity solve demonstrably flips -0.0 to +0.0, so the deleted
path's only arithmetic effect (a zero's sign) was exactly within the
goldens' detection power; identical digests mean even that residual
did not occur on any fixture. The identity solves' replacements are
pass-throughs in identical term order; verify_stability takes N's
eigenvalues (the discarded verdict's sole caller is a test); the five
oracle tests keep their LU oracle as test-local factorizations of
I + D (18/18 release repeats, no flake).

### RT-3, 2026-10-02, commit `44da330c` (the thermal solver, allocation-free)

The step and the ideal-capacity solve lost every per-step allocation:
the exterior-temperature save/restore through a construction-sized
buffer, the latent map round-tripped to `latent_buf`, `last_u` and the
output vectors sized at construction (`output_into`), both scalar
solves over a solver-owned `SolveScratch` (same `mul_to` kernels, same
operand order), and the solar slot map retained unless the surface-id
sequence changes. The stop-and-add rule fired once: the zero-allocation
test first caught three MORE per-step allocation sites the change list
did not name (`build_input_vector`'s diag clone and the two
capacity-resetting buffer round-trips), all removed the same way.
Acceptance: the bracket around prepare, the per-zone ideal-capacity
solves and integrate reads zero allocations over 100 steps after the
first, with couplings active and without, plus 100 `resolve` calls;
the solar slot map rebuilds only on surface change; goldens bitwise
identical in all three profiles and the consumer's B0 identical. Wall
unchanged (A/B against a pristine-parent worktree: 1.112 s vs 1.099 s
median, about 1% faster); hot-path allocations for a full
`consumer_shape_900s` year fell from 8,473,313 to 7,957,954
(-515,359). The stop rule fired: the profiled `ideal_capacity` share
is 6% of `step_total` (at the 5% band), so the new entry RT-3a (one
shared-term resolve per step) is scheduled as row 21a. The
construction-time divisor check the spec required (every `1 + d_i`
finite and positive, typed error naming the state) landed with its
red-green test, extended to the TARP injection's tilt (the one input
the per-step film coefficient recomputes).

### RT-3a, 2026-10-02, commit `69950a98` (the shared-term resolve)

The per-equipment ideal-capacity solves of one step now share the
`N·x`, `B_eff·u` and rhs prefix (the first solve of a step fills it;
every `x`/`last_u` mutation invalidates it: prepare, integrate,
restore_state, each mutation-verified); later calls run only the
target-dependent tail into a separate scratch region. On
`consumer_shape_900s` the fixture's 35,040 steps run 23,072 single
solves and 11,968 double solves, so about a third of the prefixes were
being recomputed; the profiled `ideal_capacity` share went 6% to 5%
(the printout's precision cannot distinguish 4.5% from 5.4%: the share
sits at the band's edge, and the next measurement cycle pins it), the
wall is unchanged within noise (A/B: 1.341 s to 1.337 s median), the
full-year allocation count is bit-identical (7,957,954), the goldens
are bitwise identical in both feature sets and the consumer's B0 is
identical. The saving scales with equipment-per-shared-state (E·R);
the fleet-scale fixtures with several equipment per zone are where it
pays.

### OR-18, 2026-10-02 (the audit's stopped rows closed; one numeric row)

Commit 2's physics fix (water heaters in HPXML locations with no
modeled zone read the OS-HPXML ambient series instead of exchanging
heat with the conditioned zone) moves the fleet golden's home 1
(ResStock bldg0000004, "other heated space"): gross consumption
201.989 to 204.911 kWh (+2.92), water heating 25.109 to 25.326 kWh,
HVAC heating 87.253 to 89.957 kWh, peak import 5.6056 to 5.6039 kW;
homes 0 and 2 unchanged; every other fixture (and the dst fixture)
bitwise unchanged; the fleet golden re-captured in the landing commit.
The direction is the physics working: the tank's standing losses no
longer dump into the conditioned zone, so the heating end use carries
them. The review pass also fixed the "other housing unit" read to use
the conditioned zone resolved by type instead of the zone list's first
entry (at `1c18543a` the field was `ambient_other_space_c.
conditioned_zone_c`; OR-18's fixes replaced it with the per-placement
ambient air series, `hares-core/src/ambient_air.rs`), and the
fixture correction for `window_area_deducted_from_host_wall`'s
total-area assertion (10.0 to 20.0 with its new floor) is recorded.

### Measurement and physics suites (independent)

Worktrees `.workstrees/audit-b8195c83` (OR-11, SL-7's parent) and
`.workstrees/audit-7b7a6a7d` (SL-7), both built
`cargo build --release -p frame_golden -F profiling`. `perf` is not usable
on this host (`perf not found for kernel 6.18.33.2-microsoft`, the WSL2
kernel has no matching perf build), so the symbol-level share of
`step_into` and `solve_for_output_input` that M-3 item 2 would give could
not be measured directly. The finest available substitute is the
profiling feature's `envelope` phase: both functions are called only from
inside `ThermalSolver`'s stepping functions, which run entirely inside the
`Envelope` phase span of `run_timestep` (`phase_clock.enter(...,
ProfilePhase::Envelope)`, `dwelling/mod.rs:6552`, unchanged by SL-7's diff
in both commits), so `envelope`'s share bounds the two functions' combined
share from above.

Ten runs of `cargo run --release -p frame_golden -- run
tests/fixtures/golden/consumer_shape_900s.toml --repeat 1 --discard-output`,
interleaved parent/SL-7/parent/SL-7 (A = SL-7 `7b7a6a7d`, B = parent
`b8195c83`), five of each, in one session:

| run | commit | simulate() s | envelope share |
|---|---|---|---|
| 1 | B (parent) | 1.439 | 36% |
| 2 | A (SL-7) | 1.347 | 37% |
| 3 | B (parent) | 1.254 | 38% |
| 4 | A (SL-7) | 1.421 | 36% |
| 5 | B (parent) | 1.293 | 38% |
| 6 | A (SL-7) | 1.272 | 38% |
| 7 | B (parent) | 1.470 | 37% |
| 8 | A (SL-7) | 1.374 | 38% |
| 9 | B (parent) | 1.352 | 38% |
| 10 | A (SL-7) | 1.516 | 37% |

Median simulate(): parent 1.352 s, SL-7 1.374 s (+1.6%). Median envelope
share: parent 38%, SL-7 37% (-1 point). Spread (max-min over median) of
each side's five runs: parent 16.0%, SL-7 17.8%. `uptime` at the start of
the session read load average 104 to 145 (another session's cargo builds
on the same host); by the time these ten runs ran it had settled to 18 to
47 and kept falling across the run, a one-directional drift riding under
the interleaving. The noise floor here (16 to 18%) is well above M-3's 5%
band and above the 1.6% before/after difference, so this is no measurable
change, consistent with the change being dead-code removal only: the
model's `m_is_identity` was always true at runtime before SL-7, so the
identity branch SL-7 kept was already the only branch ever executed; SL-7
removed code that never ran, not code on the measured path.

Physics suites, run at both commits:

| suite | command | `7b7a6a7d` (SL-7) | `b8195c83` (OR-11) |
|---|---|---|---|
| ASHRAE RC reference | `bash scripts/check_ashrae_reference.sh` | PASS | PASS |
| hares-core oracles | `cargo nextest run -p hares-core --release --no-fail-fast --test bestest --test parity --test conditioned_oracle --test freefloat_oracle --test structural_envelope_oracle --test envelope_oracle` | 25/25 passed | 25/25 passed |
| hares-physics | `cargo nextest run -p hares-physics --release --no-fail-fast --test physics_validation_tests --test solar_parity` | 53/53 passed | 53/53 passed |
| frame_golden compare-all, default | `cargo run --release -p frame_golden -- compare-all` | 5/5 identical | 5/5 identical |
| frame_golden compare-all, `-F dst` | `cargo run --release -p frame_golden -F dst -- compare-all` | 1/1 identical | 1/1 identical |
| frame_golden compare-all, `-F profiling` | `cargo run --release -p frame_golden -F profiling -- compare-all` | 5/5 identical | 5/5 identical |
| Python OCHRE parity / thermal trace | `uv run --all-extras --all-groups --no-sync pytest -m "" -rxX tests/python/test_ochre_parity.py tests/python/test_thermal_trace.py` | 2 skipped, 0 run | 2 skipped, 0 run |

The `hares-core` line starts 6 binaries but only 25 tests exist across
them; `conditioned_oracle` and `freefloat_oracle` each contribute 0 tests
in this tree, at both commits, so the 25/25 is the full count, not a
truncation. The two Python suites skip at both commits:
`test_ochre_parity.py:190` and `test_thermal_trace.py:45` both call
`pytest.importorskip("ochre", ...)`, and neither worktree's `.venv` has
the OCHRE package or the maturin-built `hares_python` extension installed
(their docstrings call for `uv pip install -e vendors/OCHRE` and `uv run
maturin develop`). Running either install step here was refused by the
permission system as a modification to a shared resource, so it was left
undone rather than attempted by another route; this is a gap in this
record's coverage, not a suite failure, and the suites have not verified
anything at either commit. `uv sync --extra ochre` (needed only for
`gen_ashrae_reference.py`'s `xmltodict` import) resolved from the local
cache and is not the same install.

### PR-1, 2026-10-02, commit `d7f0e381` (the profiled step partitioned)

The M-3 phase shares (profiling example, three 45-day runs at 900 s)
and the new phase clock's shares for `consumer_shape_900s` (one full
year at 900 s, `frame_golden run -F profiling`), side by side; the
first table's rows are M-3's coarse phases, the second's are PR-1's
eleven, so the two do not map one to one - the side-by-side is the
coarse-to-fine reference for the rows that follow:

| M-3 phase | share | | PR-1 phase | share |
|---|---|---|---|---|
| envelope_solve | 9.1% | | environment | 4% |
| schedule_load | 4.7% | | control | 3% |
| hvac | 3.5% | | ideal_capacity | 2% |
| other | 70.3% | | actors | 0% |
| (absorbed) | | | dispatch | 15% |
| | | | equipment | 20% |
| | | | envelope | 13% |
| | | | invariants | 16% |
| | | | state_snapshot | 8% |
| | | | output | 2% |
| | | | accounting | 16% |

(The old `schedule_load` named a fraction of what the new `control`
and `dispatch` phases split; the old `other` residual dissolved into
`invariants`, `state_snapshot`, `output` and `accounting`. The four
per-step HWM reads PR-2 removes still ran inside these spans.)

Instrumentation neutrality: `compare-all` identical in both feature
sets; the release wall is unchanged (A/B against a pristine-HEAD
worktree: 1.19 s both sides, three runs each). Host note: the M-3
session recorded 1.53 s for the same command and today's re-runs of
the same binary record 1.19 s - the absolute wall numbers drift with
host conditions between sessions; only same-session A/B comparisons
are noise-safe. The acceptance test (the eleven phases sum to
`step_total` in exact `Duration` arithmetic) passed three consecutive
runs; `examples/profile_sim.rs` is deleted and `frame_golden run -F
profiling` is the only profiling driver.

### PR-2, 2026-10-02, commit `21d90588` (one HWM read per summary)

The profiling build's own overhead was mostly the four per-step
`/proc/self/status` read-and-parses. Same-session A/B on
`consumer_shape_900s` (three runs per side, medians): profiled
`step_total` 3.36 s at the parent (`d7f0e381`, pristine worktree)
against 1.20 s at this commit, a 2.8x drop; profiled wall over
unprofiled wall went from about 2.75x to 0.98x: profiling now costs
roughly nothing over the release run. The red team's independent
parent-side re-measure matched (3.36 s) and observed the HWM
monotonically rising across repetitions within one process (147,496 to
179,788 kB over three reps), confirming the per-summary read is live,
not stale. `compare-all` identical in both feature sets; frame bytes
unchanged (15,340,640).

### PR-3, 2026-10-02, commit `26626710` (the per-thread allocation counter)

The shared counting allocator replaces three local copies and a stub;
`hot_path_alloc_violations` became `Option<u64>` alongside a new
`hot_path_allocations: Option<u64>`, both `None` without the allocator.
First real counts, `consumer_shape_900s` (one full year at 900 s,
`frame_golden run -F profiling`): 8,473,313 allocations across 35,040
steps, every step violating (about 242 allocations per step: the
telemetry map clones and recorder growth the runtime-memory rows
RT-1 and RT-3 will eliminate). The red team's correction, landed
before the commit: a clean (zero-delta) run previously reported
`violations` as `None` ("unavailable") instead of a measured
`Some(0)`; the counter now records both fields on every measured run.
Count exactness proven in debug AND release (7/5/1000 deltas exact
across repeated runs, `--release` included); the per-thread isolation
(barrier test) proves a process-global counter would have been wrong;
release exactness matters because the later allocation-free assertions
run in their test binaries' own profiles. The bench's parallel group
now takes its allocation figure from a labelled sequential counting
pass outside the timed loop; the timed loop measures identical work to
pre-PR-3 (fresh dwellings per iteration). The default release binary
keeps the system allocator (the cfg-gated install lives in the tool's
library under `-F profiling` only). `compare-all` identical in both
feature sets.

Phase-attribution note for the record: PR-2's removal of the four
per-step HWM reads is visible in the profiled shares - the
`invariants` and `accounting` phases, which had absorbed the reads
(16% and 16% in the PR-1 run), now read 1% and 1% on this single-rep
run; single-rep phase shares swing with conditions, the partition
itself is exact.

### OD-16, 2026-10-02, engine `fe804b59` (the controlled-run reconciliation)

No measured number moved: the goldens are bitwise identical and the
consumer's B0 de-trace identical (no baseline fixture runs actuate).
The four ami2 cases that OR-11's enforcement made loud (the release
signal's zeroed hysteresis degenerating the EBM, and the tolerated
failure's stale power in the consistency sums) now pass against a
reconciled trace; the full consumer gate is 336 passed / 0 failed /
7 precondition skips. The fix adds no allocation and no per-step work:
the consistency checks' `step_succeeded` filter is a boolean test per
equipment, and the release-form pass-through removes the hysteresis
write the old path did per signal.

### BR-13, 2026-10-02, commit `32044ab5` (the defaults digest checked)

No measured number changed: the digest check adds one tree hash per
compare (a few milliseconds over the defaults tree), the committed
fixtures all pass with their recorded digests, and the negative probe
(one byte changed in `zip_parameters.toml`) fails all six fixtures
naming both digests with exactly one difference each. The zero-row
read-back pin (the digest of a schema-only parquet equals the empty
input's SHA-256) is committed with the row.

### BR-14, 2026-10-02, commit `c6889589` (strict tariffs + the billing fixture)

The new `month_boundary_billing_3600s` golden: 288 frame rows (12 days
at 3600 s) plus ONE billing row, the anniversary-based Monthly cycle's
single period (Jan 25 to Feb 6): energy 53.137119 USD (354.247461 kWh
at 0.15 USD/kWh), fixed 10.00 USD, net 63.137119 USD, peak 5.689487 kW;
`demand_charge_usd` is pinned to the positive zero: an empty rate
list's `sum()` returns the reduction initialization's sign, and on
rustc 1.94 -O that is -0.0 (proven with a standalone probe) - the
evaluator adds +0.0, so the bits are `0x0` and must not depend on the
toolchain (BR-20's compiler upgrade would have flipped them and failed
compare). The golden is 27,300 B; `flat_tariff.json` is 614 B. The
existing five goldens bitwise unchanged; the dst fixture identical;
the pytest census unchanged.

### RT-4, 2026-10-12, commit `2434deb8` (the shared parses)

The fleet construction's wall time at 1000 homes: the measurement test
`fleet_construction_wall_time` (ignored), 1000 homes in the ResStock
shape (a unique HPXML and schedule file per home; one weather file and
one defaults tree shared), `SteppableFleet::from_configs` at
`n_threads = 0` (the global pool), the canonical release build.
Before (two runs at the parent): 38.59 s, 37.20 s. After (three runs at
the commit): 31.07 s, 31.09 s, 32.14 s. About 17% less; the duplicate
weather parse, the duplicate defaults load and the duplicate profiles
load per home are the parse work the shared handles remove, and the
home's own HPXML parse, schedule parse, equipment resolve and the
dwelling build remain.

The parse counts, the memory proof's instrument (`hares_io::parse_counts`,
the doc-hidden counters; the tests
`many_home_construction_parses_each_unique_input_once` and
`streaming_simulate_parses_each_unique_input_once`): three homes sharing
one HPXML file, one weather file and one defaults tree, both the
steppable construction and the streaming simulate, parse hpxml 1,
weather 1, defaults 1, profiles 1. The parent parses each three times
(RED: the counter test's first run read weather 3 for 3 homes).

The fit path's per-candidate cost (the measurement test `fit_path_cost`
(ignored), 50 candidates over the consumer_shape_900s inputs, the
candidate's own HPXML parse plus the build from the shared weather,
schedule and defaults handles, against a full parse of every input per
candidate): the batch's shared parses 6.42 ms once, then 3.20 ms per
candidate from the shared handles against 11.01 ms per candidate with
the full parse (about 3.4x less per candidate).

Re-measured at the review round's Rust tip `862660b3` with the
committed harness, unchanged since the row's commit, in the canonical
release build (`cargo test --release -p hares-core --test
fit_path_cost -- --ignored --nocapture`, the pinned toolchain rustc
1.99.0): five runs, load average 8.25 to 8.44 (1 min), every run within
5% of its median, the shared parses 6.59 ms once, then 3.58 ms per
candidate from the shared handles against 11.89 ms per candidate with
the full parse, 3.3x less per candidate. The recorded numbers above
reproduce. The same test in the debug profile prints 145.9 ms and
183.7 ms per candidate (1.3x): the fit path's ratio is a release
number, and a debug run's wall is not comparable to the record.

M-3 item 5's three memory-growth readings repeated at the commit (the
same commands, `--discard-output`, VmRSS from the run's own report):
500 construct-and-simulate repetitions (1-day runs) 49,112 kB at rep 1,
52,284 kB at rep 100, 52,284 kB at rep 500 (the file and allocator
caches warm by rep 100, flat 100 to 500); the long horizon 49,124 kB at
10^3 steps against 49,044 kB at 10^6 steps (flat; the parent's own
readings at this head are flat too: 42,640 to 43,292 kB, the retention
the M-3 record held on 2026-10-02 having been removed by the rows since);
100 stepped episodes (the consumer_shape_900s dwelling, `reports=[]`,
`save_state` then 100 times `load_state` plus 96 steps) 112,860 kB at
episode 1 against 112,872 kB at episode 100 (+12 kB, flat).

### BR-15, 2026-10-02, commit `50a5c261` (fleet weights + resolution stated)

The fleet golden is byte-unchanged: explicit `weight = 1.0` is the
exact default (`x * 1.0` is bit-exact in IEEE, and aggregation is
unconditionally weighted), and `resolution = "hourly"` is what the
deleted step-derived rule computed for a 3600 s step. The weights
product plumbing is pinned end to end by the new two-home test (1.0
and 3.0 read back in home order through `with_sample_weights`); the
invalid-weight rejection (NaN, negative) fails at the run boundary
naming the bldg_id.

### BR-10, 2026-10-02, commit `4a1f2c19` (CI on the initiative's branches)

No runtime number: the workflow's every cargo line was executed
locally exactly as written (7168 default, 7181 dst, 7174 profiling,
7227 observe, 7125 release, 2600 equipment-release, 1348
core-observe-profiling, all exit 0) leaving 7 distinct junit reports;
the golden lines ran on the capture toolchain (1.94.1, ubuntu-24.04's
platform) with all fixtures identical. The CI run itself is checked at
the closing sequence's first push (row 99a).

### OD-15, 2026-10-02, commit `3ff4583a` (the January cooling pin)

No numeric change: the 3.45 kWh of `hvac_cooling` is the crankcase
heater to the last Wh (69 of 72 steps below the 12.78 °C threshold each
draw exactly 0.05 kW; the three warm-afternoon steps draw nothing;
`hvac_cooling` equals the crankcase energy within 1e-9). HARES's run
condition and booking match EnergyPlus (`DXCoils.cc`) and OCHRE
(`HVAC.py`) alike; the pin test binds the composition. The entry's
prose error (the fixture is Idaho, station G1600270, not "Connecticut
G0900090") is corrected in the ledger.

### OR-18b, 2026-10-02, folded `f719895b` + `b7ea5d0a` + `b3807202` (the wet-bulb, the fallback removals, the OS-HPXML defaulting)

The fix-or18 branch's three commits cherry-picked onto the branch after
an independent review (conflict forecast clean) and red team (22/22
ambient and wet-bulb pins; 7192 debug + 7149 release in the branch
worktree). The measured corrections the ledger's OR-18b required: an
ambient HPWH's COP input is now the placement's WET-bulb (the scratch
repro's zone-resolved 2.130 vs the dry-bulb error 2.555 is corrected to
the placement's thermodynamic wet-bulb; the blended placements compute
it from the blended humidity ratio with ASHRAE ch.1's adiabatic rule,
the super-saturated blend clamping to the dry-bulb); the last silent
`ZoneId(1)`/`21.0` fallbacks are typed errors (the audit's 18 sites);
ambient water heaters declare no thermal port and join after stepping
(the frozen-table defect); the synthetic closure counts Slab as Floor
and FoundationWall as Wall with positive area; a second conditioned
zone is a construction error; the HPWH wall share default is explicit
from parse (0.5 only for locations resolving to the conditioned zone,
0.0 otherwise: the kept behavior change m4 recorded). The fleet golden
was re-captured from a clean committed tree (provenance-only delta);
the DE trace compares identical and every golden fixture is bitwise
unchanged at the fold head (7214 tests debug, goldens 6/6 + dst).

### BR-12, 2026-10-02, commits `6646c801` + `aab05da0` (the never-compiled tests)

The archaeology at `b674a767^`: twelve then-ignored tests ran, nine
passed, three FAILED - the three alignment-oracle helpers whose parquet
read-back the pre-BR-7 `write_output = false` default silenced; the
defect was `b674a767`'s own subject and the helpers no longer exist at
HEAD. The shadowed file's `debug_bldg0000002_winter_heating` ran PASS
under a temporary shadow target with `-F observe`. The four seasonal
tests ran for the first time and pass: 2025.1 summer `hvac_cooling`
23.886 kWh, 2024.2 summer 43.629 kWh, 2025.1 winter `hvac_heating`
7.615 kWh + gas 11.884 therms, 2024.2 winter 2.370 kWh + gas 3.830
therms. Process notes, recorded honestly: the first commit's pipeline
into `tail` masked two failing smoke tests and the commit landed with
them red (the pipe's exit code, not nextest's); the failure was a newly
added bound asserting `Total Electric Power >= 0`, which is wrong
physics - the column is the NET of load minus PV generation and the
two smoke homes export; the corrected bound is per end use (loads
non-negative, the PV end use non-positive, the net unbounded), and
`aab05da0` fixes it forward. The lesson recorded: a pipeline into
`tail` in a verification chain masks the failure exit; verification
commands run bare or with `set -o pipefail`.

### fix-rt9's fold, 2026-10-02, commits `89eae0ba` + `d09a3b64` (the checkpoint's coupling state, the atomic load)

No measured number moved: the goldens are bitwise identical, the B0
de-trace identical, 7236 debug tests green. The checkpoint schema's
fold reconciliation: both fix branches had claimed CHECKPOINT_VERSION 9
(event-load streams; the ThermalSnapshot); the fold bumps once to 10
carrying both, the outer shape fingerprint re-verified green (the
thermal layout is gated independently by ThermalSnapshot's own
schema_version), and the resume-equality tests (the HRV variant
included, the derated recovery effectiveness carried) pass. The
red-team's blob-rollback probe proved the path sound; promoting it
into `checkpoint_regressions.rs` is the next checkpoint row's
follow-up.

### OD-33 + OD-11, 2026-10-02, the vendor sentinel fix (submodule `35c5edd` + `8681122`) and the first real thermal-trace comparisons

The vendored OCHRE's event sentinel (nanosecond `Timestamp.max` into a
pandas 3 microsecond column) crashed every OCHRE run with event-based
appliances at 40 of 48 hours; the patch writes the sentinel at each
column's own resolution (`as_unit`, a pandas 2 API: the vendor's floor
moves to `pandas >=2.0` in the follow-up vendor commit). The three
thermal-trace parity tests then ran their comparisons for the first
time since the `NameError` was introduced; none passed, so every
strict xfail stands on freshly measured numbers: the 48 h trace's
worst 6 h zone-temperature divergence 2.2616 °C (threshold 0.1) and
48 h mean 0.8048 °C (threshold 0.5); the overnight losses' worst
4.5975 °C and mean 0.8671 °C (threshold 0.3); the window solar peak's
ratio mean 1.305 with 36 of 36 sampled steps outside the 5 % band,
the worst step (640) OCHRE 416.6 W against HARES 547.9 W, 31.5 % =
(547.9 - 416.6) / 416.6. The first component divergence the detector
finds is step 1's infiltration heat gain (+8.7 %): the divergence is
in the physics inputs, not the accounting, and is the numeric block's
territory. The pytest census went 9 to 19 lines: the three
"OCHRE not installed" module skips are gone (the vendor is importable:
the ochre comparison tests run), and eight new intra-module parity
skips name the column mappings HARES's output schema does not provide
under OCHRE's names (the HVAC end uses among them: OD-13's scope).
The goldens are bitwise identical (the vendor runs only inside the
comparison tests).

### BR-21, 2026-10-04, fold branch `fold-br21c` (`398d21a6`, on `38398bd4`)

No product moved. Regenerating the OCV tables with the locked PyBaMM
(26.9.0.0) changed only their header line, which changes the defaults
tree BR-13's digest covers, so all seven goldens were re-captured
(`398d21a6`), each in its own clean worktree at `ede151da`
(`git_dirty` false): with `defaults_digest`, `git_head` and `git_dirty`
removed, every golden document is identical to the committed one (7
files, 20 lines in, 20 out); the new digest is `275cd399…` throughout.
Lint coverage: nine `[clippy*]` lines, all clean. Test counts at the
fold: default 7237, `observe` 7295, `dst` 7250, `profiling` 7243,
`observe,profiling` 1379, release 7194, release `observe` 7252 (the
six `conditioned_oracle` tests now run there); pytest 1381 passed, 19
skipped (the census), 3 xfailed.

### OD-13, 2026-10-04, the OCHRE comparison and fleet conversion tests run; the census empties

The pytest census went from 19 lines to zero: the OCHRE comparison tests
run against the vendored reference (pytest's `pythonpath` resolves the
submodule; the stale editable install is gone), the five diagnostic
visualization tests and the two never-failing benchmark tests are
deleted, the six fleet conversion tests build from one committed
ResStock building instead of three missing paths, and no skip site
remains that can fire in the block environment. The parity suite's
measured pairs for the one-hour BEopt case (kWh, OCHRE then HARES):
Total 0.4015574052175426 / 0.40155740521754263; HVAC Cooling 0.025 /
0.025000000000000012; HVAC Heating 0.0 / 0.0; Ventilation Fan
0.019949999999999996 / 0.019950000000000002; MELs 0.1339573399469733 /
0.13395733994697331; TV 0.07605636517135847 / 0.0760563651713584;
Refrigerator 0.05403247132666145 / 0.05403247132666145; Indoor Lighting
0.08545972626253563 / 0.08545972626253563; Exterior Lighting
0.007101502510013819 / 0.007101502510013806; Water Heating 0.0 / 0.0;
Lighting 0.09256122877254946 / 0.08545972626253563 (a relative error of
7.7 percent: HARES's aggregate excludes Exterior Lighting, a found
defect recorded in 11-other-defects.md, the case a strict xfail);
Other 0.28399617644499314 / 0.28399617644499314 (the fifteen-member
per-equipment sum; bitwise equal). Three of the nine original cases
were bitwise equal and six differed in the last one to two ulp
(summation order); all sit far inside their tolerances. The fleet
aggregate frame is pinned to exactly 24 rows by 6 columns, the
per-dwelling frame to 1 by 5, sample weight 1.0, and the aggregation
math to a relative error of 1e-12 (total 12.168140276367446 kWh, peak
2.0241609988980414 kW). Full suite: 1396 passed, 4 xfailed, 0 skipped;
`-m "slow or ochre"`: 95 passed, 4 xfailed, 0 skipped. The goldens are
untouched (the Python comparison layer runs beside the Rust gate, not
inside it).

### BR-22, 2026-10-04, the lint-suppression sweep (95 sites to 31 kept)

No runtime change: the goldens are bitwise identical in both profiles
before and after the cleanup pass, and the sweep's parameter-struct
refactors were proven value-faithful twice independently (the
implementer's stripped-identifier audit, which caught and fixed three
slips, and the red team's scripted field-by-field diff of roughly 120
call sites, which found none). Test counts at the commit `8082592e`:
default 7237, observe 7295, dst 7250, profiling 7243, release 7194,
observe,profiling 7307, hares-core after the TOML cleanup 1324; pytest
1396 passed, 4 xfailed, 0 skipped. One HELICS flake observed once
(test_stale_broker_raises_timeout_instead_of_hanging, green standalone
and on the two subsequent full runs: the OD-32 family's
machine-load sensitivity, module untouched). The suppression census
goes from 95 code sites to 31, each kept with a stated constraint.

### BR-23, 2026-10-04, the pyright gate over the Python package

First run: 617 errors; final: 0 errors, 0 warnings, with the package's
rules at strict and the typings directory self-checked. The gate was
proven real by negative probes in all three environments (package,
tests, stub), each seeded error caught and reverted byte-identical.
Pytest at the commit `787487df`: 1396 passed, 4 xfailed, 0 skipped;
the wheel builds. The HELICS flake family appeared in two of the
implementer's four full runs and in a HEAD worktree (pre-existing,
the OD-32 family); the final runs were clean. The typing surface's
honesty is runtime-verified: the six step TypedDicts' key sets were
checked against their Rust producers (py_dwelling.rs:858-874,
py_fleet.rs:461-514), and the exception bases against the
create_exception sites.

### OD-12, 2026-10-04, the pvlib generator's silent zeros (commit `901354e0`)

The regeneration hazard was worse than drift: under the block
environment the generator's pvlib extraction raised AttributeError and
the blind except wrote all-zero irradiance. Archaeology: the committed
freefloat fixtures' night rows (dhi == 0) carry aoi 0.054831 =
radians(3.14159), the except path's marker, from the pvlib 0.15.0
capture environment where perez raised ZeroDivisionError on every
dhi==0 row. After the fix: every dhi>0 row regenerates byte-identical
(5520 rows across the three scenarios), pvlib's numerics are
bit-identical across 0.15.0 and 0.16.1 (a six-condition differential
sweep), and the only residual difference is the marker rows' real
sun-position aoi plus genuine beam/ground values on the 120 rows where
dhi == 0 but dni > 0 per spring/summer scenario (winter: none). The
fixture stands as committed (night irradiance zero is physically
correct and the oracle consumes it); retiring the marker rows is a
deliberate-copy decision for the numeric block. Census at the commit:
1398 passed, 4 xfailed, 0 skipped.

### OD-14, 2026-10-04, pint 0.24.4 to 0.26.1 (commit `699cfad2`)

The alias `inch_H2O_39F = inch_H2O_4C` is quantity-identical at the
base-unit scale (bitwise); the Pa conversion lands one ULP from
0.24.4's native value because pint 0.26.1's own conversion machinery
shifts it: a control experiment that replicates 0.24.4's exact water
constant under 0.26.1 produces the same shifted ULP, and pint's liter
factor itself is inexact in both versions (decimeter ** 3). End to
end the one-hour parity dwelling is bitwise identical to 0.24.4's
across all twelve end uses; the 72 h freefloat envelope's indoor flow
and heat gain move at most 4 ULP (relative 4e-16), the same
machinery drift, everything else bitwise. Census: 1400 passed,
4 xfailed, 0 skipped. Process note: the red team's environment probe
accidentally ran a full freefloat generation, rewriting six committed
fixture files in the working tree; they were restored from the index
before anything was staged, and the incident is a live demonstration
of why the fixture_regeneration_hazard rule and BR-24's stray-file
gate exist.

### OD-42 + OD-45, 2026-10-04, one config generation from spec to equipment (commit `ee7bee85`)

The mapping found the root of both entries: two config generations per
equipment (the constructor's un-merged payload, init's merged one).
The landing's semantics: per-field merge with a schema-gated
normalize-and-retry (the Python builders' raw-text enum vocabulary
lands in canonical form; one retry; a failure raises naming the
equipment, the field and the value). The first round's regression
(three working Python constructions failing on landed raw-text enums)
was caught by execution-verified review and red team and fixed in the
shape above. Goldens bitwise identical in both profiles; the assembly
performance probe (3 fleet dwellings x 15 iterations): 659.6 ms at
the base against 656.4 ms on the diff, within noise. Census: 1412
passed, 4 xfailed, 0 skipped; nextest 7051 default, 7109 observe;
pyright 0 errors.

### OD-3, 2026-10-04, the indirect tank's loop survives autosizing (commit `249912f9`)

Two premise corrections were proven before the fix: the wired
non-autosized boiler ALSO lost its id (the missing CoolingCapacity
flags cooling autosizing, whose trailing rebuild wrote loop_id: None;
the unmodified base-dhw-multiple boiler sat on the LoopId(0) fallback,
not its wired loop 1), and the tank's own water-heater autosize wiped
its wired id through the stale serialized null in its parameters. The
red team's independent topology dump: the boiler's supply loop and the
tank's supply loop both the wired LoopId(1), distinct from the DHW
demand loop (65534); two boiler-plus-tank pairs allocate loops 1 then
2 with no collision or counter skip; the adversarial paths (a string
loop_id, a missing idref, a water-heater wiring target) are typed
resolve errors naming the tank and the idref. The sweep: 425
OS-HPXML samples, build errors 112 before and after, zero new (no
sample carries a dangling or non-boiler tank wiring). Goldens bitwise
identical in both profiles; nextest 7075 default and 7133 observe
(the four failures are the red team's untracked scratch files pinning
the pre-fix behavior the fix reverses); pytest 1412 passed, 4 xfailed,
0 skipped.

### OD-6, 2026-10-04, the schedule-path contract (commit `13881b48`)

The hazard quantified: at the base head, a DwellingConfig pointing at
a nonexistent schedule file silently generated a schedule whose digest
(fb3ae7ac...) differs bitwise from the real schedule's, so a mistyped
path ran with different occupancy and loads and no error. The None
arm's generation is bitwise the old silent path's (both ca65bbaf...,
verified from a HEAD worktree). Every entrance closed and probed: the
two Rust constructors, the Python bindings (the error names the path),
the fleet (a missing per-building CSV fails that building's
construction naming the path; a mixed fleet reports exactly the broken
one), the golden adapter (a manifest with a missing schedule fails the
capture loudly). Census at the commit: 1412 passed, 4 xfailed,
0 skipped; nextest 7066 default, 7124 observe; pyright 0 errors; the
goldens bitwise in both profiles.

### The worktree batch, 2026-10-04/05, rows 24a, 24f, 24i, 24j folded; head `11ca3bf3`

Three parallel branch worktrees (row/od10-structural, row/od17-
schedule-defaults, row/od22-od23), each implemented, reviewed and
red-teamed in its own worktree (every branch SHIP from both), then
cherry-picked onto the branch after the OR-19 fold landed under them
(`17bd7291`, `2f85338d`): `18fbf72e` (OD-10 structural), `8c034e4f`
(OD-22), `6c33e689` (OD-23), `952e5486` (OD-17), the two doc-committer
follow-ups, and the batch fix `dc4a456d` + `a416b901` + `11ca3bf3`
(the fleet's all-failed error carries every dwelling's cause: the
OD-6 red team's observability finding, promoted by use; the fleet and
helics tests pass a defaults path the strict three-source rule
requires; the goldens' provenance re-captured for OD-17's Ceiling Fan
row: products bitwise per name). One fold interaction surfaced (the
schedule-injection error test needed the garage-modeled flag its
signature gained) and was fixed. A native defect found and mitigated:
the helics wheel's libzmq exports the C++ runtime without declaring
it (OD-59): helics before pyarrow segfaulted every worker that wrote
a parquet; the conftest now imports pyarrow first and the canary pins
the defect. The head `11ca3bf3`: nextest 7342 default, 7400 observe,
all green; pytest 1419 passed, 4 xfailed, 0 skipped; the goldens
bitwise in both profiles; clippy and fmt clean. One flake observed
(test_mismatched_subscription_topic_logs_stale_warning under xdist,
green standalone: the HELICS family, the test-hygiene branch's fix).

### Rows 24k to 24n, 2026-10-05, folded from the last two worktrees; head `db5efe80`

row/od24-od25 (`1eba7760`, `7ac4d4d9`, `9a087efa`) and row/od26-od27
(`f4e4ff9a`, `db5efe80`), each implemented, reviewed and red-teamed in
its own worktree (both branches SHIP from the third-agent review+red-
team), then cherry-picked. The OD-24 granularity consequence recorded:
a malformed mapping now degrades the whole defaults store under the
blueprint's documented policy, not just the mapping. The OD-25
reconciliations verified against the vendored reference: OCHRE's
convert_power_column yields a determined zero for a zero-declared load
and RAISES when no power source exists; its schedule converter creates
no equipment from a column alone, so the 13 phantom TV specs' removal
is reference-faithful and physics-free (goldens bitwise). OD-26's
values are the seed path's own (6.6 kW away-charge, the catalogue's
fueleconomy figure); the L1 cap is a value-preserving constant
extraction. At the tip: nextest 7348 default, 7406 observe; pytest
1420 passed, 4 xfailed, 0 skipped; pyright 0 errors; the goldens
bitwise in both profiles; the B0 de-trace identical. OD-60 filed
(the inject_constant_power_schedule zero-fill, the same class).

### OD-34 and OD-35 in the branches, 2026-10-05 (awaiting fix-headbreaks)

row/od34-tariff (`7b33858a` + `eeebc450`): the evaluator's full billing
state joins the checkpoint (version 12, the shape pin re-pinned);
periods prorate to the active span at every exit (replacement, finalize,
boundary), a joined finalize bitwise the replacement close of the same
instant; the resume pins cover the demand window, the ratchet, the TOU
peaks, the post-restore attach and the replacement across a resume, all
bitwise against their continuous analogues; the mutation battery caught
the running_sum and the ratchet histories, and the two masked
mutations (push_count, the ring head at capacity 2) are stated in the
test, not hidden. From-start arithmetic untouched: the goldens bitwise
in both profiles. row/od35-ev-capacity (`2e8fb0f4`): the negative fade
was the Smith 2017 BOL fitted levels above nameplate (1.0711/1.009:
raw fade −0.009) with HARES's port missing SSC's only-decreases guard
(verified verbatim against NREL/ssc at `2527d557`); the fade floors at
the previous reported state and the belt raises in every build; the
reference curve agrees to the last bit with an independent port;
3650-day extreme sweeps bounded. Both branches wait for fix-headbreaks
(both base `db5efe80`; the fold order is theirs).

### The fold-process lesson, 2026-10-05 (the 24o drain's audit)

The audit of the session's 13 commits found: the cherry-picked fold
series left broken intermediate commits (952e5486 broke a hares-io
test compile until a416b901; the OR-19 fold's eight commits inherited
13881b48's break until 2f85338d; 952e5486 left the goldens' defaults
digest stale until dc4a456d's re-capture), and the OD-23 branch's
golden re-capture recorded a dirty tree and a cherry-pick-orphaned
head sha. The rule going forward: every committed sha builds and
passes the gates on its own; a fold lands as ONE squashed commit per
row, or carries its fix-forward in the same push; the goldens'
provenance is re-captured from a clean tree whose recorded head is
the folded sha. Bisecting the branch must stay truthful.

### Fold order 1: fix-headbreaks squashed onto the branch, 2026-10-06; head `5949d509`

The dead session's fix-headbreaks branch (20 commits + the handoff
commit `9ee0f6d7`) squash-folded as `10588d48` (42 files: the pyright
pythonVersion 3.12 gate, the TypeIs import from typing_extensions, the
repeated default-profile row an error, a defaults-less config says so,
the loop allocator's loud read failures, the heat pump's FUEL
declaration from init's config, the garage and basement lighting
warning rules per OS-HPXML v1.12.0 (lighting.rb:113-122), the boiler's
placeholder loop id gone, the stand-in conditioned volume gone, the
ResStock fleet's defaults directory, the pvlib test's group guard, the
type-ignore comments gone), plus the provenance re-capture `5949d509`
(git_head at the folded sha, git_dirty false, the digests unchanged).
The independent audit: FOLD-CLEAN (the squash tree is the branch's diff
applied verbatim; the per-commit walk found no file whose change is
missing; the golden provenance's only deltas are git_head and
git_dirty). The e113a61f message overstates its mechanism (it says the
declaration came from the constructor's typed().ok() read; the truth:
assembly already passed one merged config to both, so they agreed
there; the hazard was an equipment initialised with a config other
than its constructor's): the reword was not done (it would orphan the
seven golden captures' recorded shas); the correction is recorded HERE
and the squash's own message states the truth. Full block at the tip:
nextest 7360 default, 7418 observe, 7317 release; the goldens 6+1
bitwise in both profiles; pytest 1420 passed, 4 xfailed, 0 skipped;
pyright 0 errors.

### OD-40, 2026-10-04, the autosize heating-only contract violations (commit `a7a9253d`)

The sweep at the head: 425 OS-HPXML samples, January and July days at
96 x 900 s: 112 build errors (pre-existing, unchanged: three causes
filed separately), 13 contract violations before the fix, 0 after, 0
step errors. Cause A's failing state, captured on the step: electric
0, compressor 0, fan 0, coil sensible cooling 82.9 W, post-DSE 52.8 W,
duty cycle 0.0232, COP 5.69, mode standby, thermal -50.76654297004957
(the violation's exact value; the mini-split's -120.96). Cause B's:
every dual-fuel sample's first burner step, undeclared flows.fuel_w.
Post-fix the dual-fuel January day burns with FUEL declared on every
step. Goldens bitwise identical in both profiles (the captured paths
carry fraction-1.0 HVAC except the fleet's 0.4-fraction cooler, which
the golden week never calls); default nextest 7243; pytest 1400
passed, 4 xfailed, 0 skipped.

### OD-30, 2026-10-05, appliance heat reaches its zone, split as OS-HPXML splits it (branch `fix-od30` on `1b708ccb`)

The numeric commits: `9a32e928` (HPXML appliance loads given their
zone), `a71f0af8` (event replay at the schedule row of the simulated
date), `e2785ed1` (appliance and plug-load heat 0.6 radiant of
sensible), `320f85c2` (lighting 0.2 convective, 0.6 radiant, 0.2
visible, the visible part absorbed like transmitted diffuse solar). The
goldens were re-captured after each on the rebased history (`21048bde`,
`7183785a`, `b41221f6`, each golden's git_head its parent); every later
commit leaves them bitwise unchanged. Neither rebase (onto `2f85338d`,
then onto `1b708ccb`) changed a golden number; the head's goldens at
`1b708ccb` carry the defaults digest of a dirty capture (`04eb8f10`,
`4baeb1ba` for consumer_shape_900s) and fail compare-all at the head
itself on the digest alone, while the branch's carry the digest the
tree's defaults give (`3f9c3a95`, `5339acb2`).

**Dwelling build time**, `Dwelling::from_config` on ResStock 2025.1
bldg0000001 (1 h at 1 min, CSV schedule), 21 release builds per run,
four alternating runs of the tip before and after the override fix
(now `7f59167d`: the override map borrowed per equipment instead of cloned,
each config merged once instead of twice). The machine was shared and
the medians overlap; the minimums: no overrides 177 to 166 ms, with a
wildcard and two load overrides 179 to 159 ms.

**Golden deltas**, base `38398bd4` to the branch, the energy and peak
leaves (each golden's other changed leaves are the comfort, unmet-hour
and warm-up residual statistics that follow them):

| Golden | Leaf | Before | After | Change |
|---|---|---|---|---|
| consumer_shape_900s | 0.grid_interaction_metrics.peak_import_kw | 3.047 | 3.207 | 5.25 % |
| consumer_shape_900s | 0.peak_power_kw.per_end_use.hvac_cooling | 1.177 | 1.192 | 1.3 % |
| consumer_shape_900s | 0.peak_power_kw.per_end_use.hvac_heating | 0.163 | 0.163 | -0.04 % |
| consumer_shape_900s | 0.total_energy_kwh.gross_consumption_kwh | 8843.667 | 8919.696 | 0.86 % |
| consumer_shape_900s | 0.total_energy_kwh.per_end_use.hvac_cooling | 1115.735 | 1198.001 | 7.37 % |
| consumer_shape_900s | 0.total_energy_kwh.per_end_use.hvac_heating | 47.4 | 41.164 | -13.16 % |
| fleet_mixed_offset | 0.grid_interaction_metrics.peak_import_kw | 5.854 | 5.977 | 2.12 % |
| fleet_mixed_offset | 0.total_energy_kwh.gross_consumption_kwh | 120.138 | 104.728 | -12.83 % |
| fleet_mixed_offset | 1.grid_interaction_metrics.peak_import_kw | 5.604 | 5.599 | -0.09 % |
| fleet_mixed_offset | 1.peak_power_kw.per_end_use.hvac_heating | 4.645 | 4.619 | -0.56 % |
| fleet_mixed_offset | 1.total_energy_kwh.gross_consumption_kwh | 204.911 | 201.524 | -1.65 % |
| fleet_mixed_offset | 1.total_energy_kwh.per_end_use.hvac_heating | 89.957 | 80.006 | -11.06 % |
| fleet_mixed_offset | 1.total_energy_kwh.per_end_use.water_heating | 25.326 | 25.355 | 0.11 % |
| fleet_mixed_offset | 2.grid_interaction_metrics.peak_import_kw | 2.679 | 2.69 | 0.42 % |
| fleet_mixed_offset | 2.peak_power_kw.per_end_use.hvac_heating | 0.116 | 0.115 | -0.45 % |
| fleet_mixed_offset | 2.total_energy_kwh.gross_consumption_kwh | 167.831 | 169.055 | 0.73 % |
| fleet_mixed_offset | 2.total_energy_kwh.per_end_use.hvac_heating | 3.58 | 3.249 | -9.24 % |
| full_year_900s_dst | 0.grid_interaction_metrics.peak_import_kw | 24.292 | 23.575 | -2.95 % |
| full_year_900s_dst | 0.peak_power_kw.per_end_use.hvac_cooling | 0.779 | 0.857 | 10.02 % |
| full_year_900s_dst | 0.peak_power_kw.per_end_use.hvac_heating | 20.845 | 20.828 | -0.08 % |
| full_year_900s_dst | 0.peak_power_kw.per_end_use.water_heating | 0.573 | 0.577 | 0.85 % |
| full_year_900s_dst | 0.total_energy_kwh.gross_consumption_kwh | 12366.674 | 12200.804 | -1.34 % |
| full_year_900s_dst | 0.total_energy_kwh.per_end_use.hvac_cooling | 459.99 | 480.309 | 4.42 % |
| full_year_900s_dst | 0.total_energy_kwh.per_end_use.hvac_heating | 6755.612 | 6570.627 | -2.74 % |
| full_year_900s_dst | 0.total_energy_kwh.per_end_use.water_heating | 419.913 | 418.709 | -0.29 % |
| full_year_hourly | 0.grid_interaction_metrics.peak_import_kw | 5.779 | 5.668 | -1.93 % |
| full_year_hourly | 0.peak_power_kw.per_end_use.hvac_cooling | 0.831 | 0.879 | 5.8 % |
| full_year_hourly | 0.peak_power_kw.per_end_use.hvac_heating | 0.211 | 0.211 | -0.28 % |
| full_year_hourly | 0.peak_power_kw.per_end_use.water_heating | 1.402 | 1.403 | 0.12 % |
| full_year_hourly | 0.total_energy_kwh.gross_consumption_kwh | 9338.211 | 9388.749 | 0.54 % |
| full_year_hourly | 0.total_energy_kwh.per_end_use.hvac_cooling | 503.933 | 528.029 | 4.78 % |
| full_year_hourly | 0.total_energy_kwh.per_end_use.hvac_heating | 342.17 | 332.533 | -2.82 % |
| full_year_hourly | 0.total_energy_kwh.per_end_use.water_heating | 3760.948 | 3797.027 | 0.96 % |
| leap_year_february_900s | 0.grid_interaction_metrics.peak_import_kw | 21.406 | 21.424 | 0.08 % |
| leap_year_february_900s | 0.peak_power_kw.per_end_use.hvac_heating | 20.675 | 20.692 | 0.08 % |
| leap_year_february_900s | 0.peak_power_kw.per_end_use.water_heating | 0.518 | 0.519 | 0.28 % |
| leap_year_february_900s | 0.total_energy_kwh.gross_consumption_kwh | 220.658 | 227.167 | 2.95 % |
| leap_year_february_900s | 0.total_energy_kwh.per_end_use.hvac_heating | 149.944 | 146.729 | -2.14 % |
| leap_year_february_900s | 0.total_energy_kwh.per_end_use.water_heating | 4.23 | 5.886 | 39.14 % |
| month_boundary_billing_3600s | 0.grid_interaction_metrics.peak_import_kw | 5.689 | 5.109 | -10.21 % |
| month_boundary_billing_3600s | 0.peak_power_kw.per_end_use.hvac_heating | 0.194 | 0.194 | -0.2 % |
| month_boundary_billing_3600s | 0.peak_power_kw.per_end_use.water_heating | 1.396 | 1.403 | 0.54 % |
| month_boundary_billing_3600s | 0.total_energy_kwh.gross_consumption_kwh | 354.247 | 366.734 | 3.52 % |
| month_boundary_billing_3600s | 0.total_energy_kwh.per_end_use.hvac_heating | 28.278 | 27.844 | -1.54 % |
| month_boundary_billing_3600s | 0.total_energy_kwh.per_end_use.water_heating | 144.524 | 150.277 | 3.98 % |
| weather_wrap_900s | 0.grid_interaction_metrics.peak_import_kw | 21.923 | 21.403 | -2.37 % |
| weather_wrap_900s | 0.peak_power_kw.per_end_use.hvac_heating | 20.961 | 20.958 | -0.01 % |
| weather_wrap_900s | 0.peak_power_kw.per_end_use.water_heating | 0.504 | 0.522 | 3.49 % |
| weather_wrap_900s | 0.total_energy_kwh.gross_consumption_kwh | 290.576 | 286.509 | -1.4 % |
| weather_wrap_900s | 0.total_energy_kwh.per_end_use.hvac_heating | 228.347 | 225.414 | -1.28 % |
| weather_wrap_900s | 0.total_energy_kwh.per_end_use.water_heating | 3.636 | 4.756 | 30.79 % |

Changed leaves per golden: consumer_shape_900s 11 of 33, fleet_mixed_offset
34 of 89, full_year_900s_dst 17 of 40, full_year_hourly 17 of 40,
leap_year_february_900s 17 of 40, month_boundary_billing_3600s 18 of 41,
weather_wrap_900s 17 of 40. The table is the four numeric commits
together; the first one's own deltas (`full_year_hourly` internal gain
+451 kWh, heating -209 kWh, cooling +77 kWh, each equal to the appliance
energy times its fraction) are in `00-schedule.md` row 21h.

**Replay-date leaves**, the ones `a71f0af8` moves, one cause per golden:
`fleet_mixed_offset` dwelling 0 `cooking` 7.93 to 15.90 kWh and `other`
52.52 to 29.14 kWh: the old replay counted its own steps from the first
schedule row through warm-up, so the reported week replayed the
schedule's 2 to 9 January rows (two warm-up days of residue), the new
replay reads the run's own 1 to 7 January rows; the new cooking reproduces
exactly from those rows' cooking events at the range's rating in all
three dwellings (2.815, 3.598 and 1.004 kW), energy and peak, and the
old week's cooking gives the captured 7.93 kWh and 1.454 kW peak from
the residue rows; `other` is the same two weeks' appliance cycles (the
old window's washer and dryer cycles sum 1.8 times the new week's).
`leap_year_february_900s` water heating 4.23 to 5.89 kWh: the run's own
late-February rows draw 1.37 to 1.41 times the schedule-head rows' hot
water, and the heat pump water heater's energy tracks the drawn volume.
`weather_wrap_900s` water heating 3.64 to 4.76 kWh: the replayed days
moved to 30 December to 2 January (the year wrap applied); the drawn
volume over those days is close to the old rows', but the cycles fall in
the night hours twice as heavily (the draw's night share 7 to 10 percent
against 14.4), so the recovery spends more per litre, and warm-up now
replays the year-end draws so production starts from a different tank
state.

**Parity against the OCHRE 1-hour fixtures**, measured at the rebased tip
by reverting each step on the tree (A: every gain convective, the state
before `5de3e872`; B: the appliance split only, the state before
`78e38545`; C: the visible part sent to the zone air), against the head.
Each of A, B and C reproduces the pre-rebase measurement of the same
state within 0.0011. Rows that change:

| Fixture, metric | A | B | C | Head |
|---|---|---|---|---|
| cz2a_gas_furnace_ac_res_wh, zone temp MAE °C | 0.6895 | 0.6772 | 0.6761 | 0.6758 |
| cz2a_pv_ev, zone temp | 0.9080 | 0.8990 | 0.8980 | 0.8977 |
| cz4a_ashp_hpwh, zone temp | 0.8880 | 0.8870 | 0.8850 | 0.8843 |
| cz4a_battery_only, zone temp | 0.8834 | 0.8667 | 0.8641 | 0.8632 |
| cz4a_pv_battery, zone temp | 0.8514 | 0.8361 | 0.8346 | 0.8342 |
| cz4a_pv_only, zone temp | 0.8937 | 0.8820 | 0.8802 | 0.8798 |
| cz5a_ev_charging, zone temp | 0.8840 | 0.8671 | 0.8655 | 0.8652 |
| cz5a_ev_only, zone temp | 0.8816 | 0.8658 | 0.8632 | 0.8623 |
| cz5a_minisplit_gas_wh, zone temp | 1.7109 | 1.6223 | 1.7488 | 1.7433 |
| cz5a_minisplit_gas_wh, short-window HVAC energy % | 64.1870 | 64.2485 | 64.1460 | 64.1498 |
| cz5a_minisplit_gas_wh, short-window site energy % | 17.1768 | 17.1473 | 17.1964 | 17.1946 |
| cz6b_pv_battery_ev, zone temp | 0.8747 | 0.8602 | 0.8581 | 0.8575 |
| cz6b_resistance_res_wh, zone temp | 0.7459 | 0.6963 | 0.7191 | 0.7084 |
| resstock_bldg0112631_24h, short-window HVAC energy % | 20.1270 | 21.7779 | 21.7779 | 21.7779 |
| resstock_bldg0112631_24h, short-window site energy % | 5.1356 | 5.2561 | 5.2561 | 5.2561 |
| resstock_bldg0112631_24h, zone temp | 0.4077 | 0.4212 | 0.4363 | 0.4407 |

The moves away from OCHRE (the mini-split's zone temperature and site
energy, the ResStock fixture's three rows) are class (a) under
`ochre_deviation_triage`, recorded as D-005 to D-007 in
`docs/alignment/DIVERGENCES.md` and beside the parity bands. In state A
seven of the nine tightened zone-temperature bands fail;
cz2a_gas_furnace_ac_res_wh, cz4a_ashp_hpwh and the mini-split HVAC-energy
band do not.

### OR-18c, 2026-10-05, the zone capacitance multiplier sizes the equivalent battery (branch `fix-or18c` on `2f85338d`)

Every zone's air node now takes OS-HPXML's temperature capacitance
multiplier, 7 by default (`simcontrols.rb:27-28`, `defaults.rb:219-221`).
HPXML homes with furniture boundaries had 1 on the conditioned zone. The
dwelling hands the conditioned zone's capacitance, multiplier included,
to each HVAC unit's equivalent battery, whose energy and its minimum and
maximum (`equivalent_battery.rs:140-156`) scale with it. OCHRE's
equivalent battery reads the same multiplied zone capacitance
(`ochre/Equipment/HVAC.py:625-640`).

| Home | Conditioned air volume | Capacitance at ×1 | At ×7 |
|---|---|---|---|
| `bldg0176227` (golden `consumer_shape_900s`) | 749.8 m³ | 0.2521 kWh/K | 1.7644 kWh/K |
| `BEopt_example.xml` | 271.8 m³ | 0.0915 kWh/K | 0.6403 kWh/K |

Consumer impact: a calibration that reads the equivalent battery's
energy, minimum and maximum (the flexibility a thermostat band offers)
sees seven times the storage it saw on these homes, and the same zone
recovers its band seven times more slowly. That is the capacitance the
thermal solver itself now integrates, so the battery and the simulated
zone agree. The register records it as D-011.

### OR-18c, 2026-10-05, the OCHRE parity comparison made fair (branch `fix-or18c`)

Two comparison defects (class d) were fixed before any band was judged:
the HVAC-energy metric counted HARES's HVAC twice (unit columns and the
end-use totals that sum them, and gas therms added to kW), and both
sides started each cold 1-hour window from different random indoor
temperatures (OCHRE's unseeded, `Envelope.py:997-1008`). Each fixture
now pins OCHRE to HARES's starting temperature, and every reference but
`cz4a_ashp_hpwh` (which the vendored OCHRE cannot rebuild,
`WaterHeater.py:456`) is regenerated; only start-dependent columns
change.

Residuals in %, at the branch's code, first with the corrected HVAC-energy
count against the old references (unpinned starts), then against the
pinned references:

| Fixture | HVAC energy, old ref | HVAC energy, pinned | Total site, old ref | Total site, pinned | Peak, old ref | Peak, pinned |
|---|---|---|---|---|---|---|
| cz2a_gas_furnace_ac_res_wh | 9.38 | 6.48 | 4.27 | 3.17 | 2.67 | 2.67 |
| cz2a_pv_ev | 40.08 | 0.53 | 36.21 | 0.50 | 0.54 | 0.54 |
| cz4a_ashp_hpwh (not regenerated) | 22.56 | 22.56 | 20.71 | 20.71 | 0.54 | 0.54 |
| cz4a_battery_only | 3.98 | 0.53 | 3.74 | 0.47 | 0.54 | 0.54 |
| cz4a_pv_battery | 8.11 | 0.53 | 7.61 | 0.47 | 0.54 | 0.54 |
| cz4a_pv_only | 17.06 | 0.53 | 15.66 | 0.50 | 0.54 | 0.54 |
| cz5a_ev_charging | 3.98 | 0.53 | 1.08 | 0.14 | 0.54 | 0.54 |
| cz5a_ev_only | 3.98 | 0.53 | 3.71 | 0.50 | 0.54 | 0.54 |
| cz5a_minisplit_gas_wh | 29.63 | 33.60 | 28.42 | 32.30 | 92.92 | 0.59 |
| cz6b_pv_battery_ev | 8.11 | 0.53 | 7.61 | 0.47 | 0.54 | 0.54 |
| cz6b_resistance_res_wh | 26.75 | 22.06 | 25.40 | 20.89 | 3.60 | 3.60 |
| resstock_bldg0112631_24h | 25.24 | 13.90 | 14.83 | 13.77 | 25.39 | 25.39 |

With the old double count, the harness printed 64.2 % (cz5a) and 58.1 %
(cz6b) at the commit before the multiplier, and 40.7 % and 46.5 % after
it, against real residuals of 17.9 %, 21.0 %, 29.6 % and 26.7 %.

Every residual is inside its default band but the mini-split's HVAC
energy and total site, OCHRE's duct losses on a ductless unit (class a,
measured: with OCHRE's distribution efficiency at 1 HARES is 5 to 8.5 %
above it), whose bands are the residuals plus about 1 % with the cause
beside them. The mini-split's 92.9 % peak was OCHRE's random start (with
the start pinned its peak matches HARES's within 0.6 %). The resstock
peak lock (25.7 %) predates the branch and stays, its cause untriaged.

### Folds 3 to 7, 2026-10-06, the dead sessions' branches drained; head `585aece4`

The fold order ran: the testhyg chain squashed to 23 building commits
(`ed9ce671`; the six broken commits folded into the helper commit, a
second unwalked break in the oracle gate fixed at the earliest sha,
the tariff-checkpoint test adapted to the branch's helper; the HELICS
50/50 run clean; the census 1494/4/0); fix-headaudit rebased (its
pyright commit dropped as the fold's duplicate; 22 commits, each
verified building; the census 1535/4/0) and folded after its
independent SHIP; fix-od30 finished (the last commit completed, the
four items done, M1/M2/L1-L8 closed; the review verified every
citation against upstream OS-HPXML v1.12.0 and EnergyPlus v24.2.0
fetches) and folded after its rebase onto the grown head (a semantic
conflict caught by the pytest gate: the duplicated defaults_path
kwarg); fix-or18c finished (the rebase completed with the branch's
attic mechanism winning its subject files; the combined physics
re-captured: the frames move where the branch's capacitance,
geometry, terrain and roughness meet the head's gain split and
schedule-row replay; every delta's cause family named in the
re-capture commits) and folded; fix-od16 finished (the rebase onto
the grown head: the eviction semantics merged with the head's actor
work; the checkpoint pin re-derived, then the independent review
BLOCKED it: the failure budget's field had landed under an unchanged
version 12: fixed to version 13 with the version-gate probe proven
RED then GREEN) and folded. The DR TurnOff question: DECIDED correct
by instrumentation (the ModeOverride holds only the delivery step;
the 225 kWh BESTEST heating reduction is the curtailment's own
thermal carry-over; OCHRE's per-end-use per-timestep signals and
EnergyPlus's separate availability schedules agree), pinned by a test.
At the tip `585aece4`: nextest 7544 default, 7604 observe; pytest
1571 passed, 4 xfailed, 0 skipped; pyright 0 errors; the goldens 6+1
bitwise in both profiles. New finds ledgered: OD-62 (the non-ideal
mode override sticks after its event), OD-63 (from_resstock's zip
paths and unset FIPS), OD-64 (the blueprint's empty-defaults
fallback).

### The checkpoint drain completed, 2026-10-07/08; the final head `62ba224b`

The last rows landed and the checkpoint 24o's gates ran at the final
head. In order: row 24h (OD-21, `e9289f7e`): a malformed HPXML
attribute fails the parse naming element, attribute and byte position,
the XML 1.0 normalization and the fixture site's tz lookup pinned, two
test fixtures whose escaped-quote xmlns parsed only through the
dropped-attribute defect repaired; row 16a1 (BR-20, `095f1095` after
the rebase): rust-toolchain.toml pins stable 1.99.0 (the runs used
1.94.1 before), nextest 0.9.148, cargo-deny 0.20.2, cargo-llvm-cov
0.9.1, uv 0.12.23, maturin 1.15.0, eleven new lint sites fixed, the
advisories clean, the goldens bitwise on the new compiler (no
toolchain field in the golden documents); the OD-56/OD-57 leftovers
(`62ba224b` after the rebase): the zone-label mapper and the loop
allocator's test collector stop substituting silently (errors naming
the boundary/spec), the attic-floor coupling candidate ruled down to
~0.14 °C of the −1.25 bias by experiment and the film divergence
registered as D-031 with pins, the register's D-003 corrected (the E+
default interior convection is ASHRAE Simple, TARP is what OS-HPXML
sets). The roll-up's own findings closed: the fleet's all-failed error
carries its causes and is pinned by a test (`41935191`), OD-46's State
records the typed_overrides fix, the BESTEST drift locks re-measured
at the combined tips with the causes. The final head's gates, all
green: fmt; clippy all-features -D warnings; nextest 7595 default,
7657 observe; the goldens 6+1 bitwise in both profiles; pytest 1582
passed, 5 xfailed, 0 skipped (the skip census empty); pyright 0/0; the
doc gate clean; the advisories clean. The B0: re-baselined at the
audit tip `27db4a1e` (the appliance bedroom fix and the diffuse and
cycling physics moved the estimator's fits; the causes in the roll-up),
verified identical across two runs and across the consumer's own
pinned-tree build; the consumer re-pinned (`090ce2a` then the drained
head) with its full gate 346 passed, 0 failed, 7 environment skips,
ruff clean. The roll-up report against `1d60f478`:
verification/ROLLUP-24o.md.

### B-16, 2026-10-08, the wet-appliance hot-water volume triage (branch `row/b16-hw-triage`, measured at `5ece4060`)

Load average at write time: 10.55 (1 min). The measurement ran the probe
below to completion twice (the second run after the cycle-boundary fix
described under Method); the numbers recorded are the second run's, and
they reproduce the planning-time arithmetic of `09-numeric-fixes.md` B-16
to its printed digits.

Method. The probe is an integration test in a scratch worktree of
`5ece4060`, `crates/hares-core/tests/b16_wet_appliance_probe.rs`
(uncommitted, never to be committed; this record is its description). It
builds each home with `Dwelling::from_config` (no warm-up, the draw is
schedule-driven, master seed 0, no overrides, the repository `defaults`),
runs the full year step by step (`Dwelling::step`), and after every step
drains the step observer (`enable_observer(8)`, `drain_observations`)
reading, per equipment, the fluid contributions its step wrote to the
well-known wet-appliance demand loop (`loop_id` = `DHW_DEMAND_LOOP`
(`u16::MAX - 1`), `fluid_type` = Water, `delta_flow_kg_s` > 0, from
`PhaseSnapshots.post_nonthermal_equipment` and `post_thermal_equipment`).
Drawn mass per step is `delta_flow_kg_s` times the step; per-cycle volume
is the sum over a maximal contiguous run of drawing steps; the annual
volume is the run's sum. This reads the engine's own realised draw on the
same accumulator the water heater's `read_dhw_demand_kg_s` reads
(`water_heater/mod.rs:86-92`), per appliance, with no engine patch. Homes
and steps: `cz4a_ashp_hpwh` at 60, 300, 900 and 3600 s; `bldg0176227`,
`bldg0000002` and `bldg0000004` at 900 and 3600 s (the schedule files are
hourly for the parity homes, 15-minute for the ResStock homes).
`bldg0000002` has no dishwasher in `home.xml`. The two parity buildings
share byte-identical appliance inputs and byte-identical
`clothes_washer`, `dishwasher` and hot-water schedule columns (diff
verified), so `cz2a_gas_furnace_ac_res_wh`'s realised volumes are the
`cz4a_ashp_hpwh` numbers and it was not re-run. Command:
`cargo nextest run -p hares-core --features observe b16_ --nocapture`
(517 s wall; a first run failed the probe's own cycle-closing bookkeeping
before the fix, engine numbers unchanged). A companion diagnostic in the
same worktree, `crates/hares-core/tests/b16_wet_appliance_diag.rs` (also
uncommitted), dumps every cycle by name at 900 s: the cz4a washer's 206
cycles are each whole-hour multiples of 36.000 kg (min 36.000), the
dishwasher's 243 whole multiples of 16.364 kg, 449 cycles in all, which
verified the cycle boundary bookkeeping against the schedule's own run
structure.

The references, computed in the probe from each home's HPXML by the cited
formulas. OpenStudio-HPXML v1.12.0 `calc_clothes_washer_energy_gpd` and
`calc_dishwasher_energy_gpd` (HPXMLtoOpenStudio/resources/
hotwater_appliances.rb:824-888 and :607-675, fetched from upstream and
checked against the latest release on 2026-10-08: v1.12 is current), the
ERI >= 2019A branch, the operational calculation (`NumberofResidents`
given: washer `scy = 123 + 61 n_occ`, dishwasher `scy = 91 + 30 n_occ`,
FSEC PF-464-15 Eqs. 1 and 3), `UsageMultiplier` applied, floored at 0.
OCHRE `parse_clothes_washer` and `parse_dishwasher`
(`vendors/OCHRE/ochre/utils/hpxml.py:1236-1278, 1336-1370`): the same
`cw_appl` and `kwh_per_cyc` algebra with the pre-Addendum-81 asset cycle
equations (`scy = 164 + 46.5 n_beds`, `dwcpy = 88.4 + 34.9 n_beds`) over
the occupancy-adjusted bedroom count (`hpxml.py:792-800`, single-family
detached: `max(-1.47 + 1.69 n_occ, 0)`); OCHRE spreads the daily volume
through the hot-water columns untempered
(`ochre/utils/schedule.py:350-370`, `ochre/Models/Water.py:286-290`), as
OpenStudio-HPXML does. Both references' daily volumes in gal/d and L/d:

| Home | Appliance | OS-HPXML | OCHRE |
|---|---|---|---|
| cz4a_ashp_hpwh (and cz2a) | washer | 2.5835 gal/d = 9.780 L/d | 2.7980 gal/d = 10.591 L/d |
| cz4a_ashp_hpwh (and cz2a) | dishwasher | 2.5629 gal/d = 9.702 L/d | 3.0307 gal/d = 11.473 L/d |
| bldg0176227 | washer | 7.4477 gal/d = 28.192 L/d | 8.5017 gal/d = 32.182 L/d |
| bldg0176227 | dishwasher | 3.7114 gal/d = 14.049 L/d | 5.1129 gal/d = 19.354 L/d |
| bldg0000002 | washer | 0.9741 gal/d = 3.687 L/d | 0.9295 gal/d = 3.518 L/d |
| bldg0000004 | washer | 3.2018 gal/d = 12.120 L/d | 3.0318 gal/d = 11.477 L/d |
| bldg0000004 | dishwasher | 1.8634 gal/d = 7.054 L/d | 1.5820 gal/d = 5.988 L/d |

(The OS-HPXML values equal the values B-17's acceptance test states for
the parity cz4a building, 2.584 and 2.563 gal/d, and for
`bldg0176227/home.xml`, 7.448 and 3.711 gal/d.)

HARES's realised draw, full-year runs, litres taken as kilograms:

| Home, step | Appliance | kg/y | L/d | cycles/y | mean cycle | min | max |
|---|---|---|---|---|---|---|---|
| cz4a, 60 s | washer | 10 008.00 | 27.419 | 206 | 48.583 | 36.000 | 144.000 |
| cz4a, 60 s | dishwasher | 7 707.27 | 21.116 | 243 | 31.717 | 16.364 | 98.182 |
| cz4a, 300 s | washer | 10 008.00 | 27.419 | 206 | 48.583 | 36.000 | 144.000 |
| cz4a, 300 s | dishwasher | 7 707.27 | 21.116 | 243 | 31.717 | 16.364 | 98.182 |
| cz4a, 900 s | washer | 10 008.00 | 27.419 | 206 | 48.583 | 36.000 | 144.000 |
| cz4a, 900 s | dishwasher | 7 707.27 | 21.116 | 243 | 31.717 | 16.364 | 98.182 |
| cz4a, 3600 s | washer | 10 008.00 | 27.419 | 206 | 48.583 | 36.000 | 144.000 |
| cz4a, 3600 s | dishwasher | 7 707.27 | 21.116 | 243 | 31.717 | 16.364 | 98.182 |
| bldg0176227, 900 s | washer | 6 246.00 | 17.112 | 277 | 22.549 | 18.000 | 54.000 |
| bldg0176227, 900 s | dishwasher | 11 376.82 | 31.169 | 360 | 31.602 | 20.455 | 94.091 |
| bldg0176227, 3600 s | washer | 14 364.00 | 39.353 | 276 | 52.043 | 36.000 | 144.000 |
| bldg0176227, 3600 s | dishwasher | 15 823.64 | 43.352 | 339 | 46.677 | 32.727 | 130.909 |
| bldg0000002, 900 s | washer | 1 557.00 | 4.266 | 83 | 18.759 | 18.000 | 36.000 |
| bldg0000002, 3600 s | washer | 3 780.00 | 10.356 | 83 | 45.542 | 36.000 | 72.000 |
| bldg0000004, 900 s | washer | 2 061.00 | 5.647 | 53 | 38.887 | 18.000 | 126.000 |
| bldg0000004, 900 s | dishwasher | 940.91 | 2.578 | 72 | 13.068 | 8.182 | 32.727 |
| bldg0000004, 3600 s | washer | 3 348.00 | 9.173 | 53 | 63.170 | 36.000 | 180.000 |
| bldg0000004, 3600 s | dishwasher | 1 816.36 | 4.976 | 70 | 25.948 | 16.364 | 49.091 |

Deltas. Against OpenStudio-HPXML at each home's 900 s step: cz4a washer
+180.4 %, dishwasher +117.6 %; bldg0176227 washer -39.3 %, dishwasher
+121.9 %; bldg0000002 washer +15.7 %; bldg0000004 washer -53.4 %,
dishwasher -63.4 %. Against OCHRE: +158.9 %, +84.0 %, -46.8 %, +61.0 %,
+12.1 %, -50.8 %, -54.0 % in the same order. An independent check of the
reference arithmetic corrected the two apartment cells: for the apartment
buildings OCHRE's occupancy adjustment is max(-0.68 + 1.09 n_occ, 0)
(vendors/OCHRE/ochre/utils/hpxml.py:795-797), not the detached form the
probe first used, so the OCHRE column for bldg0000002 washer is 3.518 L/d
(delta +21.3 %, was +12.1 %) and for bldg0000004 dishwasher 5.988 L/d
(delta -56.9 %, was -54.0 %). Every cell stays two orders of magnitude
beyond the 1 % bound; the verdict keys on the OS-HPXML leg and the step
leg, both untouched. The 900 s to 3600 s leg:
cz4a 0.00 % on both appliances (an hourly schedule upsampled by
zero-order hold keeps its non-zero runs at every step, so the draw is
step-independent there); bldg0176227 washer +130.0 % (17.112 to 39.353
L/d) and dishwasher +39.1 %; bldg0000002 washer +142.8 %; bldg0000004
washer +62.4 % and dishwasher +93.0 % (a 15-minute schedule aggregated to
3600 s by the column's mean merges runs across short zero gaps, so the
same physical cycles draw more, in fewer, longer events: bldg0176227's
washer drawing steps go 694 to 399 with the annual mass 6 246.00 to
14 364.00 kg).

Reading. The mechanism is as B-16 states: the replay holds
`phase_index` at 0, a water-drawing phase, across every step of a
contiguous non-zero run of the schedule's power series
(`event_load.rs` `extract_events_from_kw_series`, `locate`,
`update_outputs`), so a cycle's volume is the fixed rate times the run's
length, and the run's length is the schedule column's shape at the run's
step. The per-cycle volume is 3.2 to 9.6 times the nominal 15 L and 6 L
(cz4a washer mean 48.583 L) because the replay draws for the run's whole
length while the configured phases' water-drawing durations total
1 500 s (washer) and 1 320 s (dishwasher); the annual volume then misses
both references in both directions, set by the schedule columns' shape
rather than the appliance inputs. The rates confirm the litres-as-
kilograms resolution: washer 0.0100 kg/s, dishwasher 0.004545 kg/s.

Verdict: CONFIRMED. Both legs of the triage rule fire on every appliance:
every realised daily volume differs from the OpenStudio-HPXML volume by
15.7 % to 208.6 % (bound 1 %), and every 15-minute-schedule home differs
between 900 s and 3600 s by 39.1 % to 142.8 % (bound 1 %). B-17 runs.

### The conditioned attic, 2026-10-08, the last two OD-56 candidates triaged (branch `row/attic-film-triage` on `d1727998`)

The OD-56 row's two remaining candidates for the conditioned attic's
winter cold bias each get the reference reading and one decisive probe.
Baseline at the branch, both modes, all three scenarios: the six
`conditioned_oracle` tests pass and reproduce the recorded oracle bounds
exactly. Attic MAE, spring/summer/winter: dynamic 0.773/0.896/1.271,
ideal 0.791/0.899/1.299. Attic mean: winter HARES 3.55 dynamic and 3.57
ideal against OCHRE 4.77 and 4.82 (bias -1.22 and -1.25), summer HARES
29.47 against OCHRE 28.98 and 28.96 (bias +0.49 and +0.51). Note the
row's State carried the two modes' labels swapped: the 0.773 set is the
dynamic mode's and the 0.791 set the ideal mode's; the values themselves
match on both lists.

The full candidate set the investigation considered for this bias, each
with its disposition and where its evidence lives, so "the last two" is
reproducible: sky long-wave, ruled out because HARES's sky coupling
matches EnergyPlus's; roof absorptance and emittance, ruled out because
the same HPXML values sit on both sides; a radiant barrier, ruled out
because BEopt has none; ventilation and terrain, not the cause, about
0.1 °C of sensitivity; roof roughness, not the winter cause, about
0.05 °C there, the departure registered as D-010; deck and framing
mass, ruled down in the same review, the same mass look-up table on
both sides, low likelihood; the interior film and ground long-wave,
triaged in this entry, both reference-equivalent. The first six's
evidence is the fix-or18c review's candidate triage of 2026-10-05
(`verification/review-OR-18c-7b409ca5.md`, a review doc) and the OD-56
row's own record (`11-other-defects.md`, the row's record). The
free-float case's boundary conditions, the other half of the OD-56
row's scope, are untouched by this entry and stay with the row
(`11-other-defects.md`, the row's record, OD-56).

Candidate 1, the interior film on the roof deck and the attic floor.

The references. OS-HPXML v1.12.0 sets EnergyPlus's inside surface
convection algorithm to TARP (`HPXMLtoOpenStudio/resources/simcontrols.rb:24-25`,
the outside one to DOE-2 at `:21-22`). Its own constructions name
air-film layers (the roof's `AirFilmIndoorRoof` at `constructions.rb:18`,
the attic floor's `AirFilmIndoorFloorAverage` at `:165`, added to the
construction at `:405`, `:508`, `:607`), but it strips the AirFilm
layers before a construction reaches EnergyPlus
(`constructions.rb:3319`): the EnergyPlus film is not a material layer,
it is the dynamic convection algorithm those settings select, evaluated
per step. EnergyPlus v24.2.0 evaluates the
TARP interior model per surface face per step: the dispatch at
`ConvectionCoefficients.cc:319-328`, `CalcASHRAETARPNatural` at
`:1879-1925` (enhanced when the surface-air temperature difference and
the face's signed cos of tilt share a sign, reduced otherwise,
`:1910-1922`), the interior face taking the negative of the exterior cos
at `:1938-1940`, and the Walton magnitudes at
`ConvectionCoefficients.hh:409` (vertical, 1.31·ΔT^(1/3)), `:433`
(enhanced, 9.482·ΔT^(1/3)/(7.238-|cos|)) and `:455` (reduced,
1.810·ΔT^(1/3)/(1.382+|cos|)). The vendored EnergyPlus tree is pinned
at `f7c103167a`, 110 develop commits past v26.2.0-IOFreeze, and the
Walton/TARP code is identical from v24.2.0 through that pin (re-checked
against the vendored copies: only formatting differs), so the v24.2.0
reading carries over at the pin's lines: the
dispatch at `ConvectionCoefficients.cc:331-342`, `CalcASHRAETARPNatural`
at `:1902-1948` (the sign logic `:1933-1945`), the negative cos at
`:1961-1963`, the Walton magnitudes at the same `.hh` lines `:409`,
`:433`, `:455`. OCHRE freezes the same TARP model once
per boundary at the clamped ΔT of 12.9 K
(`ochre/utils/envelope.py:342-402`, the clamp at `:374`, one value for
both faces): the committed oracle fixtures carry the frozen interior
films 0.5611 (attic floor, both faces) and 0.5363 (the deck's attic
face), the formula's values 1.782 and 1.865 W/(m²·K), constant across
the seasons' fixtures as a frozen film is.

HARES. The A-matrix freezes the ASHRAE Simple values instead
(`crates/hares-physics/src/film_coefficients.rs:444-460`: the attic
floor 0.948 both faces, R 1.055; the deck's attic face 2.281, R 0.438),
and the per-step TARP correction is wired but disabled
(`crates/hares-core/src/dwelling/solver_builder.rs:1248-1252`). The
frozen non-vertical baseline is the registered divergence D-031 (class
(a) against OCHRE), pinned by
`the_attic_floor_s_frozen_films_are_the_ashrae_simple_horizontal_values`;
D-031's justification's `DataHeatBalance.hh:613` and `:1842` are the
vendored pin's lines, the file unchanged from v26.2.0-IOFreeze
(v24.2.0 has the same defaults at `:588` and `:1780`).

Probe 1 (freeze OCHRE's TARP form on the deck's attic face and the
attic floor, `tarp_h_natural` at the clamped 12.9 K in place of the
Simple values for those two boundaries only; the ideal mode's numbers
throughout): winter attic mean 3.57 to
3.71 (bias -1.25 to -1.11), MAE 1.299 to 1.138; summer mean 29.47 to
29.30 (bias +0.51 to +0.34), MAE 0.899 to 0.563; spring MAE 0.791 to
0.589. This reproduces the D-031 record (measured at `62ba224b`) and
extends it to the summer case.

Probe 2 (the per-step TARP model enabled on every interior face,
`solver_builder.rs` `FilmCoefficientModel::PerStepTarp`; the means and
MAEs are the ideal mode's, the roof-gain figures the dynamic mode's):
winter mean 3.57 to 3.54 (ideal; MAE 1.299 to 1.324), summer mean 29.47
to 29.44 (ideal; MAE 0.899 to 0.910), spring MAE 0.791 to 0.789 (ideal);
the run stable at the 60 s step. The correction fires on the deck (the
winter roof heat gain -266.7 to -359.0 W against OCHRE's -231.2, the
dynamic mode's figures) and barely on the floor, and the attic
moves 0.03 °C.

The stable 60 s run bears on the registered reason the switch is
disabled: `solver_builder.rs:1248` pins AshraeSimple and `:1249-1252`
registers the disable under T-0034, citing T-1922's Courant-condition
constraint claimed to block PerStepTarp at the 60 s step with typical
surface-layer capacitances.
The probe is evidence that the constraint does not bite at the oracle
fixtures' capacitances: the registered blocking reason does not hold
there, and the probe says nothing about any other capacitance. The
switch stays disabled: enabling it is a code fix, and the triage's rule
fixes nothing without a failing test pair and a reference rule.

Verdict: reference-equivalent for the bias, no defect. The films'
leverage on the winter bias is at most 0.14 °C on the two boundaries
probed, the deck's attic face and the attic floor (probe 1), and the
OS-HPXML-literal per-step form moves it 0.03 °C the other way (probe
2); neither explains the -1.25. The slab's conditioned face is in the
same film class and was not probed. The frozen-baseline divergence
stays D-031's, class (a).

Candidate 2, ground long-wave exchange.

The references. EnergyPlus v24.2.0's outside-face long-wave has a ground
view whose temperature is the outdoor dry-bulb:
`HeatBalanceSurfaceManager.cc:9197` (`TGround =
state.dataEnvrn->OutDryBulbTemp`), the ground share terms at `:9388`
and `:9396`, the view split
`ViewFactorGroundIR = 1 - ViewFactorSkyIR` (`SolarShading.cc:10362`),
the same form on windows (`DataSurfaces.cc:318-323`); the vendored
tree at its pin `f7c103167a` carries the same code at
`HeatBalanceSurfaceManager.cc:9764` (v26.2.0-IOFreeze itself has the
ground view at `:9701`: the pin sits 110 develop commits past the tag
and this file's lines shift in between), the ground share at `:9955` and
`:9963`, `SolarShading.cc:10694` and `DataSurfaces.cc:320-325`. OCHRE
radiates
only outdoor-facing surfaces (`ochre/Models/Envelope.py:186`, `:772`)
and sends the non-sky share to the outdoor-air temperature
(`Envelope.py:145-149`). HARES: the same four-component form with the
ground share at the outdoor-air temperature
(`crates/hares-envelope/src/longwave_radiation.rs:194-196`, pinned by
`test_4component_matches_energyplus_reference`; the injections at
`crates/hares-envelope/src/thermal_solver/longwave.rs:220-221` and
`:299-304`), and ground-facing boundaries take no exterior long-wave at
all (`solver_builder.rs:148-152`, `:1011`).

Probe 3 (upper bound: the whole non-sky share at the EPW shallow ground
temperature on every opaque exterior face, a term neither reference
has; the dynamic mode's numbers throughout): winter mean 3.55 to 3.64
(bias -1.22 to -1.13), MAE 1.271 to
1.174; summer mean 29.47 to 29.26 (bias +0.49 to +0.28), MAE 0.896 to
0.868; spring MAE 0.773 to 0.786. The term carries 0.1 to 0.2 °C of
leverage on this scenario and would diverge from OS-HPXML, OCHRE and
EnergyPlus at once.

Verdict: reference-equivalent, no defect. No ground-temperature
long-wave term exists on either side of the oracle; the D-018
ground-temperature difference reaches the building by conduction, where
D-018 owns it, class (a).

Reading. Both remaining candidates are classed and neither establishes
the winter cold bias: it stays open at -1.25 (ideal) and -1.22
(dynamic) with its cause unestablished. The probes were temporary
patches at this branch, each reverted after its run; the oracle bounds
reproduce unchanged.

### The rename row's consumer gate captures, 2026-10-09 (consumer branch `row/cn13-follow-rename`, gate engine `b2419945`)

Four DE-trace captures with the CN-0 tool, this host, the production
set unchanged:

- The shipped baseline `B0-de-trace.json`: captured 2026-10-07 at
  consumer `3840751` on engine `27db4a1e`.
- The old consumer (`78568b5`, importing the old name) against the
  pre-rename engine `97e96368`: differs from the shipped baseline on
  260 field paths (every premise's trace, fit and score). The
  baseline's engine sits on the audit line that diverged at
  `ebdb1a17`; `27db4a1e` is not an ancestor of the phase-1 line the
  rename sits on, and the difference is already fully present at the
  pre-rename engine. The rename is not its cause.
- This consumer at the renamed engine `f35eef8a` (the pure rename
  commit): `compare` prints `identical` against the previous capture,
  bitwise. The rename moves no number through the consumer's
  production capture: old consumer and pre-rename engine versus this
  consumer and the renamed engine is the only variable pair, and it
  compares equal.
- This consumer at the gate engine `b2419945` (rows 28 to 30b and the
  shipped-parquet weight fix on top of the rename): `identical` to the
  rename-commit capture, and the capture ran twice (`identical`), so
  rows 28 to 30b are bitwise for the consumer and the capture is
  deterministic.

The gate: 348 passed, 0 failed, 7 skipped (the base seven environment
skips; the two new acceptance cases are the +2), ruff clean, the
contract check PASS on the schema regenerated against the gate engine
(engine sha256 prefix `da6f1391ab79bcc0`, 84 witnessed homes, 9 of 9
cross-check agreements), the bundle VERIFY clean (exit 1 on the base
skips, as the gate records). The cache moved from `~/.cache/ochre_next`
to `~/.cache/hares` under the operator checkpoint: 6741 files, sha256
manifests identical before and after, the source not nested in the
target. The engine worktrees `97e96368` and `f35eef8a` remain under
`.workstrees/`; the old-consumer checkout used for the pre-rename leg
is `/tmp/opencode/consumer-base` (a worktree of the consumer at
`78568b5`).

Correction, 2026-10-09 (the independent review's required finding). As
committed, the consumer tree at `70ebbfc` does not resolve its engine
extra: the submodule pin it carries, `5ece4060`, predates the package
rename, and that pin's `pyproject.toml` still names the old package, so
`uv sync --extra engine` fails at the committed state with "Package
metadata name ochre-next does not match given name hares-py". The
committed tree resolves its engine extra only once the pin bump lands,
and the pin bump follows the engine folds (the engine rows on this
branch fold first); it is the row's closing step. Every gate recorded
above therefore ran against the gate engine `b2419945` by editable
install of that checkout, not against the pinned tree. The independent
review re-measured the engine at the gate engine, by editable install:
`cargo nextest run --workspace --all-features` 7799 passed (7718 in
the default profile), `uv run --no-sync pytest tests/python` 1520
passed.

### The row 33 stop condition, 2026-10-09, the OCHRE parity tolerances on the held weather (branch `row/r33-in6`, worktree of `56ae417e`, uncommitted working tree)

Load average at write time: 13.60 (1 min).

The entry's stop condition fired: a `test_ochre_parity.py` metric left
its existing tolerance, so the entry stopped before committing and the
operator decides. The mechanism is the one the entry's stop condition
names: the old override hold was applied inside the engine's resampling,
whose weather indexing carries today's half-hour shift
(`midpoint_offset_secs` 1800, the same 30-minute lag OCHRE's
midpoint-stamped `resample().ffill()` produces; the sides agreed step
for step, which is why the 1-hour cooling case passed at 1e-9 before).
The held file is read through the interval-end label at its own interval
(`midpoint_offset_secs` 1800/N per the entry: 30 s at N=60), so the
HARES weather timeline now runs about 29.5 minutes ahead of OCHRE's.
The held values are the same zero-order hold (the committed-file
equality test proves the bytes; the record order is the hold), so this
is a pure phase shift of the weather grid, not a different value.

Measured on the branch (Denver, window 2019-05-05 13:00 to 14:00 local,
1-minute steps, both sides seeded 42):

- 1 h, Total Electric Power: OCHRE 0.4015574052175426 kWh, HARES
  0.3773907385508761 kWh, relative error 6.018e-02, tolerance 1e-9.
- 1 h, HVAC Cooling Electric Power: OCHRE 0.025 kWh, HARES
  0.0008333333333333334 kWh, relative error 9.667e-01, tolerance 1e-9.
- 24 h record check, HVAC Heating Electric Power: HARES measured
  31.6769 kWh against its pinned record 32.5037 kWh, 2.5% off, record
  band 2%.

What held within its existing bounds: the Rust parity corpus
(`parity_outputs_against_reference_corpus`, every fixture within its
drift-locked bands against the OCHRE reference parquets), the
conditioned, freefloat and envelope oracles, `envelope_opaque_loads`,
`conditioned_basement`, the remaining 59 OCHRE-marked Python cases and
the other 24 h record checks.

The goldens are bitwise in both profiles (compare-all and -F dst), and
the engine's own resampling defaults are unchanged: the tests that
pinned the default methods' exact values passed through the refactor
untouched, and no golden uses the overrides.

Options for the operator (a tolerance is never widened inside this
structural entry): (a) re-capture the 1 h/24 h parity records and
tightened windows with the delta and the phase-shift cause recorded
here, keeping the strict band on every case that still discriminates;
(b) have the comparison feed OCHRE the held file too (its midpoint
stamps at the file's own interval then agree with the engine's
interval-end reading step for step), which changes the reference side
of the comparison and is outside this entry's rule; (c) park the row.
The working tree keeps the full implementation, uncommitted, with the
block green except the three cases above.

### The frozen row plan's per-step column access, 2026-10-09 (branch `row/r37-or2`, engine `084294dd` on `b17ed658`)

CORRECTED the same day, on review: the first version of this record read
about 390 ns against about 4.4 ns and claimed about 89x. Both sides were
artifacts of the bench, which a reviewer found: the before side allocated
its row buffer inside the iteration loop, while the real old code reused
the roster-held `record_scratch` and paid no allocation; and the after
side observed one slot of the row (`row[0]`, a slot the loop itself
writes), which let the compiler dead-store-eliminate `begin_row`'s fill.
The bench in the review-fix commit replaces both: the before side reuses
its buffer across iterations exactly as `record_step` reused
`record_scratch`, and both sides observe the whole row (an xor over every
slot's value bits, the read the recorder's push performs in production).
The numbers below are that corrected bench's.

Criterion, `cargo bench -p hares-core --bench output_row_plan` (20
samples, 1 s warm-up, 3 s measurement, this host, WSL2):

- `per_step_hash_lookup`: the step path's before mechanism, one step's
  worth of output work on the verbosity-2 schema of a six-equipment
  two-zone model: the reused row's fill, then 28 column-name lookups
  against the freeze-time name-to-position map (the totals, the weather
  and setpoint context columns, and every envelope gain name, hits and
  misses alike), writing the hits. Median about 320 ns per step (318 to
  323 ns across three runs).
- `frozen_row_plan_write`: the after mechanism, the same one step's
  worth: the frozen plan's `begin_row` (fill, validity clear, UTC epoch
  timestamp) and the same hit slots written by index, the whole row
  observed. Median about 10.1 to 13.6 ns per step across three runs.

The honest apples-to-apples ratio is about 24x to 32x (the medians sit
near 25-30x), not 89x. The lookups cost under 1 percent of a full-year
900 s run's wall (about 14 ms of the 1.5 s full-year baseline wall), so
the end-to-end effect sits inside noise; the per-step win compounds with
the reporting work that follows on this path. The goldens are bitwise
after the change (compare-all 7 manifests identical,
`full_year_900s_dst` identical), so no value moved.

### The diagnostics report's per-step recording, 2026-10-09 (branch `row/r41-or4`, engine `81448376` on `c9c3399e`)

Criterion, `cargo bench -p hares-core --bench diagnostics_report` (new in
this row; 20 samples, 2 s warm-up, 6 s measurement, this host, WSL2).
Both sides run the same one-day two-zone model (the cz4a parity fixture,
verbosity 2, output on, 1440 steps of 60 s); the only difference is the
diagnostics request.

- `without`: the run requests no diagnostics, so no diagnostics code runs
  in the step: no schema column, no capture, no accumulator. Median
  about 199.0 ms per day, about 138 µs per step.
- `with`: the run records the diagnostics block (20 columns on this
  model): the zone and port reads, the plan-row writes and the batch
  building. Median about 201.7 ms per day in the same process, the
  pair's delta about 2.7 ms per day.

CORRECTED the same day, on review: the first version of this record read
that one pair's delta as a per-step cost, about 1.9 µs per step, and
called it the honest figure. The bench cannot resolve a cost that
small. Re-running the paired benches gave deltas of about +150, -1.7
and +20 ms per day: the sign flips, so the 2.7 ms pair was one sample
of this noise, not a stable estimate. The bench's own order biases it
besides: the two benches run in one process in a fixed order, and the
second inherits the first's warmed state, so the pair's delta carries
the order's effect along with anything it measures. The magnitude is
therefore recorded as a bound, not a point estimate: the with-request
recording's per-step cost is within this host's noise on this bench.
The observe profile's cross-process medians sit in the same noise (with
209.8 ms against without 229.3 ms in one run, 233.1 against 231.2 in
another). What the row holds is the structural claim: with no request
no diagnostics code runs in the step (no schema column, no capture, no
accumulator), proven by the code path and the tests (the schema pins
hold byte-unchanged without the request, and the pin fixture's
differential holds row for row). The goldens are bitwise in every
profile after the change (the diagnostics columns exist only under the
request), so no value moved.

### The Time column's string formats retire, 2026-10-10 (branch `row/r45-or7`, engine `7652eecb` on `7e95e974`)

The output Time column's dtype moves: Utf8 (an RFC 3339 string with the
step's offset, formatted per row with chrono `%+` and copied per row in
`StreamingRecorder::push_row`) to `Timestamp(Millisecond, None)` (the
row's naive local wall-clock milliseconds, written once per row as an
`i64`). The cause is the schema change itself: one time convention
across the surface, the local wall clock in the column and the site's
standard-time UTC offset in the schema metadata `hares:utc_offset_s`
(UTC = Time - offset). No value column moved.

Golden movement (cause: the declared dtype change; the values' wall
clocks are the same instants the strings spelled): the six dwelling
frames' Time column digest moved and the schema's Time field reads
`Timestamp(ms)`; every other column's digest is bit-identical; row
counts unchanged. The two fleet manifests compared identical (their
aggregate Time stays a Utf8 RFC 3339 UTC string; the aggregate products
are unchanged and were not re-captured). The re-captured files are
byte-identical across two captures at `7652eecb`.

Per-row string work for a full-year run, before and after: before, two
string operations per recorded row (the `%+` format! into the reused
buffer plus the push_row copy); after, zero (one `i64` push). A
full-year run records 8,760 rows at 3,600 s (`full_year_hourly`,
17,520 calls before) and 35,040 rows at 900 s
(`full_year_900s_dst`, 70,080 calls before). The fleet aggregation's
per-row `parse_timestamp` (one RFC 3339 parse per home row before) is
also gone: the fleet reads the timestamp's milliseconds and applies the
home's metadata offset. No wall-time bench was run: the work removed is
linear in rows and the goldens prove the outputs equal.
### The run metrics' per-step cost with and without the request, 2026-10-10 (branch `row/r46-or8`, worktree of `7e95e974`)

Criterion, `cargo bench -p hares-core --bench metrics_request` (new in
this row; 20 samples, 2 s warm-up, 6 s measurement, this host, WSL2).
Both sides run the same one-day two-zone model (the cz4a parity fixture,
verbosity 2, one memory report, 96 steps of 900 s, no warm-up); the only
difference is the metrics request.

- `without`: the run requests no metrics, so no metrics code runs in the
  step: no input plan, no accumulator, no feed. The step pays one
  `Option` branch.
- `with`: the accumulator is fed per step from the row plan's producer
  slots and its arithmetic runs (the end-use and envelope reads, the
  rolling peak window, the comfort and unmet checks).

Three paired runs of the bench: without 25.1 / 21.2 / 22.7 ms per run,
with 23.3 / 21.5 / 22.4 ms. The pair's delta flips sign across runs
(-1.8, +0.2, -0.3 ms per run), and the run-to-run spread of the medians
itself (about 4 ms) is an order above the deltas. The bench cannot
resolve a per-step cost this small: 96 steps per run puts the deltas
under 4 microseconds per step even before the noise is subtracted, and
the honest figure is a bound, not a point estimate: the metrics feed's
per-step cost is within this host's noise on this bench, and the share
of `simulate()` wall time the requested metrics take is not resolvable
here (under the noise floor on a one-day run). What the row holds is
the structural claim: with no request no metrics code runs in the step
(no input plan, no accumulator, no feed), proven by the code path and
the tests (`metrics_absent_unless_requested`, the in-crate pin that the
dwelling holds no accumulator without the request). The consumer's fit
path later gets exactly this shape: no request, no cost.

### The unread checkpoint serialisation leaves `initialize()`, 2026-10-10 (branch `row/r47-or9`, commit `9db43ee7` on `16b3c993`)

Quantity: `PyDwelling.initialize()` wall time, before and after the
deletion of the unread checkpoint serialisation. Command: the release
wheel (`uv run --locked --all-extras --all-groups maturin develop
--release --uv`), a script timing `initialize()` alone over 41 fresh
dwellings (the ochre_samples `base.xml` fixture, the Denver example
weather and BEopt schedule, 3600 s at 60 s, seed 0), construction
untimed, one warm-up pass first; median per run, three runs per side.
Load average at write time: 5.11 (1 min) before, 19.95 (1 min) after
(another session's build on the host; the per-run minima, 50 to 54
microseconds before and 1.9 to 2.0 after, are the load-free floor).

- before: medians 0.0641, 0.0584, 0.0598 ms per call.
- after: medians 0.0029, 0.0030, 0.0035 ms per call.

The serialisation dominated the call: about 60 microseconds against
about 3, about 20x lower, and what remains is the Python call and the
flag set. The behavior is unchanged: the checkpoint the field held was
never read, the dwelling's state and every step's results are the same,
and the goldens are bitwise (compare-all 7 manifests identical,
`full_year_900s_dst` identical).

### JSON floats parse exactly in every build: the three proofs, 2026-10-10 (branch `row/r47-or9`, commit `f81618c8` on `16b3c993`)

The manifest change itself already sat on the row's base: the phase-1
fold `c561eb69` unified `float_roundtrip` into the workspace
dependency and took `tools/frame_golden`'s own feature line out, so the
branch's manifest diff is empty and the commit lands the acceptance
test and the proofs. Load average at write time: 2 to 5 (1 min).

1. The goldens: `frame_golden compare-all` on the default build, 7
   manifests identical (`consumer_shape_900s`, `fleet_mixed_offset`,
   `full_year_hourly`, `leap_year_february_900s`,
   `month_boundary_billing_3600s`, `resstock_2025_fleet`,
   `weather_wrap_900s`), and `full_year_900s_dst` identical on the dst
   build.
2. The consumer: a worktree of the B0 engine commit `1d60f478` with
   only the manifest change applied (one line:
   `serde_json = { version = "1", features = ["float_roundtrip"] }`),
   built through the engine-under-test mechanism (release, the
   maturin 1.15.0 constraint from that commit's own `uv.lock`,
   editable into the B0 consumer worktree `aec2e3f`'s venv) and run
   against the B0 consumer `aec2e3f`: the gate `run_all` 332 passed,
   0 failed, 7 skipped, the skip set exactly the base seven; the
   contract check `[hares-contract] PASS`; `ruff check` clean. The
   DE-trace capture compares identical (exit 0) to the row-16g sweep's
   base sanity capture of `1d60f478`
   (`/tmp/opencode/i08/b0-1d60f478.json`, consumer `18a8ec3`), and the
   same-consumer before/after pair is identical too: the feature
   stripped from the worktree, rebuilt, re-captured
   (`capture-before.json`), the two captures compare identical, so the
   only delta between the pair is the feature and it moved nothing.
   The shipped `B0-de-trace.json` baseline could not serve as the
   compare target: its header names engine `27db4a1e`, consumer
   `384075185`, a capture from the divergent audit line (the
   divergence recorded at the rename row), and its payload carries
   consumer fields (`first_rejection`) that postdate `aec2e3f`; the
   compare against it exits 1 on that structural difference before any
   number is reached.
3. The Python suite: the five tariff test files
   (`test_tariff_builder.py`, `test_tariff_evaluator.py`,
   `test_tariff_integration.py`, `test_tariff_telemetry.py`,
   `test_ev_tariff_integration.py`) pass identically without the
   feature (79 passed, a temporary no-feature wheel) and with it (79
   passed), so the values they read are equal before and after.

The acceptance test's RED/GREEN: with the feature stripped from the
workspace dependency, `json_floats_round_trip_bitwise` failed on
`std::f64::consts::PI * 1e-30` parsed one ULP off the value written
(bits 4165790117795483861 against 4165790117795483862); with the
feature, the suite passes (197/197 in `hares-tariff`).

### The checkpoint resume row's checkpoint size and save wall, 2026-10-11 (branch `row/r48-or10`, tip `6a25ec97`)

The measurement the checkpoint resume row's entry promises: the
checkpoint's size in bytes and the `save_checkpoint` wall time for a
full-year fixture, before and after, re-measured to confirm the figures
the independent review took (284 KB at 1 week, 3.2 MB at a quarter,
6.3 MB at half-year, 13.9 MB at a year, save about 8 ms at year-end).

Method: a temporary release-built `hares-core` example (deleted after
the run, never committed) builds the cz4a parity fixture
(`tests/fixtures/parity/cz4a_ashp_hpwh`, start
`2023-01-25T00:00:00-07:00`, duration 31 536 000 s at 900 s, 35 040
steps, `metrics = true`, no warm-up) with one verbosity-2 memory
report, steps `Dwelling::step()` 35 040 times in one continuous run,
and saves a checkpoint at each milestone: 1 week (672 steps), a quarter
(8 736), half-year (17 472) and the year (35 040). `save_checkpoint()`
is timed with `Instant` (the state capture the API performs); the size
is the written file's (`DwellingCheckpoint::save`, the JSON bytes with
their checksum line); five year-end saves give the wall-time median.
The "before" side runs the same harness on a temporary worktree of the
parent `16b3c993`, whose checkpoint carried physics and tariff state
but none of the run-surface sections. Load average at write time: 10 to
14 (1 min; the host was busy).

Before (parent `16b3c993`): 6 720 bytes at 1 week, 6 840 at a quarter,
6 853 at half-year, 6 899 at the year, constant in the recorded rows,
capture under 0.1 ms.

After (this branch): 283 510 bytes at 1 week, 3 562 797 at a quarter,
6 957 553 at half-year, 13 946 305 at the year. The growth is linear in
the recorded rows, about 400 bytes per row (the frame's every row's
timestamp and value columns in JSON text): that is the tradeoff the
row's entry states, the checkpoint carries every memory report's row so
the resumed run's frames continue bitwise, and the price is a
checkpoint proportional to the recorded rows. The `save_checkpoint()`
capture runs 6.5 ms median at year-end (five saves, 4.6 to 8.1 ms); the
file write a caller adds on top (`cp.save()`, the JSON serialization
plus checksum) runs about 50 to 84 ms at year-end sizes.

Against the review's figures: week and year agree to rounding (284 KB,
13.9 MB), quarter and half-year read slightly higher here (3.6 MB and
7.0 MB); linearity holds in every recorded row on both sides, and the
sizes are exact (all five year-end saves byte-count identical).

### The streaming fleet fold's golden homes, wall time and memory bound, 2026-10-11 (branch `row/r50-fl1`, worktree of `16b3c993`)

**The golden homes' premise record (FL-2's premise).** Every home of
both fleet goldens, run standalone at its own building id, seed and
inputs (the manifest's sim config: `fleet_same_offset_300s` at
2018-01-01T00:00:00-08:00, two days at 300 s, no warm-up;
`fleet_mixed_offset` at 2018-01-01T00:00:00-06:00, seven days at
3600 s, the converging warm-up): every home carries an empty
`RunResult.warnings` (zero dropped) and every `RunHealth` counter zero
(`port_rollbacks`, `rejected_control_signals`, `clamped_actions`,
`curve_index_clamps`, `fluid_payload_absences`, `evicted_actors`). The
same_offset homes' warm-up is disabled; the mixed_offset homes' warm-up
leaves no warning. The premise FL-2's bitwise claim rests on holds:
every golden home is clean.

**The goldens' re-capture (the second commit).** `fleet_same_offset_300s`:
the `aggregate` product (36 columns, 192 rows) and the `metrics` product
compare bitwise with the first commit's capture (captured on the parent
code's collect-then-aggregate fold); the `covered_weight` and `homes`
products are new shapes (the first commit carried the `weights`
stand-in) and were captured in the second commit. `fleet_mixed_offset`:
the `aggregate` stays the empty schema (zero rows: its three homes
declare different variable sets, the mismatch empties the aggregate
until FL-2) and `metrics` compares bitwise; `covered_weight` (zero rows,
same empty schema) and the new-shape `homes` captured here.
`resstock_2025_fleet` was re-captured too: its `aggregate` (zero rows)
and `fleet_report` products compare bitwise, its `homes` product takes
the new shape, its `metrics` product goes from three rows to zero (the
ResStock constructor's homes request no run metrics; per-home metrics
are computed only when the home's `SimulationConfig` requests them) and
its `weights` stand-in is replaced by the zero-row `covered_weight`.

**Wall time (the `perf:` commit's rule).** `frame_golden run`, five
fresh runs, median, this host, WSL2, the release build at the parent
(16b3c993's collect-then-aggregate) and after (the streaming fold at
chunk 64, the Python default):

- `fleet_same_offset_300s`: parent median 0.413 s (0.399 to 0.474);
  after 0.405 s (0.394 to 0.429). Within the 5% band, on the fast side.
- `fleet_mixed_offset`: parent median 0.252 s (0.246 to 0.263); after
  0.253 s (0.249 to 0.256). Within the 5% band.

**The memory bound (the new `fleet_fold` bench group,
`cargo bench -p hares-core --bench fleet`).** Copies of the base
fixture at 24 h and 900 s steps, folded at the hour, 8 threads; peak
RSS (VmHWM) read after the runs, the bench's chunk size in the
benchmark id. The chunk bound's effect is the pair at one fleet size:
the one-chunk shape (`chunk_1000`, the parent commit's collect
equivalent) holds every home's result at once; `chunk_64` holds at most
64.

- 100 homes: `chunk_64` 66.1 MB VmHWM (one chunk at this size).
- 1000 homes: `chunk_64` 71.7 MB; `chunk_1000` 71.8 MB. 1.3 s either way.
- 10000 homes: `chunk_64` 104.0 MB; `chunk_1000` 158.3 MB. 12.5 s vs
  12.4 s: the chunk barrier costs nothing measurable, and the bound
  saves about 54 MB of resident home results at ten thousand homes.
- The bookkeeping slope at `chunk_64` from 1000 to 10000 homes is about
  3.6 kB per home (the entry list, the homes row and the fold's running
  cells share it with the criterion harness's own footprint; the pair
  difference isolates the home results).

### The fleet fold's memory bound, re-measured in fresh processes, 2026-10-11

A reviewer found the memory-bound record above artifactual: its pairs
were read out of one criterion process in which both chunkings and every
fleet size ran sequentially, so the second benchmark's VmHWM inherited
the first's high-water mark and the samples' own accumulation, and the
ten-thousand-home `chunk_1000` reading was that inherited peak, not the
one-chunk shape's own.

Re-measured soundly, one fleet run per fresh process: the bench gained a
one-shot mode, `HARES_FLEET_FOLD_ONCE=<chunk_size>/<homes> cargo bench
-p hares-core --bench fleet`, which runs the fold once and prints the
run's wall seconds and the VmHWM read from `/proc/self/status` at run
end (the bench's `peak_rss_kb` reads `/proc/self/status` now). Five
fresh processes per pair, medians, the same fleet shape as before
(copies of the base fixture at 24 h and 900 s steps, folded at the hour,
8 threads):

- 100 homes: `chunk_64` 66.2 MB, `chunk_1000` 66.5 MB VmHWM; wall 0.14 s
  either way.
- 1000 homes: `chunk_64` 68.9 MB, `chunk_1000` 69.0 MB; wall 1.33 s vs
  1.28 s.
- 10000 homes: `chunk_64` 85.1 MB, `chunk_1000` 85.3 MB; wall 13.20 s vs
  12.84 s.

The honest delta: none measurable. The two chunkings' peaks agree within
the runs' spread at every size, because a home's result in the fold is
its reduced period table at the report's frequency (a few kB per home at
the hourly frequency), so the chunk bound has nothing to hold back at
these sizes. The chunk barrier's wall cost at ten thousand homes is
+0.36 s of median (2.8%), inside the 5% band: nothing measurable. The
bookkeeping slope at `chunk_64` from 1000 to 10000 homes is about
1.8 kB per home. The earlier record's "saves about 54 MB" and its
104.0-against-158.3 MB pair were the artifact; the bound's honest
evidence is structural (`peak_live_home_results` pinned at 1 and 2)
plus the reduced table's bytes below.

One home's reduced table bytes (`RecordBatch` columns'
`get_array_memory_size` summed, one `base.xml` home at 24 h and 900 s
steps, verbosity 0, output chunk 10 000): at `Timestep`, the home's
selected-variable frame, the shape the parent commit's collect held per
home (the fold's reducer refuses the timestep frequency, so the frame
comes from a standalone memory report), 96 rows, 480 576 bytes; at
`Hourly`, the fold's reduced table read as the one-home fleet's
aggregate, 24 rows, 6 columns, 46 684 bytes. The batch numbers carry the
builders' buffer capacity; the fold's live hold is the reduced table
itself, a few kB per home.

The callback destination in fleet homes, stated correctly: the row's
entry said each home runs the reports of its own `SimulationConfig`; for
a `Destination::Callback` report that is wrong. Through `Fleet::simulate`
the dwellings are built internally, so nothing can attach the report's
callback, and a run now refuses a callback-destination report with
nothing attached at its first step (the refusal latches and names the
report) instead of streaming its batches to nowhere: a home config
carrying `Destination::Callback` fails that home at run start, its
fellows unaffected. Pinned at the engine (the first step refuses) and at
the fleet (the home's status is `failed` with 0 steps).

### FL-2's golden re-capture and fixture corrections, 2026-10-11 (branch `row/r51-fl2`, worktree of `5a9c4ac8`)

**The fleet goldens' changes.** Two of the eight goldens move, both in
the way the entry's verification paragraph declares and its closing
premise paragraph governs:

- `fleet_mixed_offset`: the `aggregate` and `covered_weight` products go
  from zero rows (the three homes declared different variable sets and
  the mismatch emptied both) to 165 rows, the union of the three homes'
  selected variables, 46 columns, over the 165 UTC instants every home's
  hourly grid shares. The `homes` product's `status` values do not
  change spelling: the entry expected them to, but FL-1's premise record
  found every mixed_offset home clean (no warning, no non-zero health
  counter, the converging warm-up), so under the new mapping every home
  is `ok`, as it was `ok` before. The only homes change is
  `periods_folded`, 0 to 165 per home: the fold's period count is now
  the union aggregate's 165 rows, where the emptied aggregate counted 0.
- `resstock_2025_fleet`: the fleet's three homes are not all clean, so
  two leave the fold. `bldg0162486` is degraded naming the envelope
  warning ("boundary 'Wall1': the outside Siding layer's 'none' names no
  material; it is treated as absent and takes the roughness of
  OS-HPXML's default for an absent element, wood siding,
  defaults.rb:1338, :1379") and `bldg0352714` is degraded naming the
  hpxml warning ("the attic has 28.6 m2 of gable walls, but its roofs
  are not one gable; its volume is OS-HPXML's square hip"). Neither
  event is a defect: both are the row-21i chain's cited-default and
  cited-derivation warnings on the vendor's own inputs. The two homes'
  `status` values change spelling `flagged` to `degraded`, their
  `reason` values carry the warning text instead of "1 warning(s)", and
  `bldg0019694` (the one clean home) takes `periods_folded` 48, folding
  alone: the `aggregate` and `covered_weight` products go from zero rows
  to 48 rows, 28 columns, the clean home's tier-2 set over its 48 hourly
  instants, covered weight 253.903673 in every cell. The `metrics`
  product stays zero rows; `fleet_report` compares bitwise.

Every other product of every other manifest compares bitwise, including
all four products of `fleet_same_offset_300s` (every home clean, the
single column set folding exactly as before) and the six dwelling
goldens (the fleet changes touch no dwelling path). Both re-captures ran
twice, the golden file's sha256 identical each time
(`fleet_mixed_offset` `4d22c64f60418710`, `resstock_2025_fleet`
`f8da5ae94527933a`).

**The fixture corrections the row's acceptance tests needed.** The
entry's acceptance tests first assert that every home they expect to be
ok is ok, so a warning a fixture carries fails the test naming it. Two
fixtures carried one:

- `tests/fixtures/hpxml/ochre_samples/base.xml` and `base-battery.xml`:
  each carries one added element, `<ShieldingofHome>normal</ShieldingofHome>`
  in the `BuildingDetails` `Site`, naming the value OS-HPXML's cited
  default already applies (defaults.rb:822-830); the warning ("the
  building gives no ShieldingofHome; the shielding is Normal") named the
  omission. The applied value is unchanged, so every number is.
- `data/examples/BEopt_example.xml`: the
  `SimulationControl/TemperatureCapacitanceMultiplier` element (1.0) is
  deleted. OS-HPXML v1.12 reads the multiplier only from
  `AdvancedResearchFeatures` (hpxml.rb:1051) and applies 7 when absent;
  the `SimulationControl` form was ignored (the warning said so), so the
  applied value is unchanged, 7 before and after, and every number is.

No golden manifest runs either fixture; the dwellings' own paths are
untouched. The two correction classes are the schedule's recognised
"fixture correction" outside the numeric block, every other product and
column bitwise, and the change is this entry.
### The fleet pre-pass's per-home parse, timed, 2026-10-11 (branch `row/r52-fl3-fl4`, tip `c5c130ec`)

FL-4's alignment pre-pass (`04-fleet.md`) reads every home's HPXML
through `read_site`, a full document parse whose `Site` extraction runs
alone, and its weather header through `read_weather_meta`, the format
detection reading only the first line for a ResStock CSV, before the
first chunk starts; the fleet pays one extra parse per home that the
home's built dwelling parses again. Quantity: that per-home pre-pass
cost, timed directly. Method: a release-built timing harness in the
fleet crate calling `read_site` and `read_weather_meta` on the three
`fleet_mixed_offset` homes' fixture inputs, 10 warm-up calls then 500
timed calls per home per pass, the per-call medians taken over four
passes, this host, WSL2, load average 2 to 4 (1 min):

- `read_site` per home: 0.168 to 0.329 ms (bldg0000002 about 0.18,
  bldg0000004 about 0.19, bldg0176227 about 0.28 to 0.33, the largest
  document).
- `read_weather_meta` per home: 0.004 to 0.017 ms.
- The two summed per home: 0.28 to 0.34 ms; the three-home fleet's
  whole pre-pass: about 0.8 to 1.0 ms.

The same fleet's `frame_golden run tests/fixtures/golden/
fleet_mixed_offset.toml --repeat 5` at this tip: construct+simulate
median 0.228 s (0.225 to 0.239), the tool's simulate-only split 0.097 s
median. The pre-pass is about 1 ms against a 0.23 s wall, under 1% and
far inside the noise rule's 5% band: nothing measurable on the fleet
wall at this size, and the direct timing bounds it. The duplicate parse
is the cost RT-4 names for every input: `06-runtime-memory.md` RT-4's
input plumbing rework, parsed handles shared between the pre-pass and
the built dwellings, owns the fix; until it lands this record bounds
the pre-pass at well under a millisecond per home on these fixtures.
### The result tables' conversion: IPC versus the Arrow PyCapsule export, 2026-10-11

The branch `row/r55-py1`, cut from `5a9c4ac8`. The first version of
this record ran its "before" numbers in the parent's own venv (Python
3.13) against the branch's here (Python 3.12) and took cross-process
minima of alternating pairs; the independent review found the callback
pair does not reproduce (paired medians reverse the direction, the
method swings about 100 percent) and the interpreter mismatch went
unnoted. This record re-derives the pairs soundly: the parent's
release extension built into THIS venv (`maturin develop --release`
with `VIRTUAL_ENV` pointed here, the same CPython 3.12.7 interpreter
both sides, the parent's `python/` package first on `PYTHONPATH`), the
same script both sides, the ambient load 1.5 to 3.4 on 28 cores.

The batch callback's delivery over a building-year run at 900 s steps
(876 batches at `output_chunk_size` 40, cz4a_ashp_hpwh, verbosity 2,
metrics off), end to end, fifteen in-process alternating pairs per
side (each pair one plain run and one run with a no-op callback
attached, the pair's order alternating, the dwelling built and
initialized outside the timed region), the pair delta (callback minus
plain) the statistic:

- before (the frame delivered through the IPC conversion): median pair
  delta +0.092 s per building-year, +0.104 ms per batch (pair deltas
  0.052 to 0.140 s, quartiles 0.078 and 0.117)
- after (the frame delivered through the capsule): median pair delta
  +0.105 s per building-year, +0.119 ms per batch (pair deltas 0.060
  to 0.126 s, quartiles 0.097 and 0.113)

The paired medians reverse the direction: on this method the
callback's end-to-end delivery cost is within run-to-run noise,
indistinguishable between the IPC delivery and the capsule delivery,
and the record claims no per-batch improvement from the row. The
honest bound: within run-to-run noise; the structural win is the
removed IPC machinery, the frame-read pair is the solid number. (The
callback destination entry's own per-batch figure, about 0.1 ms per
batch end to end at verbosity 2, agrees with both medians here; the
first record's 0.129-to-0.057 pair and the review's 0.8 ms
instrumentation figure were the same cost read through looser
methods.)

The frame-read pair, the same venv (the `full_year_hourly` report's
frame, 8 760 rows, 32 columns, verbosity 2, metrics off; median of 20
report reads in the warm process; the first record's 0.318 ms and "36
columns" did not reproduce, 32 is what verbosity 2 builds on both
sides):

- before (IPC: `FileWriter` bytes through `BytesIO`, `polars.read_ipc`):
  0.224 ms (min 0.175)
- after (capsule: pyo3-arrow's `PyTable` over the report's batches,
  `polars.DataFrame(table)`): 0.050 ms rechunked (min 0.047), 0.049 ms
  with `rechunk=False`; about 4.5x lower on the median. On a
  multi-batch chunking of the same frame (`output_chunk_size` 1 000,
  nine batches) the IPC read is 2.45 ms and the capsule's rechunked
  read 0.12 ms: the IPC round trip pays per batch, the capsule does
  not. The entry's context figures (about 6.5 ms IPC, about 2.7 ms
  capsule rechunked) were measured on a slower host against a denser
  frame; the ratio's direction is the row's point, the IPC
  serialize-and-parse round trip is gone and the buffers are shared.

The export call's own costs, on fresh reports in the warm process (the
first record's "0.0024 ms on the first (rechunking and caching) call"
was a cached call mislabeled):

- the first rechunked call on a fresh report: about 0.04 to 0.05 ms on
  a multi-batch report (2 to 18 batches over this frame: the one real
  2.2 MB `concat_batches` copy), 0.11 ms at 219 batches
  (`output_chunk_size` 40), and 0.002 ms on the default single-batch
  chunking (8 760 rows under the 10 000 default: arrow's `concat`
  short-circuits one input to a slice, no copy, so on this shape the
  cache buys nothing measurable);
- the process's first-ever materialization of a report's table: 3.8 ms
  with only polars' native path warm (the capsule importer's one-time
  init); the next reads decay through 0.29, 0.12, 0.10 and 0.065 ms to
  the steady read;
- a cached call (the held wrapper's later `arrow()`): 0.0005 to
  0.0009 ms, which share the cached batch's buffers.

### The surface-independent forcing computed once per weather and time grid, 2026-10-12 (branch `row/r62-rt2`, based on `9cc6d578`)

Raw record: `.shipwright/runs/r62-rt2-measurement.md` (every set, its
validity verdict under the 5% rule, and the host condition: WSL2, shared
with another worktree's test suite at load 7 to 18, which the wall sets
could not beat).

The row's numbers, the consumer shape (the ResStock bldg0176227 home at
900 s over one weather file):

- The forcing block's own work (the profiling-gated accumulator, exact):
  the stepped-episode loop's 9600 per-step computations (about 0.95 us
  each, 8.6 to 14.0 ms of the loop) become one shared table build and
  about 0.5 to 1.5 ms of reads; a full-year run's forcing drops from 35 to
  43 ms (2.9% of `simulate()`) to 3.6 to 4.6 ms (0.3%).
- The stepped-episode loop's wall (M-3 item 5's shape, 100 x
  load-state-and-96-steps), paired in one binary (the table against the
  per-step computation the `FORCE_PER_STEP_FORCING` switch selects):
  median 348.7 against 378.2 ms, and 354.6 against 376.6 ms in the second
  set: 5.8% to 7.8% lower with the table, above the band; no set is valid
  under the noise rule on this host (the spread exceeds 5% in every set),
  so the verdict rests on the two sets' agreement in direction plus the
  exact accumulator's removal of the per-step computation.
- The fleet step loop (12 homes on one weather file, 96 steps, 4 threads):
  median 47.2 against 49.8 ms, -5.2%, the sets overlapping: at the band's
  edge, recorded as not resolved on this host. The fleet's construction
  wall (12 homes): about 604 ms with the table against about 534 ms
  without (the identity hash per home plus one shared build).
- The single-pass full year: table median 1395 ms, per-step 1364 ms
  (+2.3%, inside the band: no measurable change, as the entry's
  parenthetical says, a single run computes each row once either way).
  The year dwelling's construction: about 77 ms against about 39 ms (the
  eager one-pass build plus the identity hash).
- The table's size: 35040 rows at 900 s, 2242560 bytes (2.24 MB); 525600
  rows at 60 s, 33638400 bytes (33.6 MB); 64 bytes per row.
- The per-surface Perez and incidence loop's share (a temporary
  accumulator, removed): 7.3% to 7.5% of `simulate()` on the full year
  and 5.7% to 5.8% of the stepped-episode loop, both at or above the 5%
  band: the per-surface branch's condition fires. Its specified mechanism
  (a per-blueprint table shared by every dwelling the blueprint builds,
  and its two-dwellings-from-one-blueprint acceptance test) needs the
  blueprint that builds many dwellings, which `06-runtime-memory.md`
  RT-4's parsed-input surface owns: the branch lands with that row, the
  shares recorded here as the measurement that fired it.

### Input digests, 2026-10-11, `9527aa6e`

Load average at write time: 19.8 (1 min), the tail of the gate runs.

Quantity: the digest cost per blueprint (bytes hashed and time), on the
cz4a parity inputs (HPXML 49,302 B, schedule 194,159 B, weather
1,602,455 B, the defaults store's read set 321,970 B): about 2.2 MB
hashed per blueprint; a generated schedule adds the profiles file
(28 KB).

Command: `cat <the input files and the store-read defaults files> |
sha256sum`, ten passes in a loop: 0.051 s total, about 5 ms per blueprint
on the warm page cache. The digest cost is negligible against the parse
it rides (the same blueprint's HPXML parse alone is tens of ms); the
re-read for the digest touches the same page-cached bytes.

### The validate-only path's wall time, 2026-10-12, `906d4a1c`

Load average at write time: under 1 (the gate runs finished).

Quantity: the requirement's feasibility query in milliseconds: the
validate-only assembly's wall time against the 100 ms target, the valid
input (the cz4a parity fixture, 48 h at 900 s, no warm-up) and the
infeasible one (the cz2a parity fixture's gas furnace with an override of
AFUE 1.33, rejected at the equipment's init in the assembly).

Command: the blueprint_validate suite's hand-run measurement test
(`validate_wall_time`, ignored, run by hand in the release build with all
features): `cargo test -p hares-core --release --all-features --test
blueprint_validate validate_wall_time -- --ignored --nocapture`, the
first validate() call discarded (page cache and lazy statics), the second
timed.

Result: the cz4a fixture's validate() 3.3 ms; the cz2a AFUE rejection
6.6 ms. Both one order of magnitude under the 100 ms target, so no phase
breakdown was recorded; the dominant cost either way is the assembly's
own (the environment init and the equipment construction), the warm-up's
simulated days absent by construction. The re-run on the final head
(after the row's commits): 3.0 ms and 5.5 ms, the same reading.

### The RL observation read and the batch loop, 2026-10-12, commit `37237b24` (the frozen observation plan)

Raw record: `.shipwright/runs/r71-rt7-rt8-measurement.md` (the harnesses'
shapes, the counts' coverage per figure and the host condition: WSL2,
the branch's own gate builds sharing the host, load average 4 to 17).

The per-environment observation read's allocations (the counting
allocator, 96 reads after one step, divided by 96): the base's
telemetry-snapshot-plus-string-walk read 42 per call (4 032 over 96);
the plan's `observe_into` reads 0, in the committed zero-allocation
test, in every run since. Per `batch_step` call of 8 environments that
is 336 read allocations the engine stops paying, beside the
Python-side per-row lists and the per-step `np.asarray` the old row
conversion built.

The whole call's calling-thread allocation count (20 calls after 10
warm ones): n=1 median 78 at the base against 34 at the commit (the
read rode the calling thread at one environment); n=8 median 120
against 121 (the reads ride the rayon workers there, so the
calling-thread figure is the GIL-held conversion section: unchanged,
as expected).

The batch loop's per-step wall (M-3 item 5's stepped-episode shape, 8
base.xml dwellings, 1 000 steps of 60 s, ten observation fields of
every kind the home answers, the median of the per-step walls): the
base's behavior 0.942 ms; the commit's copy=True 0.898 and 0.934 ms in
two sets, copy=False 0.881 ms. The sets overlap the base's under this
host's noise (the loop's wall is the simulation step's, the observation
a small share of it), so the wall is recorded as no measurable change
and the verdict rests on the structural claim, the removed read, which
the zero-allocation test proves exactly.

### The RL reset's wall against a full construction, 2026-10-12, commit `fe79a83d` (the reseed reset)

Raw record: `.shipwright/runs/r71-rt7-rt8-measurement.md`. The harness:
`.shipwright/runs/r71a-reset-wall.py`, the consumer_shape_900s inputs
(the bldg0176227 ResStock home, its envelope-materials override,
warm-up off per the fixture) at 1 d, 900 s steps, 20 constructions and
20 resets, each timed, the medians recorded; the reset is the sequence
the gym environments run, `load_state(snapshot)` then the seed. Load
average 4 to 8 at write time.

- the base's path (`reset_with_seed`): construction median 39.6 ms,
  reset median 5.620 ms, 14.4% of construction (a first set at 14.2%);
- the row's path (`reseed`): construction median 39.0 ms, reset median
  0.047 ms, 0.1% of construction (a second set at 0.046 ms against
  38.3 ms).

About 120x lower. The reset that was 14% of a construction now costs a
twentieth of a millisecond: the checkpoint deserialize plus the stream
re-derivation. Requirement RL-3's well-under-10% holds with orders of
magnitude to spare.
### The pool-sizing row's production-capture wall time, 2026-10-12

Before and after the sizing commit (`60ccfbc`, consumer branch
`row/cn5-pool-sizing`): the wall time of one production-set capture (four
premises, 365 days, 16 generations, `fleet_workers` 4) through
`tools/capture_de_trace.py capture`, the entry's measurement.

Before, the consumer at `cf7f2c3`, the sizing bug live: each premise's fit
pools at `default_workers()`, 26 on this 28-core host, four premises
concurrently, up to 104 worker processes on 28 cores. After, the sizing
commit: the inner pools take the fleet-aware share (`_pool_sizes` gives 7)
and each fit waves its population into pools of 6, four premises
concurrently, 24 worker processes.

Result: before 185.8 s and 203.3 s (load average 11.4 and 23.5 at start);
after 225.6 s, 255.3 s and 241.3 s (load 82.9, 6.3 and 47.3 at start; the
first after-run began 12 s after the before-run ended, its load still
draining, the cleanest start is the 255.3 s one). Total CPU: about 69.4
minutes before, about 66 minutes after (fewer worker imports to pay). The
five captures are bitwise identical across two engine builds of the same
pin source. The fixed sizing is about 1.2 to 1.4x slower in wall on this
busy 28-core host while consuming about 3 minutes less CPU: the
oversubscription it removes was smoothing the premises' serial phases
across each other. The engine-under-test for every run was the pin
`a169354e` (the row's worktree build; the before runs used the main
checkout's Oct 9 `.so` of the same source and compared identical).

### The consumer's rows 72-73 landed on main, 2026-10-13 (main `d8d4f56f`)

The consumer repository's fold branch `fold-cn1-cn5` (`8d9c96f8`: row 72's
three commits folded as `e4a3137d`/`e5e2b62e`/`8d9c96f8` and row 73's
`60ccfbc`) merged `--no-ff` onto `main` as `7dc661fc` (`merge: the report
API and the pool sizing rows`; no conflict, the fold's side taken, a
superset of the pre-rename main `78568b5`), then the pin `d8d4f56f` (the
gitlink to the folded engine head `21506ea2`); main pushed
`78568b5..d8d4f56f` (origin's PR rule bypassed, the ref update accepted).
Before the merge, the shared checkout's staged uncommitted debris — a
revert of row 72's `05427c6b` (the parse-once `build_kwargs` handoff) and
`6a628523` (the `ordered=False` completion-order return), 5 files +9/−98 —
was backed up as a patch and discarded (the fold carries both). Gates at
`d8d4f56f`: `uv sync --extra engine` clean (the stale `a169354e` extension
rebuilt by `--reinstall-package hares-py`), run_all 367 passed / 0 failed /
7 skipped (the base seven), ruff clean, the B0 production set captured
twice and `identical` (the two runs mutual-bitwise), the compare against
the shipped baseline `B0-de-trace.json` printing exactly the recorded 260
differing paths (the divergent audit line's delta, attributed in the
engine ledger; the baseline not re-captured).

### The weather interpolation rows, 2026-10-14 (the tip `487df2e9` on `row/r79-b1a-b1b`)

The two weather rows' golden movement, per golden, measured from the run
metrics (the end-use totals, the rolling peak and the frame's indoor
column mean). The point-value row's deltas run from the base `e30f70c1`
to the point-value commit `2d3e6721`; the solar row's from `2d3e6721` to
the solar commit `487df2e9`. Load average at the captures: 1 to 2 (1
min).

Point values (`2d3e6721` against `e30f70c1`), HVAC heating kWh / HVAC
cooling kWh / peak 60 min kW / mean indoor temperature C:

- `consumer_shape_900s`: 92.960 → 91.842 / 1137.40 → 1147.04 /
  3.4944 → 3.4932 / +0.0120
- `fleet_mixed_offset`: 17.769 → 17.769 / 0.000 → 0.000 / 4.7833 →
  4.7833 / (fleet frame)
- `fleet_same_offset_300s`: 2.6265 → 2.4729 / 1.7500 → 1.7458 / 2.5592 →
  2.5510 / (fleet frame)
- `full_year_hourly`: 369.30 → 367.58 / 493.42 → 489.08 / 5.8569 →
  5.8462 / −0.0145
- `leap_year_february_900s`: 156.98 → 156.88 / 4.950 → 4.963 / 7.3882 →
  7.4180 / −0.0019
- `month_boundary_billing_3600s`: 30.600 → 30.589 / 14.400 → 14.400 /
  5.1850 → 5.1840 / −0.0010
- `weather_wrap_900s`: 244.21 → 243.66 / 4.800 → 4.800 / 11.1439 →
  9.3194 / −0.0183
- `full_year_900s_dst`: 7092.16 → 7085.47 / 485.63 → 486.50 / 11.5703 →
  10.3276 / +0.0011

Solar (`487df2e9` against `2d3e6721`), gross PV generation kWh / HVAC
heating kWh / HVAC cooling kWh / peak 60 min kW:

- `consumer_shape_900s`: 10495.57 → 10492.86 / 91.842 → 91.666 /
  1147.04 → 1148.47 / 3.4932 → 3.4905
- `fleet_same_offset_300s`: 21.027 → 21.000 / 2.4729 → 2.4727 / 1.7458 →
  1.7458 / 2.5510 → 2.5512
- `full_year_900s_dst`: (no PV) / 7085.47 → 7078.02 / 486.50 → 488.75 /
  10.3276 → 10.3258
- `leap_year_february_900s`: (no PV) / 156.88 → 156.87 / 4.963 → 4.963 /
  7.4180 → 7.4182
- `weather_wrap_900s`: (no PV) / 243.66 → 243.64 / 4.800 → 4.800 /
  9.3194 → 9.3187
- `full_year_hourly`, `month_boundary_billing_3600s`,
  `fleet_mixed_offset`, `resstock_2025_fleet`: the frame digests moved
  only by the provenance fields; every metric value identical (the gas
  furnace's heating and the fleet's 3600 s reads take the hour's record
  whole, which the phase correction had already aligned).

The annual incident solar on the horizontal: unchanged. The midpoint
rule preserves the hourly and annual totals exactly (the weights sum to
one over each hour's steps; EnergyPlus's own note, "the total solar for
a day should agree"): the annual mean of the step values over the cz4a
file at 900 s measured identical (225.750685 W/m2 both rules, delta
0.000000). The movement is within-day phase, and the goldens'
verbosity-2 frames carry no insolation or solar-gain columns, so the
solar heat gains' per-golden delta is not derivable from the committed
products; the PV and HVAC deltas above carry the record.

### The calendar weather row, 2026-10-15 (the row 81 worktree `r81-b1c`)

The golden movement of the calendar-date addressing: exactly one golden
moved; the other seven are bitwise identical under both feature sets
(compare-all 8 with the re-captured mover + the dst set's
`full_year_900s_dst` identical). The movement's cause: the 2024 leap
year's Feb 29 is not simulated over a typical file without Feb 29
records (the EnergyPlus rule), so the run's simulated step count is the
calendar's less the skipped day's, and every date after Feb 28 reads
its own calendar date's records where the previous running-index rule
read one day late.

- `leap_year_february_900s` (2024-02-27, 5 days, 900 s, warm-up 7 d):
  the frame's rows 480 to 384 (480 calendar steps less the 96 of Feb
  29); the run's simulated duration 120.0 to 96.0 h; the row after Feb
  28 23:45 labelled Mar 1 00:00. Run metrics (the parent
  `9356e2ec`'s capture against the fix): gross/net consumption
  236.908 -> 197.662 kWh; hvac_heating 156.867 -> 136.318 kWh;
  hvac_cooling 4.963 -> 4.300 kWh; cooking 10.858 -> 7.857; lighting
  17.164 -> 13.616; other 17.226 -> 12.010; plug loads 17.241 ->
  13.477; refrigeration 5.551 -> 4.559; ventilation 2.394 -> 1.915;
  water heating 4.644 -> 3.610. The rolling demand-window peaks are
  unchanged (the 15/30/60-min peaks fall before Feb 29: 21.4137,
  12.3710, 7.4182 kW identical on bits); the per-end-use peaks
  identical except other (2.5666 -> 2.5473 kW) and water heating
  (0.51959 -> 0.51958 kW). Frame column means: outdoor dry bulb 5.553
  -> 4.361 C, indoor 20.775 -> 20.760 C, HVAC heating mean 1.307 ->
  1.420 kW. The end-use totals drop by close to the one day in five the
  run no longer simulates, with the remainder the re-aligned weather's
  own phase.
- `consumer_shape_900s`, `fleet_mixed_offset`, `fleet_same_offset_300s`,
  `full_year_hourly`, `month_boundary_billing_3600s`, `weather_wrap_900s`,
  `resstock_2025_fleet`: bitwise identical (their spans carry no leap
  Feb 29; the calendar addressing reproduces the running-index rule's
  reads for non-leap spans exactly). The warm-up replay reads the run's
  first day only, so no warm-up read moves.
- `full_year_900s_dst` (the dst feature set): bitwise identical (2023,
  non-leap).

The BESTEST drift locks re-measured with the cause (the 600FF fixture
runs 2024, so its simulated year is 8736 steps and its annual sums move
with the re-aligned weather): 600 heating 3122.278 -> 3090.707 kWh,
600 cooling 5542.032 -> 5506.833 kWh, 640 heating energy 2018.333 ->
1995.592 kWh, 900 heating 1105.795 -> 1069.393 kWh; the 600FF and 900FF
free-float peaks and mins stayed inside their baselines' 1 percent and
were not re-measured. The `case_600ff_zone_air_balance_residual_bounded`
bounds held (max 931.4 W, mean 195.4 W, inside the measured envelope)
and its expected step count now reads the engine's own simulated year
(8736). The checkpoint acceptance's tail now runs the engine's
simulated step count (the leap-year run's end sits 96 steps before the
calendar arithmetic's).

### The warm-up criteria row, 2026-10-15 (the worktree `r83-b2-b3`)

The warm-up's stop rule changed from the old rule (the last step's
conditioned-zone temperatures between consecutive days at 0.5 delta C,
25 days, no minimum) to the EnergyPlus criteria (every zone's daily
temperature extremes against 0.4 delta C and daily peak HVAC delivery
against 0.04, with the 100 W floor and the 1e-4 W pass-through, at
least 1 and at most 25 days). Days run per fixture, the old rule
against the new criteria: `full_year_hourly` 2 -> 3,
`full_year_900s_dst` 2 -> 3, `leap_year_february_900s` 3 -> 2,
`weather_wrap_900s` 2 -> 3, `month_boundary_billing_3600s` 2 -> 3, the
BESTEST 600FF 8 -> 9 and the BESTEST 900 2 -> 4. The movement's cause:
the old rule stopped on last-step temperatures while the day-over-day
daily-extreme and load residuals were still open (the cz2a fixture's
first comparison carries a 2.18 delta C daily-min residual on its
garage and a 47 percent heating-load residual on the cz4a heat pump);
conversely the leap fixture's old rule needed a third day its daily
extremes did not.

Fixture selection (recorded, then pinned). The candidates, each warmed
up on its own first day with `min_days = max_days = 25` so every
compared day's four residuals per zone are recorded, from the cold
construction state: (A) `full_year_hourly`'s cz2a gas furnace and AC
(2023-01-01, 3600 s), (B) `full_year_900s_dst`'s cz4a heat pump
(2023-01-01, 900 s), (C) `leap_year_february_900s`'s cz4a (2024-02-27,
900 s), (D) `tests/fixtures/parity/cz6b_resistance_res_wh/`
(2023-01-01, 3600 s), and two heavyweight BESTEST homes added by this
row: (E) `900ff` (concrete, free float; the entry's four candidates are
all lightweight and cannot exhibit the tolerance band or a
load-blocked day) and (F) `900` (concrete, conditioned ideal HVAC).
The defaults' days run: A 3, B 3, C 2, D 3, E 9, F 4. The decisive
residuals (day, zone, the four residuals):

- A day 2: zone 1 dmax 0.016 dmin 0.114 heat 0.083; zone 2 dmax 0.042
  dmin 2.179 (the unconditioned garage's day-1 warm-up transient; the
  old rule never looked at it). Day 3: every residual inside tolerance.
- B day 2: zone 1 dmax 3.603 dmin 0.007 heat 0.881; day 3 zone 1 dmin
  0.278 heat 0.003. The 3.6 swings reappear on days 4 to 7 and 11 to
  17 (the heat pump water heater's stochastic draws), decaying to
  0.28/0.0001.
- C day 2: zone 1 dmax 0.003 dmin 0.007 heat 0.034; zone 2 dmax 0.015
  dmin 0.190. Converged on day 2.
- D day 2: zone 1 dmax 0.000003 dmin 0.0014 heat 0.081; zone 2 dmin
  2.190 (the same garage transient as A's). Day 3: inside tolerance.
- E day 2 to 8: dmax 8.31, 4.90, 2.90, 1.72, 1.02, 0.605, 0.359; dmin
  11.38, 6.18, 3.52, 2.09, 1.21, 0.718, 0.426 (the day-8 daily-min
  residual lies in (0.4, 0.5]). The loads are 0 (free float): the load
  criterion never binds.
- F day 2: dmax 1.704 dmin 5.222 heat 0.900; day 3: dmax 0.300 dmin
  0.343 heat 0.116 (the temperature residuals pass at day 3 while the
  daily peak heating still changes 11.6 percent); day 4: heat 0.025.
  The loads extend the warm-up one day past the temperature rule.

The acceptance tests pin, by name, the first candidate in the order A
to D, extended to E and F where the entry's four have no candidate:
`warmup_tolerance_changes_days_run` on E (days run 8 at 0.5 against 9
at 0.4), `warmup_load_criterion_extends_warmup` on F (4 days at the
default 0.04 against 3 at the loose 0.5), `warmup_min_days_is_honoured`
on C (min_days 3 runs exactly 3), `warmup_nonconvergence_is_recorded`
on A (max_days 2 stops with converged false and the warning naming the
failing zones), and `warmup_nan_residual_fails_the_run` on A.

First-day energy delta, the old stop against the new criteria's stop
(the warm-up forced to the old day count, then the default; the run's
first day's HVAC delivery summed): A 458.7 Wh (0.68 percent of the
day's heating), B 140.7 Wh (0.19 percent), C 3.7 Wh (0.009 percent), D
485.3 Wh (0.71 percent); no cooling delivered in January on either
side.

The BESTEST drift locks re-measured with the cause (the warm-up days
moved: the 900 pair 2 to 4 and 8 to 9, the lightweight 600 and 640 one
day longer under the daily-extreme criteria): 600 heating
3090.707 -> 3090.701 kWh, 600 cooling 5506.833 -> 5506.836 kWh, 640
heating energy 1995.592 -> 1995.574 kWh, 900 heating 1069.393 ->
1068.706 kWh, 600FF peak 74.010 -> 74.370 C, 900FF peak 46.151 ->
46.392 C, the 900FF min unchanged on bits. Every move is inside the
lock's 1 percent; the pins re-captured to the measured values with the
cause in `tests/bestest/mod.rs`'s `drift_baseline`.

The OCHRE comparison's premise is cold-on-both-sides and is now stated
in the test: OCHRE's `initialization_time` is `None` there (the
reference simulates from its own initial state; the fixed
initialization run only happens when a caller asks for it) and the
HARES side's config holds its warm-up disabled (`warmup = "disabled"`,
spelled through `from_toml`) for the same premise. The warm-up's
initial-condition movement is the goldens' own, measured above; the
parity's strict agreement is bitwise-close again once both sides hold
the premise.

The Python surface row's own measurement: the `consumer_shape_900s`
inputs run through Python with no `warmup` keyword (the default, the
warm-up ran 2 days, converged) against the same run with `warmup=None`
(disabled), both 48 h at 900 s, the per-column maximum absolute
difference: the zone temperatures 1.358 delta C (Zone 3), 0.733 delta C
(Indoor), 0.652 delta C (Zone 2); the gas power 0.086 therms/hour (the
total the same); the total electric power 0.046 kW. The warmed run's
first 48 h draws less gas: the warmed initial condition's heating
demand is the smaller one, and a Python evaluation that never passes
the keyword (the calibration consumer's own case) inherits it from
this row on.

### The billing month boundary row, 2026-10-15 (the worktree `r85-b4`, the fix commit `45cd54e4`)

The entry's acceptance restated against the anniversary semantics (the
row 32 record's landed behavior, the notes' reconciliation): the monthly
cycle's period ends stay anniversary-based, the anchor day carried
across the closes so no clamped boundary drifts the chain, and the
dwelling passes the step's end time, so the step ending at a boundary is
the closing period's last step. The goldens are bitwise: compare-all 8
manifests identical on the default build and `full_year_900s_dst`
identical on the dst build. The one tariffed golden,
`month_boundary_billing_3600s`, closes no billing boundary inside its
12-day run (the January 25 anniversary boundary is February 25, outside
January 25 to February 6), and its billing product is bitwise unchanged:
one period, the run's single period, its totals the same bits.

The movement lives on runs that span a cycle boundary. The acceptance
test's run (the `month_boundary_billing_3600s` inputs held 35 days at
3600 s, January 25 to March 1, crossing the February 25 00:00 MST
boundary): at the parent commit the first period's summary held the
February 25 00:00 step's energy and closed with a total import of
941.51821526152 kWh; after the change it closes on its own 744 steps at
941.0620277257223 kWh, and the second period (February 25 to the
March 1 finalize) holds 112.319426955769 kWh with the boundary step's
0.4562 kWh among them. The two periods' combined metered energy is
unchanged: every kWh is billed exactly once, the boundary step only
changed periods. The fixed charges do not move on this run: the first
period closed at its own boundary (no proration) and the finalize bills
the open period's full monthly charge.

The mid-run boundary-attach convention changed with it: at the parent, a
from-start run's close fired one step late, so the boundary step's
energy sat in the closing period while a tariff attached exactly at the
boundary put it in the new one (one step's attribution differed between
the routes). After the change both routes book the boundary step to the
period it opens; the pin
`a_tariff_attached_at_a_period_boundary_books_the_boundary_step_to_the_new_period`
was restated from the asymmetry to the agreement.

### The typed end-use rollup row, 2026-10-16 (the worktree `r86-b5`)

The golden deltas, recorded as measured (the entry expected none; the
expectation was wrong: every golden carries a cooking range whose
descriptor declares `other` and the resstock homes carry a `TV` whose
descriptor declares `plug_loads`, so the rollup re-buckets in all of
them; no number moves that is not that re-bucketing).

The per-end-use metrics deltas, per golden (every one the same two
causes, the `Cooking Range`'s power from `cooking` to `other` and the
`TV`'s from `other` to `plug_loads`; the aggregate column order
follows the equipment's first-encounter order under the typed
classification):

- `full_year_hourly`, `month_boundary_billing_3600s` (the cz2a home):
  the `cooking` bucket is gone from `peak_power_kw.per_end_use` and
  `total_energy_kwh.per_end_use` (the range's 12.416503494827793 kWh
  and 0.7601316557361266 kW peak now in `other`), `other`
  57.591771350939354 to 44.46726066701062 kWh (the range in, the TV
  out), `plug_loads` 44.9851411019389 to 70.52615528069546 kWh (the TV
  in); every other per-end-use bucket and every product bitwise.
- `full_year_900s_dst`, `leap_year_february_900s`, `weather_wrap_900s`
  (the cz4a home): the same shape (the range's 4.943797448258921 kWh
  and 0.49287948497904466 kW peak into `other`, `other` 10.580524188911019
  to 7.036155551000382 kWh, `plug_loads` 14.950124784027626 to
  23.438290870197193 kWh).
- `consumer_shape_900s`, `fleet_mixed_offset`, `fleet_same_offset_300s`,
  `resstock_2025_fleet` (the resstock homes): the same two causes per
  home plus the `cooking` bucket's removal from the fleet products'
  schemas; the `aggregate` and `covered_weight` products' schemas
  re-ordered accordingly.

The totals are conserved across the re-bucketing (the rollup changes
which aggregate column each equipment's own power feeds, nothing
else): the per-fixture sums of the end-use aggregates are bitwise the
same set of equipment contributions before and after. Parity homes'
per-equipment columns are unchanged: every registered class the
fixtures resolve has a declared end use that agrees with the old name
mapping except the range and the TV, which have no per-equipment
column movement (their power columns stay; only the aggregate bucket
changed).

The two HPXML fixtures' end-use rollups, before and after (the sum of
each end-use aggregate column over the acceptance run, the fixtures'
only 48 h at 3600 s; the after column that does not exist before is
marked new):

- `base-pv.xml`: before, no PV aggregate existed and the two systems'
  power sat in `other` with everything else unclassified (other
  -23.75208103912645, cooking 2.1395205338916705, plug loads
  13.454351479350198); after: `PV` -28.39957228899651 (new; the two
  systems' -22.51061713374031 and -5.888955155256206 exactly),
  `other` 3.389023733380829 (the range and the fixture's TV moved as
  above), the remaining buckets bitwise (hvac heating 5.562059173933972,
  hvac cooling 2.3999999999999995, water heating 16.78328294188367,
  refrigeration 2.978443851365222, lighting 9.259208140430788).
- `base-misc-loads-large-uncommon.xml`: the pool and spa heaters'
  rollups do not move (pool heater 92.7282568893422, spa heater
  6.559918911885161, both bitwise before and after; the pool/spa
  defect was the HVAC column family, not the rollup); the
  refrigerator's power leaves `other` for the `refrigeration`
  aggregate (new, 13.059330732909059), the range's leaves `cooking`
  for `other` (cooking 6.54459411034136 to 4.40507357644969, the gas
  grill's own `cooking` power remaining), the TV's leaves `other` for
  `plug loads` (16.818890382031096 to 20.695904332219598), `other`
  36.58455659238225 to 16.594950057373204, the remaining buckets
  bitwise (hvac heating 4.882350651723648, hvac cooling
  2.3999999999999995, water heating 16.780562518290658, lighting
  13.650796626149587, ev 56.50070307968867, pool pump 17.085702691883775,
  spa pump 4.58222130979265).

The all-classes catalogue pins (`tests/fixtures/schema/v0..v8.json`,
`kinds.json`) re-captured with the cause read column by column: the
phantom EV code columns of the event-based load and the television
pin are gone (the v8 count 10 to 6), the pool and spa heaters leave
the HVAC family (setpoint, capacity, COP, speed, main, fan, runtime,
schedule, compressor, supply/return air), the ideal load, the `HPWH`
alias and the indirect tank gain the HVAC family their declarations
carry, the ground-source and water-loop heat pump heaters gain the
defrost and ER columns their pan heater declaration carries (the old
name matcher missed them), and the end-use aggregate columns
re-ordered by first encounter.

The re-captures are byte-identical twice (all nine goldens, each
captured twice on the staged state and compared byte for byte), the
default build's compare-all 8 manifests identical and
`full_year_900s_dst` identical on the dst build after them.
### The null-for-not-applicable row, 2026-10-16 (the worktree `r87-b6`)

The measurement the entry asks for, read off the frame diff between the
parent commit's materialized frames (`frame_golden materialize-all` on
`0e897801`, plus the dst manifest under `-F dst`, kept aside in
`target/golden-base/`) and the fix tree's captures: the cells that
change from 0.0 to null are `Unmet HVAC Load (C)`'s alone: the column
no producer claims in any dwelling (declared at verbosity 2, never
written; the cell read the row's zero every step). Per golden, the
cells 0.0-to-null: `consumer_shape_900s` 35040 of 35040 rows,
`full_year_900s_dst` 35040, `full_year_hourly` 8760 of 8760,
`leap_year_february_900s` 384, `month_boundary_billing_3600s` 288,
`weather_wrap_900s` 384, `fleet_mixed_offset` aggregate 165 of 165,
`fleet_same_offset_300s` aggregate 192 of 192, `resstock_2025_fleet`
aggregate 48 of 48. In the fleet folds the column's covered-weight
cells move from the homes' weights (the zero cells counted as
contributions) to the fold's zero weight behind the now-null aggregate.
The other movement class is declarative only: the schema's per-field
`nullable` flags (the not-nullable classes flipped in every product and
schema pin; the values and digests of those columns bitwise unchanged).
No metric, health or billing figure moved in any golden. The deeper
nulls the row introduces (COP, SHR, mode, setpoint, SOC, speed,
defrost, the v8 pass-throughs, the setpoint chain, the duct losses)
live at verbosity 3 and above, which no golden fixture's schema
reaches; they are pinned by the acceptance tests over the
`cz4a_ashp_hpwh` fixture at verbosity 7 and 8: the ASHP Cooler's SHR
null in all 192 steps, the ASHP Heater's COP null in its 8 Off steps
and non-null in the heating steps, the appliance mode cells and the
HPWH setpoint null, the never-emitted v8 keys null, and the
never-produced dwelling columns null in a no-HVAC dwelling.
### The declared-kind fleet fold row, 2026-10-17 (the worktree `r87a-fl5`)

The golden record, recorded as measured: every golden unchanged.
`frame_golden compare-all` on the default build reports the 8 manifests
identical and on the dst build `full_year_900s_dst` identical. The
fleet goldens' verbosity-2 selection holds only energy, power and
temperature columns, whose unit-keyed rules already matched the
declared ones, so no number moves; every variable's field metadata
gains the declared kind, aggregation and across-homes tags, and the
goldens' schema digest holds name, type and nullability, so the
metadata addition does not reach the digests.

The acceptance run's delta, before and after: the run is the
`fleet_same_offset_300s` inputs at 15-minute frequency, the selection
naming every verbosity-8 variable the homes declare plus the three
weather channels no tier preset carries (192 periods, 195 columns),
dumped as CSV at the parent `ff39b24b` and at `a39d17a7` and compared
column by column. The aggregate moves 3 of 195 columns, exactly the
columns whose unit-keyed rule differs from the declared one, each cell
from the weighted sum to the declared weighted mean (the covered
weight is 4, the homes' weights 1.0 + 2.5 + 0.5, so every moved cell's
relative change is -0.75):

- `Outdoor Pressure (kPa)`: period 0, 405.3 to 101.325.
- `Outdoor Humidity Ratio (kg/kg)` (a ratio): period 0, 0.0184815804823842064
  to 0.0046203951205960516.
- `Wind Speed (m/s)`: period 0, 1.825 to 0.45625.

The `covered_weight` product moves nothing (the covered weight is
rule-independent), and every other column, the row counts and the
column order compare bitwise. A verbosity-3 run of the same inputs
(the fleet goldens' tier family) moves nothing: its power,
temperature and enum columns' two rules agree in both worlds.

The acceptance tests' before and after values (before, run at the
parent `ff39b24b` over the tests alone; after, at `a39d17a7`):

- `fleet_fold_follows_declared_kinds`: the ratio cell 1.15 to 0.2875
  (the unit-keyed weighted sum to the declared weighted mean; the
  test's first failure, at period 0, so the state and the intensive
  `V` quantity cells stand at their parent sums behind it). The
  `count` quantity's cell is the weighted sum in both worlds (its
  declared across-homes rule is the unit-keyed fallback's).
- `home_reducer_sums_a_quantity_declared_sum`: the reduced period 2.0
  to 6.0 (the mean of 1, 2 and 3 to the declared sum).
- `fleet_cells_follow_declared_kinds`: `Outdoor Pressure (kPa)` period
  0, the cell 405.3 against the declared 101.325; after the fix, every
  non-enum column's every period cell equals the declared combination
  of the three homes' period values, each computed from that home's
  own Timestep Memory report by the declared time rule, added in entry
  order and compared on bits.
- `fleet_ratio_and_state_are_weighted_means`:
  `Outdoor Humidity Ratio (kg/kg)` period 0, the cell 0.0184815805
  outside the homes' period values [0.0042444, 0.0060966]; after the
  fix, every ratio and state cell lies between the smallest and largest
  of the homes' period values.

### The fleet Mode aggregation row, 2026-10-16 (the worktree `r88-b11`)
The golden record, recorded as measured: every golden unchanged. The
fleet goldens (`fleet_mixed_offset`, `fleet_same_offset_300s`,
`resstock_2025_fleet`) select the verbosity-2 tier, which holds no
`Enum` column (the mode columns start at verbosity 3), and no dwelling
golden has a fleet fold: `frame_golden compare-all` on the default
build reports 8/8 identical and on the dst build 1/1 identical. The
`Enum` column fields' schema metadata gained the `enum_labels` label
count; the goldens' schema digest holds name, type and nullability and
their values are untouched, so no golden moves.
The acceptance tests' before and after values (before, run at the
parent `1f64ff8b`; after, at `19332c1f`):
- `fleet_mode_uses_summed_weight`: the cell 1.2 to 2.0 (the weighted
  mean of codes 0, 0 and 2 at weights 1, 1 and 3, to the weighted
  Mode), the covered weight 5.0 both ways.
- `fleet_mode_ties_break_to_the_lowest_code`: the cell 1.5 to 1.0 (the
  two homes' codes 2 and 1 mean to a non-code; the tie breaks to the
  lowest code), the covered weight 2.0 both ways.
- `home_mode_over_time_is_the_most_frequent_code`: the reduced period
  1.3333333333333333 to 2.0 (the probe's codes 0, 2 and 2 mean to a
  non-code over time; the most frequent code wins after).
- `fleet_enum_cells_follow_mode`: `Air Conditioner Mode (-)` period 39
  read 4.5 at the parent where the declared weighted Mode is 0.0; after
  the fix, every `Enum` column's every period cell equals the code with
  the greatest summed weight of the homes' own period codes, each the
  most frequent code of that home's own steps by the declared rules,
  compared on bits, and every covered-weight cell equals the voting
  homes' weight.
- `fleet_binary_state_is_weighted_duty_fraction` and
  `fleet_mode_ignores_null_homes`: 0.25 and the null rule at the parent
  and after (pins, unchanged).
The run's `Enum` columns and their delta: the verbosity-3 selection of
the `fleet_same_offset_300s` homes holds 15 `Enum` columns, the
per-equipment operating modes (`Air Conditioner`, `Clothes Dryer`,
`Clothes Washer`, `Cooking Range`, `Dishwasher`, `Exterior Lighting`,
`Garage Lighting`, `Gas Furnace`, `Gas Water Heater`, `Indoor Lighting`,
`MELs`, `PV`, `Refrigerator #1`, `Refrigerator #2`, `TV`, each
`{name} Mode (-)` with 13 labels), 192 periods each. Every period's
cell moves from the two-stage weighted mean of mode codes to the
weighted Mode: the cells that differed from the declared mode at the
parent are exactly the periods whose homes' period codes disagree in a
way a mean misrepresents (a mean can be a non-code, or a code no home
reported); the cells the mean and the Mode agree on are the periods
every voting home reported one code. The first moved period in column
order is `Air Conditioner Mode (-)` period 39 (4.5 to 0.0, above). The
covered-weight columns do not move: the voting homes' weight is the
same before and after, the validity of a mode cell is unchanged by the
rule switch.
The EV code columns and the compressor speed column (`Connection State
(-)`, `Charging Level (-)`, `Speed (-)`) fold by the same rule where a
selection holds them (verbosity 8 and the compressor tier); the
verbosity-3 run above carries none, so their movement is the unit
tests'. Binary state columns (`Schedule (-)`, `Defrost State (-)`) do
not move: the duty fraction is pinned.
### The metrics-verbosity row, 2026-10-16 (the worktree `r89-b7`)
The entry's measurement, per golden: the metrics delta at verbosity 0
before and after, the after equal to the verbosity-8 values. Before
(read at the parent commit `62763042`, the acceptance probe over the
`cz2a_gas_furnace_ac_res_wh` inputs, 7 days at 3600 s, warm-up
disabled, `metrics = true`): at verbosity 0 the metrics carry an empty
`per_end_use` in both energy and peak (the end-use aggregates are
verbosity-1 columns the Verbosity(0) union lacks), `envelope_loads_kwh`
None (no verbosity-4 delivered column and no verbosity-6 gain column in
the union), both HVAC COPs None (the delivered slots resolve None), and
`renewable_energy_fraction` None without PV; at verbosity 8 the same
run computes the full set (per_end_use 8 entries, the envelope map, the
COPs Some). The totals, gas, rolling peaks and grid peaks are identical
at 0 and 8 already: the defect is the components' availability, not
their values.

After the fix every verbosity and the no-report run are bitwise equal
and equal the parent's verbosity-8 values on bits (net
192.1459400473185, window solar 20.45049862961817, heating COP
25.000046265309397, the same 8 end uses). The verbosity-8 metrics are
unchanged by the row: the full producer surface is the verbosity-8
union (all non-diagnostics catalogue variables in catalogue order), so
the plan's slots and the producers' values are the same bits.

The goldens' movement, read off `frame_golden compare-all` against the
parent's committed goldens: every difference is a metrics field; every
frame product and schema is bitwise. The moved fields are exactly
three, all null to value: `envelope_loads_kwh` (null to the 11-entry
component map), `efficiency.hvac_heating_cop`, and
`efficiency.hvac_cooling_cop` (None where the home's end-use electric
stayed at or under the epsilon, as fleet_mixed_offset home 0's cooling
did). Per golden, differing metrics fields: `consumer_shape_900s` 3
(heating 39.19207139825246, cooling 2.8496322866695993),
`fleet_mixed_offset` 8 (home 0 heating 39.04530965758018, cooling None;
home 1 heating 1.3936443044181308, cooling 0.0; home 2 heating
39.192071398252764, cooling 0.0), `fleet_same_offset_300s` 9 (three
homes, heating 39.19... family, cooling 0.0 each),
`full_year_hourly` 3 (heating 25.000046265309333, cooling
1.438338904146268), `leap_year_february_900s` 3 (heating
1.4514610069063882, cooling 0.0), `month_boundary_billing_3600s` 3
(heating 25.000046265309397, cooling 0.0), `weather_wrap_900s` 3
(heating 1.206260996423724, cooling 0.0), `full_year_900s_dst` 3 (on
the dst build; heating 1.3835810203963161, cooling 1.5616858307293597),
`resstock_2025_fleet` identical (no metrics rows captured). The three
`fleet_same_offset_300s` homes' heating COPs are 39.19207139825271,
39.19207139825278 and 39.192071398252764, cooling 0.0 each. No
`renewable_energy_fraction` and no `per_end_use` field moves anywhere:
the verbosity-1 aggregates were already in every verbosity-2 manifest's
union. The exposed COP magnitudes are the verbosity-8 values the parent
always computed; the gas furnaces' ratios are delivered thermal over
the end use's electric (the fan draw), as the formula documents.

The goldens re-captured TWICE BYTE-IDENTICAL (compare-all 8 + dst 1),
the two captures' sha256 equal per manifest; after the re-capture
compare-all is green on both builds.

### The ResStock weather year and offset row, 2026-10-16 (the worktree `r91-b9`)

The B-9 delta on `resstock_2025_fleet`, read with `frame_golden delta`
against the parent `588b4c51`'s materialized frames: the movement is the
timestamps only, and every energy, power and weather column is bitwise
unchanged.

- `aggregate`, `covered_weight`: the `Time` column's 48 cells move,
  row 0 `2019-01-01T08:00:00Z` to `2018-01-01T08:00:00Z` and in step to
  the end of the run; no other column of either product differs (0
  differing cells). The start moves from the hardcoded 2019-01-01 00:00
  +00:00 to the weather file's own year at the site's standard offset
  (2018-01-01 00:00 -08:00, the HPXML Site's own UTCOffset for the
  Washington homes, county G5300330); the `Z` rendering is the fold's
  +00:00 alignment for a single-offset fleet, the same string shape in
  both runs.
- `homes`, `fleet_report`: bitwise identical (every per-home total,
  status, weight and the construction report's counts).

Why nothing else moves: the parent's run started 2019-01-01 00:00 +00:00
under the typical rule and read the file's Jan 1 records at its first
steps (the calendar addressing, row 81); the row's run starts the
file's own year 2018-01-01 00:00 -08:00 under the actual rule and reads
the same records of the file at the same step indices (the same month,
day and hour of an hourly file; the schedule's month-and-day rows
likewise, both years non-leap). The 2-day fixture's energies are
identical bit for bit; the timestamps carry the whole movement. The
entry's "annual energy per home" premise does not arise on the fixture:
the duration is 2 days and no energy column moves.

The capture re-run twice byte-identical (the golden's sha256 equal,
`be37d362748c7c8e9949676a042794418b2343a93b7cbc14758ec14cb5066c25`);
compare-all 8 identical, the dst set's `full_year_900s_dst` identical.
No physics comparison suite moves: the row changes no engine physics
path (the fleet's construction declares the config; the dwelling,
environment and solver surfaces are untouched and the full workspace
suite's BESTEST and parity tests pass unchanged).

### The fleet local time labels row, 2026-10-16 (the worktree `r93-b12`)

The B-12 delta, read with `frame_golden delta` against the parent
`cf3985f3`'s materialized frames: the movement is the `Time` labels
only, in the fleet's `aggregate` and `covered_weight` products of the
three fleet goldens; every energy, power and weather column and the
`homes` (and `fleet_report`) products are bitwise unchanged, and the
row counts are unchanged (the alignments' period semantics untouched).
The delta command's per-column statistics are skipped where the schema
differs (the `Time` column's type moved), so the labels were read off
the parent's materialized Parquet against a run of the fixed engine.

- `fleet_same_offset_300s` (FL-1, undeclared, homes at UTC-8): the
  labels move from the UTC `Z` strings to the homes' local wall clock,
  row 0 `2018-01-01T08:00:00Z` to `2018-01-01 00:00` and row 191
  `2018-01-03T07:45:00Z` to `2018-01-02 23:45`, the -8 h shift, 192
  cells per product; `arrow_type` `Utf8` to `Timestamp(ms)`; no other
  column differs (0 differing cells); `homes` identical.
- `resstock_2025_fleet` (B-9a, undeclared, the same UTC-8 offset): the
  same shape, row 0 `2018-01-01T08:00:00Z` to `2018-01-01 00:00` and
  row 47 `2018-01-03T07:00:00Z` to `2018-01-02 23:00`, 48 cells per
  product; `homes` identical.
- `fleet_mixed_offset` (M-2, declared `Instant(+00:00)`): the labels
  carry the same instants at the declared reference zone's wall clock
  (+00:00, the instant itself): row 0 `2018-01-01T08:00:00Z` to
  `2018-01-01 08:00` and the last row `2018-01-08T04:00:00Z` to
  `2018-01-08 04:00`, 165 rows in both runs; the movement is the type
  and rendering (the RFC 3339 `Z` string to the naive timestamp); no
  other column differs; `homes` identical.

The schema metadata gains `hares:utc_offset_s` on both products of
every fleet run that has one offset (the single-offset fleets' shared
site offset, or the declared reference zone: -28800 on the first two,
0 on the declared `+00:00` mixed offset); a `LocalClock` fleet records
none (each home's own wall clock). The six non-fleet goldens compare
identical (compare-all 8 after the re-capture) and the dst set's
`full_year_900s_dst` is identical. The three fleet goldens re-captured
TWICE BYTE-IDENTICAL, the two captures' sha256 equal per manifest
(`fleet_mixed_offset`
`a0a250302f75f21556a5483731bab4ca62246f6a9270e9cb4d6f64744c539d1a`,
`fleet_same_offset_300s`
`a0ee127390a1f1e674f917061e2e819b30bea3953c8f6553d0d7993d66b3e79c`,
`resstock_2025_fleet`
`6f0aacb4e697868b2ffc49a3d85a8745ad71973d1272558f1684f0a99dafa0dd`).
No physics comparison suite moves: the row changes no physics path
(the fleet's label rendering and its schema metadata only; the fold's
keys, weights and arithmetic untouched) and the full workspace suite's
BESTEST and parity tests pass unchanged.
### The opaque-R precedence row, 2026-10-16 (the worktree `r94-b13`)
The B-13 delta, per golden, as annual heating and cooling energy
(`envelope_loads_kwh.hvac_heating_kwh` / `.hvac_cooling_kwh`, the
committed golden's value against the re-captured one; `resstock_2025_fleet`
captures no metrics rows, so its delta reads off its frame products):
- `consumer_shape_900s`: heating 3592.57 to 4374.14 kWh (+781.58),
  cooling 3272.71 to 3457.43 kWh (+184.71).
- `full_year_hourly`: heating 9188.65 to 9356.33 kWh (+167.68), cooling
  703.46 to 734.70 kWh (+31.24).
- `full_year_900s_dst`: heating 9781.93 to 9956.20 kWh (+174.27), cooling
  758.10 to 794.95 kWh (+36.85).
- `month_boundary_billing_3600s`: heating 764.03 to 777.46 kWh (+13.43),
  cooling 0.0 unchanged.
- `leap_year_february_900s`: heating 197.90 to 200.69 kWh (+2.78), cooling
  0.0 unchanged.
- `weather_wrap_900s`: heating 293.43 to 299.16 kWh (+5.73), cooling 0.0
  unchanged.
- `fleet_mixed_offset`: heating 693.81 to 688.26 kWh (-5.55), cooling 0.0
  unchanged. The one golden whose heating falls: home bldg0000002's
  Floor1 declares 2.10 IP (0.3698 m²·K/W) against a "Standard" row whose
  layers sum to 1.32 m²·K/W, so k = 0.16 there and the floor loses
  resistance; the home's other declared boundaries move under 1%.
- `fleet_same_offset_300s`: heating 96.91 to 94.62 kWh (-2.29), cooling
  0.0 unchanged (the same home bldg0176227 three times, its own
  boundaries' k in [0.9874, 1.0985]).
- `resstock_2025_fleet`: no metrics rows captured; the three homes'
  frame products move in the power and temperature columns.
The direction is mostly up in heating because most declared assembly
R-values sit slightly below their matched row's construction R in the
LUT's convention, so k < 1: the distribution of k over the homes'
declared boundaries is [0.9874, 1.0985] over bldg0176227's 17,
[0.1601, 1.0047] over bldg0000002's 7, [0.4136, 1.0047] over
bldg0000004's 6, and [0.5521, 0.9946] over the cz2a and cz4a parity
homes' 10 each (the Est. R-3.1 garage walls 0.5521, the R-15 walls
0.9275, the uninsulated roofs 0.9311, the doors 0.9946). Every k is
positive and finite; no boundary's declared R touches its convention
film sum (the minimum declared R in the corpus is 0.3698 m²·K/W against
0.2642 m²·K/W, the floor/ceiling constant).
The goldens re-captured TWICE BYTE-IDENTICAL per manifest (the two
captures' sha256 equal; compare-all 8 on the default build and the dst
set's `full_year_900s_dst` on the dst build, both green after the
re-capture). The corrected acceptance fixture's own pins: the
`diagnostics_cz4a.csv` pin re-captured (the preamble's provenance lines
identical, the zone temperatures and the opaque LWR column moved at the
printed precision), and the structural envelope oracle's per-boundary UA
comparison for declared-R groups pinned to the declaration's expectation.

### The surrogate-search trial, 2026-10-17 (the row 98 worktree `r98-cn12`, report only)

The consumer entry's trial, run as it specifies and recorded here because the row
lands no consumer or engine change. The question: can a surrogate model, a cheap
predictor of the fit's loss surface, cut the production fit's engine-run count at
the same fit quality? Scope note first: this is the report-only execution. The
consumer tree is untouched: no dependency in the manifest, no registered tool, no
`docs/Surrogate_Trial.md`; the entry's registered `tools/surrogate_trial.py`, its
check-mode acceptance case and its `trial` dependency group belong to the adoption
row an operator decision would open. egobox 1.0.0 was installed into the consumer
venv only (`uv pip install egobox`; the manifest and `uv.lock` unchanged, and
`uv sync --locked --extra engine --dry-run` reports no changes; the venv's ruff
stays the lock's 0.16.10). The trial tool is `scratch/surrogate_trial.py`
(scratch/ is gitignored by design) and its raw numbers are
`scratch/surrogate_trial_results.json`; to reproduce, `uv pip install
egobox==1.0.0`, then run the commands below.

The trial's baseline is the recorded B0 production capture
(`baselines/B0-de-trace.json`), because row 97 (the B1 re-baseline) has not
landed. Row 96's gate re-captured the DE trace twice on this engine and consumer
and it was bitwise identical to B0, so B0's per-premise values are this tree's DE
behaviour; run counts are the recorded `n_eval` plus the one TOWT warm-start
regression run `n_eval` never sees (`scale.py`'s `engine_runs`).

The design, as run. Production set: `feeder.build_group("G4802010",
mix=scale._mix_for(4))[:4]`, seed 0, a 365-day year, the `Fitter` built exactly as
`calibrate_premise` builds it (the full per-home resolved set: 9, 8, 10 and 10
factors), the oracle the same batched `Fitter.evaluate` with the 1e6 PENALTY for a
refusal, the strict health policy armed on every evaluation. Two methods, each
scored exactly as the production capture scores its fit: in-sample F0 and
out-of-sample F1 through `Experiment.candidate_series` and `scale._score`.

- BO, egobox 1.0.0 `Egor`: a Gaussian-process surrogate with expected-improvement
  infill, qEI batch 4, seed 0. The initial design is 10 points evaluated as one
  batch: the nominal vector, the TOWT warm start (its regression is one engine run
  outside `evaluate`, counted), and 8 egobox LHS draws (seed 0). The infill
  iteration count was fixed per premise before the run so the BO budget is half
  the premise's recorded engine runs: 55, 48, 61 and 61 iterations of 4, so 231,
  203, 255 and 255 engine runs against the recorded 462, 411, 513 and 513.
- M+DE, Morris screening then DE on the retained factors: elementary effects
  hand-rolled (numpy `default_rng(0)`), 8 trajectories, one batched `evaluate` per
  trajectory (n+1 points), delta 0.2 of each factor's span in unit space, the
  perturbation sign alternating and the dimension order shuffled per trajectory;
  the retention rule fixed before the run (mu* at least 10 percent of mu*_max, at
  least 3 and at most 8 factors); then the Fitter's own DE protocol on the
  retained box (`generations=16`, seed 0, the default population rule
  `max(12, 3n)`).

The numbers. Loss is the fit objective, lower is better; "out" is the F1 monthly
CVRMSE, the honest one (`scale.py`). The baseline's runs are the recorded
`n_eval + 1`:

| premise | baseline runs / loss / out | BO runs / loss / out | M+DE runs / loss / out |
|---|---|---|---|
| 0 ashp bldg0053954 | 462 / 0.597421 / 5.08 | 231 / 0.578216 / 3.28 | 489 / 0.599948 / 3.11 |
| 1 elecfurn bldg0092742 | 411 / 0.741001 / 4.02 | 203 / 0.713439 / 2.53 | 430 / 0.749879 / 4.65 |
| 2 gasfurn bldg0031262 | 513 / 0.593824 / 3.97 | 255 / 0.548655 / 3.34 | 293 / 0.565166 / 3.95 |
| 3 gasfurn bldg0063724 | 513 / 0.546443 / 3.56 | 255 / 0.563510 / 4.40 | 293 / 0.759215 / 16.56 |

M+DE's totals are screening plus retained-box DE: 80 + 409, 72 + 358, 88 + 205 and
88 + 205 runs (the retained sets 8/9, 7/8, 3/10 and 3/10 factors; populations 24,
21, 12 and 12). Loss deltas against the baseline: BO -3.21, -3.72, -7.61 and
+3.12 percent; the retained-box DE +0.42, +1.20, -4.83 and +38.94 percent.

The trajectories. BO's best-so-far plateaus by about 100 runs on premises 0 to 2
(premise 0: 0.6120 at 50 runs, 0.5782 at 98, its final 0.5782; premise 1: 0.8813
at 50, 0.7134 at 150, its final; premise 2: 0.6085 at 50, 0.5487 at 98, its
final), and keeps creeping on premise 3 (0.5742 at 98, 0.5706 at 198, 0.5635 at
255) without reaching the baseline's 0.5464. The baseline DE's traces run 17
generations each (the initial batch plus 16), from first-generation minima
0.6710, 0.9052, 0.8376 and 0.6780 to their finals.

Surrogate fidelity, measured by the trial rather than taken from egobox's
internals: a 5-fold cross-validated q2 of a GP fitted on every point the BO stage
evaluated (egobox's default `GpMix` configuration), on the penalty-free points:
0.841, 0.622, 0.931 and 0.594 per premise (Pearson r 0.917, 0.798, 0.967 and
0.770). With the refused 1e6 penalties included, premise 2's q2 falls to -0.184:
the one premise where the BO stage drew refusals (6 of 255 evaluations; the
engine's strict construction rules reject some corners of the box, and a GP
cannot interpolate a 1e6 cliff). Morris's own honesty check: the factors the
baseline fits actually moved (the fitted value at least 2 percent of span from
nominal) number 9 of 9, 8 of 8, 10 of 10 and 9 of 10, so the surface is not
sparse, and the elementary effects measured from uniform-random box points rank
the heating factors last on both gas premises (heating_eff_scale 10th of 10 on
premise 3) while the baseline fits move them: the directions that matter are
visible only near the good region, not from the box at large.

Reproducibility: every method is seeded (egobox `Egor` seed 0, `lhs` seed 0,
numpy `default_rng(0)`, the Fitter's own seed 0; budgets derive from the recorded
baseline counts, fixed inputs). The tool's check mode re-ran premise 0 with the
report's seeds and compared its loss, in- and out-of-sample scores and
engine-run counts with the stored row on bits: PASS, exit 0. The provenance the
report carries: consumer `24c27a72` (main), engine `1d91a684` (the submodule pin,
build clean, sha256 `4c45b677756c74ad`), egobox 1.0.0, host `Rich-AZX`, 13-worker
pools, premises run sequentially (`scale.py`'s measured claim: per-home results
byte-identical serial versus concurrent).

The gates on the same tree: the consumer `run_all` 383 passed, 0 failed, 7
skipped (the base seven), ruff (the configured select, E9 and F821) clean; the
engine repo untouched. No golden, baseline or recorded number moves (report
only).

The verdict: NOT WORTH ADOPTING as the production fit stands, with one measured
positive an operator may want to pursue as a new entry.

1. Morris screening followed by DE is rejected on this evidence. The surface is
   not sparse (above), so the screening saves runs only where it breaks the fit:
   on premises 0 and 1 it retains 8/9 and 7/8 factors and the method totals 106
   and 105 percent of the baseline's runs for a worse loss; on the gas premises it
   retains 3 factors that miss the heating directions, and premise 3's
   retained-box DE lands 38.9 percent above the baseline's loss at 16.56 percent
   out-of-sample monthly CVRMSE. The retention is also unstable across two
   same-archetype homes (the same 3 retained factors: premise 2 -4.83 percent,
   premise 3 +38.94 percent), so no fixed retention rule can serve the fleet.
2. BO halves the run count (49 to 50 percent of the recorded runs) and matches or
   beats the baseline's loss on 3 of the 4 premises while improving the
   out-of-sample monthly CVRMSE on the same three (5.08 to 3.28, 4.02 to 2.53,
   3.97 to 3.34 percent), but regresses the fourth (premise 3: loss +3.12
   percent, out-of-sample 3.56 to 4.40 percent; the weakest GP fidelity, q2
   0.594). One seed over four premises does not clear the entry's bar (the same
   fit quality at fewer runs); a method that loses one home in four is not a
   drop-in for the production fit.
3. The positive worth a new entry: BO's first 100 runs already sit within about 3
   percent of the DE's final loss on 3 of the 4 premises, and a BO premise's wall
   cost was 83 to 109 s against the retained-box DE's 34 to 73 s (the same
   13-worker pool). A budgeted BO stage seeding or bracketing the DE, measured
   over more premises and seeds with the out-of-sample regression watched, is the
   only shape this trial measured that could cut the count without premise 3's
   risk. Adopting anything was never part of this row; that study needs its own
   entry and an operator decision.

### M-4a, 2026-10-17, the cross-platform workflow (unrun)

Commit `5da29cb4` (branch `row/r99-m4a`, worktree `r99-m4a`, based on
`f14beb17`; this record lands in the next commit).

The entry's workflow is `.github/workflows/reproducibility.yml`. Triggers:
`workflow_dispatch` and a pull request into `initial-implementation` (BR-10's
triggers; no feature-branch name appears in the file). One job over a matrix
of `ubuntu-24.04` (x86_64) and `macos-15` (arm64) with `fail-fast` off, so one
leg's outcome cannot cancel the other's. Each leg runs the
`rust-toolchain.toml` pin (BR-10's golden-job release, through the same
`dtolnay/rust-toolchain@stable` mechanism the main `ci.yml` golden job uses),
builds `frame_golden` in release with and without `-F dst`, writes the full
products of every golden manifest with `run --output-dir` (one directory per
manifest under `target/reproducibility/`, the flat per-product parquet layout
`frame_golden diff` consumes), and runs `compare-all` per feature set. Each
pass's exit status is written to a file instead of failing the step, so a
macOS difference cannot skip the remaining passes; a verdict step aggregates
the statuses and fails the leg on any difference. The artifact
`reproducibility-<runner>` (uploaded on success or failure) carries the
platform record (runner name, architecture from `RUNNER_ARCH` and `uname -m`,
`rustc -V`, `cargo -V`, the head), the full products and both compare-all
outputs.

The agreement rule is M-1's compare: schema, row count, the per-column bit
digests (floats by `to_bits`, so NaN payloads count) over the whole column and
per 96-row block, and every metrics field on bits. The committed goldens are
Linux x86_64 captures, so the Linux leg's `compare-all` must pass; the macOS
arm64 leg is the cross-platform measurement, identical or not, and M-4b
publishes either outcome.

No hosted run exists: this environment has no macOS arm64 runner and the
branch is unpushed, so the workflow lands UNRUN and row 100 (M-4b) waits on a
run. What was verified locally at the commit, on the Linux x86_64 side
(rustc 1.99.0, the pinned release, `CARGO_BUILD_JOBS=2`, the warm target of
the main checkout at the same commit): the workflow's exact command list ran
green. The products step ran all nine manifests, and `compare-all` is
identical in the default build (8: `consumer_shape_900s`, `fleet_mixed_offset`,
`fleet_same_offset_300s`, `full_year_hourly`, `leap_year_february_900s`,
`month_boundary_billing_3600s`, `resstock_2025_fleet`, `weather_wrap_900s`)
and in the `-F dst` build (`full_year_900s_dst`); the compare outputs and the
products live under `/tmp/opencode/r99-m4a-verify/`. The workflow's YAML
validates by parse (actionlint unavailable on this host); the main `ci.yml`
and the engine's tests are untouched by the commit.

### The generated-schedule setpoint row, 2026-10-17 (the worktree `r94ab`)

The generated schedule's constant setpoint columns are gone from the
engine, and the two acceptance runs are the measurement. This host's
history carries the change already: the phase-1 fold `c561eb69` replaced
the HPXML-schedule generator (which wrote `heating_setpoint` and
`cooling_setpoint` columns holding one constant each, the HPXML weekday
profile's first hourly value, or 20.0 C and 24.0 C when the HPXML had no
thermostat) with the profile generator that writes no setpoint column, so
each side resolves by the setpoint-source rule to the HPXML profiles or
the default profile. The row's commit lands the acceptance tests and the
per-side debug log of the chosen source; the numeric state is the fold's.

RED, against the reverted source: with the generator locally restored to
writing the two constant columns (20.0 C heating, 24.0 C cooling; the
literals the fold deleted), both acceptance tests fail at the same parent
`generated_schedule_keeps_the_hpxml_setpoint_profile` on the read-back
(`Gas Boiler`'s heating source `ScheduleColumn { column: "heating_setpoint" }`,
want `Hpxml` with the 48 hourly values) and
`generated_schedule_without_thermostat_uses_the_default_profile` on the run
(step 0 reads 20.0 C, the literal, want the default profile's 68 F
conversion). GREEN with the generator as landed: both pass, with
`crates/hares-io`'s no-column pins and the injection test
`hvac_without_a_schedule_file_keeps_the_hpxml_hourly_setpoints` around
them.

The acceptance run's every variable, before and after: the run of the
entry's fixture (`bldg0000002`, no schedule file, weather `G0900090_2018.csv`,
start `2018-01-05T00:00:00-05:00`, 3 d at 3600 s, no warm-up) dumped every
report column's every step as raw bits, the resolved sources and the
warnings, once at the parent `ff39b24b` (the tree plus only the dump
harness, an untracked scratch test since deleted) and once at the row's
final state: the two dumps are byte-identical (176 lines each, `diff`
empty). The delta of the acceptance run is zero on every variable; the
cause is the fold's, recorded above.

Goldens: compare-all 8 (`consumer_shape_900s`, `fleet_mixed_offset`,
`fleet_same_offset_300s`, `full_year_hourly`, `leap_year_february_900s`,
`month_boundary_billing_3600s`, `resstock_2025_fleet`,
`weather_wrap_900s`) and the dst set's `full_year_900s_dst` are bitwise
identical, compared twice on the final state. No golden manifest runs a
generated schedule (every manifest sets an explicit `schedule` path), so
no golden is expected to move and none is re-captured.

### The schedule setpoint Fahrenheit row, 2026-10-17 (the worktree `r94ab`)

The delta, on the parity inputs (`cz4a_ashp_hpwh`, 48 h at 900 s from
`2023-01-01T00:00:00-07:00`, the fixture's `schedule.csv` with a constant
`heating_setpoint` column of 70.0 appended, so the column wins over the
HPXML heating setpoints and the overlap is warned). Before: the column
read as 70 C, at the row's parent `a36f3485` (the tree plus only the dump
harness, an untracked scratch test since deleted). After: the column read
as 70 F, converted at the parse.
- Delivered heating energy (`envelope_loads_kwh.hvac_heating_kwh`, the
  run's 48-h total): 788.4359349702 to 137.3248583510 kWh (bits
  `4088a37ccb79423f` to `40612a653d572ad0`); at the 48 h to year scale
  (x 8760/48) the annual-equivalent: 143889.5581320653 to
  25061.7866490558 kWh.
- Mean zone temperature (the mean of `Temperature - Indoor (C)` over the
  run's 192 steps): 64.2784703820 to 21.1235089246 C (bits
  `405011d2756ff305` to `40351f9e47e7d030`).
Both fall, the direction the defect predicts: the Fahrenheit file read as
Celsius drove the thermostat at 70 C (the home overheated until the
engine's safety cutoff held the zone near 64.3 C mean and the heating
energy ballooned); the Fahrenheit read heats to the converted 21.11 C.
Goldens: compare-all 8 and the dst set's `full_year_900s_dst` bitwise
identical, compared twice on the final state. The census behind it:
`grep -l -E '(heating|cooling)_setpoint' -r tests/fixtures/golden
tests/fixtures/resstock` lists no schedule (only the metadata parquet's
binary match), so no golden manifest's schedule carries a setpoint
column; any golden change would have stopped the row, none is re-captured.
RED: at the parent `setpoint_column_is_read_in_fahrenheit` (the heating
leg, on bits) and `schedule_column_wins_over_hpxml_and_warns` (the run's
heating setpoint the column's converted value, on bits) fail reading
70.0 as 70 C (the bits of 70.0 where the conversion is wanted); GREEN at
the row, both legs (the cooling leg's witness is the cooler's own
setpoint column from a July start: a January run's compressor outdoor
lockout never evaluates the cooler's control, and the dwelling-level
scheduled-cooling read reports the first HVAC unit's telemetry, a
heating-only unit on this fixture).

### Hardening, 2026-10-17, defrost consolidation (branch `row/hw1-defrost-boiler`)

Load average at write time: 17.24 (1 min), three hardening agents building
concurrently at two jobs each. Command: the defrost probe's accumulated-energy
runs (48 h at 10 s steps, 1 C / RH 85 percent, static operating point, 48 h
measured after warm-up; the probe test in `crates/hares-equipment/tests/hvac_parity.rs`),
then `frame_golden compare-all` (default and dst feature sets).

- Discrete-FSM duty (the Defrosting-step fraction): 0.01053 to 0.09442, the
  continuous model's time fraction at these conditions 0.09395. Before: the
  trigger divided the cycle duration by the time fraction while frost
  accumulated at dt x tf per step, so the FSM fired at about tf^2 of its
  documented duty.
- Defrost-electric integral, discrete run over continuous-alone run: 2.11x
  before (the full-intensity events stacked under the continuous average's
  own metering), 1.0050 after (the event draw is the continuous average
  concentrated into the event window).
- Delivered heat, discrete run over continuous-alone run: 1.0003 after (the
  plan's 1 percent band).
- Resistive-defrost destination: the discrete event's element heat went 100
  percent to the zone; after, the configurable zone fraction (default 0)
  and the remainder is frost-melt loss.
- Goldens: `fleet_mixed_offset`, `leap_year_february_900s`,
  `weather_wrap_900s` and `full_year_900s_dst` moved (the ASHP-bearing
  fixtures: the corrected duty and the single application), re-captured
  twice byte-identical. `crates/hares-core/tests/fixtures/diagnostics_cz4a.csv`
  re-pinned (the run's zone temperature diverges from row 6, the ASHP's
  first-on step, 17.7979 to 17.7990 C at that row).
- The 48 h OCHRE thermal trace (1-minute steps, BEopt example, May 5 2019):
  HARES heating 31.3747 to 28.9580 kWh (OCHRE's own 47.052 kWh unchanged),
  the overnight worst divergence against OCHRE 1.2309 to 0.7800 C (the mean
  0.3823 to 0.3680 C): the discrete cycle's events concentrate the
  continuous average instead of stacking a second duty factor under it, so
  the ASHP tracks OCHRE's continuous model more closely. RECORD NOTE
  2026-10-18: an interim state of this record claimed those numbers were
  "a different scenario's" and re-pinned the tests to 31.7942 kWh and
  1.2309 C; that interim was measured on a stale extension built before
  the defrost fix reached the Python venv, and the canonical-env rebuild
  after the SI fold restored the true measurements (28.9580, 0.7800): the
  constants now carry the true values again; the lesson recorded: pytest
  records re-verify only after maturin develop, never on a stale build.

### Hardening, 2026-10-17, the boiler's combustion-loss split (branch `row/hw1-defrost-boiler`)

Load average at write time: 17.24 (1 min) at the defrost entry's write,
the same session. Command: the jacket-vs-flue destination tests
(`crates/hares-equipment/tests/combustion_tests.rs`, a non-condensing
AFUE 0.80 boiler at 10 kW, full load, steady state), then
`frame_golden compare-all` (default and dst feature sets).

- The zone's JacketLoss gain at the test's operating point: 2179.6 W
  (the entire combustion residual) to 218.0 W (the jacket share, 0.1 of
  the residual under the default flue fraction 0.9; the flue share
  leaves outdoors). The defect at this operating point put about a sixth
  of the burned gas back into the zone as free heat.
- The gas water heater's split is bit-identical (its flue-versus-tank
  arithmetic is the efficiency model the helper's pattern extracts, and
  adopting the residual helper would reorder its floating-point
  operations).
- Goldens: `fleet_mixed_offset` moved again (the fleet's gas boiler
  homes: the flue share is no longer free zone heat, so the boilers'
  gas power and the coupled indoor temperatures move), re-captured
  twice byte-identical. Every other golden bit-identical, compared
  twice on both feature sets.

### Hardening, 2026-10-17, the defrost discrete cycle's coarse-dt duty (branch `row/hw1-defrost-boiler`)

The independent review's measurement at this branch's tip, recorded as
the draft row's evidence: at timesteps coarse against the 210 s defrost
event the event occupies whole steps, so the realized duty quantizes
upward, and at 3600 s the ASHP delivers zero heat every other frost hour.
`fleet_mixed_offset` runs 7 d at 3600 s, so the magnitude lands on a
committed golden.

- The reviewer's coarse-dt duty table (the realized duty as a multiple
  of the documented time fraction): 1.00 at 10 s, 1.12 at 60 s, 1.33 at
  300 s, 2.66 at 900 s, 5.32 at 3600 s. The duty itself at 3600 s is 0.5.
- The pre-fix coarse-dt duty was about 0.125 at 3600 s, so the fix
  quadrupled the coarse-dt over-application while fixing the fine-dt
  duty (the pre-fix fine-dt duty 0.01053 against the documented 0.094,
  the fix bringing it to 0.09442).
- Draft disposition (not landed): auto-select the continuous model when
  dt exceeds the cycle duration, or document a max dt for the discrete
  cycle. No golden moves from this record; the row is a draft.

### The constant setpoint override row, 2026-10-17 (the worktree `r94ef`, branch `row/r94ef`)

The OD-2 site 6 delta: a constant setpoint override now replaces the
schedule and HPXML setpoint sources, so the acceptance run's frame moves
where the override is set. The run is the acceptance test's own
(`tests/fixtures/hpxml/ochre_samples/base.xml`, no schedule file, weather
`USA_CO_Denver.Intl.AP.725650_TMY3.epw`, start `2023-01-15T00:00:00-07:00`,
24 h at 900 s, no warm-up, one verbosity-8 frame report, the dwelling-level
override `{"Gas Furnace": {"setpoint": {"heating_setpoint_c": 25.0}}}`),
dumped every report column's every step to parquet once at the parent
`7e78923d` (the tree plus only the dump harness, an untracked scratch test
since deleted) and once at the row's final state.

RED at the parent: `constant_setpoint_override_applies` and
`constant_setpoint_override_replaces_the_schedule_column` both fail with
the furnace's heating setpoint 20.0 where 25.0 is wanted (the HPXML tier
in the first test: base.xml's `SetpointTempHeatingSeason` 68 F expanded to
a daily profile and stamped as the side's source; the schedule tier in the
second: a constant 68 F `heating_setpoint` column read in Fahrenheit and
stamped as the source). The second test is a compile error at the parent
in its final shape (the merge's shadow removal did not exist); its
behavioral RED is witnessed by running the same run through the unmodified
merge, which reads the column's 20.0 every step.

GREEN at the row: both pass, with the merge-level pins
(`constant_setpoint_override_removes_the_matching_schedule_source`,
`constant_setpoint_override_from_the_wildcard_layer_shadows_too`,
`a_null_constant_does_not_shadow_the_schedule_source`), the reconciliation
pin `reconciliation_keeps_the_named_axis_and_opens_the_gap_on_the_other`
and the three column-source error pins around them.

The delta, per variable: 165 report columns, 36 move, 129 identical. The
36 moved columns, as means over the 96 steps (before to after):

- `Gas Furnace Setpoint (C)`: 20 to 25, `Scheduled Heating Setpoint (C)`:
  20 to 25 (the override applies, on every step).
- `Temperature - Indoor (C)`: 20.4723 to 24.841; `Temperature - Attic (C)`:
  5.86387 to 6.02538 (the zone is held at the override's constant).
- `HVAC Heating Delivered (W)` and `HVAC Heating Delivered - Indoor (W)`:
  1661.5 to 3971.35; `Gas Furnace Capacity (W)`: 1661.5 to 3971.35;
  `Gas Furnace Main Power (kW)`: 2.21657 to 5.2981;
  `Gas Furnace Gas Power (therms/hour)` and `Total Gas Power
  (therms/hour)`: 0.0756325 to 0.180779; `Gas Furnace Energy (kWh)`:
  0.0185189 to 0.0442644; `HVAC Heating End Use Electric Power (kW)`,
  `Gas Furnace Electric Power (kW)`, `Gas Furnace Fan Power (kW)`:
  0.0740757 to 0.177058; `Total Electric Power (kW)`: 1.22019 to 1.32064.
- `Gas Furnace Mode (-)` and `Gas Furnace Schedule (-)`: heating on 35
  more steps (mean 0 to 1); `Gas Furnace Runtime Fraction (-)`: 0.193283
  to 0.46199; `Gas Furnace Reactive Power (kVAR)`: 0.0419806 to 0.100343;
  `Total Reactive Power (kVAR)`: 0.374095 to 0.432458;
  `Gas Furnace Power Factor (-)`: 67 steps' value changes, the mean holds
  0.87.
- The envelope and balance columns follow the warmer zone:
  `HVAC Duct Losses (W)`: 435.986 to 1042.11; `Infiltration Heat Gain -
  Indoor (W)`: -273.883 to -390.829; `Natural Ventilation Heat Gain -
  Indoor (W)`: 0 to -2041.14 on the 13 steps the warmer zone crosses the
  ventilation threshold (0 on the rest); `Opaque Surface Heat Gain -
  Indoor (W)`: -992.091 to -1722.02; `Roof Heat Gain - Indoor (W)`:
  -238.975 to -424.802; `Floor Heat Gain - Indoor (W)`: -140.694 to
  -259.686; `Wall Heat Gain - Indoor (W)`: -612.421 to -1037.53; `Window
  Heat Gain - Indoor (W)`: -1031.02 to -1304.81; `Internal Mass Heat Gain
  - Indoor (W)`: 68.0709 to -134.768; `Net Sensible Heat Gain - Indoor
  (W)`: 3241.18 to 5155.34; `Zone Air Heat Balance Residual (W)`: -309.782
  to -569.296; `Infiltration Heat Gain - Attic (W)`: -7.6657 to -8.3357.
- `Electric Resistance Water Heater Electric Power (kW)` and `Water
  Heating End Use Electric Power (kW)`: 0.519528 to 0.516235 (74 of 96
  steps; the warmer house cools the colder-inlet hours' draw slightly).

The direction is the override's: every moved column is the heating
setpoint's own chain or a flow the warmer zone drives. No equipment
parameter moved; the read-back's resolved sources record the override
tier on the overridden side and are unchanged on every other equipment.

Goldens: compare-all 8 (`consumer_shape_900s`, `fleet_mixed_offset`,
`fleet_same_offset_300s`, `full_year_hourly`, `leap_year_february_900s`,
`month_boundary_billing_3600s`, `resstock_2025_fleet`,
`weather_wrap_900s`) and the dst set's `full_year_900s_dst` are bitwise
identical on the row's final state. No golden manifest sets a setpoint
override, so none is expected to move and none is re-captured. The
resolved product: the golden tool at this base carries no resolved
product (B-14's first commit, row 94c, adds it); the run above's own
read-back is recorded instead, unchanged outside the overridden side.
### The infiltration defaults row, 2026-10-17 (the worktree `r94ef`, branch `row/r94ef`, based on the commit `e6c29015`)

The OD-10 numeric-commit delta: the three infiltration defaults take the
OpenStudio-HPXML rules. The delta runs are the entry's own (base.xml and
the two `base-bldgtype-multifamily*` files, 7 d at 900 s with
`data/examples/BEopt_example_schedule.csv` and
`USA_CO_Denver.Intl.AP.725650_TMY3.epw`, no warm-up, one verbosity-8
frame report), dumped every report column's every step to parquet once at
the parent `e6c29015` (the tree plus only the dump harness, an untracked
scratch test since deleted) and once at the row's final state.

RED, at the parent state (the row's source changes stashed, the
acceptance tests present): `missing_infiltration_height_follows_openstudio_hpxml`,
`missing_flue_follows_openstudio_hpxml` and
`missing_shielding_follows_facility_type` are RED as compile failures
(the read-back record's `height_m`, `has_flue` and `shielding` fields do
not exist at the parent); the behavioral movement is the delta below.

GREEN at the row: all three pass, with the fixture-input assertions
first (the entry's premise: base.xml's 2700 ft2, two conditioned floors
with one above grade, the 8 ft average ceiling, the conditioned basement,
the unvented attic declared outside the infiltration volume; the
multifamily fixture's apartment-unit facility type and absent
ShieldingofHome), and the neither-element pin: a multifamily fixture
with its `ResidentialFacilityType` removed fails construction naming
`ResidentialFacilityType`.

The delta, per fixture:

- `base.xml`: 33 of 165 report columns move, 132 identical. The cause is
  the estimate's foundation term: the AIM-2's height runs 2.4384 m
  (the old fallback, the average ceiling height times the one floor above
  grade) to 2.9718 m (9.75 ft: the base term's 8 ft plus the conditioned
  basement's 1 ft above grade plus its 9 in rim joist and sill plate),
  the value OS-HPXML v1.12.0 defaults for the same file
  (`HPXMLtoOpenStudio/tests/test_defaults.rb`,
  `test_infiltration_height_and_volume`, relative 0.01). As means over
  the 672 steps (before to after): `Infiltration Heat Gain - Indoor (W)`
  -345.216 to -389.123 (the taller column's stack term pulls harder);
  `HVAC Heating Delivered (W)` and `Gas Furnace Capacity (W)` 2342.75 to
  2392.05; `Gas Furnace Gas Power (therms/hour)` and `Total Gas Power
  (therms/hour)` 0.106644 to 0.108888; `Gas Furnace Main Power (kW)`
  3.12542 to 3.19118; `Gas Furnace Energy (kWh)` 0.0261122 to 0.0266616;
  `Gas Furnace Runtime Fraction (-)` 0.272534 to 0.278269;
  `Temperature - Indoor (C)` 20.305 to 20.2998; `HVAC Duct Losses (W)`
  614.752 to 627.687; the envelope and balance columns follow at the
  same scale (`Net Sensible Heat Gain - Indoor (W)` 3179.39 to 3173.19,
  `Opaque Surface Heat Gain - Indoor (W)` -976.65 to -974.182, the
  window, wall, roof, floor and internal-mass gains within 2.5 W,
  `Zone Air Heat Balance Residual (W)` -297.764 to -296.927);
  `Electric Resistance Water Heater Electric Power (kW)` 0.397748 to
  0.397752; `Gas Furnace Mode (-)` and `Gas Furnace Schedule (-)`: one
  step crosses its mode boundary (mean 0 to 1 over the run's steps); the
  furnace's reactive power and power factor and the fan and end-use
  electric power follow the delivery. The flue flag evaluates false at
  the fixture's own systems (the gas furnace's AFUE 0.92 clears 0.89,
  the water heater is electric), the same value the parent's constant
  false ran at, so the flag's rule moves nothing here.
- `base-bldgtype-multifamily` and
  `base-bldgtype-multifamily-shared-boiler-only-baseboard`: 0 columns
  move, bitwise. The shielding default's facility-type rule (an apartment
  unit is well-shielded) has run since the phase-1 fold carried it, the
  flue rule evaluates false (the boiler's AFUE 0.92; the electric water
  heater is skipped by fuel), and the estimate's SI-native arithmetic
  returns the declared-terms-zero height on the bits the old fallback
  ran, so the runs are untouched and only the read-back's new fields
  appear.

Goldens: compare-all 8 (`consumer_shape_900s`, `fleet_mixed_offset`,
`fleet_same_offset_300s`, `full_year_hourly`, `leap_year_february_900s`,
`month_boundary_billing_3600s`, `resstock_2025_fleet`,
`weather_wrap_900s`) and the dst set's `full_year_900s_dst` are bitwise
identical, compared twice (on the staged change and on the commit). Every
golden's HPXML declares `InfiltrationHeight`,
`HasFlueOrChimneyInConditionedSpace` and `ShieldingofHome`, so no golden
is expected to move and none is re-captured. The resolved product: the
golden tool at this base carries no resolved product (B-14's first
commit, row 94c, adds it); the read-back's own new fields are recorded
above, none moving on the goldens' fixtures.

### The review round's cross-tier deadband guard, 2026-10-17 (the worktree `r94ef`, branch `row/r94ef`)

The runtime guard observes only: the cross-tier deadband detection reads
the resolved setpoint pair each step and raises no delivery, so no
report column moves on any run. The probe (base.xml over the Denver
TMY3 weather, 24 h at 900 s, no warm-up, the schedule file carrying a
`cooling_setpoint` column of 68 F and the override's heating constant
25 C): the January day delivers heating and cooling on the same steps
(the furnace's override crosses its surviving HPXML cooling profile at
25.5556 C, the air conditioner's HPXML heating profile meets the
schedule column at 20 C), 94 of 96 steps both positive, and the run
warns once per equipment naming the equipment, the step and both tiers,
with the collision steps counted in `RunHealth.cross_tier_deadband_events`.
The merge-time scan's construction warning names the equipment and both
values and moves no number. The checkpoint's run-health section gained
the counter: `CHECKPOINT_VERSION` 18 to 19, the schema fingerprint
re-pinned. Goldens bitwise (compare-all 8 + dst 1), none re-captured.

### The element-less fixtures' census, 2026-10-17 (the worktree `r94ef`, branch `row/r94ef`)

The infiltration-defaults row's delta covered the entry's own three
fixtures (base.xml and the two `base-bldgtype-multifamily*` files); the
census covers the rest of the corpus that lacks the elements, one row
per fixture: the air infiltration measurement declares no
`InfiltrationHeight` in all 14 `ochre_samples` fixtures and no
`HasFlueOrChimneyInConditionedSpace` in any, and `ShieldingofHome` is
declared only in base.xml and base-battery (both `normal`). The old
height is the parent fallback's exact expression (the ceiling height
times the floors above grade), the old flue the parent's constant
false, the shielding the same facility-type rule both sides ran. A run
moves exactly when its height or flue moved: the AIM-2 coefficients read
those (and the unchanged shielding and terrain), so the rest of each
run is bitwise.

| fixture | height (parent to row, m) | flue (parent to row) | shielding | run |
|---|---|---|---|---|
| base-appliances-dehumidifier | 2.4384 to 2.4384 | false to false | normal, the rule | bitwise |
| base-battery | 2.4384 to 2.9718 | false to false | normal, declared | moved |
| base-bldgtype-multifamily | 2.4384 to 2.4384 | false to false | well-shielded, the rule | bitwise |
| base-bldgtype-multifamily-shared-boiler-only-baseboard | 2.4384 to 2.4384 | false to false | well-shielded, the rule | bitwise |
| base-dhw-multiple | 2.4384 to 2.9718 | false to true | normal, the rule | moved |
| base-enclosure-garage | 2.4384 to 2.9718 | false to false | normal, the rule | moved |
| base-enclosure-windows-physical-properties | not runnable standalone: the window-property rule rejects the parse before any infiltration read; its declared geometry is base.xml's (21600 ft3 over 2700 ft2, two floors with one above grade, the conditioned basement), so its terms are the 9.75 ft form, flue false (the AFUE 0.92 furnace, the 0.95 EF water heater) |
| base-foundation-basement-garage | 2.4384 to 2.9718 | false to false | normal, the rule | moved |
| base-hvac-multiple | 2.4384 to 2.9718 | false to true | normal, the rule | moved |
| base-lighting-mixed | 2.4384 to 2.9718 | false to false | normal, the rule | moved |
| base-misc-loads-large-uncommon | 2.4384 to 2.9718 | false to false | normal, the rule | moved |
| base-pv | 2.4384 to 2.9718 | false to false | normal, the rule | moved |
| base-pv-battery | 2.4384 to 2.9718 | false to false | normal, the rule | moved |
| base.xml (the row's own record) | 2.4384 to 2.9718 | false to false | normal, declared | moved |

The two flue risers: `base-dhw-multiple`'s second water heater is a gas
storage tank at EF 0.59, below the 0.63 threshold, and
`base-hvac-multiple`'s second furnace is below 0.89; the parent's
constant false ran over them. The height risers: the estimate's
foundation term (the conditioned basements), the same cause base.xml's
recorded delta runs. The battery fixtures build with a `bms_mode`
override in the census harness (the battery rule's own requirement);
the runs are otherwise the fixtures'.
### Row 94c, 2026-10-18, the SI config move's golden movement and the EV cold-snap delta (branch `row/r94c-b14`, worktree `r94c-b14`, commits `a776983b` + `b847aedf`)

The record of rows 94c, 94g and 94h that the schedule requires, the 94c
part: B-14's commit 2 (`b847aedf`, the SI config move) against its parent
`a776983b`. The probes' raw data lives beside this file in
`ledger/94c-probes/` so the record is self-contained: the ten goldens'
before and after dump documents (`old_<name>.json`, `new_<name>.json`,
the before side the parent's committed goldens, the after side the fix
commit's, byte-equal to them) and the two cold-snap full frames
(`coldsnap_frame_old.csv`, `coldsnap_frame_new.csv`, 4320 rows x 90
columns each). Load average at the captures: 1 to 2 (1 min).

Per-golden movement, compared field by field on bits (the dumps'
`git_head`/`git_dirty` provenance fields excluded; both sides captured on
a dirty tree, the before side at `ff39b24b` with commit 1 staged, the
after side at `a776983b` with the fix staged, and each side byte-equal to
the golden committed at its commit): four goldens bitwise outright and
seven with movement, every moved value in the last bits:

| Golden | Movement | Largest relative difference | The conversion that explains it |
|---|---|---|---|
| `leap_year_february_900s` | bitwise | none | no renamed field's value on this input passes an inexact pair |
| `month_boundary_billing_3600s` | bitwise | none | as above |
| `resstock_2025_fleet` | bitwise | none | as above |
| `weather_wrap_900s` | bitwise | none | as above |
| `der_pv_battery_900s` | the Clothes Washer's and the Other End Use's electric power, 4 cells each (the State's counts) | one ulp | the event load's kW working value: the schedule resolver's x1000 write and the load's /1000 construction read |
| `der_pv_battery_ev_900s` | the EV's power and end use (3 cells), the BMS actor's `load_kw` (2), the EV driver's `charge_kw` (3) and `soc` (120), the Total Electric Power column, and four metrics: renewable fraction 2 ulp, gross consumption 2 ulp, net energy 3 ulp, EV end use 1 ulp | 5.4e-16 (net energy 79.3022132717606 to ...56) | the EV fuel economy's J/m round trip (0.325 kWh/mi is not binary-exact in J/m), beside the x1000//1000 pair |
| `consumer_shape_900s` | the total electric (6695 cells), the total and water-heater gas power (5), the total reactive (10781), the Refrigerator #2 and refrigeration end-use power, one indoor and 1779 zone-2 temperature cells, and four metrics: the 30-min rolling peak 1 ulp, the renewable fraction 2 ulp, the gross 2 ulp, the net 2 ulp (the State's frame counts, the metrics' distances from the dumps) | 4.0e-16 (gross consumption 9154.879909050913 to ...916) | the x1000//1000 pair on the schedule resolver's write and the loads' read |
| `fleet_mixed_offset` | the aggregates' Cooking Range, Other, Refrigeration, Refrigerator #2, Zone 2 temperature, Total Electric and Total Reactive columns and the homes' energies; home 2's metrics: the refrigeration peak 1 ulp, the net 1 ulp, the refrigeration energy 4 ulp | 4.6e-16 (home 2's refrigeration energy 30.796765985629122 to ...136) | the same pair |
| `fleet_same_offset_300s` | the aggregates' Refrigeration, Refrigerator #2, Zone 2 temperature, Total Electric and Total Reactive columns and the homes' energies; home 0's renewable fraction 2 ulp, gross 2 ulp, net 1 ulp, the three homes' refrigeration peak 1 ulp and refrigeration energy 1 ulp each | 2.8e-16 (home 0's gross consumption 50.310569621864964 to ...98) | the same pair |
| `full_year_hourly` | nine frame columns (the total electric and reactive, the total and gas-furnace gas power, the Cooking Range, Dishwasher, Gas Furnace, HVAC heating end use and Other powers); metrics bitwise | one-ulp cells (the State: every difference last-bit) | the same pair |
| `full_year_900s_dst` | eight frame columns (the total electric and reactive, the ASHP Heater, Clothes Washer, Cooking Range, HVAC heating end use and Other powers, the indoor temperature); metrics bitwise | one-ulp cells (as above) | the same pair |

The resolved products of every golden compare bitwise (the guard's
expectation: no resolved equipment parameter moved). The committed goldens
were re-captured in `b847aedf` itself and compare-all is green at it (10
default + `full_year_900s_dst` on the dst build).

The 45-day EV cold-snap run (the Measurement section's EV dwelling: the
`bldg0000007` ResStock home with its EV under the Nightly strategy,
constant -7.0 C weather, 45 days at 900 s, warm-up disabled, seed 0; the
manifest in the worktree's `target/ev45/`), the full frame compared cell
by cell, per variable:

| Variable | Differing cells | Movement |
|---|---|---|
| `EV SOC (-)`, `actor:EvDriver:EV:soc` | 304 each | 245 cells at one f64 ulp and 59 at two ulps, the max absolute 2.2e-16 |
| `EV Electric Power (kW)`, `EV End Use Electric Power (kW)`, `EV Energy (kWh)`, `actor:EvDriver:EV:charge_kw` | 4 each | max relative 6.557e-15 (37 ulps at the first cell) |
| `Total Electric Power (kW)` | 4 | max relative 6.445e-15 |
| every other of the 90 columns | 0 | bitwise |

The SOC's differing cells run in five spans (rows 227 to 287, 323 to 379,
418 to 479, 2338 to 2399 and 4162 to 4223; the first at 2018-01-03
08:30), the drive and charge spans where the fuel economy's J/m round
trip moves the SOC's energy integral by its last bit; the power columns'
four cells are the charge-taper steps against the 0.90 target (rows 288,
480, 2400 and 4224, the actor's `charge_kw` column one row later each),
where one ulp of SOC is worth 37 ulps of power. The first differing rows
match `frame_golden`'s diff (226 to 288). No scaled difference above
1e-9 and no scale ratio near a unit factor anywhere: the movement is the
conversion pairs, and nothing else.

## The PV roof coverage's typed-config read, 2026-10-18

The PV roof coverage entry (OD-20, row 94i) at its parent `6a5d9769` and
at its commit, on the same worktree and wheel. The parent's triage (the
acceptance tests run at the parent, the probe read from the blueprint's
specs and the environment's coverage map): base-pv.xml's two arrays
attach to its omnidirectional roof (surface 2) with coverage
0.18678323692125798 registered; bldg0176227's 7000 W array attaches to
its tilt-0 garage roof (surface 9) with coverage 0.41527432163231953;
bldg0352714's 11000 W array attaches to its 180° roof (surface 12) with
coverage 0.7532717153914619. The attachment and the coverage the typed
read computes are on bits those values: the typed config's capacity and
attachment are the bag's serialized values, the geometry read picks the
same roofs, and no fixture over-subscribes.

| Golden | Movement | Cause |
|---|---|---|
| `consumer_shape_900s` | bitwise | as above |
| `fleet_same_offset_300s` | bitwise | as above |
| `fleet_mixed_offset` | bitwise | as above |
| `resstock_2025_fleet` | bitwise | as above |
| `der_pv_battery_900s` | bitwise | as above |
| `der_pv_battery_ev_900s` | bitwise | as above |
| every other fixture (4 default + dst) | bitwise | as above |

The collector-area constant's per-watt spelling moves the 4000 W
collector area by one f64 ulp from the old `cap / 420.0 * 2.0`
expression; the coverage division on the fixture roofs' areas absorbs it
(the quotients' bits are equal for 4000 W, 1500 W, 4200 W and 11000 W
against the parsed roof areas), which is why no fixture's roof solar
moves. No golden re-captured. compare-all: 10 default-feature fixtures
and the dst fixture, bitwise identical, run twice (at the row and at the
final tree).

## The unavailable periods applied, 2026-10-18

The unavailable-periods entry (B-15, row 94j) at its parent `6a5d9769`
(the schedule-side slice already carried there: the parser, the table,
the calendar flags, the column zeroing and the pre-period design
levels) and at its commit.

The parent's RED triage: the vacant home's exterior lighting drew 3.1 W
through the all-year vacancy (the profile-driven load the column
zeroing cannot reach); the bldg0000003 differential delivered heating
through its No Space Heating period; the check columns did not exist.
The commit's gates close them; the acceptance tests' details are in the
entry's State.

| Golden | Movement | Cause |
|---|---|---|
| `consumer_shape_900s` | bitwise | the home's only period window (2018-10-04, No Space Heating) flags steps the home does not call for heat on: the differential run over Oct 3 to Oct 5 delivers zero heating with and without the period (the heating setpoint 18.33 C, the zone above it throughout) |
| every other fixture (9 default + dst) | bitwise | no period in any home's window; the gates are all-default false without a period |

compare-all: 10 default-feature fixtures and the dst fixture, bitwise
identical, run at the row and again at the final tree. No golden
re-captured.
### M-6, 2026-10-18, rows 94g and 94h, the run-state and control-layer SI commits

The record the schedule requires of rows 94g and 94h: B-14's commit 3
(the run state in SI) against its parent `6a5d9769`, measured before the
goldens were re-captured. The parent side is the parent commit's
materialized frames (`frame_golden materialize-all` at `6a5d9769`, the
ten non-dst goldens plus the dst build's `full_year_900s_dst`); the after
side is this commit's fresh frames (`frame_golden run <manifest>
--output-dir`), the same manifests, `master_seed = 0`, warm-up disabled
as each manifest states. The comparison is per column, the guard's
metric: the scaled difference `max_t |a_t - b_t| / max_t |a_t|` and the
scale ratio `max_t |b_t| / max_t |a_t|` (NaN cells excluded; the `Unmet
HVAC Load (C)` column is NaN on both sides in these windows and carries
no comparison).

The largest scaled difference per golden (frame products; the
`month_boundary_billing_3600s` billing product's `total_import_kwh`
moved 4.759e-16 scaled, ratio 1.000000000):

| Golden | Largest scaled difference | The conversion that explains it |
|---|---|---|
| `der_pv_battery_900s` | 1.397e-7 (Total Gas Power and Gas Furnace Gas Power, therms/hour), ratio 0.999999860 | the therm: the parent's `GAS_THERMS_PER_HOUR_TO_W` = 29 307.107 017 222 2 W (NIST-exact 1055.05585262 J per Btu); the commit's therm is 100 000 × `uom`'s rounded `btu_it` = 105 505 600 J = 29 307.1111 W, 14.738 J per therm, 1.3969e-7 relative, the entry's own recorded movement |
| `der_pv_battery_ev_900s` | 6.324e-15 (the battery end use), ratio 1.000000000 | the x1000//1000 pair and the J/kWh pair, in the last bits; the home has no gas |
| `leap_year_february_900s` | 7.195e-16, ratio 1.000000000 | the same pairs |
| `weather_wrap_900s` | 9.811e-16, ratio 1.000000000 | the same pairs |
| `month_boundary_billing_3600s` | 1.397e-7 (the gas columns), ratio 0.999999860 | the therm, as above |
| `full_year_hourly` | 1.397e-7 (the gas columns), ratio 0.999999860 | the therm, as above |

Every other moved column of every measured golden moved at or below
1e-14 scaled with ratio 1.000000000, the last-bit dust of the conversion
pairs. The pre-fix measurement caught the BMS and EV-driver actor
columns moving at exactly 1000x (scale ratio 0.001000000: the actors' kW
working values written into kW-declared slots the row plan converts
again): fixed in this commit by converting each actor's declared-kW
telemetry to W once at the row write (`build_actor_column_map` carries
the declared unit), after which the same columns measure 2.8e-16 to
4.8e-16 scaled with ratio 1.000000000.

The committed goldens were re-captured twice in this commit, each pair
byte-identical (`diff -r` clean over the whole `tests/fixtures/golden/`
tree, twice: the ten non-dst goldens on the default build and
`full_year_900s_dst` on the dst build), and `frame_golden compare-all`
is green at the commit in both feature configurations (the 10 default +
the dst build's own).

`si_census` at this commit: 4616 lines (the parent's 8857), the file at
`target/si-census.txt`, the four witnesses green. Load average at the
captures: 1 to 2 (1 min).

### M-6b, 2026-10-19, row 94h, the control-layer SI commit

The record the schedule requires of row 94h: B-14's commit 4 (the control
layer in SI) against its parent `51152c52` (the landed commit 3), the
after side the fix's working tree, measured before the goldens were
re-captured. The before side is commit 3's committed goldens (captured at
row 94g); the after side is the fresh frames of the ten non-dst goldens
plus the dst build's `full_year_900s_dst`, `master_seed = 0`, warm-up as
each manifest states. The comparison is per column, the guard's metric.

Ten of the eleven goldens compare bitwise outright:
`consumer_shape_900s`, `der_pv_battery_900s`,
`full_year_900s_dst`, `full_year_hourly`, `leap_year_february_900s`,
`fleet_mixed_offset`, `fleet_same_offset_300s`,
`month_boundary_billing_3600s`, `resstock_2025_fleet`,
`weather_wrap_900s`. One moves, in its last bits:

| Golden | Movement |
|---|---|
| `der_pv_battery_ev_900s` | `Total Electric Power (kW)`, `Electric Vehicle Electric Power (kW)`, `EV End Use Electric Power (kW)`, `actor:EvDriver:Electric Vehicle:charge_kw` 1 cell each, max relative 2.3e-14; `actor:EvDriver:Electric Vehicle:soc` 39 cells, max relative 1.3e-16; the metrics' `total_energy_kwh.net_energy_kwh` 1 ulp (79.30221327176058 to ...59); every other column bitwise |

The conversion that explains it: the EV driver's fuel economy now enters
once as `fuel_economy_j_per_m` (the `uom` conversion of the catalogue's
0.3 kWh per mile, 671.0808876163206 J/m, not binary-exact) and the
driver's trip arithmetic runs in metres and joules, where the parent ran
in miles and kWh with a conversion at each use; the pairs' last bits
differ and propagate from the first driven step (row 38's SOC last bit)
to the charge rate (row 76). No scaled difference above 1e-9 and no
scale ratio near a unit factor anywhere: the movement is the conversion
pairs, and nothing else. (The same dwelling's identical movement at row
94c, recorded above, was the config-side J/m conversion; this one is the
driver's parameter.)

The 45-day EV cold-snap run (the Measurement section's EV dwelling: the
`bldg0000007` ResStock home with its injected 75 kWh / 11.5 kW EV under
the Nightly strategy, constant -7.0 C weather, 45 days at 900 s, warm-up
disabled, seed 0; the manifest in this worktree's `target/ev45/`, the
before side run at `51152c52`, the after side on the fix's tree), the
full frame compared cell by cell, per variable:

| Variable | Differing cells | Movement |
|---|---|---|
| `EV SOC (-)`, `actor:EvDriver:EV:soc` | 364 each | 1 to 2 ulps, the max absolute 2.2204e-16 |
| `EV Electric Power (kW)`, `EV End Use Electric Power (kW)`, `EV Energy (kWh)`, `actor:EvDriver:EV:charge_kw`, `Total Electric Power (kW)` | 6 each | max relative 6.4e-15, first at rows 287 to 288 (a charge-taper step) |
| every other of the 90 columns | 0 | bitwise |

The SOC's differing cells are the drive and charge spans where the fuel
economy's J/m conversion pair moves the SOC integral's last bit; the
power cells are the taper steps one ulp of SOC is worth many ulps of
power at. The regression's own gate is green at the fix (the run charges
on most of the 45 days, and no trip is cancelled). No scaled difference
above 1e-9 and no scale ratio near a unit factor anywhere: the movement
is the conversion pairs, and nothing else.

`si_census` at this commit: 2987 lines (the parent's 4616), the file at
`target/si-census.txt`. The witnesses: the constants' file
(`telemetry_keys.rs`'s `ELECTRIC_KW`), `python/hares/**`,
`notebooks/simulation_explorer.py` (the UI's display-unit knobs and the
OCHRE-boundary dict keys), and `benches/output_row_plan.rs` (the
row-plan bench's declared reporting column) - the bench witness moved
from `rl_episode.rs`, whose observation keys the run-state commit
renamed, to the reporting column that stays. Load average at the
captures: 1 to 2 (1 min).

### B1, 2026-10-20, consumer `48547ac1` on branch `row/cn4-b1`, engine `cb7f7ba11`

The one consumer re-baseline (row 97), captured per the B0 protocol: the same
production set, the same fit seeds and grids, every float `float.hex`. Files in
`baselines/`: `B1-de-trace.json` (sha256
`5439d2fa834c0d9154ecba7dd6dd92d71411defbd7fca8b07ce239f6c925df51`),
`B1-losses.json` (sha256
`290317750955559665223d733436a98ae9c33b83c3ae3f563b05a84134cf0053`),
`B1-record.json`. Engine build sha256 `8298bbf2bab65a9e` (the pinned submodule
`cb7f7ba11`, built editable into the consumer venv: `uv sync --locked --all-extras
--all-groups` then `maturin develop --release` from the submodule's Python-binding
crate; the binding's `build_info()` reports commit `cb7f7ba11`, dirty False, profile
release, and the wheel's `Generator:` line maturin 1.15.0, so the stale-extension
failure mode is excluded by the same check that records the build). `uv.lock` sha256
`1146b777fb9a5b52bc720f3a9aa51303c68b4b3638d22fe17ccb9a41e05e161e` (unchanged by the
pin move: the engine is a directory source in the lock). Consumer commit `48547ac1`
(the tree every capture ran on; the row's own commits follow it on branch
`row/cn4-b1` and touch no number the fit path reads).

- DE-trace capture: 4 premises, 180.8 s wall (user 3282.9 s, sys 20.7 s across the
  fit's processes), peak RSS 6.2 GB, 28 cores, the host otherwise idle (the
  exclusive-host rule). Captured twice; the two files are byte-identical (sha256
  above, both) and `compare` exits 0 across them: the production set is
  deterministic across processes.
- The 13 losses of `tools/check_losses.py`'s fit, as `float.hex` in `B1-losses.json`
  (keys `hvac:truth_bldg`): 13 of 13 moved against the B0 pins; every movement is
  attributed in the table below. `check_losses.py` re-pinned from `B1-losses.json`
  and re-run on the pinned engine: all 13 IDENTICAL, exit 0 (a second full 13-premise
  fit, reproducing every loss bitwise).
- Gate at the consumer commit: `383 passed, 0 failed, 7 skipped, 390 total`; the
  failing set is empty; the skip set is the seven environmental base skips. ruff
  exit 0. `hares_contract_check.py` prints `[hares-contract] PASS` (two
  informational NEW items: the catalogue's `Other` and `Refrigeration` end-use
  aggregates; considered, the adapter reads neither as an aggregate, the same two
  items row 96's gate recorded).

**The consumer follows the SI tail in this row's commit.** The first gate run
against the corrected engine failed 8 cases the drift check's own scan cannot see:
the run-state step dict key `net_electric_power_kw` is now `net_electric_power_w`
(W), the battery BMS override key `surplus_deadband_kw` is now `surplus_deadband_w`,
the telemetry field `reactive_power_kvar` is now `reactive_power_var` (var), and the
grid voltage override is validated to exactly 0.0 or [0.5, 1.5] pu. The consumer's
follows, all in this commit: the three step-dict readers
(`ami2/dr_actuation.py`, `tests/suite_ami2.py`'s two runners, `cosim.py`'s two) read
the W field and divide by 1000 to keep every kW-unit band and record in the cases
intact; the two `_q` seams convert var to kvar; the battery spec writes the W key
(its value 0.0 is unit-free); `coupling.py`'s fixed-point probe clamps the iteration's
voltage trials into the engine's validated band (a constant-power iterate can
overshoot to a collapsed-looking guess on the way in; the dwelling's answer outside
the band is not defined, the converged fixed point is in band, and `advance()` still
fails loudly on an out-of-band solved voltage); the `PowerLimit` docstring names
`max_power_w`. With those, the gate reads 383/0/7.

**The capture exposed an engine defect, fixed before the pin advanced.** The first
two captures, on the tail head `bdae4fec`, ran 689.1 s and 697.4 s wall (user 3906.9
and 3953.3 s, sys 2174.0 s, max RSS 6.2 and 6.5 GB) and wrote 4.2 GB of stderr:
`crates/hares-equipment/src/hvac/air_conditioner.rs` carried a leftover `eprintln!`
("DBG perf: steady=... plr=...") in the coil step's hot path, landed with the row 94c
SI commit `fbb48172`, firing once per cooling-coil step (about 23 million locked
unbuffered writes per capture; the sys time is those writes). Payload-neutral by
construction and proven by capture: the pre-fix pair is byte-identical within itself
and `compare` exits 0 against the committed pair (only the header's engine commit and
build sha differ). The print is deleted on the engine branch (`cb7f7ba11`, the full
gate block green: fmt, clippy `-D warnings`, nextest `--workspace --all-features
-j 2` 8274/8274 (8 skipped), `pytest -m ''` 1800 passed + 6 xfailed after `maturin
develop`, pyright 0, the goldens compare-all 10 + dst 1 bitwise) and pushed; the
fixed engine's captures are the ones recorded above.

**B0 to B1, per loss (the 13).** The attribution is a record, not an equation: the
DE trace is data-dependent, so the B0 to B1 difference is not a sum of the entries'
deltas. For each loss, the causes are the engine rows between the consumer's old pin
(`92b1b80a`) and the new one, each with its own recorded delta in this log; the
premise-specific rows are named first.

| loss | B0 (hex) | B1 (hex) | delta |
|---|---|---|---|
| ashp:0043535 | `0x1.2c0be26d9f926p-1` | `0x1.26d44d1ec6a07p-1` | -1.74% |
| ashp:0143031 | `0x1.5d5d8e4f6ed52p-1` | `0x1.978f0fa95183dp-1` | +16.66% |
| ashp:0353695 | `0x1.0e4a741970677p-1` | `0x1.14992bee9b57dp-1` | +2.33% |
| ashp:0476433 | `0x1.386414f2a1fedp-1` | `0x1.3924f88becc57p-1` | +0.24% |
| elecfurn:0046363 | `0x1.272f7c4dfbc3dp-1` | `0x1.799aee853d82ap-1` | +27.92% |
| elecfurn:0116457 | `0x1.9e85309dee694p-1` | `0x1.4550af6e92093p-1` | -21.52% |
| elecfurn:0239995 | `0x1.d8976e12f4849p-1` | `0x1.7a3984eb7122fp-1` | -19.97% |
| elecfurn:0515804 | `0x1.9e709b84a46fap-1` | `0x1.84ef2a287a62ep-1` | -6.15% |
| gasfurn:0012134 | `0x1.1c23d5e7409c7p-1` | `0x1.2eecfac516508p-1` | +6.61% |
| gasfurn:0059183 | `0x1.3167228ee573fp-1` | `0x1.1ce2212e9165ep-1` | -6.72% |
| gasfurn:0105529 | `0x1.9e9c1ae10db33p-1` | `0x1.3e4472f72aab6p-1` | -23.24% |
| gasfurn:0141500 | `0x1.dcac610365380p-1` | `0x1.aaedc8610c395p-1` | -10.44% |
| gasfurn:0188636 | `0x1.2e10e98c7b54ep-1` | `0x1.3636d97113effp-1` | +2.70% |

The causes, with their own records:
- every premise: the same-zone precomputed boundary's zone-facing half-layer fix
  (`011e9bcb`; every conditioned dwelling's auto-generated partition wall resolves
  the multi-layer "Interior Wall" row, the partition-wall pin 0.5916... to
  0.5177..., all ten goldens moved); the warm-up stochastic-neutrality fix
  (`2ed4e816`; the post-warm-up decision state restores every stochastic stream, the
  event patterns move); the SI unit sweep (`5958ef72`, `fbb48172`, `e40b46e1`,
  `c37b526a`; the config, state, telemetry and actor surfaces hold W, J and Pa, the
  M-6/M-6b records' 1 to 2 ulp per-step movements propagate through the coupled
  steps); the infiltration defaults row (`a5ca5bce`, `1caa0ff3`; the default
  infiltration height, flue and shielding follow the OpenStudio-HPXML rules); the
  faithful Fahrenheit/Celsius conversion formulas (`086092c6`, `6b0be6b2`).
- the four ASHP premises only: the defrost consolidation (`04651a24`; the double
  application, the duty and the resistive destination, the OCHRE trace heating
  31.3747 to 28.9580 kWh).
- none of the 13 premises: the boiler flue split (`7b4aeaff`; no premise heats with
  a boiler, and the gas storage water heater's own split is untouched by that row),
  the EV island clamp (`414fc2f1`) and the tariff coverage rows (`1f61e4d4`,
  `44deba5c`; no premise declares a vehicle), the roof shading row (`81f6de5d`; no
  PV), the dehumidifier row (`e1661683`; none), the unavailable-periods row
  (`ebccdc9a`; the premises' schedule CSVs declare no Vacancy, Power Outage or No
  Space Heating/Cooling periods), the schedule calendar anchoring (`4e0945f2`; the
  fit's Jan 1 non-leap starts are the bitwise-unchanged case), the grid voltage
  guard (`17a2170e`) and the rollback accounting (`871453e0`; nominal-run neutral by
  their goldens), and the constant-setpoint and generated-schedule rows
  (`384a1c4c`, `46647af2`; not in the fit path).

The fits' own health: zero rejected evaluations on every premise (the strict health
policy refused nothing, as row 96's gate found on the production set), improvements
0.28 to 0.82, `n_eval` 459, 408, 510, 510 on the production set and 459/408/510 on
the 13-set's archetypes.

**B0 to B1, walls.** The recorded B0-era baselines and this record's, all on this
28-core host, the production set (four premises, 365 days, 16 generations):

| when | record | capture wall | CPU |
|---|---|---|---|
| B0, 2026-09-30 | M-0 | 174 s | 69 m 42 s user, peak RSS 6.2 GB |
| pool sizing before, 2026-10-12 | row 73's record | 185.8 s, 203.3 s (load 11.4, 23.5 at start) | about 69.4 min total |
| pool sizing after, 2026-10-12 | row 73's record | 225.6 s, 255.3 s, 241.3 s (load 82.9, 6.3, 47.3) | about 66 min total |
| B1 with the print, 2026-10-19 | the defect record above | 689.1 s, 697.4 s | user 3906.9/3953.3 s, sys 2174.0 s |
| B1, 2026-10-20 | this record | 180.8 s, 179.9 s | user 3282.9/3229.2 s, sys 20.7 s, peak RSS 6.2 GB |

Direction: the finished engine is FASTER per home than every recorded B0-era
baseline, and the engine's physics is lighter. Per engine run the fit's user CPU
moved 2.20 to 1.74 CPU-s (B0: 69 m 42 s user over the recorded 1895 runs; B1: 3282.9
s over 1891), 21 percent lower: the rows landed since the pool-sizing record removed
work the per-run cost used to carry (row 75's metrics-and-metadata request, row 77's
process-pool pickling of every evaluation's results), and the numeric tail and the
hardening wave added no per-step compute cost on top. Per evaluation the capture
runs 0.317 to 0.341 s per premise (the per-premise fit walls 148.1, 139.1, 161.9 and
171.9 s, four premises concurrent) against the pool-sizing era's 0.49 to 0.55 s per
evaluation. The per-capture wall 180.8 s sits under the pool-sizing records (225.6
to 255.3 s, measured on a host whose load average ranged 6.3 to 82.9 at the starts)
with the host idle here, so part of the wall difference is host context; the
per-run CPU comparison is the host-independent one and it favors the finished engine
on its own. The warm-up default (B-3, already in the old pin) is inside both sides'
per-run figures and is not a B0 to B1 delta.

Per-row savings where a row claimed one: row 76's parent-probe removal is visible in
B1's counts (`n_eval` 459, 408, 510, 510 against B0's recorded 461, 410, 512, 512,
the declared -2 per premise); row 75's and row 77's removals are inside the 2.20 to
1.74 CPU-s per run; row 73's sizing kept its recorded CPU saving (about 3 minutes)
at its recorded wall cost on the busy host; row 77's threads kept their memory
target, and this record is the first wall measurement of the thread shape at the
production set.

## The measurement ladder (the consumer as the instrument, per the operator's mandate)

Every fold re-syncs the consumer and appends a row: the capture wall, the per-run
CPU cost where recorded, the 13 losses' verdict, the suite counts. The goldens prove
the engine did not change what it computes; the ladder proves the end-to-end workload
holds. A capture-wall regression over 5 percent without an attributed, accepted trade
halts the pipeline until investigated.

| date | engine | the capture wall | the 13 losses | the suite | note |
| ---- | ------ | ---------------- | ------------- | --------- | ---- |
| 2026-10-19 | `cb7f7ba11` (B1, row 97) | 179.9 / 180.8 s | re-pinned, 13/13 | 383/0/7 | the baseline; 1.74 CPU-s/run (was 2.20) |
| 2026-10-20 | `ea46b1f2` (row 112) | 181.7 s | IDENTICAL 13/13 | 383/0/7 | +0.5-1 percent vs the baseline: the docs + the macro-forwarding delta, within noise; the compare vs B1 bitwise identical; the build_info stale-cache catch recorded in row 112's State |
| 2026-10-20 | `e3bb5257` (rows 113/114, the package A fold) | 186.8 s | skipped (the heavy pass at the boundary) | 383/0/7 | flat within noise expected: the compare vs B1 bitwise identical; the wall +2.8 percent vs row 112, inside the 5 percent band; the build_info reports `e3bb5257`, dirty False, release |
| 2026-10-20 | `da1767f1c` (row 115, the package B fold) | 179.1 s | skipped (the heavy pass at the boundary) | 383/0/7 | flat expected: the envelope ceilings, the floor counting and the solar/init guards, physics untouched; the compare vs B1 bitwise identical; the wall under every prior row (0.4 to 0.9 percent under the B1 baseline, 4.1 percent under row 113's), the host quiet (load 0.82 at the start); the build_info reports `da1767f1c`, dirty False, release |
| 2026-10-20 | `b46c9c83` (rows 116/117, the package C fold) | 181.3 s | skipped (the heavy pass at the boundary) | 383/0/7 | flat expected: the RNG stream hygiene and the EV driver's failure propagation, and the shipped fixtures carry no schedule source configs and run Constant-source drivers, so the goldens were bitwise at the tip; the compare vs B1 bitwise identical; the wall +1.2 percent vs row 115, inside the 5 percent band, the host quiet (load 0.97 at the start); NOTE: the noise series for the UNSPECIFIED-seed sources changed key derivation (the dwelling identity now keys it), irrelevant to the shipped fixtures, which declare no such sources; the build_info reports `b46c9c83`, dirty False, release |
| 2026-10-21 | `b17d267b` (rows 118/119, the package D fold) | 177.3 s | skipped (the heavy pass at the boundary) | 383/0/7 | flat expected: the fleet seed/DR latch/event-rate normalization and the setpoint plausibility band, and the shipped fixtures carry no schedule setpoint columns while the moved test CSVs are test-only; the setpoint band validated at the FSM boundary, the shipped ResStock profiles 49-84 F in-band; the compare vs B1 bitwise identical; the wall under every prior row (1.4 percent under the B1 baseline, 2.2 percent under row 117's), the host quiet (load 0.80 at the start); the build_info reports `b17d267b`, dirty False, release |
| 2026-10-21 | `c668dc25` (rows 120/121, the package E fold) | 176.4 s | skipped (the heavy pass at the boundary) | 383/0/7 | flat expected: the clamp census and the constant/dead-code sweep package, the census diagnostics-scoped and proven pure with observe on/off, the sweep dead expected; the compare vs B1 bitwise identical; the wall under every prior row (1.9 percent under the B1 baseline, 0.5 percent under row 119's), the host quiet (load 1.58 at the start); the build_info reports `c668dc25`, dirty False, release |
