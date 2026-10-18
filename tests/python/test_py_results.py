"""RunResult, finish(), health() and reports in Python.

The Rust lifecycle's Python surface: simulate() and finish() return the
run's RunResult, health() reads the run's live health totals, and the
reports' results are the RunResult's own. The RED tests here are written
against that surface before it exists.
"""

from __future__ import annotations

import ast
from datetime import datetime
from pathlib import Path
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[2]
HARES_DEFAULTS = ROOT / "python" / "hares" / "defaults"

FIXTURE = ROOT / "tests/fixtures/parity/cz4a_ashp_hpwh"
HPXML = str(FIXTURE / "building.xml")
SCHEDULE = str(FIXTURE / "schedule.csv")
WEATHER = str(FIXTURE / "weather.epw")


def result_config(**extra: Any):
    """The acceptance fixture's config: the cz4a inputs, start
    2023-01-01T00:00:00-07:00, 24 h at 900 s, one Memory report at
    Selection.verbosity(2), output_chunk_size=40."""
    from hares import Destination, ReportSpec, Selection, SimulationConfig

    return SimulationConfig(
        start_time="2023-01-01T00:00:00-07:00",
        duration=86400,
        time_res=900,
        output_chunk_size=40,
        reports=[ReportSpec("frame", Selection.verbosity(2), Destination.memory())],
        **extra,
    )


def build(config: Any, **kw: Any):
    from hares import Dwelling

    dw = Dwelling.from_hpxml(
        HPXML,
        SCHEDULE,
        WEATHER,
        defaults=str(HARES_DEFAULTS),
        bldg_id=1,
        config=config,
        **kw,
    )
    dw.initialize()
    return dw


def test_python_stepped_finish_equals_simulate() -> None:
    """96 steps then finish() equals simulate() on an identical dwelling:
    the reports' tables, the metrics and the health equal."""
    from hares import Dwelling

    simulated = build(result_config())
    simulated_result = simulated.simulate()

    stepped = build(result_config())
    for _ in stepped.timesteps():
        stepped.step()
    finished_result = stepped.finish()

    assert finished_result.reports[0].table().equals(
        simulated_result.reports[0].table()
    ), "the reports' frames are equal"
    assert finished_result.metrics == simulated_result.metrics, "metrics equal"
    assert finished_result.health == simulated_result.health, "health equal"


def test_health_readable_with_no_reports_requested() -> None:
    """reports=[]: health() after 10 steps reads zero counters and the
    warm-up outcome the config asked for. The live health needs no report."""
    from hares import SimulationConfig

    dw = build(SimulationConfig(start_time="2023-01-01T00:00:00-07:00", duration=86400, time_res=900))
    for _ in range(10):
        dw.step()

    health = dw.health()
    assert health.clamped_actions == 0
    assert health.rejected_control_signals == 0
    assert health.port_rollbacks == 0
    assert health.curve_index_clamps == 0
    assert health.fluid_payload_absences == 0
    assert health.evicted_actors == 0
    assert health.warmup.outcome == "ran"
    assert health.warmup.converged is True
    days_run = health.warmup.days_run
    assert days_run is not None and days_run >= 1


