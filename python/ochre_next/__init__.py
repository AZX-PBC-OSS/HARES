"""HARES: High-performance Architecture for Residential Energy Simulation."""

from __future__ import annotations

from ._hares import Dwelling
from ._hares import Fleet
from ._hares import FleetResults
from ._hares import SimulationConfig
from ._hares import DwellingConfig
from ._hares import ControlSignal
from ._hares import Telemetry

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
    BatteryProductId,
    ChargingLevel,
    VehicleType,
    EvConnectionState,
    PlugInPolicy,
    ChargingStrategy,
    VehicleId,
    EvArchetypeId,
    DRLevel,
    EquipmentDescriptor,
    TelemetryField,
    BmsMode,
    BmsAction,
    BmsScheduleWindow,
    GridExportRule,
    StormWatchTrigger,
    DepartureConstraint,
)

# Metrics types
from ._hares import SimulationMetrics
from ._hares import AnnualEnergyKwh
from ._hares import PeakPowerKw
from ._hares import RollingPeakKw
from ._hares import GridInteractionMetrics
from ._hares import EnvelopeComponentLoadsKwh
from ._hares import EfficiencyMetrics
from ._hares import GasEnergyMetrics

# Tariff types
from ._hares import ElectricTariff
from ._hares import TariffBuilder
from ._hares import GasTariff
from ._hares import GasTariffBuilder
from ._hares import BillingPeriodSummary
from ._hares import TariffTelemetry

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

# Iterator types
from ._hares import TimestepsIter

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
    "BatteryProductId",
    "ChargingLevel",
    "VehicleType",
    "EvConnectionState",
    "PlugInPolicy",
    "ChargingStrategy",
    "VehicleId",
    "EvArchetypeId",
    "DRLevel",
    "EquipmentDescriptor",
    "TelemetryField",
    "BmsMode",
    "BmsAction",
    "BmsScheduleWindow",
    "GridExportRule",
    "StormWatchTrigger",
    "DepartureConstraint",
    # Metrics
    "SimulationMetrics",
    "AnnualEnergyKwh",
    "PeakPowerKw",
    "RollingPeakKw",
    "GridInteractionMetrics",
    "EnvelopeComponentLoadsKwh",
    "EfficiencyMetrics",
    "GasEnergyMetrics",
    # Tariffs
    "ElectricTariff",
    "TariffBuilder",
    "GasTariff",
    "GasTariffBuilder",
    "BillingPeriodSummary",
    "TariffTelemetry",
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
    # Iterator types
    "TimestepsIter",
    # Gym / RL
    "batch_step",
]
