"""Unit tests for timeout-aware HELICS federate lifecycle helpers.

These tests use in-process fakes only — no HELICS networking — so they can
exercise the timeout/abort paths deterministically and quickly.
"""

from __future__ import annotations

import importlib.util
import time
from collections.abc import Mapping
from typing import Any, NoReturn

import pytest

# These tests use in-process fakes (no HELICS networking), so any hang is a
# logic bug; the thread method also catches blocking inside C extensions.
pytestmark = [
    pytest.mark.skipif(
        importlib.util.find_spec("helics") is None, reason="helics not installed"
    ),
    pytest.mark.timeout(60, method="thread"),
]


class _AsyncFakeFederate:
    """Federate double exposing the HELICS async API.

    ``completes_after`` is the number of ``is_async_operation_completed``
    polls before the pending operation reports done; ``None`` never completes.
    """

    def __init__(self, completes_after: int | None = 0, granted: float = 42.0) -> None:
        self._completes_after = completes_after
        self._granted = granted
        self._polls = 0
        self.calls: list[tuple[Any, ...]] = []

    def enter_executing_mode(self) -> None:
        self.calls.append(("enter_executing_mode",))

    def enter_executing_mode_async(self) -> None:
        self.calls.append(("enter_executing_mode_async",))

    def enter_executing_mode_complete(self) -> None:
        self.calls.append(("enter_executing_mode_complete",))

    def request_time(self, requested: float) -> float:
        self.calls.append(("request_time", requested))
        return requested

    def request_time_async(self, requested: float) -> None:
        self.calls.append(("request_time_async", requested))

    def request_time_complete(self) -> float:
        self.calls.append(("request_time_complete",))
        return self._granted

    def is_async_operation_completed(self) -> bool:
        self._polls += 1
        if self._completes_after is None:
            return False
        return self._polls > self._completes_after

    def disconnect_async(self) -> None:
        self.calls.append(("disconnect_async",))


class _BlockingOnlyFederate:
    """Federate double without the async API (like the fakes in sibling tests)."""

    def __init__(self) -> None:
        self.calls: list[tuple[Any, ...]] = []

    def enter_executing_mode(self) -> None:
        self.calls.append(("enter_executing_mode",))

    def request_time(self, requested: float) -> float:
        self.calls.append(("request_time", requested))
        return requested


def test_enter_executing_mode_completes_via_async_api() -> None:
    from ochre_next.helics.federate import enter_executing_mode_with_timeout

    fed = _AsyncFakeFederate(completes_after=2)
    enter_executing_mode_with_timeout(fed, timeout_s=5.0, fed_name="fed_a")

    assert ("enter_executing_mode_async",) in fed.calls
    assert ("enter_executing_mode_complete",) in fed.calls
    assert ("enter_executing_mode",) not in fed.calls
    assert ("disconnect_async",) not in fed.calls


def test_enter_executing_mode_times_out_and_aborts() -> None:
    from ochre_next.helics.federate import enter_executing_mode_with_timeout

    fed = _AsyncFakeFederate(completes_after=None)
    start = time.monotonic()
    with pytest.raises(TimeoutError, match="did not enter executing mode within 0.2s"):
        enter_executing_mode_with_timeout(fed, timeout_s=0.2, fed_name="fed_b")
    elapsed = time.monotonic() - start

    # Load-insensitive wall-clock bound: the assertion's purpose is that
    # the timeout path FIRES rather than blocks — which `pytest.raises`
    # above already proves — not that it fires within a fixed wall-clock
    # budget. Under the suite's default `-n auto` (one xdist worker per
    # core, 28 here) GIL and scheduler contention can stretch a 0.2 s
    # poll loop well past a tight budget, which made this test flake
    # non-deterministically (different tests per run). The generous bound
    # still catches a pathological slow-fire; a true hang is caught by
    # this file's `pytest.mark.timeout(60, method="thread")`.
    assert elapsed < 30.0, "timeout should fire rather than block"
    assert ("disconnect_async",) in fed.calls, "stuck federate must be aborted non-blockingly"
    assert ("enter_executing_mode_complete",) not in fed.calls


