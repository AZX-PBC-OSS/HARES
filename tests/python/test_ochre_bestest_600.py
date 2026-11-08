"""Integration test: OCHRE BESTEST Case 600 runner script.

Runs the completed OCHRE BESTEST runner and verifies that:
  1. The script produces a time-series CSV with the expected columns.
  2. Zone temperature stays within the thermostat setpoints (20-27 C).
  3. Annual heating and cooling loads are computed and comparable to
     ASHRAE 140-2023 Annex B8 Tables B8-1 and B8-2 reference bands.

Requires the OCHRE dependency group::

    uv sync --group ochre
"""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
SCRIPTS_DIR = ROOT / "scripts"

# Ensure the scripts directory is importable
if str(SCRIPTS_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPTS_DIR))


@pytest.mark.slow
def test_ochre_bestest_600_produces_valid_results(tmp_path: Path) -> None:
    """Run the OCHRE BESTEST 600 runner and validate output."""
    from ochre_bestest_600 import ASHRAE_140_BANDS, run_bestest_600

    metrics = run_bestest_600(output_dir=tmp_path)

    # 1. CSV file was produced
    csv_path = tmp_path / "ochre_bestest_600_results.csv"
    assert csv_path.exists(), "Results CSV was not produced"

    # 2. Annual loads are positive and finite
    heating = metrics["annual_heating_load_kwh"]
    cooling = metrics["annual_cooling_load_kwh"]
    assert heating > 0, f"Heating load must be positive, got {heating}"
    assert cooling > 0, f"Cooling load must be positive, got {cooling}"
    assert heating < 100000, f"Heating load implausibly high: {heating}"
    assert cooling < 100000, f"Cooling load implausibly high: {cooling}"

    # 3. Zone temperature stayed within thermostat bounds
    peak = metrics["peak_zone_temp_c"]
    mn = metrics["min_zone_temp_c"]
    assert mn >= 19.9, f"Zone temp dropped below heating setpoint: {mn}"
    assert peak <= 27.1, f"Zone temp exceeded cooling setpoint: {peak}"

    # 4. Annual heating load is within or near the ASHRAE 140 band
    h_min, h_max = ASHRAE_140_BANDS["annual_heating_load_kwh"]
    assert h_min <= heating <= h_max, (
        f"Annual heating load {heating:.0f} kWh outside ASHRAE 140 "
        f"band [{h_min}, {h_max}] kWh"
    )

    # 5. Annual cooling load should be within a tight range of the band.
    # Measured actual is 5746.6 kWh, 105.8% of the 5432 kWh band minimum
    # (inside the band). The +-15% allowance stays from the comparison's
    # original contract: it admits OCHRE's weather-alignment and film
    # semantics without licensing any band change. A ±15% band is tight
    # enough to catch a meaningful regression (e.g. broken solar gains,
    # wrong RC network) while allowing headroom for weather-year
    # variation.
    c_min, c_max = ASHRAE_140_BANDS["annual_cooling_load_kwh"]
    cooling_ratio = cooling / c_min
    assert 0.85 <= cooling_ratio <= 1.15, (
        f"Annual cooling load {cooling:.0f} kWh is {cooling_ratio:.1%} of "
        f"ASHRAE 140 band minimum {c_min} kWh -- expected 85-115% for "
        f"OCHRE's convection-only film model"
    )


@pytest.mark.slow
def test_ochre_bestest_600_csv_has_expected_columns(tmp_path: Path) -> None:
    """Verify the time-series CSV contains the required output columns."""
    import pandas as pd

    from ochre_bestest_600 import run_bestest_600

    run_bestest_600(output_dir=tmp_path)

    csv_path = tmp_path / "ochre_bestest_600_results.csv"
    df = pd.read_csv(csv_path, index_col="Time", parse_dates=True)

    expected_cols = {
        "Temperature - Indoor (C)",
        "Heating Load (W)",
        "Cooling Load (W)",
        "Outdoor Temperature (C)",
    }
    assert expected_cols.issubset(set(df.columns)), (
        f"CSV missing columns. Expected {expected_cols}, got {set(df.columns)}"
    )

    # Should have approximately 8736 timesteps (364 days after 1-day warmup)
    assert len(df) == 8736, f"Expected 8736 timesteps, got {len(df)}"

    # No NaN values in critical columns
    for col in expected_cols:
        assert not df[col].isna().any(), f"NaN values in column {col}"


# ---------------------------------------------------------------------------
# Preflight witnesses: the comparison's declared input mismatches, pinned so
# the runner and any factor study over it cannot silently change calendar,
# duration, weather identity, or gains representation. These are contracts
# of the current construction; the measured attribution lives in
# docs/ochre-bestest-600-comparison-diagnosis.md.
# ---------------------------------------------------------------------------


def _load_script():
    scripts_dir = ROOT / "scripts"
    if str(scripts_dir) not in sys.path:
        sys.path.insert(0, str(scripts_dir))
    import ochre_bestest_600

    return ochre_bestest_600


