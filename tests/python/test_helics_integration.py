"""End-to-end HELICS integration tests with mock aggregator federates."""

from __future__ import annotations

import builtins
from collections.abc import Callable
import importlib
import importlib.util
import itertools
import json
import os
import re
import socket
import subprocess
import sys
import threading
from typing import Any, Self, TYPE_CHECKING

import pytest

from conftest import HARES_DEFAULTS, HPXML, SCHEDULE, WEATHER

from ochre_next import Battery, ControlSignal, Dwelling, DwellingConfig, SimulationConfig, SteppableFleet

if TYPE_CHECKING:
    # The extension's step() returns a plain dict at runtime; its declared
    # shape lives in ochre_next._hares_types.
    from ochre_next._hares_types import StepResult
from ochre_next.helics import (
    HELICSDwelling,
    HELICSFleet,
    create_broker,
    enter_executing_mode_with_timeout,
    get_broker_port,
    request_time_with_timeout,
    wait_for_pending_aborts,
)
from ochre_next.helics.federate import core_init_string

helics_available = importlib.util.find_spec("helics") is not None
if not helics_available:  # pragma: no cover - exercised when optional dependency is missing
    pytest.skip("helics not installed", allow_module_level=True)
import helics

pytestmark = [
    # Last-resort backstop: raw HELICS calls block inside the C library where
    # SIGALRM cannot interrupt them, so use pytest-timeout's thread method.
    pytest.mark.timeout(120, method="thread"),
    pytest.mark.usefixtures("helics_environment_guard"),
]


_TIME_RES_S = 60.0
_TOTAL_STEPS = 10
_DURATION_S = int(_TIME_RES_S * _TOTAL_STEPS)

# Short explicit timeouts so a stale broker or stalled peer fails a test in
# seconds with a clear TimeoutError instead of hanging the whole suite.
_CONNECT_TIMEOUT_S = 20.0
_GRANT_TIMEOUT_S = 30.0

# Default HELICS broker ports (zmq broker port and its priority channel).
# Tests never use these (ephemeral ports only), but a leftover process bound
# to them is the signature of stale HELICS state on this machine.
_HELICS_DEFAULT_PORTS = (23404, 23405)

_STALE_PROCESS_PATTERN = re.compile(
    r"helics[-_]broker"  # standalone broker binary
    r"|python[0-9.]*\S*\s+\S*helics\S*\.py",  # e.g. `python /tmp/helics_debug_test16.py`
    re.IGNORECASE,
)


def _listening_process(port: int) -> str | None:
    """Return ``'PID <pid> (<command>)'`` for a listener on ``port``, else ``None``."""
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.settimeout(0.25)
        if sock.connect_ex(("127.0.0.1", port)) != 0:
            return None
    try:
        out = subprocess.run(
            ["lsof", "-nP", f"-iTCP:{port}", "-sTCP:LISTEN", "-Fpc"],
            capture_output=True,
            text=True,
            timeout=5.0,
        ).stdout
    except (OSError, subprocess.TimeoutExpired):
        return "an unidentified process (lsof unavailable)"
    pid: str | None = None
    command: str | None = None
    for line in out.splitlines():
        if line.startswith("p"):
            pid = line[1:]
        elif line.startswith("c") and command is None:
            command = line[1:]
    if pid is None:
        return "an unidentified process"
    return f"PID {pid} ({command or 'unknown command'})"


def _process_table() -> list[tuple[int, int, str]]:
    """Return ``(pid, ppid, command)`` rows from ``ps``; empty list on failure."""
    try:
        out = subprocess.run(
            ["ps", "-axo", "pid=,ppid=,command="],
            capture_output=True,
            text=True,
            timeout=5.0,
        ).stdout
    except (OSError, subprocess.TimeoutExpired):
        return []
    rows: list[tuple[int, int, str]] = []
    for line in out.splitlines():
        parts = line.split(None, 2)
        if len(parts) < 3:
            continue
        try:
            rows.append((int(parts[0]), int(parts[1]), parts[2]))
        except ValueError:
            continue
    return rows


def _stale_helics_processes() -> list[str]:
    """Find pre-existing HELICS broker/federate processes, excluding this session.

    Our own process tree (pytest, uv, xdist controller) is excluded because its
    command lines legitimately mention HELICS test file paths.
    """
    rows = _process_table()
    parent_by_pid = {pid: ppid for pid, ppid, _ in rows}
    own_tree = {os.getpid()}
    cursor = os.getpid()
    for _ in range(64):
        cursor = parent_by_pid.get(cursor, 0)
        if cursor <= 1:
            break
        own_tree.add(cursor)

    offenders: list[str] = []
    for pid, _, command in rows:
        if pid in own_tree:
            continue
        if _STALE_PROCESS_PATTERN.search(command):
            offenders.append(f"PID {pid} ({command.strip()})")
    return offenders


@pytest.fixture(scope="session")
def helics_environment_guard() -> None:
    """Fail fast when leftover HELICS state would make these tests hang.

    A stale broker or federate process (e.g. a forgotten debug script) holds
    HELICS federation state; tests that join it block indefinitely with no
    output.  Detect that state up front and fail with the offending PID/port
    instead of hanging.
    """
    problems: list[str] = []
    for port in _HELICS_DEFAULT_PORTS:
        holder = _listening_process(port)
        if holder is not None:
            problems.append(f"HELICS default port {port} is held by {holder}")
    for offender in _stale_helics_processes():
        problems.append(f"pre-existing HELICS process: {offender}")

    if problems:
        pytest.fail(
            "Stale HELICS state detected before running HELICS tests:\n  - "
            + "\n  - ".join(problems)
            + "\nLeftover HELICS broker/federate processes make these tests hang "
            "indefinitely with no output. Kill the offending process(es) "
            "(e.g. `kill <PID>`) and re-run. If another HELICS test session is "
            "running concurrently, wait for it to finish.",
            pytrace=False,
        )


class _FederateThread:
    """A federate driven on a thread of its own.

    HELICS federate handles have thread affinity: a handle created with
    ``helicsCreateValueFederate`` must be driven (``enterExecutingMode``,
    ``requestTime``) on the thread that created it, or the exec-mode barrier
    deadlocks. Construct and run ``HELICSDwelling``/``HELICSFleet`` inside the
    target.
    """

    def __init__(self, target: Callable[[], None], name: str) -> None:
        self.completed = False
        self.exception: BaseException | None = None
        self._thread = threading.Thread(target=self._run, args=(target,), name=name, daemon=True)
        self._thread.start()

    def _run(self, target: Callable[[], None]) -> None:
        try:
            target()
            self.completed = True
        except BaseException as exc:
            self.exception = exc

    def join(self) -> None:
        """Wait for the federate to leave the federation.

        Unbounded: every blocking HELICS call a federate makes is bounded by
        its own connect or grant timeout, so the thread always ends.
        """
        self._thread.join()

    def result(self) -> None:
        """Join, then re-raise the federate's exception on the test thread."""
        self.join()
        if self.exception is not None:
            raise self.exception
        assert self.completed


