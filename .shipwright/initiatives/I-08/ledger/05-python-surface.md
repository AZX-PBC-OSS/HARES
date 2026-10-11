# Python surface

Implements, on the Python side, `plan.md` §2.1 (the `RunResult` surface),
§2.2 (the callback destination, the catalogue accessor), §2.7 (one config
type, the package decision) and the zero-copy export
(`dl-i08-architecture.python_export`); the RL reward contract; build and
provenance identity. Fixes V15, V18, V20, V32 and V33.

Order (`ledger/00-schedule.md` carries it): PY-7 runs after the opening
sequence, the profiler entries and SL-8, and before `11-other-defects.md`
OD-1 (which then registers exceptions under `hares._hares`). PY-5 and
PY-12 follow PY-7. PY-8 and PY-2 run after OD-2 and `12-inputs.md` IN-6
and before `03-output-reporting.md` OR-3. PY-9 runs directly after OR-6
(and after OR-12). After the fleet entries the order is PY-1, PY-3, PY-4,
PY-6, PY-10, PY-11.

Every commit keeps the workspace compiling and every test passing. When a
Rust entry in `03-output-reporting.md` changes an API the bindings call
(OR-3, OR-6), that commit adapts `hares-python` internally and keeps the
Python-facing behaviour, except where the entry itself states a Python
change. OR-3 removes the output keywords and adds the report
constructors, so Python can request output from that commit on; each
later entry that adds a report, frequency, destination or policy type, or
a built-now capability with a Python surface (OR-12, OR-13, OR-16,
`12-inputs.md` IN-2 to IN-4), adds its Python half in its own commit.
The one exception is OR-15's `metrics_from_table`, whose binding needs
the Python `Catalogue` and the Arrow stream import: PY-4 adds it. The
Python-facing lifecycle change is PY-9's; until PY-1, PY-9's
`ReportResult.table()` converts through the existing IPC function, which
PY-1 then deletes.

Test rules: as `03-output-reporting.md`. Python tests run under the
block's `[python-build]` and `[pytest]` lines (`plan.md` §3), which install
every extra and group, so the gym tests run rather than skip. A
Verification line that names `uv run --no-sync pytest <path>` is the
focused form of `[pytest]` used while implementing; the whole block still
runs after the commit.

## PY-7: rename the Python package to `hares-py` / `hares`

**Kind:** initiative work (`dl-i08-architecture.package`).

**Problem:** the distribution is `ochre_next` (`pyproject.toml:6`), the
extension module is `ochre_next._hares` (`pyproject.toml:81`), the package
lives in `python/ochre_next/`, the download caches are
`~/.cache/ochre_next/weather` and `~/.cache/ochre_next/resstock`
(`python/ochre_next/data/weather.py:36`, `data/resstock.py:126`), and the
exceptions are created under the bare module name `_hares`
(`crates/hares-python/src/py_dwelling.rs:30-37`). At `ebdb1a17`,
`git grep -c -i -E 'ochre[_-]next'` finds 787 occurrences.

**Change:**

- `git mv python/ochre_next python/hares`; `pyproject.toml`:
  `name = "hares-py"`, `module-name = "hares._hares"`. The Rust `[lib]`
  name `_hares` and `#[pymodule] fn _hares` (`crates/hares-python/src/lib.rs:52`)
  stay. `uv lock` regenerates `uv.lock`.
- `python/hares/_hares.pyi`, every import in `python/`, `tests/python/`,
  `scripts/`, `notebooks/simulation_explorer.py`, `README.md`,
  `docs/python.md`, `docs/development.md`, and the path strings in code
  comments (`crates/hares-equipment/src/pv/lut.rs:36`).
- Cache directories become `<cache base>/hares/weather` and
  `<cache base>/hares/resstock`; existing `ochre_next` caches are not
  read, moved or aliased.
- No `ochre_next` alias package, re-export or import fallback.
- Census after the change:
  `git grep -n -i -E 'ochre[_-]?next|\.cache/ochre'` returns nothing. The
  planning and review records that name the old package left the tracked
  tree in `10-slop-cleanup.md` SL-8 and are excluded from the census by not
  being tracked (`dl-i08-architecture.public_planning_docs`); they are not
  rewritten. The pattern does not match the OCHRE reference itself
  (`vendors/OCHRE`, the `ochre` dependency group and pytest marker), which
  keeps its name.

**Acceptance tests** (`tests/python/test_package.py`, new):
`import_ochre_next_fails` (`importlib.util.find_spec("ochre_next") is None`);
`extension_module_is_hares_hares` (`hares._hares.__name__ == "hares._hares"`);
`distribution_name_is_hares_py`
(`importlib.metadata.distribution("hares-py")` resolves);
`cache_dirs_are_under_hares` (with `XDG_CACHE_HOME` set to a temporary
directory, the weather and ResStock cache functions return paths ending in
`hares/weather` and `hares/resstock`).

**Verification:** the census command above; `uv run --no-sync maturin
develop --release`; `uv run --no-sync pytest tests/python`.

**Commit:** `refactor: rename the Python package to hares (distribution hares-py)`

**Dependencies:** the opening sequence (`plan.md` §3); `10-slop-cleanup.md`
SL-8. Must precede OD-1, OD-7 and `08-consumer.md` CN-13, which follows it
directly (the consumer imports `hares` from CN-13 on); it is the first
engine row the unmigrated consumer cannot run, so B0 checks with the CN-0
consumer end at the row before it (`08-consumer.md` preamble).

**State:** committed `d3e2ec62` (2026-10-09). The package is `hares`, the
distribution `hares-py`, the extension module `hares._hares`; the census
over the tracked tree is clean outside the planning and review records
that leave the tree in `10-slop-cleanup.md`'s pending move (they were
still tracked when this row ran) and the acceptance test's own lines
naming the old name it pins absent. The download caches resolve under
`hares/weather` and `hares/resstock`. Gates: cargo fmt, clippy
--all-features --all-targets and the doc links unchanged (the three
broken intra-doc links predate this row), nextest --workspace
--all-features 7795/7795 (7575 in the default profile), pytest 1487
passed 0 skipped (four acceptance tests new),
pyright 0 errors, the eight golden fixtures bitwise across both
`frame_golden` feature sets, `uv lock` regenerated.

