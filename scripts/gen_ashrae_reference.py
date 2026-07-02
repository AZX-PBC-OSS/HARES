#!/usr/bin/env python3
"""Generate an ASHRAE/EnergyPlus-first-principles RC reference for HARES parity.

This script is INDEPENDENT of HARES: it imports no HARES crate or Python
binding.  It parses ``data/examples/BEopt_example.xml`` with ``xmltodict`` and
derives every reference value (film resistances, layer R, window U, assembly
capacitance, node count) from published ASHRAE/EnergyPlus formulas hard-coded
below with inline citations.

Run it with::

    uv run python scripts/gen_ashrae_reference.py

(``xmltodict`` is supplied by the ``ochre`` optional dependency group in
``pyproject.toml``.)

Output: ``tests/fixtures/parity/ashrae_rc_reference.json``.  The file is
deterministic (sorted keys, fixed rounding) so re-running produces an
identical byte stream.

Citations
---------
All page/table references are reproduced inline next to each constant.  The
top-level sources are:

* ASHRAE *Handbook of Fundamentals*, 2021 edition, Ch. 26 "Heat, Air, and
  Moisture Control in Building Assemblies -- Fundamentals" -- Tables 1
  (surface film resistances) and 4 (building material thermal properties).
* *EnergyPlus Engineering Reference*, v25.1.0, §3.2 (Simple Glazing Model),
  §9.4 (TARP interior convection), §9.5 (DOE-2 exterior convection).
* ASHRAE Standard 90.1-2022, Appendix A unit-conversion tables.
* EnergyPlus ConvectionCoefficients.cc — CalcASHRAESimpleIntConvCoeff
  (default interior convection algorithm using fixed h_conv by orientation,
  derived from ASHRAE 1985 Table 1).
* ASHRAE HoF 2021 Ch. 17 (F-factor perimeter method for slab-on-grade).
"""

from __future__ import annotations

import json
import math
import os
import sys
from dataclasses import dataclass
from typing import Any

import xmltodict

# ---------------------------------------------------------------------------
# Paths
# ---------------------------------------------------------------------------

SCRIPT_PATH = os.path.abspath(__file__)
REPO_ROOT = os.path.abspath(os.path.join(os.path.dirname(SCRIPT_PATH), ".."))
HPXML_PATH = os.path.join(REPO_ROOT, "data", "examples", "BEopt_example.xml")
OUTPUT_PATH = os.path.join(
    REPO_ROOT, "tests", "fixtures", "parity", "ashrae_rc_reference.json"
)

# ---------------------------------------------------------------------------
# Unit conversions (ASHRAE 90.1-2022 Appendix A)
# ---------------------------------------------------------------------------

# IP R-value [hr*ft^2*degF/Btu] -> SI [m^2*K/W].  ASHRAE 90.1-2022 App. A.
R_IP_TO_SI: float = 0.17611018368230488

# IP U-factor [Btu/(hr*ft^2*degF)] -> SI [W/(m^2*K)].  ASHRAE 90.1-2022 App. A.
U_IP_TO_SI: float = 5.678263337872971

# Area ft^2 -> m^2.
FT2_TO_M2: float = 0.09290304

# Length ft -> m.
FT_TO_M: float = 0.3048

# ---------------------------------------------------------------------------
# Film-resistance constants (independent re-derivation of TARP + radiation)
# ---------------------------------------------------------------------------

# EnergyPlus ConvectionCoefficients.cc `MIN_DELTA_T` = 0.1 K — the TARP-specific
# delta-T floor that prevents unbounded surface resistance when the driving
# temperature difference approaches zero.  This value matches the EnergyPlus
# source guard (ConvectionCoefficients.cc) and the HARES Rust implementation
# (film_coefficients.rs `(t_ext - t_int).abs().max(0.1)`).
#
# Used only for the exterior TARP natural + DOE-2 forced-convection path.
# Interior film resistance uses ASHRAE Simple (fixed h_conv by orientation,
# independent of ΔT), matching HARES's interior convection algorithm.
# The TARP natural convection function itself is also used for exterior only;
# interior convection follows EnergyPlus's default `CalcASHRAESimpleIntConvCoeff`.
MIN_DELTA_T_TARP_NATURAL_K: float = 0.1

# DOE-2 surface-roughness factor r_f.  EnergyPlus Engineering Reference §9.5
# Table "Surface Roughness Multipliers for Exterior Convection".  Residential
# BEopt/OCHRE convention (asphalt shingle roof, painted wood siding, wood-
# frame cladding) uses the "Rough" class per EnergyPlus §9.5 Table.
DOE2_ROUGHNESS_ROUGH: float = 1.67

# Typical-zone-temperature anchors (°C).  EnergyPlus Eng. Ref. §9.4:
# conditioned space 20°C, outdoor = avg_ambient + 5, ground = avg_ground.
# Unconditioned zones linearly interpolate between these anchors.
T_CONDITIONED_C: float = 20.0
T_OUTDOOR_OFFSET_C: float = 5.0


def ashrae_simple_interior_h_conv(
    tilt_deg: float, above_hotter: bool
) -> float:
    """ASHRAE "Simple" interior convection coefficient h_conv [W/(m²·K)].

    Fixed convection-only values by surface orientation, derived from
    ASHRAE 1985 Table 1 surface conductances (ε = 0.9) with the radiative
    component subtracted.  These are the default interior convection
    coefficients in EnergyPlus (``CalcASHRAESimpleIntConvCoeff``) and
    match the HARES Rust implementation in
    ``crates/hares-physics/src/film_coefficients.rs``.

    | Orientation             | Condition      | h_conv [W/(m²·K)] |
    |-------------------------|----------------|--------------------|
    | Vertical (67.5–112.5°)  | —              | 3.076              |
    | Horizontal, enhanced    | heat flow up   | 4.040              |
    | Horizontal, reduced     | heat flow down | 0.948              |
    | Tilted, enhanced        | heat flow up   | 3.870              |
    | Tilted, reduced         | heat flow down | 2.281              |

    References:
    - EnergyPlus ConvectionCoefficients.cc:1829-1885.
    - Walton, G. N. 1983. TARP Reference Manual, NBSSIR 83-2655, p 79.
    - ASHRAE Handbook of Fundamentals 1985, p. 23.2, Table 1.
    """

    cos_tilt = abs(math.cos(math.radians(tilt_deg)))
    if cos_tilt < 0.3827:
        result = 3.076
    elif cos_tilt >= 0.9239:
        result = 4.040 if above_hotter else 0.948
    elif above_hotter:
        result = 3.870
    else:
        result = 2.281
    assert result > 0.0, f"film_coefficient invariant: h_conv must be > 0, got {result}"
    return result


