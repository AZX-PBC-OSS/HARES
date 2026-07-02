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
        self._info: str | None = None

    def set_info(self, info: str) -> None:
        self._info = info
        self._log.append(("set_info", self.key, info))

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
    def __init__(self, fed_name: str, fedinfo: Any, log: list[tuple[Any, ...]], *, raise_on_max_time: bool = False) -> None:
        self.fed_name = fed_name
        self.fedinfo = fedinfo
        self.flags: list[tuple[int, bool]] = []
        self.publications: dict[str, _FakePublication] = {}
        self.subscriptions: dict[str, _FakeSubscription] = {}
        self.requested_times: list[float] = []
        self.disconnected = False
        self._raise_on_max_time = raise_on_max_time
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
        if self._raise_on_max_time and requested == _FakeHelicsModule.HELICS_TIME_MAXTIME:
            raise RuntimeError("synthetic max-time failure")
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
    HELICS_TIME_MAXTIME = 9223372036.854774

    class HelicsFederateInfo:
        def __init__(self) -> None:
            self.core_type = ""
            self.core_init = ""
            self.core_name = ""
            self.property: dict[int, float] = {}

    def __init__(self) -> None:
        self.log: list[tuple[Any, ...]] = []
        self.last_fedinfo: _FakeHelicsModule.HelicsFederateInfo | None = None
        self.last_fed: _FakeFederate | None = None
        self.raise_on_max_time = False

    def helicsCreateValueFederate(self, fed_name: str, fedinfo: Any) -> _FakeFederate:
        self.last_fedinfo = fedinfo
        fed = _FakeFederate(fed_name, fedinfo, self.log, raise_on_max_time=self.raise_on_max_time)
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


