"""HARES: High-performance Agent-based Residential Energy Simulation."""

from __future__ import annotations

from ._hares import Dwelling
from ._hares import DwellingBlueprint
from ._hares import Fleet
from ._hares import SteppableFleet
from ._hares import FleetResults
from ._hares import SimulationConfig
from ._hares import DwellingConfig
from ._hares import ControlSignal
from ._hares import Telemetry

from ochre_next.simulation_plan import SimulationPlan, SimulationSegment

# Equipment types
from ._hares import Battery
from ._hares import CoreOutput
from ._hares import PV
from ._hares import PvSoilingConfig
from ._hares import EV
from ._hares import ProtocolBridge

# HVAC equipment types
from ._hares import GasFurnace
from ._hares import AirConditioner
from ._hares import ASHPHeater
from ._hares import ASHPCooler
from ._hares import ElectricBaseboard
from ._hares import ElectricBoiler
from ._hares import ElectricFurnace
from ._hares import GasBoiler
from ._hares import IdealHVAC

# Water heater equipment types
from ._hares import GasWaterHeater
from ._hares import ElectricResistanceWH
from ._hares import HeatPumpWH
from ._hares import IndirectTank
from ._hares import TanklessWaterHeater

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
    IdealCapacityMode,
    TelemetryField,
    BmsMode,
    BmsAction,
    BmsScheduleWindow,
    GridExportRule,
    StormWatchTrigger,
    DepartureConstraint,
    ExtrapolationStrategy,
    RoofShape,
)

# Metrics types
from ._hares import SimulationMetrics
from ._hares import TotalEnergyKwh
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
from ._hares import UsableRoofArea
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

# Standalone PV sizing functions
from ._hares import (
    compute_annual_diffuse_fraction,
    default_diffuse_fraction,
    is_north_facing,
    infer_roof_shape,
    compute_usable_area,
    enumerate_pv_candidates,
    size_pv_system,
    required_main_panel_ampacity,
)

# Iterator types
from ._hares import TimestepsIter

# Gym / RL
from ._hares import batch_step

__all__ = [
    # Core simulation
    "Dwelling",
    "DwellingBlueprint",
    "Fleet",
    "SteppableFleet",
    "FleetResults",
    "SimulationConfig",
    "DwellingConfig",
    "ControlSignal",
    "Telemetry",
    # Equipment
    "Battery",
    "CoreOutput",
    "PV",
    "PvSoilingConfig",
    "EV",
    "ProtocolBridge",
    # HVAC equipment
    "GasFurnace",
    "AirConditioner",
    "ASHPHeater",
    "ASHPCooler",
    "ElectricBaseboard",
    "ElectricBoiler",
    "ElectricFurnace",
    "GasBoiler",
    "IdealHVAC",
    # Water heater equipment
    "GasWaterHeater",
    "ElectricResistanceWH",
    "HeatPumpWH",
    "IndirectTank",
    "TanklessWaterHeater",
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
    "IdealCapacityMode",
    "TelemetryField",
    "BmsMode",
    "BmsAction",
    "BmsScheduleWindow",
    "GridExportRule",
    "StormWatchTrigger",
    "DepartureConstraint",
    "ExtrapolationStrategy",
    "RoofShape",
    # Metrics
    "SimulationMetrics",
    "TotalEnergyKwh",
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
    "UsableRoofArea",
    "PvCandidate",
    "PvSizingResult",
    # Weather
    "parse_weather",
    "parse_epw",
    "parse_psm3",
    "parse_tmy3",
    "parse_resstock_csv",
    "WeatherTimeSeries",
    # Standalone PV functions
    "compute_annual_diffuse_fraction",
    "default_diffuse_fraction",
    "is_north_facing",
    "infer_roof_shape",
    "compute_usable_area",
    "enumerate_pv_candidates",
    "size_pv_system",
    "required_main_panel_ampacity",
    # Iterator types
    "TimestepsIter",
    # Gym / RL
    "batch_step",
    # Multi-segment simulation
    "SimulationPlan",
    "SimulationSegment",
]