**State:** committed `04e3a481` (2026-10-09, branch `row/r34-py8`). The
differential REDs proved at the parent before the unification: the Python
constructor's `retain_batches` default true against the TOML path's false
(the kwargs path hard-coded it too, so every `from_hpxml` caller ran with
batches retained and read full `results()` frames from a default nothing
declared); the TOML path's silent acceptance of an out-of-range site
coordinate while the Python constructor rejected the same value with two
different messages of its own; the synthetic `[simulation]` table's silent
acceptance of unknown keys, an `[output]` table and an
`initialization_duration_s` key; and `Dwelling::from_config` running no
validation, so a directly constructed config with a misaligned duration
built a dwelling. The unification: `SimulationConfig::validate()` (the
five `from_toml` checks, the u32 seconds check, the site-location check
with the binding's messages) runs on every construction path;
`SimulationConfig::to_toml()` added; the REMOVED_KEYS rows landed with the
one message per key shared by the `[simulation]` table's check and the
Python keyword surfaces (pinned equal); the synthetic `[simulation]` table
deserialises as `SimulationConfig` with `initialization_duration_s` a
top-level dwelling key and the twin configs deleted, the ten bestest
fixtures migrated; the Python surface is the newtype with the keyword-only
constructor, the Rust serde default constants referenced by name in the
signature, `duration`/`time_res` taking int seconds or a timedelta, the
frozen `SiteLocationOverride` pyclass raising validate()'s message, the
`**unexpected` removed-key lookups, getters, `validate()`, `to_toml()`,
`from_toml()`; `PyDwellingConfig` wraps the Rust `DwellingConfig`; the
mirror fields, conversions, Python-side default constants and
`build_config`/`KNOWN_KWARGS` deleted, `from_hpxml` (Dwelling and
DwellingBlueprint) taking the one config plus the dwelling-level keywords,
so `duration_s=`, `time_res_s=`, the four site keywords and
`output_to_parquet` raise their removed-key messages from this row;
`SimulationPlan` builds `SimulationConfig` through the constructor, the
gym's `config=sim_config` calls unchanged; the stale `sim_config` copy on
`PyDwelling` read through the config instead. One consequence of the
unified default `retain_batches = false`, recorded here as the row's
intent: a dwelling built on the default config reads a `metrics()` that
raises ("no batches flushed yet") and a `results()` that returns the
two-column steps fallback, because no batches are retained; a caller
that wants the batch schema or `metrics()` opts in explicitly with
`retain_batches = True` on the config, as `SimulationPlan` does (its
`run()` returns `results()`, so the plan's own kwargs default it true,
overridable by an explicit caller pass). Gates: fmt, clippy
--all-features --all-targets clean, nextest --all-features 7595/7595,
pytest 1537 passed 0 failed 0 skipped, pyright 0, the eight goldens
bitwise (compare-all 7 identical, -F dst identical), no golden moved.

## PY-8: one config type across TOML, synthetic TOML and Python

**Kind:** initiative work (`dl-i08-architecture.config`).

**Problem:**

- Python mirrors the Rust config: `PySimulationConfig`
  (`crates/hares-python/src/py_config.rs:47-550`, with
  `to_sim_config`/`from_sim_config` at `:469,518`) has its own defaults
  (`DEFAULT_DURATION_S`, `DEFAULT_STEP_S` at `py_config.rs:17-18`, again
  at `py_dwelling.rs:61-62`; `write_output` true at `py_config.rs:201`,
  `retain_batches` true at `:211`, where Rust defaults it false);
  `PyDwellingConfig` (`py_config.rs:552`) mirrors the Rust
  `DwellingConfig` (`crates/hares-core/src/dwelling/mod.rs:210`); and
  `build_config` (`py_dwelling.rs:2228-2460`) hand-parses keyword
  arguments against `KNOWN_KWARGS` (`py_dwelling.rs:2142-2165`) with its
  own fallbacks.
- Synthetic TOML has its own twins: `SyntheticOutputConfig`
  (`crates/hares-core/src/dwelling/synthetic.rs:291-327`, its own
  `write_output` default) and `SyntheticSimulationConfig`
  (`synthetic.rs:125-131`: `start_time`, `time_res_s`, `duration_s`,
  `initialization_duration_s`).
- `SimulationConfig` validation runs only inside `from_toml`
  (`crates/hares-io/src/config.rs:144-185`), so a config built in Rust or
  Python code is never validated there.

**Change:**

- `SimulationConfig::validate(&self) -> Result<(), ConfigError>` holds
  every check on the fields `SimulationConfig` has at this commit:
  today's five in `from_toml`; `time_res` in `u32` seconds; and
  `site_location`, today checked only by the Python binding
  (`extract_site_location_override`, `py_dwelling.rs:2208-2265` at
  `872cf289`): each set field finite, `latitude_deg` in [-90, 90],
  `longitude_deg` in [-180, 180], `elevation_m` in [-500, 9000] and
  `utc_offset_h` in [-14, 14], with the binding's messages. The report
  and frequency checks join `validate()` with the fields they check
  (`03-output-reporting.md` OR-3 and OR-13). `from_toml` is deserialise
  then `validate()`; `Dwelling::from_config` and
  `DwellingBlueprint::from_config` call `validate()`.
- `hares_io::config::REMOVED_KEYS`, its scopes and its lookup
  `removed_key(scope, key)` exist from `12-inputs.md` IN-6, which checks
  them on the TOML tables before deserialising and on Python keyword
  arguments, so both report the same message (`plan.md` §2.7 table). This
  entry adds the rows `plan.md` §2.7 assigns to it, each with its scope:
  `output` (the synthetic `[output]` table, `DwellingTable`);
  `initialization_duration_s` (the synthetic `[simulation]` key,
  `SimulationConfig`, naming the top-level `initialization_duration_s`,
  which stays valid in the `DwellingTable` scope until
  `09-numeric-fixes.md` B-2); `duration_s` and `time_res_s`
  (`SimulationConfig`); the four site keywords `latitude`, `longitude`,
  `elevation_m` and `utc_offset_h`, and `output_to_parquet` (naming
  `SimulationConfig.output_format` until OR-3 rewrites the row), all
  `PythonKeyword`. OR-3 adds the output keys.
- Synthetic TOML: `[simulation]` deserialises as `SimulationConfig`
  (`time_res`, `duration` in seconds); `initialization_duration_s` becomes
  a top-level dwelling key; `SyntheticOutputConfig` and
  `SyntheticSimulationConfig` are deleted. Every synthetic TOML fixture is
  migrated in this commit (census:
  `git grep -ln '^\[simulation\]' -- '*.toml'`).
- Python: `#[pyclass(name = "SimulationConfig")] pub struct
  PySimulationConfig(SimulationConfig)` with a keyword-only constructor
  whose defaults are the Rust serde default constants, referenced by name
  in the `#[pyo3(signature = ...)]` (one table). The keywords are exactly
  the Rust field names, with the Python types and defaults of the
  `plan.md` §2.7 keyword table: at this commit `start_time` and `duration`
  (both required, as in Rust), `time_res`, `output_verbosity`,
  `output_path`, `write_output`, `output_format`, `retain_batches`,
  `rotation` (the output fields, until OR-3 removes them),
  `output_chunk_size`, `setpoint_deadband_c`, `master_seed`,
  `civil_timezone` and `site_location`; later fields join with the entry
  that adds them. `duration` and `time_res` take `int` seconds or a
  `timedelta` (a non-whole-second `timedelta` is a `ValueError`).
  `site_location` takes `hares.SiteLocationOverride(latitude_deg=None,
  longitude_deg=None, elevation_m=None, utc_offset_h=None)`, a frozen
  pyclass over `hares_io::SiteLocationOverride`
  (`crates/hares-io/src/site_location.rs:134`) whose constructor calls the
  same Rust site-location check `validate()` runs (above) and raises its
  message as `ValueError`; the Python copy of the checks is deleted.
  The constructor's signature ends in `**unexpected`: pyo3 raises its own
  `TypeError` for an unknown keyword before the constructor body runs, so
  without it a removed keyword (`duration_s=`, `latitude=`) could never
  name its replacement. A keyword in `unexpected` is looked up in the
  `SimulationConfig` and `PythonKeyword` scopes and raises the removed-key
  message; any other is a `TypeError` naming it, as Python's own is.
  Getters for every field, `validate()`, `to_toml()`, `from_toml()`.
  `to_toml()` wraps the Rust `SimulationConfig::to_toml(&self) ->
  Result<String, ConfigError>`, which this entry adds (`from_toml`'s
  inverse, `plan.md` §2.7), so Rust tests round-trip a config the same
  way (`03-output-reporting.md` OR-12 and OR-14 use it).
  `PyDwellingConfig` wraps the Rust `DwellingConfig` the same way. The
  mirror fields, conversions, Python-side default constants and
  `build_config`/`KNOWN_KWARGS` are deleted.
- The gym needs no conversion here: `11-other-defects.md` OD-7 (row 29)
  has already deleted its `SimulationConfig`-to-keyword mapping, and the
  gym passes `config=sim_config` to `from_hpxml` unchanged.
  `SimulationPlan` (`python/hares/simulation_plan.py:46-58,100-106`)
  builds `SimulationConfig` through the constructor, passing every field
  the caller set, so a plan can configure recording like any other
  caller.

**Acceptance tests:**

- `python_and_toml_defaults_come_from_one_table`: a config built in
  Python with only the required arguments, `to_toml()`, equals
  `SimulationConfig::from_toml` of the same two required keys, field for
  field.
- `synthetic_toml_rejects_unknown_key`,
  `synthetic_output_table_names_reports`.
- `validate_runs_for_rust_built_config`: `Dwelling::from_config` with a
  directly constructed config whose `duration` is not a multiple of
  `time_res` fails with the same message `from_toml` gives.
- `removed_key_messages_equal_across_toml_and_python`, parametrized over
  this entry's rows present on both surfaces (`duration_s` and
  `time_res_s`: a synthetic `[simulation]` key and a
  `SimulationConfig(...)` keyword give the same message).
- `python_removed_keyword_names_replacement`, parametrized over the
  Python-only rows (the four site keywords and `output_to_parquet`) on
  `SimulationConfig(...)`; PY-2's `removed_keyword_names_replacement`
  covers the dwelling constructors.
