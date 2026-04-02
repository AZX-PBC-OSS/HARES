"""Unit tests for HELICSDwelling orchestration behavior."""

from __future__ import annotations

from collections.abc import Iterator
from datetime import datetime
import importlib
from types import ModuleType, SimpleNamespace
import builtins
import json
import sys
from typing import Any

import pytest


class _FakePublication:
    def __init__(self, key: str, data_type: str, log: list[tuple[Any, ...]]) -> None:
        self.key = key
        self.data_type = data_type
        self.published: list[float] = []
        self._log = log

    def publish(self, value: float) -> None:
        self.published.append(value)
        self._log.append(("publish", self.key, value))


class _FakeSubscription:
    def __init__(self, key: str, data_type: str) -> None:
        self.key = key
        self.data_type = data_type
        self.double = 0.0
        self.string = ""
        self._double_updates: list[float] = []
        self._string_updates: list[str] = []

    def push_double(self, value: float) -> None:
        self._double_updates.append(value)

    def push_string(self, value: str) -> None:
        self._string_updates.append(value)

    def is_updated(self) -> bool:
        if self._double_updates:
            self.double = self._double_updates.pop(0)
            return True
        if self._string_updates:
            self.string = self._string_updates.pop(0)
            return True
        return False


class _FakeFederate:
    def __init__(self, fed_name: str, fedinfo: Any, log: list[tuple[Any, ...]]) -> None:
        self.fed_name = fed_name
        self.fedinfo = fedinfo
        self.flags: list[tuple[int, bool]] = []
        self.publications: dict[str, _FakePublication] = {}
        self.subscriptions: dict[str, _FakeSubscription] = {}
        self.requested_times: list[float] = []
        self.disconnected = False
        self._log = log

    def set_flag_option(self, flag: int, enabled: bool) -> None:
        self.flags.append((flag, enabled))
        self._log.append(("set_flag", flag, enabled))

    def register_publication(self, key: str, data_type: str) -> _FakePublication:
        self._log.append(("register_publication", key, data_type))
        pub = _FakePublication(key, data_type, self._log)
        self.publications[key] = pub
        return pub

    def register_subscription(self, key: str, data_type: str) -> _FakeSubscription:
        self._log.append(("register_subscription", key, data_type))
        sub = _FakeSubscription(key, data_type)
        self.subscriptions[key] = sub
        return sub

    def enter_executing_mode(self) -> None:
        self._log.append(("enter_executing_mode",))

    def request_time(self, requested: float) -> float:
        self.requested_times.append(requested)
        self._log.append(("request_time", requested))
        return requested

    def disconnect(self) -> None:
        self.disconnected = True
        self._log.append(("disconnect",))


class _FakeHelicsModule:
    HELICS_FLAG_UNINTERRUPTIBLE = 31
    HELICS_FLAG_TERMINATE_ON_ERROR = 72
    HELICS_PROPERTY_TIME_PERIOD = 141

    class HelicsFederateInfo:
        def __init__(self) -> None:
            self.core_type = ""
            self.core_init = ""
            self.property: dict[int, float] = {}

    def __init__(self) -> None:
        self.log: list[tuple[Any, ...]] = []
        self.last_fedinfo: _FakeHelicsModule.HelicsFederateInfo | None = None
        self.last_fed: _FakeFederate | None = None

    def helicsCreateValueFederate(self, fed_name: str, fedinfo: Any) -> _FakeFederate:
        self.last_fedinfo = fedinfo
        fed = _FakeFederate(fed_name, fedinfo, self.log)
        self.last_fed = fed
        self.log.append(("create_value_federate", fed_name))
        return fed

    def helicsFederateInfoSetTimeProperty(
        self,
        fedinfo: _FakeHelicsModule.HelicsFederateInfo,
        property_key: int,
        value: float,
    ) -> None:
        fedinfo.property[property_key] = value
        self.log.append(("set_time_property", property_key, value))


