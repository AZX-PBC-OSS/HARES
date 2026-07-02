"""End-to-end HELICS integration tests with mock aggregator federates."""

from __future__ import annotations

import builtins
from collections.abc import Callable
from dataclasses import dataclass
import importlib
import importlib.util
import json
import sys
import threading
from typing import Any

import pytest

from conftest import HARES_DEFAULTS, HPXML, SCHEDULE, WEATHER

from ochre_next import Battery, ControlSignal, Dwelling, DwellingConfig, SimulationConfig, SteppableFleet
from ochre_next.helics import HELICSDwelling, HELICSFleet, create_broker, get_broker_port
from ochre_next.helics.broker import allocate_ephemeral_port

helics_available = importlib.util.find_spec("helics") is not None
pytestmark = pytest.mark.skipif(not helics_available, reason="helics not installed")

if helics_available:
    import helics
else:  # pragma: no cover - exercised when optional dependency is missing
    helics = None  # type: ignore[assignment]


_TIME_RES_S = 60.0
_TOTAL_STEPS = 10
_DURATION_S = int(_TIME_RES_S * _TOTAL_STEPS)


@dataclass
class _ThreadResult:
    completed: bool = False
    exception: BaseException | None = None


class _FederateProbe:
    """Proxy around a HELICS federate that records requested time and disconnect calls."""

    def __init__(self, fed: Any) -> None:
        self._fed = fed
        self.requested_times: list[float] = []
        self.disconnect_called = False

    def enter_executing_mode(self) -> Any:
        return self._fed.enter_executing_mode()

    def request_time(self, requested: float) -> float:
        self.requested_times.append(float(requested))
        return float(self._fed.request_time(requested))

    def disconnect(self) -> None:
        self.disconnect_called = True
        self._fed.disconnect()

    def __getattr__(self, name: str) -> Any:
        return getattr(self._fed, name)


class _RecordingDwelling:
    """Decorator around ``Dwelling`` that records control events and battery power trace."""

    def __init__(self, dwelling: Dwelling, tracked_equipment: str = "Battery") -> None:
        self._dwelling = dwelling
        self._tracked_equipment = tracked_equipment
        self.applied_controls: list[str] = []
        self.battery_power_trace_kw: list[float] = []

    def timesteps(self):  # noqa: ANN201
        return self._dwelling.timesteps()

    def step(self) -> dict[str, Any]:
        result = self._dwelling.step()
        telemetry = self._dwelling.telemetry().equipment()
        names = telemetry.get("names", [])
        power = telemetry.get("power_kw", [])
        if self._tracked_equipment in names:
            index = names.index(self._tracked_equipment)
            self.battery_power_trace_kw.append(float(power[index]))
        return result

    def telemetry(self):  # noqa: ANN201
        return self._dwelling.telemetry()

    def set_grid_voltage(self, voltage_pu: float) -> None:
        self._dwelling.set_grid_voltage(voltage_pu)

    def set_price_signal(self, signal: dict[str, float | None]) -> None:
        self._dwelling.set_price_signal(signal)

    def apply_control(self, name: str, signal: ControlSignal) -> None:
        self.applied_controls.append(name)
        self._dwelling.apply_control(name, signal)


class _FaultyDwelling:
    """Decorator around ``Dwelling`` that fails at a configured simulation step."""

    def __init__(self, dwelling: Dwelling, fail_step: int) -> None:
        self._dwelling = dwelling
        self._fail_step = fail_step
        self._step_count = 0

    def timesteps(self):  # noqa: ANN201
        return self._dwelling.timesteps()

    def step(self) -> dict[str, Any]:
        self._step_count += 1
        if self._step_count == self._fail_step:
            raise RuntimeError("synthetic dwelling step failure")
        return self._dwelling.step()

    def telemetry(self):  # noqa: ANN201
        return self._dwelling.telemetry()

    def set_grid_voltage(self, voltage_pu: float) -> None:
        self._dwelling.set_grid_voltage(voltage_pu)

    def set_price_signal(self, signal: dict[str, float | None]) -> None:
        self._dwelling.set_price_signal(signal)

    def apply_control(self, name: str, signal: ControlSignal) -> None:
        self._dwelling.apply_control(name, signal)


def _new_dwelling(*, bldg_id: int = 42) -> Dwelling:
    dwelling = Dwelling.from_hpxml(
        HPXML,
        SCHEDULE,
        WEATHER,
        start_time="2019-01-01T00:00:00",
        duration_s=_DURATION_S,
        time_res_s=int(_TIME_RES_S),
        defaults_path=str(HARES_DEFAULTS),
        bldg_id=bldg_id,
        master_seed=0,
        output_verbosity=0,
    )
    dwelling.initialize()
    return dwelling


