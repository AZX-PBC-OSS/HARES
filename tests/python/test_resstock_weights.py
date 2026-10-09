"""ResStock sample-weight lookups name what is missing.

A building's sample weight is the population multiplier fleet aggregation
depends on, and the lookups behind it fail in two distinguishable ways (the
metadata has no weight column, the building is absent from it). Each failure
is a typed error naming the parquet and the columns searched or the missing
building, never a silent default: a caller that passed ``metadata_path``
asked for the lookup. Each test writes a small metadata parquet into a
temporary directory and fetches nothing over the network.
"""

from __future__ import annotations

import io
import zipfile
from pathlib import Path
from typing import Any
from unittest import mock

import polars as pl
import pytest

from hares.data.resstock import (
    _lookup_sample_weight,
    fetch_resstock_building,
    fetch_resstock_fleet,
)


def _write_parquet(path: Path, columns: dict[str, list[Any]]) -> Path:
    df = pl.DataFrame(columns)
    df.write_parquet(path)
    return path


def _weighted_metadata(path: Path, bldg_ids: list[int]) -> Path:
    """A metadata parquet with the ResStock weight column and two buildings."""
    n = len(bldg_ids)
    return _write_parquet(
        path,
        {
            "bldg_id": bldg_ids,
            "in.state": ["CO"] * n,
            "in.sample_weight": [2.5] * n,
        },
    )


def _unweighted_metadata(path: Path, bldg_ids: list[int]) -> Path:
    """A metadata parquet with no recognisable weight column."""
    n = len(bldg_ids)
    return _write_parquet(
        path,
        {"bldg_id": bldg_ids, "in.state": ["CO"] * n},
    )


def _refuse_download(url: str, dest: Path) -> None:
    raise AssertionError(f"the fetch reached the network: {url} -> {dest}")


def test_missing_weight_column_is_named(tmp_path: Path):
    """A lookup against a parquet with no weight column is an error naming
    the parquet and the columns searched, with no "not found" wording: the
    column is what is missing, not the building."""
    meta = _unweighted_metadata(tmp_path / "metadata.parquet", [1, 2])

    with pytest.raises(ValueError, match="sample-weight") as excinfo:
        _lookup_sample_weight(meta, 1)

    message = str(excinfo.value)
    assert str(meta) in message, f"the error must name the parquet: {message}"
    assert "bldg_id" in message and "in.state" in message, (
        f"the error must name the columns searched: {message}"
    )
    assert "not found" not in message
    assert excinfo.type.__name__ == "ResStockMetadataError"


def test_missing_building_is_named(tmp_path: Path):
    """A lookup for a building the parquet does not hold is an error naming
    the building and the parquet."""
    meta = _weighted_metadata(tmp_path / "metadata.parquet", [1, 2])

    with pytest.raises(ValueError, match="building 999") as excinfo:
        _lookup_sample_weight(meta, 999)

    message = str(excinfo.value)
    assert str(meta) in message, f"the error must name the parquet: {message}"
    assert excinfo.type.__name__ == "ResStockMetadataError"


def test_fleet_without_weight_column_is_an_error(tmp_path: Path):
    """A fleet fetch whose metadata has no weight column is an error up
    front, not a silent fleet of weight-1.0 homes."""
    meta = _unweighted_metadata(tmp_path / "metadata.parquet", [1, 2])

    with (
        mock.patch("hares.data.resstock._download_file", side_effect=_refuse_download),
        pytest.raises(ValueError, match="sample-weight") as excinfo,
    ):
        fetch_resstock_fleet(meta, bldg_ids=[1, 2], cache_dir=tmp_path)

    assert excinfo.type.__name__ == "ResStockMetadataError"


def test_fleet_requested_ids_absent_from_metadata_are_named(tmp_path: Path):
    """Requested building ids the metadata does not hold are named, every
    one of them, instead of being dropped by the filter without a word."""
    meta = _weighted_metadata(tmp_path / "metadata.parquet", [1, 2])

    with (
        mock.patch("hares.data.resstock._download_file", side_effect=_refuse_download),
        pytest.raises(ValueError, match="7") as excinfo,
    ):
        fetch_resstock_fleet(meta, bldg_ids=[2, 7, 9], cache_dir=tmp_path)

    message = str(excinfo.value)
    assert "9" in message, f"the error must name every absent id: {message}"
    assert str(meta) in message, f"the error must name the parquet: {message}"
    assert excinfo.type.__name__ == "ResStockMetadataError"


