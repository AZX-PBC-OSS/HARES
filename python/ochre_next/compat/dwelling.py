"""OCHRE-compatible Dwelling wrapper.

Behavioral note: unlike OCHRE, unknown ``update_model`` control keys are logged as
warnings and skipped.
"""

from __future__ import annotations

from collections.abc import Mapping
from datetime import datetime, timedelta
import logging
from typing import Any

import polars as pl

from ochre_next._hares import PyDwelling

try:
    from ochre_next._hares import PyControlSignal
except ImportError:
    from ochre_next._hares import ControlSignal as PyControlSignal

LOGGER = logging.getLogger(__name__)


class Dwelling:
    """OCHRE-compatible wrapper over ``PyDwelling``."""

    def __init__(
        self,
        *,
        hpxml_file: str,
        hpxml_schedule_file: str,
        weather_file: str,
        start_time: datetime,
        time_res: timedelta,
        duration: timedelta,
        initialization_time: timedelta | None = None,
        Equipment: dict[str, Any] | None = None,
        **kwargs: Any,
    ) -> None:
        self._equipment_overrides = Equipment or {}
        self._extra_kwargs = kwargs
        self._last_results: pl.DataFrame | None = None

        py_kwargs: dict[str, Any] = {
            "start_time": start_time,
            "time_res": time_res,
            "duration": duration,
        }
        if initialization_time is not None:
            py_kwargs["initialization_duration"] = initialization_time

        self._dwelling = PyDwelling.from_hpxml(
            hpxml=hpxml_file,
            schedule=hpxml_schedule_file,
            weather=weather_file,
            **py_kwargs,
        )

    def simulate(self) -> tuple[pl.DataFrame, dict[str, float], pl.DataFrame]:
        """Run full simulation and return OCHRE-style tuple.

        Returns: ``(timeseries_df, metrics_dict, hourly_df)``.
        """
        df = self._dwelling.simulate()
        self._last_results = df
        metrics = self.generate_results()
        df_hourly = _hourly_aggregate(df)
        return df, metrics, df_hourly

    def update_model(self, control_signal: dict[str, Any]) -> None:
        """Apply OCHRE-style control dict and advance one timestep."""
        if not isinstance(control_signal, Mapping):
            LOGGER.warning("control_signal must be a mapping; received %r", type(control_signal))
            self._dwelling.step()
            return

        for equipment_name, payload in control_signal.items():
            if not isinstance(payload, Mapping):
                LOGGER.warning(
                    "control payload for %s must be a mapping; skipping",
                    equipment_name,
                )
                continue

            signal = _map_ochre_payload(payload)
            if signal is None:
                LOGGER.warning(
                    "unrecognized control keys for %s; payload keys=%s",
                    equipment_name,
                    sorted(payload.keys()),
                )
                continue

            self._dwelling.apply_control(str(equipment_name), signal)

        self._dwelling.step()

    def generate_results(self) -> dict[str, float]:
        """Compute summary metrics from the stored simulation DataFrame."""
        if self._last_results is None:
            raise RuntimeError("generate_results() requires prior simulate() call")

        metrics: dict[str, float] = {}
        for column in self._last_results.columns:
            series = self._last_results.get_column(column)
            if not series.dtype.is_numeric():
                continue
            value = series.drop_nulls().mean()
            if value is not None:
                metrics[column] = float(value)

        return metrics


def _map_ochre_payload(payload: Mapping[str, Any]) -> PyControlSignal | None:
    if "Setpoint Temperature (C)" in payload:
        return PyControlSignal.thermal_setpoint(
            heat_c=float(payload["Setpoint Temperature (C)"]),
            cool_c=None,
            deadband_c=None,
        )

    if "P Setpoint" in payload:
        return PyControlSignal.power_setpoint(
            kw=float(payload["P Setpoint"]),
            reactive_kvar=None,
        )

    if "Duty Cycle" in payload:
        return PyControlSignal.from_dict(
            {
                "type": "DutyCycle",
                "on_fraction": float(payload["Duty Cycle"]),
            }
        )

    if "Load Fraction" in payload:
        return PyControlSignal.from_dict(
            {
                "type": "LoadFraction",
                "fraction": float(payload["Load Fraction"]),
            }
        )

    if "SOC" in payload:
        return PyControlSignal.soc_target(
            target=float(payload["SOC"]),
            min=float(payload["Min SOC"]) if "Min SOC" in payload else None,
            max=float(payload["Max SOC"]) if "Max SOC" in payload else None,
        )

    if "Self Consumption Mode" in payload:
        return PyControlSignal.from_dict(
            {
                "type": "SelfConsumption",
                "enabled": bool(payload["Self Consumption Mode"]),
                "solar_only_charging": False,
            }
        )

    return None


def _hourly_aggregate(df: pl.DataFrame) -> pl.DataFrame:
    if "Time" not in df.columns:
        return pl.DataFrame()

    with_dt = df.with_columns(
        pl.col("Time")
        .str.to_datetime(format="%Y-%m-%dT%H:%M:%S%z", strict=False)
        .alias("_time")
    ).drop_nulls(["_time"])

    if with_dt.is_empty():
        return pl.DataFrame()
    numeric_cols = [
        name
        for name in with_dt.columns
        if name not in {"Time", "_time"} and with_dt.get_column(name).dtype.is_numeric()
    ]
    if not numeric_cols:
        return pl.DataFrame({"Time": with_dt.get_column("Time")})

    return (
        with_dt.group_by_dynamic(index_column="_time", every="1h", period="1h")
        .agg([pl.col(name).mean().alias(name) for name in numeric_cols])
        .sort("_time")
        .rename({"_time": "Time"})
    )