class _Federation:
    """One test's HELICS federation, torn down whole on exit, failure included.

    By default the broker is in-process under a name no other federation
    uses, so concurrent tests never share a broker or a socket.
    ``core_type="zmq"`` runs the networked transport on a broker port of its
    own. Teardown disconnects the test's own federates and the broker, which
    releases every federate still waiting on a peer, then joins the federate
    threads before the library frees the disconnected cores.
    """

    _ids = itertools.count()

    def __init__(self, n_federates: int, *, core_type: str = "inproc", port: int | None = None) -> None:
        self.core_type = core_type
        self._federates: list[Any] = []
        self._threads: list[_FederateThread] = []
        if core_type == "inproc":
            self.address = f"federation_{os.getpid()}_{next(self._ids)}"
            self._broker = helics.helicsCreateBroker(core_type, self.address, f"--federates={n_federates}")
        else:
            self._broker = create_broker(n_federates, core_type=core_type, port=port)
            self.port = get_broker_port(self._broker)
            self.address = f"127.0.0.1:{self.port}"

    def __enter__(self) -> Self:
        return self

    def __exit__(self, *_exc: object) -> None:
        try:
            for fed in self._federates:
                fed.disconnect()
        finally:
            self._broker.disconnect()
            for thread in self._threads:
                thread.join()
            assert wait_for_pending_aborts(), "a timed-out federate is still inside HELICS"
            helics.helicsCleanupLibrary()

    def federate_info(self, core_name: str, time_res_s: float = _TIME_RES_S) -> Any:
        fedinfo = helics.helicsCreateFederateInfo()
        helics.helicsFederateInfoSetCoreTypeFromString(fedinfo, self.core_type)
        helics.helicsFederateInfoSetCoreName(fedinfo, f"core_{core_name}")
        helics.helicsFederateInfoSetCoreInitString(
            fedinfo, core_init_string(self.core_type, self.address, _CONNECT_TIMEOUT_S)
        )
        helics.helicsFederateInfoSetTimeProperty(fedinfo, helics.HELICS_PROPERTY_TIME_PERIOD, time_res_s)
        return fedinfo

    def value_federate(self, name: str, time_res_s: float = _TIME_RES_S) -> Any:
        """A federate the test drives from its own thread, disconnected at teardown."""
        fed = helics.helicsCreateValueFederate(name, self.federate_info(name, time_res_s))
        self._federates.append(fed)
        return fed

    def helics_dwelling(self, dwelling: Any, fed_name: str, **kwargs: Any) -> HELICSDwelling:
        return HELICSDwelling(dwelling=dwelling, fed_name=fed_name, **self._federate_kwargs(kwargs))

    def helics_fleet(self, fleet: SteppableFleet, fed_name: str, **kwargs: Any) -> HELICSFleet:
        return HELICSFleet(fleet=fleet, fed_name=fed_name, **self._federate_kwargs(kwargs))

    def start(self, target: Callable[[], None], name: str) -> _FederateThread:
        thread = _FederateThread(target, name)
        self._threads.append(thread)
        return thread

    def _federate_kwargs(self, overrides: dict[str, Any]) -> dict[str, Any]:
        return {
            "broker_address": self.address,
            "core_type": self.core_type,
            "connect_timeout_s": _CONNECT_TIMEOUT_S,
            "grant_timeout_s": _GRANT_TIMEOUT_S,
        } | overrides


class _FederateProbe:
    """Proxy around a HELICS federate that records requested time and disconnect calls.

    Both the blocking and async request/disconnect entry points are recorded:
    production code drives real federates through the async API (with a
    wall-clock deadline) and falls back to the blocking API for test doubles.

    ``granted_times`` is populated from both ``request_time()`` (blocking) and
    ``request_time_complete()`` (async) so that callers see the grant
    regardless of which path ``request_time_with_timeout`` chose.
    """

    def __init__(self, fed: Any) -> None:
        self._fed = fed
        self.requested_times: list[float] = []
        self.granted_times: list[float] = []
        self.disconnect_called = False

    def enter_executing_mode(self) -> Any:
        return self._fed.enter_executing_mode()

    def request_time(self, requested: float) -> float:
        self.requested_times.append(float(requested))
        granted = float(self._fed.request_time(requested))
        self.granted_times.append(granted)
        return granted

    def request_time_async(self, requested: float) -> Any:
        self.requested_times.append(float(requested))
        return self._fed.request_time_async(requested)

    def request_time_complete(self) -> float:
        granted = float(self._fed.request_time_complete())
        self.granted_times.append(granted)
        return granted

    def register_publication(self, key: str, type_: str) -> Any:
        return self._fed.register_publication(key, type_)

    def register_global_publication(self, key: str, type_: str) -> Any:
        return self._fed.register_global_publication(key, type_)

    def register_subscription(self, key: str, type_: str) -> Any:
        return self._fed.register_subscription(key, type_)

    def set_flag_option(self, flag: int, enabled: bool) -> None:
        self._fed.set_flag_option(flag, enabled)

    def disconnect(self) -> None:
        self.disconnect_called = True
        self._fed.disconnect()

    def disconnect_async(self) -> Any:
        self.disconnect_called = True
        return self._fed.disconnect_async()

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

    def step(self) -> StepResult:
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

    def step(self) -> StepResult:
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


