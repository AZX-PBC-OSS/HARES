"""OCHRE-compatible Dwelling wrapper.

Behavioral note: unlike OCHRE, unknown ``update_model`` control keys are logged as
warnings and skipped.
"""

from __future__ import annotations

from collections.abc import Mapping
from datetime import datetime, timedelta
from enum import StrEnum
import logging
from typing import Any, NotRequired, TypedDict

import polars as pl

from ochre_next import Dwelling as PyDwelling
from ochre_next import ControlSignal as PyControlSignal

LOGGER = logging.getLogger(__name__)


class ControlPayloadKey(StrEnum):
    SETPOINT = "Setpoint"
    DEADBAND = "Deadband"
    P_SETPOINT = "P Setpoint"
    Q_SETPOINT = "Q Setpoint"
    DUTY_CYCLE = "Duty Cycle"
    LOAD_FRACTION = "Load Fraction"
    SOC = "SOC"
    SOC_MIN = "Min SOC"
    SOC_MAX = "Max SOC"
    SELF_CONSUMPTION = "Self Consumption Mode"
    SOLAR_ONLY = "Solar Only Charging"


class OchreThermalPayload(TypedDict, total=False):
    Setpoint: float
    Deadband: float


class OchrePowerPayload(TypedDict, total=False):
    """P or Q setpoint payload."""

    P_Setpoint: NotRequired[float]
    Q_Setpoint: NotRequired[float]


class OchreControlSignal(TypedDict, total=False):
    """One equipment's control signal in OCHRE format."""

    Setpoint: float
    Deadband: float
    P_Setpoint: float
    Q_Setpoint: float
    Duty_Cycle: float
    Load_Fraction: float
    SOC: float
    Min_SOC: float
    Max_SOC: float
    Self_Consumption_Mode: bool
    Solar_Only_Charging: bool


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
            "time_res_s": time_res,
            "duration_s": duration,
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

            signal = _map_ochre_payload(equipment_name, payload)
            if signal is None:
                LOGGER.warning(
                    "unrecognized or invalid control for %s; payload keys=%s",
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
            if isinstance(value, (int, float)):
                metrics[column] = float(value)

        return metrics


_COOLING_NAMES: frozenset[str] = frozenset({
    "HVAC Cooling",
    "Air Conditioner",
    "Room AC",
    "ASHP Cooler",
    "MSHP Cooler",
})


def _is_cooling_equipment(name: str) -> bool:
    return name in _COOLING_NAMES


def _safe_float(value: Any, key: str, equipment_name: str) -> float | None:
    try:
        return float(value)
    except (TypeError, ValueError):
        LOGGER.warning(
            "invalid value for %s on %s: %r; skipping", key, equipment_name, value,
        )
        return None


def _map_ochre_payload(
    equipment_name: str, payload: Mapping[str, Any]
) -> PyControlSignal | None:
    is_cooling = _is_cooling_equipment(equipment_name)

    if ControlPayloadKey.SETPOINT in payload:
        sp = _safe_float(payload[ControlPayloadKey.SETPOINT], ControlPayloadKey.SETPOINT, equipment_name)
        if sp is None:
            return None
        deadband_c: float | None = None
        if ControlPayloadKey.DEADBAND in payload:
            deadband_c = _safe_float(payload[ControlPayloadKey.DEADBAND], ControlPayloadKey.DEADBAND, equipment_name)
        if is_cooling:
            return PyControlSignal.thermal_setpoint(
                heat_c=None, cool_c=sp, deadband_c=deadband_c,
            )
        return PyControlSignal.thermal_setpoint(
            heat_c=sp, cool_c=None, deadband_c=deadband_c,
        )

    if ControlPayloadKey.DEADBAND in payload:
        db = _safe_float(payload[ControlPayloadKey.DEADBAND], ControlPayloadKey.DEADBAND, equipment_name)
        if db is None:
            return None
        return PyControlSignal.thermal_setpoint(
            heat_c=None, cool_c=None, deadband_c=db,
        )

    if ControlPayloadKey.P_SETPOINT in payload:
        kw = _safe_float(payload[ControlPayloadKey.P_SETPOINT], ControlPayloadKey.P_SETPOINT, equipment_name)
        if kw is None:
            return None
        return PyControlSignal.power_setpoint(kw=kw, reactive_kvar=None)

    if ControlPayloadKey.DUTY_CYCLE in payload:
        dc = _safe_float(payload[ControlPayloadKey.DUTY_CYCLE], ControlPayloadKey.DUTY_CYCLE, equipment_name)
        if dc is None:
            return None
        return PyControlSignal.from_dict({"type": "DutyCycle", "on_fraction": dc})

    if ControlPayloadKey.LOAD_FRACTION in payload:
        lf = _safe_float(payload[ControlPayloadKey.LOAD_FRACTION], ControlPayloadKey.LOAD_FRACTION, equipment_name)
        if lf is None:
            return None
        return PyControlSignal.from_dict({"type": "LoadFraction", "fraction": lf})

    if ControlPayloadKey.SOC in payload:
        soc = _safe_float(payload[ControlPayloadKey.SOC], ControlPayloadKey.SOC, equipment_name)
        if soc is None:
            return None
        return PyControlSignal.soc_target(
            target=soc,
            min=_safe_float(payload[ControlPayloadKey.SOC_MIN], ControlPayloadKey.SOC_MIN, equipment_name)
            if ControlPayloadKey.SOC_MIN in payload
            else None,
            max=_safe_float(payload[ControlPayloadKey.SOC_MAX], ControlPayloadKey.SOC_MAX, equipment_name)
            if ControlPayloadKey.SOC_MAX in payload
            else None,
        )

    if ControlPayloadKey.SELF_CONSUMPTION in payload:
        return PyControlSignal.from_dict(
            {
                "type": "SelfConsumption",
                "enabled": bool(payload[ControlPayloadKey.SELF_CONSUMPTION]),
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
