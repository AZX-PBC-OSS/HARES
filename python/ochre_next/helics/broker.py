"""HELICS broker lifecycle helpers for multi-federate co-simulation.

These helpers centralize broker startup details used by GridLAB-D + HARES runs.
"""

from __future__ import annotations

from collections.abc import Callable
import os
import random
import socket
import time
from typing import Any
import weakref

try:
    import helics
except ImportError as exc:  # pragma: no cover - exercised via import test
    raise ImportError(
        "HELICS not installed. Install with: pip install 'ochre_next[helics]'"
    ) from exc

_PORT_BY_BROKER: weakref.WeakKeyDictionary[Any, int] = weakref.WeakKeyDictionary()

# Port pairs are drawn from 24200-29999. Below it sit the HELICS default
# ports (23404-23415 zmq, 23500, 23901 udp, 24160 tcp), which a stock broker
# on this host may hold; above it the Kubernetes NodePort range (30000-32767)
# and the default kernel ephemeral ranges (Linux 32768-60999, macOS and
# Windows 49152-65535). Hosts that widen their ephemeral range are covered by
# the bind probe and create_broker's connected check, not by the range.
_PORT_RANGE_START = 24200
_PORT_RANGE_END = 29999
_PORT_PROBE_ATTEMPTS = 128
_BROKER_PORT_ATTEMPTS = 16


def create_broker(
    n_federates: int,
    core_type: str = "zmq",
    port: int | None = None,
) -> helics.HelicsBroker:
    """Create and start a HELICS broker.

    When ``port`` is ``None``, this function reserves an ephemeral local port and
    passes it to HELICS to avoid collisions under parallel test execution.

    Example:
        >>> from ochre_next.helics.runner import FederateConfig, generate_cosim_config
        >>> broker = create_broker(n_federates=2)
        >>> broker_port = get_broker_port(broker)
        >>> config = generate_cosim_config(
        ...     [
        ...         FederateConfig(name="gridlabd", exec_command="gridlabd feeder.glm"),
        ...         FederateConfig(name="house_1", exec_command=f"python run_house.py --broker=localhost:{broker_port}"),
        ...     ]
        ... )
        >>> wait_for_broker(broker)

    Args:
        n_federates: Total number of federates expected to connect.
        core_type: HELICS core transport (for example ``"zmq"`` or ``"tcp"``).
        port: Explicit broker port; when omitted an ephemeral free port is used.

    Returns:
        HELICS broker handle.
    """

    # type() rather than isinstance(): bool is an int subclass, and
    # create_broker(True) would silently mean one federate.
    if type(n_federates) is not int:
        raise TypeError("n_federates must be an int")
    if n_federates <= 0:
        raise ValueError("n_federates must be positive")

    if port is not None:
        return _create_listening_broker(n_federates=n_federates, core_type=core_type, port=port)

    last_error: Exception | None = None
    for _ in range(_BROKER_PORT_ATTEMPTS):
        try:
            return _create_listening_broker(
                n_federates=n_federates,
                core_type=core_type,
                port=allocate_ephemeral_port(),
            )
        except Exception as exc:
            last_error = exc
    raise RuntimeError("Unable to create broker on an ephemeral port") from last_error


def wait_for_broker(broker: helics.HelicsBroker, timeout: float = 60.0) -> None:
    """Block until a HELICS broker reports connected, or raise on timeout.

    Example:
        >>> broker = create_broker(2)
        >>> wait_for_broker(broker, timeout=10.0)
        >>> # Start GridLAB-D and HARES federates only after broker is ready.

    Args:
        broker: HELICS broker handle.
        timeout: Maximum wall-clock seconds to wait for connectivity.

    Raises:
        TimeoutError: If the broker does not report connected before ``timeout``.
    """

    if timeout <= 0:
        raise ValueError("timeout must be positive")

    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if _broker_is_connected(broker):
            return
        time.sleep(0.05)

    raise TimeoutError(f"Broker did not connect within {timeout:.1f}s")


def destroy_broker(broker: helics.HelicsBroker) -> None:
    """Disconnect a HELICS broker and release its resources.

    Call after all federates have disconnected.  This does **not** call
    ``helicsCloseLibrary()`` because that is a process-global teardown that
    would invalidate all remaining HELICS handles in the process.

    Args:
        broker: HELICS broker handle to shut down.
    """
    try:
        if hasattr(broker, "disconnect"):
            broker.disconnect()
        elif hasattr(helics, "helicsBrokerDisconnect"):
            helics.helicsBrokerDisconnect(broker)
    except Exception:
        pass


