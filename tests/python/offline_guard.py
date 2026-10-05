"""Keep the Python tests off the network unless a test is marked ``network``.

An audit hook (PEP 578) refuses every operation that could reach past this
host: socket connects and datagram sends to a non-loopback address, at any
socket layer including raw ``_socket``, and every resolver entry point for a
non-loopback name. It covers fixtures of any scope and import-time code,
because it is installed once per process and consults the innermost active
scope: a test's own scope while pytest runs that test (its setup, call and
teardown, so a module- or session-scoped fixture set up for it counts), and a
refusing scope outside any test.

A subprocess that is a Python interpreter inherits an environment whose
``sitecustomize`` (``_offline_site``) installs the same refusal for the whole
child. Any other subprocess, and a Python child that would skip
``sitecustomize`` (``-S``, ``-I``, or an environment of its own without it),
could reach the network unseen, so inside a refusing scope spawning one is
refused as well.

Every refused attempt is recorded as well as raised, so an attempt swallowed
by the code under test (a retry loop, a skip-on-failure fetch) still fails the
test that made it. This module imports nothing beyond the standard library:
every Python child imports it at startup.
"""

from __future__ import annotations

import ipaddress
import os
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


def _guarded_python_child(executable: object, argv: object, env: Mapping[str, str] | None) -> bool:
    args = [_text(a) for a in argv] if isinstance(argv, (list, tuple)) else []
    program = _text(executable) or (args[0] if args else "")
    if not (Path(program).name.startswith("python") or program == sys.executable):
        return False
    if any(flag in ("-S", "-I") for flag in args[1:]):
        return False
    environment = os.environ if env is None else env
    return environment.get(ENV_FLAG) == "1" and str(SITE_DIR) in environment.get("PYTHONPATH", "")


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
    if _guarded_python_child(executable, argv, env):
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


def install() -> None:
    """Install the audit hook once for this process (audit hooks cannot be removed)."""
    global _installed
    if not _installed:
        sys.addaudithook(_audit)
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
