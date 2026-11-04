"""Unit tests for HELICS broker and runner helpers."""

from __future__ import annotations

import builtins
import importlib
import json
import os
from pathlib import Path
import shlex
import socket
import subprocess
import sys
from types import ModuleType, SimpleNamespace
from typing import Any

import pytest

# These tests use in-process fakes (no HELICS networking), so any hang is a
# logic bug; the thread method also catches blocking inside C extensions.
pytestmark = [pytest.mark.timeout(60, method="thread"), pytest.mark.usefixtures("restore_helics_modules")]


class _FakeBroker:
    def __init__(self, core_type: str, name: str, init_string: str) -> None:
        self.core_type = core_type
        self.name = name
        self.init_string = init_string
        self.disconnected = False
        self.address = "tcp://127.0.0.1:24007"

    def is_connected(self) -> bool:
        return True

    def disconnect(self) -> None:
        self.disconnected = True


class _NeverConnectedBroker(_FakeBroker):
    """A broker whose sockets failed to bind: HELICS returns it unconnected."""

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
        self.unbindable_ports: set[int] = set()
        self.cli = _FakeCli()

    def helicsCreateBroker(self, core_type: str, name: str, init_string: str) -> _FakeBroker:
        port = int(init_string.rpartition("--port=")[2])
        broker_type = _NeverConnectedBroker if port in self.unbindable_ports else _FakeBroker
        broker = broker_type(core_type, name, init_string)
        self.created_brokers.append(broker)
        return broker

    def helicsBrokerIsConnected(self, broker: _FakeBroker) -> bool:
        return broker.is_connected()


def _clear_helics_modules() -> None:
    sys.modules.pop("hares.helics", None)
    sys.modules.pop("hares.helics.broker", None)
    sys.modules.pop("hares.helics.runner", None)


def _import_broker_runner_modules(
    monkeypatch: pytest.MonkeyPatch,
) -> tuple[ModuleType, ModuleType, _FakeHelicsModule]:
    fake_helics = _FakeHelicsModule()
    _clear_helics_modules()
    monkeypatch.setitem(sys.modules, "helics", fake_helics)
    broker_module = importlib.import_module("hares.helics.broker")
    runner_module = importlib.import_module("hares.helics.runner")
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
        importlib.import_module("hares.helics.broker")


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
        importlib.import_module("hares.helics.runner")


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
    monkeypatch.setattr(broker_module, "allocate_ephemeral_port", lambda: 26001)

    broker = broker_module.create_broker(n_federates=2, core_type="zmq", port=None)
    created = fake_helics.created_brokers[0]

    assert "--federates=2" in created.init_string
    assert "--port=26001" in created.init_string
    assert broker_module.get_broker_port(broker) == 26001


def test_create_broker_raises_when_the_broker_cannot_listen(monkeypatch: pytest.MonkeyPatch) -> None:
    broker_module, _, fake_helics = _import_broker_runner_modules(monkeypatch)
    fake_helics.unbindable_ports.add(24567)

    with pytest.raises(RuntimeError, match="could not listen on port 24567"):
        broker_module.create_broker(n_federates=2, core_type="zmq", port=24567)

    assert fake_helics.created_brokers[0].disconnected is True