def _new_fleet(n_dwellings: int = 3) -> SteppableFleet:
    sim_config = SimulationConfig(duration_s=_DURATION_S, time_res_s=int(_TIME_RES_S))
    configs = [
        DwellingConfig(
            hpxml=HPXML,
            schedule=SCHEDULE,
            weather=WEATHER,
            config=sim_config,
            bldg_id=i + 1,
        )
        for i in range(n_dwellings)
    ]
    return SteppableFleet.from_configs(configs, n_threads=0)


def _disconnect_broker(broker: Any) -> None:
    try:
        if hasattr(broker, "disconnect"):
            broker.disconnect()
            return
        if helics is not None and hasattr(helics, "helicsBrokerDisconnect"):
            helics.helicsBrokerDisconnect(broker)
    finally:
        if helics is not None and hasattr(helics, "helicsCloseLibrary"):
            helics.helicsCloseLibrary()


def _start_thread(target: Callable[[], None], name: str) -> tuple[threading.Thread, _ThreadResult]:
    # HELICS federate handles have thread affinity: a handle created via
    # helicsCreateValueFederate must be driven (enterExecutingMode, requestTime)
    # on the same OS thread. Creating in one thread and driving in another
    # causes a ZMQ-level deadlock at the exec-mode barrier. Always construct
    # and run HELICSDwelling/HELICSFleet within the same thread target.
    result = _ThreadResult()

    def _runner() -> None:
        try:
            target()
            result.completed = True
        except BaseException as exc:  # pragma: no cover - asserted in tests
            result.exception = exc

    thread = threading.Thread(target=_runner, name=name, daemon=True)
    thread.start()
    return thread, result


def _new_federate_info(
    port: int, core_type: str = "zmq", time_res_s: float = _TIME_RES_S, core_name: str = "aggregator",
):
    if hasattr(helics, "HelicsFederateInfo"):
        try:
            fedinfo = helics.HelicsFederateInfo()
        except TypeError:
            fedinfo = helics.helicsCreateFederateInfo()
    else:
        fedinfo = helics.helicsCreateFederateInfo()

    if hasattr(helics, "helicsFederateInfoSetCoreTypeFromString"):
        helics.helicsFederateInfoSetCoreTypeFromString(fedinfo, core_type)
    else:
        fedinfo.core_type = core_type

    if hasattr(helics, "helicsFederateInfoSetCoreName"):
        helics.helicsFederateInfoSetCoreName(fedinfo, f"core_{core_name}")

    # Each federate's core needs its own local ZMQ listen port -- without an
    # explicit --port, multiple auto-named cores in the same process can
    # collide on the auto-assigned local port and silently deadlock at
    # enterExecutingMode instead of raising a bind error (reproduced on
    # macOS/arm64 with HELICS 3.6.1).
    local_port = allocate_ephemeral_port()
    core_init = f"--broker_address=tcp://127.0.0.1:{port} --port={local_port}"
    if hasattr(helics, "helicsFederateInfoSetCoreInitString"):
        helics.helicsFederateInfoSetCoreInitString(fedinfo, core_init)
    else:
        if hasattr(fedinfo, "core_init"):
            fedinfo.core_init = core_init
        if hasattr(fedinfo, "core_init_string"):
            fedinfo.core_init_string = core_init

    if hasattr(fedinfo, "property"):
        fedinfo.property[helics.HELICS_PROPERTY_TIME_PERIOD] = time_res_s
    else:
        helics.helicsFederateInfoSetTimeProperty(
            fedinfo,
            helics.HELICS_PROPERTY_TIME_PERIOD,
            time_res_s,
        )

    return fedinfo


def _run_single_dwelling_exchange(voltage_pu: float) -> list[float]:
    broker = create_broker(n_federates=2, port=None)
    broker_port = get_broker_port(broker)

    try:
        dwelling = _new_dwelling()
        federate_ready = threading.Event()

        def _run_federate() -> None:
            helics_dwelling = HELICSDwelling(
                dwelling=dwelling,
                fed_name="house_1",
                broker_address=f"localhost:{broker_port}",
            )
            helics_dwelling.register_publications()
            helics_dwelling.register_subscriptions(voltage_topic="grid/voltage")
            federate_ready.set()
            helics_dwelling.run()

        thread, thread_result = _start_thread(_run_federate, name="dwelling-federate")
        federate_ready.wait()

        fedinfo = _new_federate_info(broker_port)
        aggregator = helics.helicsCreateValueFederate("aggregator_1", fedinfo)
        sub_power = aggregator.register_subscription("house_1/total_power_kw", "double")
        pub_voltage = aggregator.register_global_publication("grid/voltage", "double")

        power_trace: list[float] = []
        try:
            aggregator.enter_executing_mode()
            for step_idx in range(_TOTAL_STEPS):
                pub_voltage.publish(float(voltage_pu))
                aggregator.request_time(step_idx * _TIME_RES_S)
                if sub_power.is_updated():
                    power_trace.append(float(sub_power.double))
        finally:
            aggregator.disconnect()

        thread.join(timeout=30.0)
        assert thread.is_alive() is False, "dwelling federate thread did not exit"
        if thread_result.exception is not None:
            raise thread_result.exception
        assert thread_result.completed is True

        return power_trace
    finally:
        _disconnect_broker(broker)


