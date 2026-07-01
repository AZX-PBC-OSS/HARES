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
from .broker import allocate_ephemeral_port

_LOG = logging.getLogger(__name__)


def _handle_time_grant(requested: float, granted: float) -> bool:
    """Return ``True`` when the dwelling should advance model state on this grant.

    HELICS guarantees ``granted <= requested``.  In multi-rate co-simulations
    ``granted < requested`` indicates an intermediate grant from a faster
    federate — the caller should publish current results without stepping and
    re-request the same ``requested`` time.

    ``granted > requested`` is a HELICS invariant violation that the HELICS
    runtime should prevent — if it occurs, step anyway and log a warning.
    """
    if granted > requested:
        _LOG.warning(
            "HELICS invariant violation: granted %.3f > requested %.3f",
            granted,
            requested,
        )
        return True
    if granted < requested:
        _LOG.info(
            "Multi-rate grant: requested=%.1fs granted=%.1fs delta=%.1fs",
            requested,
            granted,
            requested - granted,
        )
        return False
    return True


@dataclass(frozen=True, slots=True)
class HELICSPublicationConfig:
    key: str
    type: str = "double"


@dataclass(frozen=True, slots=True)
class HELICSSubscriptionConfig:
    key: str
    type: str = "double"


class HELICSDwelling:
    """Wrap a single ``PyDwelling`` with HELICS federate lifecycle management.

    Args:
        dwelling: Initialized ``PyDwelling`` instance.
        fed_name: Unique name for this federate in the HELICS federation.
        broker_address: Broker address (host or host:port or tcp://host:port).
        core_type: HELICS core transport (e.g. ``"zmq"``).
        time_offset_s: HELICS time offset in seconds.  Use a positive offset
            (e.g. ``1.0``) so that this federate steps *after* an aggregator
            federate at the same granted time, allowing the aggregator to
            publish control signals before the dwelling reads them.
    """

    def __init__(
        self,
        dwelling: PyDwelling,
        fed_name: str,
        broker_address: str = "localhost",
        core_type: str = "zmq",
        time_offset_s: float = 0.0,
    ) -> None:
        self._dwelling = dwelling
        self._fed_name = fed_name
        self._broker_address = broker_address
        self._core_type = core_type
        self._time_offset_s = time_offset_s

        self._start_time, self._period_s = self._peek_timing(dwelling)

        fedinfo = self._create_federate_info()
        self._configure_federate_info(fedinfo)
        self._fed = helics.helicsCreateValueFederate(fed_name, fedinfo)

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
        """Register typed double publications and return config metadata.

        HELICS prepends the federate name to every local publication name, so
        the name passed to ``register_publication`` must omit ``fed_name`` --
        passing the fully-qualified key here would register
        ``fed_name/fed_name/...`` instead of ``fed_name/...``.

        The ``prefix`` is a namespace prepended to the *config* key returned
        to callers (e.g. ``"grid/"``), not to the local publication name.
        """
        power_name = "total_power_kw"
        reactive_name = "reactive_power_kvar"

        self._pub_power = self._fed.register_publication(power_name, "double")
        self._pub_reactive = self._fed.register_publication(reactive_name, "double")

        self._publication_configs = [
            HELICSPublicationConfig(key=f"{prefix}{self._fed_name}/{power_name}"),
            HELICSPublicationConfig(key=f"{prefix}{self._fed_name}/{reactive_name}"),
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
        """Run HELICS-coupled stepping loop until dwelling timesteps are exhausted.

        Each dwelling timestep at ``start_time + n * period`` represents the
        state *before* ``step()`` advances it to ``start_time + (n+1) * period``.
        The HELICS time to request for that step is therefore the *exit* time
        of the interval (``(n+1) * period``), not the timestep's own entry
        time -- a federate is already at simulation time 0 immediately after
        ``enter_executing_mode()``, and HELICS never re-grants a time at or
        before the federate's current time, so requesting entry time 0 for
        the first step would be granted the *next* period boundary instead,
        silently shifting every subsequent request one period late.
        """
        try:
            if self._pub_power is None or self._pub_reactive is None:
                raise RuntimeError("Publications are not registered; call register_publications() first")

            self._fed.enter_executing_mode()
            for timestamp in self._timesteps:
                exit_time_s = (timestamp - self._start_time).total_seconds() + self._period_s
                granted = float(self._fed.request_time(exit_time_s))
                while not _handle_time_grant(exit_time_s, granted):
                    self._publish_results()
                    granted = float(self._fed.request_time(exit_time_s))
                self._read_subscriptions()
                self._dwelling.step()
                self._publish_results()
            # Signal completion to the HELICS federation. A federate that has
            # no further time steps must inform the broker so it can coordinate
            # clean shutdown. Without this handshake other federates may stall
            # at their next request_time() call until the broker detects the
            # disconnect after the default federate timeout (30-120s).
            try:
                _LOG.info(
                    "HELICS federate %s signalling completion via request_time(HELICS_TIME_MAXTIME)",
                    self._fed_name,
                )
                self._fed.request_time(helics.HELICS_TIME_MAXTIME)
            except Exception:
                _LOG.warning(
                    "HELICS federate %s request_time(HELICS_TIME_MAXTIME) failed during completion signalling",
                    self._fed_name,
                    exc_info=True,
                )
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
            try:
                self._dwelling.set_grid_voltage(float(self._sub_voltage.double))
            except Exception as exc:
                _LOG.warning("Failed to apply grid voltage: %s", exc)

        if self._sub_price is not None and self._sub_price.is_updated():
            try:
                price = float(self._sub_price.double)
                self._dwelling.set_price_signal({"electricity_price": price})
            except Exception as exc:
                _LOG.warning("Failed to apply price signal: %s", exc)

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
        telemetry = self._dwelling.telemetry()
        self._pub_power.publish(float(telemetry.total_power_kw))  # type: ignore[union-attr]
        self._pub_reactive.publish(float(telemetry.reactive_power_kvar))  # type: ignore[union-attr]

    def _peek_timing(self, dwelling: PyDwelling) -> tuple[datetime, float]:
        """Infer start time and period from the first two timesteps.

        Requires ``dwelling.timesteps()`` to return a fresh iterator per call;
        the consumed items are chained back so the ``run()`` loop sees all steps.
        """
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

        if hasattr(helics, "helicsFederateInfoSetCoreName"):
            helics.helicsFederateInfoSetCoreName(fedinfo, f"core_{self._fed_name}")

        broker_address = self._normalize_broker_address(self._broker_address)
        # Each federate's core needs its own local ZMQ listen port. Without an
        # explicit --port, multiple auto-named cores in the same process can
        # collide on the auto-assigned local port and silently deadlock at
        # enterExecutingMode instead of raising a bind error (reproduced on
        # macOS/arm64 with HELICS 3.6.1).
        local_port = allocate_ephemeral_port()
        core_init_value = f"--broker_address={broker_address} --port={local_port}"
        if hasattr(helics, "helicsFederateInfoSetCoreInitString"):
            helics.helicsFederateInfoSetCoreInitString(fedinfo, core_init_value)
        else:
            if hasattr(fedinfo, "core_init"):
                fedinfo.core_init = core_init_value
            if hasattr(fedinfo, "core_init_string"):
                fedinfo.core_init_string = core_init_value

        # Set time period to 0 (no minimum step constraint) so HELICS can grant
        # at any time — including intermediate grants from faster federates in
        # multi-rate co-simulations. The dwelling's actual step cadence is
        # controlled by the for-loop over self._timesteps, not by this property.
        # A non-zero period would prevent grants at non-period-aligned times,
        # foreclosing the while-loop's intermediate-grant branch.
        self._set_time_property(
            fedinfo,
            helics.HELICS_PROPERTY_TIME_PERIOD,
            0.0,
        )

        if self._time_offset_s != 0.0:
            self._set_time_property(
                fedinfo,
                helics.HELICS_PROPERTY_TIME_OFFSET,
                self._time_offset_s,
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
