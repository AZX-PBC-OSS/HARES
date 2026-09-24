"""Tests for stratified fixture selection in scripts/download_resstock_fixtures.py."""

from __future__ import annotations

import io
import sys
from pathlib import Path

import polars as pl


# ---------------------------------------------------------------------------
# Access internal functions from the script
# ---------------------------------------------------------------------------

# The script is at scripts/download_resstock_fixtures.py relative to repo root.
# Add the scripts directory to the path so we can import the module.
_REPO_ROOT = Path(__file__).resolve().parent.parent.parent
_SCRIPTS_DIR = _REPO_ROOT / "scripts"
sys.path.insert(0, str(_SCRIPTS_DIR))
import download_resstock_fixtures as _sut  # noqa: E402

sys.path.pop(0)


# ---------------------------------------------------------------------------
# Synthetic data helpers
# ---------------------------------------------------------------------------


def _make_parquet(data: dict[str, list]) -> bytes:
    """Write a polars DataFrame to an in-memory parquet buffer."""
    df = pl.DataFrame(data)
    buf = io.BytesIO()
    df.write_parquet(buf)
    return buf.getvalue()


def _write_parquet(tmp_path: Path, data: dict[str, list]) -> Path:
    """Write a synthetic metadata parquet to *tmp_path* and return the path."""
    p = tmp_path / "metadata.parquet"
    p.write_bytes(_make_parquet(data))
    return p


def _minimal_hpxml(elements: str = "") -> str:
    """Build a minimal HPXML document with optional nested *elements*."""
    return (
        '<?xml version="1.0"?>'
        '<HPXML xmlns="http://hpxmlonline.com/2023/09">'
        f"{elements}"
        "</HPXML>"
    )


# ---------------------------------------------------------------------------
# 1. _discover_columns
# ---------------------------------------------------------------------------


class TestDiscoverColumns:
    def test_maps_known_columns(self):
        columns = [
            "in.iecc_climate_zone",
            "in.vintage",
            "in.heating_fuel",
            "bldg_id",
            "sample_weight",
        ]
        result = _sut._discover_columns(columns)
        assert result["climate_zone"] == "in.iecc_climate_zone"
        assert result["vintage"] == "in.vintage"
        assert result["heating_fuel"] == "in.heating_fuel"

    def test_returns_none_for_missing(self):
        columns = ["bldg_id", "sample_weight"]
        result = _sut._discover_columns(columns)
        assert result["climate_zone"] is None
        assert result["building_type"] is None

    def test_prefers_first_candidate(self):
        columns = ["in.has_pv", "in.pv", "in.pv_system"]
        result = _sut._discover_columns(columns)
        assert result["pv"] == "in.has_pv"

    def test_empty_columns(self):
        result = _sut._discover_columns([])
        for v in result.values():
            assert v is None


# ---------------------------------------------------------------------------
# 2. _select_stratified_ids
# ---------------------------------------------------------------------------


