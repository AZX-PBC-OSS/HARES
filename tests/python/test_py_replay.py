"""The replay export in Python: ``RunResult.export_replay`` and
``hares.replay``.

RED: written against that surface before it exists, so the import fails
against the parent commit.
"""

from __future__ import annotations

import shutil
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
HARES_DEFAULTS = ROOT / "defaults"

FIXTURE = ROOT / "tests/fixtures/parity/cz4a_ashp_hpwh"
HPXML = str(FIXTURE / "building.xml")
SCHEDULE = str(FIXTURE / "schedule.csv")
WEATHER = str(FIXTURE / "weather.epw")


def replay_config() -> Any:
    from hares import Destination, ReportSpec, Selection, SimulationConfig

    return SimulationConfig(
        start_time="2023-01-01T00:00:00-07:00",
        duration=14400,
        time_res=900,
        output_chunk_size=40,
        metrics=True,
        reports=[ReportSpec("frame", Selection.verbosity(2), Destination.memory())],
    )


def build_replayable(tmp_path: Path) -> Any:
    """The cz4a inputs copied into the temporary directory, the dwelling
    built from the copies: the originals can be deleted before replay."""
    import hares

    for name in ("building.xml", "schedule.csv", "weather.epw"):
        shutil.copy(FIXTURE / name, tmp_path / name)
    dwelling = hares.Dwelling.from_hpxml(
        str(tmp_path / "building.xml"),
        str(tmp_path / "schedule.csv"),
        str(tmp_path / "weather.epw"),
        defaults=str(HARES_DEFAULTS),
        bldg_id=1,
        config=replay_config(),
    )
    dwelling.initialize()
    return dwelling


def test_replay_round_trip_in_python(tmp_path: Path) -> None:
    """The same export and replay through ``RunResult.export_replay`` and
    ``hares.replay``: the reports and the metrics equal."""
    import hares

    inputs = tmp_path / "inputs"
    inputs.mkdir()
    original_dwelling = build_replayable(inputs)
    original = original_dwelling.simulate()

    export = tmp_path / "export"
    original.export_replay(str(export))
    assert (export / "replay.toml").is_file(), "the export writes replay.toml"
    shutil.rmtree(inputs)

    replayed_dwelling = hares.replay(str(export), defaults=str(HARES_DEFAULTS))
    replayed = replayed_dwelling

    assert replayed.reports[0].table().equals(
        original.reports[0].table()
    ), "the replayed reports' frames are equal"
    assert replayed.metrics == original.metrics, "the metrics are equal"
    assert (
        replayed.provenance.bldg_id == original.provenance.bldg_id
    ), "the replay runs for the exported building id"


def test_replay_requires_defaults(tmp_path: Path) -> None:
    """``hares.replay`` with ``defaults=None`` names the required
    argument: the packaged defaults have not landed yet."""
    import hares

    inputs = tmp_path / "inputs"
    inputs.mkdir()
    original_dwelling = build_replayable(inputs)
    original = original_dwelling.simulate()

    export = tmp_path / "export"
    original.export_replay(str(export))

    try:
        hares.replay(str(export))
    except Exception as err:  # noqa: BLE001
        assert "defaults" in str(err), "the error names the defaults argument"
    else:
        raise AssertionError("replay without defaults must fail")


def test_provenance_tariff_readable(tmp_path: Path) -> None:
    """A run with a tariff attached reads ``Provenance.tariff`` as a dict
    with the tariff and its zone; a run without one reads ``None``."""
    import hares

    inputs = tmp_path / "inputs"
    inputs.mkdir()
    dwelling = build_replayable(inputs)
    plain = dwelling.simulate()
    assert plain.provenance.tariff is None, "no tariff attached: None"

    tariff_path = ROOT / "tests/fixtures/urdb/flat_rate.json"
    inputs2 = tmp_path / "inputs2"
    inputs2.mkdir()
    dwelling2 = build_replayable(inputs2)
    dwelling2.set_electric_tariff(
        hares.ElectricTariff.from_urdb_json(str(tariff_path)), timezone="America/Denver"
    )
    attached = dwelling2.simulate()
    record = attached.provenance.tariff
    assert record is not None, "the attached tariff reads back"
    assert record["zone"] == "America/Denver", "the zone reads back"
    assert isinstance(record["tariff"], dict), "the tariff is a dict"
