"""Keep the Python tests off the network unless a test is marked ``network``.

An audit hook (PEP 578) refuses every operation that could reach past this
host: socket connects and datagram sends to a non-loopback address, at any
socket layer including raw ``_socket``, and every resolver entry point for a
non-loopback name. It covers fixtures of any scope and import-time code,
because it is installed once per process and consults the innermost active
scope: a test's own scope while pytest runs that test (its setup, call and
teardown, so a module- or session-scoped fixture set up for it counts), and a
refusing scope outside any test.

CPython resolves a host name given to ``connect`` or ``sendto`` inside the
socket call, before it raises the audit event, and that lookup raises no
resolver event of its own. So the guard also checks the address where Python
code hands it to a ``socket.socket`` method, before any lookup: a name is
refused there without a query being sent, and a name that does not resolve
is recorded instead of ending in an unrecorded ``gaierror``.

A subprocess that is a Python interpreter inherits an environment whose
``sitecustomize`` (``_offline_site``) installs the same refusal for the whole
child. Any other subprocess, and a Python child that would skip
``sitecustomize`` (``-S``, ``-I`` or ``-E`` among its interpreter options, or
an environment of its own without it), could reach the network unseen, so
inside a refusing scope spawning one is refused as well.

Every refused attempt is recorded as well as raised, so an attempt swallowed
by the code under test (a retry loop, a skip-on-failure fetch) still fails the
test that made it. This module imports nothing beyond the standard library:
every Python child imports it at startup.

Known limits, each because no Python-level hook sees the operation:

- Native code that opens sockets itself (``ctypes`` calls into libc, or a C
  library such as HELICS's ZMQ transport) raises no audit event. The HELICS
  tests reach loopback only, by construction.
- A host name passed to a raw ``_socket.socket`` (not ``socket.socket``) is
  resolved by CPython before the audit event, and ``_socket.socket`` is a
  built-in type whose methods cannot be wrapped: the connect is still
  refused, but the DNS query has gone out, and a name that does not resolve
  raises ``gaierror`` unrecorded.
"""

from __future__ import annotations

import functools
import ipaddress
import os
import shutil
import socket
import sys
from collections.abc import Iterator, Mapping
from contextlib import contextmanager
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

SITE_DIR = Path(__file__).resolve().parent / "_offline_site"
ENV_FLAG = "HARES_TESTS_OFFLINE"

_SOCKET_EVENTS = frozenset({"socket.connect", "socket.sendto", "socket.sendmsg"})
_RESOLVER_EVENTS = frozenset(
    {"socket.getaddrinfo", "socket.gethostbyname", "socket.gethostbyaddr", "socket.getnameinfo"}
)
_SPAWN_EVENTS = frozenset({"subprocess.Popen", "os.posix_spawn", "os.spawn", "os.system", "os.exec"})
_GUARDED_EVENTS = _SOCKET_EVENTS | _RESOLVER_EVENTS | _SPAWN_EVENTS
_INET_FAMILIES = (2, 10)  # AF_INET, AF_INET6


@dataclass(frozen=True)
class _LocalCommand:
    binaries: tuple[str, ...]
    operands: int


# Commands that cannot reach the network: ``uname -p``, which
# ``platform.processor()`` runs in every xdist worker at startup, and
# ``readelf -d`` of one file, which the helics wheel canary reads. A command
# line is its fixed leading arguments followed by exactly ``operands`` more,
# and it is allowed only when the program that would run is the system's own
# binary.
_LOCAL_COMMANDS: dict[tuple[str, ...], _LocalCommand] = {
    ("uname", "-p"): _LocalCommand(("/usr/bin/uname", "/bin/uname"), operands=0),
    ("readelf", "-d"): _LocalCommand(("/usr/bin/readelf", "/bin/readelf"), operands=1),
}


class NetworkRefused(ConnectionRefusedError):
    """Raised for an operation a test that is not marked ``network`` may not perform."""


@dataclass
class _Scope:
    allow_network: bool
    attempts: list[str] = field(default_factory=list)


_scopes: list[_Scope] = []
_installed = False


def _is_loopback(host: object) -> bool:
    if isinstance(host, bytes):
        host = host.decode()
    if host in (None, "", "localhost"):
        return True
    if not isinstance(host, str):
        return False
    try:
        return ipaddress.ip_address(host.split("%", 1)[0]).is_loopback
    except ValueError:
        return False


def _remote_socket_address(sock: Any, address: object) -> bool:
    family = getattr(sock, "family", None)
    if address is None or family is None or int(family) not in _INET_FAMILIES:
        return False
    return not _is_loopback(address[0] if isinstance(address, tuple) else address)


def _text(value: object) -> str:
    if isinstance(value, (str, bytes)):
        return os.fsdecode(value)
    if isinstance(value, os.PathLike):
        return os.fsdecode(os.fspath(value))
    return ""


def _skips_sitecustomize(options: list[str]) -> bool:
    """Whether interpreter options ``-I``, ``-S`` or ``-E`` precede the program.

    Parsed as CPython parses them: single-letter options may be clustered
    (``-sS``); ``-c`` and ``-m`` end the options; ``-W`` and ``-X`` take a
    value, attached or as the next argument.
    """
    position = 0
    while position < len(options):
        option = options[position]
        if option in ("-", "--") or not option.startswith("-"):
            return False
        if option.startswith("--"):
            position += 2 if option == "--check-hash-based-pycs" else 1
            continue
        for index, letter in enumerate(option[1:], start=1):
            if letter in "ISE":
                return True
            if letter in "cm":
                return False
            if letter in "WX":
                if index == len(option) - 1:
                    position += 1
                break
        position += 1
    return False


