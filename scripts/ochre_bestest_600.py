"""Run BESTEST Case 600 in OCHRE and compare annual loads to ASHRAE 140 bands.

Instantiates an OCHRE ``Envelope`` with boundary materials matching
``tests/fixtures/bestest/600.toml`` exactly, loads the identical Denver TMY3
EPW weather file, runs an annual simulation with ideal HVAC (thermostat at
20 C heating / 27 C cooling, zero deadband), and compares the resulting
annual heating and cooling loads against the ASHRAE 140-2017 Table B8-2
reference bands for Case 600.

The script uses OCHRE's full envelope pipeline: multi-node RC network with
material lookups from custom CSV files, TARP interior / DOE-2 exterior film
resistances (convection-only, OCHRE's standard convention), exterior
longwave radiation, interior longwave radiation exchange between surfaces,
and pvlib-based solar irradiance on each oriented surface.

Ideal HVAC is implemented using OCHRE's ``solve_for_inputs`` method -- the
same method OCHRE's own HVAC equipment uses -- to solve for the heat
injection that maintains the zone temperature at the setpoint at each
timestep.

Outputs:
  - Time-series CSV (``ochre_bestest_600_results.csv``): zone air
    temperature, heating load, cooling load, and outdoor temperature
    at each hourly timestep.
  - Annual summary printed to stdout: peak/min zone temps, annual
    heating/cooling loads, and ASHRAE 140 band pass/fail.

Usage::

    python scripts/ochre_bestest_600.py [--output-dir DIR]

Requires the OCHRE dependency group::

    uv sync --group ochre
"""

from __future__ import annotations

import argparse
import datetime as dt
import sys
import tempfile
from pathlib import Path

import pandas as pd

# Ensure vendored OCHRE is importable
_ROOT = Path(__file__).resolve().parents[1]
_VENDOR_OCHRE = _ROOT / "vendors" / "OCHRE"
if str(_VENDOR_OCHRE) not in sys.path:
    sys.path.insert(0, str(_VENDOR_OCHRE))

from ochre.Models.Envelope import Envelope  # noqa: E402
from ochre.utils import envelope as env_utils  # noqa: E402
from ochre.utils.schedule import import_weather, resample_and_reindex  # noqa: E402


# ---------------------------------------------------------------------------
# BESTEST Case 600 parameters -- sourced from tests/fixtures/bestest/600.toml
# ---------------------------------------------------------------------------

# Geometry: 8 m x 6 m x 2.7 m single zone
ZONE_VOLUME_M3 = 8 * 6 * 2.7  # 129.6

# Wall areas (gross wall = boundary height x width; south wall net of window)
# South wall: 8 m wide x 2.7 m tall - 12 m2 window = 9.6 m2 net
# North wall: 8 m wide x 2.7 m tall = 21.6 m2
# East wall:  6 m wide x 2.7 m tall = 16.2 m2
# West wall:  6 m wide x 2.7 m tall = 16.2 m2
WALL_AREAS_M2 = [9.6, 21.6, 16.2, 16.2]
WALL_AZIMUTHS_DEG = [180, 0, 90, 270]  # south, north, east, west

# Windows: two 6 m2 south-facing windows (12 m2 total), per 600.toml
WINDOW_AREAS_M2 = [6.0, 6.0]
WINDOW_AZIMUTHS_DEG = [180, 180]
WINDOW_U_FACTOR = 3.0  # W/m2-K, per 600.toml
WINDOW_SHGC = 0.789  # per 600.toml

# Roof: flat, 48 m2
ROOF_AREA_M2 = 48.0
ROOF_TILT_DEG = 0.0

# Floor: over crawlspace, 48 m2, exterior zone = Ground
FLOOR_AREA_M2 = 48.0
FLOOR_TILT_DEG = 180.0  # inverted horizontal (interior above, ground below)

# Simulation parameters (matching 600.toml)
TIMESTEP_S = 3600
WARMUP_HOURS = 24  # 1 day, per 600.toml initialization_duration_s = 86400
SIM_YEAR = 2023  # non-leap year for 365 days = 8760 hours

