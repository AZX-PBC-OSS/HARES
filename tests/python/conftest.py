"""Shared pytest fixtures for HARES tests."""

from __future__ import annotations

from collections.abc import Callable, Iterator, Mapping
from contextlib import contextmanager
import ipaddress
from pathlib import Path
import socket
import sys
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[2]
PYTHON_SRC = ROOT / "python"

if str(PYTHON_SRC) not in sys.path:
    sys.path.insert(0, str(PYTHON_SRC))

# The helics wheel's libzmq exports the C++ standard-library symbols it was
# built against without declaring libstdc++ as a dependency, so importing
# helics before pyarrow poisons symbol resolution and the first pyarrow
# parquet writer segfaults. Importing pyarrow first resolves every symbol
# from libstdc++ and both libraries work. Every worker process imports this
# conftest before any test module, which fixes the order for the whole run;
# the defect and its signal are pinned in test_helics_pyarrow_symbol_order.py.
import pyarrow.parquet  # noqa: F401  (must precede any helics import)

HARES_DEFAULTS = ROOT / "defaults"
HPXML = str(ROOT / "tests/fixtures/hpxml/ochre_samples/base.xml")
WEATHER = str(ROOT / "data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw")
SCHEDULE = str(ROOT / "data/examples/BEopt_example_schedule.csv")


def _is_loopback(host: object) -> bool:
    if host in (None, "", "localhost"):
        return True
    if not isinstance(host, str):
        return False
    try:
        return ipaddress.ip_address(host.split("%", 1)[0]).is_loopback
    except ValueError:
        return False


@contextmanager
def refuse_network(monkeypatch: pytest.MonkeyPatch) -> Iterator[list[str]]:
    """Refuse name lookups and socket connects beyond loopback; yield the attempts.

    Every refused attempt is recorded as well as raised, so one swallowed by
    the code under test (a retry loop, a skip-on-failure fleet fetch) is still
    seen. Loopback stays open for in-process brokers and local services.
    """
    attempts: list[str] = []

    def refuse(what: str) -> OSError:
        attempts.append(what)
        return ConnectionRefusedError(f"offline test reached the network: {what}")

    real_getaddrinfo = socket.getaddrinfo
    real_connect = socket.socket.connect
    real_connect_ex = socket.socket.connect_ex

    def getaddrinfo(host: str | bytes | None, *args: Any, **kwargs: Any) -> Any:
        if not _is_loopback(host.decode() if isinstance(host, bytes) else host):
            raise refuse(f"getaddrinfo({host!r})")
        return real_getaddrinfo(host, *args, **kwargs)

    def remote(sock: socket.socket, address: object) -> bool:
        return sock.family in (socket.AF_INET, socket.AF_INET6) and not _is_loopback(
            address[0] if isinstance(address, tuple) else address
        )

    def connect(sock: socket.socket, address: object) -> None:
        if remote(sock, address):
            raise refuse(f"connect({address!r})")
        real_connect(sock, address)

    def connect_ex(sock: socket.socket, address: object) -> int:
        if remote(sock, address):
            raise refuse(f"connect_ex({address!r})")
        return real_connect_ex(sock, address)

    monkeypatch.setattr(socket, "getaddrinfo", getaddrinfo)
    monkeypatch.setattr(socket.socket, "connect", connect)
    monkeypatch.setattr(socket.socket, "connect_ex", connect_ex)
    yield attempts


@pytest.fixture(autouse=True)
def _offline_unless_marked_network(
    request: pytest.FixtureRequest, monkeypatch: pytest.MonkeyPatch
) -> Iterator[None]:
    """Fail any test that reaches past this host unless it is marked ``network``."""
    if request.node.get_closest_marker("network") is not None:
        yield
        return
    with refuse_network(monkeypatch) as attempts:
        yield
    if attempts:
        pytest.fail(
            "test is not marked network but reached the network: " + "; ".join(attempts),
            pytrace=False,
        )


def output_in_test_dir(config: Mapping[str, object], filename: str) -> Callable[..., None]:
    """An autouse fixture pointing ``config["output_path"]`` into each test's ``tmp_path``.

    For a module whose shared run config must write output: bind the result to
    a module-level name and every test in the module writes its own file,
    removed with the test's temporary directory.
    """

    @pytest.fixture(autouse=True)
    def _output_in_test_dir(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
        monkeypatch.setitem(config, "output_path", str(tmp_path / filename))

    return _output_in_test_dir


def make_dwelling(
    duration_s: int = 300,
    time_res_s: int = 60,
    seed: int = 0,
    output_verbosity: int = 0,
    start_time: str = "2019-01-01T00:00:00",
    write_output: bool = False,
    **kw,
):
    from ochre_next import Dwelling

    dw = Dwelling.from_hpxml(
        HPXML,
        SCHEDULE,
        WEATHER,
        start_time=start_time,
        duration_s=duration_s,
        time_res_s=time_res_s,
        defaults_path=str(HARES_DEFAULTS),
        bldg_id=42,
        master_seed=seed,
        output_verbosity=output_verbosity,
        write_output=write_output,
        **kw,
    )
    dw.initialize()
    return dw
