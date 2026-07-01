"""Unit tests for HELICSFleet orchestration behavior."""

from __future__ import annotations

import builtins
import importlib
import json
import sys
from types import ModuleType, SimpleNamespace
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
    def __init__(
        self,
        fed_name: str,
        fedinfo: Any,
        log: list[tuple[Any, ...]],
        *,
        raise_on_enter: bool = False,
    ) -> None:
        self.fed_name = fed_name
        self.fedinfo = fedinfo
        self.flags: list[tuple[int, bool]] = []
        self.publications: dict[str, _FakePublication] = {}
        self.subscriptions: dict[str, _FakeSubscription] = {}
        self.requested_times: list[float] = []
        self.disconnected = False
        self._raise_on_enter = raise_on_enter
        self._log = log

    def set_flag_option(self, flag: int, enabled: bool) -> None:
        self.flags.append((flag, enabled))
        self._log.append(("set_flag", flag, enabled))

    def register_publication(self, key: str, data_type: str) -> _FakePublication:
        pub = _FakePublication(key, data_type, self._log)
        self.publications[key] = pub
        self._log.append(("register_publication", key, data_type))
        return pub

    def register_subscription(self, key: str, data_type: str) -> _FakeSubscription:
        sub = _FakeSubscription(key, data_type)
        self.subscriptions[key] = sub
        self._log.append(("register_subscription", key, data_type))
        return sub

    def enter_executing_mode(self) -> None:
        if self._raise_on_enter:
            raise RuntimeError("synthetic enter failure")
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
    HELICS_PROPERTY_TIME_OFFSET = 142

    class HelicsFederateInfo:
        def __init__(self) -> None:
            self.core_type = ""
            self.core_init = ""
            self.core_init_string = ""
            self.core_name = ""
            self.property: dict[int, float] = {}

    def __init__(self) -> None:
        self.log: list[tuple[Any, ...]] = []
        self.last_fedinfo: _FakeHelicsModule.HelicsFederateInfo | None = None
        self.last_fed: _FakeFederate | None = None
        self.raise_on_enter = False

    def helicsCreateValueFederate(self, fed_name: str, fedinfo: Any) -> _FakeFederate:
        self.last_fedinfo = fedinfo
        fed = _FakeFederate(
            fed_name,
            fedinfo,
            self.log,
            raise_on_enter=self.raise_on_enter,
        )
        self.last_fed = fed
        self.log.append(("create_value_federate", fed_name))
        return fed

    def helicsFederateInfoSetCoreName(
        self,
        fedinfo: _FakeHelicsModule.HelicsFederateInfo,
        core_name: str,
    ) -> None:
        fedinfo.core_name = core_name
        self.log.append(("set_core_name", core_name))

    def helicsFederateInfoSetTimeProperty(
        self,
        fedinfo: _FakeHelicsModule.HelicsFederateInfo,
        property_key: int,
        value: float,
    ) -> None:
        fedinfo.property[property_key] = value
        self.log.append(("set_time_property", property_key, value))