- `synthetic_simulation_initialization_duration_names_toplevel`: a
  synthetic TOML file with `initialization_duration_s` under
  `[simulation]` fails naming the top-level key;
  `toplevel_initialization_duration_is_accepted`: the same key at the top
  level (`tests/fixtures/bestest/600.toml` after its migration) is
  accepted.
- `python_keyword_names_equal_rust_field_names`: the keyword names of
  `inspect.signature(hares.SimulationConfig)` (pyo3 generates the
  constructor's `__text_signature__`), skipping the `VAR_KEYWORD`
  parameter `unexpected`, equal the keys of
  `SimulationConfig(start_time=..., duration=...).to_toml()` parsed back
  with `tomllib` after every optional field is set to a non-default value;
  the test holds no list of field names.
- `site_location_override_round_trips`: `SiteLocationOverride(latitude_deg=40.0)`
  through `SimulationConfig`, `to_toml()` and `from_toml()` is equal, and
  `latitude_deg=95.0` is a `ValueError` naming the field.
- `toml_site_location_out_of_range_is_a_validate_error`: a TOML config
  with `site_location.latitude_deg = 95.0`, and a Rust-built config with
  the same value, fail `validate()` with the message the Python
  constructor raises.

**Verification:** `cargo nextest run -p hares-io -p hares-core config`;
`uv run --no-sync pytest tests/python/test_py_config.py`.

**Commit:** `refactor: one SimulationConfig type for TOML, synthetic TOML and Python`

**Dependencies:** PY-7; `11-other-defects.md` OD-2 (adds
`deny_unknown_fields` to `SimulationConfig` and removes the
`time_res_secs_u32` fallback); `12-inputs.md` IN-6 (creates
`REMOVED_KEYS`, `KeyScope` and `removed_key`, to which this entry adds
rows).

## PY-2: `from_hpxml` takes one typed config

**Kind:** initiative work (V20).

**Problem:** `Dwelling.from_hpxml(config=..., **kwargs)` silently lets
`config=` win over sibling keywords (`py_dwelling.rs:2246-2258`), and the
keyword path builds a second config with its own defaults
(`py_dwelling.rs:2260-2345`) and a hard-coded `retain_batches: true`
(`py_dwelling.rs:2408`). Unknown keywords raise `ValueError`
(`py_dwelling.rs:2233-2240`), not Python's `TypeError`.

**Change:** `Dwelling.from_hpxml(hpxml, schedule, weather, *, config,
bldg_id=0, initialization_duration=None, defaults=None, overrides=None,
**unexpected)` (the dwelling-level keywords of today's `KNOWN_KWARGS`, less
`resample_overrides`, which `12-inputs.md` IN-6 has already removed;
`initialization_duration` becomes `SimulationConfig.warmup` in
`09-numeric-fixes.md` B-2, and `defaults=None` means the defaults shipped in
the package from B-10) and the same signature on
`DwellingBlueprint.from_hpxml`; `DwellingConfig(hpxml, schedule, weather,
*, config, bldg_id=0, initialization_duration=None, defaults=None,
overrides=None)` takes the same names. `config` is a `SimulationConfig`
and is required. `**unexpected` exists only to name the right error, in
this order: a key in `REMOVED_KEYS` under the `DwellingTable`,
`SimulationConfig` or `PythonKeyword` scope (`plan.md` §2.7) raises the
removed-key message naming its replacement; a current `SimulationConfig` field name (for example
`master_seed=`) is a `TypeError` naming
`config=SimulationConfig(master_seed=...)`; any other keyword is a
`TypeError` naming it, as Python's own is. The HPXML, schedule and weather
arguments accept a path or content, and `defaults` a path
(`06-runtime-memory.md` RT-4 adds content sources and the parsed handles
of `plan.md` §2.7, which these arguments then also accept).

Disposition of every `KNOWN_KWARGS` entry at HEAD `872cf289`
(`py_dwelling.rs:2183-2206`, 22 entries):

| Keyword | After this entry | Error for the old keyword names |
|---|---|---|
| `config` | kept, required | |
| `bldg_id`, `overrides` | kept, dwelling keyword | |
| `initialization_duration` | kept until `09-numeric-fixes.md` B-2 | from B-2: `SimulationConfig.warmup` (`REMOVED_KEYS`) |
| `defaults_path` | renamed `defaults` | `defaults` (`REMOVED_KEYS` row, scope `PythonKeyword`, added here) |
| `resample_overrides` | removed by `12-inputs.md` IN-6 | its `REMOVED_KEYS` row |
| `start_time`, `output_chunk_size`, `master_seed`, `civil_timezone`, `setpoint_deadband_c` | `SimulationConfig` field | `TypeError` naming `config=SimulationConfig(<name>=...)` |
| `duration_s`, `time_res_s` | `SimulationConfig.duration`, `.time_res` | `REMOVED_KEYS` rows (PY-8) |
| `latitude`, `longitude`, `elevation_m`, `utc_offset_h` | `SimulationConfig.site_location` | `REMOVED_KEYS` rows (PY-8) |
| `write_output`, `output_path`, `output_verbosity`, `rotation` | `SimulationConfig` fields until `03-output-reporting.md` OR-3, then `SimulationConfig.reports` | `TypeError` naming `config=SimulationConfig(<name>=...)`; from OR-3, their `REMOVED_KEYS` rows |
| `output_to_parquet` | removed (PY-8) | its `REMOVED_KEYS` row: `SimulationConfig.output_format`, then from OR-3 `SimulationConfig.reports` |

**Acceptance tests:** `sim_keyword_next_to_config_is_type_error`
(parametrized over the five current-field keywords of the table);
`unknown_keyword_is_type_error`;
`removed_keyword_names_replacement`, parametrized over every removed or
renamed keyword of the table at the entry's commit (`defaults_path`,
`resample_overrides`, `duration_s`, `time_res_s`, the four site keywords,
`output_to_parquet`),
each on `Dwelling.from_hpxml`, `DwellingBlueprint.from_hpxml` and
`DwellingConfig`; OR-3 and B-2 extend its parameters with their rows
(B-2 completes the `initialization_duration` row: from its commit the
keyword is removed and names `SimulationConfig.warmup`).
`09-numeric-fixes.md` B-9a adds the `hpxml_dir` row (Python
`Fleet.from_resstock`, naming `buildings_dir`) and B-14 its SI rows
(equipment config keys, PV-sizing keywords, actor parameters,
control-signal fields and observation names, `plan.md` §2.7), each with
its own test on the surface that row belongs to.

**Verification:** `uv run --no-sync pytest tests/python`.