def test_create_broker_moves_past_an_ephemeral_port_it_cannot_listen_on(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    broker_module, _, fake_helics = _import_broker_runner_modules(monkeypatch)
    fake_helics.unbindable_ports.add(26001)
    ports = iter([26001, 26003])
    monkeypatch.setattr(broker_module, "allocate_ephemeral_port", lambda: next(ports))

    broker = broker_module.create_broker(n_federates=2, core_type="zmq", port=None)

    assert [b.disconnected for b in fake_helics.created_brokers] == [True, False]
    assert broker_module.get_broker_port(broker) == 26003


class _PortTable:
    """Stands in for the ``socket`` module with a fixed set of ports already bound."""

    AF_INET = socket.AF_INET
    SOCK_STREAM = socket.SOCK_STREAM

    def __init__(self, bound: set[int]) -> None:
        self.bound = bound

    def socket(self, *_args: Any) -> _PortTable._Socket:
        return _PortTable._Socket(self.bound)

    class _Socket:
        def __init__(self, bound: set[int]) -> None:
            self._bound = bound

        def __enter__(self) -> _PortTable._Socket:
            return self

        def __exit__(self, *_exc: object) -> None:
            return None

        def bind(self, address: tuple[str, int]) -> None:
            if address[1] in self._bound:
                raise OSError("address in use")


def test_allocate_ephemeral_port_needs_the_port_and_the_next_one_free(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    broker_module, _, _ = _import_broker_runner_modules(monkeypatch)
    monkeypatch.setattr(broker_module, "socket", _PortTable({25001, 26000}))
    candidates = iter([25000, 26000, 27000])
    monkeypatch.setattr(broker_module, "random", SimpleNamespace(randint=lambda _lo, _hi: next(candidates)))

    assert broker_module.allocate_ephemeral_port() == 27000


def test_allocate_ephemeral_port_draws_clear_of_ports_with_other_owners(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    broker_module, _, _ = _import_broker_runner_modules(monkeypatch)
    bounds: list[tuple[int, int]] = []

    def _record(lo: int, hi: int) -> int:
        bounds.append((lo, hi))
        return lo

    monkeypatch.setattr(broker_module, "socket", _PortTable(set()))
    monkeypatch.setattr(broker_module, "random", SimpleNamespace(randint=_record))

    broker_module.allocate_ephemeral_port()

    (lo, hi), = bounds
    drawable = range(lo, hi + 2)  # a pair: the port and the next one
    assert lo >= 1024
    assert hi + 1 < 30000, "the pair must stay below the Kubernetes NodePort and kernel ephemeral ranges"
    helics_defaults = (23404, 23405, 23414, 23415, 23500, 23901, 24160)
    assert not any(port in drawable for port in helics_defaults), "HELICS default ports"


def test_allocate_ephemeral_port_raises_when_no_pair_is_free(monkeypatch: pytest.MonkeyPatch) -> None:
    broker_module, _, _ = _import_broker_runner_modules(monkeypatch)
    monkeypatch.setattr(broker_module, "socket", _PortTable({25000}))
    monkeypatch.setattr(broker_module, "random", SimpleNamespace(randint=lambda _lo, _hi: 25000))

    with pytest.raises(RuntimeError, match="No free localhost port pair"):
        broker_module.allocate_ephemeral_port()


def test_create_broker_rejects_non_int_federate_count(monkeypatch: pytest.MonkeyPatch) -> None:
    broker_module, _, _ = _import_broker_runner_modules(monkeypatch)

    not_an_int: Any = 1.5
    with pytest.raises(TypeError, match="must be an int"):
        broker_module.create_broker(n_federates=not_an_int)


def test_get_broker_port_can_parse_address(monkeypatch: pytest.MonkeyPatch) -> None:
    broker_module, _, _ = _import_broker_runner_modules(monkeypatch)

    broker = _FakeBroker("zmq", "b", "--federates=1")
    broker.address = "tcp://localhost:33333"
    assert broker_module.get_broker_port(broker) == 33333


def test_get_broker_port_raises_when_unresolvable(monkeypatch: pytest.MonkeyPatch) -> None:
    broker_module, _, _ = _import_broker_runner_modules(monkeypatch)
    broker: Any = object()

    with pytest.raises(RuntimeError, match="Unable to determine broker port"):
        broker_module.get_broker_port(broker)


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


def test_run_cosimulation_path_passthrough(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    _, runner_module, fake_helics = _import_broker_runner_modules(monkeypatch)
    path = tmp_path / "cosim.json"
    path.write_text(json.dumps({"name": "existing", "broker": False, "federates": []}), encoding="utf-8")

    runner_module.run_cosimulation(path)

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
        start="2019-01-01T00:00:00",
        **{"duration-s": 86400, "time-res-s": 900},
    )

    assert fed.name == "house_1"
    assert "python -m hares.helics.dwelling" in fed.exec_command
    assert "--hpxml=/data/house.xml" in fed.exec_command
    assert "--weather=/data/weather.epw" in fed.exec_command
    assert "--schedule=/data/schedule.csv" in fed.exec_command
    assert "--broker=localhost:24007" in fed.exec_command
    assert "--start=2019-01-01T00:00:00" in fed.exec_command
    assert "--duration-s=86400" in fed.exec_command
    assert "--time-res-s=900" in fed.exec_command


def test_make_dwelling_federate_config_accepts_the_entry_points_flags(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _, runner_module, _ = _import_broker_runner_modules(monkeypatch)

    fed = runner_module.make_dwelling_federate_config(
        name="house_1",
        hpxml_path=Path("/data/house.xml"),
        weather_path=Path("/data/weather.epw"),
        schedule_path=Path("/data/schedule.csv"),
        broker="localhost:24007",
        start="2019-01-01T00:00:00",
        **{"duration-s": 86400, "time-res-s": 900, "core-type": "zmq", "master-seed": 7},
    )

    assert "--broker=localhost:24007" in fed.exec_command
    assert "--core-type=zmq" in fed.exec_command
    assert "--master-seed=7" in fed.exec_command


def test_make_dwelling_federate_config_refuses_a_flag_the_entry_point_does_not_define(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """A key the entry point does not define is refused where the command is
    built: the emitted ``--broker_address`` would otherwise pass this helper
    and die at the child's argparse exit after the launch."""
    _, runner_module, _ = _import_broker_runner_modules(monkeypatch)

    with pytest.raises(ValueError, match="Unknown dwelling federate argument 'broker_address'"):
        runner_module.make_dwelling_federate_config(
            name="house_1",
            hpxml_path=Path("/data/house.xml"),
            weather_path=Path("/data/weather.epw"),
            schedule_path=Path("/data/schedule.csv"),
            broker_address="localhost:24007",
            start="2019-01-01T00:00:00",
            **{"duration-s": 86400, "time-res-s": 900},
        )


def test_make_dwelling_federate_config_refuses_a_command_without_its_horizon(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """The entry point has no default horizon, so a command without one exits
    at its own required-argument check. The helper refuses it at build time
    instead, naming the missing flags."""
    _, runner_module, _ = _import_broker_runner_modules(monkeypatch)

    with pytest.raises(ValueError, match="needs its horizon.*--start"):
        runner_module.make_dwelling_federate_config(
            name="house_1",
            hpxml_path=Path("/data/house.xml"),
            weather_path=Path("/data/weather.epw"),
            schedule_path=Path("/data/schedule.csv"),
            broker="localhost:24007",
        )


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
        start="2019-01-01T00:00:00",
        **{"duration-s": 86400, "time-res-s": 900},
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


@pytest.mark.timeout(120, method="thread")
def test_the_emitted_dwelling_command_joins_the_federation() -> None:
    """The runner's emitted command launches a process that JOINS the federation.

    The emitted command is executed verbatim against a real in-process broker
    (real helics, the repo's own package on the interpreter's path): the probe
    federate on the same broker cannot enter executing mode until the dwelling
    process registers and enters, and then receives the dwelling's published
    power. The old no-op (a module with no entry point: it imported, exited 0,
    and joined nothing) fails here at the probe's executing-mode timeout with
    every peer waiting out its timeout behind it.
    """
    import helics

    from conftest import HPXML, ROOT, SCHEDULE, WEATHER

    broker_module = importlib.import_module("hares.helics.broker")
    federate_module = importlib.import_module("hares.helics.federate")
    runner_module = importlib.import_module("hares.helics.runner")

    broker = broker_module.create_broker(n_federates=2, core_type="zmq", port=None)
    probe: Any = None
    try:
        port = broker_module.get_broker_port(broker)
        config = runner_module.make_dwelling_federate_config(
            name="house_1",
            hpxml_path=Path(HPXML),
            weather_path=Path(WEATHER),
            schedule_path=Path(SCHEDULE),
            broker=f"127.0.0.1:{port}",
            **{
                "core-type": "zmq",
                "start": "2019-01-01T00:00:00",
                "duration-s": 1800,
                "time-res-s": 900,
            },
        )

        env = dict(os.environ)
        env["PATH"] = f"{Path(sys.executable).parent}{os.pathsep}{env.get('PATH', '')}"
        # The offline guard's sanctioned python child: the guard's own
        # sitecustomize dir leads PYTHONPATH and the flag is set, so the
        # child installs the same refusal (its zmq transport reaches
        # loopback only, by construction).
        from offline_guard import SITE_DIR

        env["HARES_TESTS_OFFLINE"] = "1"
        env["PYTHONPATH"] = os.pathsep.join([str(SITE_DIR), str(ROOT / "python")])
        proc = subprocess.Popen(
            shlex.split(config.exec_command),
            cwd=str(ROOT),
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        try:
            probe = federate_module.create_value_federate(
                "probe_1",
                "zmq",
                f"127.0.0.1:{port}",
                20.0,
                lambda core_name, core_init: _probe_fedinfo("zmq", core_name, core_init),
            )
            sub_power = probe.register_subscription("house_1/total_power_w", "double")
            federate_module.enter_executing_mode_with_timeout(probe, 20.0, "probe_1")

            received: list[float] = []
            granted = federate_module.request_time_with_timeout(probe, 60.0, 30.0, "probe_1")
            step = 120.0
            while granted < helics.HELICS_TIME_MAXTIME and len(received) < 2 and step <= 3600.0:
                if sub_power.is_updated():
                    received.append(float(sub_power.double))
                granted = federate_module.request_time_with_timeout(probe, step, 30.0, "probe_1")
                step += 60.0
        finally:
            stdout, stderr = proc.communicate(timeout=60)
            if probe is not None:
                probe.disconnect()

        assert proc.returncode == 0, (
            f"the emitted command must run to its horizon's end: rc={proc.returncode}\n"
            f"stdout: {stdout.decode()[-2000:]}\nstderr: {stderr.decode()[-2000:]}"
        )
        assert received, (
            "a joined dwelling federate publishes its power to the federation; "
            "the probe received nothing"
        )
    finally:
        broker_module.destroy_broker(broker)


@pytest.mark.timeout(60, method="thread")
def test_the_dwelling_command_reports_its_input_errors_without_a_traceback() -> None:
    """The command's input errors are the typed error's exit, not a traceback.

    The entry point builds the dwelling before it joins anything, so a bad
    HPXML path and a bad horizon are configuration errors: the child prints
    the typed message (the file it could not read, the field it could not
    parse) and exits nonzero, rather than dumping the interpreter's frames
    at the runner's log. No broker is created: the failures are before
    registration.
    """
    from conftest import ROOT, SCHEDULE, WEATHER

    env = dict(os.environ)
    env["PATH"] = f"{Path(sys.executable).parent}{os.pathsep}{env.get('PATH', '')}"
    from offline_guard import SITE_DIR

    env["HARES_TESTS_OFFLINE"] = "1"
    env["PYTHONPATH"] = os.pathsep.join([str(SITE_DIR), str(ROOT / "python")])

    def _run(hpxml: Path, start: str) -> subprocess.CompletedProcess[bytes]:
        return subprocess.run(
            [
                sys.executable,
                "-m",
                "hares.helics.dwelling",
                "--name=house_1",
                f"--hpxml={hpxml}",
                f"--weather={WEATHER}",
                f"--schedule={SCHEDULE}",
                f"--start={start}",
                "--duration-s=900",
                "--time-res-s=900",
            ],
            cwd=str(ROOT),
            env=env,
            capture_output=True,
            timeout=45,
        )

    bad_hpxml = _run(
        ROOT / "tests/fixtures/hpxml/ochre_samples/no_such_file.xml",
        "2019-01-01T00:00:00",
    )
    assert bad_hpxml.returncode != 0, "a bad HPXML path must fail the launch"
    assert "Traceback" not in bad_hpxml.stderr.decode(), (
        "the typed configuration error is the exit's message, not a traceback:\n"
        f"{bad_hpxml.stderr.decode()}"
    )
    assert "no_such_file.xml" in bad_hpxml.stderr.decode(), (
        f"the refusal names the file it could not read:\n{bad_hpxml.stderr.decode()}"
    )

    bad_start = _run(
        ROOT / "tests/fixtures/hpxml/ochre_samples/base.xml",
        "not-a-timestamp",
    )
    assert bad_start.returncode != 0, "an unparseable start must fail the launch"
    assert "Traceback" not in bad_start.stderr.decode(), (
        "the horizon's typed validation is the exit's message, not a traceback:\n"
        f"{bad_start.stderr.decode()}"
    )
    assert "invalid start_time format" in bad_start.stderr.decode(), (
        f"the refusal names the start it could not parse:\n{bad_start.stderr.decode()}"
    )


def _probe_fedinfo(core_type: str, core_name: str, core_init: str) -> Any:
    import helics

    fedinfo = helics.helicsCreateFederateInfo()
    helics.helicsFederateInfoSetCoreTypeFromString(fedinfo, core_type)
    helics.helicsFederateInfoSetCoreName(fedinfo, core_name)
    helics.helicsFederateInfoSetCoreInitString(fedinfo, core_init)
    helics.helicsFederateInfoSetTimeProperty(
        fedinfo, helics.HELICS_PROPERTY_TIME_PERIOD, 60.0
    )
    return fedinfo


def test_destroy_broker_disconnects(monkeypatch: pytest.MonkeyPatch) -> None:
    broker_module, _, _ = _import_broker_runner_modules(monkeypatch)

    broker = _FakeBroker("zmq", "test_broker", "--federates=1")
    assert broker.disconnected is False

    broker_module.destroy_broker(broker)
    assert broker.disconnected is True

    # Idempotent -- does not raise on second call
    broker_module.destroy_broker(broker)