class _FakeFleet:
    def __init__(
        self,
        log: list[tuple[Any, ...]],
        *,
        raise_on_step: int | None = None,
    ) -> None:
        self._log = log
        self._raise_on_step = raise_on_step
        self._step_count = 0
        self._time_res = 60.0
        self._power_steps = [
            [(1.0, 0.1), (2.0, 0.2), (3.0, 0.3)],
            [(1.5, 0.15), (2.5, 0.25), (3.5, 0.35)],
        ]

        self.grid_voltage_all_values: list[float] = []
        self.grid_voltage_values: list[tuple[int, float]] = []
        self.applied_controls: list[tuple[int, str, Any]] = []

    def __len__(self) -> int:
        return 3

    def time_res_s(self) -> float:
        return self._time_res

    def total_steps(self) -> int:
        return len(self._power_steps)

    def current_step(self) -> int:
        return self._step_count

    def is_finished(self) -> bool:
        return self._step_count >= self.total_steps()

    def step(self) -> list[dict[str, Any]]:
        self._step_count += 1
        self._log.append(("step", self._step_count))
        if self._raise_on_step == self._step_count:
            raise RuntimeError("synthetic fleet failure")
        return []

    def telemetry(self, dwelling_index: int) -> Any:
        idx = max(self._step_count - 1, 0)
        p_kw, q_kvar = self._power_steps[idx][dwelling_index]
        return SimpleNamespace(total_power_kw=p_kw, reactive_power_kvar=q_kvar)

    def set_grid_voltage_all(self, voltage_pu: float) -> None:
        self.grid_voltage_all_values.append(voltage_pu)
        self._log.append(("set_grid_voltage_all", voltage_pu))

    def set_grid_voltage(self, dwelling_index: int, voltage_pu: float) -> None:
        self.grid_voltage_values.append((dwelling_index, voltage_pu))
        self._log.append(("set_grid_voltage", dwelling_index, voltage_pu))

    def apply_control(self, dwelling_index: int, name: str, signal: Any) -> None:
        if name == "Unknown":
            raise ValueError("unknown equipment")
        self.applied_controls.append((dwelling_index, name, signal))
        self._log.append(("apply_control", dwelling_index, name, signal))


def _clear_helics_modules() -> None:
    sys.modules.pop("ochre_next.helics", None)
    sys.modules.pop("ochre_next.helics.dwelling", None)
    sys.modules.pop("ochre_next.helics.fleet", None)


def _import_fleet_module(
    monkeypatch: pytest.MonkeyPatch,
) -> tuple[ModuleType, _FakeHelicsModule]:
    fake_helics = _FakeHelicsModule()
    _clear_helics_modules()
    monkeypatch.setitem(sys.modules, "helics", fake_helics)
    module = importlib.import_module("ochre_next.helics.fleet")
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
        importlib.import_module("ochre_next.helics.fleet")


def test_helics_fleet_config_and_registration(monkeypatch: pytest.MonkeyPatch):
    module, fake_helics = _import_fleet_module(monkeypatch)
    fleet = _FakeFleet(fake_helics.log)

    orchestrator = module.HELICSFleet(
        fleet,
        fed_name="fleet_1",
        broker_address="10.0.0.7:23404",
        core_type="tcp",
    )

    fedinfo = fake_helics.last_fedinfo
    assert fedinfo is not None
    assert fedinfo.core_type == "tcp"
    # Each federate's core needs a unique name and local port: without them,
    # multiple auto-named/ported cores in the same process can silently
    # deadlock at enterExecutingMode instead of raising a bind error.
    assert fedinfo.core_name == "core_fleet_1"
    assert fedinfo.core_init.startswith("--broker_address=tcp://10.0.0.7:23404")
    assert fedinfo.core_init_string.startswith("--broker_address=tcp://10.0.0.7:23404")
    assert "--port=" in fedinfo.core_init
    assert fedinfo.property[fake_helics.HELICS_PROPERTY_TIME_PERIOD] == pytest.approx(0.0)

    pubs = orchestrator.register_publications(prefix="grid/")
    assert [p.key for p in pubs] == [
        "grid/fleet_1/aggregate_power_kw",
        "grid/fleet_1/aggregate_reactive_kvar",
        "grid/fleet_1/dwelling_0/total_power_kw",
        "grid/fleet_1/dwelling_0/reactive_power_kvar",
        "grid/fleet_1/dwelling_1/total_power_kw",
        "grid/fleet_1/dwelling_1/reactive_power_kvar",
        "grid/fleet_1/dwelling_2/total_power_kw",
        "grid/fleet_1/dwelling_2/reactive_power_kvar",
    ]

    per_dwelling_topics = [
        "grid/voltage/dwelling_0",
        "grid/voltage/dwelling_1",
        "grid/voltage/dwelling_2",
    ]
    subs = orchestrator.register_subscriptions(
        voltage_topic="grid/voltage",
        per_dwelling_voltage_topics=per_dwelling_topics,
        control_topic="grid/control",
    )
    assert [(s.key, s.type) for s in subs] == [
        ("grid/voltage", "double"),
        ("grid/voltage/dwelling_0", "double"),
        ("grid/voltage/dwelling_1", "double"),
        ("grid/voltage/dwelling_2", "double"),
        ("grid/control", "string"),
    ]