class _FakeDwelling:
    def __init__(
        self,
        log: list[tuple[Any, ...]],
        *,
        raise_on_step: int | None = None,
        n_zones: int = 1,
        n_equipment: int = 1,
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
        self._n_zones = n_zones
        self._n_equipment = n_equipment
        self._zone_temps = [21.5] * n_zones
        self._equip_powers = [1.0] * n_equipment
        self._equip_socs = [0.5] * n_equipment
        self._equip_modes = [1.0] * n_equipment
        self._equip_names = [f"Equip_{i}" for i in range(n_equipment)]

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

        zt = self._zone_temps
        en = self._equip_names
        ep = self._equip_powers
        es = self._equip_socs
        em = self._equip_modes

        class _FakeZoneDict(dict):
            pass

        class _FakeEquipDict(dict):
            pass

        zone_dict = _FakeZoneDict({
            "names": [f"Zone_{i}" for i in range(self._n_zones)],
            "temperature_c": list(zt),
        })

        equip_dict = _FakeEquipDict({
            "names": list(en),
            "power_kw": list(ep),
            "soc": list(es),
            "modes": list(em),
        })

        return SimpleNamespace(
            total_power_kw=p_kw,
            reactive_power_kvar=q_kvar,
            zone=lambda: zone_dict,
            equipment=lambda: equip_dict,
            timestep_index=self._step_count,
        )

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
    # Each federate's core needs a unique name and local port: without them,
    # multiple auto-named/ported cores in the same process can silently
    # deadlock at enterExecutingMode instead of raising a bind error.
    assert fedinfo.core_name == "core_house_1"
    assert fedinfo.core_init.startswith("--broker_address=tcp://10.0.0.7:23404")
    assert "--port=" in fedinfo.core_init
    assert fedinfo.property[fake_helics.HELICS_PROPERTY_TIME_PERIOD] == pytest.approx(0.0)

    fed = fake_helics.last_fed
    assert fed is not None
    assert (fake_helics.HELICS_FLAG_UNINTERRUPTIBLE, True) not in fed.flags
    assert (fake_helics.HELICS_FLAG_TERMINATE_ON_ERROR, True) in fed.flags

    pubs = orchestrator.register_publications(prefix="grid/")
    assert [p.key for p in pubs] == [
        "grid/house_1/total_power_kw",
        "grid/house_1/reactive_power_kvar",
        "grid/house_1/zone_0/temp_air_c",
        "grid/house_1/equipment_0/power_kw",
        "grid/house_1/equipment_0/soc_pct",
        "grid/house_1/equipment_0/operating_mode",
        "grid/house_1/pv_generation_kw",
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
    assert fed.requested_times == [60.0, 120.0, fake_helics.HELICS_TIME_MAXTIME]

    # HELICS prepends the federate name to local publication names, so the
    # fake federate (which does not simulate that prefixing) records bare
    # names -- the "house_1/" prefix only exists in the HELICSPublicationConfig
    # metadata returned to callers, not in the name passed to HELICS itself.
    first_request_idx = fake_helics.log.index(("request_time", 60.0))
    first_step_idx = fake_helics.log.index(("step", 1))
    first_publish_idx = fake_helics.log.index(("publish", "total_power_kw", 3.0))
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

    power_pub = fed.publications["total_power_kw"]
    reactive_pub = fed.publications["reactive_power_kvar"]
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
    assert fed.requested_times == [60.0, 120.0, fake_helics.HELICS_TIME_MAXTIME]
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
    assert fake_helics.HELICS_TIME_MAXTIME not in fed.requested_times


def test_helics_dwelling_time_offset_property(monkeypatch: pytest.MonkeyPatch):
    module, fake_helics = _import_dwelling_module(monkeypatch)

    dwelling = _FakeDwelling(fake_helics.log)

    # Default: no offset property written
    module.HELICSDwelling(dwelling, fed_name="house_1")
    fedinfo_default = fake_helics.last_fedinfo
    assert fedinfo_default is not None
    assert fake_helics.HELICS_PROPERTY_TIME_OFFSET not in fedinfo_default.property

    # Non-zero offset: property written
    module.HELICSDwelling(dwelling, fed_name="house_2", time_offset_s=1.0)
    fedinfo_offset = fake_helics.last_fedinfo
    assert fedinfo_offset is not None
    assert fedinfo_offset.property[fake_helics.HELICS_PROPERTY_TIME_OFFSET] == pytest.approx(1.0)


def test_helics_dwelling_run_does_not_propagate_completion_signal_exception(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    module, fake_helics = _import_dwelling_module(monkeypatch)
    fake_helics.raise_on_max_time = True

    dwelling = _FakeDwelling(fake_helics.log)
    orchestrator = module.HELICSDwelling(dwelling, fed_name="house_1")
    orchestrator.register_publications()
    orchestrator.register_subscriptions()

    orchestrator.run()

    fed = fake_helics.last_fed
    assert fed is not None
    assert fed.disconnected is True
    assert fake_helics.HELICS_TIME_MAXTIME not in fed.requested_times
    assert fake_helics.log.count(("disconnect",)) == 1


class _MultiGrantFederate(_FakeFederate):
    """Fake federate that returns a predefined sequence of granted times on each ``request_time`` call."""

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


def test_helics_dwelling_handle_time_grant_helper(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    module, fake_helics = _import_dwelling_module(monkeypatch)
    fn = module._handle_time_grant

    assert fn(60.0, 60.0) is True
    assert fn(60.0, 10.0) is False
    assert fn(60.0, 0.0) is False
    assert fn(60.0, 30.0) is False
    assert fn(60.0, 90.0) is True
    assert fn(60.0, fake_helics.HELICS_TIME_MAXTIME) is True


def test_helics_dwelling_multi_rate_grants_only_steps_at_requested_time(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    module, fake_helics = _import_dwelling_module(monkeypatch)

    dwelling = _FakeDwelling(fake_helics.log)
    orchestrator = module.HELICSDwelling(dwelling, fed_name="house_1")
    orchestrator.register_publications()
    orchestrator.register_subscriptions()

    # Exit-time semantics: step 1 requests exit_time=60s. Five intermediate
    # grants (10-50s) are republished without stepping, then 60s is granted
    # and the dwelling steps. Step 2 requests exit_time=120s and is granted.
    multi_fed = _MultiGrantFederate(
        fed_name="house_1",
        fedinfo=fake_helics.last_fedinfo,
        log=fake_helics.log,
        grant_sequence=[10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 120.0],
    )
    orchestrator._fed = multi_fed

    orchestrator.run()

    assert dwelling._step_count == 2
    assert multi_fed.requested_times == [60.0, 60.0, 60.0, 60.0, 60.0, 60.0, 120.0, fake_helics.HELICS_TIME_MAXTIME]

    fed = fake_helics.last_fed
    assert fed is not None
    pub = fed.publications["total_power_kw"]
    # 5 intermediate publishes (before step 1) + 1 step-1 publish + 1 step-2 publish
    assert len(pub.published) == 7
    # All intermediate publishes carry state from before step 1
    for i in range(5):
        assert pub.published[i] == 3.0  # intermediate, unchanged
    assert pub.published[5] == 3.0  # step 1 publish (telemetry idx=0)
    assert pub.published[6] == 4.0  # step 2 publish

    assert multi_fed.disconnected is True


def test_helics_dwelling_federation_termination_sentinel_exits_without_stepping(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    module, fake_helics = _import_dwelling_module(monkeypatch)

    dwelling = _FakeDwelling(fake_helics.log)
    orchestrator = module.HELICSDwelling(dwelling, fed_name="house_1")
    orchestrator.register_publications()
    orchestrator.register_subscriptions()

    # Simulate federation termination: first request_time returns
    # HELICS_TIME_MAXTIME. After grant_sequence is exhausted the fallback
    # returns the requested value (handling the completion-signal call).
    term_fed = _MultiGrantFederate(
        fed_name="house_1",
        fedinfo=fake_helics.last_fedinfo,
        log=fake_helics.log,
        grant_sequence=[fake_helics.HELICS_TIME_MAXTIME],
    )
    orchestrator._fed = term_fed

    orchestrator.run()

    assert orchestrator._federation_terminated is True
    # Dwelling must NOT have stepped after receiving the sentinel
    assert dwelling._step_count == 0
    # No publish must have occurred for the terminated step
    fed = fake_helics.last_fed
    assert fed is not None
    pub = fed.publications["total_power_kw"]
    assert len(pub.published) == 0
    assert term_fed.disconnected is True


def test_helics_dwelling_federation_termination_sentinel_during_multi_rate_grant(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    module, fake_helics = _import_dwelling_module(monkeypatch)

    dwelling = _FakeDwelling(fake_helics.log)
    orchestrator = module.HELICSDwelling(dwelling, fed_name="house_1")
    orchestrator.register_publications()
    orchestrator.register_subscriptions()

    # Simulate: step 1 gets intermediate grants, then termination sentinel
    # before the full grant. The dwelling must NOT step.
    term_fed = _MultiGrantFederate(
        fed_name="house_1",
        fedinfo=fake_helics.last_fedinfo,
        log=fake_helics.log,
        grant_sequence=[10.0, 20.0, fake_helics.HELICS_TIME_MAXTIME],
    )
    orchestrator._fed = term_fed

    orchestrator.run()

    assert orchestrator._federation_terminated is True
    assert dwelling._step_count == 0
    fed = fake_helics.last_fed
    assert fed is not None
    pub = fed.publications["total_power_kw"]
    # Only intermediate publishes (before the termination sentinel)
    assert len(pub.published) == 2
    assert pub.published[0] == 3.0  # unchanged state, republished
    assert pub.published[1] == 3.0
    assert term_fed.disconnected is True


def test_peek_timing_derives_period_from_first_two_timesteps(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """_peek_timing computes period_s as the difference between the first two timesteps."""
    module, fake_helics = _import_dwelling_module(monkeypatch)
    dwelling = _FakeDwelling(fake_helics.log)
    orchestrator = module.HELICSDwelling(dwelling, fed_name="house_1")
    assert orchestrator._period_s == pytest.approx(60.0)
    assert orchestrator._start_time == datetime(2020, 1, 1, 0, 0, 0)


def test_peek_timing_defaults_period_for_single_timestep(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """_peek_timing defaults period_s to 1.0s when only one timestep is available."""
    module, fake_helics = _import_dwelling_module(monkeypatch)
    dwelling = _SingleTimestepDwelling(fake_helics.log)
    orchestrator = module.HELICSDwelling(dwelling, fed_name="house_1")
    assert orchestrator._period_s == pytest.approx(1.0)
    assert orchestrator._start_time == datetime(2020, 1, 1, 0, 0, 0)


def test_voltage_out_of_range_flagged_and_forwarded(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    module, fake_helics = _import_dwelling_module(monkeypatch)
    dwelling = _FakeDwelling(fake_helics.log)
    orchestrator = module.HELICSDwelling(dwelling, fed_name="house_1")
    orchestrator.register_publications()
    orchestrator.register_subscriptions(voltage_topic="grid/voltage")

    assert orchestrator._sub_voltage is not None

    # Voltage below range (< 0.5)
    orchestrator._sub_voltage.push_double(0.1)
    orchestrator._read_subscriptions()
    assert orchestrator._last_voltage_out_of_range is True
    assert dwelling.grid_voltage_values == [0.1]

    # Voltage above range (> 1.5)
    orchestrator._sub_voltage.push_double(10.0)
    orchestrator._read_subscriptions()
    assert dwelling.grid_voltage_values == [0.1, 10.0]


def test_voltage_in_range_forwarded_correctly(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    module, fake_helics = _import_dwelling_module(monkeypatch)
    dwelling = _FakeDwelling(fake_helics.log)
    orchestrator = module.HELICSDwelling(dwelling, fed_name="house_1")
    orchestrator.register_publications()
    orchestrator.register_subscriptions(voltage_topic="grid/voltage")

    assert orchestrator._sub_voltage is not None

    orchestrator._sub_voltage.push_double(0.95)
    orchestrator._read_subscriptions()
    assert dwelling.grid_voltage_values == [0.95]


def test_negative_price_flagged_and_forwarded(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    module, fake_helics = _import_dwelling_module(monkeypatch)
    dwelling = _FakeDwelling(fake_helics.log)
    orchestrator = module.HELICSDwelling(dwelling, fed_name="house_1")
    orchestrator.register_publications()
    orchestrator.register_subscriptions(price_topic="grid/price")

    assert orchestrator._sub_price is not None

    orchestrator._sub_price.push_double(-0.05)
    orchestrator._read_subscriptions()
    assert orchestrator._last_price_negative is True
    assert dwelling.price_signals == [{"electricity_price": -0.05}]


def test_non_negative_price_forwarded_correctly(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    module, fake_helics = _import_dwelling_module(monkeypatch)
    dwelling = _FakeDwelling(fake_helics.log)
    orchestrator = module.HELICSDwelling(dwelling, fed_name="house_1")
    orchestrator.register_publications()
    orchestrator.register_subscriptions(price_topic="grid/price")

    assert orchestrator._sub_price is not None

    orchestrator._sub_price.push_double(0.12)
    orchestrator._read_subscriptions()
    assert dwelling.price_signals == [{"electricity_price": 0.12}]


def test_set_publication_info_attaches_unit_metadata(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    module, fake_helics = _import_dwelling_module(monkeypatch)
    dwelling = _FakeDwelling(fake_helics.log)
    orchestrator = module.HELICSDwelling(dwelling, fed_name="house_1")
    orchestrator.register_publications()

    fed = fake_helics.last_fed
    assert fed is not None

    power_pub = fed.publications["total_power_kw"]
    reactive_pub = fed.publications["reactive_power_kvar"]
    assert power_pub._info == "units=kW"
    assert reactive_pub._info == "units=kvar"


def test_register_publications_includes_zone_temperature_keys(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    module, fake_helics = _import_dwelling_module(monkeypatch)
    dwelling = _FakeDwelling(fake_helics.log, n_zones=2)
    orchestrator = module.HELICSDwelling(dwelling, fed_name="house_1")
    pubs = orchestrator.register_publications()

    keys = [p.key for p in pubs]
    assert "house_1/zone_0/temp_air_c" in keys
    assert "house_1/zone_1/temp_air_c" in keys


def test_register_publications_includes_equipment_keys(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    module, fake_helics = _import_dwelling_module(monkeypatch)
    dwelling = _FakeDwelling(fake_helics.log, n_equipment=3)
    orchestrator = module.HELICSDwelling(dwelling, fed_name="house_1")
    pubs = orchestrator.register_publications()

    keys = [p.key for p in pubs]
    assert "house_1/equipment_0/power_kw" in keys
    assert "house_1/equipment_0/soc_pct" in keys
    assert "house_1/equipment_0/operating_mode" in keys
    assert "house_1/equipment_2/power_kw" in keys
    assert "house_1/equipment_2/soc_pct" in keys
    assert "house_1/equipment_2/operating_mode" in keys


def test_register_publications_includes_pv_generation_key(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    module, fake_helics = _import_dwelling_module(monkeypatch)
    dwelling = _FakeDwelling(fake_helics.log)
    orchestrator = module.HELICSDwelling(dwelling, fed_name="house_1")
    pubs = orchestrator.register_publications()

    keys = [p.key for p in pubs]
    assert "house_1/pv_generation_kw" in keys


def test_publish_results_outputs_zone_temperature(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    module, fake_helics = _import_dwelling_module(monkeypatch)
    dwelling = _FakeDwelling(fake_helics.log, n_zones=1)
    orchestrator = module.HELICSDwelling(dwelling, fed_name="house_1")
    orchestrator.register_publications()

    # Simulate a step
    dwelling._step_count = 1
    orchestrator._publish_results()

    fed = fake_helics.last_fed
    assert fed is not None
    zone_pub = fed.publications["zone_0/temp_air_c"]
    assert zone_pub.published == [21.5]


def test_publish_results_outputs_equipment_soc_as_percentage(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    module, fake_helics = _import_dwelling_module(monkeypatch)
    dwelling = _FakeDwelling(fake_helics.log, n_equipment=2)
    dwelling._equip_socs = [0.45, 0.80]
    orchestrator = module.HELICSDwelling(dwelling, fed_name="house_1")
    orchestrator.register_publications()

    dwelling._step_count = 1
    orchestrator._publish_results()

    fed = fake_helics.last_fed
    assert fed is not None
    soc_pub_0 = fed.publications["equipment_0/soc_pct"]
    soc_pub_1 = fed.publications["equipment_1/soc_pct"]
    assert soc_pub_0.published == [45.0]  # 0.45 * 100
    assert soc_pub_1.published == [80.0]  # 0.80 * 100


def test_publish_results_outputs_equipment_mode(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    module, fake_helics = _import_dwelling_module(monkeypatch)
    dwelling = _FakeDwelling(fake_helics.log, n_equipment=1)
    dwelling._equip_modes = [2.0]
    orchestrator = module.HELICSDwelling(dwelling, fed_name="house_1")
    orchestrator.register_publications()

    dwelling._step_count = 1
    orchestrator._publish_results()

    fed = fake_helics.last_fed
    assert fed is not None
    mode_pub = fed.publications["equipment_0/operating_mode"]
    assert mode_pub.published == [2.0]


def test_compute_pv_generation_sums_negative_power_from_pv_equipment(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    module, fake_helics = _import_dwelling_module(monkeypatch)

    class _PvTelemetry:
        def zone(self):
            return {"names": [], "temperature_c": []}

        def equipment(self):
            return {
                "names": ["PV_Array", "Battery", "Load"],
                "power_kw": [-5.0, 0.0, 2.0],
                "soc": [0.0, 0.5, 0.0],
                "modes": [0.0, 0.0, 0.0],
            }

    telemetry = _PvTelemetry()
    pv_kw = module.HELICSDwelling._compute_pv_generation(telemetry)
    assert pv_kw == 5.0


def test_compute_pv_generation_zero_when_no_pv_equipment(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    module, fake_helics = _import_dwelling_module(monkeypatch)

    class _NoPvTelemetry:
        def zone(self):
            return {"names": [], "temperature_c": []}

        def equipment(self):
            return {
                "names": ["Battery", "Load"],
                "power_kw": [0.0, 2.0],
                "soc": [0.5, 0.0],
                "modes": [0.0, 0.0],
            }

    telemetry = _NoPvTelemetry()
    pv_kw = module.HELICSDwelling._compute_pv_generation(telemetry)
    assert pv_kw == 0.0