class _FaultyAggregator:
    """Mock aggregator federate that triggers a HELICS-level error at a configurable step.

    The aggregator sets ``HELICS_FLAG_TERMINATE_ON_ERROR`` and signals its
    fault via ``helicsFederateLocalError`` — exercising the same
    broker-termination path that the production ``HELICSDwelling`` and
    ``HELICSFleet`` guard against.  When the aggregator fails, the surviving
    federate must receive ``HELICS_TIME_MAXTIME`` on its next
    ``request_time()`` call rather than hanging indefinitely.
    """

    def __init__(
        self,
        federation: _Federation,
        fed_name: str,
        fail_step: int = 5,
        total_steps: int = _TOTAL_STEPS,
    ) -> None:
        self._federation = federation
        self._fed_name = fed_name
        self._fail_step = fail_step
        self._total_steps = total_steps
        self._step_count = 0
        self._fed: Any = None
        self.granted_times: list[float] = []

    def run(self) -> None:
        fedinfo = self._federation.federate_info(self._fed_name)
        self._fed = helics.helicsCreateValueFederate(self._fed_name, fedinfo)
        helics.helicsFederateSetFlagOption(
            self._fed, helics.HELICS_FLAG_TERMINATE_ON_ERROR, 1
        )
        _pub_voltage = self._fed.register_global_publication("grid/voltage", "double")
        _sub_power = self._fed.register_subscription("house_1/total_power_kw", "double")

        try:
            _enter_exec(self._fed)
            for step_idx in range(self._total_steps):
                _pub_voltage.publish(1.0)
                granted = _request(self._fed, (step_idx + 1) * _TIME_RES_S)
                self.granted_times.append(float(granted))
                self._step_count += 1
                if self._step_count >= self._fail_step:
                    raise RuntimeError(
                        "synthetic aggregator failure at step {}".format(
                            self._fail_step
                        )
                    )
        except RuntimeError:
            helics.helicsFederateLocalError(
                self._fed,
                -1,
                "synthetic aggregator failure at step {}".format(self._fail_step),
            )
            raise
        finally:
            if self._fed is not None:
                try:
                    self._fed.disconnect()
                except Exception:
                    pass


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
        write_output=False,
    )
    dwelling.initialize()
    return dwelling


def _new_fleet(n_dwellings: int = 3) -> SteppableFleet:
    sim_config = SimulationConfig(duration_s=_DURATION_S, time_res_s=int(_TIME_RES_S), write_output=False)
    configs = [
        DwellingConfig(
            hpxml=HPXML,
            schedule=SCHEDULE,
            weather=WEATHER,
            config=sim_config,
            bldg_id=i + 1,
            defaults_path=str(HARES_DEFAULTS),
        )
        for i in range(n_dwellings)
    ]
    return SteppableFleet.from_configs(configs, n_threads=0)


def _enter_exec(fed: Any) -> None:
    """Enter executing mode with the short test connect timeout."""
    enter_executing_mode_with_timeout(fed, _CONNECT_TIMEOUT_S, fed_name="test-aggregator")


def _request(fed: Any, requested_time_s: float) -> float:
    """Request a time grant with the short test grant timeout."""
    return request_time_with_timeout(fed, requested_time_s, _GRANT_TIMEOUT_S, fed_name="test-aggregator")


def _run_single_dwelling_exchange(voltage_pu: float) -> list[float]:
    with _Federation(n_federates=2) as federation:
        dwelling = _new_dwelling()
        federate_ready = threading.Event()

        def _run_federate() -> None:
            helics_dwelling = federation.helics_dwelling(dwelling, "house_1")
            helics_dwelling.register_publications()
            helics_dwelling.register_subscriptions(voltage_topic="grid/voltage")
            federate_ready.set()
            helics_dwelling.run()

        thread = federation.start(_run_federate, name="dwelling-federate")
        federate_ready.wait()

        aggregator = federation.value_federate("aggregator_1")
        sub_power = aggregator.register_subscription("house_1/total_power_kw", "double")
        pub_voltage = aggregator.register_global_publication("grid/voltage", "double")

        power_trace: list[float] = []
        _enter_exec(aggregator)
        for step_idx in range(_TOTAL_STEPS):
            pub_voltage.publish(float(voltage_pu))
            _request(aggregator, step_idx * _TIME_RES_S)
            if sub_power.is_updated():
                power_trace.append(float(sub_power.double))
        aggregator.disconnect()

        thread.result()
        return power_trace


def _avg(values: list[float]) -> float:
    if not values:
        return 0.0
    return sum(values) / len(values)


def _clear_helics_modules() -> None:
    sys.modules.pop("ochre_next.helics", None)
    sys.modules.pop("ochre_next.helics.broker", None)
    sys.modules.pop("ochre_next.helics.dwelling", None)
    sys.modules.pop("ochre_next.helics.federate", None)
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
    with _Federation(n_federates=2) as federation:
        fleet = _new_fleet(n_dwellings=3)
        federate_ready = threading.Event()

        def _run_federate() -> None:
            helics_fleet = federation.helics_fleet(fleet, "fleet_1")
            helics_fleet.register_publications()
            helics_fleet.register_subscriptions(voltage_topic="grid/voltage")
            federate_ready.set()
            helics_fleet.run()

        thread = federation.start(_run_federate, name="fleet-federate")
        federate_ready.wait()

        aggregator = federation.value_federate("aggregator_1")
        sub_aggregate = aggregator.register_subscription("fleet_1/aggregate_power_kw", "double")
        sub_dwelling = [
            aggregator.register_subscription(f"fleet_1/dwelling_{idx}/total_power_kw", "double")
            for idx in range(3)
        ]
        pub_voltage = aggregator.register_global_publication("grid/voltage", "double")

        aggregate_samples: list[tuple[float, float]] = []
        _enter_exec(aggregator)
        for step_idx in range(_TOTAL_STEPS):
            pub_voltage.publish(0.98)
            _request(aggregator, step_idx * _TIME_RES_S)

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
        aggregator.disconnect()

        thread.result()
        assert aggregate_samples, "aggregator should receive aggregate and per-dwelling updates"

        for aggregate_kw, summed_kw in aggregate_samples:
            assert aggregate_kw == pytest.approx(summed_kw, rel=1e-6, abs=1e-6)


def test_control_signal_via_helics() -> None:
    with _Federation(n_federates=2) as federation:
        baseline = _RecordingDwelling(_new_dwelling())
        baseline._dwelling.add_battery(
            Battery("Battery", 13.5, max_charge_kw=5.0, max_discharge_kw=5.0, initial_soc=0.4)
        )
        baseline_ready = threading.Event()

        def _run_federate() -> None:
            baseline_orchestrator = federation.helics_dwelling(baseline, "house_base")
            baseline_orchestrator.register_publications()
            baseline_orchestrator.register_subscriptions(control_topic="grid/control")
            baseline_ready.set()
            baseline_orchestrator.run()

        baseline_thread = federation.start(_run_federate, name="dwelling-baseline")
        baseline_ready.wait()

        aggregator = federation.value_federate("aggregator_base")
        pub_control = aggregator.register_global_publication("grid/control", "string")
        _enter_exec(aggregator)
        for step_idx in range(_TOTAL_STEPS):
            _request(aggregator, step_idx * _TIME_RES_S)
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
        aggregator.disconnect()

        baseline_thread.result()
        assert baseline.applied_controls == ["Battery"]
        assert baseline.battery_power_trace_kw, "battery telemetry trace should be captured"
        controlled_power = abs(baseline.battery_power_trace_kw[-1])
        assert controlled_power > 0.1, "battery should show non-trivial power after control injection"


