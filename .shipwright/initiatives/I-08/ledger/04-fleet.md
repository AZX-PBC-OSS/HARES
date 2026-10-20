# Fleet memory and status

Implements `plan.md` §2.4 and the fleet half of §2.6. Fixes V16, V17 and
V41, and adds the fleet determinism test that V30 shows is missing (the
single-dwelling test is rewritten by RT-5 in `06-runtime-memory.md`). The
ResStock year and weather-path defects (V42) are B-9 in
`09-numeric-fixes.md`, after B-9a there makes `Fleet::from_resstock` read
the extracted building layout (`buildings_dir`) and each building's
weather station from its HPXML; the fleet label fix B-12 there uses FL-4's
mixed-offset inputs.

Facts every entry below relies on, at `ebdb1a17`:

- `Fleet::simulate(n_threads)` (`crates/hares-fleet/src/fleet.rs:313-324`)
  builds a local rayon pool of `n_threads` threads; `n_threads == 0` uses the
  global pool. `RAYON_NUM_THREADS` is read only when the global pool is first
  built, so it cannot vary the thread count inside one test process.
- `simulate_parallel` runs `par_iter().map(..).collect()` over the fleet's
  entries (`fleet.rs:345-409`). `collect` keeps entry order, so the returned
  `Vec` is in entry order whatever the completion order.
- Each `DwellingOutcome` holds a full `SimulationResults` (`fleet.rs:43-48`),
  and `aggregate(&[DwellingOutcome], ..)` (`aggregation.rs:248`) runs only
  after every home has finished. ResStock fleets set `retain_batches: true`
  (`fleet.rs:797`), so every home's full frame is alive at once.
- The fleet status type is `SimStatus { Ok, Flagged(String), Failed(String) }`
  (`fleet.rs:33-40`). There is no `HomeStatus` yet.
- Aggregation has two stages, each keyed on the column's `unit` metadata
  string. Over time, per home, `ColumnAggregation`
  (`crates/hares-fleet/src/aggregation.rs:43-50`): `kWh` sums; `kW`, `C`,
  `°C`, `-`, `W`, `therms/hour`, `kVAR`, `kWh/mi`, `s`, `enum`, `W/m2` and
  `deg` average; any other unit averages (`:63-72`). Across homes,
  `FleetAggregation` (`:119-126`): `kWh`, `kW`, `therms/hour`, `kVAR` and
  `W` take the weighted sum Σ wᵢ·vᵢ; `C`, `°C`, `-`, `W/m2`, `s`, `deg`,
  `kWh/mi` and `enum` take the weighted mean Σ wᵢ·vᵢ / Σ wᵢ; any other unit
  takes the weighted sum (`:141-150`). A fleet `kW` cell is the population
  total.
- The units on the "any other unit" path (census
  `git grep -ohE 'unit: "[^"]*"' -- 'crates/*.rs'`) are `fraction`,
  `ratio`, `%`, `bool`, `code`, `index`, `count`, `min`, `V`, `A`, `Ah`,
  `kg/s`, `L/day`, `J/(kg*K)`, `bytes` and the empty unit. Each averages
  over time and sums across homes, so a fleet ratio, binary state, mode code
  or voltage is a weighted sum today.
- Per home, the time accumulator adds step values in row order into one
  `f64` and divides once by the count (`Accumulator`, `aggregation.rs:208-243`);
  one null step nulls the home's period (`Accumulator::finish`, `:235-238`).
  Across homes, the fold visits homes in entry order for each bucket
  (`:646-675`), computing `v * w` per home; one null or missing home cell
  nulls the fleet cell (`has_null`, `:644, 663, 673, 693`). Rows are the
  intersection of every home's buckets (`:588-597`). Each home buckets its
  own local wall clock (`bucket_start`, `:448-466`) and the bucket is keyed
  by its epoch second (`:508`); rows are labelled in UTC with `Z`
  (`:636-637`).
- Until `09-numeric-fixes.md` B-6 the recorder writes 0.0 in every cell it
  does not fill (`crates/hares-core/src/dwelling/mod.rs:7166`), so no
  per-step value is null and neither null rule above changes a number.
- `AggregationResolution::{FifteenMin, Hourly}` (`aggregation.rs:15-18`) is
  the only frequency choice. Python's `Fleet.simulate` defaults to
  `FifteenMin` (`crates/hares-python/src/py_fleet.rs:180-182`), and ResStock
  fleets run at 60 s (`fleet.rs:787`), so their default output is a
  15-minute reduction of 1-minute steps.
- A fleet home can write its own files: its `SimulationConfig` carries its
  output settings (`fleet.rs:1147-1150` builds such a home).

## FL-1: streaming ordered fold with a deterministic memory bound

**State:** committed `9eb66255` + `b57be63c` (branch `row/r50-fl1`,
2026-10-11). The first commit captures `fleet_same_offset_300s` on the
parent code, twice byte-identical. The second commit replaces
`Fleet::simulate(n_threads)` + `aggregate()` with
`Fleet::simulate(&FleetReport, n_threads, chunk_size) -> FleetResult`:
each home runs its own reports plus the fleet's appended `fleet` report
(the selection restricted to the home's catalogue, `Timestep`, a
`Callback` destination) whose batches feed a per-home `PeriodReducer`
and drop, so the home's result in the fold is its reduced period table,
never its frame; homes run in consecutive chunks of the entry order and
each finished chunk folds into the running aggregate one home at a time
in entry order before the next chunk starts. The bound is a
max-resident-homes bound, the chunk size: `FleetResult.
peak_live_home_results` reads it (the acceptance test pins exactly 1
and 2 at chunk sizes 1 and 2), and the bench pair at 10000 homes
measures it (104 MB at chunk 64 against 158 MB at the one-chunk
collect shape, wall equal at 12.5 s vs 12.4 s; corrected on review: the
first record's pair was an artifact of one shared criterion process,
and the fresh-process remeasurement in `02-measurement.md` has the pair
agree, 85.1 MB against 85.3 MB, wall 13.20 s vs 12.84 s).
The ordered-merge correctness: the fold's per-cell accumulation adds
`v * w` and `w` home by home in entry order and divides a weighted mean
once after the last home, the parent fold's operations and order; the
`fleet_same_offset_300s` `aggregate` (36 columns, 192 rows) and the
`metrics` products of both fleet goldens compare bitwise with the
parent code's capture, the chunk-1/2/64 by thread-1/2/8 differential
and the 12-home six-barrier-crossing differential agree on bits, and
the unit-level fold tests fail on a mutated accumulation order
(verified by mutation). The fold empties on a variable mismatch
between homes, as the parent did (until FL-2): `fleet_mixed_offset`'s
`aggregate` and `covered_weight` stay zero rows. The rules stay
unit-keyed in `unit_keyed_fleet_rules(Unit) -> (TimeRule, AcrossHomes)`
(FL-5 deletes it); the null rule is the entry's covered-weight rule (no
step value is null before B-6, so no number moves). The per-home
Diagnostics/reports integration rides rows 45 and 46's surfaces: the
fold keys the reduced periods by the instant of the period start (the
`Time` column's naive local milliseconds minus the home's
`hares:utc_offset_s`) and the homes table's metrics columns exist only
when the home's config requested them. `FleetBuildReport` is the
construction report's new name (`FleetReport` is the report request's).
Python: `FleetReport(selection, frequency, health_policy=HealthPolicy())`,
`Fleet.simulate(report, *, n_threads=None, chunk_size=64,
raise_on_failure=False, progress=None) -> FleetResult`, `Coverage`,
the `Frequency` constructors (`minutes`, `hourly`, `daily`, `monthly`,
`annual`, `run_period`); removed `AggregationResolution`, the
`resolution=` keyword and `FleetResults` with `per_dwelling_metrics`,
`aggregate_timeseries` and `failures` (a failed home is a `homes` row
with status `failed` and its reason); the stub and the docs' changes
section follow. The goldens: both fleet manifests' `resolution` key
replaced by the required `[fleet]` table (`fleet_same_offset_300s`:
`frequency = { minutes = 15 }`; `fleet_mixed_offset`: `frequency =
"hourly"`; both `selection = "verbosity-2"`); `resstock_2025_fleet`
re-captured, its `metrics` product to zero rows and its `weights`
stand-in replaced by the zero-row `covered_weight` (the reason in
`02-measurement.md`). Full block green on the commit (fmt; clippy
--workspace --all-targets --all-features -D warnings; nextest
--workspace --all-features 7901 passed; -F dst 7902 passed; pytest
1669 passed, 6 xfailed, 0 skipped; pyright 0; compare-all 8 manifests
identical, `full_year_900s_dst` identical); the wall-time rule held at
the parent and after (`02-measurement.md`). Review + red team owed.

