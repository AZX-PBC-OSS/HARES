"""HELICS single-dwelling co-simulation orchestrator."""

from __future__ import annotations

from collections.abc import Iterator
from dataclasses import dataclass
from datetime import datetime
import json
import logging
from itertools import chain
from typing import Any

try:
    import helics
except ImportError as exc:  # pragma: no cover - exercised via import test
    raise ImportError(
        "HELICS not installed. Install with: pip install 'ochre_next[helics]'"
    ) from exc

from ochre_next import ControlSignal
from ochre_next._hares import Dwelling as PyDwelling

from ._types import HelicsFederateInfoLike, HelicsPublicationLike, HelicsSubscriptionLike

_LOG = logging.getLogger(__name__)


@dataclass(frozen=True, slots=True)
class HELICSPublicationConfig:
    key: str
    type: str = "double"


@dataclass(frozen=True, slots=True)
class HELICSSubscriptionConfig:
    key: str
    type: str = "double"


class HELICSDwelling:
    """Wrap a single ``PyDwelling`` with HELICS federate lifecycle management."""

    def __init__(
        self,
        dwelling: PyDwelling,
        fed_name: str,
        broker_address: str = "localhost",
        core_type: str = "zmq",
    ) -> None:
        self._dwelling = dwelling
        self._fed_name = fed_name
        self._broker_address = broker_address
        self._core_type = core_type

        self._start_time, self._period_s = self._peek_timing(dwelling)

        fedinfo = self._create_federate_info()
        self._configure_federate_info(fedinfo)
        self._fed = helics.helicsCreateValueFederate(fed_name, fedinfo)

        self._set_flag(helics.HELICS_FLAG_UNINTERRUPTIBLE, True)
        self._set_flag(helics.HELICS_FLAG_TERMINATE_ON_ERROR, True)

        self._pub_power: HelicsPublicationLike | None = None
        self._pub_reactive: HelicsPublicationLike | None = None
        self._sub_voltage: HelicsSubscriptionLike | None = None
        self._sub_control: HelicsSubscriptionLike | None = None
        self._sub_price: HelicsSubscriptionLike | None = None

        self._publication_configs: list[HELICSPublicationConfig] = []
        self._subscription_configs: list[HELICSSubscriptionConfig] = []
        self._finalized = False

    def register_publications(self, prefix: str = "") -> list[HELICSPublicationConfig]:
        """Register typed double publications and return config metadata."""
        base = f"{prefix}{self._fed_name}/"

        power_key = f"{base}total_power_kw"
        reactive_key = f"{base}reactive_power_kvar"

        self._pub_power = self._fed.register_publication(power_key, "double")
        self._pub_reactive = self._fed.register_publication(reactive_key, "double")

        self._publication_configs = [
            HELICSPublicationConfig(key=power_key),
            HELICSPublicationConfig(key=reactive_key),
        ]
        return list(self._publication_configs)

    def register_subscriptions(
        self,
        voltage_topic: str | None = None,
        control_topic: str | None = None,
        price_topic: str | None = None,
    ) -> list[HELICSSubscriptionConfig]:
        """Register subscriptions for grid voltage, controls, and price."""
        configs: list[HELICSSubscriptionConfig] = []

        if voltage_topic is not None:
            self._sub_voltage = self._fed.register_subscription(voltage_topic, "double")
            configs.append(HELICSSubscriptionConfig(key=voltage_topic, type="double"))

        if control_topic is not None:
            self._sub_control = self._fed.register_subscription(control_topic, "string")
            configs.append(HELICSSubscriptionConfig(key=control_topic, type="string"))

        if price_topic is not None:
            self._sub_price = self._fed.register_subscription(price_topic, "double")
            configs.append(HELICSSubscriptionConfig(key=price_topic, type="double"))

        self._subscription_configs = configs
        return list(self._subscription_configs)

    def run(self) -> None:
        """Run HELICS-coupled stepping loop until dwelling timesteps are exhausted."""
        try:
            self._fed.enter_executing_mode()
            for timestamp in self._timesteps:
                sim_time_s = (timestamp - self._start_time).total_seconds()
                self._fed.request_time(sim_time_s)
                self._read_subscriptions()
                self._dwelling.step()
                self._publish_results()
        finally:
            self.finalize()

    def finalize(self) -> None:
        """Disconnect federate explicitly (idempotent)."""
        if self._finalized:
            return
        self._fed.disconnect()
        self._finalized = True

    def _read_subscriptions(self) -> None:
        if self._sub_voltage is not None and self._sub_voltage.is_updated():
            self._dwelling.set_grid_voltage(float(self._sub_voltage.double))

        if self._sub_price is not None and self._sub_price.is_updated():
            price = float(self._sub_price.double)
            self._dwelling.set_price_signal({"electricity_price": price})

        if self._sub_control is None or not self._sub_control.is_updated():
            return

        payload = self._sub_control.string
        try:
            control_message = json.loads(payload)
        except (TypeError, ValueError) as exc:
            _LOG.warning("Invalid control payload JSON: %s", exc)
            return

        try:
            entries = self._iter_control_entries(control_message)
        except ValueError as exc:
            _LOG.warning("Invalid control message shape: %s", exc)
            return

        for equipment, signal_body in entries:
            try:
                signal = ControlSignal.from_dict(signal_body)
                self._dwelling.apply_control(equipment, signal)
            except (ValueError, TypeError, KeyError) as exc:
                _LOG.warning(
                    "Failed to apply control for equipment '%s': %s",
                    equipment,
                    exc,
                )

    def _publish_results(self) -> None:
        if self._pub_power is None or self._pub_reactive is None:
            raise RuntimeError("Publications are not registered; call register_publications() first")

        telemetry = self._dwelling.telemetry()
        self._pub_power.publish(float(telemetry.total_power_kw))
        self._pub_reactive.publish(float(telemetry.reactive_power_kvar))

    def _peek_timing(self, dwelling: PyDwelling) -> tuple[datetime, float]:
        iterator = iter(dwelling.timesteps())
        start_time = next(iterator)
        second_time = next(iterator, None)
        prefetch: list[datetime] = [start_time]

        period_s = 1.0
        if second_time is not None:
            period_s = (second_time - start_time).total_seconds()
            prefetch.append(second_time)
        else:
            _LOG.warning(
                "Dwelling timesteps() yielded a single timestamp; defaulting HELICS period to 1.0s"
            )

        # Reuse the same iterator stream so prefetched timestamps are not lost.
        self._timesteps: Iterator[datetime] = chain(prefetch, iterator)

        return start_time, period_s

    @staticmethod
    def _create_federate_info() -> HelicsFederateInfoLike:
        if hasattr(helics, "HelicsFederateInfo"):
            try:
                return helics.HelicsFederateInfo()
            except TypeError:
                # Some HELICS builds expose HelicsFederateInfo but require an internal handle.
                pass
        if hasattr(helics, "helicsCreateFederateInfo"):
            return helics.helicsCreateFederateInfo()
        raise RuntimeError("HELICS Python module does not expose federate info creation API")

    def _configure_federate_info(self, fedinfo: HelicsFederateInfoLike) -> None:
        if hasattr(helics, "helicsFederateInfoSetCoreTypeFromString"):
            helics.helicsFederateInfoSetCoreTypeFromString(fedinfo, self._core_type)
        else:
            fedinfo.core_type = self._core_type
        broker_address = self._normalize_broker_address(self._broker_address)
        core_init_value = f"--broker_address={broker_address}"
        if hasattr(helics, "helicsFederateInfoSetCoreInitString"):
            helics.helicsFederateInfoSetCoreInitString(fedinfo, core_init_value)
        else:
            if hasattr(fedinfo, "core_init"):
                fedinfo.core_init = core_init_value
            if hasattr(fedinfo, "core_init_string"):
                fedinfo.core_init_string = core_init_value

        self._set_time_property(
            fedinfo,
            helics.HELICS_PROPERTY_TIME_PERIOD,
            self._period_s,
        )

    @staticmethod
    def _set_time_property(
        fedinfo: HelicsFederateInfoLike, property_key: int, value: float
    ) -> None:
        if hasattr(fedinfo, "property"):
            fedinfo.property[property_key] = value
            return
        helics.helicsFederateInfoSetTimeProperty(fedinfo, property_key, value)

    def _set_flag(self, flag: int, enabled: bool) -> None:
        if hasattr(self._fed, "set_flag_option"):
            self._fed.set_flag_option(flag, enabled)
            return
        helics.helicsFederateSetFlagOption(self._fed, flag, int(enabled))

    @staticmethod
    def _normalize_broker_address(address: str) -> str:
        if "://" in address:
            return address
        return f"tcp://{address}"

    @staticmethod
    def _iter_control_entries(message: Any) -> list[tuple[str, dict[str, Any]]]:
        if not isinstance(message, dict):
            raise ValueError("Control payload must decode to a JSON object")

        if "equipment" in message and "signal" in message:
            equipment = message["equipment"]
            signal = message["signal"]
            if not isinstance(equipment, str) or not isinstance(signal, dict):
                raise ValueError("Single-equipment payload requires string equipment + dict signal")
            return [(equipment, signal)]

        entries: list[tuple[str, dict[str, Any]]] = []
        for equipment, signal in message.items():
            if not isinstance(equipment, str) or not isinstance(signal, dict):
                raise ValueError("Multi-equipment payload must be {str: dict}")
            entries.append((equipment, signal))
        return entries