def test_helics_time_domain_is_simulation_relative() -> None:
    with _Federation(n_federates=1) as federation:
        dwelling = _new_dwelling()
        helics_dwelling = federation.helics_dwelling(dwelling, "house_1")
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


def test_federate_cleanup_on_exception() -> None:
    with _Federation(n_federates=1) as federation:
        faulty_dwelling = _FaultyDwelling(_new_dwelling(), fail_step=3)
        helics_dwelling = federation.helics_dwelling(faulty_dwelling, "house_1")
        helics_dwelling.register_publications()

        probe = _FederateProbe(helics_dwelling._fed)
        helics_dwelling._fed = probe

        with pytest.raises(RuntimeError, match="synthetic dwelling step failure"):
            helics_dwelling.run()

        assert probe.disconnect_called is True


def test_malformed_control_payload_does_not_crash_federate(
    caplog: pytest.LogCaptureFixture,
) -> None:
    with _Federation(n_federates=2) as federation:
        dwelling = _new_dwelling()
        federate_ready = threading.Event()

        def _run_federate() -> None:
            helics_dwelling = federation.helics_dwelling(dwelling, "house_1")
            helics_dwelling.register_publications()
            helics_dwelling.register_subscriptions(control_topic="grid/control")
            federate_ready.set()
            helics_dwelling.run()

        thread = federation.start(_run_federate, name="dwelling-malformed-control")
        federate_ready.wait()

        aggregator = federation.value_federate("aggregator_1")
        sub_power = aggregator.register_subscription("house_1/total_power_kw", "double")
        pub_control = aggregator.register_global_publication("grid/control", "string")

        with caplog.at_level("WARNING"):
            _enter_exec(aggregator)
            payloads = [
                "not json",
                json.dumps({"unknown_equipment": {"type": "PowerSetpoint", "active_power_kw": 3.0}}),
                json.dumps({"equipment": "Battery", "signal": {"type": "InvalidType"}}),
            ]
            for step_idx in range(_TOTAL_STEPS):
                _request(aggregator, step_idx * _TIME_RES_S)
                if step_idx < len(payloads):
                    pub_control.publish(payloads[step_idx])
            aggregator.disconnect()

        thread.result()
        assert "Invalid control payload JSON" in caplog.text or "Failed to apply control" in caplog.text
        assert sub_power.is_updated() or "publish" in caplog.text


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


def test_stale_broker_raises_timeout_instead_of_hanging() -> None:
    """A broker waiting for federates that never join must not hang the federate.

    Regression test for the stale-broker incident: a leftover broker process
    holds federation state, and any federate joining it blocks forever at
    ``enter_executing_mode`` with no diagnostics.  With connect timeouts the
    federate must raise a clear ``TimeoutError`` within seconds instead.
    """
    # Broker expects 2 federates; only 1 ever joins, so executing mode is
    # never reached: exactly the stale-broker hang signature. Raising the
    # TimeoutError at all is the proof: a hang never returns.
    with _Federation(n_federates=2) as federation:
        dwelling = _new_dwelling()
        helics_dwelling = federation.helics_dwelling(dwelling, "house_stale_broker", connect_timeout_s=2.0)
        helics_dwelling.register_publications()

        with pytest.raises(TimeoutError, match="did not enter executing mode within 2.0s"):
            helics_dwelling.run()


def test_multi_rate_dwelling_steps_only_at_own_period() -> None:
    """Dwelling at 60s timestep, aggregator at 10s. Verify dwelling only steps at 60s boundaries."""
    DWELL_TIME_RES_S = 60
    AGG_TIME_RES_S = 10
    DWELL_STEPS = 3
    DWELL_DURATION_S = DWELL_STEPS * DWELL_TIME_RES_S  # 180
    AGG_STEPS = DWELL_DURATION_S // AGG_TIME_RES_S  # 18

    with _Federation(n_federates=2) as federation:
        dwelling_raw = Dwelling.from_hpxml(
            HPXML, SCHEDULE, WEATHER,
            start_time="2019-01-01T00:00:00",
            duration_s=DWELL_DURATION_S,
            time_res_s=DWELL_TIME_RES_S,
            defaults_path=str(HARES_DEFAULTS),
            bldg_id=99,
            master_seed=0,
            output_verbosity=0,
            write_output=False,
        )
        dwelling_raw.initialize()
        recording = _RecordingDwelling(dwelling_raw)
        federate_ready = threading.Event()

        def _run_federate() -> None:
            helics_dwelling = federation.helics_dwelling(recording, "house_mr")
            helics_dwelling.register_publications()
            helics_dwelling.register_subscriptions(voltage_topic="grid/voltage")
            federate_ready.set()
            helics_dwelling.run()

        thread = federation.start(_run_federate, name="dwelling-mr")
        federate_ready.wait()

        aggregator = federation.value_federate("agg_mr", time_res_s=float(AGG_TIME_RES_S))
        sub_power = aggregator.register_subscription("house_mr/total_power_kw", "double")
        pub_voltage = aggregator.register_global_publication("grid/voltage", "double")

        power_trace: list[float] = []
        _enter_exec(aggregator)
        # request_time(t) asks to be granted time t; since the federate
        # is already at time 0 after enter_executing_mode(), the first
        # request must target the exit time of the first interval
        # (AGG_TIME_RES_S), not entry time 0.
        for step_idx in range(1, AGG_STEPS + 1):
            pub_voltage.publish(1.0)
            _request(aggregator, step_idx * AGG_TIME_RES_S)
            if sub_power.is_updated():
                power_trace.append(float(sub_power.double))
        aggregator.disconnect()

        thread.result()

        # Dwelling publishes its current state at every HELICS grant (every 10s),
        # but its model state only advances at 60s boundaries.
        assert len(power_trace) == AGG_STEPS, (
            f"Expected {AGG_STEPS} publications, got {len(power_trace)}"
        )
        assert any(abs(p) > 1e-6 for p in power_trace), "published dwelling power should be non-zero"


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
    with _Federation(n_federates=2) as federation:
        dwelling = _new_dwelling()
        federate_ready = threading.Event()
        probe_times: list[float] = []

        def _run_federate() -> None:
            helics_dwelling = federation.helics_dwelling(dwelling, "house_1")
            helics_dwelling.register_publications()
            helics_dwelling.register_subscriptions(voltage_topic="grid/voltage")

            probe = _FederateProbe(helics_dwelling._fed)
            helics_dwelling._fed = probe

            federate_ready.set()
            helics_dwelling.run()

            probe_times.extend(probe.requested_times)

        thread = federation.start(_run_federate, name="dwelling-federate")
        federate_ready.wait()

        aggregator = federation.value_federate("aggregator_1")
        sub_power = aggregator.register_subscription("house_1/total_power_kw", "double")
        pub_voltage = aggregator.register_global_publication("grid/voltage", "double")

        _enter_exec(aggregator)
        for step_idx in range(_TOTAL_STEPS):
            pub_voltage.publish(1.0)
            _request(aggregator, step_idx * _TIME_RES_S)
            if sub_power.is_updated():
                _ = float(sub_power.double)

        # Disconnect aggregator FIRST so the dwelling's blocked
        # request_time(HELICS_TIME_MAXTIME) is granted after the last
        # remaining peer leaves the federation.
        aggregator.disconnect()

        thread.result()

        assert len(probe_times) > 0, "dwelling made no request_time calls"
        assert probe_times[-1] == helics.HELICS_TIME_MAXTIME, (
            f"Expected final request_time to be HELICS_TIME_MAXTIME; got {probe_times[-1]}"
        )