def test_helics_fleet_run_loop_aggregate_publish_and_routing(monkeypatch: pytest.MonkeyPatch):
    module, fake_helics = _import_fleet_module(monkeypatch)

    class _FakeControlSignal:
        @staticmethod
        def from_dict(d: dict[str, Any]) -> dict[str, Any]:
            return {"parsed": d}

    monkeypatch.setattr(module, "ControlSignal", _FakeControlSignal)

    fleet = _FakeFleet(fake_helics.log)
    orchestrator = module.HELICSFleet(fleet, fed_name="fleet_1")
    orchestrator.register_publications()
    per_dwelling_topics = [
        "grid/voltage/dwelling_0",
        "grid/voltage/dwelling_1",
        "grid/voltage/dwelling_2",
    ]
    orchestrator.register_subscriptions(
        voltage_topic="grid/voltage",
        per_dwelling_voltage_topics=per_dwelling_topics,
        control_topic="grid/control",
    )

    assert orchestrator._sub_voltage_all is not None
    assert len(orchestrator._sub_voltage_dwelling) == 3
    assert orchestrator._sub_control is not None

    orchestrator._sub_voltage_all.push_double(0.98)
    orchestrator._sub_voltage_all.push_double(0.97)
    orchestrator._sub_voltage_dwelling[1].push_double(0.95)
    orchestrator._sub_voltage_dwelling[2].push_double(0.96)
    orchestrator._sub_control.push_string(
        json.dumps(
            {
                "1": {
                    "Battery": {"type": "PowerSetpoint", "active_power_kw": 2.0},
                    "Unknown": {"type": "PowerSetpoint", "active_power_kw": 1.0},
                }
            }
        )
    )

    orchestrator.run()

    fed = fake_helics.last_fed
    assert fed is not None
    assert fed.requested_times == [60.0, 120.0]

    first_request_idx = fake_helics.log.index(("request_time", 60.0))
    first_step_idx = fake_helics.log.index(("step", 1))
    first_publish_idx = fake_helics.log.index(("publish", "aggregate_power_kw", 6.0))
    assert first_request_idx < first_step_idx < first_publish_idx

    assert fleet.grid_voltage_all_values == [0.98, 0.97]
    assert fleet.grid_voltage_values == [(1, 0.95), (2, 0.96)]
    assert fleet.applied_controls == [
        (1, "Battery", {"parsed": {"type": "PowerSetpoint", "active_power_kw": 2.0}}),
    ]

    assert fed.publications["aggregate_power_kw"].published == [6.0, 7.5]
    assert fed.publications["aggregate_reactive_kvar"].published == pytest.approx(
        [0.6, 0.75]
    )
    assert fed.publications["dwelling_0/total_power_kw"].published == [1.0, 1.5]
    assert fed.publications["dwelling_0/reactive_power_kvar"].published == pytest.approx(
        [0.1, 0.15]
    )
    assert fed.publications["dwelling_1/total_power_kw"].published == [2.0, 2.5]
    assert fed.publications["dwelling_1/reactive_power_kvar"].published == pytest.approx(
        [0.2, 0.25]
    )
    assert fed.publications["dwelling_2/total_power_kw"].published == [3.0, 3.5]
    assert fed.publications["dwelling_2/reactive_power_kvar"].published == pytest.approx(
        [0.3, 0.35]
    )

    assert fed.disconnected is True