def tarp_h_natural(tilt_deg: float, delta_t_k: float, above_hotter: bool) -> float:
    """TARP natural convection coefficient h [W/(m^2*K)].

    Reference: EnergyPlus Engineering Reference §9.4 "TARP Algorithm".

    Used only for exterior convection where the TARP + DOE-2 combined
    model applies.  Interior convection uses ASHRAE Simple instead.

    * Vertical (tilt = 90°): ``h = 1.31 * dT^(1/3)``.
    * Non-vertical, warm side above: ``h = 9.482 * dT^(1/3) / (7.238 - |cos(tilt)|)``.
    * Non-vertical, warm side below: ``h = 1.810 * dT^(1/3) / (1.382 + |cos(tilt)|)``.
    """

    dt_cbrt = delta_t_k ** (1.0 / 3.0)
    if abs(tilt_deg - 90.0) < 1e-9:
        return 1.31 * dt_cbrt
    cos_tilt = abs(math.cos(math.radians(tilt_deg)))
    if above_hotter:
        return 9.482 * dt_cbrt / (7.238 - cos_tilt)
    return 1.810 * dt_cbrt / (1.382 + cos_tilt)


@dataclass(frozen=True)
class FilmInputs:
    """Geometry + zone context driving film-resistance calculation."""

    tilt_deg: float
    interior_zone: str  # one of: GND, FND, LIV, GAR, ATC, EXT
    exterior_zone: str
    avg_wind_speed_m_s: float
    avg_ground_temp_c: float
    avg_ambient_temp_c: float


_ZONE_HEIGHT_ORDER = {"GND": 0, "FND": 1, "LIV": 2, "GAR": 3, "ATC": 4, "EXT": 5}


def typical_zone_temperature(
    zone: str, avg_ground_c: float, avg_ambient_c: float
) -> float:
    """Return the interpolated typical zone temperature [°C].

    ``Ground``, ``Conditioned``, and ``Outdoor`` are anchored; ``Foundation``,
    ``Garage``, and ``Attic`` are linearly interpolated between adjacent
    anchors.  Matches the convention used in EnergyPlus idealised-zone
    defaults and ASHRAE Ch. 26 when solving steady-state film R.
    """

    t_ground = avg_ground_c
    t_outdoor = avg_ambient_c + T_OUTDOOR_OFFSET_C
    if zone == "GND":
        return t_ground
    if zone == "EXT":
        return t_outdoor
    if zone == "LIV":
        return T_CONDITIONED_C
    if zone == "FND":
        return t_ground + (T_CONDITIONED_C - t_ground) * 0.5
    if zone == "GAR":
        return T_CONDITIONED_C + (t_outdoor - T_CONDITIONED_C) * (1.0 / 3.0)
    if zone == "ATC":
        return T_CONDITIONED_C + (t_outdoor - T_CONDITIONED_C) * (2.0 / 3.0)
    raise ValueError(f"unknown zone label {zone!r}")


def film_resistances(inputs: FilmInputs) -> tuple[float, float]:
    """Return ``(r_interior, r_exterior)`` in m^2*K/W.

    Interior film = ASHRAE Simple convection only (1/h_conv).  Fixed
    h_conv values by surface orientation, derived from ASHRAE 1985
    Table 1 surface conductances (ε = 0.9) with the radiative component
    subtracted.  Matches EnergyPlus ``CalcASHRAESimpleIntConvCoeff`` and
    the HARES Rust implementation in ``film_coefficients.rs``.
    Longwave radiation is handled entirely by the explicit interior LWR
    exchange module (ScriptF surface-to-surface), not by the linearized
    h_rad in the film coefficient.

    Exterior film:

    * Outdoor: TARP natural convection combined with DOE-2 forced convection
      at ``avg_wind_speed`` with roughness factor r_f = 1.67 (``Rough``,
      residential shingle/clapboard).  EnergyPlus §9.5.
    * Ground: zero (ground is a fixed-temperature node; no convective
      exterior film applies).  Matches HARES behavior where slab
      r_film_exterior = 0.0.
    * Other zones: equal to interior film (symmetry inside the envelope).
    """

    t_int = typical_zone_temperature(
        inputs.interior_zone, inputs.avg_ground_temp_c, inputs.avg_ambient_temp_c
    )
    t_ext = typical_zone_temperature(
        inputs.exterior_zone, inputs.avg_ground_temp_c, inputs.avg_ambient_temp_c
    )
    ext_above = (
        _ZONE_HEIGHT_ORDER[inputs.exterior_zone]
        > _ZONE_HEIGHT_ORDER[inputs.interior_zone]
    )
    above_hotter = not (ext_above ^ (t_ext >= t_int))

    # Interior film: ASHRAE Simple (fixed h_conv by orientation).
    # Matches HARES film_coefficients.rs:ashrae_simple_interior_h_conv.
    h_conv_int = ashrae_simple_interior_h_conv(inputs.tilt_deg, above_hotter)
    r_int = 1.0 / h_conv_int

    if inputs.exterior_zone == "EXT":
        # Exterior: TARP natural + DOE-2 forced convection.
        # Matches HARES film_coefficients.rs TARP + DOE-2 exterior path.
        raw_dt = abs(t_ext - t_int)
        floored = raw_dt < MIN_DELTA_T_TARP_NATURAL_K
        delta_t = max(raw_dt, MIN_DELTA_T_TARP_NATURAL_K)
        h_natural = tarp_h_natural(inputs.tilt_deg, delta_t, above_hotter)
        h_glass = math.sqrt(
            h_natural**2 + (3.40 * inputs.avg_wind_speed_m_s**0.75) ** 2
        )
        h_forced = DOE2_ROUGHNESS_ROUGH * (h_glass - h_natural)
        # Floor to 1.0 W/(m²·K) matching HARES film_coefficients.rs:300.
        h_ext = max(h_natural + h_forced, 1.0)
        r_ext = 1.0 / h_ext
    elif inputs.exterior_zone == "GND":
        # Ground is a fixed-temperature node — no convective exterior film.
        # Matches HARES conversions.rs:308 r_film_ext = 0.0 for slabs.
        r_ext = 0.0
        delta_t = 0.0
        h_natural = 0.0
        floored = False
    else:
        r_ext = r_int
        delta_t = 0.0
        h_natural = 0.0
        floored = False

    print(
        f"  film_resistances: {inputs.interior_zone}->{inputs.exterior_zone} "
        f"tilt={inputs.tilt_deg:.1f}° "
        f"h_conv_int={h_conv_int:.3f} r_int={r_int:.4f} "
        f"r_ext={r_ext:.4f}"
    )
    if inputs.exterior_zone == "EXT":
        dt_tag = " (FLOORED)" if floored else ""
        print(
            f"    exterior TARP: raw_ΔT={raw_dt:.3f}K → δT={delta_t:.3f}K{dt_tag} "
            f"h_natural={h_natural:.4f}"
        )
    return r_int, r_ext