def test_fleet_completion_signal_unblocks_aggregator() -> None:
    """After fleet signals completion, the co-simulation completes cleanly.

    Mirrors ``test_single_dwelling_completion_signal_unblocks_aggregator``
    for the fleet federate — uses a ``_FederateProbe`` to verify
    ``request_time(HELICS_TIME_MAXTIME)`` is the final call in a 2-federate
    co-simulation.
    """
    with _Federation(n_federates=2) as federation:
        fleet = _new_fleet(n_dwellings=3)
        federate_ready = threading.Event()
        probe_times: list[float] = []

        def _run_federate() -> None:
            helics_fleet = federation.helics_fleet(fleet, "fleet_1")
            helics_fleet.register_publications()
            helics_fleet.register_subscriptions(voltage_topic="grid/voltage")

            probe = _FederateProbe(helics_fleet._fed)
            helics_fleet._fed = probe

            federate_ready.set()
            helics_fleet.run()

            probe_times.extend(probe.requested_times)

        thread = federation.start(_run_federate, name="fleet-federate")
        federate_ready.wait()

        aggregator = federation.value_federate("aggregator_1")
        sub_aggregate = aggregator.register_subscription("fleet_1/aggregate_power_kw", "double")
        _ = [
            aggregator.register_subscription(f"fleet_1/dwelling_{idx}/total_power_kw", "double")
            for idx in range(3)
        ]
        pub_voltage = aggregator.register_global_publication("grid/voltage", "double")

        _enter_exec(aggregator)
        for step_idx in range(_TOTAL_STEPS):
            pub_voltage.publish(0.98)
            _request(aggregator, step_idx * _TIME_RES_S)
            if sub_aggregate.is_updated():
                _ = float(sub_aggregate.double)

        # Disconnect aggregator FIRST so the fleet's blocked
        # request_time(HELICS_TIME_MAXTIME) is granted after the last
        # remaining peer leaves the federation.
        aggregator.disconnect()

        thread.result()

        assert len(probe_times) > 0, "fleet made no request_time calls"
        assert probe_times[-1] == helics.HELICS_TIME_MAXTIME, (
            f"Expected final request_time to be HELICS_TIME_MAXTIME; got {probe_times[-1]}"
        )


def test_publication_info_via_helics_api_fallback() -> None:
    """_set_publication_info attaches metadata via helicsPublicationSetInfo fallback.

    On HELICS 3.6.1 ``HelicsPublication`` has no ``set_info`` method, so
    ``_set_publication_info`` falls through to ``helics.helicsPublicationSetInfo``.
    This integration test exercises that production code path by retrieving the
    metadata with ``helicsPublicationGetInfo``.
    """
    with _Federation(n_federates=1) as federation:
        dwelling = _new_dwelling()
        helics_dwelling = federation.helics_dwelling(dwelling, "house_meta")
        helics_dwelling.register_publications()

        assert helics_dwelling._pub_power is not None
        assert helics_dwelling._pub_reactive is not None

        power_info = helics.helicsPublicationGetInfo(helics_dwelling._pub_power)
        assert power_info == "units=kW"

        reactive_info = helics.helicsPublicationGetInfo(helics_dwelling._pub_reactive)
        assert reactive_info == "units=kvar"


def test_voltage_in_volts_triggers_out_of_range_warning() -> None:
    """Publish grid voltage in absolute volts (240 V) instead of per-unit.

    An external federate that publishes voltage in volts (e.g. 240) when
    HARES expects per-unit (~0.95) produces a 240× error. The HELICS boundary
    must detect the unit mismatch so the operator or an external caller can
    act on it.
    """
    with _Federation(n_federates=2) as federation:
        dwelling_raw = Dwelling.from_hpxml(
            HPXML, SCHEDULE, WEATHER,
            start_time="2019-01-01T00:00:00",
            duration_s=300,  # 5 steps at 60s
            time_res_s=60,
            defaults_path=str(HARES_DEFAULTS),
            bldg_id=99,
            master_seed=0,
            output_verbosity=0,
            write_output=False,
        )
        dwelling_raw.initialize()
        federate_ready = threading.Event()
        orchestrator_ref: list[HELICSDwelling] = []

        def _run_federate() -> None:
            helics_dwelling = federation.helics_dwelling(dwelling_raw, "house_240v")
            orchestrator_ref.append(helics_dwelling)
            helics_dwelling.register_publications()
            helics_dwelling.register_subscriptions(voltage_topic="grid/voltage")
            federate_ready.set()
            helics_dwelling.run()

        thread = federation.start(_run_federate, name="dwelling-240v")
        federate_ready.wait()

        aggregator = federation.value_federate("aggregator_240v", time_res_s=60.0)
        pub_voltage = aggregator.register_global_publication("grid/voltage", "double")

        _enter_exec(aggregator)
        for step_idx in range(5):
            # Publish 240 V (absolute volts, not per-unit)
            pub_voltage.publish(240.0)
            _request(aggregator, step_idx * 60.0)
        aggregator.disconnect()

        thread.result()

        assert len(orchestrator_ref) == 1
        assert orchestrator_ref[0]._last_voltage_out_of_range is True, (
            "Expected out-of-range voltage detection for 240 V; "
            "the HELICS boundary should detect unit mismatch between volts and per-unit"
        )


