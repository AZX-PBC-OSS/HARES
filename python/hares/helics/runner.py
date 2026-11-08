"""HELICS runner config helpers: the federate entries and the JSON configuration the external ``helics`` CLI consumes."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
import shlex
from typing import Any

from .dwelling import federate_argument_names


@dataclass(frozen=True, slots=True)
class FederateConfig:
    """Typed HELICS runner federate entry.

    Example:
        >>> FederateConfig(
        ...     name="gridlabd",
        ...     exec_command="gridlabd feeder.glm",
        ...     directory="./grid",
        ... )
    """

    name: str
    exec_command: str
    host: str = "localhost"
    directory: str = "."


def generate_cosim_config(federates: list[FederateConfig], broker: bool = True) -> dict[str, Any]:
    """Generate a HELICS CLI runner configuration dictionary.

    Example:
        >>> cfg = generate_cosim_config(
        ...     [
        ...         FederateConfig(name="gridlabd", exec_command="gridlabd feeder.glm"),
        ...         FederateConfig(name="house_1", exec_command="python run_house.py"),
        ...     ]
        ... )
        >>> cfg["name"], len(cfg["federates"])
        ('hares-cosim', 2)

    Args:
        federates: Typed federate descriptors.
        broker: Whether HELICS CLI should launch a broker process.

    Returns:
        JSON-serializable dict matching HELICS runner schema.
    """

    if not federates:
        raise ValueError("At least one federate is required")

    federate_entries = [
        {
            "name": fed.name,
            "host": fed.host,
            "directory": fed.directory,
            "exec": fed.exec_command,
        }
        for fed in federates
    ]

    return {
        "name": "hares-cosim",
        "broker": bool(broker),
        "federates": federate_entries,
    }


def make_dwelling_federate_config(
    name: str,
    hpxml_path: Path,
    weather_path: Path,
    schedule_path: Path,
    **kwargs: str | int | float | bool | Path,
) -> FederateConfig:
    """Build a standard dwelling federate command entry.

    This helper follows a common GridLAB-D + HARES pattern where GridLAB-D is
    one federate and each HARES dwelling process is another.

    Example:
        >>> make_dwelling_federate_config(
        ...     name="house_1",
        ...     hpxml_path=Path("house_1.xml"),
        ...     weather_path=Path("weather.epw"),
        ...     schedule_path=Path("schedule.csv"),
        ...     broker="localhost:23404",
        ...     start="2019-01-01T00:00:00",
        ...     **{"duration-s": 86400, "time-res-s": 900},
        ... )

    Args:
        name: Federate name.
        hpxml_path: HPXML file for dwelling initialization.
        weather_path: Weather input path.
        schedule_path: Schedule input path.
        **kwargs: The entry point's remaining CLI arguments rendered as
            ``--key=value``. The run's horizon (``start``, ``duration-s``,
            ``time-res-s``) is required: the entry point has no default
            because a silently defaulted horizon would misalign every
            federation the dwelling joins. Every key must be a flag the
            entry point defines; an unknown key or a missing horizon is a
            ``ValueError`` here, not the child's argparse exit after launch.

    Returns:
        Typed federate config for HELICS runner JSON.
    """

    accepted = federate_argument_names() - {"name", "hpxml", "weather", "schedule"}
    for key in kwargs:
        if not key or not all(c.isalnum() or c in "-_" for c in key):
            raise ValueError(f"Invalid CLI argument key: {key!r}")
        if key not in accepted:
            raise ValueError(
                f"Unknown dwelling federate argument {key!r}: the entry point "
                f"accepts {sorted(accepted)}"
            )
    missing = [
        key for key in ("start", "duration-s", "time-res-s") if key not in kwargs
    ]
    if missing:
        raise ValueError(
            "The dwelling federate command needs its horizon: missing "
            f"{['--' + key for key in missing]}; the entry point has no "
            "default because a silently defaulted horizon would misalign "
            "every federation the dwelling joins"
        )

    extra_args = [
        f"--{key}={shlex.quote(str(value))}"
        for key, value in sorted(kwargs.items())
    ]
    command_parts = [
        "python",
        "-m",
        "hares.helics.dwelling",
        f"--name={shlex.quote(name)}",
        f"--hpxml={shlex.quote(str(hpxml_path))}",
        f"--weather={shlex.quote(str(weather_path))}",
        f"--schedule={shlex.quote(str(schedule_path))}",
        *extra_args,
    ]
    command = " ".join(command_parts)

    return FederateConfig(name=name, exec_command=command)
