"""OCHRE config to HARES config migration.

This module provides translation from OCHRE-style config parameters to HARES-style
config parameters. HARES uses clean, native units internally; this adapter allows
users migrating from OCHRE to use familiar parameter names and units.

Usage:
    from ochre_next.compat.ochre_config import migrate_ochre_equipment_config

    hares_config = migrate_ochre_equipment_config(ochre_params, "Generator")
"""

from __future__ import annotations

from typing import Any


# OCHRE -> HARES config key mapping
# OCHRE uses kW/min for ramp rate; HARES uses kW/s
OCHRE_TO_HARES_EQUIPMENT_KEYS: dict[str, dict[str, str]] = {
    "Generator": {
        "capacity": "rated_power_kw",
        "capacity_min": "capacity_min_kw",
        "ramp_rate": "delta_kw_per_s",  # kW/min -> kW/s (divide by 60)
        "efficiency": "eta_electric",
        "efficiency_type": "efficiency_type",
        "efficiency_chp": "eta_thermal",
        "import_limit": "grid_import_limit_kw",
        "export_limit": "export_limit_kw",
    },
    "Battery": {
        "capacity": "capacity_kwh",
        "power": "power_kw",
        "efficiency": "efficiency",
    },
    "PV": {
        "system_capacity": "system_capacity_kw",
        "tilt": "tilt",
        "azimuth": "azimuth",
    },
}


def migrate_ochre_equipment_config(
    ochre_params: dict[str, Any],
    equipment_type: str,
) -> dict[str, Any]:
    """Translate OCHRE equipment config to HARES config.

    Args:
        ochre_params: Dictionary of OCHRE-style config parameters.
        equipment_type: One of "Generator", "Battery", "PV".

    Returns:
        Dictionary of HARES-style config parameters with translated keys and units.
    """
    key_map = OCHRE_TO_HARES_EQUIPMENT_KEYS.get(equipment_type, {})
    hares_config: dict[str, Any] = {}

    for ochre_key, value in ochre_params.items():
        if ochre_key in key_map:
            hares_key = key_map[ochre_key]

            # Unit conversions
            if ochre_key == "ramp_rate" and value is not None:
                hares_config[hares_key] = value / 60.0
            else:
                hares_config[hares_key] = value
        else:
            hares_config[ochre_key] = value

    return hares_config


def ochre_generator_config(
    capacity: float,
    *,
    ramp_rate: float | None = 0.1,
    efficiency: float = 0.30,
    capacity_min: float | None = None,
    import_limit: float = 0.0,
    export_limit: float = 0.0,
    **kwargs: Any,
) -> dict[str, Any]:
    """Create a HARES generator config from OCHRE-style parameters.

    Args:
        capacity: Rated power in kW.
        ramp_rate: Ramp rate in kW/min (OCHRE default: 0.1).
        efficiency: Electrical efficiency (default: 0.30).
        capacity_min: Minimum operating power in kW.
        import_limit: Grid import limit in kW.
        export_limit: Grid export limit in kW.
        **kwargs: Additional OCHRE parameters.

    Returns:
        HARES-compatible config dictionary.
    """
    config: dict[str, Any] = {
        "rated_power_kw": capacity,
        "delta_kw_per_s": ramp_rate / 60.0 if ramp_rate is not None else None,
        "eta_electric": efficiency,
    }

    if capacity_min is not None:
        config["capacity_min_kw"] = capacity_min

    if import_limit is not None:
        config["grid_import_limit_kw"] = import_limit

    if export_limit is not None:
        config["export_limit_kw"] = export_limit

    config.update(kwargs)

    return config
