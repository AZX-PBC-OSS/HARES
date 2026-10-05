"""Timeout-aware HELICS federate lifecycle helpers.

HELICS blocking calls have no client-side deadline: ``enter_executing_mode()``
waits until every expected federate joins the broker, and ``request_time()``
waits until the broker grants the requested time.  A stale broker (for
example, one left behind by a crashed co-simulation) therefore turns either
call into an infinite, diagnostic-free hang.

These helpers bound every blocking federate call with a wall-clock deadline
using the HELICS async API (``enterExecutingModeAsync`` /
``requestTimeAsync`` + ``isAsyncOperationCompleted``).  On timeout the
federate is torn down best-effort on a daemon thread (see
:func:`_abort_federate` for why no teardown call can be trusted to return)
and a ``TimeoutError`` with an actionable message is raised.

Federate *registration* (``helicsCreateValueFederate``) is bounded separately
via the core init-string ``--timeout`` option; see
:func:`core_init_timeout_option`.
"""

from __future__ import annotations

from collections.abc import Callable
import logging
import math
import socket
import threading
import time
from typing import Any
from urllib.parse import urlsplit

try:
    import helics
except ImportError as exc:  # pragma: no cover - exercised via import test
    raise ImportError(
        "HELICS not installed. Install with: pip install 'ochre_next[helics]'"
    ) from exc

from ._types import HelicsFederateInfoLike, HelicsFederateLike
from .broker import allocate_ephemeral_port

_LOG = logging.getLogger(__name__)

# Core types whose broker lives in the same process and is addressed by name.
_IN_PROCESS_CORE_TYPES = frozenset({"inproc", "test"})

# Cores a network federate tries before its registration failure is raised:
# each failure costs up to the connect timeout, and a fresh draw from the
# port range collides with a racing process only rarely.
_CORE_PORT_ATTEMPTS = 4

# The HELICS zmq broker port a bare host in ``broker_address`` means.
_DEFAULT_BROKER_PORT = 23404
_BROKER_PROBE_TIMEOUT_S = 2.0

# Default wall-clock budget for broker registration plus entering executing
# mode.  Large enough for a slow federation to assemble, small enough that a
# stale broker surfaces as a clear error instead of an open-ended hang.
DEFAULT_CONNECT_TIMEOUT_S = 30.0

# Default wall-clock budget for a single HELICS time grant.  A healthy broker
# detects a dead peer within its own tick timeout (30-120s), so a grant that
# takes longer than this indicates a stalled federation.
DEFAULT_GRANT_TIMEOUT_S = 300.0

_ASYNC_POLL_INTERVAL_S = 0.005

# How long to wait for the best-effort teardown of a stuck federate before
# leaving it to finish on its daemon thread.
_ABORT_JOIN_TIMEOUT_S = 2.0

__all__ = [
    "DEFAULT_CONNECT_TIMEOUT_S",
    "DEFAULT_GRANT_TIMEOUT_S",
    "core_init_string",
    "core_init_timeout_option",
    "create_value_federate",
    "enter_executing_mode_with_timeout",
    "request_time_with_timeout",
    "validate_timeout",
    "wait_for_pending_aborts",
]

# Teardown threads spawned by _abort_federate that have not yet finished
# their HELICS call.  helicsCloseLibrary() while one is in flight is a
# use-after-free; call wait_for_pending_aborts() first.
_PENDING_ABORT_THREADS: set[threading.Thread] = set()
_PENDING_ABORT_LOCK = threading.Lock()


def wait_for_pending_aborts(timeout_s: float = 10.0) -> bool:
    """Wait for background federate teardowns to leave the HELICS library.

    A federate aborted on timeout is torn down on a daemon thread that can
    itself stay blocked inside HELICS until the offending broker goes away.
    Call this after disconnecting brokers (which unblocks those teardowns)
    and *before* process-global cleanup such as ``helicsCloseLibrary()``,
    which must not run concurrently with any HELICS call.

    Returns:
        ``True`` when no teardown remains in flight, ``False`` on timeout.
    """
    deadline = time.monotonic() + validate_timeout(timeout_s, "timeout_s")
    with _PENDING_ABORT_LOCK:
        threads = list(_PENDING_ABORT_THREADS)
    for thread in threads:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            break
        thread.join(remaining)
    return not any(thread.is_alive() for thread in threads)


def validate_timeout(value: float, name: str) -> float:
    """Validate that ``value`` is a positive, finite timeout in seconds."""
    try:
        result = float(value)
    except (TypeError, ValueError) as exc:
        raise TypeError(f"{name} must be a number of seconds, got {value!r}") from exc
    if not math.isfinite(result) or result <= 0.0:
        raise ValueError(f"{name} must be a positive, finite number of seconds, got {value!r}")
    return result