def test_request_time_completes_via_async_api() -> None:
    from ochre_next.helics.federate import request_time_with_timeout

    fed = _AsyncFakeFederate(completes_after=1, granted=120.0)
    granted = request_time_with_timeout(fed, 120.0, timeout_s=5.0, fed_name="fed_c")

    assert granted == pytest.approx(120.0)
    assert ("request_time_async", 120.0) in fed.calls
    assert ("request_time_complete",) in fed.calls
    assert ("disconnect_async",) not in fed.calls


def test_request_time_times_out_and_aborts() -> None:
    from ochre_next.helics.federate import request_time_with_timeout

    fed = _AsyncFakeFederate(completes_after=None)
    with pytest.raises(TimeoutError, match="was not granted time 60.0s within 0.2s"):
        request_time_with_timeout(fed, 60.0, timeout_s=0.2, fed_name="fed_d")

    assert ("disconnect_async",) in fed.calls, "stuck federate must be aborted non-blockingly"
    assert ("request_time_complete",) not in fed.calls


def test_blocking_fallback_without_async_api() -> None:
    from ochre_next.helics.federate import (
        enter_executing_mode_with_timeout,
        request_time_with_timeout,
    )

    fed = _BlockingOnlyFederate()
    enter_executing_mode_with_timeout(fed, timeout_s=1.0, fed_name="fed_e")
    granted = request_time_with_timeout(fed, 30.0, timeout_s=1.0, fed_name="fed_e")

    assert granted == pytest.approx(30.0)
    assert fed.calls == [("enter_executing_mode",), ("request_time", 30.0)]


def test_wait_for_pending_aborts_tracks_blocked_teardown() -> None:
    """A teardown that blocks inside HELICS is tracked until it returns.

    ``helicsCloseLibrary()`` must never run concurrently with an in-flight
    HELICS call, so ``wait_for_pending_aborts`` has to report the blocked
    teardown until it finishes.
    """
    import threading

    from ochre_next.helics.federate import (
        enter_executing_mode_with_timeout,
        wait_for_pending_aborts,
    )

    release = threading.Event()

    class _BlockedTeardownFederate(_AsyncFakeFederate):
        def disconnect_async(self) -> None:
            release.wait(timeout=30.0)
            super().disconnect_async()

    fed = _BlockedTeardownFederate(completes_after=None)
    try:
        with pytest.raises(TimeoutError):
            enter_executing_mode_with_timeout(fed, timeout_s=0.1, fed_name="fed_f")

        assert wait_for_pending_aborts(timeout_s=0.2) is False, (
            "a teardown still blocked inside HELICS must be reported as pending"
        )
    finally:
        release.set()

    assert wait_for_pending_aborts(timeout_s=10.0) is True
    assert ("disconnect_async",) in fed.calls


def test_core_init_timeout_option_renders_milliseconds() -> None:
    from ochre_next.helics.federate import core_init_timeout_option

    assert core_init_timeout_option(2.5) == "--timeout=2500ms"
    assert core_init_timeout_option(0.0004) == "--timeout=1ms"


def test_core_init_string_names_an_in_process_broker_without_opening_a_port() -> None:
    from ochre_next.helics.federate import core_init_string

    assert core_init_string("inproc", "federation_a", 2.0) == "--broker=federation_a --timeout=2000ms"


def test_core_init_string_gives_a_network_core_the_broker_url_and_a_local_port() -> None:
    from ochre_next.helics.federate import core_init_string

    init = core_init_string("zmq", "10.0.0.7:23404", 2.0).split()

    assert init[0] == "--broker_address=tcp://10.0.0.7:23404"
    assert init[1].startswith("--port=")
    assert init[2] == "--timeout=2000ms"
    assert core_init_string("zmq", "tcp://host:1", 2.0).startswith("--broker_address=tcp://host:1 ")