def _avg(values: list[float]) -> float:
    if not values:
        return 0.0
    return sum(values) / len(values)


def _clear_helics_modules() -> None:
    sys.modules.pop("ochre_next.helics", None)
    sys.modules.pop("ochre_next.helics.broker", None)
    sys.modules.pop("ochre_next.helics.dwelling", None)
    sys.modules.pop("ochre_next.helics.fleet", None)
    sys.modules.pop("ochre_next.helics.runner", None)


def test_single_dwelling_cosim_with_mock_aggregator() -> None:
    nominal_power = _run_single_dwelling_exchange(voltage_pu=1.0)
    low_voltage_power = _run_single_dwelling_exchange(voltage_pu=0.95)

    assert nominal_power, "aggregator should receive dwelling power publications"
    assert low_voltage_power, "aggregator should receive dwelling power publications"
    assert any(abs(p) > 1e-6 for p in low_voltage_power), "published dwelling power should be non-zero"
    assert _avg(low_voltage_power) != pytest.approx(
        _avg(nominal_power),
        abs=1e-6,
    ), "voltage override should change dwelling load"


def test_fleet_cosim_with_mock_aggregator() -> None:
    broker = create_broker(n_federates=2, port=None)
    broker_port = get_broker_port(broker)

    try:
        fleet = _new_fleet(n_dwellings=3)
        federate_ready = threading.Event()

        def _run_federate() -> None:
            helics_fleet = HELICSFleet(
                fleet=fleet,
                fed_name="fleet_1",
                broker_address=f"localhost:{broker_port}",
            )
            helics_fleet.register_publications()
            helics_fleet.register_subscriptions(voltage_topic="grid/voltage")
            federate_ready.set()
            helics_fleet.run()

        thread, thread_result = _start_thread(_run_federate, name="fleet-federate")
        federate_ready.wait()

        fedinfo = _new_federate_info(broker_port)
        aggregator = helics.helicsCreateValueFederate("aggregator_1", fedinfo)
        sub_aggregate = aggregator.register_subscription("fleet_1/aggregate_power_kw", "double")
        sub_dwelling = [
            aggregator.register_subscription(f"fleet_1/dwelling_{idx}/total_power_kw", "double")
            for idx in range(3)
        ]
        pub_voltage = aggregator.register_global_publication("grid/voltage", "double")

        aggregate_samples: list[tuple[float, float]] = []
        try:
            aggregator.enter_executing_mode()
            for step_idx in range(_TOTAL_STEPS):
                pub_voltage.publish(0.98)
                aggregator.request_time(step_idx * _TIME_RES_S)

                if not sub_aggregate.is_updated():
                    continue
                aggregate_kw = float(sub_aggregate.double)

                dwelling_values = []
                all_updated = True
                for sub in sub_dwelling:
                    if not sub.is_updated():
                        all_updated = False
                        break
                    dwelling_values.append(float(sub.double))
                if all_updated:
                    aggregate_samples.append((aggregate_kw, sum(dwelling_values)))
        finally:
            aggregator.disconnect()

        thread.join(timeout=30.0)
        assert thread.is_alive() is False, "fleet federate thread did not exit"
        if thread_result.exception is not None:
            raise thread_result.exception
        assert thread_result.completed is True
        assert aggregate_samples, "aggregator should receive aggregate and per-dwelling updates"

        for aggregate_kw, summed_kw in aggregate_samples:
            assert aggregate_kw == pytest.approx(summed_kw, rel=1e-6, abs=1e-6)
    finally:
        _disconnect_broker(broker)


