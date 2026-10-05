"""Every way out to the network is refused for a test not marked ``network``.

Each probe runs under a nested refusing scope, so the refused attempt is
recorded there and does not fail the probe's own test. The addresses are
TEST-NET (192.0.2.0/24) and reserved names, which no packet may reach anyway:
the guard refuses before anything is sent.
"""

from __future__ import annotations

import _socket
import os
import socket
import subprocess
import sys
from collections.abc import Callable

import offline_guard
import pytest

_REMOTE = ("192.0.2.1", 9)


def _refused(probe: Callable[[], object]) -> list[str]:
    with offline_guard.scope(allow_network=False) as attempts, pytest.raises(offline_guard.NetworkRefused):
        probe()
    return attempts


def test_a_connect_at_the_socket_module_layer_is_refused() -> None:
    def probe() -> None:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
            sock.connect(_REMOTE)

    assert _refused(probe) == [f"socket.connect({_REMOTE!r})"]


def test_a_connect_on_a_raw_socket_is_refused() -> None:
    def probe() -> None:
        sock = _socket.socket(_socket.AF_INET, _socket.SOCK_STREAM)
        try:
            sock.connect(_REMOTE)
        finally:
            sock.close()

    assert _refused(probe) == [f"socket.connect({_REMOTE!r})"]


def test_a_datagram_sent_without_connecting_is_refused() -> None:
    def probe() -> None:
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
            sock.sendto(b"x", _REMOTE)

    assert _refused(probe) == [f"socket.sendto({_REMOTE!r})"]


@pytest.mark.parametrize(
    "lookup",
    [
        lambda: socket.getaddrinfo("example.invalid", 443),
        lambda: socket.gethostbyname("example.invalid"),
        lambda: socket.gethostbyname_ex("example.invalid"),
        lambda: socket.gethostbyaddr("192.0.2.1"),
        lambda: socket.getnameinfo(_REMOTE, 0),
    ],
    ids=["getaddrinfo", "gethostbyname", "gethostbyname_ex", "gethostbyaddr", "getnameinfo"],
)
def test_every_resolver_entry_point_is_refused(lookup: Callable[[], object]) -> None:
    assert len(_refused(lookup)) == 1


def test_a_python_child_inherits_the_refusal() -> None:
    child = subprocess.run(
        [sys.executable, "-c", f"import socket; socket.create_connection({_REMOTE!r}, timeout=5)"],
        capture_output=True,
        text=True,
        check=False,
    )
    assert child.returncode != 0
    assert "offline test reached the network" in child.stderr


def test_a_python_child_that_would_skip_the_refusal_is_not_started() -> None:
    attempts = _refused(lambda: subprocess.run([sys.executable, "-I", "-c", "pass"], check=False))
    assert len(attempts) == 1


def test_a_python_child_given_an_environment_without_the_refusal_is_not_started() -> None:
    bare = {key: value for key, value in os.environ.items() if key != offline_guard.ENV_FLAG}
    attempts = _refused(lambda: subprocess.run([sys.executable, "-c", "pass"], env=bare, check=False))
    assert len(attempts) == 1


def test_any_other_subprocess_is_not_started() -> None:
    assert len(_refused(lambda: subprocess.run(["true"], check=False))) == 1


def test_loopback_stays_open() -> None:
    with offline_guard.scope(allow_network=False) as attempts, socket.create_server(("127.0.0.1", 0)) as server:
        port = server.getsockname()[1]
        with socket.create_connection(("127.0.0.1", port), timeout=5.0):
            pass
        with socket.create_connection(("localhost", port), timeout=5.0):
            pass
    assert attempts == []


@pytest.fixture(scope="session")
def _refused_during_session_fixture_setup() -> bool:
    return offline_guard.refuses_network()


@pytest.fixture(scope="module")
def _refused_during_module_fixture_setup() -> bool:
    return offline_guard.refuses_network()


def test_fixtures_above_function_scope_run_refused(
    _refused_during_session_fixture_setup: bool, _refused_during_module_fixture_setup: bool
) -> None:
    assert _refused_during_session_fixture_setup
    assert _refused_during_module_fixture_setup


_SESSION_PROBES = """
import socket

import pytest


@pytest.fixture(scope="module")
def reaches_out():
    try:
        socket.create_connection(("192.0.2.1", 9), timeout=5)
    except OSError:
        pass


def test_through_a_module_fixture(reaches_out):
    pass


def test_swallowing_the_refusal():
    try:
        socket.getaddrinfo("example.invalid", 443)
    except OSError:
        pass


def test_on_loopback_only():
    with socket.create_server(("127.0.0.1", 0)) as server:
        socket.create_connection(server.getsockname(), timeout=5).close()
"""


def test_an_attempt_fails_the_test_that_made_it_even_when_swallowed(pytester: pytest.Pytester) -> None:
    pytester.makeconftest('pytest_plugins = ["offline_guard_plugin"]')
    pytester.makepyfile(test_probes=_SESSION_PROBES)
    result = pytester.runpytest_inprocess("-p", "no:cacheprovider", "-p", "no:xdist")
    result.assert_outcomes(passed=3, errors=2)
    result.stdout.fnmatch_lines(
        [
            "*ERROR at teardown of test_through_a_module_fixture*",
            "*not marked network but reached the network: socket.getaddrinfo*",
            "*ERROR at teardown of test_swallowing_the_refusal*",
        ]
    )