def test_zone_temperature_publication_subscribed_by_aggregator() -> None:
    """An external federate subscribes to zone_0/temp_air_c and receives non-zero values.

    Regression test: zone temperature must be published via HELICS so external
    controllers can implement closed-loop HVAC control.
    """
    with _Federation(n_federates=2) as federation:
        dwelling = _new_dwelling()
        federate_ready = threading.Event()

        def _run_federate() -> None:
            helics_dwelling = federation.helics_dwelling(dwelling, "house_zt")
            helics_dwelling.register_publications()
            helics_dwelling.register_subscriptions(voltage_topic="grid/voltage")
            federate_ready.set()
            helics_dwelling.run()

        thread = federation.start(_run_federate, name="dwelling-zt")
        federate_ready.wait()

        aggregator = federation.value_federate("aggregator_zt")
        sub_zone_temp = aggregator.register_subscription("house_zt/zone_0/temp_air_c", "double")
        pub_voltage = aggregator.register_global_publication("grid/voltage", "double")

        zone_temp_trace: list[float] = []
        _enter_exec(aggregator)
        for step_idx in range(_TOTAL_STEPS):
            pub_voltage.publish(1.0)
            _request(aggregator, step_idx * _TIME_RES_S)
            if sub_zone_temp.is_updated():
                zone_temp_trace.append(float(sub_zone_temp.double))
        aggregator.disconnect()

        thread.result()

        assert zone_temp_trace, "aggregator should receive zone temperature publications"
        assert all(abs(t) > 0.1 for t in zone_temp_trace), (
            f"zone temperature should be non-zero (winter outdoor temp); got {zone_temp_trace}"
        )


def test_mismatched_subscription_topic_logs_stale_warning() -> None:
    """Dwelling subscribes to a misspelled topic; stale warning fires.

    The dwelling registers a subscription to ``grid/voltage_typo`` while the
    aggregator publishes to ``grid/voltage``.  After 10 consecutive timesteps
    without an update the dwelling logs a stale-subscription warning and the
    diagnostic row reflects the stale count.
    """
    with _Federation(n_federates=2) as federation:
        dwelling = _new_dwelling(bldg_id=88)
        federate_ready = threading.Event()
        orchestrator_ref: list[HELICSDwelling] = []

        def _run_federate() -> None:
            helics_dwelling = federation.helics_dwelling(dwelling, "house_stale")
            orchestrator_ref.append(helics_dwelling)
            helics_dwelling.register_publications()
            # Subscribe to a topic the aggregator does NOT publish to
            helics_dwelling.register_subscriptions(voltage_topic="grid/voltage_typo")
            federate_ready.set()
            helics_dwelling.run()

        thread = federation.start(_run_federate, name="dwelling-stale")
        federate_ready.wait()

        aggregator = federation.value_federate("aggregator_stale")
        sub_power = aggregator.register_subscription("house_stale/total_power_kw", "double")
        # Aggregator publishes to the correct topic (not to voltage_typo)
        pub_voltage = aggregator.register_global_publication("grid/voltage", "double")

        _enter_exec(aggregator)
        for step_idx in range(_TOTAL_STEPS):
            pub_voltage.publish(1.0)
            _request(aggregator, step_idx * _TIME_RES_S)
            if sub_power.is_updated():
                _ = float(sub_power.double)
        aggregator.disconnect()

        thread.result()

        assert len(orchestrator_ref) == 1
        orch = orchestrator_ref[0]
        # After _TOTAL_STEPS (=10) steps with no data, the stale count should
        # reflect the unresponsive subscription.
        row = orch.get_diagnostic_row()
        stale_count = row["helics_stale_subscription_count"]
        assert stale_count >= 1, (
            "Expected at least one stale subscription after %d steps without data; "
            "got stale_count=%d, update_mask=%d"
            % (_TOTAL_STEPS, row["helics_stale_subscription_count"], row["helics_update_mask"])
        )
        assert row["helics_update_mask"] == 0, (
            "No subscription should have received data; got update_mask=%d"
            % row["helics_update_mask"]
        )


# ---------------------------------------------------------------------------
# Year-scale co-simulation tests (8760 hourly steps)
# ---------------------------------------------------------------------------

_YEAR_TIME_RES_S = 3600.0
_YEAR_TOTAL_STEPS = 8760
_YEAR_DURATION_S = _YEAR_TOTAL_STEPS * int(_YEAR_TIME_RES_S)
_YEAR_DRIFT_TOLERANCE_S = 8.76e-3  # 1e-6 per step × 8760 steps


def _new_year_dwelling(bldg_id: int = 42) -> Dwelling:
    dwelling = Dwelling.from_hpxml(
        HPXML,
        SCHEDULE,
        WEATHER,
        start_time="2019-01-01T00:00:00",
        duration_s=_YEAR_DURATION_S,
        time_res_s=int(_YEAR_TIME_RES_S),
        defaults_path=str(HARES_DEFAULTS),
        bldg_id=bldg_id,
        master_seed=0,
        output_verbosity=0,
        write_output=False,
    )
    dwelling.initialize()
    return dwelling