**Kind:** initiative work (`context.md` item 13). Structural: every fleet
number equals its parent commit's bitwise. The numeric halves are FL-5
(declared kinds) and `09-numeric-fixes.md` B-11 (Mode).

**Problem:** fleet memory grows with homes × steps × columns because every
home's full frame is collected before aggregation (facts above). Both
aggregation stages infer a column's rule from its unit string
(`ColumnAggregation::for_column`, `aggregation.rs:312`;
`FleetAggregation::for_column`, `:546`), against `plan.md` §2.6, and no
document states the cross-home rule. The only frequencies are 15 minutes
and one hour. No test checks that a fleet result is identical across
thread counts: the only determinism check
(`tests/regression/determinism.rs:17-80`, V30) runs single dwellings
through `engine.run`, varies `RAYON_NUM_THREADS` (which has no effect
there) and compares sorted metric totals with an epsilon.

**Change:**

*Fleet report and per-home runs.*
- `Fleet::simulate(&self, report: &FleetReport, n_threads: usize,
  chunk_size: NonZeroUsize) -> Result<FleetResult, FleetError>` replaces
  `simulate(n_threads)` and the separate `aggregate(&[DwellingOutcome],
  AggregationResolution)`. `FleetReport` is `plan.md` §2.4's: a `Selection`,
  a `Frequency` and the fleet's `HealthPolicy`, which is applied to every
  home's run (a violation fails that home). `Frequency::Minutes(n)`
  (`03-output-reporting.md` OR-13) carries sub-hourly fleet output: today's
  `FifteenMin` is `Frequency::Minutes(15)` and today's `Hourly` is
  `Frequency::Hourly`. `AggregationResolution`, `DwellingOutcome`,
  `DwellingMetrics`, `FleetResults`, `aggregate()`, `resample_rows`,
  `build_aggregate_batch` and both unit tables are deleted.
- Each home runs the reports of its own `SimulationConfig` (a home can still
  write its own files; a home whose own config carries a `Destination::Callback`
  report fails that home at run start: through `Fleet::simulate` the dwellings
  are built internally, so nothing can attach the report's callback, and the run
  refuses a callback report with nothing attached instead of streaming its
  batches to nowhere, pinned) plus one report the fleet appends, named `fleet`: the
  fleet selection restricted to that home's catalogue (below), at
  `Timestep`, to a `Callback` destination (`03-output-reporting.md` OR-14).
  The fleet attaches a Rust `BatchCallback` that feeds each batch into a
  period reducer the fleet owns and then drops the batch, so a home's result
  in the fold is its reduced period table, never its frame. A home whose
  config already has a report named `fleet` is a construction error,
  `FleetError::ReservedReportName { bldg_id }`.
- The per-home reducer is OR-13's `PeriodReducer` (`plan.md` §2.6):
  `PeriodReducer::new(frequency, calendar, rules)` with the fleet's
  frequency, a `PeriodCalendar` (the home's own local calendar until
  FL-4) and one `TimeRule` per variable (the rule below); the attached
  callback feeds each batch to `push_batch`, and the fold closes each
  home's reducer with `finish` to emit its last partial period. Per-home
  metrics are computed only when the home's `SimulationConfig` requests
  them (`plan.md` §2.2).

*Selection across homes.*
- A home's `fleet` report selects the fleet selection restricted to that
  home's catalogue: `Named` keeps the names the home declares, `Pattern`
  matches within the home's catalogue, and `Verbosity(n)` is the home's own
  tier. A variable a home does not declare is not applicable to it (a home
  without a battery has no battery variables): the home does not fail and is
  not part of that variable's covered weight. This is the one fleet
  exception to `plan.md` §2.2's rule that an unknown `Named` name or an empty
  `Pattern` is an error.
- A home whose restricted selection holds no variable (a `Named` list of
  battery variables over a home without a battery) runs its own reports,
  gets no `fleet` report, folds no value and enters no cell's covered
  weight; its status is decided as for any other home (`ok` when its run is
  clean), its `periods_folded` is 0, and the aggregate's rows are the
  periods every home that folded at least one value has a row for.
- A `Named` name that no home declares, or a `Pattern` that matches nothing
  in any home, is `FleetError::UnmatchedSelection { names }`, raised when
  the last home has been built and before its chunk runs. A catalogue exists
  only once a home's inputs are parsed, so the check cannot run at fleet
  construction without parsing every home twice or holding every blueprint,
  which the memory bound forbids.

*Chunks and fold order.*
- Homes run in consecutive chunks of the fleet's entry order (`chunk_size`,
  default 64 in Python). A chunk runs in parallel on the fleet's pool; when
  it has finished, its homes are folded into the running `FleetResult` one
  at a time in entry order, and their reduced tables are dropped before the
  next chunk starts. The global fold order is therefore the entry order, the
  same sequence for every chunk size and thread count.
- `FleetResult.peak_live_home_results` is the largest number of home
  results held at once; it is bounded by the chunk size by construction.
  The frame memory is bounded by the chunk size times one home's reduced
  table. At `Timestep` frequency that table is the home's selected-variable
  frame, so "fleets never retain full frames" means never more than one
  chunk's frames at once. The rest of the fleet's memory grows with the
  number of homes and is bookkeeping, not frames: the entry list (one
  `DwellingConfig` and weight per home), the `homes` table (one row per
  home, plus its metrics columns when requested), and the set of file
  destinations checked for duplicates (one path per file destination).
  FL-1's Measurement records its bytes per home.

*The two-stage rule.* The fleet reduces each variable twice, each stage with
its own rule:

| Kind | Over time, per home | Across homes |
|---|---|---|
| `Energy` | `Sum` | weighted sum Σ wᵢ·vᵢ |
| `Power` | `Mean` | weighted sum |
| `Temperature`, `Ratio` | `Mean` | weighted mean Σ wᵢ·vᵢ / Σ wᵢ |
| `State` | `Mean` (duty fraction) | weighted mean |
| `Enum` | `Mode` | `Mode`: the code with the greatest summed weight, ties to the lowest code |
| `Quantity` | as `VariableDef.aggregation` declares (`Sum` or `Mean`) | as the variable declares (`VariableKind::Quantity { across_homes }`, `Sum` or `Mean`) |

The time stage is §2.6's per-dwelling rule. Power sums across homes because
a fleet's power is the population's demand, the same population its energy
totals, so fleet energy over a period stays equal to fleet mean power times
the period. Irradiance (`W/m2`), durations, efficiencies and other intensive
quantities declare `Mean`; flows, counts and volumes declare `Sum`.

At this commit both stages keep the rules of the parent commit, keyed on
`Unit::symbol()` (the string the `unit` metadata carries since
`03-output-reporting.md` OR-1): over time, `kWh` sums and every other unit
averages; across homes, the tables at `aggregation.rs:119-126` with their
weighted-sum fallback. They live in one function,
`unit_keyed_fleet_rules(Unit) -> (TimeRule, AcrossHomes)`, which FL-5 deletes.
The variables whose current rule differs from the table above change in
FL-5 and B-11, not here: every `Ratio` (`fraction`, `ratio`, `%`) and
`State` (`bool`) variable and every `Enum` variable on `code` or `index`
sums across homes today; intensive `Quantity` variables (`V`, `J/(kg*K)`,
`min`, `bytes`) sum across homes today; a `Quantity` declared `Sum` over
time averages today; `Enum` averages in both stages today.

*Accumulation order.* Over time, step values are added in step order into
one `f64` per period and variable; `Sum` returns it and `Mean` divides it
once by the count of valid steps, exactly as `Accumulator` does
(`aggregation.rs:208-243`); OR-13's reducer performs the same operations in
the same order. Across homes, each cell adds `v * w` and `w` home by home in
entry order, and a weighted mean divides once after the last home. The
operations and their order equal today's, which is what makes the result
bitwise equal.

*Null rule.* A null value contributes to neither the cell's weighted sum
nor its weight. A cell whose covered weight is zero is null. Each cell's
covered weight is reported in `FleetResult.covered_weight`. Today one null
home nulls the whole cell (`has_null`, `aggregation.rs:644, 663, 673, 693`)
and one null step nulls the home's period (`Accumulator::finish`,
`:235-238`); no step value is null before B-6 (facts above), so the new rule
changes no number at this commit. Its tests are FL-2's not-applicable
variables and B-6's nulls.

*Rows and labels.* Until FL-4, each home reduces on its own local calendar
and the fold aligns period rows by the instant of the period start (today's
epoch key). The aggregate's rows are the periods every folded home has a
row for (today's intersection). `Time` keeps today's Utf8 RFC 3339 UTC `Z`
label (`aggregation.rs:636-637`) until `09-numeric-fixes.md` B-12.

*Columns.*
- `aggregate`: `Time`, then one `Float64` column per folded variable, in the
  first folded home's order (today's order). The per-home `Step Count` and
  `[valid steps]` companions of §2.6 are not fleet columns: a cell's coverage
  is its covered weight. A variable mismatch between homes still empties the
  aggregate, as today (`aggregation.rs:572-586`), until FL-2.