@pytest.mark.parametrize("bad", [0.0, -1.0, float("nan"), float("inf")])
def test_validate_timeout_rejects_non_positive_and_non_finite(bad: float) -> None:
    from ochre_next.helics.federate import validate_timeout

    with pytest.raises(ValueError):
        validate_timeout(bad, "timeout_s")


def test_validate_timeout_rejects_non_numeric() -> None:
    from ochre_next.helics.federate import validate_timeout

    not_a_number: Any = "soon"
    with pytest.raises(TypeError):
        validate_timeout(not_a_number, "timeout_s")


INVALID = "[-3] core object is not valid"
RELEASE_STEPS = ("disconnect", "wait", "free")
ALL_INVALID: dict[str, str] = dict.fromkeys(RELEASE_STEPS, INVALID)
_TAKEN_PORTS = frozenset({30000, 30010, 30020, 30030})
_FREE_PORT = 31000


class _FakeCores:
    """Stands in for the HELICS core calls ``create_value_federate`` makes.

    ``connects`` gives each created core's connect outcome in order: a bool to
    return or a message to raise as a ``HelicsException``. ``releases`` maps a
    release step to the message it raises, and ``register`` is what
    registering the federate raises.
    """

    def __init__(
        self,
        connects: list[bool | str],
        releases: Mapping[str, str] | None = None,
        register: BaseException | None = None,
    ) -> None:
        self._connects = iter(connects)
        self._releases = releases or {}
        self._register = register
        self.created: list[str] = []
        self.released: list[tuple[str, str]] = []

    def install(self, monkeypatch: pytest.MonkeyPatch, module: Any) -> None:
        for name in (
            "helicsCreateCore",
            "helicsCoreConnect",
            "helicsCoreDisconnect",
            "helicsCoreWaitForDisconnect",
            "helicsCoreFree",
            "helicsCreateValueFederate",
        ):
            monkeypatch.setattr(module.helics, name, getattr(self, name))

    def _raise(self, message: str) -> NoReturn:
        import helics

        raise helics.HelicsException(message)

    def helicsCreateCore(self, core_type: str, name: str, init: str) -> str:
        self.created.append(name)
        return name

    def helicsCoreConnect(self, core: str) -> bool:
        outcome = next(self._connects)
        if isinstance(outcome, str):
            self._raise(outcome)
        return outcome

    def _release_step(self, step: str, core: str) -> None:
        self.released.append((step, core))
        if step in self._releases:
            self._raise(self._releases[step])

    def helicsCoreDisconnect(self, core: str) -> None:
        self._release_step("disconnect", core)

    def helicsCoreWaitForDisconnect(self, core: str, timeout_ms: int) -> bool:
        self._release_step("wait", core)
        return True

    def helicsCoreFree(self, core: str) -> None:
        self._release_step("free", core)

    def helicsCreateValueFederate(self, name: str, info: object) -> str:
        if self._register is not None:
            raise self._register
        return f"federate:{name}"


@pytest.fixture
def federate_module(monkeypatch: pytest.MonkeyPatch) -> Any:
    from ochre_next.helics import federate

    monkeypatch.setattr(federate, "_port_pair_taken", lambda port: port in _TAKEN_PORTS)
    return federate


def _create(module: Any, monkeypatch: pytest.MonkeyPatch, cores: _FakeCores, ports: list[int]) -> Any:
    cores.install(monkeypatch, module)
    draws = iter(ports)
    monkeypatch.setattr(module, "allocate_ephemeral_port", lambda: next(draws))
    return module.create_value_federate("house_1", "zmq", "127.0.0.1:23404", 2.0, lambda name, init: (name, init))


def _released_in_full(cores: _FakeCores, created: list[str]) -> bool:
    return cores.released == [(step, core) for core in created for step in RELEASE_STEPS]