class TestSelectStratifiedIds:
    def make_df_parquet(
        self, tmp_path: Path, bldg_ids: list[int], **characteristics
    ) -> tuple[Path, pl.DataFrame]:
        """Create a metadata parquet with building IDs and characteristic columns.

        *characteristics* keys are column names (e.g. "in.vintage"),
        values are lists of the same length as *bldg_ids*.
        """
        data: dict[str, list] = {"bldg_id": bldg_ids}
        for col, vals in characteristics.items():
            assert len(vals) == len(bldg_ids)
            data[col] = vals
        return _write_parquet(tmp_path, data), pl.DataFrame(data)

    def test_selects_buildings_across_dimensions(self, tmp_path: Path):
        """A basic stratified selection with known column names."""
        path, df = self.make_df_parquet(
            tmp_path,
            [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
            **{
                "in.iecc_climate_zone": [
                    "1A", "3B", "5A", "7B", "2C",  # groups 1-2, 3-4, 5-6, 7-8
                    "4A", "6C", "8A",
                    "1B", "3C", "5B", "7A",
                    "2B", "4C", "6B",
                ],
                "in.vintage": [
                    "1950s", "1940s", "2000s", "2020s", "1960s",
                    "1970s", "1990s", "2010s",
                    "pre-1940", "1980s", "2020s", "2000s",
                    "1950s", "1940s", "1970s",
                ],
                "in.heating_fuel": [
                    "natural gas", "electricity", "propane", "fuel oil", "natural gas",
                    "electricity", "propane", "fuel oil",
                    "natural gas", "electricity", "propane", "fuel oil",
                    "natural gas", "electricity", "natural gas",
                ],
            },
        )

        col_map = _sut._discover_columns(df.columns)
        selected_ids, metadata = _sut._select_stratified_ids(df, col_map, target=10, version="2024.2")

        assert len(selected_ids) >= 4  # at least climate zone groups
        assert len(metadata) == len(selected_ids)

        # Check that each building ID appears in metadata
        meta_ids = {int(m["bldg_id"]) for m in metadata.values()}
        assert set(selected_ids) == meta_ids

        # Check dimension coverage from metadata rows
        dims_found: dict[str, set[str]] = {}
        for m in metadata.values():
            d = m.get("dimension", "")
            v = m.get("value", "")
            if d and v and d != "random_fill":
                dims_found.setdefault(d, set()).add(v)

        # Climate zone groups should have at least one entry.
        assert "climate_zone_group" in dims_found
        # At least some additional dimensions should be covered — a building can
        # only be tagged with one dimension when already selected by another.
        covered_dims = set(dims_found.keys())
        assert len(covered_dims) >= 2, (
            f"Expected >= 2 dimension types covered, got {covered_dims}"
        )

    def test_metadata_dict_keyed_by_building_id(self, tmp_path: Path):
        """metadata dict keys must match selected_ids so callers can look up
        the correct dimension info per building.

        Regression: previously _select_stratified_ids returned sorted IDs with
        an unsorted metadata list, and the caller used zip() — mismatching
        IDs to wrong dimension metadata.
        """
        path, df = self.make_df_parquet(
            tmp_path,
            [1, 2, 3, 4, 5, 6, 7, 8],
            **{
                "in.has_pv": ["0", "1", "0", "1", "0", "0", "0", "0"],
                "in.heating_fuel": [
                    "natural gas", "natural gas", "propane", "natural gas",
                    "fuel oil", "natural gas", "natural gas", "natural gas",
                ],
                "in.vintage": ["2000s"] * 8,
            },
        )

        col_map = _sut._discover_columns(df.columns)
        selected_ids, metadata = _sut._select_stratified_ids(
            df, col_map, target=5, version="2024.2",
        )

        # Every selected ID must be a key in the metadata dict
        assert set(selected_ids) == set(metadata.keys())
        # Each metadata entry's bldg_id must match its dict key
        for bid in selected_ids:
            assert int(metadata[bid]["bldg_id"]) == bid
        # PV-tagged buildings must actually have PV in the source data
        pv_bldgs = {bid for bid, m in metadata.items()
                    if m.get("dimension") == "pv"}
        assert pv_bldgs.issubset({2, 4})

    def test_handles_missing_columns_gracefully(self, tmp_path: Path):
        """Selection should still produce IDs even if no stratification columns exist."""
        path, df = self.make_df_parquet(
            tmp_path,
            [1, 2, 3, 4, 5],
        )

        col_map = _sut._discover_columns(df.columns)
        selected_ids, metadata = _sut._select_stratified_ids(
            df, col_map, target=3, version="2024.2",
        )

        # All columns absent → all fills are random
        assert len(selected_ids) == 3
        for m in metadata.values():
            assert m["dimension"] == "random_fill"

    def test_no_buildings_match_stratification(self, tmp_path: Path):
        """When no building matches any stratification filter, fall back to random fill."""
        path, df = self.make_df_parquet(
            tmp_path,
            [1, 2, 3],
            **{
                "in.vintage": ["2000s", "2010s", "2020s"],
                "in.heating_fuel": ["natural gas", "natural gas", "natural gas"],
            },
        )

        col_map = _sut._discover_columns(df.columns)
        selected_ids, metadata = _sut._select_stratified_ids(
            df, col_map, target=3, version="2024.2",
        )

        assert len(selected_ids) == 3
        # Vintage 2020+ should be found; heating fuel = electricity, propane, fuel oil not found
        # But natural gas should be found. So at least some stratified matches expected.
        dims = {m["dimension"] for m in metadata.values()}
        # Should have climate zone group if available, heating fuel, vintage
        assert "heating_fuel" in dims or "vintage" in dims

    def test_target_limits_total_selection(self, tmp_path: Path):
        """The target parameter should limit the total number of selected IDs."""
        path, df = self.make_df_parquet(
            tmp_path,
            list(range(1, 101)),
            **{
                "in.vintage": ["2000s"] * 100,
                "in.heating_fuel": ["natural gas"] * 100,
            },
        )

        col_map = _sut._discover_columns(df.columns)
        selected_ids, _ = _sut._select_stratified_ids(
            df, col_map, target=5, version="2024.2",
        )

        assert len(selected_ids) <= 5

    def test_pv_and_ev_selection(self, tmp_path: Path):
        """Buildings with PV and EV flags should be selected."""
        path, df = self.make_df_parquet(
            tmp_path,
            [1, 2, 3, 4, 5, 6, 7, 8],
            **{
                "in.has_pv": [
                    "0", "1", "0", "1", "0", "0", "0", "0",
                ],
                "in.has_ev": [
                    "0", "0", "1", "0", "1", "0", "0", "0",
                ],
                "in.vintage": [
                    "2000s", "2000s", "2000s", "2000s",
                    "2000s", "2000s", "2000s", "2000s",
                ],
            },
        )

        col_map = _sut._discover_columns(df.columns)
        selected_ids, metadata = _sut._select_stratified_ids(
            df, col_map, target=6, version="2024.2",
        )

        # Check that PV-present buildings were selected
        pv_bldgs = {int(m["bldg_id"]) for m in metadata.values() if m.get("dimension") == "pv"}
        assert len(pv_bldgs) > 0
        assert pv_bldgs.issubset({2, 4})  # only 2 and 4 have PV

        # Check that EV-present buildings were selected
        ev_bldgs = {int(m["bldg_id"]) for m in metadata.values() if m.get("dimension") == "ev"}
        assert len(ev_bldgs) > 0
        assert ev_bldgs.issubset({3, 5})  # only 3 and 5 have EV

    def test_heat_pump_wh_selection(self, tmp_path: Path):
        """Buildings with heat pump water heater should be selected."""
        path, df = self.make_df_parquet(
            tmp_path,
            [1, 2, 3, 4],
            **{
                "in.water_heater_type": [
                    "electric resistance",
                    "heat pump",
                    "gas",
                    "electric resistance",
                ],
                "in.vintage": ["2000s"] * 4,
            },
        )

        col_map = _sut._discover_columns(df.columns)
        selected_ids, metadata = _sut._select_stratified_ids(
            df, col_map, target=3, version="2024.2",
        )

        hpwh_bldgs = {int(m["bldg_id"]) for m in metadata.values() if m.get("dimension") == "water_heater"}
        assert len(hpwh_bldgs) > 0
        assert hpwh_bldgs == {2}

    def test_pre_1980_vintage_selection(self, tmp_path: Path):
        """Pre-1980 vintage buildings should be selected."""
        path, df = self.make_df_parquet(
            tmp_path,
            [1, 2, 3, 4, 5, 6, 7],
            **{
                "in.vintage": [
                    "1940s", "1950s", "1960s", "1970s",  # pre-1980
                    "1980s", "2020s", "2010s",
                ],
            },
        )

        col_map = _sut._discover_columns(df.columns)
        selected_ids, metadata = _sut._select_stratified_ids(
            df, col_map, target=5, version="2024.2",
        )

        pre1980_ids = {int(m["bldg_id"]) for m in metadata.values()
                       if m.get("dimension") == "vintage" and m.get("value") == "pre-1980"}
        assert len(pre1980_ids) > 0
        assert pre1980_ids.issubset({1, 2, 3, 4})

    def test_post_2020_vintage_selection(self, tmp_path: Path):
        """2020+ vintage buildings should be selected."""
        path, df = self.make_df_parquet(
            tmp_path,
            [1, 2, 3, 4],
            **{
                "in.vintage": [
                    "2020s", "2000s", "2010s", "1990s",
                ],
            },
        )

        col_map = _sut._discover_columns(df.columns)
        selected_ids, metadata = _sut._select_stratified_ids(
            df, col_map, target=2, version="2024.2",
        )

        post2020_ids = {int(m["bldg_id"]) for m in metadata.values()
                        if m.get("dimension") == "vintage" and m.get("value") == "2020+"}
        assert len(post2020_ids) == 1
        assert post2020_ids == {1}

    def test_climate_zone_grouping(self, tmp_path: Path):
        """IECC zones should be grouped into 1-2, 3-4, 5-6, 7-8 groups."""
        path, df = self.make_df_parquet(
            tmp_path,
            list(range(1, 17)),
            **{
                "in.iecc_climate_zone": [
                    "1A", "2B", "3A", "3C", "4A", "5A", "6B", "7A",
                    "1C", "2C", "4B", "5C", "6A", "7B", "8A", "3B",
                ],
            },
        )

        col_map = _sut._discover_columns(df.columns)
        selected_ids, metadata = _sut._select_stratified_ids(
            df, col_map, target=8, version="2024.2",
        )

        groups_found = {
            m["value"] for m in metadata.values()
            if m.get("dimension") == "climate_zone_group"
        }
        assert "climate_1_2" in groups_found
        assert "climate_3_4" in groups_found
        assert "climate_5_6" in groups_found
        assert "climate_7_8" in groups_found


# ---------------------------------------------------------------------------
# 3. _verify_hpxml_equipment
# ---------------------------------------------------------------------------


class TestVerifyHpxmlEquipment:
    def test_detects_pv_system(self, tmp_path: Path):
        hpxml = _minimal_hpxml("<Building><PVSystem/></Building>")
        path = tmp_path / "home.xml"
        path.write_text(hpxml)

        results = _sut._verify_hpxml_equipment(
            path, {"dimension": "pv", "value": "present"}
        )
        assert any("PV: found" in r for r in results)

    def test_detects_missing_pv(self, tmp_path: Path):
        hpxml = _minimal_hpxml("<Building></Building>")
        path = tmp_path / "home.xml"
        path.write_text(hpxml)

        results = _sut._verify_hpxml_equipment(
            path, {"dimension": "pv", "value": "present"}
        )
        assert any("NOT FOUND" in r for r in results)

    def test_detects_ev_vehicle(self, tmp_path: Path):
        hpxml = _minimal_hpxml("<Building><Vehicle/></Building>")
        path = tmp_path / "home.xml"
        path.write_text(hpxml)

        results = _sut._verify_hpxml_equipment(
            path, {"dimension": "ev", "value": "present"}
        )
        assert any("EV: found" in r for r in results)

    def test_detects_missing_ev(self, tmp_path: Path):
        hpxml = _minimal_hpxml("<Building></Building>")
        path = tmp_path / "home.xml"
        path.write_text(hpxml)

        results = _sut._verify_hpxml_equipment(
            path, {"dimension": "ev", "value": "present"}
        )
        assert any("NOT FOUND" in r for r in results)

    def test_detects_heat_pump_wh(self, tmp_path: Path):
        hpxml = _minimal_hpxml(
            "<Building>"
            "<Systems>"
            "<WaterHeatingSystem>"
            "<WaterHeaterType>heat pump water heater</WaterHeaterType>"
            "</WaterHeatingSystem>"
            "</Systems>"
            "</Building>",
        )
        path = tmp_path / "home.xml"
        path.write_text(hpxml)

        results = _sut._verify_hpxml_equipment(
            path, {"dimension": "water_heater", "value": "heat pump"}
        )
        assert any("HPWH: found" in r for r in results)

    def test_does_not_match_non_hpwh(self, tmp_path: Path):
        hpxml = _minimal_hpxml(
            "<Building>"
            "<Systems>"
            "<WaterHeatingSystem>"
            "<WaterHeaterType>electric resistance</WaterHeaterType>"
            "</WaterHeatingSystem>"
            "</Systems>"
            "</Building>",
        )
        path = tmp_path / "home.xml"
        path.write_text(hpxml)

        results = _sut._verify_hpxml_equipment(
            path, {"dimension": "water_heater", "value": "heat pump"}
        )
        assert any("NOT FOUND" in r for r in results)

    def test_detects_heating_fuel(self, tmp_path: Path):
        hpxml = _minimal_hpxml(
            "<Building>"
            "<Systems>"
            "<HeatingSystem>"
            "<HeatingSystemFuel>natural gas</HeatingSystemFuel>"
            "</HeatingSystem>"
            "</Systems>"
            "</Building>",
        )
        path = tmp_path / "home.xml"
        path.write_text(hpxml)

        results = _sut._verify_hpxml_equipment(
            path, {"dimension": "heating_fuel", "value": "natural gas"}
        )
        assert any("found 'natural gas'" in r for r in results)

    def test_detects_heating_fuel_in_heat_pump(self, tmp_path: Path):
        """Electric heating in ResStock HPXML uses HeatPump, not HeatingSystem.

        HeatPump elements use <HeatPumpFuel> (not <HeatingSystemFuel>), so
        the verification must search both element and fuel-tag names.
        """
        hpxml = _minimal_hpxml(
            "<Building>"
            "<Systems>"
            "<HeatPump>"
            "<HeatPumpFuel>electricity</HeatPumpFuel>"
            "</HeatPump>"
            "</Systems>"
            "</Building>",
        )
        path = tmp_path / "home.xml"
        path.write_text(hpxml)

        results = _sut._verify_hpxml_equipment(
            path, {"dimension": "heating_fuel", "value": "electricity"}
        )
        assert any("found 'electricity'" in r for r in results)

    def test_bad_xml_graceful(self, tmp_path: Path):
        path = tmp_path / "home.xml"
        path.write_text("not xml <<<")

        results = _sut._verify_hpxml_equipment(
            path, {"dimension": "pv", "value": "present"}
        )
        assert any("Could not parse" in r for r in results)

    def test_no_checks_applicable(self, tmp_path: Path):
        hpxml = _minimal_hpxml("<Building></Building>")
        path = tmp_path / "home.xml"
        path.write_text(hpxml)

        results = _sut._verify_hpxml_equipment(
            path, {"dimension": "unknown_dim", "value": "x"}
        )
        assert any("No equipment checks" in r for r in results)

    def test_namespace_free_pv(self, tmp_path: Path):
        """PVSystem without XML namespace should still be detected."""
        hpxml = (
            '<?xml version="1.0"?>'
            "<HPXML>"
            "<Building>"
            "<PVSystem/>"
            "</Building>"
            "</HPXML>"
        )
        path = tmp_path / "home.xml"
        path.write_text(hpxml)

        results = _sut._verify_hpxml_equipment(
            path, {"dimension": "pv", "value": "present"}
        )
        assert any("PV: found" in r for r in results)


# ---------------------------------------------------------------------------
# 4. --help output
# ---------------------------------------------------------------------------


class TestHelpOutput:
    def test_bldg_ids_has_no_default(self):
        """--help should not show a default value for --bldg-ids."""
        import subprocess

        script = _REPO_ROOT / "scripts" / "download_resstock_fixtures.py"
        result = subprocess.run(
            [sys.executable, str(script), "--help"],
            capture_output=True,
            text=True,
        )
        help_text = result.stdout
        assert "default=" not in help_text, (
            f"--bldg-ids should have no default, but help shows: {help_text}"
        )
        assert "--bldg-ids" in help_text


# ---------------------------------------------------------------------------
# 5. _build_summary
# ---------------------------------------------------------------------------


class TestBuildSummary:
    def test_all_succeeded(self):
        summary = _sut._build_summary(
            total_attempted=3,
            succeeded=["bldg0000001", "bldg0000002", "bldg0000003"],
            failed=[],
            total_retries=0,
            mismatch_count=0,
        )
        assert "Attempted: 3" in summary
        assert "Succeeded: 3" in summary
        assert "bldg0000001" in summary
        assert "bldg0000002" in summary
        assert "bldg0000003" in summary
        assert "Failed: 0" in summary
        assert "Retries:" not in summary
        assert "SHA256 mismatches:" not in summary

    def test_some_failed(self):
        summary = _sut._build_summary(
            total_attempted=3,
            succeeded=["bldg0000002", "bldg0000003"],
            failed=[
                {"bldg_name": "bldg0000001", "version": "2024.2", "reason": "download failed"},
            ],
            total_retries=0,
            mismatch_count=0,
        )
        assert "Attempted: 3" in summary
        assert "Succeeded: 2" in summary
        assert "bldg0000002" in summary
        assert "bldg0000003" in summary
        assert "Failed: 1" in summary
        assert "bldg0000001 (2024.2): download failed" in summary

    def test_all_failed(self):
        summary = _sut._build_summary(
            total_attempted=3,
            succeeded=[],
            failed=[
                {"bldg_name": "bldg0000001", "version": "2024.2", "reason": "download failed"},
                {"bldg_name": "bldg0000002", "version": "2024.2", "reason": "download failed"},
                {"bldg_name": "bldg0000003", "version": "2024.2", "reason": "download failed"},
            ],
            total_retries=0,
            mismatch_count=0,
        )
        assert "Attempted: 3" in summary
        assert "Succeeded: 0" in summary
        assert "Failed: 3" in summary
        assert "bldg0000001 (2024.2): download failed" in summary

    def test_includes_retries(self):
        summary = _sut._build_summary(
            total_attempted=2,
            succeeded=["bldg0000002", "bldg0000003"],
            failed=[],
            total_retries=5,
            mismatch_count=0,
        )
        assert "Retries: 5" in summary

    def test_includes_mismatches(self):
        summary = _sut._build_summary(
            total_attempted=2,
            succeeded=["bldg0000002", "bldg0000003"],
            failed=[],
            total_retries=0,
            mismatch_count=3,
        )
        assert "SHA256 mismatches: 3" in summary

    def test_zero_retries_not_shown(self):
        summary = _sut._build_summary(
            total_attempted=1,
            succeeded=["bldg0000001"],
            failed=[],
            total_retries=0,
            mismatch_count=0,
        )
        assert "Retries:" not in summary
        assert "SHA256 mismatches:" not in summary

    def test_empty_succeeded_shows_none(self):
        summary = _sut._build_summary(
            total_attempted=1,
            succeeded=[],
            failed=[{"bldg_name": "bldg0000001", "version": "2024.2", "reason": "download failed"}],
            total_retries=0,
            mismatch_count=0,
        )
        assert "Succeeded: 0 (none)" in summary

    def test_multiple_versions_in_failures(self):
        summary = _sut._build_summary(
            total_attempted=2,
            succeeded=[],
            failed=[
                {"bldg_name": "bldg0000001", "version": "2024.2", "reason": "download failed"},
                {"bldg_name": "bldg0000007", "version": "2025.1", "reason": "SHA256 verification failed"},
            ],
            total_retries=1,
            mismatch_count=1,
        )
        assert "bldg0000001 (2024.2): download failed" in summary
        assert "bldg0000007 (2025.1): SHA256 verification failed" in summary
        assert "Retries: 1" in summary
        assert "SHA256 mismatches: 1" in summary


# ---------------------------------------------------------------------------
# 6. _should_exit_with_error
# ---------------------------------------------------------------------------


class TestShouldExitWithError:
    def test_zero_attempts_returns_false(self):
        assert _sut._should_exit_with_error(0, 0) is False

    def test_no_failures_returns_false(self):
        assert _sut._should_exit_with_error(0, 10) is False

    def test_exactly_50_percent_returns_false(self):
        assert _sut._should_exit_with_error(5, 10) is False

    def test_more_than_50_percent_returns_true(self):
        assert _sut._should_exit_with_error(6, 10) is True

    def test_all_failed_returns_true(self):
        assert _sut._should_exit_with_error(10, 10) is True

    def test_single_failure_out_of_three_returns_false(self):
        assert _sut._should_exit_with_error(1, 3) is False

    def test_two_failures_out_of_three_returns_true(self):
        assert _sut._should_exit_with_error(2, 3) is True