class _FakeDwelling:
    def __init__(
        self,
        log: list[tuple[Any, ...]],
        *,
        raise_on_step: int | None = None,
    ) -> None:
        self._log = log
        self._raise_on_step = raise_on_step
        self._step_count = 0
        self._times = [
            datetime(2020, 1, 1, 0, 0, 0),
            datetime(2020, 1, 1, 0, 1, 0),
        ]
        self._telemetry_values = [(3.0, 0.5), (4.0, 0.6)]
        self.grid_voltage_values: list[float] = []
        self.price_signals: list[dict[str, float | None]] = []
        self.applied_controls: list[tuple[str, Any]] = []

    def timesteps(self) -> Iterator[datetime]:
        return iter(self._times)

    def step(self) -> dict[str, Any]:
        self._step_count += 1
        self._log.append(("step", self._step_count))
        if self._raise_on_step == self._step_count:
            raise RuntimeError("synthetic failure")
        return {}

    def telemetry(self) -> Any:
        idx = max(self._step_count - 1, 0)
        p_kw, q_kvar = self._telemetry_values[idx]
        return SimpleNamespace(total_power_kw=p_kw, reactive_power_kvar=q_kvar)

    def set_grid_voltage(self, voltage_pu: float) -> None:
        self.grid_voltage_values.append(voltage_pu)
        self._log.append(("set_grid_voltage", voltage_pu))

    def set_price_signal(self, signal: dict[str, float | None]) -> None:
        self.price_signals.append(signal)
        self._log.append(("set_price_signal", signal))

    def apply_control(self, name: str, signal: Any) -> None:
        if name == "Unknown":
            raise ValueError("unknown equipment")
        self.applied_controls.append((name, signal))
        self._log.append(("apply_control", name, signal))


class _SinglePassDwelling(_FakeDwelling):
    def __init__(self, log: list[tuple[Any, ...]]) -> None:
        super().__init__(log)
        self._single_pass_iter = iter(self._times)

    def timesteps(self) -> Iterator[datetime]:
        return self._single_pass_iter


class _SingleTimestepDwelling(_FakeDwelling):
    def __init__(self, log: list[tuple[Any, ...]]) -> None:
        super().__init__(log)
        self._times = [datetime(2020, 1, 1, 0, 0, 0)]


def _clear_helics_modules() -> None:
    sys.modules.pop("ochre_next.helics", None)
    sys.modules.pop("ochre_next.helics.dwelling", None)


def _import_dwelling_module(
    monkeypatch: pytest.MonkeyPatch,
) -> tuple[ModuleType, _FakeHelicsModule]:
    fake_helics = _FakeHelicsModule()
    _clear_helics_modules()
    monkeypatch.setitem(sys.modules, "helics", fake_helics)
    module = importlib.import_module("ochre_next.helics.dwelling")
    return module, fake_helics


def test_import_guard_without_helics(monkeypatch: pytest.MonkeyPatch):
    _clear_helics_modules()
    monkeypatch.delitem(sys.modules, "helics", raising=False)

    original_import = builtins.__import__

    def _raising_import(name: str, *args: Any, **kwargs: Any):
        if name == "helics":
            raise ImportError("missing helics")
        return original_import(name, *args, **kwargs)

    monkeypatch.setattr(builtins, "__import__", _raising_import)

    with pytest.raises(ImportError, match="HELICS not installed"):
        importlib.import_module("ochre_next.helics.dwelling")


