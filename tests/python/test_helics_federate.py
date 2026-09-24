"""Unit tests for timeout-aware HELICS federate lifecycle helpers.

These tests use in-process fakes only — no HELICS networking — so they can
exercise the timeout/abort paths deterministically and quickly.
"""

from __future__ import annotations

import importlib.util
import time
from typing import Any

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

    assert elapsed < 5.0, "timeout should fire promptly"
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


@pytest.mark.parametrize("bad", [0.0, -1.0, float("nan"), float("inf")])
def test_validate_timeout_rejects_non_positive_and_non_finite(bad: float) -> None:
    from ochre_next.helics.federate import validate_timeout

    with pytest.raises(ValueError):
        validate_timeout(bad, "timeout_s")


def test_validate_timeout_rejects_non_numeric() -> None:
    from ochre_next.helics.federate import validate_timeout

    with pytest.raises(TypeError):
        validate_timeout("soon", "timeout_s")  # type: ignore[arg-type]
