"""Type stubs for the HARES Rust extension module."""

from __future__ import annotations

from collections.abc import Iterator
from typing import Any

import polars as pl


class PyControlSignal:
    @staticmethod
    def power_setpoint(kw: float, reactive_kvar: float | None = ...) -> PyControlSignal: ...
    @staticmethod
    def thermal_setpoint(
        heat_c: float | None = ...,
        cool_c: float | None = ...,
        deadband_c: float | None = ...,
    ) -> PyControlSignal: ...
    @staticmethod
    def soc_target(
        target: float,
        min: float | None = ...,
        max: float | None = ...,
    ) -> PyControlSignal: ...
    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> PyControlSignal: ...


class ControlSignal(PyControlSignal): ...


class PyTelemetry:
    def zone(self) -> dict[str, Any]: ...
    def equipment(self) -> dict[str, Any]: ...
    def total_power_kw(self) -> float: ...


class PyDwelling:
    @classmethod
    def from_hpxml(
        cls,
        hpxml: str,
        schedule: str,
        weather: str,
        **kwargs: Any,
    ) -> PyDwelling: ...
    def initialize(self) -> None: ...
    def timesteps(self) -> Iterator[Any]: ...
    def simulate(self) -> pl.DataFrame: ...
    def step(self) -> dict[str, Any]: ...
    def apply_control(self, name: str, signal: PyControlSignal) -> None: ...
    def set_price_signal(self, signal: dict[str, float | None]) -> None: ...
    def set_grid_voltage(self, voltage_pu: float) -> None: ...
    def telemetry(self) -> PyTelemetry: ...
    def results(self) -> pl.DataFrame: ...
    def reset_with_seed(self, seed: int) -> None: ...
    def save_state(self) -> bytes: ...
    def load_state(self, state: bytes) -> None: ...


class PyFleet:
    @classmethod
    def from_resstock(
        cls,
        metadata_path: str,
        hpxml_dir: str,
        weather_dir: str,
        filter: dict[str, str] | None = ...,
        resstock_version: str | None = ...,
    ) -> PyFleet: ...
    def simulate(self, n_threads: int | None = ...) -> PyFleetResults: ...


class PyFleetResults:
    @property
    def per_dwelling_metrics(self) -> pl.DataFrame: ...
    @property
    def aggregate_timeseries(self) -> pl.DataFrame: ...


def batch_step(
    dwellings: list[PyDwelling],
    actions: list[list[float]],
    observation_fields: list[str],
) -> list[dict[str, Any]]: ...


class Battery:
    def __init__(
        self,
        name: str,
        capacity_kwh: float,
        max_charge_kw: float | None = ...,
        max_discharge_kw: float | None = ...,
    ) -> None: ...


class PV:
    def __init__(self, name: str, capacity_kw: float, tilt: float, azimuth: float) -> None: ...


class EV:
    def __init__(
        self,
        name: str,
        capacity_kwh: float | None = ...,
        max_charging_kw: float | None = ...,
    ) -> None: ...