def test_control_signal_via_helics() -> None:
    broker = create_broker(n_federates=2, port=None)
    broker_port = get_broker_port(broker)

    try:
        baseline = _RecordingDwelling(_new_dwelling())
        baseline._dwelling.add_battery(
            Battery("Battery", 13.5, max_charge_kw=5.0, max_discharge_kw=5.0, initial_soc=0.4)
        )
        baseline_ready = threading.Event()

        def _run_federate() -> None:
            baseline_orchestrator = HELICSDwelling(
                dwelling=baseline,
                fed_name="house_base",
                broker_address=f"localhost:{broker_port}",
            )
            baseline_orchestrator.register_publications()
            baseline_orchestrator.register_subscriptions(control_topic="grid/control")
            baseline_ready.set()
            baseline_orchestrator.run()

        baseline_thread, baseline_result = _start_thread(
            _run_federate,
            name="dwelling-baseline",
        )
        baseline_ready.wait()

        fedinfo = _new_federate_info(broker_port)
        aggregator = helics.helicsCreateValueFederate("aggregator_base", fedinfo)
        pub_control = aggregator.register_global_publication("grid/control", "string")
        try:
            aggregator.enter_executing_mode()
            for step_idx in range(_TOTAL_STEPS):
                aggregator.request_time(step_idx * _TIME_RES_S)
                if step_idx == 1:
                    # A discharge setpoint (negative active_power_kw), not a charge
                    # setpoint: the dwelling's default scenario starts at January
                    # midnight outdoor temperature, below the battery's
                    # min_charge_temp_c safety lockout, so a charge request would be
                    # correctly blocked and only the ~5W standby draw would show up
                    # regardless of whether the control signal was ever delivered.
                    pub_control.publish(
                        json.dumps(
                            {
                                "equipment": "Battery",
                                "signal": {"type": "PowerSetpoint", "active_power_kw": -3.0},
                            }
                        )
                    )
        finally:
            aggregator.disconnect()

        baseline_thread.join(timeout=30.0)
        assert baseline_thread.is_alive() is False
        if baseline_result.exception is not None:
            raise baseline_result.exception
        assert baseline_result.completed is True
        assert baseline.applied_controls == ["Battery"]
        assert baseline.battery_power_trace_kw, "battery telemetry trace should be captured"
        controlled_power = abs(baseline.battery_power_trace_kw[-1])
        assert controlled_power > 0.1, "battery should show non-trivial power after control injection"
    finally:
        _disconnect_broker(broker)


def test_helics_time_domain_is_simulation_relative() -> None:
    broker = create_broker(n_federates=1, port=None)
    broker_port = get_broker_port(broker)

    try:
        dwelling = _new_dwelling()
        helics_dwelling = HELICSDwelling(
            dwelling=dwelling,
            fed_name="house_1",
            broker_address=f"localhost:{broker_port}",
        )
        helics_dwelling.register_publications()

        probe = _FederateProbe(helics_dwelling._fed)
        helics_dwelling._fed = probe
        helics_dwelling.run()

        # Each step requests the *exit* time of its interval: a federate is
        # already at time 0 after enter_executing_mode(), so the first
        # request must be for one period ahead, not for time 0 itself.
        expected = [(step + 1) * _TIME_RES_S for step in range(_TOTAL_STEPS)]
        # The final request is HELICS_TIME_MAXTIME to signal completion.
        expected.append(helics.HELICS_TIME_MAXTIME)
        assert probe.requested_times == expected
        assert all(requested <= (_TOTAL_STEPS * _TIME_RES_S) for requested in probe.requested_times[:-1])
    finally:
        _disconnect_broker(broker)


def test_federate_cleanup_on_exception() -> None:
    broker = create_broker(n_federates=1, port=None)
    broker_port = get_broker_port(broker)

    try:
        faulty_dwelling = _FaultyDwelling(_new_dwelling(), fail_step=3)
        helics_dwelling = HELICSDwelling(
            dwelling=faulty_dwelling,
            fed_name="house_1",
            broker_address=f"localhost:{broker_port}",
        )
        helics_dwelling.register_publications()

        probe = _FederateProbe(helics_dwelling._fed)
        helics_dwelling._fed = probe

        with pytest.raises(RuntimeError, match="synthetic dwelling step failure"):
            helics_dwelling.run()

        assert probe.disconnect_called is True
    finally:
        _disconnect_broker(broker)


def test_malformed_control_payload_does_not_crash_federate(
    caplog: pytest.LogCaptureFixture,
) -> None:
    broker = create_broker(n_federates=2, port=None)
    broker_port = get_broker_port(broker)

    try:
        dwelling = _new_dwelling()
        federate_ready = threading.Event()

        def _run_federate() -> None:
            helics_dwelling = HELICSDwelling(
                dwelling=dwelling,
                fed_name="house_1",
                broker_address=f"localhost:{broker_port}",
            )
            helics_dwelling.register_publications()
            helics_dwelling.register_subscriptions(control_topic="grid/control")
            federate_ready.set()
            helics_dwelling.run()

        thread, thread_result = _start_thread(_run_federate, name="dwelling-malformed-control")
        federate_ready.wait()

        fedinfo = _new_federate_info(broker_port)
        aggregator = helics.helicsCreateValueFederate("aggregator_1", fedinfo)
        sub_power = aggregator.register_subscription("house_1/total_power_kw", "double")
        pub_control = aggregator.register_global_publication("grid/control", "string")

        with caplog.at_level("WARNING"):
            try:
                aggregator.enter_executing_mode()
                payloads = [
                    "not json",
                    json.dumps({"unknown_equipment": {"type": "PowerSetpoint", "active_power_kw": 3.0}}),
                    json.dumps({"equipment": "Battery", "signal": {"type": "InvalidType"}}),
                ]
                for step_idx in range(_TOTAL_STEPS):
                    aggregator.request_time(step_idx * _TIME_RES_S)
                    if step_idx < len(payloads):
                        pub_control.publish(payloads[step_idx])
            finally:
                aggregator.disconnect()

        thread.join(timeout=30.0)
        assert thread.is_alive() is False
        if thread_result.exception is not None:
            raise thread_result.exception
        assert thread_result.completed is True
        assert "Invalid control payload JSON" in caplog.text or "Failed to apply control" in caplog.text
        assert sub_power.is_updated() or "publish" in caplog.text
    finally:
        _disconnect_broker(broker)