def test_helics_dwelling_config_and_registration(monkeypatch: pytest.MonkeyPatch):
    module, fake_helics = _import_dwelling_module(monkeypatch)
    dwelling = _FakeDwelling(fake_helics.log)

    orchestrator = module.HELICSDwelling(
        dwelling,
        fed_name="house_1",
        broker_address="10.0.0.7:23404",
        core_type="tcp",
    )

    fedinfo = fake_helics.last_fedinfo
    assert fedinfo is not None
    assert fedinfo.core_type == "tcp"
    assert fedinfo.core_init == "--broker_address=tcp://10.0.0.7:23404"
    assert fedinfo.property[fake_helics.HELICS_PROPERTY_TIME_PERIOD] == pytest.approx(60.0)

    fed = fake_helics.last_fed
    assert fed is not None
    assert (fake_helics.HELICS_FLAG_UNINTERRUPTIBLE, True) in fed.flags
    assert (fake_helics.HELICS_FLAG_TERMINATE_ON_ERROR, True) in fed.flags

    pubs = orchestrator.register_publications(prefix="grid/")
    assert [p.key for p in pubs] == [
        "grid/house_1/total_power_kw",
        "grid/house_1/reactive_power_kvar",
    ]

    subs = orchestrator.register_subscriptions(
        voltage_topic="grid/voltage",
        control_topic="grid/control",
        price_topic="grid/price",
    )
    assert [(s.key, s.type) for s in subs] == [
        ("grid/voltage", "double"),
        ("grid/control", "string"),
        ("grid/price", "double"),
    ]


def test_helics_dwelling_single_timestep_defaults_period_with_warning(
    monkeypatch: pytest.MonkeyPatch,
    caplog: pytest.LogCaptureFixture,
):
    module, fake_helics = _import_dwelling_module(monkeypatch)
    dwelling = _SingleTimestepDwelling(fake_helics.log)

    with caplog.at_level("WARNING"):
        _ = module.HELICSDwelling(dwelling, fed_name="house_1")

    assert "single timestamp" in caplog.text


def test_helics_dwelling_run_loop_relative_time_and_routing(monkeypatch: pytest.MonkeyPatch):
    module, fake_helics = _import_dwelling_module(monkeypatch)

    class _FakeControlSignal:
        @staticmethod
        def from_dict(d: dict[str, Any]) -> dict[str, Any]:
            return {"parsed": d}

    monkeypatch.setattr(module, "ControlSignal", _FakeControlSignal)

    dwelling = _FakeDwelling(fake_helics.log)
    orchestrator = module.HELICSDwelling(dwelling, fed_name="house_1")
    orchestrator.register_publications()
    orchestrator.register_subscriptions(
        voltage_topic="grid/voltage",
        control_topic="grid/control",
        price_topic="grid/price",
    )

    assert orchestrator._sub_voltage is not None
    assert orchestrator._sub_control is not None
    assert orchestrator._sub_price is not None

    orchestrator._sub_voltage.push_double(0.97)
    orchestrator._sub_voltage.push_double(0.97)
    orchestrator._sub_price.push_double(0.19)
    orchestrator._sub_price.push_double(0.19)
    orchestrator._sub_control.push_string(
        json.dumps(
            {
                "Battery": {"type": "PowerSetpoint", "active_power_kw": 3.0},
                "EV": {"type": "PowerLimit", "max_power_kw": 4.2},
                "Unknown": {"type": "PowerSetpoint", "active_power_kw": 1.0},
            }
        )
    )

    orchestrator.run()

    fed = fake_helics.last_fed
    assert fed is not None
    assert fed.requested_times == [0.0, 60.0]

    first_request_idx = fake_helics.log.index(("request_time", 0.0))
    first_step_idx = fake_helics.log.index(("step", 1))
    first_publish_idx = fake_helics.log.index(("publish", "house_1/total_power_kw", 3.0))
    assert first_request_idx < first_step_idx < first_publish_idx

    assert dwelling.grid_voltage_values == [0.97, 0.97]
    assert dwelling.price_signals == [
        {"electricity_price": 0.19},
        {"electricity_price": 0.19},
    ]

    assert dwelling.applied_controls == [
        ("Battery", {"parsed": {"type": "PowerSetpoint", "active_power_kw": 3.0}}),
        ("EV", {"parsed": {"type": "PowerLimit", "max_power_kw": 4.2}}),
    ]

    power_pub = fed.publications["house_1/total_power_kw"]
    reactive_pub = fed.publications["house_1/reactive_power_kvar"]
    assert power_pub.published == [3.0, 4.0]
    assert reactive_pub.published == [0.5, 0.6]

    assert fed.disconnected is True