# HVAC setpoints (matching 600.toml)
HEATING_SETPOINT_C = 20.0
COOLING_SETPOINT_C = 27.0
DEADBAND_C = 0.0  # 600.toml specifies deadband_c = 0.0

# Infiltration (matching 600.toml)
INFILTRATION_ACH = 0.5

# Internal gains (matching 600.toml)
INTERNAL_GAINS_W = 200.0
# Note: OCHRE injects all internal gains convectively (Radiative Gain Fraction
# = 0, hardcoded in hpxml.py:1567).  HARES 600.toml uses 30% radiant.
# This script runs OCHRE with OCHRE's own convention (0% radiant) to produce
# OCHRE-side reference data.  The radiant-fraction discrepancy is documented
# in the review finding scr-03 Finding 3.

# Surface optical properties (matching 600.toml)
SOLAR_ABSORPTANCE = 0.6
EMITTANCE = 0.9

# Weather file -- identical EPW used by HARES
EPW_PATH = str(
    _VENDOR_OCHRE
    / "ochre"
    / "defaults"
    / "Weather"
    / "USA_CO_Denver.Intl.AP.725650_TMY3.epw"
)

# ASHRAE 140-2017 Table B8-2: Case 600 annual load reference bands (kWh)
ASHRAE_140_BANDS = {
    "annual_heating_load_kwh": (4296.0, 5709.0),
    "annual_cooling_load_kwh": (6137.0, 7964.0),
}


# ---------------------------------------------------------------------------
# Custom material CSV files for BESTEST 600 boundaries
# ---------------------------------------------------------------------------
# OCHRE looks up boundary RC parameters from CSV files.  We create custom
# files with material layers matching 600.toml exactly so OCHRE's full
# pipeline (multi-node RC, film resistances, solar irradiance, LWR) runs
# with correct materials.

_BOUNDARIES_CSV = """\
Boundary Name,Boundary Label,Exterior Zone Label,Interior Zone Label
Exterior Wall,EW,EXT,LIV
Roof,RF,EXT,LIV
Floor,FL,GND,LIV
Window,WD,EXT,LIV
"""

# Assembly R Value = material-only R (without film resistances).
# OCHRE adds film_r (0.9 for most boundaries) during lookup matching.
# Since each boundary name has exactly one type, matching always succeeds.
_BOUNDARY_TYPES_CSV = """\
Boundary Name,Boundary Type,Finish Type,Construction Type,Insulation Details,Received From,Assembly R Value
Exterior Wall,BESTEST600,,,,BESTEST,1.789
Roof,BESTEST600,,,,BESTEST,2.994
Floor,BESTEST600,,,,BESTEST,25.254
"""

# Material layers from 600.toml:
#   Wall: 9mm wood siding (k=0.140, rho=530, cp=900)
#         + 66mm insulation (k=0.040, rho=12, cp=840)
#         + 12mm plasterboard (k=0.160, rho=950, cp=840)
#   Roof: 19mm wood (k=0.140, rho=530, cp=900)
#         + 111.8mm insulation (k=0.040, rho=12, cp=840)
#         + 10mm board (k=0.160, rho=950, cp=840)
#   Floor: 1003mm insulation (k=0.040, rho=12, cp=840)
#          + 25mm wood (k=0.140, rho=530, cp=900)
# Resistance = thickness / conductivity (m2-K/W)
# Capacitance = thickness * density * specific_heat / 1000 (kJ/m2-K)
_MATERIALS_CSV = """\
Boundary Name,Boundary Type,Finish Type,Construction Type,Insulation Details,Material Name,Thickness (m),Conductivity (W/m-K),Density (kg/m^3),Specific Heat (kJ/kg-K),Resistance (m^2-K/W),Capacitance (kJ/m^2-K),Received From,Specific Heat (J/kg-K)
Exterior Wall,BESTEST600,,,,BESTEST Wood Siding,0.009,0.140,530.0,0.900,0.064286,4.293,BESTEST,900
Exterior Wall,BESTEST600,,,,BESTEST Wall Insulation,0.066,0.040,12.0,0.840,1.650000,0.665,BESTEST,840
Exterior Wall,BESTEST600,,,,BESTEST Plasterboard,0.012,0.160,950.0,0.840,0.075000,9.576,BESTEST,840
Roof,BESTEST600,,,,BESTEST Roof Wood,0.019,0.140,530.0,0.900,0.135714,9.063,BESTEST,900
Roof,BESTEST600,,,,BESTEST Roof Insulation,0.1118,0.040,12.0,0.840,2.795000,1.127,BESTEST,840
Roof,BESTEST600,,,,BESTEST Roof Board,0.010,0.160,950.0,0.840,0.062500,7.980,BESTEST,840
Floor,BESTEST600,,,,BESTEST Floor Insulation,1.003,0.040,12.0,0.840,25.075000,10.110,BESTEST,840
Floor,BESTEST600,,,,BESTEST Floor Wood,0.025,0.140,530.0,0.900,0.178571,11.925,BESTEST,900
"""