- `covered_weight`: `Time` and the same variable columns, each cell the
  weight behind the matching aggregate cell.
- `homes`: one row per home: `bldg_id`, `weight`, `status` (`ok`, `flagged`,
  `failed`: today's `SimStatus`), `reason` (null for `ok`), `steps` (steps
  the home ran) and `periods_folded` (aggregate periods in which the home
  entered the fold), then the run metrics when the home requested them.
- `coverage`: `weight_total`, `weight_ok`, `weight_flagged`, `weight_failed`.

*Status.* Homes enter the fold as today: `Ok` and `Flagged` fold, `Failed`
does not (`aggregation.rs:274`). FL-2 narrows the fold to `Ok` and
replaces `flagged` with `degraded`.

*Destinations.* Fleet construction rejects two homes whose file
destinations resolve to the same path (`FleetError::DuplicateDestination`,
naming both building ids). The write-then-rename and overwrite rules for
each destination are `03-output-reporting.md` OR-3's.

*Python.*

```python
hares.FleetReport(selection, frequency, health_policy=HealthPolicy())  # frequency has no default
Fleet.simulate(report, *, n_threads=None, chunk_size=64,
               raise_on_failure=False, progress=None) -> FleetResult
FleetResult.aggregate         # tables, exported as the dwelling's tables are at this commit;
FleetResult.covered_weight    # 05-python-surface.md PY-1 moves all three to its Arrow export
FleetResult.homes
FleetResult.coverage          # Coverage: weight_total, weight_ok, weight_flagged, weight_failed (FL-2: weight_degraded)
FleetResult.n_succeeded       # homes that folded: ok and flagged here, ok only from FL-2
FleetResult.peak_live_home_results
```

The frequency has no default: no single period is a whole multiple of
every fleet's step (15 minutes is not one for a 3600 s fleet).
`raise_on_failure=True` raises `HaresSimulationError` naming every failed
home and its reason. Removed: the `resolution=` keyword,
`AggregationResolution` and `PyAggregationResolution` with its `from_str`,
and `FleetResults` with `per_dwelling_metrics`, `aggregate_timeseries` and
`failures` (a failed home is a `homes` row with status `failed` and its
reason).

*Callers.* Every site of two censuses moves in this commit. The first,
`git grep -n 'AggregationResolution\|aggregation::aggregate\|FleetResults\|aggregate_timeseries\|per_dwelling_metrics\|resolution='`,
names the old types: `crates/hares-fleet/src/{aggregation.rs,lib.rs:7}`;
`crates/hares-python/src/{py_fleet.rs,py_enums.rs:832-870,conversions.rs:9-16,146-147,lib.rs:29,40,86,101}`;
`python/ochre_next/__init__.py:9,60,146,189` and
`python/ochre_next/_hares.pyi:278-286,2267-2304` (under `python/hares/`
after `05-python-surface.md` PY-7); `docs/architecture.md:585`;
`tools/frame_golden/src/adapter.rs`. The second,
`git grep -nE '\bfleet(_clone)?\.simulate\(' -- benches tests crates tools docs`,
names the callers of `Fleet::simulate(n_threads)`, which the first pattern
does not match (at `21d90588` and `afa9a071`): `benches/fleet.rs:36`;
`tests/regression/fleet_scale.rs:30`;
`tests/regression/aggregation_check.rs:153` (`run_fleet_integration`);
the `hares-fleet` unit tests
(`crates/hares-fleet/src/fleet.rs:1221, 1318, 1341, 1378, 1395, 1414,
1431`); `crates/hares-python/src/py_fleet.rs:199, 201`;
`tools/frame_golden/src/adapter.rs:299`; `docs/architecture.md:629`; and
the Python tests below. `benches/aggregation.rs` and its `[[bench]]
aggregation` entry (`crates/hares-core/Cargo.toml:109-112`) are deleted;
the fold is benchmarked by the `fleet_fold` group of `benches/fleet.rs`
(Measurement). The tests pinned to the old API are each rewritten:
- `tests/python/test_py_enums.py:242-251` (`TestAggregationResolution`) is
  deleted with the enum; the Python `Frequency` binding and its tests are
  OR-13's.
- `tests/python/test_py_fleet.py:326-352` (at `21d90588`: the
  `resolution=` tests and the default-resolution test) become
  `test_simulate_minutes_15_report` and `test_simulate_hourly_report`,
  asserting the aggregate's row count and `n_succeeded`; `:440` and the
  other `simulate()` calls pass a `FleetReport`.
- `tests/python/test_py_conversions.py:164-216` read `FleetResult.aggregate`
  and `FleetResult.homes` and assert their schemas, on the committed
  `fleet` fixture `11-other-defects.md` OD-13 gives them (they run in the
  block from that row on).
- `tests/regression/aggregation_check.rs` (synthetic `DwellingOutcome`s
  through `aggregation::aggregate`, `:271`, and `run_fleet_integration`'s
  `fleet.simulate(1)`, `:153`) is rewritten as one test, which replaces
  both halves: it runs `Fleet::simulate` on four
  `tests/fixtures/hpxml/ochre_samples/base.xml` homes with weights 1, 2, 5
  and 1, the last given a nonexistent schedule path so it fails, and checks
  the same properties: the failed home is listed with its reason and
  excluded from `coverage.weight_ok`, a `Total Electric Power (kW)` cell is
  the weighted sum of the three homes' period means, a zone temperature
  (`C`) cell is their weighted mean, and the row count is the period count.

*Golden tool.* `frame_golden`'s fleet manifests replace the top-level
`resolution` key (`01-baseline-repair.md` BR-15) with a required `[fleet]`
table, a `FleetReport` verbatim (`02-measurement.md` M-1); each home keeps
BR-15's `weight`. The adapter calls the new `simulate` and emits the
`aggregate`, `covered_weight`, `homes` and `metrics` products; the
`weights` product (the covered-weight stand-in, `adapter.rs:349-370`) is
replaced by `covered_weight`.