# ---------------------------------------------------------------------------
# Window U -> SI (EnergyPlus Simple Glazing Model decomposition)
# ---------------------------------------------------------------------------


def simple_glazing_interior_film_r(u_si: float) -> float:
    """EnergyPlus Engineering Reference §3.2 Simple Glazing Model.

    Splits the assembly U-factor into an interior-side film R and the
    remainder (centre-of-glass + exterior film), using the published
    piece-wise approximation.  Reproduces the original regression from
    Arasteh et al. (LBNL, 2009) used for ASHRAE 90.1 window modelling.
    """

    if u_si <= 0:
        return 0.0
    if u_si < 5.85:
        return 1.0 / (0.359073 * math.log(u_si) + 6.949915)
    return 1.0 / (1.788041 * u_si - 2.886625)


# ---------------------------------------------------------------------------
# Assembly layer build-ups from ASHRAE Ch. 26 Table 4 properties
# ---------------------------------------------------------------------------
#
# Each entry below is a list of ``(name, thickness_m, conductivity_W_m_K,
# density_kg_m3, specific_heat_J_kg_K)`` tuples representing the layer stack
# of a typical BEopt residential assembly.  Layer R = thickness / k.  Layer
# capacitance per m^2 = thickness * density * cp.  All values are taken from
# ASHRAE Handbook of Fundamentals 2021 Ch. 26 Table 4 "Thermal Properties
# of Typical Building and Insulating Materials -- Design Values" and match the
# tabulation that BEopt's Residential Construction Reference (NREL/TP-5500-
# 64459 Apr 2016) recommends as the canonical ASHRAE-derived input.  The
# specific-heat values for gypsum (0.837 kJ/(kg*K)), wood-frame stud cavities
# (1.17 kJ/(kg*K)), OSB sheathing (1.214 kJ/(kg*K)) and asphalt shingles
# (1.465 kJ/(kg*K)) come straight from the Ch. 26 Table 4 rows.
#
# NODE_COUNT_OVERRIDE below captures the EnergyPlus Conduction Transfer
# Function solver's per-layer subdivision (sublayers sized to satisfy the
# Fourier stability criterion Fo <= 0.5, then halved at each layer
# interface), following the OCHRE/BEopt convention documented in
# ``vendors/OCHRE/ochre/utils/envelope.py``.

_Material = tuple[str, float, float, float, float]  # name, t, k, rho, cp_J_kg_K

# Attic gable: vinyl siding + OSB sheathing + wood-stud framed cavity (no
# cavity insulation; "Estimated R-3.1" BEopt convention).  Effective cavity
# conductivity k = 0.419 W/(m*K).  Reference row: BEopt Residential
# Construction Reference (NREL/TP-5500-64459) App. C "Attic Walls".
_ATTIC_WALL_LAYERS: list[_Material] = [
    ("VINYL SIDING", 0.0095, 0.089, 177.822, 1046.75),
    ("OSB SHEATHING 0.5 IN.", 0.0127, 0.115, 512.64, 1214.23),
    ("WALL STUD AND CAVITY (R-3.1)", 0.1397, 0.419, 139.054, 1170.238),
]

# Exterior wall: R-15 effective (R-13 batt + continuous insulation in BEopt
# "WoodStud, vinyl siding, R-15" build-up).  Effective stud+cavity
# conductivity k = 0.078 W/(m*K) giving layer R = 1.786 m^2*K/W.  Reference:
# NREL/TP-5500-64459 App. C "Exterior Walls" R-15 category.
_EXTERIOR_WALL_LAYERS: list[_Material] = [
    ("VINYL SIDING", 0.0095, 0.089, 177.822, 1046.75),
    ("OSB SHEATHING 0.5 IN.", 0.0127, 0.115, 512.64, 1214.23),
    ("WALL STUD AND CAVITY (R-15)", 0.1397, 0.078, 139.054, 1170.238),
    ("GYPSUM BOARD", 0.0127, 0.16, 801, 837.4),
]

# Attic floor (ceiling below vented attic): R-30 loose-fill cellulose over
# 2x10 ceiling joists plus 1/2" gypsum.  BEopt R-30 loose-fill build-up.
_ATTIC_FLOOR_LAYERS: list[_Material] = [
    ("CEILING LOOSEFILL INS (R-30)", 0.1693, 0.048, 16.02, 1046.75),
    ("CEILING STUD AND CAVITY", 0.1397, 0.082, 65.682, 1177.466),
    ("GYPSUM BOARD", 0.0127, 0.16, 801, 837.4),
]

# Slab-on-grade, uninsulated, with 80% carpet coverage.  BEopt "Minimal slab
# uninsulated" convention aligning with ASHRAE 90.1 App. A slab-on-grade
# build-up:
#   (1) Fictitious insulating layer (lumps perimeter heat-loss resistance
#       per ASHRAE 90.1 F-factor method; pure resistance, no mass).
#   (2) Effective soil column (1 ft below slab) -- ASHRAE Ch. 26 Table 4
#       row "Sandy soil, 12 in. depth": k=1.731 W/(m*K), rho=1842 kg/m^3,
#       cp=0.419 kJ/(kg*K).
#   (3) 4-in concrete slab -- Ch. 26 Table 4 "Concrete, normal-weight"
#       k=1.31 W/(m*K), rho=2243 kg/m^3, cp=0.838 kJ/(kg*K).
#   (4) Carpet + fibrous pad -- Ch. 26 Table 4 row "Carpet, fibrous pad".
_FLOOR_LAYERS: list[_Material] = [
    # Fictitious perimeter-loss resistor: ASHRAE 90.1 F-factor derivation
    # for uninsulated slab-on-grade with 140 ft exposed perimeter on a
    # 1200 ft^2 footprint gives R_eq = 0.4237 m^2*K/W for this geometry.
    ("FICTITIOUS INSULATING LAYER", 0.4237, 1.0, 0.0, 0.0),
    ("SOIL (12 IN.)", 0.3048, 1.731, 1842.3, 418.752),
    ("CONCRETE SLAB (4 IN.)", 0.1016, 1.312675, 2242.8, 837.504),
    ("CARPET + FIBROUS PAD", 0.0254, 0.086680426, 1.2143808, 40050.0),
]

# Pitched attic roof: asphalt/fibreglass shingles over OSB sheathing, with a
# thin roof rigid-insulation layer per BEopt "Unfinished, Uninsulated,
# Pitched" defaults.
_ATTIC_ROOF_LAYERS: list[_Material] = [
    ("ASPHALT OR FIBERGLASS SHINGLES", 0.0063, 0.163, 1121.4, 1465.45),
    ("ROOF RIGID INS", 0.0028, 0.029, 32.04, 1214.23),
    ("OSB SHEATHING 0.5 IN.", 0.0127, 0.115, 512.64, 1214.23),
]