def test_health_counts_clamped_gym_actions() -> None:
    """A DwellingGymEnv and a VecDwellingGymEnv of one environment over the
    same inputs, each given one action whose first value is above its
    field_bounds upper bound: health().clamped_actions reads 1 after that
    step on both paths, and 0 after a step whose action is inside the
    bounds."""
    np = pytest.importorskip("numpy")
    pytest.importorskip("gymnasium")
    from datetime import timedelta

    from hares import SimulationConfig
    from hares._hares import Dwelling as PyDwelling
    from hares.rl.gym_env import DwellingGymEnv, GymDwellingConfig
    from hares.rl.vec_env import VecDwellingGymEnv

    def make_config():
        return GymDwellingConfig(
            hpxml=HPXML,
            schedule=SCHEDULE,
            weather=WEATHER,
            defaults=str(HARES_DEFAULTS),
            sim_config=SimulationConfig(
                start_time="2023-01-01T00:00:00-07:00",
                duration=86400,
                time_res=900,
            ),
        )

    def make_dwelling():
        from hares._hares import Dwelling as PyDwelling

        dw = PyDwelling.from_hpxml(
            hpxml=HPXML,
            schedule=SCHEDULE,
            weather=WEATHER,
            defaults=str(HARES_DEFAULTS),
            config=SimulationConfig(
                start_time="2023-01-01T00:00:00-07:00",
                duration=86400,
                time_res=900,
            ),
        )
        dw.initialize()
        return dw

    def make_env():
        return DwellingGymEnv(
            config=make_config(),
            observation_fields=["total_power_kw"],
            action_space_config={"ASHP Heater": ["heat_c"]},
            reward_fn=lambda ctx: -ctx["total_power_kw"],
            episode_length=timedelta(minutes=15 * 20),
        )

    env = make_env()
    env.reset()
    env.step(np.array([100.0]))
    assert env.dwelling.health().clamped_actions == 1, "the 100.0 action is one clamped value"
    env.step(np.array([20.0]))
    assert env.dwelling.health().clamped_actions == 1, "the in-bounds step clamps nothing new"

    vec = VecDwellingGymEnv(
        [make_dwelling()],
        observation_fields=["total_power_kw"],
        action_space_config={"ASHP Heater": ["heat_c"]},
        reward_fn=lambda ctx: -ctx["total_power_kw"],
        episode_length=timedelta(minutes=15 * 20),
    )
    vec.reset()
    vec.step(np.array([[100.0]]))
    assert vec.envs[0].health().clamped_actions == 1, "the vec path clamps and counts"
    vec.step(np.array([[20.0]]))
    assert vec.envs[0].health().clamped_actions == 1, "the in-bounds step clamps nothing new"


def test_billing_summaries_survive_finish() -> None:
    """The same inputs with an attached tariff, run to the end:
    billing_summaries() after simulate() holds the run's period, and
    dwelling_fatal reads False."""
    from hares import ElectricTariff

    dw = build(result_config())
    tariff = ElectricTariff.from_urdb_json(str(ROOT / "tests/fixtures/urdb/flat_rate.json"))
    dw.set_electric_tariff(tariff, timezone="America/Denver")

    dw.simulate()

    summaries = dw.billing_summaries()
    assert len(summaries) > 0, "the run's billing period survives finish()"
    assert dw.dwelling_fatal is False


def test_simulation_plan_reads_run_result() -> None:
    """SimulationPlan's run reads the RunResult's reports: its frame is the
    memory report's table."""
    from hares import SimulationPlan

    plan = SimulationPlan(
        HPXML,
        SCHEDULE,
        WEATHER,
        defaults=str(HARES_DEFAULTS),
        bldg_id=5,
        time_res_s=900,
        verbosity=2,
    )
    plan.add_segment(
        start=datetime(2023, 1, 1, 0, 0),
        end=datetime(2023, 1, 1, 6, 0),
    )
    frame = plan.run()

    from hares import Destination, Dwelling, ReportSpec, Selection, SimulationConfig

    dw = Dwelling.from_hpxml(
        HPXML,
        SCHEDULE,
        WEATHER,
        defaults=str(HARES_DEFAULTS),
        bldg_id=5,
        config=SimulationConfig(
            start_time="2023-01-01T00:00:00-07:00",
            duration=6 * 3600,
            time_res=900,
            master_seed=42,
            reports=[ReportSpec("frame", Selection.verbosity(2), Destination.memory())],
        ),
    )
    dw.initialize()
    result = dw.simulate()

    # The plan's own "segment" column aside, the frame IS the run's report
    # table: the plan's segments start on naive local wall-clock times, so
    # the value comparison pins the schema and the row count (the manual
    # run's fixed offset start shifts the values, not the surface).
    assert "segment" in frame.columns
    planned = frame.drop("segment")
    report_frame = result.reports[0].table()
    assert planned.columns == report_frame.columns
    assert planned.shape == report_frame.shape
    assert planned.schema == report_frame.schema
    assert planned.height == 24, "6 h at 900 s is 24 rows"


def test_table_matches_arrow_export() -> None:
    """table() equals polars.DataFrame(arrow()) and has the report's
    schema: the frame is read through the Arrow PyCapsule interface the
    arrow() export exposes, its buffers shared, no IPC round trip."""
    import polars as pl

    dw = build(result_config())
    result = dw.simulate()
    report = result.reports[0]

    table = report.arrow()
    assert hasattr(table, "__arrow_c_stream__"), "arrow() exposes the capsule interface"
    frame = report.table()
    imported = pl.DataFrame(table)
    assert frame.equals(imported), "table() is the arrow() export's frame"

    assert list(imported.schema.keys()) == frame.columns, (
        "the frame carries the report's columns"
    )
    assert imported.schema["Time"] == pl.Datetime("ms"), (
        "Time is the report's millisecond timestamp"
    )
    for name in frame.columns:
        if name != "Time":
            assert imported.schema[name] == pl.Float64, f"{name} is a Float64 column"