**State:** committed `9904a698` (2026-10-09, branch `row/r35-py2`, cut
from `b17ed658` with PY-8 not yet landed, so this entry ran ahead of its
dependency and the row 34 surface it assumes is still to come). The three
constructors take one typed config: `Dwelling.from_hpxml`,
`DwellingBlueprint.from_hpxml` and `DwellingConfig` share the signature
`*, config` (required, a `SimulationConfig`), `bldg_id=0`,
`initialization_duration=None`, `defaults=None` (the renamed
`defaults_path`), `overrides=None`, `**unexpected`. The kwargs path's
second config, its own defaults and the hard-coded `retain_batches: true`
are deleted with `KNOWN_KWARGS` and the site-keyword extractor;
`config.to_sim_config()` is the one config build. `**unexpected` names
the right error through one shared helper (`utils.rs`
`check_unexpected_kwargs`/`unexpected_keyword_error`): a removed-key
lookup in the `DwellingTable`, `SimulationConfig` then `PythonKeyword`
scope raises the table's replacement-naming message, a current
`SimulationConfig` field name raises `TypeError` naming
`config=SimulationConfig(<name>=...)`, anything else raises `TypeError`
naming it, as Python's own is; all three constructors raise the same
class (the old surface's `ValueError` on `from_hpxml` was the entry's
named problem). The entry's `PythonKeyword` rows landed here (not with
PY-8, whose `SimulationConfig`-scope TOML rows wait for its fixture
migration): `defaults_path` naming `defaults`; `duration_s` and
`time_res_s` naming `SimulationConfig.duration` and `.time_res`; the
four site keywords naming `SimulationConfig.site_location`;
`output_to_parquet` naming `SimulationConfig.output_format`; and
`hpxml_dir` naming `buildings_dir`, the pin `12-inputs.md` B-9a deferred
to this machinery, landed now (`Fleet.from_resstock` gained
`**unexpected` and its test asserts the replacement). The gym wrapper is
consistent: `GymDwellingConfig.defaults_path` renamed `defaults`, the
gym passes `config=` on every construction (the engine's defaults when
the plan sets none), and `Dwelling.config` became a getter (the method
call sites migrated; the stub declares the property).
`SimulationPlan` builds each segment's `SimulationConfig` and keeps the
dwelling-level keywords beside it. RED: the rejection suite failed on
the base (the five sim keywords beside `config=` succeeded, the removed
keywords raised no replacement-naming error, the unknown keyword raised
`ValueError`, the differential failed on the then-unknown `defaults=`);
GREEN: `sim_keyword_next_to_config_is_type_error` (five),
`unknown_keyword_is_type_error`, `removed_keyword_names_replacement`
(nine keys, all three constructors), the fleet pin, and the differential
pinning every old kwarg's value read back off `dw.config` (35 tests).
Every caller migrated: about 30 test files, conftest's `make_dwelling`
splitting its kwargs at the helper, the notebook, the docs' example.
Gates: goldens bitwise both feature sets (7 manifests plus
`full_year_900s_dst`, no movement, no re-capture), nextest
--all-features 7585/7585, pytest 1556 passed / 0 failed / 0 skipped,
pyright 0 errors, fmt and clippy clean. One environmental note for the
next runner: the ResStock ctrl_c resilience test hangs without the
`resstock` extra's httpx (the child's stall is never cut and the urllib
fallback blocks the join), green with every extra synced.

**Commit:** `fix: from_hpxml takes one typed SimulationConfig`

**Dependencies:** PY-8; `12-inputs.md` IN-6.

## PY-1: zero-copy Python export via `pyo3-arrow`

**Kind:** initiative work (`dl-i08-architecture.python_export`).

**Problem:** `simulate()` and `results()` clone the batch vector
(`py_dwelling.rs:797-798,1539-1540`) and convert it through Arrow IPC bytes
and `polars.read_ipc` (`record_batches_to_polars_df`,
`crates/hares-python/src/conversions.rs:52-111`); the fleet aggregate goes
the same way (`py_fleet.rs:303`). `batches_or_steps_to_polars_df` and
`steps_to_polars_df` (`conversions.rs:20-49`) are the two-column fallback,
and `record_batch_arc_to_polars_df` (`conversions.rs:158-163`) has no
caller. `metrics()` (`py_dwelling.rs:1545-1577`) clones batches but does
not use IPC.

**Change:** add `pyo3-arrow = "0.19"` to `crates/hares-python/Cargo.toml`
(it requires the arrow 59, pyo3 0.29, numpy 0.29 set `01-baseline-repair.md`
BR-6b installs; it brings `thiserror` 1 beside the workspace's `thiserror`
2, which `[advisories]` does not check, since the block runs cargo-deny's
advisory check only). `ReportResult` exports (PY-9):

- `arrow(rechunk: bool = True)` returns pyo3-arrow's `PyTable` class (a
  `#[pyclass(module = "arro3.core._core", name = "Table")]`,
  `pyo3-arrow-0.19.0/src/table.rs:36`; it works without `arro3`
  installed; there is no `pyo3_arrow` Python module) over the report's
  batches. With `rechunk=True` it holds one batch, built in Rust by
  `arrow::compute::concat_batches` (`hares-python` depends on `arrow`,
  which re-exports `arrow-select`'s `concat_batches`,
  `arrow-select-59.3.0/src/concat.rs:613`) once per report and kept in a
  `OnceLock<RecordBatch>` field of the `hares-python` `ReportResult`
  pyclass (the Rust `ReportResult` of `plan.md` §2.1 is unchanged), so
  later calls share that batch's buffers and copy nothing. A `PyTable`
  exposes a repeatable `__arrow_c_stream__`. `_hares.pyi` annotates the
  return as `ArrowStreamExportable`, a `typing.Protocol` declaring
  `__arrow_c_stream__(self, requested_schema: object | None = None) -> object`.
- `table(rechunk: bool = True) -> polars.DataFrame`:
  `polars.DataFrame(self.arrow(rechunk))`.
- A report whose output is a file or a callback holds no batches: its
  `arrow()` and `table()` raise `HaresSimulationError` naming the report
  and its destination (the file's `path`, or `batches_delivered` for a
  callback), never an empty table.

The three `FleetResult` tables (`04-fleet.md` FL-1: `aggregate`,
`covered_weight` and `homes`) use the same export: each is an attribute
with `arrow(rechunk=True)` and `table(rechunk=True)` as above.
`record_batches_to_polars_df`,
`steps_to_polars_df`, `batches_or_steps_to_polars_df` and
`record_batch_arc_to_polars_df` are deleted.

**Acceptance tests:**
`capsule_export_is_bitwise_identical` (a `hares-python` Rust test): for the
`full_year_hourly` manifest's run, the table exported through
`__arrow_c_stream__` and imported back through the `pyo3-arrow` crate's
`PyTable::from_arrow_pycapsule` equals the
report's `RecordBatch`es column by column, floats by `to_bits()`.
`table_matches_arrow_export` (Python): `table()` equals
`polars.DataFrame(arrow())` and has the report's schema.
`report_can_be_exported_twice`.
`rechunk_false_keeps_batch_boundaries`: `arrow(rechunk=False)` has one
chunk per built batch.
`second_export_does_not_recopy` (a `hares-python` Rust test): two
`arrow()` calls on one `ReportResult` yield tables whose column buffers
have equal data pointers.
`file_report_export_raises_naming_destination`: a `Parquet` report's
`arrow()` and `table()` raise `HaresSimulationError` naming its path.
`fleet_tables_export_through_capsule`: on the `fleet_same_offset_300s`
homes (`04-fleet.md` FL-1) at `Hourly`, `aggregate`, `covered_weight` and
`homes` each export through `__arrow_c_stream__` and equal, imported back,
the `FleetResult`'s batches, floats by `to_bits()`.

**Verification:** `cargo nextest run -p hares-python`;
`uv run --no-sync pytest tests/python`.

**Commit:** `feat: export result tables through the Arrow PyCapsule interface`

**Dependencies:** `01-baseline-repair.md` BR-6b; OR-6; PY-9 (the
`ReportResult` class these exports attach to); `04-fleet.md` FL-1 (the
`FleetResult` tables).