def _write_material_csvs(output_dir: Path) -> tuple[str, str, str]:
    """Write custom CSV files for BESTEST 600 materials and return their paths."""
    boundaries_path = output_dir / "bestest_boundaries.csv"
    boundary_types_path = output_dir / "bestest_boundary_types.csv"
    materials_path = output_dir / "bestest_materials.csv"

    boundaries_path.write_text(_BOUNDARIES_CSV)
    boundary_types_path.write_text(_BOUNDARY_TYPES_CSV)
    materials_path.write_text(_MATERIALS_CSV)

    return str(boundaries_path), str(boundary_types_path), str(materials_path)


# ---------------------------------------------------------------------------
# Schedule construction
# ---------------------------------------------------------------------------


def _build_schedule(
    weather_df: pd.DataFrame,
    location: dict,
    boundaries: dict,
    start_time: dt.datetime,
    duration: dt.timedelta,
    time_res: dt.timedelta,
) -> pd.DataFrame:
    """Build the full schedule DataFrame from weather, solar, and HVAC setpoints.

    ``start_time`` must be timezone-naive.  OCHRE's ``resample_and_reindex``
    strips the weather timezone when the start time is naive (matching the
    pattern in ``load_schedule``), then ``calculate_solar_irradiance``
    re-localizes using the saved timezone for solar position calculations.
    """
    # Save weather timezone before resampling strips it
    weather_tz = weather_df.index.tzinfo

    # Resample weather to simulation timestep.
    # resample_and_reindex strips the timezone when start_time is naive,
    # matching the load_schedule pattern (schedule.py:587).
    house_args = {
        "start_time": start_time,
        "duration": duration,
        "time_res": time_res,
    }
    df_weather = resample_and_reindex(weather_df, **house_args)

    # Compute solar irradiance for each exterior boundary.
    # calculate_solar_irradiance re-localizes the index with weather_tz
    # for solar position calculations, then strips it back.
    df_weather = env_utils.calculate_solar_irradiance(
        df_weather, weather_tz, location, boundaries
    )

    # Add HVAC setpoints and internal gains (constant for BESTEST)
    df_weather["HVAC Heating Setpoint (C)"] = HEATING_SETPOINT_C
    df_weather["HVAC Cooling Setpoint (C)"] = COOLING_SETPOINT_C
    df_weather["HVAC Heating Deadband (C)"] = DEADBAND_C
    df_weather["HVAC Cooling Deadband (C)"] = DEADBAND_C
    df_weather["Internal Gains (W)"] = INTERNAL_GAINS_W
    df_weather["Occupancy (Persons)"] = 0.0

    return df_weather


# ---------------------------------------------------------------------------
# Boundary definitions
# ---------------------------------------------------------------------------


