"""HARES: High-performance Architecture for Residential Energy Simulation."""

from __future__ import annotations

from ._hares import PyDwelling as Dwelling
from ._hares import PyFleet as Fleet

try:
    from ._hares import PyControlSignal as ControlSignal
except ImportError:
    from ._hares import ControlSignal as ControlSignal

__all__ = ["Dwelling", "Fleet", "ControlSignal"]