def test_fleet_invalid_dwelling_index() -> None:
    fleet = _new_fleet(n_dwellings=3)
    with pytest.raises((IndexError, ValueError, RuntimeError)):
        fleet.set_grid_voltage(999, 0.95)


def test_helics_import_guard(monkeypatch: pytest.MonkeyPatch) -> None:
    _clear_helics_modules()
    monkeypatch.delitem(sys.modules, "helics", raising=False)

    original_import = builtins.__import__

    def _raising_import(name: str, *args: Any, **kwargs: Any):
        if name == "helics":
            raise ImportError("missing helics")
        return original_import(name, *args, **kwargs)

    monkeypatch.setattr(builtins, "__import__", _raising_import)

    with pytest.raises(ImportError, match="HELICS not installed"):
        from ochre_next.helics import HELICSDwelling  # noqa: F401


def test_multi_rate_dwelling_steps_only_at_own_period() -> None:
    """Dwelling at 60s timestep, aggregator at 10s. Verify dwelling only steps at 60s boundaries."""
    broker = create_broker(n_federates=2, port=None)
    broker_port = get_broker_port(broker)

    DWELL_TIME_RES_S = 60
    AGG_TIME_RES_S = 10
    DWELL_STEPS = 3
    DWELL_DURATION_S = DWELL_STEPS * DWELL_TIME_RES_S  # 180
    AGG_STEPS = DWELL_DURATION_S // AGG_TIME_RES_S  # 18

    try:
        dwelling_raw = Dwelling.from_hpxml(
            HPXML, SCHEDULE, WEATHER,
            start_time="2019-01-01T00:00:00",
            duration_s=DWELL_DURATION_S,
            time_res_s=DWELL_TIME_RES_S,
            defaults_path=str(HARES_DEFAULTS),
            bldg_id=99,
            master_seed=0,
            output_verbosity=0,
        )
        dwelling_raw.initialize()
        recording = _RecordingDwelling(dwelling_raw)
        federate_ready = threading.Event()

        def _run_federate() -> None:
            helics_dwelling = HELICSDwelling(
                dwelling=recording,
                fed_name="house_mr",
                broker_address=f"localhost:{broker_port}",
            )
            helics_dwelling.register_publications()
            helics_dwelling.register_subscriptions(voltage_topic="grid/voltage")
            federate_ready.set()
            helics_dwelling.run()

        thread, thread_result = _start_thread(_run_federate, name="dwelling-mr")
        federate_ready.wait()

        fedinfo = _new_federate_info(broker_port, time_res_s=float(AGG_TIME_RES_S))
        aggregator = helics.helicsCreateValueFederate("agg_mr", fedinfo)
        sub_power = aggregator.register_subscription("house_mr/total_power_kw", "double")
        pub_voltage = aggregator.register_global_publication("grid/voltage", "double")

        power_trace: list[float] = []
        try:
            aggregator.enter_executing_mode()
            # request_time(t) asks to be granted time t; since the federate
            # is already at time 0 after enter_executing_mode(), the first
            # request must target the exit time of the first interval
            # (AGG_TIME_RES_S), not entry time 0.
            for step_idx in range(1, AGG_STEPS + 1):
                pub_voltage.publish(1.0)
                aggregator.request_time(step_idx * AGG_TIME_RES_S)
                if sub_power.is_updated():
                    power_trace.append(float(sub_power.double))
        finally:
            aggregator.disconnect()

        thread.join(timeout=30.0)
        assert thread.is_alive() is False, "dwelling federate thread did not exit"
        if thread_result.exception is not None:
            raise thread_result.exception
        assert thread_result.completed is True

        # Dwelling publishes its current state at every HELICS grant (every 10s),
        # but its model state only advances at 60s boundaries.
        assert len(power_trace) == AGG_STEPS, (
            f"Expected {AGG_STEPS} publications, got {len(power_trace)}"
        )
        assert any(abs(p) > 1e-6 for p in power_trace), "published dwelling power should be non-zero"
    finally:
        _disconnect_broker(broker)


