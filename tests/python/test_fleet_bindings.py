from pathlib import Path

import pytest


def _pyfleet_class():
    try:
        from ochre_next import Fleet
    except ModuleNotFoundError:
        pytest.skip("ochre_next is not available in this environment")
    return Fleet


def _write_resstock_2024_2_metadata(path: Path) -> None:
    pa = pytest.importorskip("pyarrow")
    pq = pytest.importorskip("pyarrow.parquet")
    table = pa.table(
        {
            "building_id": [12345],
            "upgrade": [0],
            "sample_weight": [1.0],
            "in.state": ["CO"],
        }
    )
    pq.write_table(table, path)


def test_fleet_from_resstock_accepts_2024_2(tmp_path: Path) -> None:
    py_fleet = _pyfleet_class()
    metadata = tmp_path / "metadata.parquet"
    _write_resstock_2024_2_metadata(metadata)

    fleet = py_fleet.from_resstock(
        str(metadata),
        str(tmp_path),
        str(tmp_path),
        resstock_version="2024.2",
    )

    assert isinstance(fleet, py_fleet)
    assert len(fleet) >= 1, f"Fleet should have at least 1 building, got {len(fleet)}"


def test_fleet_from_resstock_rejects_invalid_version(tmp_path: Path) -> None:
    py_fleet = _pyfleet_class()
    with pytest.raises(ValueError, match="invalid"):
        py_fleet.from_resstock(
            str(tmp_path / "metadata.parquet"),
            str(tmp_path),
            str(tmp_path),
            resstock_version="2.2.1",
        )