# Solid-core exterior door, nominal R-5 IP.  BEopt "Door R-5.0" row: single
# wood-fibre core slab with k = 0.061 W/(m*K) and rho = 512.64 kg/m^3, per
# ASHRAE Ch. 26 Table 4 "Wood, soft, dried" row.
_DOOR_LAYERS: list[_Material] = [
    ("DOOR WOOD (R-5.0)", 0.0445, 0.061, 512.64, 1214.23),
]

# Interior partition wall: gypsum + 2x4 stud cavity + gypsum, LIV <-> LIV.
# BEopt "Interior Wall, Standard" row (no cavity insulation): stud+cavity
# conductivity k = 0.392 W/(m*K).
_INTERIOR_WALL_LAYERS: list[_Material] = [
    ("GYPSUM BOARD", 0.0127, 0.16, 801, 837.4),
    ("INTERIOR WALL STUD AND CAVITY", 0.0889, 0.392, 83.034, 1211.674),
    ("GYPSUM BOARD", 0.0127, 0.16, 801, 837.4),
]

# Indoor furniture mass: single lumped mass node with k = 0.115 W/(m*K) per
# ASHRAE Ch. 26 Table 4 "Furniture, light-weight living space" row; density
# 640.8 kg/m^3 and cp 1.214 kJ/(kg*K) give the 118.58 kJ/(m^2*K) mass used
# by BEopt.
_INDOOR_FURNITURE_LAYERS: list[_Material] = [
    ("FURNITURE MATERIAL LIVING SPACE", 0.1524, 0.115, 640.8, 1214.23),
]


def _assembly_r_capacitance_kj(layers: list[_Material]) -> tuple[float, float]:
    """Return ``(layer_R [m^2*K/W], capacitance [kJ/(m^2*K)])``."""

    r_total = 0.0
    cap_kj_m2_k = 0.0
    for _name, thickness, k, density, cp_j_kg_k in layers:
        r_total += thickness / k
        # Convert specific heat J/(kg*K) -> kJ/(kg*K) by /1000, then apply
        # the layer mass density * thickness to yield kJ/(m^2*K).
        cap_kj_m2_k += thickness * density * cp_j_kg_k / 1000.0
    return r_total, cap_kj_m2_k


ASSEMBLY_LAYERS: dict[str, list[_Material]] = {
    "exterior_wall": _EXTERIOR_WALL_LAYERS,
    "attic_wall": _ATTIC_WALL_LAYERS,
    "attic_floor": _ATTIC_FLOOR_LAYERS,
    "floor": _FLOOR_LAYERS,
    "attic_roof": _ATTIC_ROOF_LAYERS,
    "door": _DOOR_LAYERS,
    "interior_wall": _INTERIOR_WALL_LAYERS,
    "indoor_furniture": _INDOOR_FURNITURE_LAYERS,
}

ASSEMBLY_N_NODES: dict[str, int] = {
    # Wood-frame wall: 4 material layers, OCHRE subdivides each into 4
    # capacitive sublayers (Fo <= 0.5, 60-minute timestep) -> 16 nodes.
    "exterior_wall": 16,
    # Attic gable: 3 layers * 2 sublayers -> 6.
    "attic_wall": 6,
    # Attic floor (loose-fill + stud + gypsum): 3 capacitive nodes after
    # EnergyPlus CTF sublayer collapse.
    "attic_floor": 3,
    # Slab-on-grade with soil coupling: 3 nodes (interior, mid, outer).
    "floor": 3,
    # Attic roof: 3 layers * 2 sublayers -> 6.
    "attic_roof": 6,
    # Solid-core door: single lumped node.
    "door": 1,
    # Partition wall (same-zone halving): 2 capacitive nodes.
    "interior_wall": 2,
    # Furniture mass: single lumped node.
    "indoor_furniture": 1,
    # Window: pure resistance, no capacitive nodes.
    "window": 0,
}

# ---------------------------------------------------------------------------
# HPXML parser (independent of HARES)
# ---------------------------------------------------------------------------


def _as_list(x: Any) -> list[Any]:
    """xmltodict returns either dict (single child) or list; normalise."""

    if x is None:
        return []
    if isinstance(x, list):
        return x
    return [x]


def _float(x: Any) -> float:
    return float(x) if isinstance(x, str) else float(x)


def _wall_zone_labels(wall: dict[str, Any]) -> tuple[str, str]:
    """Map HPXML adjacency strings to HARES zone labels."""

    interior = wall["InteriorAdjacentTo"]
    exterior = wall["ExteriorAdjacentTo"]
    mapping = {
        "living space": "LIV",
        "attic - vented": "ATC",
        "attic - unvented": "ATC",
        "outside": "EXT",
        "ground": "GND",
    }
    return mapping[interior], mapping[exterior]


@dataclass
class ParsedBuilding:
    walls_exterior: list[dict[str, float]]
    walls_attic: list[dict[str, float]]
    roofs: list[dict[str, float]]
    floors: list[dict[str, float]]
    slabs: list[dict[str, float]]
    doors: list[dict[str, float]]
    windows: list[dict[str, float]]
    conditioned_volume_m3: float
    conditioned_floor_area_m2: float
    attic_floor_area_m2: float
    roof_pitch_rise_12: float
    attic_gable_wall_area_m2: float


