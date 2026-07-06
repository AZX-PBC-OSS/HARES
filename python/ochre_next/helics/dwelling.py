"""HELICS single-dwelling co-simulation orchestrator.

Expected HELICS publication units
----------------------------------

All publication keys and their expected physical units:

``{prefix}{fed_name}/total_power_kw``
    Active power in kilowatts (kW).

``{prefix}{fed_name}/reactive_power_kvar``
    Reactive power in kilovolt-amperes reactive (kvar).

``{prefix}{fed_name}/zone_{index}/temp_air_c``
    Zone indoor air temperature in degrees Celsius (degC).

``{prefix}{fed_name}/equipment_{index}/power_kw``
    Per-equipment active power in kilowatts (kW).

``{prefix}{fed_name}/equipment_{index}/soc_pct``
    Per-equipment state of charge as a percentage (0-100).

``{prefix}{fed_name}/equipment_{index}/operating_mode``
    Per-equipment operating mode as a numeric code (see ``OperatingMode`` in
    ``hares-types``).

``{prefix}{fed_name}/pv_generation_kw``
    PV generation in kilowatts (kW), identified heuristically from equipment
    name and negative power.

Expected HELICS subscription units
-----------------------------------

``grid/voltage`` (or caller-provided topic)
    Grid voltage in per-unit (pu). Expected range: 0.5–1.5 pu.
    Voltage values in absolute volts (e.g. 240) are out of range and will
    trigger a diagnostic warning.  External federates must publish per-unit.

``grid/price`` (or caller-provided topic)
    Real-time electricity price in currency/kWh. Must be non-negative;
    a negative value triggers a diagnostic warning.
"""

from __future__ import annotations

from collections.abc import Iterator
from dataclasses import dataclass
from datetime import datetime
import json
import logging
import math
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

# Per-unit voltage range guards: values outside [0.5, 1.5] pu are physically
# implausible for a connected grid and indicate a likely unit mismatch between
# the external federate (publishing in volts) and HARES (expecting per-unit).
VOLTAGE_PU_MIN = 0.5
VOLTAGE_PU_MAX = 1.5

# Control signal value range guards.
# These Python-level guards mirror the Rust layer's validate_numeric_bounds()
# ranges so that out-of-range values produce a diagnostic warning at the HELICS
# boundary before deserialization, consistent with the Rust rejection in
# apply_control(). Mismatched ranges produce inconsistent diagnostics (valid
# per Python, rejected per Rust with no boundary warning).
#   ThermalSetpoint.heating_setpoint_c: [-50, 100] °C  (Rust: [-50, 100])
#   ThermalSetpoint.cooling_setpoint_c: [0, 60] °C      (Rust: [0, 60])
#   DutyCycle.on_fraction: [0, 1]
#   PowerSetpoint.active_power_kw: finite only (Rust: finite only)
THERMAL_SETPOINT_HEAT_MIN_C = -50.0
THERMAL_SETPOINT_HEAT_MAX_C = 100.0
THERMAL_SETPOINT_COOL_MIN_C = 0.0
THERMAL_SETPOINT_COOL_MAX_C = 60.0

# DutyCycle on_fraction must be in [0, 1].
DUTY_CYCLE_ON_FRACTION_MIN = 0.0
DUTY_CYCLE_ON_FRACTION_MAX = 1.0

# Number of consecutive timesteps a subscription can go without receiving
# data before a stale-subscription warning is emitted.
STALE_SUBSCRIPTION_THRESHOLD = 10


