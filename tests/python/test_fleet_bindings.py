from pathlib import Path

import pyarrow as pa
import pyarrow.parquet as pq
import pytest
from hares import Fleet

ROOT = Path(__file__).resolve().parents[2]
CZ4A_TYPICAL_EPW = ROOT / "tests" / "fixtures" / "parity" / "cz4a_ashp_hpwh" / "weather.epw"

_RESSTOCK_HPXML = """<?xml version="1.0" encoding="UTF-8"?>
<HPXML xmlns="http://hpxmlonline.com/2023/09" schemaVersion="4.0">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site>
          <Elevation>30.0</Elevation>
          <Latitude>41.77</Latitude>
          <Longitude>-72.69</Longitude>
          <TimeZone>
            <UTCOffset>-5.0</UTCOffset>
          </TimeZone>
        </Site>
      </BuildingSummary>
      <ClimateandRiskZones>
        <WeatherStation>
          <SystemIdentifier id="WeatherStation"/>
          <Name>G0900090</Name>
        </WeatherStation>
      </ClimateandRiskZones>
    </BuildingDetails>
  </Building>
</HPXML>"""


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


def _write_extracted_building(buildings_dir: Path, bldg_id: int) -> Path:
    """Write one building in the extracted layout from_resstock requires."""
    bldg_dir = buildings_dir / f"bldg{bldg_id:07d}"
    bldg_dir.mkdir(parents=True, exist_ok=True)
    (bldg_dir / "home.xml").write_text(_RESSTOCK_HPXML)
    (bldg_dir / "in.schedules.csv").write_text("occupants\n1\n")
    return buildings_dir


def test_fleet_from_resstock_accepts_2024_2(tmp_path: Path) -> None:
    metadata = tmp_path / "metadata.parquet"
    _write_resstock_2024_2_metadata(metadata)
    _write_extracted_building(tmp_path, 12345)
    weather_dir = tmp_path / "weather"
    weather_dir.mkdir()
    (weather_dir / "G0900090.epw").write_bytes(CZ4A_TYPICAL_EPW.read_bytes())

    fleet = Fleet.from_resstock(
        str(metadata),
        str(tmp_path),
        str(weather_dir),
        resstock_version="2024.2",
        typical_year=2018,
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
