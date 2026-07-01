"""HELICS fleet-as-single-federate co-simulation orchestrator."""

from __future__ import annotations

import json
import logging
from typing import Any

try:
    import helics
except ImportError as exc:  # pragma: no cover - exercised via import test
    raise ImportError(
        "HELICS not installed. Install with: pip install 'ochre_next[helics]'"
    ) from exc

from ochre_next import ControlSignal
from ochre_next._hares import SteppableFleet as PySteppableFleet

from ._types import HelicsFederateInfoLike, HelicsPublicationLike, HelicsSubscriptionLike
from .broker import allocate_ephemeral_port
from .dwelling import HELICSPublicationConfig, HELICSSubscriptionConfig, _handle_time_grant

_LOG = logging.getLogger(__name__)


class HELICSFleet:
    """Wrap a ``PySteppableFleet`` as a single HELICS value federate.

    Args:
        fleet: Initialized ``PySteppableFleet`` instance.
        fed_name: Unique name for this federate in the HELICS federation.
        broker_address: Broker address (host or host:port or tcp://host:port).
        core_type: HELICS core transport (e.g. ``"zmq"``).
        time_offset_s: HELICS time offset in seconds.  Use a positive offset
            so this federate steps after an aggregator at the same granted time.
    """

    def __init__(
        self,
        fleet: PySteppableFleet,
        fed_name: str,
        broker_address: str = "localhost",
        core_type: str = "zmq",
        time_offset_s: float = 0.0,
    ) -> None:
        self._fleet = fleet
        self._fed_name = fed_name
        self._broker_address = broker_address
        self._core_type = core_type
        self._time_offset_s = time_offset_s

        self._time_res_s = float(fleet.time_res_s())
        self._total_steps = int(fleet.total_steps())
        self._n_dwellings = len(fleet)

        fedinfo = self._create_federate_info()
        self._configure_federate_info(fedinfo)
        self._fed = helics.helicsCreateValueFederate(fed_name, fedinfo)

        self._set_flag(helics.HELICS_FLAG_TERMINATE_ON_ERROR, True)

        self._pub_aggregate_power: HelicsPublicationLike | None = None
        self._pub_aggregate_reactive: HelicsPublicationLike | None = None
        self._pub_dwelling_power: list[HelicsPublicationLike] = []
        self._pub_dwelling_reactive: list[HelicsPublicationLike] = []

        self._sub_voltage_all: HelicsSubscriptionLike | None = None
        self._sub_voltage_dwelling: list[HelicsSubscriptionLike] = []
        self._sub_control: HelicsSubscriptionLike | None = None

        self._publication_configs: list[HELICSPublicationConfig] = []
        self._subscription_configs: list[HELICSSubscriptionConfig] = []
        self._finalized = False
        self._federation_terminated = False

    def register_publications(self, prefix: str = "") -> list[HELICSPublicationConfig]:
        """Register aggregate and per-dwelling typed double publications.

        HELICS prepends the federate name to every local publication name, so
        the name passed to ``register_publication`` must omit ``fed_name`` --
        passing the fully-qualified key here would register
        ``fed_name/fed_name/...`` instead of ``fed_name/...``.

        The ``prefix`` is a namespace prepended to the *config* key returned
        to callers (e.g. ``"grid/"``), not to the local publication name.
        """
        aggregate_power_name = "aggregate_power_kw"
        aggregate_reactive_name = "aggregate_reactive_kvar"
        self._pub_aggregate_power = self._fed.register_publication(aggregate_power_name, "double")
        self._pub_aggregate_reactive = self._fed.register_publication(aggregate_reactive_name, "double")

        self._pub_dwelling_power = []
        self._pub_dwelling_reactive = []
        configs = [
            HELICSPublicationConfig(key=f"{prefix}{self._fed_name}/{aggregate_power_name}"),
            HELICSPublicationConfig(key=f"{prefix}{self._fed_name}/{aggregate_reactive_name}"),
        ]

        for dwelling_index in range(self._n_dwellings):
            power_name = f"dwelling_{dwelling_index}/total_power_kw"
            reactive_name = f"dwelling_{dwelling_index}/reactive_power_kvar"
            self._pub_dwelling_power.append(self._fed.register_publication(power_name, "double"))
            self._pub_dwelling_reactive.append(self._fed.register_publication(reactive_name, "double"))
            configs.append(HELICSPublicationConfig(key=f"{prefix}{self._fed_name}/{power_name}"))
            configs.append(HELICSPublicationConfig(key=f"{prefix}{self._fed_name}/{reactive_name}"))

        self._publication_configs = configs
        return list(self._publication_configs)

    def register_subscriptions(
        self,
        voltage_topic: str | None = None,
        per_dwelling_voltage_topics: list[str] | None = None,
        control_topic: str | None = None,
    ) -> list[HELICSSubscriptionConfig]:
        """Register fleet-wide/per-dwelling voltage and control subscriptions.

        Args:
            voltage_topic: Fleet-wide voltage topic (applied to all dwellings).
            per_dwelling_voltage_topics: Explicit per-dwelling voltage topics.
                Length must match the fleet size.  These are only registered when
                provided -- they are **not** auto-derived from ``voltage_topic``.
            control_topic: JSON control topic for equipment setpoints.
        """
        configs: list[HELICSSubscriptionConfig] = []

        self._sub_voltage_dwelling = []
        if voltage_topic is not None:
            self._sub_voltage_all = self._fed.register_subscription(voltage_topic, "double")
            configs.append(HELICSSubscriptionConfig(key=voltage_topic, type="double"))
        else:
            self._sub_voltage_all = None

        if per_dwelling_voltage_topics is not None:
            if len(per_dwelling_voltage_topics) != self._n_dwellings:
                raise ValueError(
                    f"per_dwelling_voltage_topics length ({len(per_dwelling_voltage_topics)}) "
                    f"must match fleet size ({self._n_dwellings})"
                )
            for key in per_dwelling_voltage_topics:
                subscription = self._fed.register_subscription(key, "double")
                self._sub_voltage_dwelling.append(subscription)
                configs.append(HELICSSubscriptionConfig(key=key, type="double"))

        if control_topic is not None:
            self._sub_control = self._fed.register_subscription(control_topic, "string")
            configs.append(HELICSSubscriptionConfig(key=control_topic, type="string"))
        else:
            self._sub_control = None

        self._subscription_configs = configs
        return list(self._subscription_configs)

    def run(self) -> None:
        """Run HELICS-coupled stepping loop until fleet timesteps are exhausted.

        Each fleet step advances state from ``sim_time_s`` to
        ``sim_time_s + time_res_s``, so the HELICS time to request is the
        *exit* time of the interval, not its entry time -- a federate is
        already at simulation time 0 immediately after
        ``enter_executing_mode()``, and HELICS never re-grants a time at or
        before the federate's current time, so requesting entry time 0 for
        the first step would be granted the *next* period boundary instead,
        silently shifting every subsequent request one period late.
        """
        try:
            if self._pub_aggregate_power is None or self._pub_aggregate_reactive is None:
                raise RuntimeError("Publications are not registered; call register_publications() first")

            self._fed.enter_executing_mode()
            sim_time_s = 0.0
            step_count = 0
            while not self._fleet.is_finished():
                if step_count >= self._total_steps:
                    _LOG.warning(
                        "Fleet exceeded configured total_steps=%d; terminating loop defensively",
                        self._total_steps,
                    )
                    break

                exit_time_s = sim_time_s + self._time_res_s
                granted = float(self._fed.request_time(exit_time_s))
                while not _handle_time_grant(exit_time_s, granted):
                    self._publish_results()
                    granted = float(self._fed.request_time(exit_time_s))

                if granted >= helics.HELICS_TIME_MAXTIME:
                    self._federation_terminated = True
                    break

                self._read_subscriptions()
                self._fleet.step()
                self._publish_results()

                step_count += 1
                sim_time_s = exit_time_s
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
        if self._sub_voltage_all is not None and self._sub_voltage_all.is_updated():
            try:
                self._fleet.set_grid_voltage_all(float(self._sub_voltage_all.double))
            except Exception as exc:
                _LOG.warning("Failed to apply fleet-wide grid voltage: %s", exc)

        for dwelling_index, subscription in enumerate(self._sub_voltage_dwelling):
            if subscription.is_updated():
                try:
                    self._fleet.set_grid_voltage(dwelling_index, float(subscription.double))
                except Exception as exc:
                    _LOG.warning("Failed to apply grid voltage for dwelling %d: %s", dwelling_index, exc)

        if self._sub_control is None or not self._sub_control.is_updated():
            return

        try:
            payload = json.loads(self._sub_control.string)
        except (TypeError, ValueError) as exc:
            _LOG.warning("Invalid control payload JSON: %s", exc)
            return

        try:
            controls = self._iter_control_entries(payload)
        except ValueError as exc:
            _LOG.warning("Invalid control message shape: %s", exc)
            return

        for dwelling_index, equipment_name, signal_dict in controls:
            if dwelling_index < 0 or dwelling_index >= self._n_dwellings:
                _LOG.warning("Control payload references invalid dwelling index %d", dwelling_index)
                continue
            try:
                signal = ControlSignal.from_dict(signal_dict)
                self._fleet.apply_control(dwelling_index, equipment_name, signal)
            except (ValueError, TypeError, KeyError) as exc:
                _LOG.warning(
                    "Failed to apply control for dwelling %d equipment '%s': %s",
                    dwelling_index,
                    equipment_name,
                    exc,
                )

    def _publish_results(self) -> None:
        aggregate_power_kw = 0.0
        aggregate_reactive_kvar = 0.0
        for dwelling_index in range(self._n_dwellings):
            telemetry = self._fleet.telemetry(dwelling_index)
            power_kw = float(telemetry.total_power_kw)
            reactive_kvar = float(telemetry.reactive_power_kvar)

            aggregate_power_kw += power_kw
            aggregate_reactive_kvar += reactive_kvar

            if dwelling_index < len(self._pub_dwelling_power):
                self._pub_dwelling_power[dwelling_index].publish(power_kw)
            if dwelling_index < len(self._pub_dwelling_reactive):
                self._pub_dwelling_reactive[dwelling_index].publish(reactive_kvar)

        self._pub_aggregate_power.publish(aggregate_power_kw)  # type: ignore[union-attr]
        self._pub_aggregate_reactive.publish(aggregate_reactive_kvar)  # type: ignore[union-attr]

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
        # multi-rate co-simulations. The fleet's actual step cadence is
        # controlled by the while-loop over self._time_res_s, not by this property.
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

    def _iter_control_entries(self, payload: Any) -> list[tuple[int, str, dict[str, Any]]]:
        if not isinstance(payload, dict):
            raise ValueError("Control payload must decode to a JSON object")

        if not payload:
            return []

        if any(self._is_negative_int_like(key) for key in payload):
            raise ValueError("Control payload dwelling indices must be non-negative")

        keys_are_dwelling_indices = all(self._is_int_like(key) for key in payload)

        entries: list[tuple[int, str, dict[str, Any]]] = []
        if keys_are_dwelling_indices:
            for dwelling_key, body in payload.items():
                dwelling_index = int(dwelling_key)
                for equipment, signal in self._iter_equipment_entries(body):
                    entries.append((dwelling_index, equipment, signal))
            return entries

        if any(self._is_int_like(key) for key in payload):
            raise ValueError("Control payload may not mix dwelling indices with equipment names")

        for equipment, signal in self._iter_equipment_entries(payload):
            for dwelling_index in range(self._n_dwellings):
                entries.append((dwelling_index, equipment, signal))
        return entries

    @staticmethod
    def _is_int_like(value: Any) -> bool:
        if isinstance(value, int):
            return value >= 0
        if not isinstance(value, str):
            return False
        try:
            return int(value) >= 0
        except ValueError:
            return False

    @staticmethod
    def _is_negative_int_like(value: Any) -> bool:
        if isinstance(value, int):
            return value < 0
        if not isinstance(value, str):
            return False
        try:
            return int(value) < 0
        except ValueError:
            return False

    @staticmethod
    def _iter_equipment_entries(body: Any) -> list[tuple[str, dict[str, Any]]]:
        if not isinstance(body, dict):
            raise ValueError("Each control entry must be a JSON object")

        if "equipment" in body and "signal" in body:
            equipment = body["equipment"]
            signal = body["signal"]
            if not isinstance(equipment, str) or not isinstance(signal, dict):
                raise ValueError("Single-control payload requires string equipment + dict signal")
            return [(equipment, signal)]

        entries: list[tuple[str, dict[str, Any]]] = []
        for equipment, signal in body.items():
            if not isinstance(equipment, str) or not isinstance(signal, dict):
                raise ValueError("Multi-control payload must be {str: dict}")
            entries.append((equipment, signal))
        return entries
