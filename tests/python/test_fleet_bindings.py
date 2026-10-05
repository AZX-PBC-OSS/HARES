from pathlib import Path

import pyarrow as pa
import pyarrow.parquet as pq
import pytest
from ochre_next import Fleet


def _write_resstock_2024_2_metadata(path: Path) -> None:
    table = pa.table(
        {
            "building_id": pa.array([12345]),
            "upgrade": pa.array([0]),
            "sample_weight": pa.array([1.0]),
            "in.state": pa.array(["CO"]),
        }
    )
    pq.write_table(table, path)


def test_fleet_from_resstock_accepts_2024_2(tmp_path: Path) -> None:
    metadata = tmp_path / "metadata.parquet"
    _write_resstock_2024_2_metadata(metadata)

    fleet = Fleet.from_resstock(
        str(metadata),
        str(tmp_path),
        str(tmp_path),
        resstock_version="2024.2",
    )

    assert isinstance(fleet, Fleet)
    assert len(fleet) >= 1, f"Fleet should have at least 1 building, got {len(fleet)}"


def test_fleet_from_resstock_rejects_invalid_version(tmp_path: Path) -> None:
    with pytest.raises(ValueError, match="invalid"):
        Fleet.from_resstock(
            str(tmp_path / "metadata.parquet"),
            str(tmp_path),
            str(tmp_path),
            resstock_version="2.2.1",
        )