def _build_boundaries() -> dict:
    """Build the OCHRE boundary properties dict for BESTEST Case 600."""
    return {
        "Exterior Wall": {
            "Exterior Zone": "Outdoor",
            "Interior Zone": "Indoor",
            "Area (m^2)": WALL_AREAS_M2,
            "Azimuth (deg)": WALL_AZIMUTHS_DEG,
            "Tilt (deg)": 90,
            "Exterior Solar Absorptivity (-)": SOLAR_ABSORPTANCE,
            "Exterior Emissivity (-)": EMITTANCE,
            "Interior Emissivity (-)": EMITTANCE,
            "Boundary R Value": 1.789 + 0.9,  # material R + approximate film R
        },
        "Roof": {
            "Exterior Zone": "Outdoor",
            "Interior Zone": "Indoor",
            "Area (m^2)": [ROOF_AREA_M2],
            "Azimuth (deg)": [0],  # flat roof, azimuth irrelevant
            "Tilt (deg)": ROOF_TILT_DEG,
            "Exterior Solar Absorptivity (-)": SOLAR_ABSORPTANCE,
            "Exterior Emissivity (-)": EMITTANCE,
            "Interior Emissivity (-)": EMITTANCE,
            "Boundary R Value": 2.994 + 0.9,
        },
        "Floor": {
            "Exterior Zone": "Ground",
            "Interior Zone": "Indoor",
            "Area (m^2)": [FLOOR_AREA_M2],
            "Azimuth (deg)": [0],
            "Tilt (deg)": FLOOR_TILT_DEG,
            "Exterior Solar Absorptivity (-)": SOLAR_ABSORPTANCE,
            "Exterior Emissivity (-)": EMITTANCE,
            "Interior Emissivity (-)": EMITTANCE,
            "Boundary R Value": 25.254 + 1.5,  # floor uses film_r=1.5 in lookup
        },
        "Window": {
            "Exterior Zone": "Outdoor",
            "Interior Zone": "Indoor",
            "Area (m^2)": WINDOW_AREAS_M2,
            "Azimuth (deg)": WINDOW_AZIMUTHS_DEG,
            "Tilt (deg)": 90,
            "U Factor (W/m^2-K)": WINDOW_U_FACTOR,
            "SHGC (-)": WINDOW_SHGC,
            "Shading Fraction (-)": 1.0,
        },
    }


# ---------------------------------------------------------------------------
# Ideal HVAC simulation
# ---------------------------------------------------------------------------


def _run_simulation(
    envelope: Envelope,
    schedule: pd.DataFrame,
    n_warmup_steps: int,
) -> pd.DataFrame:
    """Run step-by-step simulation with ideal HVAC control.

    Uses OCHRE's ``solve_for_inputs`` to compute the heat injection that
    maintains the zone temperature at the heating or cooling setpoint at
    each timestep.  This is the same method OCHRE's own HVAC equipment
    uses (HVAC.py:422).

    Returns a DataFrame with columns: Temperature - Indoor (C),
    Heating Load (W), Cooling Load (W), Outdoor Temperature (C).
    """
    zone = envelope.indoor_zone
    t_idx = zone.t_idx
    h_idx = zone.h_idx

    results = []
    total_steps = len(schedule)

    for step_idx in range(total_steps):
        # Advance schedule and prepare inputs (solar, infiltration, etc.)
        schedule_inputs = schedule.iloc[step_idx].to_dict()
        envelope.update_inputs(schedule_inputs)

        # Solve for the heat injection needed at each setpoint.
        # solve_for_inputs returns the TOTAL heat (W) at h_idx required to
        # achieve the target temperature, accounting for all inputs already
        # in inputs_init (solar, infiltration, occupancy).
        # internal_sens_gain (200 W) is NOT in inputs_init -- it gets added
        # in update_model.  So:
        #   hvac_sens_gain = u_desired - internal_sens_gain
        internal_gains = zone.internal_sens_gain  # 200 W from schedule

        h_heating = envelope.solve_for_inputs(
            t_idx, [h_idx], HEATING_SETPOINT_C
        )
        h_cooling = envelope.solve_for_inputs(
            t_idx, [h_idx], COOLING_SETPOINT_C
        )

        if h_heating > internal_gains:
            # Free-floating temp would be below heating setpoint
            hvac_gain = h_heating - internal_gains
            heating_w = hvac_gain
            cooling_w = 0.0
        elif h_cooling < internal_gains:
            # Free-floating temp would be above cooling setpoint
            hvac_gain = h_cooling - internal_gains
            heating_w = 0.0
            cooling_w = -hvac_gain
        else:
            # Within deadband -- no HVAC needed
            hvac_gain = 0.0
            heating_w = 0.0
            cooling_w = 0.0

        zone.hvac_sens_gain = hvac_gain
        envelope.update_model()
        envelope.update_results()

        # Collect results
        t_zone = zone.temperature
        t_out = schedule_inputs.get("Ambient Dry Bulb (C)", float("nan"))

        results.append(
            {
                "Time": schedule.index[step_idx],
                "Temperature - Indoor (C)": t_zone,
                "Heating Load (W)": heating_w,
                "Cooling Load (W)": cooling_w,
                "Outdoor Temperature (C)": t_out,
            }
        )

    df = pd.DataFrame(results).set_index("Time")

    # Remove warmup period from output
    if n_warmup_steps > 0:
        df = df.iloc[n_warmup_steps:]

    return df