def get_broker_port(broker: helics.HelicsBroker) -> int:
    """Return the TCP port associated with a HELICS broker.

    The value comes from the explicit port passed to :func:`create_broker` or
    from parsed broker address metadata when available.

    Example:
        >>> broker = create_broker(3)
        >>> port = get_broker_port(broker)
        >>> print(f"GridLAB-D federates should connect to localhost:{port}")

    Args:
        broker: HELICS broker handle.

    Returns:
        Integer port number.

    Raises:
        RuntimeError: If a port cannot be resolved from known broker metadata.
    """

    known_port = _get_cached_broker_port(broker)
    if known_port is not None:
        return known_port

    address_getters: list[Callable[[], str]] = []
    if hasattr(broker, "address"):
        address_getters.append(lambda: str(broker.address))
    if hasattr(broker, "get_address"):
        address_getters.append(lambda: str(broker.get_address()))
    if hasattr(helics, "helicsBrokerGetAddress"):
        address_getters.append(lambda: str(helics.helicsBrokerGetAddress(broker)))

    for getter in address_getters:
        try:
            address = getter()
        except Exception:
            continue
        parsed = _parse_port_from_address(address)
        if parsed is not None:
            _set_cached_broker_port(broker, parsed)
            return parsed

    raise RuntimeError("Unable to determine broker port from HELICS broker handle")


def _create_listening_broker(n_federates: int, core_type: str, port: int) -> Any:
    """Create a broker on ``port``; raise if it could not bind its sockets.

    HELICS binds while creating the broker, and a failed bind does not raise:
    it logs, returns an unconnected broker, and every federate joining it then
    waits out its registration timeout.
    """
    broker_name = f"hares_broker_{os.getpid()}_{port}"
    init_string = f"--federates={n_federates} --port={port}"
    broker = _create_broker_handle(core_type, broker_name, init_string)
    if not _broker_is_connected(broker):
        destroy_broker(broker)
        raise RuntimeError(f"HELICS broker {broker_name} could not listen on port {port}")
    _set_cached_broker_port(broker, port)
    return broker


def _get_cached_broker_port(broker: Any) -> int | None:
    try:
        return _PORT_BY_BROKER.get(broker)
    except TypeError:
        return None


def _set_cached_broker_port(broker: Any, port: int) -> None:
    try:
        _PORT_BY_BROKER[broker] = port
    except TypeError:
        return


def allocate_ephemeral_port() -> int:
    """Return a port ``p`` with ``p`` and ``p + 1`` both free on localhost.

    A ZMQ broker or core binds two sockets, its port and the next one, so both
    are probed. Candidates come from a range clear of ports with other owners
    (see ``_PORT_RANGE_START``): kernel ephemeral ports in particular are
    handed to outgoing connections, including the federates' own connections
    to a broker, so a free probe gives no protection against them. The probe
    sockets close before the port is returned, so another process can still
    claim it first; :func:`create_broker` detects that and moves on.
    """
    for _ in range(_PORT_PROBE_ATTEMPTS):
        candidate = random.randint(_PORT_RANGE_START, _PORT_RANGE_END - 1)
        with (
            socket.socket(socket.AF_INET, socket.SOCK_STREAM) as first,
            socket.socket(socket.AF_INET, socket.SOCK_STREAM) as second,
        ):
            try:
                first.bind(("127.0.0.1", candidate))
                second.bind(("127.0.0.1", candidate + 1))
            except OSError:
                continue
            return candidate
    raise RuntimeError(
        f"No free localhost port pair in {_PORT_RANGE_START}-{_PORT_RANGE_END} "
        f"after {_PORT_PROBE_ATTEMPTS} attempts"
    )


def _create_broker_handle(core_type: str, broker_name: str, init_string: str) -> Any:
    if hasattr(helics, "helicsCreateBroker"):
        return helics.helicsCreateBroker(core_type, broker_name, init_string)
    if hasattr(helics, "HelicsBroker"):
        return helics.HelicsBroker(core_type, broker_name, init_string)
    raise RuntimeError("HELICS Python module does not expose a broker creation API")


def _broker_is_connected(broker: Any) -> bool:
    if hasattr(broker, "is_connected"):
        return bool(broker.is_connected())
    if hasattr(helics, "helicsBrokerIsConnected"):
        return bool(helics.helicsBrokerIsConnected(broker))
    raise RuntimeError("HELICS Python module does not expose broker connectivity APIs")


def _parse_port_from_address(address: str) -> int | None:
    if not address:
        return None
    value = address.strip()
    if "://" in value:
        value = value.split("://", 1)[1]
    value = value.rstrip("/")

    if value.startswith("[") and "]:" in value:
        _, _, port_text = value.rpartition("]:")
    elif ":" in value:
        _, _, port_text = value.rpartition(":")
    else:
        return None

    if not port_text.isdigit():
        return None
    port = int(port_text)
    if port <= 0:
        return None
    return port