def test_report_can_be_exported_twice() -> None:
    """A report's arrow() export is repeatable: every call yields a stream
    that reads back to the same frame, and table() after both agrees."""
    import polars as pl

    dw = build(result_config())
    result = dw.simulate()
    report = result.reports[0]

    first = pl.DataFrame(report.arrow())
    second = pl.DataFrame(report.arrow())
    assert first.equals(second), "both exports read back to the same frame"
    assert report.table(rechunk=False).equals(first), "table() after both exports agrees"


def test_rechunk_false_keeps_batch_boundaries() -> None:
    """arrow(rechunk=False) has one chunk per built batch (96 rows at
    output_chunk_size=40: batches of 40, 40 and 16); arrow(rechunk=True)
    holds one batch."""
    import polars as pl

    dw = build(result_config())
    result = dw.simulate()
    report = result.reports[0]

    unchunked = pl.DataFrame(report.arrow(rechunk=False))
    assert unchunked.height == 96
    assert unchunked.n_chunks() == 3, "one chunk per built batch"
    rechunked = pl.DataFrame(report.arrow(rechunk=True))
    assert rechunked.height == 96
    assert rechunked.n_chunks() == 1, "one rechunked batch"
    assert unchunked.equals(rechunked), "both reads hold the same rows"


def test_file_report_export_raises_naming_destination(tmp_path) -> None:
    """arrow() and table() on a file report raise HaresSimulationError
    naming the report and its destination (the file's path), never an
    empty table."""
    import re

    from hares import Destination, ReportSpec, Selection, SimulationConfig
    from hares._hares import HaresSimulationError

    target = tmp_path / "run.parquet"
    dw = build(
        SimulationConfig(
            start_time="2023-01-01T00:00:00-07:00",
            duration=86400,
            time_res=900,
            reports=[
                ReportSpec(
                    "parquet_file",
                    Selection.verbosity(2),
                    Destination.parquet(str(target)),
                )
            ],
        )
    )
    result = dw.simulate()

    report = result.reports[0]
    assert report.name == "parquet_file"
    escaped = re.escape(str(target))
    with pytest.raises(HaresSimulationError, match=escaped):
        report.arrow()
    with pytest.raises(HaresSimulationError, match=escaped):
        report.table()


def test_file_report_table_raises_naming_destination(tmp_path) -> None:
    """table() on a file report raises HaresSimulationError naming the
    report and its destination."""
    from hares import Destination, ReportSpec, Selection, SimulationConfig
    from hares._hares import HaresSimulationError

    target = tmp_path / "run.csv"
    dw = build(
        SimulationConfig(
            start_time="2023-01-01T00:00:00-07:00",
            duration=86400,
            time_res=900,
            reports=[
                ReportSpec(
                    "file",
                    Selection.verbosity(2),
                    Destination.csv(str(target)),
                )
            ],
        )
    )
    result = dw.simulate()

    report = result.reports[0]
    assert report.name == "file"
    with pytest.raises(HaresSimulationError, match="file"):
        report.table()
    assert report.path == str(target)
    assert target.exists()
    assert report.rows == 96
    assert report.batches_delivered >= 1


def test_stubs_cover_every_binding() -> None:
    """Every public name of hares._hares is in the stub file and every stub
    name exists."""
    import hares
    import hares._hares as _hares

    def stub_names_of(tree: ast.Module) -> set[str]:
        names: set[str] = set()
        for node in tree.body:
            if isinstance(node, (ast.ClassDef, ast.FunctionDef)):
                names.add(node.name)
            elif isinstance(node, ast.Assign):
                for target in node.targets:
                    if isinstance(target, ast.Name):
                        names.add(target.id)
        return names

    stub_names = stub_names_of(ast.parse((ROOT / "python/hares/_hares.pyi").read_text()))

    module_names = {name for name in dir(_hares) if not name.startswith("_")}
    module_names = {
        name
        for name in module_names
        if isinstance(getattr(_hares, name), type)
    }

    missing_from_stub = module_names - stub_names
    assert missing_from_stub == set(), f"public classes missing from the stub: {missing_from_stub}"
    missing_from_module = {name for name in stub_names if not hasattr(_hares, name)}
    assert missing_from_module == set(), f"stub names missing from the module: {missing_from_module}"