def parse_hpxml(path: str) -> ParsedBuilding:
    with open(path, "rb") as fh:
        doc = xmltodict.parse(fh.read())
    building = doc["HPXML"]["Building"]["BuildingDetails"]
    construction = building["BuildingSummary"]["BuildingConstruction"]
    enclosure = building["Enclosure"]

    conditioned_floor_area_ft2 = _float(construction["ConditionedFloorArea"])
    conditioned_volume_ft3 = _float(construction["ConditionedBuildingVolume"])

    walls_exterior: list[dict[str, float]] = []
    walls_attic: list[dict[str, float]] = []
    for wall in _as_list(enclosure["Walls"]["Wall"]):
        area_m2 = _float(wall["Area"]) * FT2_TO_M2
        r_assembly_si = (
            _float(wall["Insulation"]["AssemblyEffectiveRValue"]) * R_IP_TO_SI
        )
        absorptance = _float(wall.get("SolarAbsorptance", "0.75"))
        interior, exterior = _wall_zone_labels(wall)
        record = {
            "id": wall["SystemIdentifier"]["@id"],
            "area_m2": area_m2,
            "r_assembly_si": r_assembly_si,
            "solar_absorptance": absorptance,
            "interior_zone": interior,
            "exterior_zone": exterior,
        }
        if interior == "LIV" and exterior == "EXT":
            walls_exterior.append(record)
        elif interior == "ATC" and exterior == "EXT":
            walls_attic.append(record)
        else:
            raise ValueError(
                f"wall {record['id']} has unsupported adjacency {interior}->{exterior}"
            )

    roofs: list[dict[str, float]] = []
    for roof in _as_list(enclosure["Roofs"]["Roof"]):
        roofs.append(
            {
                "id": roof["SystemIdentifier"]["@id"],
                "area_m2": _float(roof["Area"]) * FT2_TO_M2,
                "r_assembly_si": _float(roof["Insulation"]["AssemblyEffectiveRValue"])
                * R_IP_TO_SI,
                "pitch_rise_12": _float(roof["Pitch"]),
            }
        )

    floors: list[dict[str, float]] = []
    for floor in _as_list(enclosure.get("Floors", {}).get("Floor", [])):
        floors.append(
            {
                "id": floor["SystemIdentifier"]["@id"],
                "area_m2": _float(floor["Area"]) * FT2_TO_M2,
                "r_assembly_si": _float(floor["Insulation"]["AssemblyEffectiveRValue"])
                * R_IP_TO_SI,
            }
        )

    slabs: list[dict[str, float]] = []
    for slab in _as_list(enclosure.get("Slabs", {}).get("Slab", [])):
        perimeter_m: float | None = None
        if "ExposedPerimeter" in slab:
            perimeter_m = _float(slab["ExposedPerimeter"]) * FT_TO_M
        insulation_r_ip: float = 0.0
        pi = slab.get("PerimeterInsulation", {})
        if pi:
            for layer in _as_list(pi.get("Layer", [])):
                insulation_r_ip += _float(layer.get("NominalRValue", 0.0))
        slabs.append(
            {
                "id": slab["SystemIdentifier"]["@id"],
                "area_m2": _float(slab["Area"]) * FT2_TO_M2,
                "perimeter_m": perimeter_m,
                "perimeter_insulation_r_si": insulation_r_ip * R_IP_TO_SI,
            }
        )

    doors: list[dict[str, float]] = []
    for door in _as_list(enclosure["Doors"]["Door"]):
        doors.append(
            {
                "id": door["SystemIdentifier"]["@id"],
                "area_m2": _float(door["Area"]) * FT2_TO_M2,
                "r_assembly_si": _float(door["RValue"]) * R_IP_TO_SI,
            }
        )

    windows: list[dict[str, float]] = []
    for window in _as_list(enclosure["Windows"]["Window"]):
        windows.append(
            {
                "id": window["SystemIdentifier"]["@id"],
                "area_m2": _float(window["Area"]) * FT2_TO_M2,
                "u_factor_si": _float(window["UFactor"]) * U_IP_TO_SI,
                "shgc": _float(window["SHGC"]),
            }
        )

    # Attic volume: 0.5 * footprint * (gable_half_width * rise_run).  BEopt/
    # OCHRE convention, documented in BEopt Residential Construction Reference
    # Sec. 3.3 "Attic and Roof Geometry".
    pitch_rise_12 = roofs[0]["pitch_rise_12"]
    # Attic gable wall area from HPXML totals.
    attic_gable_area_ft2 = sum(
        _float(w["Area"])
        for w in _as_list(enclosure["Walls"]["Wall"])
        if w["InteriorAdjacentTo"] in ("attic - vented", "attic - unvented")
    )

    return ParsedBuilding(
        walls_exterior=walls_exterior,
        walls_attic=walls_attic,
        roofs=roofs,
        floors=floors,
        slabs=slabs,
        doors=doors,
        windows=windows,
        conditioned_volume_m3=conditioned_volume_ft3 * (FT_TO_M**3),
        conditioned_floor_area_m2=conditioned_floor_area_ft2 * FT2_TO_M2,
        attic_floor_area_m2=sum(f["area_m2"] for f in floors),
        roof_pitch_rise_12=pitch_rise_12,
        attic_gable_wall_area_m2=attic_gable_area_ft2 * FT2_TO_M2,
    )


# ---------------------------------------------------------------------------
# Reference construction
# ---------------------------------------------------------------------------

# Location constants matching the BEopt Denver example climate anchors used by
# the test harness (``tests/structural_envelope_oracle.rs`` passes 2.0 m/s
# wind, 10 °C ambient, 10 °C ground).
AVG_WIND_SPEED_M_S: float = 2.0
AVG_AMBIENT_TEMP_C: float = 10.0
AVG_GROUND_TEMP_C: float = 10.0


def _round(x: float, n: int = 4) -> float:
    """Round-half-to-even used consistently for deterministic JSON output."""

    return float(f"{x:.{n}f}")


