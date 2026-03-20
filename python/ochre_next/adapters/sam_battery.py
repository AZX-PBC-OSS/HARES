"""SAM battery model adapter.

Extracts cell-level parameters from PySAM for a given chemistry and capacity.
Output is a TOML parameter file consumed by the Rust battery model in
``hares-equipment``.
"""

from __future__ import annotations

import hashlib
import logging
from dataclasses import dataclass, field as dataclass_field
from pathlib import Path
from typing import Any

try:
    import tomllib
except ModuleNotFoundError:
    import tomli as tomllib  # type: ignore[no-redef]

try:
    import PySAM.BatteryStateful as _battery_sam  # type: ignore[import-untyped]

    _HAS_PYSAM = True
except ImportError:
    _battery_sam = None
    _HAS_PYSAM = False

LOGGER = logging.getLogger(__name__)

_BUILTIN_DEFAULTS: dict[str, dict[str, Any]] = {
    "NMC": {
        "cell": {
            "chemistry": "NMC",
            "v_nominal": 3.6,
            "ah_rated": 50.0,
            "r_internal_ohm": 0.004,
            "n_series": 14,
            "n_parallel": 4,
        },
        "soc_ocv": {
            "soc": [0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0],
            "v_oc": [3.0, 3.4, 3.5, 3.55, 3.6, 3.63, 3.66, 3.7, 3.8, 3.95, 4.2],
        },
        "thermal": {
            "r_thermal_k_per_w": 0.5,
            "c_thermal_j_per_k": 90000,
        },
        "losses": {
            "standby_power_w": 5.0,
            "self_discharge_pct_per_day": 0.05,
            "inverter_efficiency": 0.96,
        },
        "degradation": {
            "model": "rainflow_arrhenius",
            "calendar_q": 4.14e-10,
            "calendar_a": -7280.0,
            "cycle_q": 2.64e-4,
            "cycle_d": 0.7,
        },
    },
    "LFP": {
        "cell": {
            "chemistry": "LFP",
            "v_nominal": 3.2,
            "ah_rated": 50.0,
            "r_internal_ohm": 0.003,
            "n_series": 14,
            "n_parallel": 4,
        },
        "soc_ocv": {
            "soc": [0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0],
            "v_oc": [2.5, 3.0, 3.1, 3.15, 3.2, 3.22, 3.25, 3.28, 3.3, 3.35, 3.6],
        },
        "thermal": {
            "r_thermal_k_per_w": 0.5,
            "c_thermal_j_per_k": 90000,
        },
        "losses": {
            "standby_power_w": 5.0,
            "self_discharge_pct_per_day": 0.05,
            "inverter_efficiency": 0.97,
        },
        "degradation": {
            "model": "rainflow_arrhenius",
            "calendar_q": 3.0e-10,
            "calendar_a": -6500.0,
            "cycle_q": 1.5e-4,
            "cycle_d": 0.8,
        },
    },
    "NCA": {
        "cell": {
            "chemistry": "NCA",
            "v_nominal": 3.6,
            "ah_rated": 50.0,
            "r_internal_ohm": 0.005,
            "n_series": 14,
            "n_parallel": 4,
        },
        "soc_ocv": {
            "soc": [0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0],
            "v_oc": [2.7, 3.3, 3.5, 3.6, 3.65, 3.7, 3.75, 3.8, 3.9, 4.0, 4.2],
        },
        "thermal": {
            "r_thermal_k_per_w": 0.45,
            "c_thermal_j_per_k": 85000,
        },
        "losses": {
            "standby_power_w": 5.0,
            "self_discharge_pct_per_day": 0.08,
            "inverter_efficiency": 0.96,
        },
        "degradation": {
            "model": "rainflow_arrhenius",
            "calendar_q": 5.0e-10,
            "calendar_a": -7500.0,
            "cycle_q": 3.0e-4,
            "cycle_d": 0.65,
        },
    },
    "LTO": {
        "cell": {
            "chemistry": "LTO",
            "v_nominal": 2.4,
            "ah_rated": 30.0,
            "r_internal_ohm": 0.002,
            "n_series": 20,
            "n_parallel": 6,
        },
        "soc_ocv": {
            "soc": [0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0],
            "v_oc": [1.5, 2.0, 2.2, 2.3, 2.35, 2.4, 2.42, 2.45, 2.5, 2.55, 2.8],
        },
        "thermal": {
            "r_thermal_k_per_w": 0.4,
            "c_thermal_j_per_k": 80000,
        },
        "losses": {
            "standby_power_w": 3.0,
            "self_discharge_pct_per_day": 0.02,
            "inverter_efficiency": 0.97,
        },
        "degradation": {
            "model": "rainflow_arrhenius",
            "calendar_q": 1.0e-10,
            "calendar_a": -5000.0,
            "cycle_q": 5.0e-5,
            "cycle_d": 0.9,
        },
    },
}


def _canonical_hash(
    chemistry: str,
    capacity_kwh: float,
    source: str,
) -> str:
    parts: dict[str, str] = {
        "capacity_kwh": f"{capacity_kwh:.6f}",
        "chemistry": chemistry,
        "source": source,
    }
    canonical = "".join(f"{k}={v};" for k, v in sorted(parts.items()))
    return hashlib.sha256(canonical.encode()).hexdigest()


