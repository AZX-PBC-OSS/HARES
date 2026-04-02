"""Unit tests for HELICS broker and runner helpers."""

from __future__ import annotations

import builtins
import importlib
import json
from pathlib import Path
import sys
import tempfile
from types import ModuleType
from typing import Any

import pytest


class _FakeBroker:
    def __init__(self, core_type: str, name: str, init_string: str) -> None:
        self.core_type = core_type
        self.name = name
        self.init_string = init_string
        self._connected = False
        self._checks = 0
        self.disconnected = False
        self.address = "tcp://127.0.0.1:24007"

    def is_connected(self) -> bool:
        self._checks += 1
        if self._checks >= 2:
            self._connected = True
        return self._connected

    def disconnect(self) -> None:
        self.disconnected = True


class _NeverConnectedBroker(_FakeBroker):
    def is_connected(self) -> bool:
        return False


class _FakeCli:
    def __init__(self) -> None:
        self.calls: list[str] = []
        self.loaded_configs: list[dict[str, Any]] = []

    def run(self, config_path: str) -> None:
        self.calls.append(config_path)
        with Path(config_path).open("r", encoding="utf-8") as handle:
            self.loaded_configs.append(json.load(handle))


class _FakeHelicsModule:
    def __init__(self) -> None:
        self.created_brokers: list[_FakeBroker] = []
        self.cli = _FakeCli()

    def helicsCreateBroker(self, core_type: str, name: str, init_string: str) -> _FakeBroker:
        broker = _FakeBroker(core_type, name, init_string)
        self.created_brokers.append(broker)
        return broker

    def helicsBrokerIsConnected(self, broker: _FakeBroker) -> bool:
        return broker.is_connected()


def _clear_helics_modules() -> None:
    sys.modules.pop("ochre_next.helics", None)
    sys.modules.pop("ochre_next.helics.broker", None)
    sys.modules.pop("ochre_next.helics.runner", None)


def _import_broker_runner_modules(
    monkeypatch: pytest.MonkeyPatch,
) -> tuple[ModuleType, ModuleType, _FakeHelicsModule]:
    fake_helics = _FakeHelicsModule()
    _clear_helics_modules()
    monkeypatch.setitem(sys.modules, "helics", fake_helics)
    broker_module = importlib.import_module("ochre_next.helics.broker")
    runner_module = importlib.import_module("ochre_next.helics.runner")
    return broker_module, runner_module, fake_helics


def test_broker_import_guard_without_helics(monkeypatch: pytest.MonkeyPatch) -> None:
    _clear_helics_modules()
    monkeypatch.delitem(sys.modules, "helics", raising=False)

    original_import = builtins.__import__

    def _raising_import(name: str, *args: Any, **kwargs: Any):
        if name == "helics":
            raise ImportError("missing helics")
        return original_import(name, *args, **kwargs)

    monkeypatch.setattr(builtins, "__import__", _raising_import)

    with pytest.raises(ImportError, match="HELICS not installed"):
        importlib.import_module("ochre_next.helics.broker")


def test_runner_import_guard_without_helics(monkeypatch: pytest.MonkeyPatch) -> None:
    _clear_helics_modules()
    monkeypatch.delitem(sys.modules, "helics", raising=False)

    original_import = builtins.__import__

    def _raising_import(name: str, *args: Any, **kwargs: Any):
        if name == "helics":
            raise ImportError("missing helics")
        return original_import(name, *args, **kwargs)

    monkeypatch.setattr(builtins, "__import__", _raising_import)

    with pytest.raises(ImportError, match="HELICS not installed"):
        importlib.import_module("ochre_next.helics.runner")


def test_create_broker_wait_and_shutdown(monkeypatch: pytest.MonkeyPatch) -> None:
    broker_module, _, fake_helics = _import_broker_runner_modules(monkeypatch)

    broker = broker_module.create_broker(n_federates=3, core_type="tcp", port=24567)
    broker_module.wait_for_broker(broker, timeout=1.0)

    assert len(fake_helics.created_brokers) == 1
    created = fake_helics.created_brokers[0]
    assert created.core_type == "tcp"
    assert "--federates=3" in created.init_string
    assert "--port=24567" in created.init_string
    assert broker_module.get_broker_port(broker) == 24567

    broker.disconnect()
    assert broker.disconnected is True