def _validate_control_signal(
    signal_body: dict[str, Any],
    equipment: str,
    logger: logging.Logger,
) -> int:
    """Validate control signal dict values at the HELICS boundary.

    Returns the count of clamped/flagged fields (0 if all values pass).
    """
    signal_type: str = signal_body.get("type", "")
    clamped = 0

    if signal_type == "ThermalSetpoint":
        for field, label, lo, hi in [
            ("heating_setpoint_c", "heating_setpoint_c", THERMAL_SETPOINT_HEAT_MIN_C, THERMAL_SETPOINT_HEAT_MAX_C),
            ("cooling_setpoint_c", "cooling_setpoint_c", THERMAL_SETPOINT_COOL_MIN_C, THERMAL_SETPOINT_COOL_MAX_C),
        ]:
            val = signal_body.get(field)
            if val is not None:
                try:
                    v = float(val)
                except (TypeError, ValueError):
                    clamped += 1
                    logger.warning(
                        "Control signal '%s' %s for equipment '%s' is not a number: %r; "
                        "expected [%.0f, %.0f] °C",
                        signal_type,
                        label,
                        equipment,
                        val,
                        lo,
                        hi,
                    )
                    continue
                if not math.isfinite(v) or v < lo or v > hi:
                    clamped += 1
                    logger.warning(
                        "Control signal '%s' %s for equipment '%s' = %.1f outside range [%.0f, %.0f] °C",
                        signal_type,
                        label,
                        equipment,
                        v,
                        lo,
                        hi,
                    )
    elif signal_type == "DutyCycle":
        val = signal_body.get("on_fraction")
        if val is not None:
            try:
                v = float(val)
            except (TypeError, ValueError):
                clamped += 1
                logger.warning(
                    "Control signal '%s' on_fraction for equipment '%s' is not a number: %r; "
                    "expected [%.1f, %.1f]",
                    signal_type,
                    equipment,
                    val,
                    DUTY_CYCLE_ON_FRACTION_MIN,
                    DUTY_CYCLE_ON_FRACTION_MAX,
                )
            else:
                if not math.isfinite(v) or v < DUTY_CYCLE_ON_FRACTION_MIN or v > DUTY_CYCLE_ON_FRACTION_MAX:
                    clamped += 1
                    logger.warning(
                        "Control signal '%s' on_fraction for equipment '%s' = %.3f outside range [%.1f, %.1f]",
                        signal_type,
                        equipment,
                        v,
                        DUTY_CYCLE_ON_FRACTION_MIN,
                        DUTY_CYCLE_ON_FRACTION_MAX,
                    )
    elif signal_type == "PowerSetpoint":
        val = signal_body.get("active_power_kw")
        if val is not None:
            try:
                v = float(val)
            except (TypeError, ValueError):
                clamped += 1
                logger.warning(
                    "Control signal '%s' active_power_kw for equipment '%s' is not a number: %r",
                    signal_type,
                    equipment,
                    val,
                )
            else:
                if not math.isfinite(v):
                    clamped += 1
                    logger.warning(
                        "Control signal '%s' active_power_kw for equipment '%s' = %.2f is not finite",
                        signal_type,
                        equipment,
                        v,
                    )

    return clamped


def _handle_time_grant(requested: float, granted: float) -> bool:
    """Return ``True`` when the dwelling should advance model state on this grant.

    HELICS guarantees ``granted <= requested``.  In multi-rate co-simulations
    ``granted < requested`` indicates an intermediate grant from a faster
    federate — the caller should publish current results without stepping and
    re-request the same ``requested`` time.

    ``HELICS_TIME_MAXTIME`` is the documented HELICS sentinel for federation
    termination (``helicsFederateRequestTime`` returns it when the federation
    has been terminated). It is recognised as an expected, handled condition —
    not an invariant violation.

    ``granted > requested`` with a non-sentinel value is a HELICS invariant
    violation that the HELICS runtime should prevent — if it occurs, step
    anyway and log a warning.
    """
    if granted >= helics.HELICS_TIME_MAXTIME:
        _LOG.warning(
            "HELICS federation terminated prematurely; requested=%.1f, granted=HELICS_TIME_MAXTIME",
            requested,
        )
        return True
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