**Acceptance test** (`crates/hares-fleet` unless stated):
- `fleet_fold_matches_collect_then_aggregate`: the golden
  `fleet_same_offset_300s` is captured with today's code in this row's
  first commit: kind `fleet`, homes `bldg_id` 1, 2 and 3, each
  `tests/fixtures/resstock/2025.1/bldg0176227/home.xml` with its
  `in.schedules.csv` and, in that order, the weather files
  `tests/fixtures/resstock/2025.1/weather/G0600770_2018.csv`,
  `G0600650_2018.csv` and `G0600790_2018.csv` (one county each, all UTC-8,
  so the homes share one column set and differ in value), weights 1.0, 2.5
  and 0.5 (`01-baseline-repair.md` BR-15's `weight`), `resolution =
  "fifteen_min"`, start `2018-01-01T00:00:00-08:00`, 2 d at 300 s,
  verbosity 2, seed 0: three steps per 15-minute bucket. In the second
  commit BR-15's `resolution` key of both fleet manifests becomes the
  `[fleet]` table (`fleet_same_offset_300s`: `frequency = { minutes = 15 }`;
  `fleet_mixed_offset`: `frequency = "hourly"`; both with the verbosity-2
  selection). Their `aggregate` (zero rows for `fleet_mixed_offset`) and
  `metrics` products compare bitwise; the `covered_weight` and `homes`
  products, whose shape is new, are captured in that commit with the reason
  recorded. The same commit records in `02-measurement.md`'s log, for every
  home of both fleet goldens, its `RunResult.warnings` and every non-zero
  `RunResult.health` field (the premise FL-2's golden expectation rests on).
- `fleet_aggregate_columns_match_home_report_schema`: on the
  `fleet_same_offset_300s` run, every `aggregate` column after `Time` has
  the name and the `unit` metadata of the variable of that name in the
  first home's own `Timestep` `Memory` report of the same selection, and
  the type `Float64`; `covered_weight` has the same names in the same
  order.
- `fleet_result_identical_across_threads_and_chunks`: the
  `fleet_same_offset_300s` homes through `Fleet::simulate` with `n_threads`
  in {1, 2, 8} and `chunk_size` in {1, 2, 64} give one `FleetResult`
  (aggregate, covered weight, homes table, coverage), compared column by
  column with `to_bits()` on every float.
- `fleet_peak_live_home_results_bounded_by_chunk`: the same fleet with chunk
  sizes 1 and 2 reports `peak_live_home_results` of exactly 1 and 2.
- `fleet_minutes_15_reduces_one_minute_steps`: two `base.xml` homes with
  `data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw`, the first with
  `data/examples/BEopt_example_schedule.csv` and weight 1, the second with
  `data/examples/bldg0112631_schedule.csv` (a different load profile) and
  weight 3, 2 h at 60 s, `Frequency::Minutes(15)`: eight rows, and each
  `Total Electric Power (kW)` cell equals, bitwise, `a·1 + b·3` where `a`
  and `b` are the means of that quarter hour's fifteen step values in each
  home's own `Timestep` `Memory` report (added in step order, divided once
  by 15). The test first asserts that `a` and `b` differ in at least one
  row, so a swapped home or weight cannot pass.
- `fleet_report_selects_columns_and_frequency`: the
  `fleet_same_offset_300s` homes with a report naming two variables at
  `Daily` give an `aggregate` and a `covered_weight` whose columns are `Time`
  and those two variables, with two rows.
- `selection_missing_from_some_homes_does_not_fail_them`: `base.xml` and
  `base-battery.xml` (whose battery declares
  `{"Battery": {"bms_mode": "\"Manual\""}}`, as `11-other-defects.md` OD-8
  requires) with `Named` of one battery variable and one variable both
  declare: both homes have status `ok`.
- `selection_matching_no_home_is_an_error`: the same fleet with a `Named`
  name neither declares fails with `UnmatchedSelection` naming it, and with
  `Pattern("NoSuchVariable*")` likewise.
- `fleet_homes_with_same_file_destination_are_rejected`: two homes whose
  configs each carry a `Parquet` report to one path.
- `fleet_home_report_named_fleet_is_rejected`: a home whose config carries
  a report named `fleet` fails construction with `ReservedReportName`
  naming its building id.
- The rewritten Python and regression tests above.

**Verification:** `cargo nextest run -p hares-fleet`; `cargo nextest run
-p hares-core --test regression` (the `regression` target is declared in
`crates/hares-core/Cargo.toml:49-51`); `uv run pytest tests/python/test_py_fleet.py
tests/python/test_py_enums.py tests/python/test_py_conversions.py`;
`frame_golden compare-all`; `cargo bench --no-run`.

**Commits:** `test: capture a golden same-offset fleet at 15-minute aggregation`,
then `perf: fold fleet homes into a bounded, deterministic streaming aggregate`.

**Dependencies:** `03-output-reporting.md` OR-1 (catalogue kinds), OR-3
(reports and destinations), OR-6 (`finish()` per home, `RunResult.warnings`),
OR-7 (from this row on the fleet reads each home's `Time` as naive local
milliseconds and subtracts the home's `hares:utc_offset_s` to recover
today's UTC epoch key), OR-12 (`HealthPolicy`), OR-13 (the period reducer
and `Frequency::Minutes`), OR-14 (the callback destination);
`02-measurement.md` M-1, M-2; `01-baseline-repair.md` BR-15 (manifest
weights and resolution).

**Measurement:** with the bench `benches/fleet.rs`, new group `fleet_fold`
(in place of the deleted `benches/aggregation.rs`), copies of
`base.xml` at 24 h and 900 s, one process per size
(`cargo bench -p hares-core --bench fleet -- fleet_fold/<n>`): peak RSS
(`VmHWM`, printed by the bench after its run) for 100, 1 000 and 10 000
homes, and the bytes of one home's reduced table
(`RecordBatch::get_array_memory_size`) at `Timestep` and `Hourly`, recorded
in `02-measurement.md` before and after. The slope of peak RSS from 1 000
to 10 000 homes at `Hourly` is recorded as the bookkeeping bytes per home
(entries, `homes` row, destination set). Wall time, as `plan.md` §3 rule 3
requires of a `perf:` commit, by `02-measurement.md` M-3's method (five
fresh runs, median, 5% noise band) at the parent and after: `frame_golden
run` of `fleet_same_offset_300s` and `fleet_mixed_offset`; and, after the
change, `fleet_fold/1000` with 8 threads at `chunk_size` 64 against
`chunk_size` 1000 (one chunk, which waits for its slowest home once, as
today's collect does), which shows what each chunk's wait for its slowest
home costs. If either golden is slower than its parent, or `chunk_size`
64 slower than `chunk_size` 1000, by more than the band, the row stops and
the chunk barrier
is replaced by an ordered completion window: up to `chunk_size` homes run
at once, home `i` is folded as soon as homes `0..i` are folded, and a new
home starts whenever one is folded, which keeps the entry fold order and
the bound of `chunk_size` live results; the three timings are then
measured again and recorded.

## FL-2: a flagged or degraded home is never counted as a success

**Kind:** initiative work. Fixes V16, V17.

**State:** committed `74ba20f9` (branch `row/r51-fl2`, worktree of
`5a9c4ac8`, 2026-10-11). The entry's change landed whole:
`HomeStatus { Ok, Degraded(String), Failed(String) }` decided by the one
`home_status` function over the home's run (warnings or any health
event degrade with the first warning and every non-zero field named;
an error, panic or `HealthPolicyViolation` fails with the error), the
degraded and failed homes excluded from the aggregate and the covered
weight, `Coverage.weight_flagged` renamed `weight_degraded`, the homes
table's status spelled `ok`/`degraded`/`failed`, `n_succeeded` the ok
homes' count, the aggregate's columns the union of the ok homes'
selected variables in first-appearance order (a mismatch no longer
empties it), and the zero-ok-weight fleet the `NoCoveredWeight` error
carrying the counts and the homes table, Python's exception carrying
the table as its `homes` attribute. The acceptance tests landed with
it, the RED first: `flagged_home_is_not_counted_as_success` failed at
the parent on the warned home counted a success (status `flagged`,
`n_succeeded` 2, the aggregate's covered weight 3) and passed after.
Two goldens re-captured: `fleet_mixed_offset`'s `aggregate` and
`covered_weight` from zero rows to the 165-row, 46-column union; and
`resstock_2025_fleet`, where the entry's expected spelling change
arose in a form the entry's premise paragraph governs: two homes
(`bldg0162486`, `bldg0352714`) carry cited-default and
cited-derivation warnings, are degraded with those reasons, and fold
alone the clean `bldg0019694` (48 rows, its tier-2 columns). The
mixed_offset homes' `status` values did NOT change spelling: FL-1's
premise record found every home clean, so the entry's expectation
yields to the premise record. The `homes` product's change is
`periods_folded` (0 to 165). The fixture corrections the acceptance
canaries demanded: `base.xml` and `base-battery.xml` declare
`ShieldingofHome normal` (the value the cited default applies), and
`BEopt_example.xml` drops the ignored `SimulationControl`
`TemperatureCapacitanceMultiplier`; both bitwise neutral, recorded in
`02-measurement.md`. Every other golden product compares bitwise,
`fleet_same_offset_300s` included.

**Problem:**
- A run whose metrics are unavailable or that ends with warnings is
  `CoreSimStatus::Flagged` (`engine.rs:146-150, 276-280`), mapped to
  `SimStatus::Flagged` (`fleet.rs:771`). Aggregation skips
  only `Failed` (`aggregation.rs:274`), records `failed: false` for a flagged
  home (`:268`), and Python counts it in `n_succeeded` (`py_fleet.rs:254,285`).
  FL-1 keeps this behaviour.
- A home with no rows is skipped after its status was already recorded
  (`aggregation.rs:257-270` then `:294-299`). The only way to get one today
  is a home whose frame is not retained (`extract_rows` returns `None`
  when `retain_batches` is false). FL-1 removes that path: the fleet
  appends its own `Timestep` report to every home whose restricted
  selection holds a variable, a run has at least one step (the one
  `SimulationConfig::validate()` every construction path runs from
  `05-python-surface.md` PY-8, row 34, rejects a zero duration and one
  that is not a multiple of the step, as `from_toml` does today,
  `crates/hares-io/src/config.rs:149-151, 166-170`), and a home whose
  restricted selection holds no variable is not applicable to every column
  (FL-1). No run reaches the fold with zero rows, so no status is defined
  for one.
- Any column difference between homes empties the whole aggregate
  (`aggregation.rs:572-584`), which FL-1 keeps;
  `test_heterogeneous_equipment_column_mismatch_emits_warning`
  (`tests/python/test_py_fleet.py:449-510` at `21d90588`) pins that
  emptying.

**Change:**
- `SimStatus` is replaced by `HomeStatus { Ok, Degraded(String),
  Failed(String) }` (`plan.md` §2.4), decided by one function,
  `home_status(&Result<RunResult, HaresError>) -> HomeStatus`. Mapping: a
  run that returns warnings, or whose `RunResult.health` records any event
  (a non-zero counter or an unconverged warm-up), is `Degraded`, its reason
  naming the first warning and each non-zero health field; a run error or
  panic, including a `HealthPolicyViolation` under the fleet's
  `health_policy`, is `Failed` with the error. Today's "metrics
  unavailable" flag (`engine.rs:146-150, 276-280`) has no counterpart:
  metrics are present exactly when requested (`plan.md` §2.2). There is no
  skipped status: `Fleet::simulate` runs every home, and the one place a
  home is skipped, `SteppableFleet::step` after an earlier failure
  (`SimError::Skipped`, `fleet.rs:745`), returns per-step results, not a
  `FleetResult`.
- A variable absent from a home's catalogue is not applicable to that home
  (FL-1's selection rule): the home stays `Ok` and is not part of that
  variable's covered weight. A home whose restricted selection holds no
  variable at all is the same case for every column (FL-1).
- `Degraded` and `Failed` homes do not enter the aggregate. They count in
  `Coverage.weight_degraded` or `weight_failed` (FL-1's `weight_flagged`
  becomes `weight_degraded`), never in `weight_ok`, and never in Python's
  `n_succeeded`, which counts `Ok` homes. The homes table's `status` takes
  `ok`, `degraded` or `failed`.
- The aggregate's columns are the union of the `Ok` homes' selected
  variables, in order of first appearance in entry order; a mismatch no
  longer empties it.
- A fleet whose `Ok` weight is zero is an error,
  `FleetError::NoCoveredWeight { ok, degraded, failed, homes: RecordBatch }`
  (replacing `ZeroWeightFleet`): the counts per status and the full `homes`
  table, so every home's status and reason survive the error. Its message
  names the counts. Python raises `HaresSimulationError` with the counts in
  the message and the table as the exception's `homes` attribute, exported
  as `FleetResult.homes` is.

**Acceptance test** (fixtures: `tests/fixtures/hpxml/ochre_samples/base.xml`
and `base-battery.xml`, the battery declaring
`{"Battery": {"bms_mode": "\"Manual\""}}` as `11-other-defects.md` OD-8
requires, with `data/examples/BEopt_example_schedule.csv` and
`data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw`, 24 h at 900 s,
`Hourly`). Each fleet test first asserts that every home it expects to be
`ok` is `ok`, so a warning a fixture carries fails the test naming it. The
degraded home is made by its inputs alone: its schedule is a copy of
`BEopt_example_schedule.csv`, written to a `tempfile` directory, with one
added column `unread_load` (every value 0.0), which no mapping reads, so
the run carries `03-output-reporting.md` OR-11's always-on unmapped-column
`Warning`.
- `home_status_follows_health_and_warnings` (unit test of `home_status`,
  over `RunResult`s built in the test): clean health with no warning and a
  disabled or converged warm-up is `Ok`; each `RunHealth` counter set to 1
  alone, `WarmupOutcome::Ran { converged: false, .. }` alone, and one
  warning alone are each `Degraded` with a reason naming that event; an
  error, including `HealthPolicyViolation`, is `Failed` naming it.
- `flagged_home_is_not_counted_as_success`: `base.xml` with the stock
  schedule (weight 1) and `base.xml` with the `unread_load` schedule
  (weight 2): the second home is `degraded` in `FleetResult.homes` with a
  reason naming `unread_load`, every aggregate cell equals the first home's
  period value with covered weight 1, `coverage.weight_ok` is 1 and
  `weight_degraded` 2, and Python `n_succeeded == 1`.
- `home_with_no_selected_variable_is_ok_and_uncovered`: `base.xml` (weight
  3) and `base-battery.xml` (weight 1) with `Named` of one battery variable
  only: the `base.xml` home is `ok` with `periods_folded` 0 and counts in
  `weight_ok` (4) and `n_succeeded` (2); the aggregate has 24 rows, and
  the battery column's covered weight is 1 in each.
- `battery_home_and_plain_home_aggregate_together`: `base-battery.xml` with
  weight 1 and `base.xml` with weight 3 give a non-empty aggregate; each battery variable's cell
  equals the battery home's period value with covered weight 1 (the null
  rule of FL-1: the plain home contributes neither value nor weight), and
  every shared variable equals the hand-computed combination of both homes'
  period values by the rule in force.
- `default_fleet_produces_aggregate_and_metrics`: the fleet above with
  `metrics = true` in both homes' configs returns a non-empty aggregate and
  non-zero per-home metrics; without it, its homes table has no metrics
  columns.
- `zero_ok_weight_raises`: two `base.xml` homes, both with the
  `unread_load` schedule, raise `NoCoveredWeight` with `ok` 0, `degraded`
  2, `failed` 0, and a `homes` table holding both building ids with status
  `degraded` and a reason naming `unread_load`; Python
  (`test_zero_ok_weight_raises_with_every_home`) reads the same two rows
  from the exception's `homes` attribute.
- `test_heterogeneous_equipment_column_mismatch_emits_warning`
  (`tests/python/test_py_fleet.py:449-510`) is rewritten to assert the
  non-empty aggregate and no warning, and renamed
  `test_heterogeneous_equipment_aggregates_the_union_of_columns`.

**Verification:** `cargo nextest run -p hares-fleet`; `uv run pytest
tests/python/test_py_fleet.py`; `frame_golden compare-all`, where
`fleet_mixed_offset`'s `aggregate` and `covered_weight` products change from
zero rows (its three homes declare different variables) and its `homes`
product's status values change spelling: those products are re-captured in
this commit and the change is recorded in `02-measurement.md`'s log; every
other product compares bitwise. That bitwise claim rests on a premise:
every home of `fleet_same_offset_300s`, and every home of
`fleet_mixed_offset`, is `ok` under the new mapping (no warning, no
non-zero health counter, a converged or disabled warm-up). FL-1's second
commit records each golden home's warnings and health; if any home carries
one, its exclusion from the aggregate is listed here as an expected change
of that golden's `aggregate`, `covered_weight` and `homes` products,
re-captured in this commit with the home and the event named, or the
event's cause is fixed first if it is a defect.

**Commit:** `fix: never count a flagged or degraded home as a fleet success`

**Dependencies:** FL-1; `03-output-reporting.md` OR-5 (`RunHealth`),
OR-11 (the unmapped schedule-column `Warning` the degraded fixture
carries), OR-12 (`HealthPolicy`), OR-6 (`RunResult.warnings`);
`11-other-defects.md` OD-8 (the battery fixture's declared control).

## FL-3: `SteppableFleet::step` past the end is one fleet-level error

**State:** committed `455eb8a3` (branch `row/r52-fl3-fl4`, 2026-10-11).
`SteppableFleet::step` returns
`Result<Vec<Result<StepResult, SimError>>, FleetError>` and after
`is_finished()` returns `Err(FleetError::AlreadyFinished { total_steps })`
with no per-home results; Python's `step()` raises `HaresSimulationError`
naming the finished step count (`test_steppable_fleet_step_after_finish_raises`,
RED at the parent: DID NOT RAISE). `telemetry` returns
`Result<DwellingTelemetry, FleetError>` (`NoSuchHome { index, len }` out of
range, `Telemetry { bldg_id, source }` for a snapshot that cannot be
built), `set_grid_voltage` and `apply_control` return `Result` with
`NoSuchHome` in place of the logged warning, and the building ids come
from an infallible `SteppableFleet::bldg_ids()`: a failed snapshot in
`step()` is that home's `ok: false` item carrying the message (never a
silent `None` reactive power) and `bldg_id(idx).unwrap_or(-1)` is gone
(`git grep 'unwrap_or(-1)'` over the binding prints nothing, SL-10's
check). Python maps the errors through one conversion function:
`NoSuchHome` to `IndexError`, `Telemetry` to `HaresSimulationError`
naming the building id and the snapshot error, and the binding's own
bound checks before `apply_control` and `set_grid_voltage` are gone. RED
first: the acceptance tests failed at the parent as compile errors on the
missing API. Full block green on the commit (fmt; clippy --workspace
--all-targets --all-features -D warnings; nextest --all-features -j 4
7951/7951, 1 skipped; pytest 1676 passed, 6 xfailed, 0 failed, 0 errors;
pyright 0; compare-all 8 + dst 1 identical).

**Kind:** initiative work. Fixes V41.

**Problem:** every step up to the end returns `Ok` per home
(`fleet.rs:1600-1604` asserts it). Calling `step()` after `is_finished()`
returns one `SimError::Failed { message: "simulation already reached
configured end" }` per home (`fleet.rs:567-578`), so a caller sees every home
"failed" when the fleet has simply finished; `fleet.rs:1608-1614` pins it,
and Python turns each into an `ok: false` item (`py_fleet.rs:444, 485-493`).

**Change:** `SteppableFleet::step` returns
`Result<Vec<Result<StepResult, SimError>>, FleetError>`; after
`is_finished()` it returns `Err(FleetError::AlreadyFinished { total_steps })`
and no per-home results. Python's `step()` raises `HaresSimulationError`
naming the finished step count.

The same commit removes the silent paths of `SteppableFleet`'s per-home
accessors (`10-slop-cleanup.md` SL-10):
- `telemetry(index)` returns `Result<DwellingTelemetry, FleetError>`:
  `FleetError::NoSuchHome { index, len }` for an index out of range and
  `FleetError::Telemetry { bldg_id, source }` for a snapshot that cannot be
  built, in place of `None` for both (`fleet.rs:638-659` at `21d90588`).
- `set_grid_voltage(index, ..)` and `apply_control(index, ..)` return
  `Result<(), FleetError>` with `NoSuchHome` in place of a logged warning
  (`fleet.rs:612-636`).
- Python maps `NoSuchHome` to `IndexError` and `Telemetry` to
  `HaresSimulationError` naming the home; the binding's own bound check
  before `apply_control` (`py_fleet.rs:540-547`) goes, since the Rust call
  now reports it. A failed snapshot is no longer reported as an index out of
  range (`py_fleet.rs:551-562`).
- `step()`'s per-home `reactive_power_kvar` reads `telemetry(idx)` with `?`
  semantics: a home whose snapshot fails carries the error in its item
  (`ok: false`, with the message) instead of a silent `None`, and the
  building ids come from an infallible iterator over the homes, so the
  unreachable `bldg_id(idx).unwrap_or(-1)` (`py_fleet.rs:446`) goes.

A `SteppableFleet` home records only the reports its own config requests
(`03-output-reporting.md` OR-3) and finalises through `finish()` (OR-6), so
requirement FL-9 (stepping a fleet without full telemetry) holds by
construction and needs no change here.

**Acceptance test:** `step_after_finish_is_already_finished_error` replaces
`fleet.rs:1608-1614`: after the loop, `step()` returns
`Err(FleetError::AlreadyFinished { .. })`. Python:
`test_steppable_fleet_step_after_finish_raises`.
`steppable_fleet_accessors_reject_a_missing_home`: `telemetry`,
`set_grid_voltage` and `apply_control` with index `len` return
`NoSuchHome { index, len }`. `fleet_error_maps_to_python_exception` (a
`hares-python` unit test of the one conversion function the binding uses):
`NoSuchHome` becomes `IndexError` and `Telemetry` becomes
`HaresSimulationError` whose message names the building id and the
snapshot error. Python:
`test_steppable_fleet_telemetry_out_of_range_raises_index_error`.

**Verification:** `cargo nextest run -p hares-fleet`; `uv run pytest
tests/python/test_py_fleet.py`.

**Commit:** `fix: stepping a finished SteppableFleet is one fleet error, not a failure per home`

**Dependencies:** FL-2 (`HomeStatus`).

## FL-4: fleet time alignment is declared

**State:** committed `d1c52c42` (branch `row/r52-fl3-fl4`, 2026-10-11).
`Alignment::{Instant(FixedOffset), LocalClock}` is declared at
construction (`Fleet::with_alignment`; Python `hares.Alignment.instant(
"-07:00")` / `hares.Alignment.local_clock()` with the `alignment=`
keyword on `Fleet.from_buildings` and `Fleet.from_resstock`). The
pre-pass (`hares_io::hpxml::read_site`, `hares_io::read_weather_meta`,
`hares_core::dwelling::site_offset` with the shared checked offset
conversion) runs over every entry before the first chunk starts, reading
no schedule, no defaults and no weather row; a fleet whose homes span
more than one UTC offset and declares no alignment fails
`AlignmentRequired { offsets_s }` there with the progress callback never
called, and the by-alignment time-grid check fails
`HeterogeneousTimeGrid { bldg_id }` naming the first home that breaks
it. The period-origin offset rides `PeriodCalendar` (`origin_offset_s`,
`home_offset_s`; the default buckets the fed wall clock itself, a home's
own reports unchanged); the rows align by period (naive local start
under `LocalClock`, the period's start instant under `Instant` and
undeclared) and the labels keep the Utf8 RFC 3339 forms (`+00:00`
renders `Z` byte for byte the undeclared label, `-07:00` renders at the
declared offset, `LocalClock` with no offset); under `Instant` a home
period whose step count is below the period's full span is null for that
home, so edge periods carry the reduced covered weight. `frame_golden`'s
`[fleet]` table takes the optional `alignment` and `fleet_mixed_offset`
declares `Instant(+00:00)`: every product compares bitwise with the
parent's capture (compare-all 8 + dst 1 identical). RED first: the nine
acceptance tests failed at the parent as compile errors on the missing
API. Full block green on the commit (fmt; clippy --workspace
--all-targets --all-features -D warnings; nextest --all-features -j 4
7960/7960, 1 skipped; pytest 1678 passed, 6 xfailed, 0 failed, 0 errors;
pyright 0). A home whose site the pre-pass cannot resolve declares no
offset and fails its run for the same cause, the way the selection's
per-home restriction treats a home whose inputs fail to parse; the
committed `cross_offset_homes_align_on_the_start_instant` pin declares
`Instant(+00:00)` now (its assertions hold verbatim: the same labels,
the same bits). One documented behavior, not a defect: a 7 h `time_res`
at `Daily` under `Instant` drifts the steps across the reference day
boundaries, so the partial-coverage rule nulls the drifted periods the
same run folds under `LocalClock`, the alignment's period semantics
applied to a resolution that does not divide the hour, and realistic
resolutions divide the hour, so the step grid never drifts like this.

**Kind:** initiative work. The label change (every alignment to a naive
local `Timestamp`) is the numeric fix B-12 (`09-numeric-fixes.md`), in its
own commit; FL-4 keeps the Utf8 RFC 3339 labels and single-offset output as
they are today.

**Problem:** today every home buckets its own local wall clock and the fold
joins buckets by epoch second (`aggregation.rs:448-466, 508`), labelled in
UTC with a `Z` suffix (`:636-637`). For whole-hour offsets at an hour or
finer that is `Instant` alignment labelled at `+00:00`. Nothing declares
it, a fleet mixing offsets gets it silently, and homes with different start
times or resolutions fall into a bucket intersection (`aggregation.rs:588-597`).

**Change:**
- A fleet declares `Alignment::{Instant(FixedOffset), LocalClock}` at
  construction (`plan.md` §2.4): `Fleet::with_alignment`, and in Python an
  `alignment=` keyword on `Fleet.from_buildings` and `Fleet.from_resstock`
  taking `hares.Alignment.instant("-07:00")` or
  `hares.Alignment.local_clock()`. The decision's "reference zone" is a
  fixed UTC offset, because HARES output time is naive local standard time
  with no daylight saving (`plan.md` §2.5).
- The calendar is chosen before reduction, not after: the fleet passes each
  home's reducer (FL-1) a period calendar. For `LocalClock`, and for a
  single-offset fleet with no declared alignment, it is the home's own local
  calendar. For `Instant(offset)` it is the declared offset's calendar: each
  step's instant (its naive local time minus the home's offset) is shifted
  to the declared offset and bucketed on that offset's period boundaries, so
  a UTC-5 home and a UTC-8 home reduce onto the same reference days. This
  commit adds that period-origin offset to `03-output-reporting.md`
  OR-13's `PeriodCalendar` (`plan.md` §2.6); a home's own reports keep its
  own offset.
- Rows align by period: `LocalClock` by the naive local period start;
  `Instant` and an undeclared single-offset fleet by the period's start
  instant. The rows stay the periods every folded home has a row for
  (FL-1).
- Partial coverage under `Instant`: a home period row whose `Step Count` is
  below the full period's step count is null for that home (FL-1's null
  rule), so an edge period that some homes cover only in part carries a
  reduced covered weight. Under `LocalClock` or a single offset, the homes
  share one grid (below), every home covers the same steps of each period,
  and a partial period folds as it does today. Homes whose start instants
  are equal cover the same steps of every reference period, so under
  `Instant` they too fold every period at full covered weight.