**State:** committed `b8037120` (branch `row/r55-py1`, cut from
`5a9c4ac8`, one commit; the Change paragraph's stale line numbers into
`conversions.rs` reflect the file before rows 43 to 50: only
`record_batches_to_polars_df` still existed to delete, the other three
conversions died with the surfaces they served). `pyo3-arrow = "0.19"`
in `crates/hares-python/Cargo.toml` (arrow 59.3, pyo3 0.29.3, numpy
0.29 resolve; the predicted `thiserror` 1.0.69 arrives beside the
workspace's `thiserror` 2). `ReportResult.arrow(rechunk=True)` returns
pyo3-arrow's `PyTable` (the class named in the entry, module
`arro3.core._core`); with `rechunk=True` the report's batches are
concatenated once (`arrow::compute::concat_batches`) on the first
export and the `OnceLock<RecordBatch>` field of the pyclass hands every
later export the same buffers (`second_export_does_not_recopy` pins
equal column-buffer data pointers); with `rechunk=False` the stream
carries one chunk per built batch. `ReportResult.table(rechunk=True)`
is `polars.DataFrame(self.arrow(rechunk))` exactly as the entry says.
A file or callback report's `arrow()` and `table()` raise
`HaresSimulationError` naming the report and its destination (the
file's `path`, the callback's `batches_delivered` count). The three
`FleetResult` tables are attributes returning the new frozen `FleetTable`
pyclass over the table's batch, with the same `arrow(rechunk=True)` and
`table(rechunk=True)`; the fleet tables are one batch, so `rechunk` is
inert there, stated in the method docs. The batch callback's delivery
frame is the capsule read (the per-batch IPC cost's death, measured in
`02-measurement.md`). `_hares.pyi` annotates both `arrow()` returns as
`ArrowStreamExportable`, the `typing.Protocol` the entry names, defined
in `python/hares/_hares_types.py` beside the other declarations the
compiled module cannot carry (the stub's own-test imports it, so the
module check sees no unknown name), and `FleetTable` is stubbed with
the `FleetResult` getters' new type; the docs' changes section carries
the row. The acceptance tests: the seven named ones, four in
`tests/python/test_py_results.py` (table matches the arrow export,
exported twice, the rechunk boundaries over 40/40/16, the parquet
report's refusal naming its path) and three in Rust (the capsule
round-trip bitwise over the `full_year_hourly` run, the shared buffer
pointers, the fleet's three tables round-tripped bitwise against their
batches on the `fleet_same_offset_300s` homes at `Hourly`); RED first
(the Python four failed with AttributeError on the missing `arrow()`,
the Rust ones with 14 compile errors on the missing surface). The
metrics-from-table binding is not here: the entry's own note (and
OR-15's) gives it to PY-4, the first row with both the `Catalogue`
pyclass and the stream import. Gates: fmt; clippy `--workspace
--all-targets --all-features -D warnings` clean; nextest `--workspace
--all-features -j 4` 7952/7952 (1 skipped: the ignored kind-pin
generator); pytest `-m ''` 1678 passed, 6 xfailed, 0 failed, 0
skipped; pyright 0; the eight goldens bitwise (compare-all 8
identical, `full_year_900s_dst` identical). The measurement (the
entry's Measurement section): the `full_year_hourly` frame read 0.318
ms over IPC to 0.056 ms capsule rechunked and 0.047 not rechunked, the
`arrow()` export itself 0.0024 ms first call and 0.0005 cached; the
callback delivery 0.129 to 0.057 ms per batch end to end over a
building-year at 876 batches; in `02-measurement.md` with the method
and the host's noise bound.

**Measurement:** conversion time of a full-year frame, IPC versus capsule,
rechunked and not, in `02-measurement.md` (`context.md`: about 6.5 ms
IPC, about 2.7 ms capsule rechunked).

## PY-3: construction releases the GIL

**Kind:** initiative work (V32).

**Problem:** construction holds the GIL throughout: `from_hpxml`
(`py_dwelling.rs:634-656`, the `Dwelling::from_config` call at `:643`),
`reset_with_seed` (`py_dwelling.rs:1577-1590`, `:1581`),
`DwellingBlueprint.from_hpxml` (`crates/hares-python/src/py_blueprint.rs:37`)
and `DwellingBlueprint.build` (`py_blueprint.rs:102-115`), unlike
`simulate`/`step`, which detach (`py_dwelling.rs:708,764`). Threads that
construct dwellings concurrently serialise on the GIL although parsing
touches no Python state.

**Change:** every one of those four Rust calls runs through one function,
`construct_detached<T: Send>(py: Python<'_>, f: impl FnOnce() -> T + Send)
-> T`, which calls `py.detach(f)`. Argument extraction from Python objects
stays attached, before the call. `initialize()` is not touched (after
OR-9 it only sets a flag).

**Acceptance tests:** `construction_runs_detached`: a `#[cfg(test)]` hook
inside `construct_detached` records `pyo3::ffi::PyGILState_Check()`; a
Rust test in `hares-python` drives each of the four entry points (on the
`cz4a_ashp_hpwh` inputs, 1 h at 900 s) and
asserts the recorded value is 0 each time. No threads, no timing.

**Verification:** `cargo nextest run -p hares-python`;
`uv run --no-sync pytest tests/python`.

**Commit:** `perf: release the GIL while constructing dwellings`

**Dependencies:** OR-9; PY-2.

## PY-4: the catalogue in Python

**Kind:** initiative work (V33).

**Problem:** polars drops Arrow field metadata on import (reproduced on
polars 1.44.2 through `read_ipc` and the `DataFrame(table)` constructor),
so the `unit` metadata `field_with_unit` attaches
(`crates/hares-io/src/output/columns.rs:43-53`) never reaches a Python
user; mode codes have no decode except the `hares_mode_map` schema
metadata, which is dropped the same way.

**Change:** `Catalogue` is exposed as a frozen pyclass with
`variables() -> list[Variable]`, `__getitem__(name) -> Variable`,
`to_json()`, `from_json()`. `Variable` is a frozen pyclass with `name`,
`unit` (the `Unit::symbol()` string), `kind`, `aggregation`, `sampling`,
`nullable`, `source`, `labels` (for `Enum`, else `None`),
`across_homes` (`"sum"` or `"mean"` for `Quantity`, else `None`) and
`description` (`VariableDef.description`, `plan.md` §2.2). Available as
`Dwelling.catalogue()` (the Rust `Dwelling::catalogue()` of
`03-output-reporting.md` OR-1, readable before the first step) and
`RunResult.catalogue`; `12-inputs.md` IN-2 adds
`DwellingBlueprint.catalogue()` with the Rust method, once assembly runs
without warm-up.

The Python binding of `03-output-reporting.md` OR-15 lands here, the
first row with both the `Catalogue` pyclass and the `pyo3-arrow` stream
import (PY-1): `hares.metrics_from_table(table, catalogue, time_res,
setpoint_deadband_c=None) -> RunMetrics`, where `table` is any object with
`__arrow_c_stream__` (imported with `PyTable::from_arrow_pycapsule`) and
`time_res` is `int` seconds or `timedelta`, with its `_hares.pyi` stub and
its line in the `docs/python.md` changes section (PY-9).

**Acceptance tests** (the `cz4a_ashp_hpwh` inputs, 24 h at 900 s):
`catalogue_unit_lookup_works_after_polars_import`:
every column of a `Verbosity(8)` table has a `Variable` with a unit; no
frame metadata is read. `enum_codes_decode_to_labels`.
`catalogue_available_before_first_step`: `Dwelling.catalogue()` on a
dwelling built without warm-up and not yet stepped equals
`RunResult.catalogue` of the same dwelling after one step.
`variable_descriptions_come_from_declarations`: the battery's
`DECLARED_CAPACITY_KWH` variable carries its declared description.
`quantity_variables_carry_across_homes`: every `Variable` whose `kind` is
`"quantity"` has `across_homes` `"sum"` or `"mean"`, and every other has
`None`.
`metrics_from_table_matches_run_metrics_in_python`: with `metrics=True`
and a `Verbosity(8)` `Memory` report, `hares.metrics_from_table` over the
report's `arrow()` table and `RunResult.catalogue` equals
`RunResult.metrics` field for field.

**Verification:** `cargo nextest run -p hares-python`;
`uv run --no-sync pytest tests/python`.

**Commit:** `feat: expose the variable catalogue and metrics_from_table in Python`

**Dependencies:** OR-1 (`Dwelling::catalogue()`), PY-9, PY-1
(`pyo3-arrow` and `arrow()`), `03-output-reporting.md` OR-15 (the Rust
`metrics_from_table`).

## PY-5: one reward contract on the Rust and Python paths

**Kind:** initiative work (V15; RL-5).

**Problem:** `VecDwellingGymEnv` requires a per-environment
`reward_fn: Callable[[RewardContext], float]`
(`python/ochre_next/rl/vec_env.py:47,119`; `RewardContext` at
`python/ochre_next/rl/gym_env.py:88-92`). The pure-Python path calls it
per environment (`vec_env.py:265-288`). The Rust `batch_step` path
ignores it: `batch_step` computes `reward = -net_electric_power_kw`
(`crates/hares-python/src/py_gym.rs:494`) and `vec_env.py:236` reads that
value. The same environment gives different rewards depending on whether
the extension exposes `batch_step`.

**Change:** delete the Rust reward and the `reward` key from each
`batch_step` row; add `step`, the same dict `PyDwelling.step()` returns
(one shared conversion function). In `vec_env.py`, one helper
`_reward_context(step_data, dwelling) -> RewardContext` builds the context
from the step dict and `dwelling.telemetry()`, and both branches call
`self._reward_fn(ctx)` per environment. The per-environment signature is
unchanged. `reward_fn` remains the seam for a future built-in reward
(`plan.md` §2.10).

**Acceptance tests:** `rust_and_python_paths_give_identical_rewards`: two
environments built from the `tests/python/conftest.py` inputs
(`tests/fixtures/hpxml/ochre_samples/base.xml`,
`data/examples/BEopt_example_schedule.csv`,
`data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw`, start
`2019-01-01T00:00:00`, 60 s steps, seeds 0 and 1), over 20 steps, with a
fixed action sequence and a reward function that reads the zone
telemetry and `total_power_kw`, once with `batch_step` and once with it
forced off; rewards are equal per environment and step, exactly.
`batch_step_rows_carry_no_reward_key`.

**Verification:** `cargo nextest run -p hares-python`;
`uv run --no-sync pytest tests/python/test_gym_batch_step.py tests/python/test_gym_env.py`.

**Commit:** `fix: batch_step and the Python path call the same reward_fn`

**Dependencies:** PY-7.

## PY-6: build and provenance identity

**Kind:** initiative work (V18; R14).

**Problem:** nothing in HARES reports which build produced a result: no
`__version__`, no `build_info`, and `Cargo.toml`'s `0.1.0` never changes.
The consumer hashes the compiled `.so` instead
(`calibration/paths.py:97-109` in the consumer at `c71c542`).

**Change:**

- `crates/hares-core/build.rs` (new) records `BuildInfo` (`plan.md`
  §2.1). Commit: read from `.git/HEAD` and the ref it names (following a
  `gitdir:` file for worktrees), without running git. Dirty: the output
  of `git status --porcelain --untracked-files=no`, parsed by a pure
  function; `None` when git or `.git` is absent, never a guess. Profile
  from `PROFILE`, features from `CARGO_FEATURE_*`, rustc from
  `$RUSTC -V`. `cargo:rerun-if-changed` paths are relative to the package
  (`crates/hares-core`), so `build.rs` resolves each from
  `CARGO_MANIFEST_DIR` to the workspace root (the git directory's `HEAD`,
  the resolved ref file, the git directory's `index`, `../../crates`,
  `../../Cargo.toml`, `../../Cargo.lock`) and emits only paths that
  exist, because a missing path makes cargo rerun the script on every
  build. The git directory is resolved once, as for `HEAD`: `../../.git`
  when it is a directory, and the directory its `gitdir:` line names when
  `.git` is a file (a worktree, as every engine-under-test checkout under
  `.workstrees/<sha>` is), so `HEAD` and `index` are that worktree's and
  `dirty` follows edits outside `crates/` there too. The parsing lives in `crates/hares-core/build_support/git.rs`,
  included by `build.rs` through `#[path]` and by a unit-test target.
- `Provenance.build`, and `Provenance.catalogue_digest` copied from
  `Catalogue::digest()` (defined in `plan.md` §2.2 and added by
  `03-output-reporting.md` OR-7, which also writes it into the schema
  metadata).
- Python: the `BuildInfo` frozen pyclass (one getter per field), returned
  by `hares.build_info()`, and the `Provenance.build` and
  `Provenance.catalogue_digest` (hex string) getters on PY-9's
  `Provenance` pyclass, with their `_hares.pyi` stubs and their lines in
  the `docs/python.md` changes section; `hares.__version__` is
  `CARGO_PKG_VERSION`, equal to the pyproject version.

**Acceptance tests:** `head_commit_resolution` against synthetic `.git`
directories written as plain files in a temporary directory (detached
HEAD, symbolic ref, packed ref, worktree `gitdir:` file, no `.git`); no
git process is started. `rerun_paths_follow_the_worktree_gitdir`: for the
worktree case, the rerun paths the parsing function returns name the
`gitdir:` directory's `HEAD` and `index`, not `.git/index`. `porcelain_parse_reports_dirty`.
`build_info_commit_matches_source_tree`: `BuildInfo.commit` equals the
file-based resolution of the workspace root at test time.
`catalogue_digest_changes_when_a_variable_is_added`,
`catalogue_digest_equal_for_two_builds_of_one_blueprint` (both on the
`cz4a_ashp_hpwh` inputs; the first adds a test actor declaring one
variable), and `provenance_digest_equals_schema_metadata`.
`python_build_info_equals_run_provenance` (Python): `hares.build_info()`
equals `RunResult.provenance.build` of a run, field for field.

**Verification:** `cargo nextest run -p hares-core build_support
provenance`; `uv run --no-sync pytest tests/python`.

**Commit:** `feat: record build identity and a catalogue digest in every result`

**Dependencies:** OR-1, OR-6, OR-7 (`Catalogue::digest()`), PY-9 (the
`Provenance` pyclass).

## PY-9: `RunResult`, `finish()`, `health()` and reports in Python

**Kind:** initiative work (`dl-i08-architecture.results`; R3, X-3, RS-1).

**Problem:** the Rust lifecycle of `plan.md` §2.1 has no Python surface.
Python today exposes `simulate()` returning a frame, `results()`
(`py_dwelling.rs:1537`), `metrics()` (`:1545`, which raises with no
flushed batches), `finalize_billing()` (`:1882`), and no way to read
health, request reports or finish a stepped run. `SimulationPlan`
(`python/ochre_next/simulation_plan.py:131`) reads `dw.results()`; the gym
and HELICS wrappers (`python/ochre_next/helics/dwelling.py`) never finish
a run.

**Change:**

- Frozen pyclasses over the Rust types: `RunResult`, `ReportResult`
  (`table() -> polars.DataFrame`, `path`, `batches_delivered`, `rows`,
  `frequency`; PY-1 adds `arrow()` and the capsule path; `table()` on a
  file or callback report raises `HaresSimulationError` naming the report
  and its destination), `RunMetrics`, `RunHealth`, `WarmupOutcome`,
  `Warning` and `Provenance`. `Provenance` exposes the fields OR-6 gives
  it (`config`, `bldg_id`, `site_utc_offset_s`, `equipment`); each entry
  that adds a `Provenance` field adds its getter in its own commit
  (PY-6 `build` and `catalogue_digest` with the `BuildInfo` pyclass,
  `12-inputs.md` IN-1 `inputs`, OR-16 `tariff`). `RunResult.metrics` is
  `Some` exactly when the run has at least one report until
  `03-output-reporting.md` OR-8 (metrics are still computed from flushed
  batches), and exactly when `metrics=True` from OR-8 on (`plan.md`
  §2.2). The report, selection, frequency, destination and
  policy constructors are not this entry's: they land with the Rust entry
  that adds each type (OR-3, OR-4, OR-12, OR-13; PY-10 for
  `Destination.callback()`).
- `Dwelling.simulate() -> RunResult`, `Dwelling.finish() -> RunResult`,
  `Dwelling.health() -> RunHealth` (live, readable at any step, needing no
  report).
- `results()`, `metrics()` and `finalize_billing()` are deleted, and with
  them the interim frame return of OR-3 and OR-6. `finish()` closes the
  final billing period, so `Dwelling.billing_summaries()` read after
  `simulate()` or `finish()` holds every period; it stays, and so does the
  `Dwelling.dwelling_fatal` property (the binding's poisoned flag,
  `py_dwelling.rs:999-1002`). `08-consumer.md` CN-1 reads
  both.
- `SimulationPlan`, `DwellingGymEnv`, `VecDwellingGymEnv` and the HELICS
  dwelling wrapper call `finish()` at the end of a run or episode and read
  `RunResult`.
- `python/hares/_hares.pyi` covers every new class and method, and
  `tests/python/test_py_stubs.py` asserts that every public name of
  `hares._hares` is in the stub file and every stub name exists.
- `docs/python.md` gains two sections. "Results" documents the lifecycle
  (`initialize()`, required once before the first `step()` or
  `simulate()`, which raise `RuntimeError` without it, and cleared by
  `reset_with_seed`; `simulate()`, stepping and `finish()`; `health()`),
  requesting reports
  (selection, frequency, destination), the Arrow type of each column (as
  the report's schema carries it, with units and enum labels read from the
  catalogue, since polars drops field metadata), and that `SteppableFleet` stepping
  does not rewind (`plan.md` §2.10); its examples run as
  `test_docs_results_examples`. "Changes in this release" lists every
  callable, class and keyword added to, changed in or removed from
  `hares._hares` since `ebdb1a17`, each removed or renamed one with its
  replacement. Every later entry that changes the stub updates this
  section in its own commit. Its "Numeric changes" list starts empty;
  every entry that moves a number (`plan.md` §2.8) adds one line in its
  own commit naming what moved, why, and the golden fixtures it
  re-captured (by their manifest names under `tests/fixtures/golden/`).
- `tests/python/fixtures/_hares_ebdb1a17.pyi` (new) is the stub file as it
  was at `ebdb1a17` (`python/ochre_next/_hares.pyi`), committed as the
  baseline the changes section is checked against, so the test needs no
  git.

**Acceptance tests** (the `cz4a_ashp_hpwh` inputs, start
`2023-01-01T00:00:00-07:00`, 24 h at 900 s, one `Memory` report at
`Selection.verbosity(2)`, `output_chunk_size=40`):
`python_stepped_finish_equals_simulate` (96 steps, not a multiple of 40;
reports, metrics and health equal);
`health_readable_with_no_reports_requested` (`reports=[]`: `health()`
after 10 steps reads zero counters and `warmup` `Disabled`);
`health_counts_clamped_gym_actions`: a `DwellingGymEnv` and a
`VecDwellingGymEnv` of one environment over the same inputs, each given
one action whose first value is above its `field_bounds` upper bound;
`health().clamped_actions` reads 1 after that step on both paths, and 0
after a step whose action is inside the bounds (OD-18 counts it in every
build);
`billing_summaries_survive_finish`: the same inputs with an attached
tariff, run to the end: `billing_summaries()` after `simulate()` holds the
run's period, and `dwelling_fatal` reads `False`;
`simulation_plan_reads_run_result`; `stubs_cover_every_binding`;
`stub_changes_are_listed` (every name added to or removed from
`_hares.pyi` against the committed `ebdb1a17` stub, compared by `ast`,
appears in the "Changes in this release" section, and every name the
section lists as added or removed is such a name);
`numeric_changes_name_existing_goldens` (every golden fixture a "Numeric
changes" line names has a manifest under `tests/fixtures/golden/`);
`file_report_table_raises_naming_destination`;
`test_docs_results_examples`.

**Verification:** `cargo nextest run -p hares-python`;
`uv run --no-sync pytest tests/python`.

**Commit:** `feat: expose RunResult, finish() and health() in Python`

**Dependencies:** OR-6 (schedule: the next row), OR-12;
`11-other-defects.md` OD-18 (row 30b, the clamped-action count).

**State:** committed `46455aef` (branch `row/r43-or6-py9`, directly
after OR-6, one commit). The surface as built: frozen pyclasses over
the Rust types, registered on the module, exported from `hares`,
stubbed in `_hares.pyi`: `RunResult` (`reports`, `metrics`, `health`,
`warnings`, `warnings_dropped`, `provenance`), `ReportResult` (`name`,
`table()` over the retained batches through the existing IPC
conversion, `path`, `batches_delivered`, `rows`, `frequency`; `table()`
on a file or callback report raises `HaresSimulationError` naming the
report and its destination, never an empty table), `RunMetrics`
(`SimulationMetrics` renamed, over the flattened `RunMetrics`; equal
compares by value), `RunHealth` (the six counters plus `warmup`), the
`WarmupOutcome` and `WarmupResiduals` pyclasses, `Warning`
(`step_index`, `source`, `message`), `Provenance` (`config`,
`bldg_id`, `site_utc_offset_s`, `equipment`) and
`EquipmentProvenance` (`name`, `zone`, `zone_type`, `ocv_source`,
`notes`). `Dwelling.simulate()` and `Dwelling.finish()` return the
`RunResult` (`finish()` idempotent, the run's health live through
`Dwelling.health()`, needing no report); `results()`, `metrics()` and
`finalize_billing()` are deleted with the interim frame return, and
`billing_summaries()` read after `simulate()` or `finish()` holds every
period (`billing_summaries_survive_finish`), `dwelling_fatal` staying.
The policy constructors are not this entry's (they land with OR-12's
row).

The wrappers finish their runs: `SimulationPlan` calls `finish()` at
each segment's end and its frame is the first `Memory` report's
`ReportResult.table()` (a plan whose config declares no `Memory` report
is a `ValueError` naming the remedy, where the interim surface's
`simulate()` named it as `HaresSimulationError`); `DwellingGymEnv`
finishes at the truncating step and exposes `run_result` and the
underlying `dwelling`; `VecDwellingGymEnv` finishes each dwelling at
its truncation and exposes `run_results` and `envs`; the HELICS
dwelling wrapper finishes after the stepping loop and exposes
`run_result`. Every `simulate()` caller in `tests/python`, the notebook
and the docs reads the frame from `result.reports[i].table()`.

The docs' forward references made true now: `docs/python.md`'s RL
section's "the count is exposed with the run's `RunResult`" (row 30b's
line), its gym paragraph reading the finished `RunResult` off
`run_result`/`run_results`/`env.dwelling`/`envs`, and row 44's
dependency on the Python `finish()` (PR-4 times the finalisation this
entry exposes). `docs/python.md` gains the Results section (the
lifecycle: `initialize()` required once and cleared by
`reset_with_seed`; `simulate()`, stepping and `finish()`; the live
`health()`; requesting reports; the Arrow type of each column with
units and labels read from the catalogue; `SteppableFleet` stepping
does not rewind), its examples run as `test_docs_results_examples`, and
the Changes in this release section listing every added, changed,
renamed and removed name against the committed baseline
`tests/python/fixtures/_hares_ebdb1a17.pyi` (the stub as it was at
`ebdb1a17`, verified byte-equal to `git show
ebdb1a17:python/ochre_next/_hares.pyi` before committing); the Numeric
changes section pre-dates this row and keeps its standing lines.