def _fully_released(cores: _FakeCores) -> bool:
    return _released_in_full(cores, cores.created)


def test_a_core_invalidated_by_its_taken_port_is_freed_and_retried_on_a_fresh_port(
    federate_module: Any, monkeypatch: pytest.MonkeyPatch
) -> None:
    cores = _FakeCores(connects=[INVALID, True], releases=ALL_INVALID)

    fed = _create(federate_module, monkeypatch, cores, [30000, _FREE_PORT])

    assert fed == "federate:house_1"
    assert len(cores.created) == 2
    assert _released_in_full(cores, cores.created[:1])


@pytest.mark.parametrize("step", RELEASE_STEPS)
def test_an_invalidated_core_at_any_release_step_does_not_stop_the_release(
    step: str, federate_module: Any, monkeypatch: pytest.MonkeyPatch
) -> None:
    cores = _FakeCores(connects=[False, True], releases={step: INVALID})

    fed = _create(federate_module, monkeypatch, cores, [30000, _FREE_PORT])

    assert fed == "federate:house_1"
    assert _released_in_full(cores, cores.created[:1])


def test_cores_invalidated_on_every_taken_port_surface_the_bind_failure(
    federate_module: Any, monkeypatch: pytest.MonkeyPatch
) -> None:
    attempts = federate_module._CORE_PORT_ATTEMPTS
    cores = _FakeCores(connects=[INVALID] * attempts, releases=ALL_INVALID)

    with pytest.raises(ConnectionError, match="could not bind a core port"):
        _create(federate_module, monkeypatch, cores, sorted(_TAKEN_PORTS)[:attempts])

    assert len(cores.created) == attempts
    assert _fully_released(cores)


def test_a_core_invalidated_on_a_free_port_surfaces_the_connect_failure(
    federate_module: Any, monkeypatch: pytest.MonkeyPatch
) -> None:
    import helics

    cores = _FakeCores(connects=[INVALID], releases=ALL_INVALID)

    with pytest.raises(ConnectionError, match="could not connect") as raised:
        _create(federate_module, monkeypatch, cores, [_FREE_PORT])

    assert isinstance(raised.value.__cause__, helics.HelicsException)
    assert str(raised.value.__cause__) == INVALID
    assert _fully_released(cores)


@pytest.mark.parametrize("connect", [False, "[-2] connection failure"], ids=["returned-false", "raised"])
def test_a_release_error_is_reported_with_the_connect_failure_it_followed(
    connect: bool | str, federate_module: Any, monkeypatch: pytest.MonkeyPatch
) -> None:
    cores = _FakeCores(connects=[connect], releases={"disconnect": "[-1] disconnect failure"})

    with pytest.raises(ConnectionError, match=r"could not connect.*releasing it failed: \[-1\]") as raised:
        _create(federate_module, monkeypatch, cores, [30000])

    cause = str(raised.value.__cause__)
    assert cause == (connect if isinstance(connect, str) else "[-1] disconnect failure")
    assert len(cores.created) == 1
    assert _fully_released(cores)


@pytest.mark.parametrize(
    "helics_error",
    ["[-4] duplicate federate name", INVALID, None],
    ids=["duplicate-name", "invalidated", "interrupt"],
)
def test_a_release_error_does_not_mask_the_registration_failure(
    helics_error: str | None, federate_module: Any, monkeypatch: pytest.MonkeyPatch
) -> None:
    import helics

    failure = KeyboardInterrupt() if helics_error is None else helics.HelicsException(helics_error)
    cores = _FakeCores(connects=[True], releases={"disconnect": "[-1] disconnect failure"}, register=failure)

    with pytest.raises(type(failure)) as raised:
        _create(federate_module, monkeypatch, cores, [_FREE_PORT])

    assert raised.value is failure
    assert any("[-1] disconnect failure" in note for note in raised.value.__notes__)
    assert _fully_released(cores)