# ---------------------------------------------------------------------------
# ASHRAE 140 band comparison
# ---------------------------------------------------------------------------


def _compare_to_bands(df: pd.DataFrame) -> dict:
    """Compute annual metrics and compare to ASHRAE 140 reference bands."""
    dt_hours = TIMESTEP_S / 3600.0

    heating_kwh = (df["Heating Load (W)"] / 1000.0 * dt_hours).sum()
    cooling_kwh = (df["Cooling Load (W)"] / 1000.0 * dt_hours).sum()
    peak_temp = df["Temperature - Indoor (C)"].max()
    min_temp = df["Temperature - Indoor (C)"].min()

    h_min, h_max = ASHRAE_140_BANDS["annual_heating_load_kwh"]
    c_min, c_max = ASHRAE_140_BANDS["annual_cooling_load_kwh"]

    metrics = {
        "annual_heating_load_kwh": heating_kwh,
        "annual_cooling_load_kwh": cooling_kwh,
        "peak_zone_temp_c": peak_temp,
        "min_zone_temp_c": min_temp,
        "heating_in_band": h_min <= heating_kwh <= h_max,
        "cooling_in_band": c_min <= cooling_kwh <= c_max,
        "heating_band": f"[{h_min}, {h_max}]",
        "cooling_band": f"[{c_min}, {c_max}]",
    }

    return metrics


# ---------------------------------------------------------------------------
# Main entry point
# ---------------------------------------------------------------------------