def test_single_dwelling_completion_signal_unblocks_aggregator() -> None:
    """After dwelling signals completion, the co-simulation completes cleanly.

    A ``_FederateProbe`` records the dwelling's ``request_time`` calls during
    a 2-federate co-simulation with a real aggregator.  The test asserts that
    ``request_time(HELICS_TIME_MAXTIME)`` is the final call — a regression
    that proves the completion signal is sent in the presence of an active
    peer federate.

    The aggregator disconnects before joining the dwelling thread so that the
    dwelling's blocked ``request_time(HELICS_TIME_MAXTIME)`` is granted after
    the last remaining peer leaves the federation.
    """
    broker = create_broker(n_federates=2, port=None)
    broker_port = get_broker_port(broker)

    try:
        dwelling = _new_dwelling()
        federate_ready = threading.Event()
        probe_times: list[float] = []

        def _run_federate() -> None:
            helics_dwelling = HELICSDwelling(
                dwelling=dwelling,
                fed_name="house_1",
                broker_address=f"localhost:{broker_port}",
            )
            helics_dwelling.register_publications()
            helics_dwelling.register_subscriptions(voltage_topic="grid/voltage")

            probe = _FederateProbe(helics_dwelling._fed)
            helics_dwelling._fed = probe

            federate_ready.set()
            helics_dwelling.run()

            probe_times.extend(probe.requested_times)

        thread, thread_result = _start_thread(_run_federate, name="dwelling-federate")
        federate_ready.wait()

        fedinfo = _new_federate_info(broker_port)
        aggregator = helics.helicsCreateValueFederate("aggregator_1", fedinfo)
        sub_power = aggregator.register_subscription("house_1/total_power_kw", "double")
        pub_voltage = aggregator.register_global_publication("grid/voltage", "double")

        aggregator.enter_executing_mode()
        for step_idx in range(_TOTAL_STEPS):
            pub_voltage.publish(1.0)
            aggregator.request_time(step_idx * _TIME_RES_S)
            if sub_power.is_updated():
                _ = float(sub_power.double)

        # Disconnect aggregator FIRST so the dwelling's blocked
        # request_time(HELICS_TIME_MAXTIME) is granted after the last
        # remaining peer leaves the federation.
        aggregator.disconnect()

        thread.join(timeout=30.0)
        assert thread.is_alive() is False, "dwelling federate thread did not exit"
        if thread_result.exception is not None:
            raise thread_result.exception
        assert thread_result.completed is True

        assert len(probe_times) > 0, "dwelling made no request_time calls"
        assert probe_times[-1] == helics.HELICS_TIME_MAXTIME, (
            f"Expected final request_time to be HELICS_TIME_MAXTIME; got {probe_times[-1]}"
        )
    finally:
        _disconnect_broker(broker)


def test_fleet_completion_signal_unblocks_aggregator() -> None:
    """After fleet signals completion, the co-simulation completes cleanly.

    Mirrors ``test_single_dwelling_completion_signal_unblocks_aggregator``
    for the fleet federate — uses a ``_FederateProbe`` to verify
    ``request_time(HELICS_TIME_MAXTIME)`` is the final call in a 2-federate
    co-simulation.
    """
    broker = create_broker(n_federates=2, port=None)
    broker_port = get_broker_port(broker)

    try:
        fleet = _new_fleet(n_dwellings=3)
        federate_ready = threading.Event()
        probe_times: list[float] = []

        def _run_federate() -> None:
            helics_fleet = HELICSFleet(
                fleet=fleet,
                fed_name="fleet_1",
                broker_address=f"localhost:{broker_port}",
            )
            helics_fleet.register_publications()
            helics_fleet.register_subscriptions(voltage_topic="grid/voltage")

            probe = _FederateProbe(helics_fleet._fed)
            helics_fleet._fed = probe

            federate_ready.set()
            helics_fleet.run()

            probe_times.extend(probe.requested_times)

        thread, thread_result = _start_thread(_run_federate, name="fleet-federate")
        federate_ready.wait()

        fedinfo = _new_federate_info(broker_port)
        aggregator = helics.helicsCreateValueFederate("aggregator_1", fedinfo)
        sub_aggregate = aggregator.register_subscription("fleet_1/aggregate_power_kw", "double")
        _ = [
            aggregator.register_subscription(f"fleet_1/dwelling_{idx}/total_power_kw", "double")
            for idx in range(3)
        ]
        pub_voltage = aggregator.register_global_publication("grid/voltage", "double")

        aggregator.enter_executing_mode()
        for step_idx in range(_TOTAL_STEPS):
            pub_voltage.publish(0.98)
            aggregator.request_time(step_idx * _TIME_RES_S)
            if sub_aggregate.is_updated():
                _ = float(sub_aggregate.double)

        # Disconnect aggregator FIRST so the fleet's blocked
        # request_time(HELICS_TIME_MAXTIME) is granted after the last
        # remaining peer leaves the federation.
        aggregator.disconnect()

        thread.join(timeout=30.0)
        assert thread.is_alive() is False, "fleet federate thread did not exit"
        if thread_result.exception is not None:
            raise thread_result.exception
        assert thread_result.completed is True

        assert len(probe_times) > 0, "fleet made no request_time calls"
        assert probe_times[-1] == helics.HELICS_TIME_MAXTIME, (
            f"Expected final request_time to be HELICS_TIME_MAXTIME; got {probe_times[-1]}"
        )
    finally:
        _disconnect_broker(broker)