def _set_publication_info(pub: HelicsPublicationLike, info: str) -> None:
    """Attach unit metadata to a HELICS publication via ``setInfo()``.

    HELICS ``setInfo()`` stores an arbitrary string that external federates can
    retrieve with ``helicsPublicationGetInfo()`` / ``helicsInputGetInfo()`` at
    the subscription side, so downstream consumers can query expected units
    programmatically rather than inferring them from the publication key.

    Calling ``setInfo()`` is a best-effort metadata operation — if the HELICS
    bindings do not expose the API (unusual but possible with older wrappers),
    the call is silently skipped.
    """
    try:
        if hasattr(pub, "set_info"):
            pub.set_info(info)
        elif hasattr(helics, "helicsPublicationSetInfo"):
            helics.helicsPublicationSetInfo(pub, info)
    except Exception:
        pass


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

    All federates must use the same time period for correct data exchange.
    HELICS does not enforce period alignment — it grants time at the minimum
    period across all federates. Mismatched periods cause silent data
    misalignment: the slower federate publishes data at unintended times.

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
        _LOG.info("HELICS federate %s period %.1fs derived from dwelling timesteps", fed_name, self._period_s)

        fedinfo = self._create_federate_info()
        self._configure_federate_info(fedinfo)
        self._fed = helics.helicsCreateValueFederate(fed_name, fedinfo)

        self._set_flag(helics.HELICS_FLAG_TERMINATE_ON_ERROR, True)

        self._pub_power: HelicsPublicationLike | None = None
        self._pub_reactive: HelicsPublicationLike | None = None
        self._sub_voltage: HelicsSubscriptionLike | None = None
        self._sub_control: HelicsSubscriptionLike | None = None
        self._sub_price: HelicsSubscriptionLike | None = None

        self._pub_zone_temp: list[HelicsPublicationLike] = []
        self._pub_equipment_power: list[HelicsPublicationLike] = []
        self._pub_equipment_soc: list[HelicsPublicationLike] = []
        self._pub_equipment_mode: list[HelicsPublicationLike] = []
        self._pub_pv_generation: HelicsPublicationLike | None = None

        self._publication_configs: list[HELICSPublicationConfig] = []
        self._subscription_configs: list[HELICSSubscriptionConfig] = []
        self._finalized = False
        self._federation_terminated = False
        self._last_voltage_out_of_range = False
        self._last_price_negative = False
        self._control_clamped = 0
        self._range_violations_total = 0
        self._stale_counts: dict[str, int] = {}
        self._update_mask: int = 0
        self._stale_subscription_count: int = 0
        self._required_publication_keys: set[str] = set()
        self._required_subscription_keys: set[str] = set()

    def register_publications(
        self,
        prefix: str = "",
        required_keys: set[str] | None = None,
    ) -> list[HELICSPublicationConfig]:
        """Register typed double publications and return config metadata.

        HELICS prepends the federate name to every local publication name, so
        the name passed to ``register_publication`` must omit ``fed_name`` --
        passing the fully-qualified key here would register
        ``fed_name/fed_name/...`` instead of ``fed_name/...``.

        The ``prefix`` is a namespace prepended to the *config* key returned
        to callers (e.g. ``"grid/"``), not to the local publication name.

        Args:
            prefix: Namespace prepended to each returned config key.
            required_keys: Optional set of fully-qualified publication keys that
                must be registered. After entering execution mode, any missing
                required key is logged as an error.
        """
        power_name = "total_power_kw"
        reactive_name = "reactive_power_kvar"

        self._pub_power = self._fed.register_publication(power_name, "double")
        self._pub_reactive = self._fed.register_publication(reactive_name, "double")

        _set_publication_info(self._pub_power, "units=kW")
        _set_publication_info(self._pub_reactive, "units=kvar")

        configs: list[HELICSPublicationConfig] = [
            HELICSPublicationConfig(key=f"{prefix}{self._fed_name}/{power_name}"),
            HELICSPublicationConfig(key=f"{prefix}{self._fed_name}/{reactive_name}"),
        ]

        # Zone temperature publications
        zonedata = self._dwelling.telemetry().zone()
        zone_names: list[str] = list(zonedata.get("names", []))
        self._pub_zone_temp = []
        for zi in range(len(zone_names)):
            key = f"zone_{zi}/temp_air_c"
            pub = self._fed.register_publication(key, "double")
            _set_publication_info(pub, "units=degC")
            self._pub_zone_temp.append(pub)
            configs.append(HELICSPublicationConfig(key=f"{prefix}{self._fed_name}/{key}"))

        # Per-equipment publications: power, SOC, operating mode
        equipdata = self._dwelling.telemetry().equipment()
        equip_names: list[str] = list(equipdata.get("names", []))
        self._pub_equipment_power = []
        self._pub_equipment_soc = []
        self._pub_equipment_mode = []
        for ei in range(len(equip_names)):
            power_key = f"equipment_{ei}/power_kw"
            soc_key = f"equipment_{ei}/soc_pct"
            mode_key = f"equipment_{ei}/operating_mode"

            pub_power = self._fed.register_publication(power_key, "double")
            _set_publication_info(pub_power, "units=kW")
            self._pub_equipment_power.append(pub_power)
            configs.append(HELICSPublicationConfig(key=f"{prefix}{self._fed_name}/{power_key}"))

            pub_soc = self._fed.register_publication(soc_key, "double")
            _set_publication_info(pub_soc, "units=pct")
            self._pub_equipment_soc.append(pub_soc)
            configs.append(HELICSPublicationConfig(key=f"{prefix}{self._fed_name}/{soc_key}"))

            pub_mode = self._fed.register_publication(mode_key, "double")
            _set_publication_info(pub_mode, "units=enum")
            self._pub_equipment_mode.append(pub_mode)
            configs.append(HELICSPublicationConfig(key=f"{prefix}{self._fed_name}/{mode_key}"))

        # PV generation publication
        pv_key = "pv_generation_kw"
        self._pub_pv_generation = self._fed.register_publication(pv_key, "double")
        _set_publication_info(self._pub_pv_generation, "units=kW")
        configs.append(HELICSPublicationConfig(key=f"{prefix}{self._fed_name}/{pv_key}"))

        self._publication_configs = configs
        if required_keys is not None:
            self._required_publication_keys = set(required_keys)
        _LOG.info(
            "HELICS federate %s registered %d publications: %s",
            self._fed_name,
            len(configs),
            [c.key for c in configs],
        )
        return list(self._publication_configs)

    def register_subscriptions(
        self,
        voltage_topic: str | None = None,
        control_topic: str | None = None,
        price_topic: str | None = None,
        required_keys: set[str] | None = None,
    ) -> list[HELICSSubscriptionConfig]:
        """Register subscriptions for grid voltage, controls, and price.

        Args:
            voltage_topic: HELICS topic for grid voltage (per-unit).
            control_topic: HELICS topic for JSON control messages.
            price_topic: HELICS topic for real-time electricity price.
            required_keys: Optional set of fully-qualified subscription keys that
                must be registered. After entering execution mode, any missing
                required key is logged as an error.
        """
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
        if required_keys is not None:
            self._required_subscription_keys = set(required_keys)
        _LOG.info(
            "HELICS federate %s registered %d subscriptions: %s",
            self._fed_name,
            len(configs),
            [c.key for c in configs],
        )
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
            self._verify_registration_completeness()
            self._verify_helics_counts()
            for timestamp in self._timesteps:
                exit_time_s = (timestamp - self._start_time).total_seconds() + self._period_s
                granted = float(self._fed.request_time(exit_time_s))
                while not _handle_time_grant(exit_time_s, granted):
                    self._publish_results()
                    granted = float(self._fed.request_time(exit_time_s))
                if granted >= helics.HELICS_TIME_MAXTIME:
                    self._federation_terminated = True
                    break
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
        self._last_voltage_out_of_range = False
        self._last_price_negative = False
        self._control_clamped = 0
        self._update_mask = 0
        self._stale_subscription_count = 0

        # Call is_updated() exactly once per registered subscription so that
        # stale tracking and business logic share the same result.  Building
        # _sub_objs in the same order as register_subscriptions ensures the
        # zip below matches _subscription_configs.
        _sub_objs: list[HelicsSubscriptionLike] = []
        if self._sub_voltage is not None:
            _sub_objs.append(self._sub_voltage)
        if self._sub_control is not None:
            _sub_objs.append(self._sub_control)
        if self._sub_price is not None:
            _sub_objs.append(self._sub_price)

        _updates: list[bool] = []
        for sub_obj in _sub_objs:
            _updates.append(sub_obj.is_updated())

        # Map subscription object → its is_updated() result for the business logic.
        _update_by_sub = dict(zip(_sub_objs, _updates))

        for bit_idx, (config, sub_obj, updated) in enumerate(
            zip(self._subscription_configs, _sub_objs, _updates)
        ):
            topic = config.key
            if updated:
                self._stale_counts[topic] = 0
                self._update_mask |= 1 << bit_idx
            else:
                count = self._stale_counts.get(topic, 0) + 1
                self._stale_counts[topic] = count
                if count >= STALE_SUBSCRIPTION_THRESHOLD:
                    self._stale_subscription_count += 1
                    if count == STALE_SUBSCRIPTION_THRESHOLD:
                        _LOG.warning(
                            "Subscription '%s' has gone %d consecutive timesteps without data",
                            topic,
                            count,
                        )

        try:
            if self._sub_voltage is not None and _update_by_sub.get(self._sub_voltage, False):
                try:
                    voltage_pu = float(self._sub_voltage.double)
                    if voltage_pu < VOLTAGE_PU_MIN or voltage_pu > VOLTAGE_PU_MAX:
                        # Unit-mismatch guard: a federate publishing absolute
                        # volts (e.g. 240) instead of per-unit would inject a
                        # physically absurd voltage into the simulation
                        # (voltage-dependent ZIP loads scale with v², so 240 pu
                        # multiplies load power by ~57,600x and destroys the
                        # thermal solution). Flag and warn, but do NOT apply —
                        # the dwelling keeps its last valid voltage.
                        self._last_voltage_out_of_range = True
                        _LOG.warning(
                            "Grid voltage %.3f pu outside expected range [%.1f, %.1f]; "
                            "check that the external federate publishes per-unit (not volts). "
                            "Value not applied; keeping last valid voltage.",
                            voltage_pu,
                            VOLTAGE_PU_MIN,
                            VOLTAGE_PU_MAX,
                        )
                    else:
                        self._dwelling.set_grid_voltage(voltage_pu)
                except Exception as exc:
                    _LOG.warning("Failed to apply grid voltage: %s", exc)

            if self._sub_price is not None and _update_by_sub.get(self._sub_price, False):
                try:
                    price = float(self._sub_price.double)
                    if price < 0.0:
                        self._last_price_negative = True
                        _LOG.warning(
                            "Price signal %.3f is negative; expected >= 0 currency/kWh",
                            price,
                        )
                    self._dwelling.set_price_signal({"electricity_price": price})
                except Exception as exc:
                    _LOG.warning("Failed to apply price signal: %s", exc)

            if self._sub_control is None:
                return
            if not _update_by_sub.get(self._sub_control, False):
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
                clamped = _validate_control_signal(signal_body, equipment, _LOG)
                self._control_clamped += clamped
                try:
                    signal = ControlSignal.from_dict(signal_body)
                    self._dwelling.apply_control(equipment, signal)
                except (ValueError, TypeError, KeyError) as exc:
                    _LOG.warning(
                        "Failed to apply control for equipment '%s': %s",
                        equipment,
                        exc,
                    )

        finally:
            if self._control_clamped > 0:
                self._range_violations_total += self._control_clamped
            if self._last_voltage_out_of_range:
                self._range_violations_total += 1
            if self._last_price_negative:
                self._range_violations_total += 1

    def _verify_registration_completeness(self) -> tuple[set[str], set[str]]:
        """Check that every required publication and subscription key is registered.

        Logs an error for each required key that was not registered.  This is a
        no-op when no required keys were configured.

        Returns:
            (missing_publications, missing_subscriptions) — the sets of required
            keys that were not registered.  Callers can inspect these to verify
            completeness without capturing log output.
        """
        registered_pub_keys: set[str] = {c.key for c in self._publication_configs}
        missing_pubs = self._required_publication_keys - registered_pub_keys
        for key in sorted(missing_pubs):
            _LOG.error(
                "Required publication '%s' was not registered; "
                "external federates subscribing to this topic will receive defaults",
                key,
            )

        registered_sub_keys: set[str] = {c.key for c in self._subscription_configs}
        missing_subs = self._required_subscription_keys - registered_sub_keys
        for key in sorted(missing_subs):
            _LOG.error(
                "Required subscription '%s' was not registered; "
                "incoming data on this topic will not reach the dwelling model",
                key,
            )

        return missing_pubs, missing_subs

    def _verify_helics_counts(self) -> None:
        """Query HELICS for actual publication and input counts.

        Compares the HELICS-reported counts against the configured config
        lists and logs a warning on mismatch.  Gracefully degrades when the
        HELICS query API is unavailable.
        """
        try:
            actual_pub_count = helics.helicsFederateGetPublicationCount(self._fed)
        except AttributeError:
            # helicsFederateGetPublicationCount not exposed by this HELICS build
            return
        except Exception:
            return

        expected_pub_count = len(self._publication_configs)
        if actual_pub_count != expected_pub_count:
            _LOG.warning(
                "HELICS publication count mismatch: expected %d, HELICS reports %d",
                expected_pub_count,
                actual_pub_count,
            )

        try:
            actual_input_count = helics.helicsFederateGetInputCount(self._fed)
        except AttributeError:
            return
        except Exception:
            return

        expected_input_count = len(self._subscription_configs)
        if actual_input_count != expected_input_count:
            _LOG.warning(
                "HELICS subscription (input) count mismatch: expected %d, HELICS reports %d",
                expected_input_count,
                actual_input_count,
            )

    def _publish_results(self) -> None:
        telemetry = self._dwelling.telemetry()
        self._pub_power.publish(float(telemetry.total_power_kw))  # type: ignore[union-attr]
        self._pub_reactive.publish(float(telemetry.reactive_power_kvar))  # type: ignore[union-attr]

        _published_count = 2

        # Zone temperatures
        if self._pub_zone_temp:
            zonedata = telemetry.zone()
            temps: list[float] = list(zonedata.get("temperature_c", []))
            for i, pub in enumerate(self._pub_zone_temp):
                value = float(temps[i]) if i < len(temps) else 0.0
                pub.publish(value)
                _published_count += 1

        # Per-equipment power, SOC, and operating mode
        if self._pub_equipment_power or self._pub_equipment_soc or self._pub_equipment_mode:
            equipdata = telemetry.equipment()
            powers: list[float] = list(equipdata.get("power_kw", []))
            socs: list[float] = list(equipdata.get("soc", []))
            modes: list[float] = list(equipdata.get("modes", []))

            for i in range(len(self._pub_equipment_power)):
                if i < len(powers):
                    self._pub_equipment_power[i].publish(float(powers[i]))
                    _published_count += 1
            for i in range(len(self._pub_equipment_soc)):
                if i < len(socs):
                    self._pub_equipment_soc[i].publish(float(socs[i]) * 100.0)
                    _published_count += 1
            for i in range(len(self._pub_equipment_mode)):
                if i < len(modes):
                    self._pub_equipment_mode[i].publish(float(modes[i]))
                    _published_count += 1

        # PV generation: sum of negative electric power from PV equipment
        if self._pub_pv_generation is not None:
            pv_gen_kw = self._compute_pv_generation(telemetry)
            self._pub_pv_generation.publish(pv_gen_kw)
            _published_count += 1

        _LOG.debug(
            "HELICS federate %s published %d signals at step %d",
            self._fed_name,
            _published_count,
            telemetry.timestep_index,
        )

    @staticmethod
    def _compute_pv_generation(telemetry: Any) -> float:
        """Sum the magnitude of negative electric power from PV equipment."""
        equipdata = telemetry.equipment()
        powers: list[float] = list(equipdata.get("power_kw", []))
        names: list[str] = list(equipdata.get("names", []))
        pv_gen_kw = 0.0
        for i, p in enumerate(powers):
            if p < 0.0 and i < len(names):
                name_lower = names[i].lower()
                if "pv" in name_lower or "solar" in name_lower:
                    pv_gen_kw += abs(float(p))
        return pv_gen_kw

    @property
    def helics_voltage_valid(self) -> bool:
        """True when the most recent voltage subscription was in [0.5, 1.5] pu."""
        return not self._last_voltage_out_of_range

    @property
    def helics_price_valid(self) -> bool:
        """True when the most recent price subscription was non-negative."""
        return not self._last_price_negative

    @property
    def helics_control_clamped(self) -> int:
        """Count of control signal fields that exceeded validation ranges this timestep."""
        return self._control_clamped

    @property
    def helics_range_violations_total(self) -> int:
        """Cumulative count of all range violations (voltage, price, control) across the run."""
        return self._range_violations_total

    def get_diagnostic_row(self) -> dict[str, object]:
        """Return a dict of HELICS diagnostic fields for this timestep.

        Keys: ``helics_voltage_valid`` (bool), ``helics_price_valid`` (bool),
        ``helics_control_clamped`` (int), ``helics_range_violations_total`` (int),
        ``helics_stale_subscription_count`` (int),
        ``helics_update_mask`` (int).

        Callers can collect these per-timestep and write to a CSV or telemetry sink.
        """
        return {
            "helics_voltage_valid": self.helics_voltage_valid,
            "helics_price_valid": self.helics_price_valid,
            "helics_control_clamped": self.helics_control_clamped,
            "helics_range_violations_total": self.helics_range_violations_total,
            "helics_stale_subscription_count": self._stale_subscription_count,
            "helics_update_mask": self._update_mask,
        }

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
            _LOG.warning(
                "HELICS period %.1fs derived from dwelling timesteps; "
                "all federates must use consistent time periods — "
                "HELICS does not enforce alignment and grants at the minimum period across all federates. "
                "Mismatched periods cause silent data misalignment.",
                period_s,
            )
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