def _load_fixture() -> dict:
    import tomllib

    fixture_path = ROOT / "tests" / "fixtures" / "bestest" / "600.toml"
    with open(fixture_path, "rb") as fh:
        return tomllib.load(fh)


@pytest.mark.ochre
def test_bands_stay_the_annex_b8_byte_pinned_values() -> None:
    """The reference bands are the standard's published values, untouched."""
    from ochre_bestest_600 import ASHRAE_140_BANDS

    assert ASHRAE_140_BANDS == {
        "annual_heating_load_kwh": (3993.0, 4504.0),
        "annual_cooling_load_kwh": (5432.0, 6162.0),
    }, (
        "the Annex B8 Case 600 bands moved: every moved value needs its named discrepancy"
    )


@pytest.mark.ochre
def test_input_identity_against_the_fixture_toml() -> None:
    """The script's OCHRE construction matches 600.toml item by item."""
    import csv as csv_mod
    import io as io_mod

    script = _load_script()
    fixture = _load_fixture()

    # Gains, setpoints, deadband, infiltration, timestep, duration, volume.
    assert fixture["internal_gains_w"] == script.INTERNAL_GAINS_W
    assert fixture["internal_gains_sensible_fraction"] == 1.0
    assert fixture["internal_gains_radiant_fraction"] == 0.6
    assert fixture["setpoints"]["heating_c"] == script.HEATING_SETPOINT_C
    assert fixture["setpoints"]["cooling_c"] == script.COOLING_SETPOINT_C
    assert fixture["simulation"]["setpoint_deadband_c"] == script.DEADBAND_C
    assert fixture["hvac"]["deadband_c"] == script.DEADBAND_C
    assert fixture["infiltration"]["ach"] == script.INFILTRATION_ACH
    assert fixture["simulation"]["time_res"] == script.TIMESTEP_S
    assert fixture["simulation"]["duration"] == 8760 * script.TIMESTEP_S
    # The script's volume is the case geometry's product 8 x 6 x 2.7; the
    # fixture rounds the same quantity to 129.6.
    assert script.ZONE_VOLUME_M3 == pytest.approx(
        fixture["geometry"]["zone_volume_m3"], rel=1e-9
    )

    # Surface optics: one absorptance and emittance pair for the solar
    # -exposed boundaries (the ground-facing floor carries none).
    for boundary in fixture["boundaries"]:
        if "solar_absorptance" in boundary:
            assert boundary["solar_absorptance"] == script.SOLAR_ABSORPTANCE
            assert boundary["emittance"] == script.EMITTANCE

    # Boundary areas and orientations: four walls, the roof, the floor.
    walls = {b["id"]: b for b in fixture["boundaries"] if b["boundary_type"] == "Wall"}
    expected_walls = {
        "south-wall": 9.6,
        "north-wall": 21.6,
        "east-wall": 16.2,
        "west-wall": 16.2,
    }
    assert {k: v["area_m2"] for k, v in walls.items()} == expected_walls
    assert [walls[k]["azimuth_deg"] for k in expected_walls] == [
        float(a) for a in script.WALL_AZIMUTHS_DEG
    ]
    roof = next(b for b in fixture["boundaries"] if b["boundary_type"] == "Roof")
    floor = next(b for b in fixture["boundaries"] if b["boundary_type"] == "Floor")
    assert (
        roof["area_m2"] == script.ROOF_AREA_M2
        and roof["tilt_deg"] == script.ROOF_TILT_DEG
    )
    assert (
        floor["area_m2"] == script.FLOOR_AREA_M2
        and floor["tilt_deg"] == script.FLOOR_TILT_DEG
    )

    # Windows: two 6 m2 south windows at U 3.0 and SHGC 0.769.
    assert len(fixture["windows"]) == len(script.WINDOW_AREAS_M2)
    for window in fixture["windows"]:
        assert window["area_m2"] in script.WINDOW_AREAS_M2
        assert window["azimuth_deg"] == 180.0
        assert window["u_factor_w_m2_k"] == script.WINDOW_U_FACTOR
        assert window["shgc"] == script.WINDOW_SHGC

    # Material layers: the CSV's resistances are the TOML layers' thickness
    # over conductivity, and the CSV's capacitances are the volumetric
    # heat capacity, boundary by boundary.
    csv_rows = list(csv_mod.DictReader(io_mod.StringIO(script._MATERIALS_CSV)))
    csv_by_boundary = {
        "Exterior Wall": walls["south-wall"],
        "Roof": roof,
        "Floor": floor,
    }
    for boundary_name, toml_boundary in csv_by_boundary.items():
        rows = [r for r in csv_rows if r["Boundary Name"] == boundary_name]
        layers = toml_boundary["material_layers"]
        assert len(rows) == len(layers), boundary_name
        for row, layer in zip(rows, layers, strict=True):
            assert float(row["Thickness (m)"]) == layer["thickness_m"]
            assert float(row["Conductivity (W/m-K)"]) == layer["conductivity_w_m_k"]
            assert float(row["Density (kg/m^3)"]) == layer["density_kg_m3"]
            assert float(row["Specific Heat (J/kg-K)"]) == layer["specific_heat_j_kg_k"]

    # The boundary types' assembly R values are the layers' R sums.
    type_rows = list(csv_mod.DictReader(io_mod.StringIO(script._BOUNDARY_TYPES_CSV)))
    r_by_name = {r["Boundary Name"]: float(r["Assembly R Value"]) for r in type_rows}
    for boundary_name, toml_boundary in csv_by_boundary.items():
        r_sum = sum(
            layer["thickness_m"] / layer["conductivity_w_m_k"]
            for layer in toml_boundary["material_layers"]
        )
        assert r_by_name[boundary_name] == pytest.approx(r_sum, abs=1e-3), boundary_name


