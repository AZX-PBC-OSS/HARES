"""Typed surface of the optional ``pvlib`` package used by HARES.

The published ``pvlib`` wheel ships no py.typed marker, so pyright infers
its functions from source with unknowns. This stub models the solar-position
and irradiance surface the PV override generator uses.
"""

from __future__ import annotations

from datetime import datetime

import pandas as pd


class solarposition:
    @staticmethod
    def get_solarposition(
        time: pd.DatetimeIndex,
        latitude: float,
        longitude: float,
        altitude: float | None = ...,
        pressure: float | None = ...,
        method: str = ...,
        temperature: float = ...,
    ) -> pd.DataFrame:
        """Return zenith/azimuth (and apparent angles) indexed by ``time``."""


class irradiance:
    @staticmethod
    def get_extra_radiation(time: pd.DatetimeIndex | datetime) -> pd.Series:
        """Return extraterrestrial radiation (W/m2) per timestamp."""

    @staticmethod
    def aoi(
        surface_tilt: float,
        surface_azimuth: float,
        solar_zenith: float,
        solar_azimuth: float,
    ) -> float: ...

    @staticmethod
    def get_total_irradiance(
        surface_tilt: float,
        surface_azimuth: float,
        solar_zenith: float,
        solar_azimuth: float,
        dni: float,
        ghi: float,
        dhi: float,
        *,
        dni_extra: float | None = ...,
        model: str = ...,
        albedo: float | None = ...,
    ) -> pd.DataFrame:
        """Return per-component POA irradiance columns for scalar inputs."""


class iotools:
    @staticmethod
    def read_epw(
        filename: str,
        *,
        coerce_year: int | None = ...,
    ) -> tuple[pd.DataFrame, dict[str, float]]:
        """Return (weather data indexed by hour, location metadata)."""
