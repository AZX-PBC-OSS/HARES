"""HELICS runner config helpers for launching GridLAB-D + HARES federates."""

from __future__ import annotations

from dataclasses import dataclass
import json
import os
from pathlib import Path
import shlex
import tempfile
from typing import Any

try:
    import helics
except ImportError as exc:  # pragma: no cover - exercised via import test
    raise ImportError(
        "HELICS not installed. Install with: pip install 'ochre_next[helics]'"
    ) from exc


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


def run_cosimulation(config: dict[str, Any] | Path) -> None:
    """Run a HELICS co-simulation via ``helics.cli.run``.

    Example:
        >>> cfg = generate_cosim_config([
        ...     FederateConfig(name="gridlabd", exec_command="gridlabd feeder.glm"),
        ...     FederateConfig(name="house_1", exec_command="python run_house.py"),
        ... ])
        >>> run_cosimulation(cfg)

    Args:
        config: Either an in-memory config dict or path to an existing JSON file.
    """

    cli = getattr(helics, "cli", None)
    if cli is None or not hasattr(cli, "run"):
        raise RuntimeError("HELICS Python module does not expose helics.cli.run")

    if isinstance(config, Path):
        cli.run(str(config))
        return

    temp_fd, temp_name = tempfile.mkstemp(suffix=".json")
    temp_path = Path(temp_name)
    try:
        with os.fdopen(temp_fd, mode="w", encoding="utf-8") as handle:
            json.dump(config, handle)
            handle.flush()
        cli.run(str(temp_path))
    finally:
        temp_path.unlink(missing_ok=True)


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
        ... )

    Args:
        name: Federate name.
        hpxml_path: HPXML file for dwelling initialization.
        weather_path: Weather input path.
        schedule_path: Schedule input path.
        **kwargs: Extra CLI arguments rendered as ``--key=value``.

    Returns:
        Typed federate config for HELICS runner JSON.
    """

    for key in kwargs:
        if not key or not all(c.isalnum() or c in "-_" for c in key):
            raise ValueError(f"Invalid CLI argument key: {key!r}")

    extra_args = [
        f"--{key}={shlex.quote(str(value))}"
        for key, value in sorted(kwargs.items())
    ]
    command_parts = [
        "python",
        "-m",
        "ochre_next.helics.dwelling",
        f"--name={shlex.quote(name)}",
        f"--hpxml={shlex.quote(str(hpxml_path))}",
        f"--weather={shlex.quote(str(weather_path))}",
        f"--schedule={shlex.quote(str(schedule_path))}",
        *extra_args,
    ]
    command = " ".join(command_parts)

    return FederateConfig(name=name, exec_command=command)