def test_publication_info_via_helics_api_fallback() -> None:
    """_set_publication_info attaches metadata via helicsPublicationSetInfo fallback.

    On HELICS 3.6.1 ``HelicsPublication`` has no ``set_info`` method, so
    ``_set_publication_info`` falls through to ``helics.helicsPublicationSetInfo``.
    This integration test exercises that production code path by retrieving the
    metadata with ``helicsPublicationGetInfo``.
    """
    broker = create_broker(n_federates=1, port=None)
    broker_port = get_broker_port(broker)

    try:
        dwelling = _new_dwelling()
        helics_dwelling = HELICSDwelling(
            dwelling=dwelling,
            fed_name="house_meta",
            broker_address=f"localhost:{broker_port}",
        )
        helics_dwelling.register_publications()

        assert helics_dwelling._pub_power is not None
        assert helics_dwelling._pub_reactive is not None

        power_info = helics.helicsPublicationGetInfo(helics_dwelling._pub_power)
        assert power_info == "units=kW"

        reactive_info = helics.helicsPublicationGetInfo(helics_dwelling._pub_reactive)
        assert reactive_info == "units=kvar"
    finally:
        _disconnect_broker(broker)


def test_voltage_in_volts_triggers_out_of_range_warning() -> None:
    """Publish grid voltage in absolute volts (240 V) instead of per-unit.

    An external federate that publishes voltage in volts (e.g. 240) when
    HARES expects per-unit (~0.95) produces a 240× error. The HELICS boundary
    must detect the unit mismatch so the operator or an external caller can
    act on it.
    """
    broker = create_broker(n_federates=2, port=None)
    broker_port = get_broker_port(broker)

    try:
        dwelling_raw = Dwelling.from_hpxml(
            HPXML, SCHEDULE, WEATHER,
            start_time="2019-01-01T00:00:00",
            duration_s=300,  # 5 steps at 60s
            time_res_s=60,
            defaults_path=str(HARES_DEFAULTS),
            bldg_id=99,
            master_seed=0,
            output_verbosity=0,
        )
        dwelling_raw.initialize()
        federate_ready = threading.Event()
        orchestrator_ref: list[HELICSDwelling] = []

        def _run_federate() -> None:
            helics_dwelling = HELICSDwelling(
                dwelling=dwelling_raw,
                fed_name="house_240v",
                broker_address=f"localhost:{broker_port}",
            )
            orchestrator_ref.append(helics_dwelling)
            helics_dwelling.register_publications()
            helics_dwelling.register_subscriptions(voltage_topic="grid/voltage")
            federate_ready.set()
            helics_dwelling.run()

        thread, thread_result = _start_thread(_run_federate, name="dwelling-240v")
        federate_ready.wait()

        fedinfo = _new_federate_info(broker_port, time_res_s=60.0)
        aggregator = helics.helicsCreateValueFederate("aggregator_240v", fedinfo)
        pub_voltage = aggregator.register_global_publication("grid/voltage", "double")

        try:
            aggregator.enter_executing_mode()
            for step_idx in range(5):
                # Publish 240 V (absolute volts, not per-unit)
                pub_voltage.publish(240.0)
                aggregator.request_time(step_idx * 60.0)
        finally:
            aggregator.disconnect()

        thread.join(timeout=30.0)
        assert thread.is_alive() is False, "dwelling federate thread did not exit"
        if thread_result.exception is not None:
            raise thread_result.exception
        assert thread_result.completed is True

        assert len(orchestrator_ref) == 1
        assert orchestrator_ref[0]._last_voltage_out_of_range is True, (
            "Expected out-of-range voltage detection for 240 V; "
            "the HELICS boundary should detect unit mismatch between volts and per-unit"
        )
    finally:
        _disconnect_broker(broker)