def _guarded_python_child(executable: object, argv: object, env: Mapping[str, str] | None) -> bool:
    args = [_text(a) for a in argv] if isinstance(argv, (list, tuple)) else []
    program = _text(executable) or (args[0] if args else "")
    if not (Path(program).name.startswith("python") or program == sys.executable):
        return False
    if _skips_sitecustomize(args[1:]):
        return False
    environment = os.environ if env is None else env
    return environment.get(ENV_FLAG) == "1" and str(SITE_DIR) in environment.get("PYTHONPATH", "")


def _local_command(executable: object, argv: object, env: Mapping[str, str] | None) -> bool:
    """Whether ``argv`` is an allowed command and the program it would run is that command's binary."""
    if not isinstance(argv, (list, tuple)):
        return False
    words = tuple(_text(a) for a in argv)
    command = next(
        (
            command
            for prefix, command in _LOCAL_COMMANDS.items()
            if words[: len(prefix)] == prefix and len(words) == len(prefix) + command.operands
        ),
        None,
    )
    if command is None:
        return False
    program = _text(executable) or _text(argv[0])
    if os.sep not in program:
        search_path = (os.environ if env is None else env).get("PATH", os.defpath)
        found = shutil.which(program, path=search_path)
        if found is None:
            return False
        program = found
    return os.path.realpath(program) in {os.path.realpath(binary) for binary in command.binaries}


def _violation(event: str, args: tuple[Any, ...]) -> str | None:
    if event in _SOCKET_EVENTS:
        return f"{event}({args[1]!r})" if _remote_socket_address(args[0], args[1]) else None
    if event in _RESOLVER_EVENTS:
        target = args[0]
        host = target[0] if isinstance(target, tuple) else target
        return None if _is_loopback(host) else f"{event}({target!r})"
    if event == "os.system":
        return f"{event}({args[0]!r}) without the offline guard"
    # subprocess.Popen(executable, args, cwd, env); os.posix_spawn and
    # os.exec(path, argv, env); os.spawn(mode, path, argv, env).
    executable, argv, env = {
        "subprocess.Popen": lambda: (args[0], args[1], args[3]),
        "os.spawn": lambda: (args[1], args[2], args[3]),
    }.get(event, lambda: (args[0], args[1], args[2]))()
    if _guarded_python_child(executable, argv, env) or _local_command(executable, argv, env):
        return None
    return f"{event}({argv!r}) without the offline guard"


def _audit(event: str, args: tuple[Any, ...]) -> None:
    if event not in _GUARDED_EVENTS or not _scopes or _scopes[-1].allow_network:
        return
    violation = _violation(event, args)
    if violation is None:
        return
    _scopes[-1].attempts.append(violation)
    raise NetworkRefused(f"offline test reached the network: {violation}")


# The socket.socket methods that take an address, the audit event CPython
# raises for each, and the address's position among the call's arguments
# (``sendto(data, address)`` or ``sendto(data, flags, address)``: last).
_ADDRESS_ARGUMENTS = (
    ("connect", "socket.connect", 0),
    ("connect_ex", "socket.connect", 0),
    ("sendto", "socket.sendto", -1),
    ("sendmsg", "socket.sendmsg", 3),
)


def _check_before_resolving(method_name: str, event: str, address_index: int) -> None:
    """Wrap ``socket.socket.<method_name>`` to check its address before the call resolves it."""
    original = getattr(socket.socket, method_name)

    @functools.wraps(original)
    def checked(sock: socket.socket, *args: Any) -> Any:
        if len(args) > address_index:
            _audit(event, (sock, args[address_index]))
        return original(sock, *args)

    setattr(socket.socket, method_name, checked)


def install() -> None:
    """Install the audit hook once for this process (audit hooks cannot be removed)."""
    global _installed
    if not _installed:
        sys.addaudithook(_audit)
        for method_name, event, address_index in _ADDRESS_ARGUMENTS:
            _check_before_resolving(method_name, event, address_index)
        _installed = True


def refuses_network() -> bool:
    """Whether the innermost active scope refuses network access."""
    return bool(_scopes) and not _scopes[-1].allow_network


@contextmanager
def scope(*, allow_network: bool) -> Iterator[list[str]]:
    """Run a block under its own policy; yield the attempts it refused."""
    install()
    current = _Scope(allow_network)
    _scopes.append(current)
    try:
        yield current.attempts
    finally:
        _scopes.remove(current)


def refuse_for_this_process() -> None:
    """Refuse the network for the rest of this process (a Python child's startup)."""
    install()
    _scopes.append(_Scope(allow_network=False))


def child_environment() -> dict[str, str]:
    """The variables a Python child needs to inherit the refusal."""
    path = os.environ.get("PYTHONPATH", "")
    if str(SITE_DIR) in path.split(os.pathsep):
        return {ENV_FLAG: "1", "PYTHONPATH": path}
    return {ENV_FLAG: "1", "PYTHONPATH": os.pathsep.join(filter(None, [str(SITE_DIR), path]))}
