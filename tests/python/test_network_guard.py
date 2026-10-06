"""The offline guard every unmarked test runs under refuses the network and keeps loopback."""

from __future__ import annotations

import socket

import pytest
from conftest import refuse_network


def test_a_remote_connect_is_refused_and_recorded() -> None:
    with pytest.MonkeyPatch.context() as mp, refuse_network(mp) as attempts:
        sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        with sock, pytest.raises(ConnectionRefusedError):
            sock.connect(("192.0.2.1", 443))
    assert attempts == ["connect(('192.0.2.1', 443))"]


def test_a_remote_name_lookup_is_refused_and_recorded() -> None:
    with (
        pytest.MonkeyPatch.context() as mp,
        refuse_network(mp) as attempts,
        pytest.raises(ConnectionRefusedError),
    ):
        socket.getaddrinfo("oedi-data-lake.s3.amazonaws.com", 443)
    assert attempts == ["getaddrinfo('oedi-data-lake.s3.amazonaws.com')"]


def test_a_swallowed_attempt_is_still_recorded() -> None:
    with pytest.MonkeyPatch.context() as mp, refuse_network(mp) as attempts:
        try:
            socket.create_connection(("example.com", 80), timeout=1.0)
        except OSError:
            pass
    assert attempts == ["getaddrinfo('example.com')"]


def test_loopback_stays_open() -> None:
    with socket.create_server(("127.0.0.1", 0)) as server:
        port = server.getsockname()[1]
        with pytest.MonkeyPatch.context() as mp, refuse_network(mp) as attempts:
            with socket.create_connection(("127.0.0.1", port), timeout=5.0):
                pass
            with socket.create_connection(("localhost", port), timeout=5.0):
                pass
    assert attempts == []