def test_zone_temperature_publication_subscribed_by_aggregator() -> None:
    """An external federate subscribes to zone_0/temp_air_c and receives non-zero values.

    Regression test: zone temperature must be published via HELICS so external
    controllers can implement closed-loop HVAC control.
    """
    broker = create_broker(n_federates=2, port=None)
    broker_port = get_broker_port(broker)

    try:
        dwelling = _new_dwelling()
        federate_ready = threading.Event()

        def _run_federate() -> None:
            helics_dwelling = HELICSDwelling(
                dwelling=dwelling,
                fed_name="house_zt",
                broker_address=f"localhost:{broker_port}",
            )
            helics_dwelling.register_publications()
            helics_dwelling.register_subscriptions(voltage_topic="grid/voltage")
            federate_ready.set()
            helics_dwelling.run()

        thread, thread_result = _start_thread(_run_federate, name="dwelling-zt")
        federate_ready.wait()

        fedinfo = _new_federate_info(broker_port)
        aggregator = helics.helicsCreateValueFederate("aggregator_zt", fedinfo)
        sub_zone_temp = aggregator.register_subscription("house_zt/zone_0/temp_air_c", "double")
        pub_voltage = aggregator.register_global_publication("grid/voltage", "double")

        zone_temp_trace: list[float] = []
        try:
            aggregator.enter_executing_mode()
            for step_idx in range(_TOTAL_STEPS):
                pub_voltage.publish(1.0)
                aggregator.request_time(step_idx * _TIME_RES_S)
                if sub_zone_temp.is_updated():
                    zone_temp_trace.append(float(sub_zone_temp.double))
        finally:
            aggregator.disconnect()

        thread.join(timeout=30.0)
        assert thread.is_alive() is False, "dwelling federate thread did not exit"
        if thread_result.exception is not None:
            raise thread_result.exception
        assert thread_result.completed is True

        assert zone_temp_trace, "aggregator should receive zone temperature publications"
        assert all(abs(t) > 0.1 for t in zone_temp_trace), (
            f"zone temperature should be non-zero (winter outdoor temp); got {zone_temp_trace}"
        )
    finally:
        _disconnect_broker(broker)


def test_mismatched_subscription_topic_logs_stale_warning() -> None:
    """Dwelling subscribes to a misspelled topic; stale warning fires.

    The dwelling registers a subscription to ``grid/voltage_typo`` while the
    aggregator publishes to ``grid/voltage``.  After 10 consecutive timesteps
    without an update the dwelling logs a stale-subscription warning and the
    diagnostic row reflects the stale count.
    """
    broker = create_broker(n_federates=2, port=None)
    broker_port = get_broker_port(broker)

    try:
        dwelling = _new_dwelling(bldg_id=88)
        federate_ready = threading.Event()
        orchestrator_ref: list[HELICSDwelling] = []

        def _run_federate() -> None:
            helics_dwelling = HELICSDwelling(
                dwelling=dwelling,
                fed_name="house_stale",
                broker_address=f"localhost:{broker_port}",
            )
            orchestrator_ref.append(helics_dwelling)
            helics_dwelling.register_publications()
            # Subscribe to a topic the aggregator does NOT publish to
            helics_dwelling.register_subscriptions(voltage_topic="grid/voltage_typo")
            federate_ready.set()
            helics_dwelling.run()

        thread, thread_result = _start_thread(_run_federate, name="dwelling-stale")
        federate_ready.wait()

        fedinfo = _new_federate_info(broker_port, time_res_s=_TIME_RES_S)
        aggregator = helics.helicsCreateValueFederate("aggregator_stale", fedinfo)
        sub_power = aggregator.register_subscription("house_stale/total_power_kw", "double")
        # Aggregator publishes to the correct topic (not to voltage_typo)
        pub_voltage = aggregator.register_global_publication("grid/voltage", "double")

        try:
            aggregator.enter_executing_mode()
            for step_idx in range(_TOTAL_STEPS):
                pub_voltage.publish(1.0)
                aggregator.request_time(step_idx * _TIME_RES_S)
                if sub_power.is_updated():
                    _ = float(sub_power.double)
        finally:
            aggregator.disconnect()

        thread.join(timeout=30.0)
        assert thread.is_alive() is False, "dwelling federate thread did not exit"
        if thread_result.exception is not None:
            raise thread_result.exception
        assert thread_result.completed is True

        assert len(orchestrator_ref) == 1
        orch = orchestrator_ref[0]
        # After _TOTAL_STEPS (=10) steps with no data, the stale count should
        # reflect the unresponsive subscription.
        row = orch.get_diagnostic_row()
        assert row["helics_stale_subscription_count"] >= 1, (
            "Expected at least one stale subscription after %d steps without data; "
            "got stale_count=%d, update_mask=%d"
            % (_TOTAL_STEPS, row["helics_stale_subscription_count"], row["helics_update_mask"])
        )
        assert row["helics_update_mask"] == 0, (
            "No subscription should have received data; got update_mask=%d"
            % row["helics_update_mask"]
        )
    finally:
        _disconnect_broker(broker)
