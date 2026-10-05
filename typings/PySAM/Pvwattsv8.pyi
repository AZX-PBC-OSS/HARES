"""Typed surface of the optional ``PySAM`` package used by HARES.

The published ``nrel-pysam`` wheel ships per-module stubs whose attribute
declarations assign type objects instead of annotations, which pyright
rejects. This stub models the PVWatts v8 API the PV adapter uses.
"""

from typing import Protocol


class _SolarResource:
    solar_resource_file: str


class _SystemDesign:
    system_capacity: float
    tilt: float
    azimuth: float
    module_type: int
    array_type: int
    inv_eff: float
    losses: float


class _Outputs:
    ac: tuple[float, ...]
    gh: tuple[float, ...]
    dn: tuple[float, ...]
    df: tuple[float, ...]
    tamb: tuple[float, ...]


class Pvwattsv8:
    SolarResource: _SolarResource
    SystemDesign: _SystemDesign
    Outputs: _Outputs

    def execute(self, int_verbosity: int = ...) -> None: ...


def new() -> Pvwattsv8: ...