def test_helics_fleet_control_decode_does_not_crash_loop(
    monkeypatch: pytest.MonkeyPatch,
    caplog: pytest.LogCaptureFixture,
):
    module, fake_helics = _import_fleet_module(monkeypatch)

    fleet = _FakeFleet(fake_helics.log)
    orchestrator = module.HELICSFleet(fleet, fed_name="fleet_1")
    orchestrator.register_publications()
    orchestrator.register_subscriptions(control_topic="grid/control")

    assert orchestrator._sub_control is not None
    orchestrator._sub_control.push_string(json.dumps({"0": {"Battery": "bad-shape"}}))

    with caplog.at_level("WARNING"):
        orchestrator.run()

    assert "Invalid control message shape" in caplog.text
    assert len(fleet.applied_controls) == 0


def test_helics_fleet_finalize_on_step_exception(monkeypatch: pytest.MonkeyPatch):
    module, fake_helics = _import_fleet_module(monkeypatch)

    fleet = _FakeFleet(fake_helics.log, raise_on_step=1)
    orchestrator = module.HELICSFleet(fleet, fed_name="fleet_1")
    orchestrator.register_publications()

    with pytest.raises(RuntimeError, match="synthetic fleet failure"):
        orchestrator.run()

    fed = fake_helics.last_fed
    assert fed is not None
    assert fed.disconnected is True
    assert fake_helics.log.count(("disconnect",)) == 1


def test_helics_fleet_finalize_on_enter_exception(monkeypatch: pytest.MonkeyPatch):
    module, fake_helics = _import_fleet_module(monkeypatch)
    fake_helics.raise_on_enter = True

    fleet = _FakeFleet(fake_helics.log)
    orchestrator = module.HELICSFleet(fleet, fed_name="fleet_1")
    orchestrator.register_publications()

    with pytest.raises(RuntimeError, match="synthetic enter failure"):
        orchestrator.run()

    fed = fake_helics.last_fed
    assert fed is not None
    assert fed.disconnected is True
    assert fake_helics.log.count(("disconnect",)) == 1


def test_helics_fleet_broadcast_control_applies_to_all_dwellings(
    monkeypatch: pytest.MonkeyPatch,
):
    module, fake_helics = _import_fleet_module(monkeypatch)

    class _FakeControlSignal:
        @staticmethod
        def from_dict(d: dict[str, Any]) -> dict[str, Any]:
            return {"parsed": d}

    monkeypatch.setattr(module, "ControlSignal", _FakeControlSignal)

    fleet = _FakeFleet(fake_helics.log)
    orchestrator = module.HELICSFleet(fleet, fed_name="fleet_1")
    orchestrator.register_publications()
    orchestrator.register_subscriptions(control_topic="grid/control")

    assert orchestrator._sub_control is not None
    orchestrator._sub_control.push_string(
        json.dumps({"Battery": {"type": "PowerSetpoint", "active_power_kw": 1.5}})
    )

    orchestrator.run()

    assert fleet.applied_controls == [
        (0, "Battery", {"parsed": {"type": "PowerSetpoint", "active_power_kw": 1.5}}),
        (1, "Battery", {"parsed": {"type": "PowerSetpoint", "active_power_kw": 1.5}}),
        (2, "Battery", {"parsed": {"type": "PowerSetpoint", "active_power_kw": 1.5}}),
    ]


def test_helics_fleet_negative_dwelling_index_payload_rejected(
    monkeypatch: pytest.MonkeyPatch,
    caplog: pytest.LogCaptureFixture,
):
    module, fake_helics = _import_fleet_module(monkeypatch)

    fleet = _FakeFleet(fake_helics.log)
    orchestrator = module.HELICSFleet(fleet, fed_name="fleet_1")
    orchestrator.register_publications()
    orchestrator.register_subscriptions(control_topic="grid/control")

    assert orchestrator._sub_control is not None
    orchestrator._sub_control.push_string(
        json.dumps({"-1": {"Battery": {"type": "PowerSetpoint", "active_power_kw": 1.0}}})
    )

    with caplog.at_level("WARNING"):
        orchestrator.run()

    assert "must be non-negative" in caplog.text
    assert fleet.applied_controls == []