The RED, proven differentially at the OR-6 commit (the extension built
there, the acceptance tests run against it): seven of the ten fail on
the missing surface (`python_stepped_finish_equals_simulate`,
`health_readable_with_no_reports_requested`,
`health_counts_clamped_gym_actions`,
`file_report_table_raises_naming_destination`,
`simulation_plan_reads_run_result`, and the two stub/doc pins
(`stub_changes_are_listed` on the missing changes section,
`test_docs_results_examples` on the missing Results section)), while
`billing_summaries_survive_finish` (the summaries pre-date the row),
`stubs_cover_every_binding` (trivially: no unstubbed class exists
either way) and `numeric_changes_name_existing_goldens` pass; GREEN:
all ten pass with the row (1559 Python tests total).

**State (review fixes, 2026-10-10):** the review's Python-surface
findings fixed on this branch: `e1c6b1df` (the one `simulate()` caller
still reading a frame off the return value migrated:
`test_pv_battery_ev_end_to_end` reads
`result.reports[0].table()` like its siblings and passes unfiltered;
the notebook's results panel, `docs/python.md`'s usage example (now
declaring the report it reads), `docs/architecture.md`'s method table
and diagram, and the conftest docstrings name the `RunResult` surface
instead of the deleted `results()`/`metrics()`; the doc example tests
re-run green); `230fe6eb` (the wrapper exposures pinned:
`DwellingGymEnv.run_result`, `VecDwellingGymEnv.run_results` and
`HELICSDwelling.run_result` each read in a test (``None`` before the
truncating step, the finished result after, cleared on ``reset()``);
the failed-batch `finish()` raise semantics documented in the docs'
Results section: a run latched at a failed batch callback or a spent
failure budget stays ended and its `finish()` raises the latched
error); and with it the HELICS guard (a termination before the first
step never finalises). Gates at the fix tip: pytest canonical 1563
passed, 0 failed, 0 skipped (1559 at the row plus the four new pins);
pyright 0 errors. The full gates and the unfiltered residue
comparison are recorded in OR-6's State.