- Labels: the `Time` column keeps FL-1's representation, a Utf8 RFC 3339
  string (`Field::new("Time", DataType::Utf8, false)`, `aggregation.rs:715`
  at `21d90588`), for every alignment. `09-numeric-fixes.md` B-12 moves
  every alignment to a naive `Timestamp(Millisecond, None)` and owns the
  local-time label tests; this entry moves no label of a fleet that today's
  code accepts. `Instant(offset)` renders each row's start instant in the
  declared offset with today's formatter
  (`to_rfc3339_opts(SecondsFormat::Secs, true)`, `:636-637`), so `+00:00`
  renders with `Z`, byte for byte today's label, and `-07:00` renders
  `2018-01-01T00:00:00-07:00`. `LocalClock` renders the shared naive local
  period start with no offset (`2018-01-01T00:00:00`), because its homes
  share no offset. An undeclared single-offset fleet keeps today's UTC `Z`
  labels.
- Each home's site UTC offset is known only once its site is resolved
  (`DwellingBlueprint::from_parts` resolves it with `resolve_site_location`
  from the HPXML `Site`, the weather header and
  `SimulationConfig.site_location`, `crates/hares-core/src/dwelling/blueprint.rs:75-98`,
  and reinterprets the naive start at it). The fleet does not parse every
  home to learn it: a pre-pass reads only what that resolution reads.
  - `hares_io::hpxml::read_site(path) -> Result<Site, HpxmlError>` reads
    the HPXML document and runs only the `Site` extraction
    (`building.rs:481-516` at `afa9a071`: elevation, latitude, longitude,
    `TimeZone/UTCOffset`), moved into one function that `parse_hpxml`
    also calls; no `Building` is built.
  - `hares_io::read_weather_meta(path) -> Result<WeatherMeta, WeatherError>`
    detects the format and reads only its header (EPW's `LOCATION` line,
    the PSM3 and TMY3 header rows; a ResStock CSV has no location and
    returns `has_embedded_location: false`, as its full parser does), the
    same header function each full parser calls; no row is read.
  - `hares_core::dwelling::site_offset(config: &DwellingConfig) ->
    Result<FixedOffset, HaresError>` passes those two and
    `config.sim_config.site_location` to `resolve_site_location` and
    converts the hours with the checked conversion `from_parts` uses
    (`11-other-defects.md` OD-2 site 8), so the pre-pass and the built
    dwelling cannot disagree except through their inputs.
  `Fleet::simulate` runs the pre-pass over every entry before the first
  chunk starts (it reads no schedule, no defaults and no weather row), and
  raises the two errors below from it, before any home runs.