def test_helics_fleet_time_offset_property(monkeypatch: pytest.MonkeyPatch):
    module, fake_helics = _import_fleet_module(monkeypatch)
    fleet = _FakeFleet(fake_helics.log)

    # Default: no offset property written
    module.HELICSFleet(fleet, fed_name="fleet_1")
    fedinfo_default = fake_helics.last_fedinfo
    assert fedinfo_default is not None
    assert fake_helics.HELICS_PROPERTY_TIME_OFFSET not in fedinfo_default.property

    # Non-zero offset: property written
    module.HELICSFleet(fleet, fed_name="fleet_2", time_offset_s=2.0)
    fedinfo_offset = fake_helics.last_fedinfo
    assert fedinfo_offset is not None
    assert fedinfo_offset.property[fake_helics.HELICS_PROPERTY_TIME_OFFSET] == pytest.approx(2.0)


def test_helics_fleet_per_dwelling_voltage_topics_length_validated(
    monkeypatch: pytest.MonkeyPatch,
):
    module, fake_helics = _import_fleet_module(monkeypatch)
    fleet = _FakeFleet(fake_helics.log)
    orchestrator = module.HELICSFleet(fleet, fed_name="fleet_1")

    with pytest.raises(ValueError, match="must match fleet size"):
        orchestrator.register_subscriptions(
            per_dwelling_voltage_topics=["topic_0", "topic_1"],
        )


class _MultiGrantFederate(_FakeFederate):
    """Fake federate that returns a predefined sequence of granted times."""

    def __init__(self, fed_name: str, fedinfo: Any, log: list[tuple[Any, ...]], *, grant_sequence: list[float] | None = None) -> None:
        super().__init__(fed_name, fedinfo, log)
        self._grant_sequence = list(grant_sequence or [])
        self._grant_idx = 0

    def request_time(self, requested: float) -> float:
        self.requested_times.append(requested)
        if self._grant_idx < len(self._grant_sequence):
            granted = self._grant_sequence[self._grant_idx]
            self._grant_idx += 1
        else:
            granted = requested
        self._log.append(("request_time", requested, granted))
        return granted


def test_helics_fleet_multi_rate_grants_only_steps_at_requested_time(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    module, fake_helics = _import_fleet_module(monkeypatch)

    fleet = _FakeFleet(fake_helics.log)
    orchestrator = module.HELICSFleet(fleet, fed_name="fleet_1")
    orchestrator.register_publications()
    orchestrator.register_subscriptions()

    # Exit-time semantics: step 1 requests exit_time=60s. Five intermediate
    # grants (10-50s) are republished without stepping, then 60s is granted
    # and the fleet steps. Step 2 requests exit_time=120s and is granted.
    multi_fed = _MultiGrantFederate(
        fed_name="fleet_1",
        fedinfo=fake_helics.last_fedinfo,
        log=fake_helics.log,
        grant_sequence=[10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 120.0],
    )
    orchestrator._fed = multi_fed

    orchestrator.run()

    assert fleet._step_count == 2
    assert multi_fed.requested_times == [60.0, 60.0, 60.0, 60.0, 60.0, 60.0, 120.0]

    fed = fake_helics.last_fed
    assert fed is not None
    pub = fed.publications["aggregate_power_kw"]
    # 5 intermediate publishes (before step 1) + 1 step-1 publish + 1 step-2 publish
    assert len(pub.published) == 7
    # All intermediate publishes carry state from before step 1
    for i in range(5):
        assert pub.published[i] == 6.0  # intermediate, unchanged
    assert pub.published[6] == 7.5  # step 2: 1.5+2.5+3.5

    assert multi_fed.disconnected is True