def test_helics_dwelling_control_decode_does_not_crash_loop(
    monkeypatch: pytest.MonkeyPatch,
    caplog: pytest.LogCaptureFixture,
):
    module, fake_helics = _import_dwelling_module(monkeypatch)
    dwelling = _FakeDwelling(fake_helics.log)
    orchestrator = module.HELICSDwelling(dwelling, fed_name="house_1")
    orchestrator.register_publications()
    orchestrator.register_subscriptions(control_topic="grid/control")

    assert orchestrator._sub_control is not None
    orchestrator._sub_control.push_string(json.dumps({"Battery": "bad-shape"}))

    with caplog.at_level("WARNING"):
        orchestrator.run()

    assert "Invalid control message shape" in caplog.text
    assert len(dwelling.applied_controls) == 0


def test_helics_dwelling_control_signal_parse_failure_is_logged(
    monkeypatch: pytest.MonkeyPatch,
    caplog: pytest.LogCaptureFixture,
):
    module, fake_helics = _import_dwelling_module(monkeypatch)

    class _FailingControlSignal:
        @staticmethod
        def from_dict(d: dict[str, Any]) -> dict[str, Any]:
            raise ValueError(f"bad control: {d}")

    monkeypatch.setattr(module, "ControlSignal", _FailingControlSignal)

    dwelling = _FakeDwelling(fake_helics.log)
    orchestrator = module.HELICSDwelling(dwelling, fed_name="house_1")
    orchestrator.register_publications()
    orchestrator.register_subscriptions(control_topic="grid/control")

    assert orchestrator._sub_control is not None
    orchestrator._sub_control.push_string(
        json.dumps({"equipment": "Battery", "signal": {"type": "PowerSetpoint"}})
    )

    with caplog.at_level("WARNING"):
        orchestrator.run()

    assert "Failed to apply control" in caplog.text
    assert len(dwelling.applied_controls) == 0


def test_helics_dwelling_single_pass_timesteps_are_not_skipped(monkeypatch: pytest.MonkeyPatch):
    module, fake_helics = _import_dwelling_module(monkeypatch)

    dwelling = _SinglePassDwelling(fake_helics.log)
    orchestrator = module.HELICSDwelling(dwelling, fed_name="house_1")
    orchestrator.register_publications()
    orchestrator.run()

    fed = fake_helics.last_fed
    assert fed is not None
    assert fed.requested_times == [0.0, 60.0]
    assert dwelling._step_count == 2


def test_helics_dwelling_publish_requires_registered_publications(
    monkeypatch: pytest.MonkeyPatch,
):
    module, fake_helics = _import_dwelling_module(monkeypatch)

    dwelling = _FakeDwelling(fake_helics.log)
    orchestrator = module.HELICSDwelling(dwelling, fed_name="house_1")

    with pytest.raises(RuntimeError, match="register_publications"):
        orchestrator.run()

    fed = fake_helics.last_fed
    assert fed is not None
    assert fed.disconnected is True


def test_helics_dwelling_finalize_on_step_exception(monkeypatch: pytest.MonkeyPatch):
    module, fake_helics = _import_dwelling_module(monkeypatch)

    dwelling = _FakeDwelling(fake_helics.log, raise_on_step=1)
    orchestrator = module.HELICSDwelling(dwelling, fed_name="house_1")
    orchestrator.register_publications()

    with pytest.raises(RuntimeError, match="synthetic failure"):
        orchestrator.run()

    fed = fake_helics.last_fed
    assert fed is not None
    assert fed.disconnected is True
    assert fake_helics.log.count(("disconnect",)) == 1