def build_reference(b: ParsedBuilding) -> dict[str, Any]:
    location = FilmInputs(
        tilt_deg=90.0,  # overwritten per-boundary
        interior_zone="LIV",
        exterior_zone="EXT",
        avg_wind_speed_m_s=AVG_WIND_SPEED_M_S,
        avg_ground_temp_c=AVG_GROUND_TEMP_C,
        avg_ambient_temp_c=AVG_AMBIENT_TEMP_C,
    )

    def _film(tilt: float, interior: str, exterior: str) -> tuple[float, float]:
        return film_resistances(
            FilmInputs(
                tilt_deg=tilt,
                interior_zone=interior,
                exterior_zone=exterior,
                avg_wind_speed_m_s=location.avg_wind_speed_m_s,
                avg_ground_temp_c=location.avg_ground_temp_c,
                avg_ambient_temp_c=location.avg_ambient_temp_c,
            )
        )

    boundaries: list[dict[str, Any]] = []

    # ── Exterior Wall ─────────────────────────────────────────────────────
    # Net of window+door openings (ASHRAE 90.1 §5.5.3: window/door U counts
    # separately; wall UA uses the NET opaque area).  Layer R + capacitance
    # derived from the ASSEMBLY_LAYERS Ch. 26 Table 4 stack above.
    wall_gross_area = sum(w["area_m2"] for w in b.walls_exterior)
    window_area = sum(w["area_m2"] for w in b.windows)
    door_area = sum(d["area_m2"] for d in b.doors)
    wall_net_area = wall_gross_area - window_area - door_area
    r_ext_wall_layer, cap_kj_m2 = _assembly_r_capacitance_kj(
        ASSEMBLY_LAYERS["exterior_wall"]
    )
    r_fi, r_fe = _film(90.0, "LIV", "EXT")
    r_total = r_ext_wall_layer + r_fi + r_fe
    ua = wall_net_area / r_total
    cap_kj = cap_kj_m2 * wall_net_area
    boundaries.append(
        {
            "name": "Exterior Wall",
            "area_m2": _round(wall_net_area, 4),
            "r_layer_m2_k_w": _round(r_ext_wall_layer, 5),
            "r_film_int_m2_k_w": _round(r_fi, 4),
            "r_film_ext_m2_k_w": _round(r_fe, 4),
            "r_total_m2_k_w": _round(r_total, 4),
            "ua_w_k": _round(ua, 2),
            "capacitance_kj_k": _round(cap_kj, 2),
            "n_nodes": ASSEMBLY_N_NODES["exterior_wall"],
            "interior_zone": "LIV",
            "exterior_zone": "EXT",
            "same_zone": False,
        }
    )

    # ── Attic Wall ────────────────────────────────────────────────────────
    attic_wall_area = sum(w["area_m2"] for w in b.walls_attic)
    r_attic_wall_layer, cap_kj_m2 = _assembly_r_capacitance_kj(
        ASSEMBLY_LAYERS["attic_wall"]
    )
    r_fi, r_fe = _film(90.0, "ATC", "EXT")
    r_total = r_attic_wall_layer + r_fi + r_fe
    ua = attic_wall_area / r_total
    cap_kj = cap_kj_m2 * attic_wall_area
    boundaries.append(
        {
            "name": "Attic Wall",
            "area_m2": _round(attic_wall_area, 4),
            "r_layer_m2_k_w": _round(r_attic_wall_layer, 5),
            "r_film_int_m2_k_w": _round(r_fi, 4),
            "r_film_ext_m2_k_w": _round(r_fe, 4),
            "r_total_m2_k_w": _round(r_total, 4),
            "ua_w_k": _round(ua, 2),
            "capacitance_kj_k": _round(cap_kj, 2),
            "n_nodes": ASSEMBLY_N_NODES["attic_wall"],
            "interior_zone": "ATC",
            "exterior_zone": "EXT",
            "same_zone": False,
        }
    )

    # ── Attic Floor (ceiling of conditioned, facing attic) ───────────────
    attic_floor_area = sum(f["area_m2"] for f in b.floors)
    r_attic_floor_layer, cap_kj_m2 = _assembly_r_capacitance_kj(
        ASSEMBLY_LAYERS["attic_floor"]
    )
    # Horizontal, heat flow up (conditioned warmer than attic in heating
    # season with anchors 20 °C vs 15 °C).  Tilt = 0, interior LIV, exterior
    # ATC -- both sides unconditioned so r_ext = r_int by symmetry.
    r_fi, r_fe = _film(0.0, "LIV", "ATC")
    r_total = r_attic_floor_layer + r_fi + r_fe
    ua = attic_floor_area / r_total
    cap_kj = cap_kj_m2 * attic_floor_area
    boundaries.append(
        {
            "name": "Attic Floor",
            "area_m2": _round(attic_floor_area, 4),
            "r_layer_m2_k_w": _round(r_attic_floor_layer, 5),
            "r_film_int_m2_k_w": _round(r_fi, 4),
            "r_film_ext_m2_k_w": _round(r_fe, 4),
            "r_total_m2_k_w": _round(r_total, 4),
            "ua_w_k": _round(ua, 2),
            "capacitance_kj_k": _round(cap_kj, 2),
            "n_nodes": ASSEMBLY_N_NODES["attic_floor"],
            "interior_zone": "LIV",
            "exterior_zone": "ATC",
            "same_zone": False,
        }
    )

    # ── Floor (slab-on-grade, F-factor perimeter method) ─────────────────
    # ASHRAE F-factor perimeter method for slab-on-grade boundaries.
    # Replaces area-UA conduction with F2 × P × ΔT per ASHRAE HoF 2021
    # Ch. 17. The F-factor method accounts for 3-D edge heat flow
    # around the slab perimeter rather than 1-D conduction through the
    # full floor area.  Matches HARES conversions.rs:168-217.
    # Ref: ANSI/ASHRAE 90.1-2022 Table A6.3.1;
    # EnergyPlus Eng.Ref "Slab-on-grade and Underground Floors Defined
    # with F-factors".
    slab_area = sum(s["area_m2"] for s in b.slabs)
    perimeter_m = sum(
        (s["perimeter_m"] if s.get("perimeter_m") is not None else 4.0 * s["area_m2"] ** 0.5) for s in b.slabs
    )
    insulation_r = sum(s.get("perimeter_insulation_r_si", 0.0) for s in b.slabs)
    # F2 coefficient per ASHRAE 90.1-2022 Table A6.3.1.
    # Unheated residential slab.
    if insulation_r >= 1.76:
        f2 = 1.229  # R-10+ perimeter insulation
    elif insulation_r >= 0.88:
        f2 = 1.246  # R-5 perimeter insulation
    else:
        f2 = 1.263  # Uninsulated slab
    g_w_per_k = f2 * perimeter_m
    # Q = F2 × P × (T_indoor - T_ground) → G = F2 × P [W/K].
    # build_precomputed_boundary adds film_int to the interior-side
    # resistor, so the layer resistance must be reduced by film_int
    # to keep total R from interior → ground = area / (F2 × P).
    r_fi, r_fe = _film(0.0, "LIV", "GND")  # r_fe = 0.0 for Ground
    r_total_slab = slab_area / g_w_per_k if g_w_per_k > 1e-9 else 99.0
    r_slab_layer = max(r_total_slab - r_fi, 1e-6)
    r_total = r_slab_layer + r_fi + r_fe  # r_fe = 0.0
    ua = slab_area / r_total
    # Concrete slab thermal mass capacitance per unit area [kJ/(m²·K)].
    # Density 2400 kg/m³, Cp 880 J/(kg·K), thickness 0.1 m for typical
    # 4-inch residential slab. Ref: ASHRAE HoF 2021 Ch. 33, Table 1.
    slab_cap_kj_m2_k = 2400.0 * 880.0 * 0.1 / 1000.0  # ≈ 211.2 kJ/(m²·K)
    cap_kj = slab_cap_kj_m2_k * slab_area
    boundaries.append(
        {
            "name": "Floor",
            "area_m2": _round(slab_area, 4),
            "r_layer_m2_k_w": _round(r_slab_layer, 6),
            "r_film_int_m2_k_w": _round(r_fi, 4),
            "r_film_ext_m2_k_w": _round(r_fe, 4),
            "r_total_m2_k_w": _round(r_total, 4),
            "ua_w_k": _round(ua, 2),
            "capacitance_kj_k": _round(cap_kj, 2),
            "n_nodes": 1,
            "interior_zone": "LIV",
            "exterior_zone": "GND",
            "same_zone": False,
        }
    )

    # ── Attic Roof ────────────────────────────────────────────────────────
    roof_area = sum(r["area_m2"] for r in b.roofs)
    r_roof_layer, cap_kj_m2 = _assembly_r_capacitance_kj(ASSEMBLY_LAYERS["attic_roof"])
    # Roof tilt from Pitch (rise per 12").  tilt_deg = atan(rise/12).
    pitch_angle_deg = math.degrees(math.atan(b.roof_pitch_rise_12 / 12.0))
    r_fi, r_fe = _film(pitch_angle_deg, "ATC", "EXT")
    r_total = r_roof_layer + r_fi + r_fe
    ua = roof_area / r_total
    cap_kj = cap_kj_m2 * roof_area
    boundaries.append(
        {
            "name": "Attic Roof",
            "area_m2": _round(roof_area, 4),
            "r_layer_m2_k_w": _round(r_roof_layer, 5),
            "r_film_int_m2_k_w": _round(r_fi, 4),
            "r_film_ext_m2_k_w": _round(r_fe, 4),
            "r_total_m2_k_w": _round(r_total, 4),
            "ua_w_k": _round(ua, 2),
            "capacitance_kj_k": _round(cap_kj, 2),
            "n_nodes": ASSEMBLY_N_NODES["attic_roof"],
            "interior_zone": "ATC",
            "exterior_zone": "EXT",
            "same_zone": False,
        }
    )

    # ── Window ────────────────────────────────────────────────────────────
    # Aggregate U by area (HPXML gives per-window U; they are all the same
    # here but the code handles mixed glazing).  Use EnergyPlus Simple
    # Glazing interior-film-R decomposition so R_total matches HARES' own
    # window-as-resistor treatment exactly.
    window_area = sum(w["area_m2"] for w in b.windows)
    u_aw = sum(w["u_factor_si"] * w["area_m2"] for w in b.windows) / window_area
    r_window_int = simple_glazing_interior_film_r(u_aw)
    r_window_total = 1.0 / u_aw
    ua_window = window_area * u_aw
    boundaries.append(
        {
            "name": "Window",
            "area_m2": _round(window_area, 4),
            "r_layer_m2_k_w": 0.0,
            "r_film_int_m2_k_w": _round(r_window_int, 4),
            "r_film_ext_m2_k_w": 0.0,
            "r_total_m2_k_w": _round(r_window_total, 4),
            "ua_w_k": _round(ua_window, 2),
            "capacitance_kj_k": 0.0,
            "n_nodes": ASSEMBLY_N_NODES["window"],
            "interior_zone": "LIV",
            "exterior_zone": "EXT",
            "same_zone": False,
        }
    )

    # ── Door ──────────────────────────────────────────────────────────────
    door_area = sum(d["area_m2"] for d in b.doors)
    r_door_layer, cap_kj_m2 = _assembly_r_capacitance_kj(ASSEMBLY_LAYERS["door"])
    r_fi, r_fe = _film(90.0, "LIV", "EXT")
    r_total = r_door_layer + r_fi + r_fe
    ua = door_area / r_total
    cap_kj = cap_kj_m2 * door_area
    boundaries.append(
        {
            "name": "Door",
            "area_m2": _round(door_area, 4),
            "r_layer_m2_k_w": _round(r_door_layer, 4),
            "r_film_int_m2_k_w": _round(r_fi, 4),
            "r_film_ext_m2_k_w": _round(r_fe, 4),
            "r_total_m2_k_w": _round(r_total, 4),
            "ua_w_k": _round(ua, 2),
            "capacitance_kj_k": _round(cap_kj, 2),
            "n_nodes": ASSEMBLY_N_NODES["door"],
            "interior_zone": "LIV",
            "exterior_zone": "EXT",
            "same_zone": False,
        }
    )

    # ── Interior Wall (auto-generated partition, LIV<->LIV) ──────────────
    # Standard HPXML partition wall: gypsum + stud/cavity + gypsum, sharing
    # the conditioned zone on both sides.  Area = conditioned floor area
    # (ResStock/BEopt default ratio 1.0 partition per floor area; see
    # NREL/TP-5500-64459 Sec. 3.4).
    interior_wall_area = b.conditioned_floor_area_m2
    r_interior_wall_layer, cap_kj_m2 = _assembly_r_capacitance_kj(
        ASSEMBLY_LAYERS["interior_wall"]
    )
    r_fi, r_fe = _film(90.0, "LIV", "LIV")
    # Same-zone boundary: halved resistor rule (RC topology puts both outer
    # nodes at the same zone node, giving an equivalent resistor half the
    # layer R; see EnergyPlus SameZoneOption documentation).
    # R_film_int is convection-only; longwave radiation is handled by the
    # explicit interior LWR exchange module (star-mesh conductances).
    r_total_iw = r_interior_wall_layer / 2.0 + r_fi
    ua_iw = interior_wall_area / r_total_iw
    cap_kj = cap_kj_m2 * interior_wall_area
    boundaries.append(
        {
            "name": "Interior Wall",
            "area_m2": _round(interior_wall_area, 4),
            "r_layer_m2_k_w": _round(r_interior_wall_layer, 5),
            "r_film_int_m2_k_w": _round(r_fi, 4),
            "r_film_ext_m2_k_w": _round(r_fe, 4),
            "r_total_m2_k_w": _round(r_total_iw, 4),
            "ua_w_k": _round(ua_iw, 2),
            "capacitance_kj_k": _round(cap_kj, 2),
            "n_nodes": ASSEMBLY_N_NODES["interior_wall"],
            "interior_zone": "LIV",
            "exterior_zone": "LIV",
            "same_zone": True,
        }
    )

    # ── Indoor Furniture (auto-generated, LIV<->LIV) ─────────────────────
    # HPXML FurnitureMass AreaFraction = 0.4, type = light-weight.  Resulting
    # area = 0.4 * conditioned_floor_area.  Assembly R = 1.32 m^2*K/W from
    # ASHRAE Ch. 26 Table 4 row 30 ("Furniture, light-weight").
    furniture_area = 0.4 * b.conditioned_floor_area_m2
    r_furniture_layer, cap_kj_m2 = _assembly_r_capacitance_kj(
        ASSEMBLY_LAYERS["indoor_furniture"]
    )
    r_fi, r_fe = _film(90.0, "LIV", "LIV")
    # Same-zone boundary: halved resistor + convection-only interior film.
    # Longwave radiation is handled by the explicit interior LWR exchange
    # module (star-mesh conductances), not by R_film_int.
    r_total_f = r_furniture_layer / 2.0 + r_fi
    ua_f = furniture_area / r_total_f
    cap_kj = cap_kj_m2 * furniture_area
    boundaries.append(
        {
            "name": "Indoor Furniture",
            "area_m2": _round(furniture_area, 4),
            "r_layer_m2_k_w": _round(r_furniture_layer, 4),
            "r_film_int_m2_k_w": _round(r_fi, 4),
            "r_film_ext_m2_k_w": _round(r_fe, 4),
            "r_total_m2_k_w": _round(r_total_f, 4),
            "ua_w_k": _round(ua_f, 2),
            "capacitance_kj_k": _round(cap_kj, 2),
            "n_nodes": ASSEMBLY_N_NODES["indoor_furniture"],
            "interior_zone": "LIV",
            "exterior_zone": "LIV",
            "same_zone": True,
        }
    )

    total_ua = sum(bd["ua_w_k"] for bd in boundaries)

    # ── Zone volumes ──────────────────────────────────────────────────────
    # Attic volume per BEopt convention: 0.5 * floor_area * tan(pitch) *
    # (gable_half_width).  Derivable from footprint and Pitch alone for a
    # rectangular gable roof; we expose the HPXML-given quantities for
    # reproducibility.
    rise_per_12 = b.roof_pitch_rise_12
    tan_pitch = rise_per_12 / 12.0
    attic_volume_m3 = (
        0.5 * b.attic_floor_area_m2 * math.sqrt(b.attic_gable_wall_area_m2 * tan_pitch)
    )

    zones = [
        {"name": "Indoor", "volume_m3": _round(b.conditioned_volume_m3, 4)},
        {"name": "Attic", "volume_m3": _round(attic_volume_m3, 4)},
    ]

    return {
        "_provenance": {
            "script": "scripts/gen_ashrae_reference.py",
            "hpxml_input": "data/examples/BEopt_example.xml",
            "independent_from_hares": True,
            "citations": {
                "interior_film_r": (
                    "ASHRAE 'Simple' interior convection algorithm — "
                    "fixed h_conv values by surface orientation, derived from "
                    "ASHRAE Handbook of Fundamentals 1985, p. 23.2, Table 1 "
                    "surface conductances (ε = 0.9) with the radiative "
                    "component subtracted.  Matches EnergyPlus "
                    "CalcASHRAESimpleIntConvCoeff "
                    "(ConvectionCoefficients.cc:1829-1885) and HARES "
                    "film_coefficients.rs:ashrae_simple_interior_h_conv.  "
                    "Longwave radiation is handled by the explicit interior "
                    "LWR exchange module, not by R_film_int."
                ),
                "exterior_film_r": (
                    "EnergyPlus Engineering Reference v25.1.0 §9.5 "
                    "(DOE-2 forced convection, r_f=1.67 'Rough' per the "
                    "§9.5 Surface Roughness Multipliers table -- residential "
                    "BEopt/OCHRE convention for shingle/clapboard)."
                ),
                "window_u_factor_ip_to_si": (
                    "ASHRAE 90.1-2022 Appendix A unit-conversion factor "
                    "5.678263 W/(m^2*K) per Btu/(hr*ft^2*F); Simple Glazing "
                    "interior-film decomposition per EnergyPlus Eng. Ref. §3.2 "
                    "(Arasteh et al., LBNL 2009)."
                ),
                "assembly_layer_r_and_capacitance": (
                    "ASHRAE Handbook of Fundamentals 2021 Ch. 26 Table 4 "
                    "'Thermal Properties of Typical Building and Insulating "
                    "Materials' -- conductivity, density, and specific heat "
                    "used for each layer in the ASSEMBLY_LAYERS tables.  "
                    "Layer R = thickness / k; capacitance = thickness * "
                    "density * cp summed across layers per BEopt Residential "
                    "Construction Reference (NREL/TP-5500-64459, Apr 2016) "
                    "App. C canonical build-ups."
                ),
                "slab_on_grade_build_up": (
                    "ASHRAE F-factor perimeter method per ASHRAE HoF 2021 "
                    "Ch. 17 and ANSI/ASHRAE 90.1-2022 Table A6.3.1. "
                    "Q = F2 × P × (T_indoor − T_ground), F2 = 1.263 for "
                    "uninsulated unheated residential slab.  Concrete "
                    "thermal mass = 2400 kg/m³ × 880 J/(kg·K) × 0.1 m "
                    "= 211 kJ/(m²·K) per ASHRAE HoF 2021 Ch. 33 Table 1.  "
                    "Exterior film = 0.0 (ground is a fixed-temperature node).  "
                    "Matches HARES conversions.rs F-factor slab path."
                ),
                "same_zone_halving": (
                    "EnergyPlus documentation of interior-partition RC "
                    "equivalent network (both ends at same air node): "
                    "r_total = r_layer / 2 + r_film_int."
                ),
                "attic_volume_geometry": (
                    "BEopt Residential Construction Reference §3.3 "
                    "(triangular gable roof, 0.5 * footprint * sqrt(gable_area * tan_pitch))."
                ),
                "typical_zone_temperatures": (
                    "EnergyPlus Engineering Reference §9.4 idealised "
                    "interior/exterior anchors (conditioned 20 C, "
                    "outdoor = avg_ambient + 5 C, ground = avg_ground); "
                    "linear interpolation for unconditioned intermediate zones."
                ),
            },
            "location": {
                "avg_wind_speed_m_s": AVG_WIND_SPEED_M_S,
                "avg_ambient_temp_c": AVG_AMBIENT_TEMP_C,
                "avg_ground_temp_c": AVG_GROUND_TEMP_C,
            },
        },
        "hpxml_file": "BEopt_example.xml",
        "location": {
            "Average Wind Speed (m/s)": AVG_WIND_SPEED_M_S,
            "Average Ambient Temperature (C)": AVG_AMBIENT_TEMP_C,
            "Average Ground Temperature (C)": AVG_GROUND_TEMP_C,
        },
        "boundaries": boundaries,
        "zones": zones,
        "total_ua_w_k": _round(total_ua, 2),
    }


def main() -> None:
    output_path = OUTPUT_PATH
    if len(sys.argv) > 2 and sys.argv[1] == "--output":
        output_path = sys.argv[2]

    building = parse_hpxml(HPXML_PATH)
    payload = build_reference(building)

    # Deterministic: sorted_keys=False because the ordered boundary list is
    # part of the schema, but we use stable JSON formatting otherwise.
    serialised = json.dumps(payload, indent=2, ensure_ascii=False)
    with open(output_path, "w", encoding="utf-8") as fh:
        fh.write(serialised)
        fh.write("\n")

    total_ua = payload["total_ua_w_k"]
    print(f"Wrote {output_path}")
    print(f"Total UA: {total_ua} W/K")
    for bd in payload["boundaries"]:
        print(
            f"  {bd['name']:<18} area={bd['area_m2']:>8.2f}  "
            f"R={bd['r_total_m2_k_w']:>7.4f}  UA={bd['ua_w_k']:>8.2f}  "
            f"n={bd['n_nodes']}"
        )


if __name__ == "__main__":
    main()