def _cache_valid(toml_path: Path, expected_hash: str) -> bool:
    hash_path = toml_path.with_suffix(".hash")
    if not toml_path.exists() or not hash_path.exists():
        return False
    stored = hash_path.read_text().strip()
    return stored == expected_hash


def _write_cache(toml_path: Path, content_hash: str) -> None:
    hash_path = toml_path.with_suffix(".hash")
    hash_path.write_text(content_hash + "\n")


def _format_toml_value(value: Any) -> str:
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, int):
        return str(value)
    if isinstance(value, float):
        return f"{value}"
    if isinstance(value, str):
        return f'"{value}"'
    if isinstance(value, list):
        items = ", ".join(_format_toml_value(v) for v in value)
        return f"[{items}]"
    return repr(value)


def _dict_to_toml(data: dict[str, Any]) -> str:
    lines: list[str] = []
    scalars = {k: v for k, v in data.items() if not isinstance(v, dict)}
    tables = {k: v for k, v in data.items() if isinstance(v, dict)}

    for k in sorted(scalars):
        lines.append(f"{k} = {_format_toml_value(scalars[k])}")

    for section in sorted(tables):
        lines.append(f"\n[{section}]")
        for k in sorted(tables[section]):
            lines.append(f"{k} = {_format_toml_value(tables[section][k])}")

    return "\n".join(lines) + "\n"


@dataclass
class CellParams:
    """Battery cell parameter set."""

    params: dict[str, Any] = dataclass_field(default_factory=dict)
    metadata: dict[str, Any] = dataclass_field(default_factory=dict)

    def save(self, path: Path) -> None:
        """Write the parameters to a TOML file on disk."""
        path = Path(path)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(_dict_to_toml(self.params))
        if "content_hash" in self.metadata:
            _write_cache(path, self.metadata["content_hash"])


def _extract_from_sam(chemistry: str, capacity_kwh: float) -> dict[str, Any]:
    """Extract cell parameters from PySAM BatteryStateful."""
    if _battery_sam is None:
        raise ImportError(
            "PySAM is required for SAM battery adapter; install with: "
            "pip install 'ochre_next[sam]'"
        )

    batt = _battery_sam.default("GenericBatteryCommercial")
    params = dict(_BUILTIN_DEFAULTS.get(chemistry, _BUILTIN_DEFAULTS["NMC"]))
    params["cell"] = dict(params["cell"])
    params["cell"]["chemistry"] = chemistry
    v_nominal = batt.ParamsCell.Vnom_default
    if v_nominal > 0:
        params["cell"]["v_nominal"] = v_nominal
    r_internal = batt.ParamsCell.resistance
    if r_internal > 0:
        params["cell"]["r_internal_ohm"] = r_internal

    total_ah = capacity_kwh * 1000.0 / params["cell"]["v_nominal"]
    n_parallel = params["cell"]["n_parallel"]
    params["cell"]["ah_rated"] = round(total_ah / n_parallel, 2)

    return params


def extract_cell_params(
    chemistry: str,
    capacity_kwh: float,
    source: str = "SAM",
    *,
    cache_dir: Path | None = None,
) -> CellParams:
    """Extract battery cell parameters.

    Parameters
    ----------
    chemistry:
        Battery chemistry identifier (e.g. ``"LFP"``, ``"NMC"``, ``"NCA"``, ``"LTO"``).
    capacity_kwh:
        Total pack energy capacity in kWh.
    source:
        Parameter source.  ``"SAM"`` uses PySAM; ``"defaults"`` uses built-in
        constants.
    cache_dir:
        Optional directory for caching.

    Returns
    -------
    CellParams
        A parameter object.  Call ``.save(path)`` to write TOML.
    """
    content_hash = _canonical_hash(chemistry, capacity_kwh, source)

    if cache_dir is not None:
        cached_path = Path(cache_dir) / "cell_params.toml"
        if _cache_valid(cached_path, content_hash):
            LOGGER.info("Battery params cache hit: %s", cached_path)
            raw = cached_path.read_text()
            return CellParams(
                params=_parse_toml_string(raw),
                metadata={"content_hash": content_hash, "cached": True},
            )

    if source == "SAM" and _HAS_PYSAM:
        LOGGER.info("Extracting battery params from PySAM for %s %.1f kWh", chemistry, capacity_kwh)
        params = _extract_from_sam(chemistry, capacity_kwh)
    else:
        if source == "SAM" and not _HAS_PYSAM:
            LOGGER.warning("PySAM not installed; using built-in defaults for %s", chemistry)
        LOGGER.info("Using built-in defaults for %s %.1f kWh", chemistry, capacity_kwh)
        chem_upper = chemistry.upper()
        base = _BUILTIN_DEFAULTS.get(chem_upper, _BUILTIN_DEFAULTS["NMC"])
        params = {k: dict(v) if isinstance(v, dict) else v for k, v in base.items()}
        params["cell"]["chemistry"] = chemistry
        total_ah = capacity_kwh * 1000.0 / params["cell"]["v_nominal"]
        n_parallel = params["cell"]["n_parallel"]
        params["cell"]["ah_rated"] = round(total_ah / n_parallel, 2)

    result = CellParams(params=params, metadata={"content_hash": content_hash, "cached": False})

    if cache_dir is not None:
        cached_path = Path(cache_dir) / "cell_params.toml"
        result.save(cached_path)

    return result


def _parse_toml_string(text: str) -> dict[str, Any]:
    """Parse a TOML string using the standard library (3.11+) or tomli."""
    return tomllib.loads(text)