def test_create_broker_ephemeral_port_path(monkeypatch: pytest.MonkeyPatch) -> None:
    broker_module, _, fake_helics = _import_broker_runner_modules(monkeypatch)
    monkeypatch.setattr(broker_module, "_allocate_ephemeral_port", lambda: 26001)

    broker = broker_module.create_broker(n_federates=2, core_type="zmq", port=None)
    created = fake_helics.created_brokers[0]

    assert "--federates=2" in created.init_string
    assert "--port=26001" in created.init_string
    assert broker_module.get_broker_port(broker) == 26001


def test_create_broker_rejects_non_int_federate_count(monkeypatch: pytest.MonkeyPatch) -> None:
    broker_module, _, _ = _import_broker_runner_modules(monkeypatch)

    with pytest.raises(TypeError, match="must be an int"):
        broker_module.create_broker(n_federates=1.5)  # type: ignore[arg-type]


def test_get_broker_port_can_parse_address(monkeypatch: pytest.MonkeyPatch) -> None:
    broker_module, _, _ = _import_broker_runner_modules(monkeypatch)

    broker = _FakeBroker("zmq", "b", "--federates=1")
    broker.address = "tcp://localhost:33333"
    assert broker_module.get_broker_port(broker) == 33333


def test_get_broker_port_raises_when_unresolvable(monkeypatch: pytest.MonkeyPatch) -> None:
    broker_module, _, _ = _import_broker_runner_modules(monkeypatch)
    broker = object()

    with pytest.raises(RuntimeError, match="Unable to determine broker port"):
        broker_module.get_broker_port(broker)  # type: ignore[arg-type]


def test_wait_for_broker_times_out(monkeypatch: pytest.MonkeyPatch) -> None:
    broker_module, _, _ = _import_broker_runner_modules(monkeypatch)
    broker = _NeverConnectedBroker("zmq", "b", "--federates=1")

    with pytest.raises(TimeoutError, match="did not connect"):
        broker_module.wait_for_broker(broker, timeout=0.01)


def test_wait_for_broker_raises_when_connectivity_api_missing(monkeypatch: pytest.MonkeyPatch) -> None:
    broker_module, _, _ = _import_broker_runner_modules(monkeypatch)

    class _NoConnectivityHelics:
        pass

    monkeypatch.setattr(broker_module, "helics", _NoConnectivityHelics())

    class _BrokerNoConnectivityApi:
        pass

    with pytest.raises(RuntimeError, match="connectivity"):
        broker_module.wait_for_broker(_BrokerNoConnectivityApi(), timeout=0.1)


def test_generate_cosim_config_structure(monkeypatch: pytest.MonkeyPatch) -> None:
    _, runner_module, _ = _import_broker_runner_modules(monkeypatch)

    federates = [
        runner_module.FederateConfig(name="gridlabd", exec_command="gridlabd feeder.glm"),
        runner_module.FederateConfig(name="house_1", exec_command="python run_house.py"),
    ]

    config = runner_module.generate_cosim_config(federates)

    assert config["name"] == "hares-cosim"
    assert config["broker"] is True
    assert len(config["federates"]) == 2
    assert config["federates"][0]["name"] == "gridlabd"
    assert config["federates"][0]["exec"] == "gridlabd feeder.glm"


def test_generate_cosim_config_broker_false(monkeypatch: pytest.MonkeyPatch) -> None:
    _, runner_module, _ = _import_broker_runner_modules(monkeypatch)

    config = runner_module.generate_cosim_config(
        [runner_module.FederateConfig(name="house_1", exec_command="python run_house.py")],
        broker=False,
    )

    assert config["broker"] is False


def test_run_cosimulation_dict_writes_temp_json(monkeypatch: pytest.MonkeyPatch) -> None:
    _, runner_module, fake_helics = _import_broker_runner_modules(monkeypatch)

    config = {
        "name": "hares-cosim",
        "broker": True,
        "federates": [{"name": "gridlabd", "host": "localhost", "directory": ".", "exec": "gridlabd feeder.glm"}],
    }

    runner_module.run_cosimulation(config)

    assert len(fake_helics.cli.calls) == 1
    temp_path = Path(fake_helics.cli.calls[0])
    assert temp_path.exists() is False
    assert fake_helics.cli.loaded_configs[0]["name"] == "hares-cosim"