def test_duplicate_bldg_id_is_named_by_the_single_lookup(tmp_path: Path):
    """A metadata parquet whose building ids repeat cannot say which row's
    weight is the building's: the single lookup is an error naming the id
    and the parquet, not the first row's weight silently winning."""
    meta = _write_parquet(
        tmp_path / "duplicate.parquet",
        {
            "bldg_id": [1, 2, 1],
            "in.state": ["CO", "CO", "CO"],
            # The rows disagree, so first-row and last-row reads disagree too.
            "in.sample_weight": [2.5, 3.5, 9.5],
        },
    )

    with pytest.raises(ValueError, match="building 1") as excinfo:
        _lookup_sample_weight(meta, 1)

    message = str(excinfo.value)
    assert str(meta) in message, f"the error must name the parquet: {message}"
    assert "not found" not in message
    assert excinfo.type.__name__ == "ResStockMetadataError"


def test_fleet_duplicate_bldg_ids_are_named(tmp_path: Path):
    """A fleet fetch from a metadata whose building ids repeat is an error
    up front, naming the duplicated ids and the parquet, fetching nothing:
    the fleet's weight dict cannot take the last row's weight silently."""
    meta = _write_parquet(
        tmp_path / "duplicate.parquet",
        {
            "bldg_id": [1, 2, 1, 2],
            "in.state": ["CO", "CO", "CO", "CO"],
            "in.sample_weight": [2.5, 3.5, 9.5, 7.5],
        },
    )

    with (
        mock.patch("hares.data.resstock._download_file", side_effect=_refuse_download),
        pytest.raises(ValueError, match="appear more than once") as excinfo,
    ):
        fetch_resstock_fleet(meta, cache_dir=tmp_path)

    message = str(excinfo.value)
    assert "[1, 2]" in message, f"the error must name every duplicated id: {message}"
    assert str(meta) in message, f"the error must name the parquet: {message}"
    assert excinfo.type.__name__ == "ResStockMetadataError"


def test_single_building_weight_check_precedes_any_download(tmp_path: Path):
    """The single-building path refuses a metadata with no weight column
    before it reaches the network, the same fail-fast order the fleet path
    keeps."""
    meta = _unweighted_metadata(tmp_path / "metadata.parquet", [1])

    with (
        mock.patch("hares.data.resstock._download_file", side_effect=_refuse_download),
        pytest.raises(ValueError, match="sample-weight") as excinfo,
    ):
        fetch_resstock_building(1, version="2024.2", cache_dir=tmp_path, metadata_path=meta)

    assert excinfo.type.__name__ == "ResStockMetadataError"


def test_single_building_without_metadata_warns_and_is_unweighted(tmp_path: Path):
    """A building fetched with neither sample_weight nor metadata_path keeps
    the documented unweighted value 1.0 and its UserWarning: the caller
    supplied nothing to look up."""
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w") as zf:
        zf.writestr("home.xml", "<HPXML></HPXML>")
        zf.writestr("in.schedules.csv", "hour,value\n0,1.0\n")
    zip_bytes = buf.getvalue()
    dummy_epw = tmp_path / "dummy.epw"
    dummy_epw.write_text("fake EPW")

    def fake_download(url: str, dest: Path) -> None:
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_bytes(zip_bytes if url.endswith(".zip") else b"fake,weather\n0,0\n")

    with (
        mock.patch("hares.data.resstock._download_file", side_effect=fake_download),
        mock.patch("hares.data.weather.get_epw_for_fips", return_value=dummy_epw),
        pytest.warns(UserWarning, match="sample_weight defaults"),
    ):
        result = fetch_resstock_building(1, version="2024.2", cache_dir=tmp_path)

    assert result.sample_weight == 1.0