def run_bestest_600(output_dir: str | Path | None = None) -> dict:
    """Run the OCHRE BESTEST Case 600 simulation.

    Args:
        output_dir: Directory for output CSV.  Defaults to CWD.

    Returns:
        Dict with annual metrics and band comparison results.
    """
    if output_dir is None:
        output_dir = Path.cwd()
    else:
        output_dir = Path(output_dir)
    output_dir.mkdir(parents=True, exist_ok=True)

    # Write custom material CSVs to a temp directory
    with tempfile.TemporaryDirectory() as tmpdir:
        boundaries_csv, boundary_types_csv, materials_csv = (
            _write_material_csvs(Path(tmpdir))
        )

        # Load weather from the EPW file.
        # import_weather uses start_time.year to set the EPW data year and
        # returns a DataFrame with a timezone-aware DatetimeIndex.
        # We start at Jan 1 of the simulation year.  The first WARMUP_HOURS
        # timesteps serve as initialization (discarded from output).  For a
        # lightweight building (time constant ~1 h), 24 h of warmup is more
        # than sufficient per ASHRAE 140 Section 5.2.1.
        start_time = dt.datetime(SIM_YEAR, 1, 1)
        duration = dt.timedelta(hours=8760)  # 365 days
        time_res = dt.timedelta(seconds=TIMESTEP_S)

        weather_df, location = import_weather(
            weather_file=EPW_PATH,
            start_time=start_time,
        )

        # Adopt the weather timezone for the simulation start time,
        # matching OCHRE's Dwelling.__init__ pattern (Dwelling.py:87).
        weather_tz = weather_df.index.tzinfo
        sim_start = start_time.replace(tzinfo=weather_tz)

        # Build boundaries dict
        boundaries = _build_boundaries()

        # Build schedule (weather + solar + HVAC setpoints + internal gains)
        schedule = _build_schedule(
            weather_df,
            location,
            boundaries,
            start_time,  # naive -- resample_and_reindex strips tz
            duration,
            time_res,
        )

        # Build location dict for film resistances.
        # import_weather already computed averages and added them to location.
        loc_for_envelope = {
            "Average Wind Speed (m/s)": location.get(
                "Average Wind Speed (m/s)", 4.02
            ),
            "Average Ambient Temperature (C)": location.get(
                "Average Ambient Temperature (C)", 10.0
            ),
            "Average Ground Temperature (C)": location.get(
                "Average Ground Temperature (C)", 10.0
            ),
        }

        # Create the Envelope with full boundary pipeline
        envelope = Envelope(
            zones={
                "Indoor": {
                    "Volume (m^3)": ZONE_VOLUME_M3,
                    "Infiltration Method": "ACH",
                    "Air Changes (1/hour)": INFILTRATION_ACH,
                },
            },
            boundaries=boundaries,
            location=loc_for_envelope,
            external_radiation_method="full",
            internal_radiation_method="full",
            schedule=schedule,
            initial_schedule=schedule.iloc[0].to_dict(),
            initial_temp_setpoint=HEATING_SETPOINT_C,
            start_time=sim_start,
            duration=duration,
            time_res=time_res,
            verbosity=5,
            save_results=False,
            main_sim_name="",
            boundaries_file=boundaries_csv,
            boundary_types_file=boundary_types_csv,
            materials_file=materials_csv,
        )

        # Run the simulation
        n_warmup = WARMUP_HOURS
        df = _run_simulation(envelope, schedule, n_warmup)

    # Save time-series CSV
    csv_path = output_dir / "ochre_bestest_600_results.csv"
    df.to_csv(csv_path, index=True, index_label="Time")

    # Compute annual metrics and compare to ASHRAE 140 bands
    metrics = _compare_to_bands(df)

    # Print summary
    print("\n=== OCHRE BESTEST Case 600 Results ===")
    print(f"  Simulation period: {df.index[0]} to {df.index[-1]}")
    print(f"  Timesteps: {len(df)}")
    print(f"  Peak zone temp:   {metrics['peak_zone_temp_c']:.2f} C")
    print(f"  Min zone temp:    {metrics['min_zone_temp_c']:.2f} C")
    print(
        f"  Annual heating:   {metrics['annual_heating_load_kwh']:.0f} kWh "
        f"(ASHRAE 140 band {metrics['heating_band']}) "
        f"-> {'PASS' if metrics['heating_in_band'] else 'FAIL'}"
    )
    print(
        f"  Annual cooling:   {metrics['annual_cooling_load_kwh']:.0f} kWh "
        f"(ASHRAE 140 band {metrics['cooling_band']}) "
        f"-> {'PASS' if metrics['cooling_in_band'] else 'FAIL'}"
    )
    print(f"\n  Results CSV: {csv_path}")

    return metrics


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Run OCHRE BESTEST Case 600 and compare to ASHRAE 140 bands."
    )
    parser.add_argument(
        "--output-dir",
        default=None,
        help="Directory for output CSV (default: current directory)",
    )
    args = parser.parse_args()
    run_bestest_600(args.output_dir)


if __name__ == "__main__":
    main()
