"""HARES: High-performance Architecture for Residential Energy Simulation."""

from __future__ import annotations

from ._hares import PyDwelling as Dwelling
from ._hares import PyFleet as Fleet
from ._hares import SimulationConfig
from ._hares import DwellingConfig
from ._hares import PySimulationMetrics as SimulationMetrics

try:
    from ._hares import PyControlSignal as ControlSignal
except ImportError:
    from ._hares import ControlSignal as ControlSignal

from ._hares import (
    EndUse,
    FuelType,
    OperatingMode,
    Mode,
    ExecutionStage,
    FluidType,
    InverterPriority,
    DutyCycleComponent,
    SimStatus,
    AggregationResolution,
    ResStockVersion,
    ControlCapabilities,
    LutType,
    BatteryChemistry,
    ChargingLevel,
    DriverArchetype,
    DRLevel,
    EquipmentDescriptor,
    TelemetryField,
)
from ._hares import (
    parse_weather,
    parse_epw,
    parse_psm3,
    parse_tmy3,
    parse_resstock_csv,
    WeatherTimeSeries,
)

__all__ = [
    "Dwelling",
    "Fleet",
    "SimulationConfig",
    "DwellingConfig",
    "SimulationMetrics",
    "ControlSignal",
    "EndUse",
    "FuelType",
    "OperatingMode",
    "Mode",
    "ExecutionStage",
    "FluidType",
    "InverterPriority",
    "DutyCycleComponent",
    "SimStatus",
    "AggregationResolution",
    "ResStockVersion",
    "ControlCapabilities",
    "LutType",
    "BatteryChemistry",
    "ChargingLevel",
    "DriverArchetype",
    "DRLevel",
    "EquipmentDescriptor",
    "TelemetryField",
    "parse_weather",
    "parse_epw",
    "parse_psm3",
    "parse_tmy3",
    "parse_resstock_csv",
    "WeatherTimeSeries",
]