def core_init_timeout_option(timeout_s: float) -> str:
    """Render the core init-string ``--timeout`` option for ``timeout_s`` seconds.

    The HELICS core ``--timeout`` option bounds broker registration: with it,
    ``helicsCreateValueFederate`` against an unreachable or unresponsive broker
    raises a ``HelicsException`` within the timeout instead of stalling for the
    library default (~30s measured on HELICS 3.6.1).
    """
    timeout_s = validate_timeout(timeout_s, "timeout_s")
    return f"--timeout={max(1, round(timeout_s * 1000.0))}ms"


def core_init_string(core_type: str, broker_address: str, connect_timeout_s: float) -> str:
    """Render a federate core's init string for ``broker_address``.

    An in-process core (``inproc``/``test``) names its broker; ``broker_address``
    is the broker's name and no socket is opened. A network core gets the
    broker's URL (``tcp://`` is added to a bare ``host[:port]``) and its own
    local listen port: without an explicit ``--port`` several auto-named cores
    in one process can collide on the auto-assigned port and deadlock at
    ``enterExecutingMode`` instead of raising a bind error (macOS/arm64,
    HELICS 3.6.1).
    """
    timeout = core_init_timeout_option(connect_timeout_s)
    if core_type in _IN_PROCESS_CORE_TYPES:
        return f"--broker={broker_address} {timeout}"
    address = broker_address if "://" in broker_address else f"tcp://{broker_address}"
    return f"--broker_address={address} --port={allocate_ephemeral_port()} {timeout}"


def create_value_federate(
    fed_name: str,
    core_type: str,
    broker_address: str,
    connect_timeout_s: float,
    federate_info: Callable[[str, str], HelicsFederateInfoLike],
) -> HelicsFederateLike:
    """Create a value federate on a core of its own for ``broker_address``.

    ``federate_info(core_name, core_init)`` builds the federate info for one
    attempt. A network core listens on a port :func:`core_init_string` draws
    and probes, but another process can take the port before HELICS binds
    it, and HELICS then fails registration with the error an unreachable
    broker gives. So a failure while the broker accepts connections is taken
    to be the core's own port: the federate is created again on a fresh port,
    under a fresh core name because the failed core keeps its own, up to
    ``_CORE_PORT_ATTEMPTS`` times. A failure while the broker does not accept
    connections is raised at once.

    Raises:
        HelicsException: Registration failed with the broker unreachable, or
            on every attempt; then its notes name each core init string tried.
    """
    core_name = f"core_{fed_name}"

    def register(attempt: int) -> HelicsFederateLike:
        core_init = core_init_string(core_type, broker_address, connect_timeout_s)
        tried.append(core_init)
        attempt_core_name = core_name if attempt == 0 else f"{core_name}_{attempt}"
        return helics.helicsCreateValueFederate(fed_name, federate_info(attempt_core_name, core_init))

    tried: list[str] = []
    if core_type in _IN_PROCESS_CORE_TYPES:
        return register(0)
    for attempt in range(_CORE_PORT_ATTEMPTS - 1):
        try:
            return register(attempt)
        except helics.HelicsException:
            if not _accepts_connections(broker_address):
                raise
            _LOG.warning(
                "HELICS federate %s could not register with %s; retrying on a fresh core port",
                fed_name,
                tried[-1],
            )
    try:
        return register(_CORE_PORT_ATTEMPTS - 1)
    except helics.HelicsException as exc:
        exc.add_note(
            f"HELICS federate '{fed_name}' failed to register on each of {len(tried)} core "
            f"ports while the broker at {broker_address} accepted connections, so a core "
            f"that could not bind its port is the likely cause: {'; '.join(tried)}"
        )
        raise


def _accepts_connections(broker_address: str) -> bool:
    """Whether a TCP connection to the broker at ``broker_address`` succeeds."""
    url = urlsplit(broker_address if "://" in broker_address else f"tcp://{broker_address}")
    try:
        port = url.port or _DEFAULT_BROKER_PORT
    except ValueError:
        return False
    if url.hostname is None:
        return False
    try:
        with socket.create_connection((url.hostname, port), timeout=_BROKER_PROBE_TIMEOUT_S):
            return True
    except OSError:
        return False


