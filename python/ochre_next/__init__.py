"""HARES: High-performance Architecture for Residential Energy Simulation."""

from __future__ import annotations

from ._hares import PyDwelling as Dwelling
from ._hares import PyFleet as Fleet
from ._hares import PyFleetResults as FleetResults
from ._hares import SimulationConfig
from ._hares import DwellingConfig
from ._hares import ControlSignal
from ._hares import PyTelemetry as Telemetry

# Equipment types
from ._hares import Battery
from ._hares import PV
from ._hares import PvSoilingConfig
from ._hares import EV

# Actor system
from ._hares import Actor
from ._hares import DispatchRequest
from ._hares import Priority
from ._hares import Signal

# Enums
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

# Metrics types
from ._hares import PySimulationMetrics as SimulationMetrics
from ._hares import PyAnnualEnergyKwh as AnnualEnergyKwh
from ._hares import PyPeakPowerKw as PeakPowerKw
from ._hares import PyRollingPeakKw as RollingPeakKw
from ._hares import PyGridInteractionMetrics as GridInteractionMetrics
from ._hares import PyEnvelopeComponentLoadsKwh as EnvelopeComponentLoadsKwh
from ._hares import PyEfficiencyMetrics as EfficiencyMetrics
from ._hares import PyGasEnergyMetrics as GasEnergyMetrics

# PV sizing types
from ._hares import RoofPlane
from ._hares import PvCandidate
from ._hares import PvSizingResult

# Weather utilities
from ._hares import (
    parse_weather,
    parse_epw,
    parse_psm3,
    parse_tmy3,
    parse_resstock_csv,
    WeatherTimeSeries,
)

# Gym / RL
from ._hares import batch_step

__all__ = [
    # Core simulation
    "Dwelling",
    "Fleet",
    "FleetResults",
    "SimulationConfig",
    "DwellingConfig",
    "ControlSignal",
    "Telemetry",
    # Equipment
    "Battery",
    "PV",
    "PvSoilingConfig",
    "EV",
    # Actor system
    "Actor",
    "DispatchRequest",
    "Priority",
    "Signal",
    # Enums
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
    # Metrics
    "SimulationMetrics",
    "AnnualEnergyKwh",
    "PeakPowerKw",
    "RollingPeakKw",
    "GridInteractionMetrics",
    "EnvelopeComponentLoadsKwh",
    "EfficiencyMetrics",
    "GasEnergyMetrics",
    # PV sizing
    "RoofPlane",
    "PvCandidate",
    "PvSizingResult",
    # Weather
    "parse_weather",
    "parse_epw",
    "parse_psm3",
    "parse_tmy3",
    "parse_resstock_csv",
    "WeatherTimeSeries",
    # Gym / RL
    "batch_step",
]
