"""End-to-end HELICS integration tests with mock aggregator federates."""

from __future__ import annotations

import builtins
from collections.abc import Callable
from dataclasses import dataclass
import importlib
import importlib.util
import json
from pathlib import Path
import sys
import threading
from typing import Any

import pytest

from conftest import HARES_DEFAULTS, HPXML, SCHEDULE, WEATHER

from ochre_next import Battery, ControlSignal, Dwelling, DwellingConfig, SimulationConfig, SteppableFleet
from ochre_next.helics import HELICSDwelling, HELICSFleet, create_broker, get_broker_port

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


def _new_federate_info(port: int, core_type: str = "zmq", time_res_s: float = _TIME_RES_S):
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
    core_init = f"--broker_address=tcp://127.0.0.1:{port}"
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
        helics_dwelling = HELICSDwelling(
            dwelling=dwelling,
            fed_name="house_1",
            broker_address=f"localhost:{broker_port}",
        )
        helics_dwelling.register_publications()
        helics_dwelling.register_subscriptions(voltage_topic="grid/voltage")

        thread, thread_result = _start_thread(helics_dwelling.run, name="dwelling-federate")

        fedinfo = _new_federate_info(broker_port)
        aggregator = helics.helicsCreateValueFederate("aggregator_1", fedinfo)
        sub_power = aggregator.register_subscription("house_1/total_power_kw", "double")
        pub_voltage = aggregator.register_publication("grid/voltage", "double")

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
        helics_fleet = HELICSFleet(
            fleet=fleet,
            fed_name="fleet_1",
            broker_address=f"localhost:{broker_port}",
        )
        helics_fleet.register_publications()
        helics_fleet.register_subscriptions(voltage_topic="grid/voltage")

        thread, thread_result = _start_thread(helics_fleet.run, name="fleet-federate")

        fedinfo = _new_federate_info(broker_port)
        aggregator = helics.helicsCreateValueFederate("aggregator_1", fedinfo)
        sub_aggregate = aggregator.register_subscription("fleet_1/aggregate_power_kw", "double")
        sub_dwelling = [
            aggregator.register_subscription(f"fleet_1/dwelling_{idx}/total_power_kw", "double")
            for idx in range(3)
        ]
        pub_voltage = aggregator.register_publication("grid/voltage", "double")

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
        baseline_orchestrator = HELICSDwelling(
            dwelling=baseline,
            fed_name="house_base",
            broker_address=f"localhost:{broker_port}",
        )
        baseline_orchestrator.register_publications()
        baseline_orchestrator.register_subscriptions(control_topic="grid/control")

        baseline_thread, baseline_result = _start_thread(
            baseline_orchestrator.run,
            name="dwelling-baseline",
        )

        fedinfo = _new_federate_info(broker_port)
        aggregator = helics.helicsCreateValueFederate("aggregator_base", fedinfo)
        pub_control = aggregator.register_publication("grid/control", "string")
        try:
            aggregator.enter_executing_mode()
            for step_idx in range(_TOTAL_STEPS):
                aggregator.request_time(step_idx * _TIME_RES_S)
                if step_idx == 1:
                    pub_control.publish(
                        json.dumps(
                            {
                                "equipment": "Battery",
                                "signal": {"type": "PowerSetpoint", "active_power_kw": 3.0},
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

        expected = [step * _TIME_RES_S for step in range(_TOTAL_STEPS)]
        assert probe.requested_times == expected
        assert all(requested <= (_TOTAL_STEPS * _TIME_RES_S) for requested in probe.requested_times)
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
        helics_dwelling = HELICSDwelling(
            dwelling=dwelling,
            fed_name="house_1",
            broker_address=f"localhost:{broker_port}",
        )
        helics_dwelling.register_publications()
        helics_dwelling.register_subscriptions(control_topic="grid/control")

        thread, thread_result = _start_thread(helics_dwelling.run, name="dwelling-malformed-control")

        fedinfo = _new_federate_info(broker_port)
        aggregator = helics.helicsCreateValueFederate("aggregator_1", fedinfo)
        sub_power = aggregator.register_subscription("house_1/total_power_kw", "double")
        pub_control = aggregator.register_publication("grid/control", "string")

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