def test_stub_changes_are_listed() -> None:
    """Every name added to or removed from _hares.pyi against the committed
    ebdb1a17 stub, compared by ast, appears in the changelog's "Changes in
    this release" section, and every name the section lists as added or
    removed is such a name."""
    baseline = ast.parse((ROOT / "tests/python/fixtures/_hares_ebdb1a17.pyi").read_text())
    current = ast.parse((ROOT / "python/hares/_hares.pyi").read_text())

    def names_of(tree: ast.Module) -> set[str]:
        return {node.name for node in tree.body if isinstance(node, (ast.ClassDef, ast.FunctionDef))}

    baseline_names = names_of(baseline)
    current_names = names_of(current)
    added = current_names - baseline_names
    removed = baseline_names - current_names

    changelog = (ROOT / "CHANGELOG.md").read_text()
    section = changelog.split("## Changes in this release", 1)[1]
    listed = set()
    for line in section.splitlines():
        stripped = line.strip(" -*")
        for name in added | removed:
            if name in stripped:
                listed.add(name)
    assert added <= listed, f"added names not in the changes section: {added - listed}"
    assert removed <= listed, f"removed names not in the changes section: {removed - listed}"


def test_numeric_changes_name_existing_goldens() -> None:
    """Every golden fixture a "Numeric changes" line names has a manifest
    under tests/fixtures/golden/."""
    docs = (ROOT / "docs/python.md").read_text()
    section = docs.split("## Numeric changes", 1)[1]
    golden_dir = ROOT / "tests/fixtures/golden"
    manifests = {path.stem.removesuffix(".golden") for path in golden_dir.glob("*")}
    named = set()
    for line in section.splitlines():
        for token in line.replace(",", " ").split("`"):
            token = token.strip()
            if token in manifests or (token.endswith(".golden.json") and token.removesuffix(".golden.json") in manifests):
                named.add(token)
        for manifest in manifests:
            if manifest in line:
                named.add(manifest)
    assert named, "the numeric changes section names golden fixtures"
    for name in named:
        assert (golden_dir / f"{name}.golden.json").exists() or (golden_dir / f"{name}.toml").exists(), (
            f"the named golden {name} has no manifest"
        )


def test_docs_results_examples() -> None:
    """The Results section's examples run as written."""
    import re

    docs = (ROOT / "docs/python.md").read_text()
    section = docs.split("## Results", 1)[1].split("## ", 1)[0]
    examples = re.findall(r"```python\n(.*?)```", section, flags=re.DOTALL)
    assert examples, "the Results section carries runnable examples"

    for i, example in enumerate(examples):
        script = example
        script = script.replace("path/to/building.xml", HPXML)
        script = script.replace("path/to/schedules.csv", SCHEDULE)
        script = script.replace("path/to/weather.epw", WEATHER)
        script = script.replace("path/to/HARES/defaults", str(HARES_DEFAULTS))
        # Each example is its own program: fresh namespace per example.
        env: dict[str, object] = {"__name__": "__main__"}
        exec(compile(script, f"<docs results {i}>", "exec"), env)  # noqa: S102
        del env


def test_python_build_info_equals_run_provenance() -> None:
    """The package's build_info() equals a run's provenance.build field
    for field, the provenance catalogue digest is a 64 char hex string,
    and the package version is the build identity's version."""
    import hares

    dw = build(result_config(metrics=True))
    result = dw.simulate()

    info = hares.build_info()
    provenance = result.provenance.build
    assert info.version == provenance.version
    assert info.commit == provenance.commit
    assert info.dirty == provenance.dirty
    assert info.profile == provenance.profile
    assert list(info.features) == list(provenance.features)
    assert info.rustc == provenance.rustc

    assert info.commit, "the build records the commit it was built from"
    assert hares.__version__ == info.version

    digest = result.provenance.catalogue_digest
    assert isinstance(digest, str), "the catalogue digest is a hex string"
    assert len(digest) == 64 and int(digest, 16) >= 0, f"a 64 char hex digest: {digest}"