@pytest.mark.timeout(600, method="thread")
def test_year_scale_time_drift() -> None:
    """A single dwelling federate running 8760 hourly steps accumulates negligible time drift.

    The broker has no peer federates, so every ``request_time()`` is granted
    immediately.  Cumulative drift across the year must stay below
    ``1e-6 * step_count ≈ 8.76e-3`` seconds.
    """
    with _Federation(n_federates=1) as federation:
        dwelling = _new_year_dwelling()
        helics_dwelling = federation.helics_dwelling(
            dwelling, "year_house_1", max_time_drift_tolerance_s=_YEAR_DRIFT_TOLERANCE_S
        )
        helics_dwelling.register_publications()

        probe = _FederateProbe(helics_dwelling._fed)
        helics_dwelling._fed = probe
        helics_dwelling.run()

        step_grants = [t for t in probe.granted_times if t < helics.HELICS_TIME_MAXTIME]
        assert len(step_grants) == _YEAR_TOTAL_STEPS, (
            f"Expected {_YEAR_TOTAL_STEPS} step grants, got {len(step_grants)} "
            f"(total grants: {len(probe.granted_times)})"
        )

        expected = [(step + 1) * _YEAR_TIME_RES_S for step in range(_YEAR_TOTAL_STEPS)]
        total_drift = 0.0
        for step, (granted, exp) in enumerate(zip(step_grants, expected)):
            drift = abs(granted - exp)
            total_drift += drift
            assert drift <= 1e-6 * (step + 1), (
                f"Per-step drift {drift:.12f}s at step {step} exceeds bound {1e-6 * (step + 1):.12f}s"
            )

        assert total_drift < _YEAR_DRIFT_TOLERANCE_S, (
            f"Cumulative drift {total_drift:.12f}s exceeds tolerance {_YEAR_DRIFT_TOLERANCE_S}s"
        )

        internal_drift = helics_dwelling.time_drift_cumulative_s
        assert internal_drift >= 0.0, f"Internal drift {internal_drift} is negative"
        assert internal_drift < _YEAR_DRIFT_TOLERANCE_S, (
            f"Internal drift {internal_drift:.12f}s exceeds tolerance {_YEAR_DRIFT_TOLERANCE_S}s"
        )
        drift_delta = abs(internal_drift - total_drift)
        assert drift_delta <= 1e-9, (
            f"Internal drift {internal_drift:.12f}s disagrees with "
            f"external drift {total_drift:.12f}s by {drift_delta:.12f}s"
        )

        assert helics_dwelling._step_index == _YEAR_TOTAL_STEPS, (
            f"Internal step index {helics_dwelling._step_index} != {_YEAR_TOTAL_STEPS}"
        )


@pytest.mark.timeout(600, method="thread")
def test_year_scale_time_drift_multi_federate() -> None:
    """Two federates (dwelling + aggregator) run 8760 hourly steps with cross-federate time alignment.

    The dwelling federate uses a ``_FederateProbe`` to record granted times.
    The aggregator federate steps in lockstep, publishing a voltage signal.
    Cumulative drift across the year must stay below
    ``1e-6 * step_count ≈ 8.76e-3`` seconds.
    """
    with _Federation(n_federates=2) as federation:
        dwelling = _new_year_dwelling()
        federate_ready = threading.Event()
        shared_probe: list[_FederateProbe] = []
        shared_dwelling: list[HELICSDwelling] = []

        def _run_federate() -> None:
            helics_dwelling = federation.helics_dwelling(
                dwelling, "year_house_2", max_time_drift_tolerance_s=_YEAR_DRIFT_TOLERANCE_S
            )
            helics_dwelling.register_publications()
            helics_dwelling.register_subscriptions(voltage_topic="grid/voltage")

            probe = _FederateProbe(helics_dwelling._fed)
            helics_dwelling._fed = probe
            shared_probe.append(probe)
            shared_dwelling.append(helics_dwelling)

            federate_ready.set()
            helics_dwelling.run()

        thread = federation.start(_run_federate, name="year-dwelling")
        federate_ready.wait()

        aggregator = federation.value_federate("year_agg", time_res_s=_YEAR_TIME_RES_S)
        sub_power = aggregator.register_subscription("year_house_2/total_power_kw", "double")
        pub_voltage = aggregator.register_global_publication("grid/voltage", "double")

        agg_granted_times: list[float] = []
        _enter_exec(aggregator)
        for step_idx in range(_YEAR_TOTAL_STEPS):
            pub_voltage.publish(1.0)
            exit_time_s = (step_idx + 1) * _YEAR_TIME_RES_S
            granted = _request(aggregator, exit_time_s)
            agg_granted_times.append(float(granted))
            if sub_power.is_updated():
                _ = float(sub_power.double)
        aggregator.disconnect()

        thread.result()

        assert len(shared_probe) == 1
        probe = shared_probe[0]
        step_grants_raw = [t for t in probe.granted_times if t < helics.HELICS_TIME_MAXTIME]

        # In multi-federate mode each step may produce multiple grants
        # (intermediate retry + final accepted). Map each grant to the
        # nearest expected step index, keeping only the closest match.
        expected = [(step + 1) * _YEAR_TIME_RES_S for step in range(_YEAR_TOTAL_STEPS)]
        per_step: dict[int, float] = {}
        for g in step_grants_raw:
            step_idx = int(round(g / _YEAR_TIME_RES_S)) - 1
            if 0 <= step_idx < _YEAR_TOTAL_STEPS:
                prev = per_step.get(step_idx)
                if prev is None or abs(g - expected[step_idx]) < abs(prev - expected[step_idx]):
                    per_step[step_idx] = g

        assert len(per_step) == _YEAR_TOTAL_STEPS, (
            f"Expected {_YEAR_TOTAL_STEPS} step-matched grants, got {len(per_step)} "
            f"(raw grants: {len(step_grants_raw)})"
        )
        assert len(agg_granted_times) == _YEAR_TOTAL_STEPS, (
            f"Expected {_YEAR_TOTAL_STEPS} aggregator grants, got {len(agg_granted_times)}"
        )

        # Dwelling drift
        dwelling_drift = 0.0
        for step_idx, granted in sorted(per_step.items()):
            exp = expected[step_idx]
            drift = abs(granted - exp)
            dwelling_drift += drift

        assert dwelling_drift < _YEAR_DRIFT_TOLERANCE_S, (
            f"Dwelling cumulative drift {dwelling_drift:.12f}s exceeds tolerance {_YEAR_DRIFT_TOLERANCE_S}s"
        )

        # Cross-federate time alignment
        for step_idx, dwell_t in sorted(per_step.items()):
            agg_t = agg_granted_times[step_idx]
            delta = abs(agg_t - dwell_t)
            assert delta <= 1e-6, (
                f"Cross-federate time misalignment {delta:.12f}s at step {step_idx}"
            )

        agg_drift = 0.0
        for granted, exp in zip(agg_granted_times, expected):
            agg_drift += abs(float(granted) - exp)
        assert agg_drift < _YEAR_DRIFT_TOLERANCE_S, (
            f"Aggregator cumulative drift {agg_drift:.12f}s exceeds tolerance {_YEAR_DRIFT_TOLERANCE_S}s"
        )

        assert len(shared_dwelling) == 1
        helics_dwelling = shared_dwelling[0]
        internal_drift = helics_dwelling.time_drift_cumulative_s
        assert internal_drift >= 0.0, f"Internal drift {internal_drift} is negative"
        assert internal_drift < _YEAR_DRIFT_TOLERANCE_S, (
            f"Internal drift {internal_drift:.12f}s exceeds tolerance {_YEAR_DRIFT_TOLERANCE_S}s"
        )
        drift_delta = abs(internal_drift - dwelling_drift)
        assert drift_delta <= 1e-9, (
            f"Internal drift {internal_drift:.12f}s disagrees with "
            f"external drift {dwelling_drift:.12f}s by {drift_delta:.12f}s"
        )

        assert helics_dwelling._step_index == _YEAR_TOTAL_STEPS, (
            f"Internal step index {helics_dwelling._step_index} != {_YEAR_TOTAL_STEPS}"
        )


