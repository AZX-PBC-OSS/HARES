"""Adapters for external simulation tools.

Provides a fallback chain for battery parameters:
  1. PyBaMM (physics-based electrochemical model)
  2. SAM (PySAM cell parameter extraction)
  3. Built-in defaults (hardcoded reference values)

Each adapter is optional.  When the backing library is absent, the adapter
either returns built-in defaults or raises ``ImportError`` with an actionable
message.
"""

from __future__ import annotations

import logging
from pathlib import Path

from .pybamm_battery import (
    DegradationParams,
    EfficiencyLut,
    generate_degradation_params,
    generate_efficiency_lut,
)
from .sam_battery import CellParams, extract_cell_params
from .sam_pv import PvLut, generate_pv_lut

LOGGER = logging.getLogger(__name__)

__all__ = [
    "CellParams",
    "DegradationParams",
    "EfficiencyLut",
    "PvLut",
    "extract_cell_params",
    "generate_degradation_params",
    "generate_efficiency_lut",
    "generate_pv_lut",
    "resolve_battery_params",
]


def resolve_battery_params(
    chemistry: str,
    capacity_kwh: float,
    *,
    cache_dir: Path | None = None,
) -> CellParams:
    """Resolve battery cell parameters using the best available source.

    Fallback chain:
      1. PyBaMM -- not used for ``CellParams`` directly but logged if present
      2. SAM (PySAM) -- ``extract_cell_params`` with ``source="SAM"``
      3. Built-in defaults

    Parameters
    ----------
    chemistry:
        Battery chemistry (e.g. ``"LFP"``, ``"NMC"``).
    capacity_kwh:
        Total pack energy capacity in kWh.
    cache_dir:
        Optional directory for caching.

    Returns
    -------
    CellParams
    """
    try:
        import pybamm  # noqa: F401

        LOGGER.info("PyBaMM available; SAM still preferred for cell params")
    except ImportError:
        pass

    try:
        import PySAM  # noqa: F401

        LOGGER.info("Using PySAM for battery cell params (%s, %.1f kWh)", chemistry, capacity_kwh)
        return extract_cell_params(
            chemistry,
            capacity_kwh,
            source="SAM",
            cache_dir=cache_dir,
        )
    except ImportError:
        pass

    LOGGER.info("No external tools available; using built-in defaults for %s", chemistry)
    return extract_cell_params(
        chemistry,
        capacity_kwh,
        source="defaults",
        cache_dir=cache_dir,
    )