Gates at the commit: fmt clean; clippy `--workspace --all-targets
--all-features -D warnings` clean; `cargo nextest run --all-features
-j 4` 7674/7674; `cargo nextest run -F dst -j 4` 7607/7607; pytest
canonical 1559 passed, 0 failed, 0 skipped; pyright 0 errors; the
goldens bitwise in both profiles (`frame_golden compare-all` 7
manifests identical on the default build, `full_year_900s_dst`
identical on the dst build).

## PY-10: the batch callback in Python

**Kind:** initiative work (`dl-i08-architecture.destinations`).

**Problem:** `03-output-reporting.md` OR-14 defines the Rust
`BatchCallback`; Python needs to attach a callable, and a Python exception
must fail the run with the exception chained.

**Change:** `Dwelling.attach_callback(report: str, fn)` where
`fn(info, batch)` receives a `BatchInfo` (`report`, `bldg_id`,
`batch_index`) and pyo3-arrow's `PyRecordBatch` (zero-copy; its
`__arrow_c_array__` is at `pyo3-arrow-0.19.0/src/record_batch.rs:183`;
`_hares.pyi` annotates it as `ArrowArrayExportable`, a `typing.Protocol`
declaring `__arrow_c_array__(self, requested_schema: object | None = None)
-> tuple[object, object]`). `Destination.callback()` (the Python
constructor of the OR-14 marker) and the exception class
`HaresCallbackError` (a subclass of `HaresSimulationError`, added to
`11-other-defects.md` OD-1's pickle round-trip test) arrive in this
commit. The adapter in `hares-python` implements
`BatchCallback` over `Py<PyAny>`; it calls `fn` under `Python::attach`
(`simulate` and `step` run detached). On an exception it stores the
`PyErr` in a slot shared with the dwelling wrapper and returns
`CallbackError`; when the binding turns the resulting
`HaresError::Callback` into `HaresCallbackError`, it takes the stored
exception and sets it as `__cause__`.

**Acceptance tests** (the `cz4a_ashp_hpwh` inputs, 24 h at 900 s,
`output_chunk_size=40`): `callback_receives_every_batch_in_order` (three
batches of 40, 40 and 16 rows; the concatenation equals a `Memory` report
of the same selection);
`callback_exception_fails_run_with_cause_chained`
(`exc.__cause__` is the raised object);
`callback_not_called_for_other_reports`;
`callback_info_carries_bldg_id`.

**Verification:** `uv run --no-sync pytest tests/python/test_callback.py`.

**Commit:** `feat: attach Python callables to report batches`

**Dependencies:** OR-14, PY-9, PY-1 (`pyo3-arrow` and its
`PyRecordBatch`).

## PY-11: Python actors declare their output variables

**Kind:** initiative work (`dl-i08-architecture.new_capability`: "Python
actors declare their output variables through the actor API").

**Problem:** the Python `Actor` base (`crates/hares-python/src/py_actor.rs:50`)
and its wrapper (`py_actor.rs:85-100`) expose no telemetry, so a Python
actor cannot put a variable into the output.

**Change:** a Python actor may define `output_variables(self) ->
list[hares.OutputVariable]` (`OutputVariable(name, unit, kind,
aggregation=None, across_homes=None, sampling="interval_mean",
nullable=False, labels=None, description="")`; `unit` must be a `Unit`
symbol; `aggregation=None` means the aggregation
`dl-i08-architecture.aggregation` gives the kind (`"sum"` for
`"energy"`, `"mode"` for `"enum"`, `"mean"` for `"state"`, `"power"`,
`"temperature"` and `"ratio"`), and it is required for
`kind == "quantity"`, whose rule
depends on what the variable holds (`"sum"`, `"mean"` or `"last"`;
`None` there is a `ValueError` naming the choices); an aggregation the
kind does not allow is a `ValueError`; `description` becomes
`VariableDef.description`; `across_homes` is
`"sum"` or `"mean"`, required exactly when `kind == "quantity"` and a
`ValueError` otherwise, and becomes `VariableKind::Quantity {
across_homes }` (`plan.md` §2.2, `dl-i08-architecture.aggregation`:
every quantity variable declares its rule across homes); `labels` is a
list of strings, required exactly when `kind == "enum"` (code `i` decodes
to `labels[i]`) and a `ValueError` otherwise, and becomes
`VariableKind::Enum { labels: EnumLabels::Runtime(..) }`, `plan.md` §2.2),
read once by `add_actor`, which builds the wrapper's sealed `Telemetry`
from them (`Telemetry::from_fields`, `03-output-reporting.md` OR-1). During `decide()`
the actor calls `self.set_output(name, value)`; an undeclared name raises
`KeyError` at once. `PyActorWrapper` returns the declared list from
`Actor::output_variables()` and its `Telemetry` from `telemetry()`, so
the variables enter the catalogue and the non-finite latch like any
actor's.

**Acceptance tests** (the `cz4a_ashp_hpwh` inputs, 24 h at 900 s, with a
test actor defined in the test module):
`python_actor_output_variable_appears_with_unit`
(a `Named` `Memory` report of `actor:<name>:<var>` has the values, and the
catalogue carries the declared unit);
`python_actor_enum_output_decodes_labels` (an `enum` variable declared
with `labels=["off", "low", "high"]` and published as codes 0, 1, 2: the
catalogue's `Variable.labels` decode the column's codes);
`python_actor_enum_without_labels_is_an_error`;
`python_actor_quantity_without_across_homes_is_an_error` (and
`across_homes="sum"` on a `"power"` variable is a `ValueError` too);
`python_actor_quantity_declares_across_homes` (a `"quantity"` variable
declared with `across_homes="mean"` and `aggregation="last"` has
`Variable.across_homes == "mean"` and `Variable.aggregation == "last"` in
the catalogue);
`python_actor_quantity_without_aggregation_is_an_error`;
`python_actor_undeclared_output_raises`;
`python_actor_non_finite_output_fails_run`.

**Verification:** `uv run --no-sync pytest tests/python`.

**Commit:** `feat: Python actors declare and publish output variables`

**Dependencies:** OR-1, OR-5, PY-9, PY-4 (the Python `Variable` the
tests read).

## PY-12: ResStock sample weights name what is missing and never default silently

**Kind:** fix (found defect; requirement R24).

**Problem:** a ResStock building's sample weight is the population
multiplier fleet aggregation depends on, and the lookup hides why it
fails. `_lookup_sample_weight` returns `None` both when the metadata has
no weight column and when the building is not in it, and
`_resolve_sample_weight` then warns "Building N not found in metadata" in
both cases, which misattributes a missing column. `fetch_resstock_fleet`
builds its weights only when the column exists and then passes
`weights.get(bid, 1.0)`, so a metadata file without a weight column gives
every home weight 1.0 with no message at all, and requested `bldg_ids`
absent from the metadata are dropped by the filter without a word.

**Evidence:** `python/ochre_next/data/resstock.py:747-765`
(`_lookup_sample_weight`), `:768-805` (`_resolve_sample_weight`),
`:1166-1179` (fleet weights only when `_weight_column` finds one),
`:1055`, `:1095` (`weights.get(bid, 1.0)`), `:1167-1170` (the
`bldg_ids` filter).

**Change** (in `python/hares/data/resstock.py` after PY-7):
- `_lookup_sample_weight` raises `ResStockMetadataError` (new, a
  `ValueError` subclass in the same module) naming the parquet and the
  columns it searched when there is no weight column, and naming the
  building and the parquet when the building is absent. A caller that
  passed `metadata_path` asked for the lookup, so a failed lookup is an
  error, not a default.
- `fetch_resstock_fleet` raises the same error when the metadata has no
  weight column, and when any requested `bldg_ids` entry is absent from
  the metadata (naming every absent id); weights are then read with
  `weights[bid]`, so no home gets a default.
- A single building fetched with neither `sample_weight` nor
  `metadata_path` keeps its documented unweighted value 1.0 and its
  `UserWarning`: the caller supplied nothing to look up.

**Acceptance tests** (`tests/python/test_resstock_weights.py`, new; each
test writes a small metadata parquet into a temporary directory and
fetches nothing over the network):
`missing_weight_column_is_named` (the error names the parquet and the
columns searched; no "not found" wording);
`missing_building_is_named`;
`fleet_without_weight_column_is_an_error`;
`fleet_requested_ids_absent_from_metadata_are_named`;
`single_building_without_metadata_warns_and_is_unweighted`.

**Verification:** `uv run --no-sync pytest tests/python/test_resstock_weights.py`;
the whole block.

**Commit:** `fix: ResStock sample-weight lookups name the missing column or building`

**Dependencies:** PY-7 (the module moves to `python/hares/`).
