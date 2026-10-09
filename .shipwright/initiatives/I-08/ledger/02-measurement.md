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
  `Pattern("*")` run on the `fleet_same_offset_300s` inputs, and the empty
  golden delta.
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
