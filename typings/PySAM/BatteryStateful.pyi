"""Typed surface of the optional ``PySAM.BatteryStateful`` module used by HARES.

The published ``nrel-pysam`` wheel ships per-module stubs whose attribute
declarations assign type objects instead of annotations, which pyright
rejects. This stub models the BatteryStateful API the battery adapter uses.
"""

from typing import Protocol


class _ParamsCell:
    Vnom_default: float
    resistance: float


class _HasParamsCell(Protocol):
    ParamsCell: _ParamsCell


class BatteryStateful:
    ParamsCell: _ParamsCell


def default(config: str) -> BatteryStateful: ...