def test_provenance_inputs_record_input_digests() -> None:
    """provenance.inputs records every file input the run read: the HPXML,
    the schedule and the weather by path and hex digest, the weather format
    the run parsed the weather as, and the digest over the defaults files;
    two runs on the same inputs record equal dicts."""
    import hashlib

    dw = build(result_config())
    result = dw.simulate()

    inputs = result.provenance.inputs
    assert inputs is not None, "a run built from files records its inputs"
    assert inputs["hpxml"]["path"] == HPXML
    assert inputs["schedule"]["path"] == SCHEDULE
    assert inputs["weather"]["path"] == WEATHER
    for name, path in (("hpxml", HPXML), ("schedule", SCHEDULE), ("weather", WEATHER)):
        expected = hashlib.sha256(Path(path).read_bytes()).hexdigest()
        assert inputs[name]["sha256"] == expected, f"the {name} digest is the file bytes' sha256"
    assert inputs["weather_format"] == "epw"
    assert len(inputs["defaults_files_sha256"]) == 64

    second = build(result_config()).simulate().provenance.inputs
    assert second == inputs, "two runs on the same inputs record equal dicts"


def store_read_walk(defaults: Path):
    """The defaults store's read set for the repo's defaults tree, walked
    independently in load order: the loaders' rules, mirrored. Each entry
    is (path relative to the defaults dir, bytes). A loader that stops
    reading a file this walk covers, or reads it in another order, digests
    a different byte stream and the pin below fails."""
    def tomls(rel: str):
        d = defaults / rel
        if not d.is_dir():
            return
        for p in sorted(d.iterdir(), key=lambda p: p.name):
            if p.is_file() and p.suffix == ".toml":
                yield p.relative_to(defaults).as_posix(), p.read_bytes()

    def hvac(rel: str):
        d = defaults / rel
        if not d.is_dir():
            return
        for p in sorted(d.iterdir(), key=lambda p: p.name):
            if p.is_file() and p.suffix in (".toml", ".csv"):
                yield p.relative_to(defaults).as_posix(), p.read_bytes()

    def csv_at(rel: str):
        p = defaults / rel
        if p.is_file():
            yield rel, p.read_bytes()

    yield from csv_at("zip_parameters.toml")
    yield from hvac("hvac_cooling")
    yield from hvac("hvac_heating")
    yield from csv_at("HVAC Multispeed Parameters.csv")
    # The envelope LUT: the two required CSVs, then the optional one.
    yield from csv_at("envelope/Envelope Boundary Types.csv")
    yield from csv_at("envelope/Envelope Materials.csv")
    yield from csv_at("envelope/Envelope Boundaries.csv")
    yield from tomls("battery")
    yield from tomls("envelope")
    yield from tomls("ev")
    yield from tomls("generator")
    # The generator curve's typed loader rereads the curve file.
    yield from csv_at("generator/efficiency_curve.toml")
    yield from tomls("loads")
    # The PV equipment defaults and the typed panel specs each read the
    # directory, so every panel TOML is logged twice.
    yield from tomls("pv")
    yield from tomls("pv")
    yield from tomls("water_heating")
    yield from csv_at("water_heating/default_paramters.csv")
    yield from csv_at("ev/vehicle_mapping.csv")


def test_defaults_files_sha256_matches_an_independent_hashlib_walk() -> None:
    """The provenance defaults digest equals an independent hashlib
    recomputation over the store's read set in load order: each file's
    relative path, a 0x00 separator and its bytes, concatenated, with the
    schedule profiles file composed after the store's read (every run with
    a defaults dir reads it)."""
    import hashlib

    dw = build(result_config())
    result = dw.simulate()

    inputs = result.provenance.inputs
    assert inputs is not None, "a run built from files records its inputs"

    hasher = hashlib.sha256()
    for rel, data in store_read_walk(HARES_DEFAULTS):
        hasher.update(rel.encode())
        hasher.update(b"\x00")
        hasher.update(data)
    profiles = "Default Schedule Parameters.csv"
    hasher.update(profiles.encode())
    hasher.update(b"\x00")
    hasher.update(Path(HARES_DEFAULTS, profiles).read_bytes())

    assert inputs["defaults_files_sha256"] == hasher.hexdigest(), (
        "the defaults digest equals the independent walk of the read set"
    )


def test_python_build_info_repr_is_python() -> None:
    """repr(BuildInfo) follows the Python repr conventions: Python bool
    or None for the dirty flag, a list repr for the features, None for
    an absent commit, never the Rust forms."""
    import hares

    info = hares.build_info()
    assert repr(info) == (
        f"BuildInfo(version={info.version!r}, commit={info.commit!r}, dirty={info.dirty!r}, "
        f"profile={info.profile!r}, features={list(info.features)!r}, rustc={info.rustc!r})"
    )
