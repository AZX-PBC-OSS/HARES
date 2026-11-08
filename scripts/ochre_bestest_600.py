"""Run BESTEST Case 600 in OCHRE and compare annual loads to ASHRAE 140 bands.

Instantiates an OCHRE ``Envelope`` with boundary materials matching
``tests/fixtures/bestest/600.toml`` exactly, loads the identical Denver TMY3
EPW weather file, runs an annual simulation with ideal HVAC (thermostat at
20 C heating / 27 C cooling, zero deadband), and compares the resulting
annual heating and cooling loads against the ASHRAE 140-2023 Annex B8
Tables B8-1 and B8-2 reference bands for Case 600.

The script uses OCHRE's full envelope pipeline: multi-node RC network with
material lookups from custom CSV files, TARP interior / DOE-2 exterior film
resistances (convection-only, OCHRE's standard convention), exterior
longwave radiation, interior longwave radiation exchange between surfaces,
and pvlib-based solar irradiance on each oriented surface.

Ideal HVAC is implemented using OCHRE's ``solve_for_inputs`` method -- the
same method OCHRE's own HVAC equipment uses -- to solve for the heat
injection that maintains the zone temperature at the setpoint at each
timestep.

The comparison's scope is stated in ``COMPARISON_LIMITATIONS`` and returned
with every metrics dict: OCHRE cannot represent the case's 60/40
radiant/convective internal-gain split (the gains enter at the zone air
node only), its weather alignment is OCHRE's own one-hour lag, its ground
series is the DOE-2 GTEMP correlation, and its annual weather index cannot
represent the fixture's leap-year start. The measured consequences and the
verdict are recorded in
``docs/ochre-bestest-600-comparison-diagnosis.md``: the runner is not an
acceptance oracle for the Annex B8 bands, and the bands are not adjusted
for it.

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
import csv
import datetime as dt
import io
import sys
import tempfile
import tomllib
from pathlib import Path
from typing import Any, cast

import pandas as pd

# Ensure vendored OCHRE is importable
_ROOT = Path(__file__).resolve().parents[1]
_VENDOR_OCHRE = _ROOT / "vendors" / "OCHRE"
if str(_VENDOR_OCHRE) not in sys.path:
    sys.path.insert(0, str(_VENDOR_OCHRE))
_TESTS_PYTHON = _ROOT / "tests" / "python"
if str(_TESTS_PYTHON) not in sys.path:
    sys.path.insert(0, str(_TESTS_PYTHON))

from ochre.Models.Envelope import Envelope  # noqa: E402
from ochre.utils import envelope as env_utils  # noqa: E402
from ochre.utils.schedule import import_weather, resample_and_reindex  # noqa: E402
import ochre.utils.units  # noqa: E402
from ochre_units import register_removed_units  # noqa: E402

# OCHRE's infiltration model converts with a unit name pint 0.25 removed from
# the default registry; re-register it before any dwelling is built.
register_removed_units(ochre.utils.units.ureg)


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
# ASHRAE 140-2017 Addendum a (2020) Annex B6.1 Table B6-1 — WINDOW 7 program output
# for the updated BESTEST Case 600 window (double-pane clear glass, 3.048 mm panes,
# 12 mm air gap, tauSol=0.834, rhoSol=0.075 per pane). The WINDOW 7 computation
# reports SHGCc = 0.769 at normal incidence (0°). The LBNL Modelica Buildings
# library (Win600/Glass600 records) defines only the layer construction properties
# and does not publish the derived whole-window SHGC.
WINDOW_SHGC = 0.769  # ASHRAE 140-2017 Addendum a (2020) Annex B6.1 Table B6-1, WINDOW 7 SHGCc

# Roof: flat, 48 m2
ROOF_AREA_M2 = 48.0
ROOF_TILT_DEG = 0.0

# Floor: over crawlspace, 48 m2, exterior zone = Ground
FLOOR_AREA_M2 = 48.0
FLOOR_TILT_DEG = 180.0  # inverted horizontal (interior above, ground below)

# Simulation parameters (matching 600.toml)
TIMESTEP_S = 3600
# The warmup: the script's fixed 24 h stand-in for the fixture's "default
# converging warm-up (the EnergyPlus criteria)". The diagnosis's one-factor
# arms measure the stand-in: extending the warmup from 24 h to 168 h or 336
# h moves the annual load by -0.02 kWh, so the state initialization is
# converged; the consumed-from-the-year window is the treatment's real
# consequence (see the warmup_treatment limitation below).
WARMUP_HOURS = 24
# The schedule's year label. The fixture's [simulation] table starts in leap
# 2024; OCHRE's annual weather index cannot represent a leap year
# (``set_annual_index`` raises "Cannot parse data for a leap year"), so the
# script pins a non-leap label. The label only shifts the solar-position
# season phase on the same 8760 TMY3 rows: one-factor arms at 2022 and 2021
# move the annual heating load by +0.34 and +0.77 kWh against the 110 kWh
# excess (docs/ochre-bestest-600-comparison-diagnosis.md).
SIM_YEAR = 2023  # non-leap year for 365 days = 8760 hours

# HVAC setpoints (matching 600.toml)
HEATING_SETPOINT_C = 20.0
COOLING_SETPOINT_C = 27.0
DEADBAND_C = 0.0  # 600.toml specifies deadband_c = 0.0

# Infiltration (matching 600.toml)
INFILTRATION_ACH = 0.5

# Internal gains (matching 600.toml): 200 W, 100% sensible. The case's
# radiant split is the standard's own case input: ASHRAE 140-2017 Addendum
# a Section 5.2.1.7 (carried by 140-2023 Section 7's fabric specification)
# declares the 200 W load 60% radiative and 40% convective, and its
# informative note names the load "equipment, lights, people, etc." -- the
# adjudication is docs/PHYSICS_DECISIONS.md, "The BESTEST internal-gain
# radiant fraction is the standard's case input", whose artifact record
# (the BESTEST-GSR reference implementation's 200 W / Fraction Radiant 0.6
# object) refutes this script's earlier claim that the 60% figure was a
# misattribution of an E+ v9.6 IDF 0.3 value.
#
# OCHRE cannot represent that split: its equipment model hardcodes
# "Radiative Gain Fraction (-)": 0 (utils/hpxml.py:1567) and folds
# convective plus radiative into one sensible fraction at the zone air node
# (Equipment.py:80-87, with the FUTURE comment acknowledging the missing
# radiation path), and the Envelope schedule path injects "Internal Gains
# (W)" straight into ``zone.internal_sens_gain`` (Models/Envelope.py:1274),
# which update_model adds at the air node with no surface term. The runner
# therefore reports OCHRE's 0%-radiant representation of the case; the
# limitation is declared in COMPARISON_LIMITATIONS below and disqualifies
# this comparison as an acceptance oracle (the diagnosis report's verdict).
INTERNAL_GAINS_W = 200.0

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

# ASHRAE 140-2023 Annex B8, Tables B8-1 and B8-2: Case 600 annual load
# reference bands (kWh)
ASHRAE_140_BANDS = {
    "annual_heating_load_kwh": (3993.0, 4504.0),
    "annual_cooling_load_kwh": (5432.0, 6162.0),
}

# What this comparison is, stated on the record. The runner is an
# OCHRE-side reference construction, not an acceptance oracle for the
# Annex B8 bands: the limitations below are OCHRE's own model and weather
# semantics, and none of them is a licensed tolerance -- the band
# assertions stay strict and the diagnosis report holds the measured
# attribution. Each entry states the mismatch and its measured annual
# leverage (the one-factor arms in
# docs/ochre-bestest-600-comparison-diagnosis.md).
COMPARISON_LIMITATIONS = {
    "internal_gains_radiant_fraction": (
        "The case's 60% radiative / 40% convective split (ASHRAE 140-2017 "
        "Addendum a Section 5.2.1.7; docs/PHYSICS_DECISIONS.md) is not "
        "representable in OCHRE: the gains enter at the zone air node only "
        "(Radiative Gain Fraction hardcoded 0 in utils/hpxml.py:1567; the "
        "Envelope schedule path folds everything into "
        "zone.internal_sens_gain, Models/Envelope.py:1274). OCHRE runs the "
        "case at 0% radiant. Unrepresentable rather than measured: no "
        "OCHRE input can carry the split."
    ),
    "weather_alignment": (
        "OCHRE's schedule path places EPW row n-1 at step n (row 0 doubled "
        "at steps 0 and 1, the final row dropped): set_annual_index stamps "
        "the hour-ending rows at :30 and resample_and_reindex bins them to "
        "the next hour. The engine and the EPW convention read row n at "
        "step n. One-factor arm (hour-beginning alignment): -0.43 kWh "
        "heating, -19.15 kWh cooling."
    ),
    "calendar_year_label": (
        "SIM_YEAR 2023 while the fixture starts in leap 2024: OCHRE's "
        "annual weather index cannot represent a leap year (set_annual_index "
        "raises), and the label only shifts the solar-position season phase "
        "on the same TMY3 rows. One-factor arms (2022, 2021): +0.34 and "
        "+0.77 kWh heating."
    ),
    "warmup_treatment": (
        "A fixed 24 h warmup consumed from the year's head: the reported "
        "window is 8736 h (Jan 2 through Dec 31), not the weather year's "
        "8760 h. The fixture names the default converging warm-up. "
        "One-factor arms (the warmup prepended, the full year reported): "
        "+36.88 kWh heating; extending the prepended warmup from 24 h to "
        "168 h and 336 h moves the annual load by -0.02 kWh, so the 24 h "
        "state initialization is converged and the window is the whole "
        "effect."
    ),
    "ground_series": (
        "The floor's Ground zone follows OCHRE's DOE-2 GTEMP correlation "
        "(the dry-bulb-derived sinusoid, damped to about 55% of the "
        "monthly half-range and phase-lagged: measured mean 10.82 C, "
        "4.62 to 17.10 C). The engine derives mean 10.875 C, amplitude "
        "11.576 C, phase day 35. One-factor arm (the engine's series): "
        "+15.71 kWh heating, +9.62 kWh cooling."
    ),
    "verdict": (
        "No admissible input correction closes the 110 kWh heating excess "
        "against the 4504 kWh band ceiling: the identified corrections "
        "net +34.6 kWh (they worsen it). The residual is the comparator's "
        "own model semantics -- the unrepresentable radiant split first of "
        "all -- so this runner is not an acceptance oracle for the bands; "
        "it is retained as a reference tool only, the strict band "
        "assertions stay as written, and the bands are not adjusted. See "
        "docs/ochre-bestest-600-comparison-diagnosis.md."
    ),
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


def _compare_c_wall() -> None:
    """Print diagnostic comparing analytically-computed wall C_wall against
    the value derived from reading ``tests/fixtures/bestest/600.toml`` at
    runtime.  A non-zero delta signals that the script's material constants
    have drifted from the canonical HARES fixture.
    """
    toml_path = _ROOT / "tests" / "fixtures" / "bestest" / "600.toml"
    if not toml_path.is_file():
        print(f"[c_wall diag] WARNING: fixture not found at {toml_path}")
        return

    with open(toml_path, "rb") as fh:
        fixture = tomllib.load(fh)

    # Analytical C_wall derived by parsing _MATERIALS_CSV — the same data
    # OCHRE consumes to build the simulation's wall assembly.  Parsing the
    # CSV directly ensures this diagnostic catches drift between the CSV
    # and the fixture even when one is edited without the other.
    reader = csv.DictReader(io.StringIO(_MATERIALS_CSV))
    analytical_c_wall = sum(
        float(row["Thickness (m)"])
        * float(row["Density (kg/m^3)"])
        * float(row["Specific Heat (J/kg-K)"])
        for row in reader
        if row["Boundary Name"] == "Exterior Wall"
    )  # J/(m²·K)

    # TOML-derived C_wall: sum thickness × density × cp for the south-wall
    # material layers (the fixture's first boundary is the south wall).
    toml_c_wall = None
    boundaries = fixture.get("boundaries", [])
    for boundary in boundaries:
        if boundary.get("id") == "south-wall":
            toml_c_wall = 0.0
            for layer in boundary.get("material_layers", []):
                toml_c_wall += (
                    layer["thickness_m"]
                    * layer["density_kg_m3"]
                    * layer["specific_heat_j_kg_k"]
                )
            break

    if toml_c_wall is None:
        print("[c_wall diag] WARNING: south-wall not found in fixture")
        return

    delta = analytical_c_wall - toml_c_wall
    delta_pct = 100.0 * delta / toml_c_wall if toml_c_wall != 0.0 else 0.0
    print(
        f"[c_wall diag] analytical C_wall = {analytical_c_wall:.1f} J/(m²·K)"
    )
    print(
        f"[c_wall diag] TOML-derived C_wall = {toml_c_wall:.1f} J/(m²·K)"
    )
    print(
        f"[c_wall diag] delta = {delta:+.1f} J/(m²·K) ({delta_pct:+.3f}%)"
    )
    if abs(delta_pct) > 0.01:
        print(
            "[c_wall diag] WARNING: analytical and fixture C_wall differ"
            " — OCHRE material CSV may have drifted from 600.toml"
        )


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
    weather_tz = cast(pd.DatetimeIndex, weather_df.index).tzinfo

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
            # The R here only picks the CSV's row when a boundary name has
            # several types; OCHRE itself adds its film R to the CSV's
            # assembly R, and "Floor" is not in OCHRE's 1.5 film list
            # (utils/envelope.py get_boundary_rc_values), so the floor's
            # lookup film is 0.9 like the walls' and roof's.
            "Boundary R Value": 25.254 + 0.9,
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

        # The vendored Zone's own HVAC equipment writes float sensible gains
        # into this attribute at runtime (HVAC.py:565), so bridge past the
        # int-typed initializer with a dynamic write.
        zone_any = cast(Any, zone)
        zone_any.hvac_sens_gain = hvac_gain
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
        "comparison_limitations": dict(COMPARISON_LIMITATIONS),
    }

    return metrics


# ---------------------------------------------------------------------------
# Main entry point
# ---------------------------------------------------------------------------


def _build_schedule_for_year() -> tuple[pd.DataFrame, dict, dt.datetime, dt.timedelta, dt.timedelta]:
    """The runner's schedule construction: the weather year, the location's
    film averages, the simulation start, the duration, and the time
    resolution. One construction path for the runner and the preflight
    witnesses.
    """
    start_time = dt.datetime(SIM_YEAR, 1, 1)
    duration = dt.timedelta(hours=8760)  # 365 days
    time_res = dt.timedelta(seconds=TIMESTEP_S)

    weather_df, location = import_weather(
        weather_file=EPW_PATH,
        start_time=start_time,
    )

    # Adopt the weather timezone for the simulation start time,
    # matching OCHRE's Dwelling.__init__ pattern (Dwelling.py:87).
    weather_tz = cast(pd.DatetimeIndex, weather_df.index).tzinfo
    sim_start = start_time.replace(tzinfo=weather_tz)

    boundaries = _build_boundaries()

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
    return schedule, loc_for_envelope, sim_start, duration, time_res


def _build_envelope(
    schedule: pd.DataFrame,
    loc_for_envelope: dict,
    sim_start: dt.datetime,
    duration: dt.timedelta,
    time_res: dt.timedelta,
    csv_dir: Path,
) -> Envelope:
    """The runner's Envelope construction with the full boundary pipeline."""
    boundaries_csv, boundary_types_csv, materials_csv = _write_material_csvs(csv_dir)
    return Envelope(
        zones={
            "Indoor": {
                "Volume (m^3)": ZONE_VOLUME_M3,
                "Infiltration Method": "ACH",
                "Air Changes (1/hour)": INFILTRATION_ACH,
            },
        },
        boundaries=_build_boundaries(),
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

    # Print diagnostic: compare analytical C_wall against 600.toml at runtime
    _compare_c_wall()

    # Write custom material CSVs to a temp directory
    with tempfile.TemporaryDirectory() as tmpdir:
        # Load weather from the EPW file.
        # import_weather uses start_time.year to set the EPW data year and
        # returns a DataFrame with a timezone-aware DatetimeIndex.
        # We start at Jan 1 of the simulation year.  The first WARMUP_HOURS
        # timesteps serve as initialization (discarded from output), so the
        # reported window is 8736 h: Jan 2 00:00 through Dec 31 23:00.  The
        # diagnosis's one-factor arms measure the treatment
        # (docs/ochre-bestest-600-comparison-diagnosis.md): the state is
        # converged by 24 h and the full-year window would add 36.9 kWh to
        # the reported heating load.
        schedule, loc_for_envelope, sim_start, duration, time_res = (
            _build_schedule_for_year()
        )

        envelope = _build_envelope(
            schedule,
            loc_for_envelope,
            sim_start,
            duration,
            time_res,
            Path(tmpdir),
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
    print(
        "  This comparison is not an acceptance oracle for the bands:"
        " the internal-gain radiant split is unrepresentable in OCHRE"
        " (0% radiant) and the other stated limitations are in the"
        " metrics' comparison_limitations entry."
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