def test_run_cosimulation_path_passthrough(monkeypatch: pytest.MonkeyPatch) -> None:
    _, runner_module, fake_helics = _import_broker_runner_modules(monkeypatch)

    with tempfile.NamedTemporaryFile(mode="w", suffix=".json", delete=False) as handle:
        json.dump({"name": "existing", "broker": False, "federates": []}, handle)
        path = Path(handle.name)

    try:
        runner_module.run_cosimulation(path)
    finally:
        path.unlink(missing_ok=True)

    assert fake_helics.cli.calls == [str(path)]


def test_run_cosimulation_cleans_temp_file_on_json_dump_error(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _, runner_module, _ = _import_broker_runner_modules(monkeypatch)
    leaked: list[Path] = []
    original_mkstemp = runner_module.tempfile.mkstemp

    def _tracking_mkstemp(*args: Any, **kwargs: Any) -> tuple[int, str]:
        fd, path = original_mkstemp(*args, **kwargs)
        leaked.append(Path(path))
        return fd, path

    monkeypatch.setattr(runner_module.tempfile, "mkstemp", _tracking_mkstemp)
    monkeypatch.setattr(runner_module.json, "dump", lambda *_args, **_kwargs: (_ for _ in ()).throw(OSError("disk full")))

    with pytest.raises(OSError, match="disk full"):
        runner_module.run_cosimulation({"name": "x", "broker": True, "federates": []})

    assert leaked, "test should capture temp file path"
    assert all(path.exists() is False for path in leaked)


def test_make_dwelling_federate_config(monkeypatch: pytest.MonkeyPatch) -> None:
    _, runner_module, _ = _import_broker_runner_modules(monkeypatch)

    fed = runner_module.make_dwelling_federate_config(
        name="house_1",
        hpxml_path=Path("/data/house.xml"),
        weather_path=Path("/data/weather.epw"),
        schedule_path=Path("/data/schedule.csv"),
        broker="localhost:24007",
    )

    assert fed.name == "house_1"
    assert "python -m ochre_next.helics.dwelling" in fed.exec_command
    assert "--hpxml=/data/house.xml" in fed.exec_command
    assert "--weather=/data/weather.epw" in fed.exec_command
    assert "--schedule=/data/schedule.csv" in fed.exec_command
    assert "--broker=localhost:24007" in fed.exec_command


def test_make_dwelling_federate_config_quotes_paths_and_values(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _, runner_module, _ = _import_broker_runner_modules(monkeypatch)

    fed = runner_module.make_dwelling_federate_config(
        name="house 1",
        hpxml_path=Path("/data/my house/house file.xml"),
        weather_path=Path("/data/weather file.epw"),
        schedule_path=Path("/data/schedule file.csv"),
        broker="localhost; rm -rf /",
    )

    assert "--name='house 1'" in fed.exec_command
    assert "--hpxml='/data/my house/house file.xml'" in fed.exec_command
    assert "--weather='/data/weather file.epw'" in fed.exec_command
    assert "--schedule='/data/schedule file.csv'" in fed.exec_command
    assert "--broker='localhost; rm -rf /'" in fed.exec_command


def test_make_dwelling_federate_config_rejects_invalid_kwarg_key(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _, runner_module, _ = _import_broker_runner_modules(monkeypatch)

    with pytest.raises(ValueError, match="Invalid CLI argument key"):
        runner_module.make_dwelling_federate_config(
            name="house_1",
            hpxml_path=Path("/data/house.xml"),
            weather_path=Path("/data/weather.epw"),
            schedule_path=Path("/data/schedule.csv"),
            **{"bad key": "x"},
        )


def test_destroy_broker_disconnects(monkeypatch: pytest.MonkeyPatch) -> None:
    broker_module, _, _ = _import_broker_runner_modules(monkeypatch)

    broker = _FakeBroker("zmq", "test_broker", "--federates=1")
    assert broker.disconnected is False

    broker_module.destroy_broker(broker)
    assert broker.disconnected is True

    # Idempotent — does not raise on second call
    broker_module.destroy_broker(broker)
