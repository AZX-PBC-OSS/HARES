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

    if not isinstance(n_federates, int) or isinstance(n_federates, bool):
        raise TypeError("n_federates must be an int")
    if n_federates <= 0:
        raise ValueError("n_federates must be positive")

    if port is not None:
        broker = _create_broker_for_port(n_federates=n_federates, core_type=core_type, port=port)
        _set_cached_broker_port(broker, port)
        return broker

    last_error: Exception | None = None
    for _ in range(16):
        selected_port = _allocate_ephemeral_port()
        try:
            broker = _create_broker_for_port(
                n_federates=n_federates,
                core_type=core_type,
                port=selected_port,
            )
        except Exception as exc:
            last_error = exc
            continue
        _set_cached_broker_port(broker, selected_port)
        return broker

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


def _create_broker_for_port(n_federates: int, core_type: str, port: int) -> Any:
    broker_name = f"hares_broker_{os.getpid()}_{port}"
    init_string = f"--federates={n_federates} --port={port}"
    return _create_broker_handle(core_type, broker_name, init_string)


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


def _allocate_ephemeral_port() -> int:
    # TOCTOU: the socket is closed before the port is returned, so another
    # process can claim it before create_broker() binds.  The 16-retry loop
    # in create_broker() mitigates this race; switching to SO_REUSEPORT or
    # passing the bound socket directly is not possible with the HELICS API.
    #
    # Using bind(0) once per fresh worker/process can repeatedly return the same
    # first ephemeral port in isolated network namespaces. Probe a random free
    # port first to avoid deterministic collisions under xdist workers.
    for _ in range(128):
        candidate = random.randint(20000, 60999)
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
            try:
                sock.bind(("127.0.0.1", candidate))
            except OSError:
                continue
            return int(candidate)

    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


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