@pytest.mark.timeout(120, method="thread")
def test_drift_guard_raises_when_cumulative_drift_exceeds_tolerance() -> None:
    """A dwelling federate with a negative drift tolerance raises ``RuntimeError``.

    The internal drift accumulator is initialised to 0.0 and grows upward with
    each non-negative per-step drift. Passing ``max_time_drift_tolerance_s=-1.0``
    — a tolerance no reachable cumulative drift satisfies — forces the guard to
    fire on the first step, proving the ``RuntimeError`` path is reachable and
    that the exception message names ``drift``.

    A modest step count (10) keeps the test fast; only the first step is needed.
    """
    _STEPS = 10
    _TIME_RES = 3600.0
    _DURATION = _STEPS * int(_TIME_RES)

    with _Federation(n_federates=1) as federation:
        dwelling = Dwelling.from_hpxml(
            HPXML,
            SCHEDULE,
            WEATHER,
            start_time="2019-01-01T00:00:00",
            duration_s=_DURATION,
            time_res_s=int(_TIME_RES),
            defaults_path=str(HARES_DEFAULTS),
            bldg_id=99,
            master_seed=0,
            output_verbosity=0,
            write_output=False,
        )
        dwelling.initialize()

        helics_dwelling = federation.helics_dwelling(dwelling, "drift_guard", max_time_drift_tolerance_s=-1.0)
        helics_dwelling.register_publications()

        with pytest.raises(RuntimeError, match="drift"):
            helics_dwelling.run()


def test_multi_federate_fault_propagation() -> None:
    """Surviving federate exits promptly when a peer signals a HELICS-level error.

    A 2-federate federation runs a dwelling and a faulty aggregator in
    separate threads.  The aggregator sets ``HELICS_FLAG_TERMINATE_ON_ERROR``
    and signals its fault via ``helicsFederateLocalError`` at step 5,
    exercising the broker-termination path.  The dwelling's next
    ``request_time()`` must resolve — via HELICS_TIME_MAXTIME
    (broker-detected termination) or a HELICS exception — rather than
    hanging indefinitely.
    """
    with _Federation(n_federates=2) as federation:
        dwelling = _new_dwelling()
        federate_ready = threading.Event()
        probe_ref: list[_FederateProbe] = []

        def _run_dwelling() -> None:
            helics_dwelling = federation.helics_dwelling(dwelling, "house_1")
            helics_dwelling.register_publications()
            helics_dwelling.register_subscriptions(voltage_topic="grid/voltage")

            probe = _FederateProbe(helics_dwelling._fed)
            helics_dwelling._fed = probe
            probe_ref.append(probe)

            federate_ready.set()
            helics_dwelling.run()

        dwelling_result = federation.start(_run_dwelling, name="dwelling-federate")
        federate_ready.wait()

        def _run_aggregator() -> None:
            aggregator = _FaultyAggregator(federation, "aggregator_1", fail_step=5)
            aggregator.run()

        agg_result = federation.start(_run_aggregator, name="aggregator-federate")

        dwelling_result.join()
        agg_result.join()

        # A dwelling that never received the broker's termination grant
        # would end only by its own grant timeout, the hang this test
        # guards against.
        assert not isinstance(dwelling_result.exception, TimeoutError), (
            f"dwelling hung after peer fault: {dwelling_result.exception}"
        )
        assert agg_result.exception is not None, (
            "aggregator should have raised a controlled fault"
        )
        assert "synthetic aggregator failure" in str(agg_result.exception)

        assert len(probe_ref) == 1
        probe = probe_ref[0]
        assert probe.disconnect_called, (
            "dwelling should have called disconnect after fault"
        )

        # When the broker terminates the federation, the surviving federate
        # receives HELICS_TIME_MAXTIME (< actual request for the next step)
        # or the call raises.  Either path proves the federate did not hang.
        if dwelling_result.completed:
            assert len(probe.granted_times) > 0, (
                "dwelling should have received time grants before termination"
            )
            last_grant = probe.granted_times[-1]
            last_request = probe.requested_times[-1]
            terminated = (
                last_grant >= helics.HELICS_TIME_MAXTIME
                or last_grant < last_request
            )
            assert terminated, (
                "surviving federate's last request_time() should indicate "
                "termination; last_request=%.1f last_grant=%.1f"
                % (last_request, last_grant)
            )


def test_multi_federate_fault_propagation_broker_cleanup() -> None:
    """After a HELICS-level fault, the broker port is reusable — resources are fully released.

    Runs a fault scenario (aggregator signals ``helicsFederateLocalError``
    at step 5) over the networked transport, tears down the broker, then
    creates a second broker on the same port and runs a simple dwelling
    through it. A broker that cannot listen there means the first one leaked
    its sockets.
    """
    with _Federation(n_federates=2, core_type="zmq") as federation:
        broker_port = federation.port
        dwelling = _new_dwelling()
        federate_ready = threading.Event()

        def _run_dwelling() -> None:
            helics_dwelling = federation.helics_dwelling(dwelling, "house_1")
            helics_dwelling.register_publications()
            helics_dwelling.register_subscriptions(voltage_topic="grid/voltage")
            federate_ready.set()
            helics_dwelling.run()

        dwelling_result = federation.start(_run_dwelling, name="dwelling-federate")
        federate_ready.wait()

        def _run_aggregator() -> None:
            aggregator = _FaultyAggregator(federation, "aggregator_1", fail_step=5)
            aggregator.run()

        agg_result = federation.start(_run_aggregator, name="aggregator-federate")

        dwelling_result.join()
        agg_result.join()
        assert agg_result.exception is not None
        assert "synthetic aggregator failure" in str(agg_result.exception)

    with _Federation(n_federates=1, core_type="zmq", port=broker_port) as federation:
        dwelling2 = _new_dwelling(bldg_id=99)
        helics_dwelling2 = federation.helics_dwelling(dwelling2, "house_2")
        helics_dwelling2.register_publications()
        helics_dwelling2.run()