@pytest.mark.ochre
def test_weather_file_is_byte_identical_to_the_fixture_weather() -> None:
    """The script's EPW and the fixture's epw_path are the same file."""
    import hashlib

    script = _load_script()
    fixture = _load_fixture()

    fixture_epw = (
        ROOT / "tests" / "fixtures" / "bestest" / fixture["weather"]["epw_path"]
    ).resolve()
    assert fixture_epw.is_file(), f"fixture weather missing: {fixture_epw}"

    def digest(path: Path) -> str:
        return hashlib.md5(path.read_bytes()).hexdigest()

    assert digest(Path(script.EPW_PATH)) == digest(fixture_epw), (
        "the runner's weather file differs from the fixture's"
    )


@pytest.mark.ochre
def test_ochre_annual_weather_index_cannot_represent_a_leap_year() -> None:
    """OCHRE raises on a leap start, so no arm can run the fixture's 2024."""
    import datetime as dt

    from ochre.utils.schedule import import_weather

    script = _load_script()

    with pytest.raises(Exception, match="Cannot parse data for a leap year"):
        import_weather(weather_file=script.EPW_PATH, start_time=dt.datetime(2024, 1, 1))


@pytest.mark.ochre
def test_schedule_alignment_is_ochre_one_hour_lag() -> None:
    """The schedule's step n carries EPW row n-1: row 0 doubled, row 8759 dropped.

    OCHRE's set_annual_index stamps the hour-ending rows at :30 and
    resample_and_reindex bins them to the next hour; the engine and the
    EPW convention read row n at step n. The lag is the runner's declared
    weather_alignment limitation.
    """
    import pandas as pd

    script = _load_script()
    schedule, _, _, _, _ = script._build_schedule_for_year()

    raw = pd.read_csv(script.EPW_PATH, skiprows=8, header=None, nrows=8760)
    epw_db = raw.iloc[:, 6].astype(float).to_numpy()
    sched_db = schedule["Ambient Dry Bulb (C)"].to_numpy()

    assert len(sched_db) == 8760
    assert sched_db[0] == epw_db[0], "step 0 must be the EPW's first row"
    assert sched_db[1] == epw_db[0], "EPW row 0 must double at step 1"
    assert (sched_db[1:] == epw_db[:-1]).all(), (
        "steps 1..8759 must carry EPW rows 0..8758 (the one-hour lag)"
    )
    assert sched_db[-1] == epw_db[-2], (
        "the reported year ends at the EPW's second-to-last row: the "
        "final row never enters the schedule"
    )


@pytest.mark.ochre
def test_internal_gains_enter_at_the_air_node_with_no_radiant_surface_term(
    tmp_path: Path,
) -> None:
    """OCHRE's gains path is the 0%-radiant representation the metadata declares.

    The schedule's Internal Gains (W) lands in zone.internal_sens_gain and
    no surface carries an internal-gain term: the case's 60/40 split has
    no input to travel on. The vendored source's hardcode and fold are
    drift-locked: a vendor bump that changes either line re-opens the
    diagnosis.
    """
    script = _load_script()
    schedule, loc_for_envelope, sim_start, duration, time_res = (
        script._build_schedule_for_year()
    )
    envelope = script._build_envelope(
        schedule, loc_for_envelope, sim_start, duration, time_res, tmp_path
    )

    envelope.update_inputs(schedule.iloc[0].to_dict())
    zone = envelope.indoor_zone
    assert zone.internal_sens_gain == script.INTERNAL_GAINS_W, (
        "the schedule's internal gains must reach the zone air node"
    )
    for surface in zone.surfaces:
        assert surface.internal_gain == 0, (
            "a surface carried an internal-gain term: OCHRE gained a "
            "radiant path and the 0%-radiant framing must be re-examined"
        )

    # The vendored drift locks: the hardcode and the fold, verbatim.
    hpxml_source = (
        ROOT / "vendors" / "OCHRE" / "ochre" / "utils" / "hpxml.py"
    ).read_text()
    assert '"Radiative Gain Fraction (-)": 0' in hpxml_source
    equipment_source = (
        ROOT / "vendors" / "OCHRE" / "ochre" / "Equipment" / "Equipment.py"
    ).read_text()
    assert (
        'self.sensible_gain_fraction = kwargs.get("Convective Gain Fraction (-)", 0) + kwargs.get('
        in equipment_source
    )