def enter_executing_mode_with_timeout(
    fed: Any,
    timeout_s: float = DEFAULT_CONNECT_TIMEOUT_S,
    fed_name: str = "federate",
) -> None:
    """Enter HELICS executing mode, raising ``TimeoutError`` after ``timeout_s``.

    Uses the async entry API when the federate exposes it; otherwise falls back
    to the blocking call (test doubles without async support cannot hang).

    On timeout the federate is disconnected with a non-blocking
    ``disconnect_async()`` and must not be used afterwards.

    Raises:
        TimeoutError: If executing mode is not reached before the deadline.
    """
    timeout_s = validate_timeout(timeout_s, "timeout_s")
    if not _supports_async(fed):
        fed.enter_executing_mode()
        return

    fed.enter_executing_mode_async()
    if _wait_async_operation(fed, timeout_s):
        fed.enter_executing_mode_complete()
        return

    _abort_federate(fed, fed_name)
    raise TimeoutError(
        f"HELICS federate '{fed_name}' did not enter executing mode within "
        f"{timeout_s:.1f}s. The broker is stale, unreachable, or still waiting "
        f"for federates that never joined (verify the broker's --federates "
        f"count and check for leftover HELICS processes, e.g. `pgrep -fl helics`). "
        f"The federate has been disconnected."
    )


def request_time_with_timeout(
    fed: Any,
    requested_time_s: float,
    timeout_s: float = DEFAULT_GRANT_TIMEOUT_S,
    fed_name: str = "federate",
) -> float:
    """Request HELICS time ``requested_time_s``, raising ``TimeoutError`` on stall.

    Uses the async request API when the federate exposes it; otherwise falls
    back to the blocking call (test doubles without async support cannot hang).

    On timeout the federate is disconnected with a non-blocking
    ``disconnect_async()`` and must not be used afterwards.

    Returns:
        The granted HELICS time in seconds.

    Raises:
        TimeoutError: If no grant arrives before the deadline.
    """
    timeout_s = validate_timeout(timeout_s, "timeout_s")
    if not _supports_async(fed):
        return float(fed.request_time(requested_time_s))

    fed.request_time_async(requested_time_s)
    if _wait_async_operation(fed, timeout_s):
        return float(fed.request_time_complete())

    _abort_federate(fed, fed_name)
    raise TimeoutError(
        f"HELICS federate '{fed_name}' was not granted time {requested_time_s:.1f}s "
        f"within {timeout_s:.1f}s. A peer federate or the broker has likely stalled "
        f"or exited without disconnecting (check for leftover HELICS processes, "
        f"e.g. `pgrep -fl helics`). The federate has been disconnected."
    )


def _supports_async(fed: Any) -> bool:
    return (
        hasattr(fed, "is_async_operation_completed")
        and hasattr(fed, "enter_executing_mode_async")
        and hasattr(fed, "enter_executing_mode_complete")
        and hasattr(fed, "request_time_async")
        and hasattr(fed, "request_time_complete")
    )


def _wait_async_operation(fed: Any, timeout_s: float) -> bool:
    """Poll the pending async operation; return ``True`` when it completed."""
    deadline = time.monotonic() + timeout_s
    while not fed.is_async_operation_completed():
        if time.monotonic() >= deadline:
            return False
        time.sleep(_ASYNC_POLL_INTERVAL_S)
    return True


def _abort_federate(fed: Any, fed_name: str) -> None:
    """Best-effort teardown of a federate stuck in a blocking call.

    ``disconnect_async()`` is the least-blocking teardown available on HELICS
    3.6.1 (``disconnect()``, ``local_error()``, and ``global_error()`` hang or
    stall for ~18s against a stale broker), but even it can block when the
    stuck broker lives in the same process.  No HELICS call can be trusted to
    return here, so the teardown runs on a daemon thread: it either finishes
    promptly or is left to complete in the background once the offending
    broker goes away — the caller regains control either way.
    """

    def _disconnect() -> None:
        try:
            if hasattr(fed, "disconnect_async"):
                fed.disconnect_async()
            elif hasattr(helics, "helicsFederateDisconnectAsync"):
                helics.helicsFederateDisconnectAsync(fed)
        except Exception:
            # Expected when the federation is torn down while this call is
            # still blocked (e.g. "HELICS system failure" after the broker
            # disconnects); the federate is unusable after an abort anyway.
            _LOG.debug(
                "HELICS federate %s: teardown after timeout raised",
                fed_name,
                exc_info=True,
            )
        finally:
            with _PENDING_ABORT_LOCK:
                _PENDING_ABORT_THREADS.discard(threading.current_thread())

    thread = threading.Thread(
        target=_disconnect, name=f"helics-abort-{fed_name}", daemon=True
    )
    with _PENDING_ABORT_LOCK:
        _PENDING_ABORT_THREADS.add(thread)
    thread.start()
    thread.join(_ABORT_JOIN_TIMEOUT_S)
    if thread.is_alive():
        _LOG.warning(
            "HELICS federate %s: teardown after timeout is itself blocked; "
            "leaving it to finish on a background thread",
            fed_name,
        )