- A fleet whose homes span more than one UTC offset and declares no
  alignment fails with `FleetError::AlignmentRequired { offsets_s }`
  naming the distinct offsets.
- The time grid is checked in the same pass, by alignment. Under
  `LocalClock`, and for a single-offset fleet with no declared alignment,
  homes must share naive local start time, duration and resolution. Under
  `Instant(offset)` the naive starts are not compared: each home's start
  instant (its naive start at its resolved offset) is, together with
  duration and resolution. Homes must share duration and resolution; each
  start instant must fall on a step boundary of the declared offset's
  calendar (a whole number of steps from that offset's midnight), so every
  step lands in one reference period; and the homes' runs must share at
  least one whole reference period at the report's frequency. Start
  instants may differ: a mixed-offset fleet whose naive starts are equal
  covers partial edge periods (the rule above), and one whose naive starts
  are chosen to share one instant covers the same physical window. A
  violation is `FleetError::HeterogeneousTimeGrid { bldg_id }` naming the
  first home, in entry order, that breaks the rule.
- `frame_golden`'s `[fleet]` manifest table gains an optional `alignment`
  (absent means no declared alignment, the engine's own meaning), and
  `fleet_mixed_offset` declares `Instant(+00:00)`.

**Acceptance test** (`crates/hares-fleet`, on M-2's `fleet_mixed_offset`
inputs: homes at UTC-5, UTC-6 and UTC-8, weight 1.0 each, each starting at
its local `2018-01-01T00:00`, 7 d at 3600 s):
- `mixed_offset_fleet_requires_declared_alignment`: `simulate` without an
  alignment fails with `AlignmentRequired` naming -18 000, -21 600 and
  -28 800 s, and the fleet's progress callback was never called (no home
  ran).
- `site_offset_pre_pass_matches_built_dwellings`: for each of the three
  homes, `site_offset` equals the `hares:utc_offset_s` of the dwelling
  built from the same config.
- `site_offset_pre_pass_reads_only_the_site_and_the_weather_header`: a
  copy of one home's config whose schedule path and defaults directory do
  not exist, and whose weather file is a `tempfile` copy holding only the
  header line of `G0900090_2018.csv`, gives the same `site_offset` as the
  original config.
- `instant_alignment_of_one_start_instant_covers_every_period`: the three
  homes with per-home starts chosen to share one instant
  (`2018-01-01T03:00` at UTC-5, `02:00` at UTC-6, `00:00` at UTC-8, each
  7 d at 3600 s), `Instant(-08:00)` and `Daily`: seven rows labelled
  `2018-01-01T00:00:00-08:00` to `2018-01-07T00:00:00-08:00`, and the
  covered weight of every variable all three homes declare is 3.0 in every
  row.
- `instant_start_off_the_reference_step_grid_is_an_error`: the same fleet
  with the UTC-8 home starting at `00:30` fails with
  `HeterogeneousTimeGrid` naming it.
- `local_clock_labels_are_shared_naive_local_time`: with `LocalClock` and
  `Hourly`, the `Time` column is Utf8, the first row is labelled
  `2018-01-01T00:00:00` and the covered weight of every variable all three
  homes declare is 3.0 in every row.
- `instant_alignment_buckets_on_reference_boundaries`: with `Daily` and
  `Instant(-07:00)`, the UTC-5 and UTC-6 homes start at 22:00 and 23:00 on
  reference day 2017-12-31 and the UTC-8 home at 01:00 on reference day
  2018-01-01, and each covers seven days from its start. The rows are the
  reference days every home has a row for, `2018-01-01` to `2018-01-07`
  (seven rows; 2017-12-31 and 2018-01-08 each lack a home), labelled
  `2018-01-01T00:00:00-07:00` to `2018-01-07T00:00:00-07:00`, and the
  covered weight of every variable all three homes declare (for example
  `Total Electric Power (kW)`) is 2.0 on the first day (the UTC-8 home
  covers 23 h of it), 3.0 on the five interior days, and 1.0 on the last
  (the UTC-5 and UTC-6 homes cover 22 h and 23 h of it).
- `instant_utc_equals_todays_output`: `Instant(+00:00)` at `Hourly`
  reproduces `fleet_mixed_offset`'s golden, captured at this entry's parent,
  bitwise, the `Time` column included (Utf8, `Z` labels).
- `heterogeneous_time_grid_is_an_error`: two `base.xml` homes whose naive
  starts differ by one hour, no alignment declared, fail with
  `HeterogeneousTimeGrid` before any home runs.

**Verification:** `cargo nextest run -p hares-fleet`; `frame_golden
compare-all`.

**Commit:** `feat: fleets spanning UTC offsets declare their time alignment`

**Dependencies:** FL-1; `03-output-reporting.md` OR-7 (each home's `Time`
as naive local milliseconds, which the reducer reads directly, offset
already subtracted by FL-1's row), OR-13 (the `PeriodCalendar` this entry
gives an origin offset);
`02-measurement.md` M-2 (`fleet_mixed_offset`); `11-other-defects.md`
OD-2 (site 8's checked offset conversion, shared by the pre-pass).
`06-runtime-memory.md` RT-4 later moves `read_site`, `read_weather_meta`
and `site_offset` from paths to `InputSource` and `WeatherInput` with the
rest of `DwellingConfig`.

## FL-5: fleet variables fold by their declared kind

**Kind:** numeric fix (a found defect: the unit tables sum ratios, binary
states, mode codes and intensive quantities across homes, and average
per-step sums over time). Its own commit and its own row in the numeric
block, following `09-numeric-fixes.md`'s rules (delta recorded; no golden
holds a changed column, below).

**Problem:** FL-1 keeps the parent commit's unit-keyed rules
(`unit_keyed_fleet_rules`) so that it moves no number. Under them a fleet's
`Ratio` (`fraction`, `ratio`, `%`) and `State` (`bool`) cells, the `code`
and `index` mode cells, and intensive quantities such as voltage are
weighted sums over homes, which have no physical meaning, and a `Quantity`
declared `Sum` over time (a per-step count or volume) is averaged.

**Evidence:** `crates/hares-fleet/src/aggregation.rs:43-50` and `:63-72`
(the time table and its mean fallback), `:119-126` and `:141-150` (the
cross-home table and its weighted-sum fallback) at `ebdb1a17`, carried as
`unit_keyed_fleet_rules` by FL-1; the unit census in this file's facts.

**Change:** `unit_keyed_fleet_rules` is deleted. Each home's fleet reducer takes
`VariableDef.aggregation` over time, except that `Enum` stays `Mean` until
B-11; across homes, the fold takes FL-1's table: `Energy` and `Power` the
weighted sum, `Temperature`, `Ratio` and `State` the weighted mean,
`Quantity` its declared `across_homes`, and `Enum` the weighted mean until
B-11. `Unit::Enum`, which `03-output-reporting.md` OR-1 kept apart so the
unit-keyed rules stayed bitwise, merges into `Unit::Code` (symbol `code`);
the `unit` metadata change of the former `enum` columns is listed in the
commit body. `tracing` at `debug`: each variable's two rules at fold setup.

**Acceptance test** (`crates/hares-fleet`). Each test fails at this entry's
parent, because its selection holds variables whose unit-keyed rule differs
from the declared one. The fleet goldens select the verbosity-2 tier, whose
columns are all `Power` or `Temperature` (units `kW`, `therms/hour`,
`kVAR`, `C` and `W`, `crates/hares-io/src/output/columns.rs:425-530`) and
already take the declared rules, so they cannot show the change.
- `fleet_fold_follows_declared_kinds` (a unit test of the cross-home fold,
  fed stated per-home period values): three homes with weights 1.0, 2.5 and
  0.5 and one period, with one variable of each kind whose rule changes: a
  `Ratio` (unit `fraction`), a `State` (`bool`), an intensive `Quantity`
  declared `AcrossHomes::Mean` (`V`), a `Quantity` declared `Sum` over time
  and `AcrossHomes::Sum` (`count`) and an `Enum` (`code`). Each cell equals
  the declared combination added in entry order and compared with
  `to_bits()`: the weighted mean for the `Ratio`, the `State`, the `V`
  quantity and, until B-11, the `Enum`; the weighted sum for the `count`
  quantity. At the parent the first four are weighted sums.
  `09-numeric-fixes.md` B-11 deletes the `Enum` case, which its own Mode
  tests replace.
- `home_reducer_sums_a_quantity_declared_sum`: one home's fleet reducer fed
  three steps of the `count` quantity (1, 2 and 3) in one 15-minute period
  reduces it to 6 (the parent averages it to 2).
- `fleet_cells_follow_declared_kinds`: the `fleet_same_offset_300s` inputs
  at `Minutes(15)` with `Selection::Pattern("*")` (every variable the homes
  declare). The test first asserts that the selection holds at least one
  `Ratio` and one intensive `Quantity` (the gas water heater's `fraction`
  and the air conditioner's `min` durations at `21d90588`). For every
  aggregate variable that is not an `Enum` and every period, the cell
  equals the declared combination of the three homes' period values, each
  computed from that home's own `Timestep` `Memory` report by the declared
  time rule, added in entry order and compared with `to_bits()`. `Enum`
  variables are excluded: their rule changes again at `09-numeric-fixes.md`
  B-11, whose `fleet_enum_cells_follow_mode` checks them on this same run,
  so this test holds unchanged from this commit on.
- `fleet_ratio_and_state_are_weighted_means`: in the same run, every
  `Ratio` and `State` cell lies between the smallest and largest of the
  homes' period values.

**Verification:** `cargo nextest run -p hares-fleet`; `frame_golden
compare-all`: `fleet_same_offset_300s` and `fleet_mixed_offset` compare
bitwise (their verbosity-2 columns keep their rules; an empty delta is the
expected result and is recorded), and any difference stops the entry. The
commit adds its `docs/python.md` "Numeric changes" line (`plan.md` §2.8):
fleet variables fold by their declared kind, no golden re-captured.

**Commit:** `fix: fold fleet variables by their declared kind, not by unit string`

**Dependencies:** FL-1, FL-4; `03-output-reporting.md` OR-1 (`VariableKind`,
with `kW`, `W`, `kVAR` and `therms/hour` variables declared `Power` and
every `Quantity` declaring `across_homes`). Lands before
`09-numeric-fixes.md` B-11, which then switches `Enum` to `Mode` in both
stages.

**Measurement:** the delta of every changed column of
`fleet_cells_follow_declared_kinds`'s run (the reconciled `Named`
selection, the verbosity-8 tier plus the three weather channels, on the
`fleet_same_offset_300s` inputs at `Minutes(15)`), before and after,
recorded in `02-measurement.md`, with the empty golden delta.

**State:** committed `a39d17a7` (branch `row/r87a-fl5`, worktree
`r87a-fl5`, based on `ff39b24b`, one commit). In brief: both fleet
stages fold every variable by its declared kind and the unit-keyed
rules are gone. The declared rules reach the fold as schema field
metadata: the schema builders write every catalogue variable's
lowercase `kind` tag, the declared over-time `aggregation` tag and,
for a `Quantity`, the declared `across_homes` tag beside the unit (an
`Enum` column's `enum_labels` label count was already there), and
`field_fleet_rules` reads them: over time the variable's declared
aggregation (the reporting reducer's own rule set, so a quantity
declared `Sum` sums its steps and a running total takes its value at
the period's last valid step); across homes energy and power the
weighted sum, temperature, ratio and state the weighted mean, a
quantity its declared `across_homes`, an enum the weighted Mode.
`unit_keyed_fleet_rules` and the fleet's unit-symbol table are
deleted, and so is the parent's name-suffix parsing: a schema field
without the declared-kind tag is a contract violation and a hard
error naming the field (`FleetError::UndeclaredKind`; no rule is
guessed from the column name; no catalogue-built field reaches it),
pinned by its own test; the fold setup's two debug traces name the
kind tag beside the two rules. `Unit::Enum` merges into `Unit::Code`
(symbol
`code`): the unit metadata change of the former `enum` columns is
none, because no catalogue variable declares the `enum` unit (its
spellings became `code` and the dimensionless `-` at the catalogue
commit), so no column's `unit` metadata changes; the symbol and the
never-called enum-unit constructor leave `Unit`. The lowercase kind,
aggregation and across-homes tags are shared spellings on the
`hares-types` types, which the Python catalogue's tag functions
delegate to. The acceptance tests landed with it, RED first, all four
run at the parent over the tests alone:
`fleet_fold_follows_declared_kinds` failed there on the ratio cell
1.15, the unit-keyed weighted sum, against the declared weighted mean
0.2875; `home_reducer_sums_a_quantity_declared_sum` failed there on
the count quantity's reduced period 2.0 (the mean of 1, 2 and 3)
against the declared sum 6.0; `fleet_cells_follow_declared_kinds`
failed there on `Outdoor Pressure (kPa)` period 0, the cell 405.3
against the declared 101.325;
`fleet_ratio_and_state_are_weighted_means` failed there on
`Outdoor Humidity Ratio (kg/kg)` period 0, the cell 0.0184815805,
outside the homes' period values [0.0042444, 0.0060966]. After: all
four green. Three reconciliations. First, B-11 (schedule row 88)
landed before this row, so the entry's "`Enum` stays `Mean` until
B-11" cases are B-11's landed Mode rule: its own tests carry it, and
the two tests the entry wrote an `Enum` case into exclude it exactly
as they planned to. Second, the acceptance run's selection
reconciled: the entry's `Pattern("*")` run expected the gas water
heater's `fraction` and the air conditioner's `min` durations among
the columns whose unit-keyed rule differs from the declared one, and
under the catalogue's canonical units those columns are dimensionless
and seconds columns (the power factor, the runtime fraction and the
COP at `-`; the minimum on and off times at `s`) whose unit-keyed
rules already agree with the declared ones; every verbosity-8 column
of these homes' catalogue agrees with its declared rules, so the run
is a `Named` selection over the verbosity-8 tier plus the three
weather channels no tier preset carries, where the differing rules
live (`Outdoor Pressure (kPa)`, `Outdoor Humidity Ratio (kg/kg)`, a
ratio, and `Wind Speed (m/s)`: intensive quantities the fallback
sums). The test's kind census assert holds against the reconciled
selection. Third, B-11's observe-gate reconciliation carries (a
pattern selection over the full catalogue panics the roster plan when
the observe feature is off), so the run selects by name. The
measurement is in `02-measurement.md`: the acceptance run's delta is
3 of 195 columns (the weather channels, each cell from the weighted
sum to the weighted mean, a division by the covered weight 4), the
`covered_weight` product bitwise, and a verbosity-3 run of the same
inputs moves nothing; the golden delta is empty, verified bitwise
(compare-all 8 manifests identical, `full_year_900s_dst` identical):
the fleet goldens' verbosity-2 selection holds only energy, power and
temperature columns, whose unit-keyed rules already matched the
declared ones. Gates at `a39d17a7`: fmt clean; clippy `--workspace
--all-targets --all-features -D warnings` clean; nextest `--workspace
--all-features -j 2` 8148/8148 (8 skipped); `pytest -m ''` 1766
passed + 6 xfailed, 0 failed, 0 errors; pyright 0; the goldens
bitwise. The commit adds `docs/python.md`'s Numeric changes line:
fleet variables fold by their declared kind, no golden re-captured.
The independent review's fix on the branch (a later commit on
`row/r87a-fl5`) lands the shape above: the production-dead
name-suffix fallback and its pin test are deleted (that test pinned
the untagged field's silent guess by column name), the untagged case
is `FleetError::UndeclaredKind` naming the field at both stages (the
cross-home fold and the reducer feed), the incidental test fixtures
that hand-built untagged fields carry the declared tags the schema
builders write instead, and the new pin runs the error at the rules
function and at the fold. No number moves: the goldens are bitwise.
Gates re-run at the fix: fmt clean; clippy `--workspace --all-targets
--all-features -D warnings` clean; nextest `--workspace --all-features
-j 2` 8148/8148 (8 skipped); `pytest -m ''` 1766 passed + 6 xfailed,
0 failed, 0 errors; pyright 0; the goldens bitwise (compare-all 8 +
dst 1).
