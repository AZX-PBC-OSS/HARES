"""An httpx transport whose connections can be cut from another thread.

Closing an httpx client does not wake a thread blocked in a connect, a TLS
handshake or a read on one of its connections; shutting the connection's
socket down does. httpx's own transport opens its sockets out of reach, so
this one runs the same httpcore connection pool on a network backend that
records every socket before it connects and every TLS socket before its
handshake. :meth:`Connections.cut` then ends every request in flight on the
transport, whatever stage it is at.
"""

from __future__ import annotations

import contextlib
import select
import socket
import ssl
import threading
import time
import weakref
from collections.abc import Generator, Iterable, Iterator, Mapping
from typing import Any, cast

import httpcore
import httpx


class Connections:
    """The sockets a transport opens; cutting them ends every request on them.

    A socket registered after the cut is shut at once, so no connection
    outlives it.
    """

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._sockets: weakref.WeakSet[socket.socket] = weakref.WeakSet()
        self._cut = False

    def add(self, sock: socket.socket) -> None:
        with self._lock:
            if not self._cut:
                self._sockets.add(sock)
                return
        _shut(sock)

    def cut(self) -> None:
        with self._lock:
            self._cut = True
            sockets = list(self._sockets)
        for sock in sockets:
            _shut(sock)


def _shut(sock: socket.socket) -> None:
    with contextlib.suppress(OSError):
        sock.shutdown(socket.SHUT_RDWR)


@contextlib.contextmanager
def _mapped(timeout: type[Exception], error: type[Exception]) -> Generator[None]:
    """Raise a socket failure as the httpcore exception httpcore's own backend raises."""
    try:
        yield
    except TimeoutError as exc:
        raise timeout(exc) from exc
    except OSError as exc:
        raise error(exc) from exc


class _Stream(httpcore.NetworkStream):
    def __init__(self, sock: socket.socket, connections: Connections) -> None:
        self._sock = sock
        self._connections = connections

    def read(self, max_bytes: int, timeout: float | None = None) -> bytes:
        with _mapped(httpcore.ReadTimeout, httpcore.ReadError):
            self._sock.settimeout(timeout)
            return self._sock.recv(max_bytes)

    def write(self, buffer: bytes, timeout: float | None = None) -> None:
        with _mapped(httpcore.WriteTimeout, httpcore.WriteError):
            self._sock.settimeout(timeout)
            self._sock.sendall(buffer)

    def close(self) -> None:
        self._sock.close()

    def start_tls(
        self,
        ssl_context: ssl.SSLContext,
        server_hostname: str | None = None,
        timeout: float | None = None,
    ) -> httpcore.NetworkStream:
        try:
            with _mapped(httpcore.ConnectTimeout, httpcore.ConnectError):
                self._sock.settimeout(timeout)
                tls = ssl_context.wrap_socket(
                    self._sock, server_hostname=server_hostname, do_handshake_on_connect=False
                )
                self._connections.add(tls)
                tls.do_handshake()
        except Exception:
            self.close()
            raise
        return _Stream(tls, self._connections)

    def get_extra_info(self, info: str) -> object:
        if info == "ssl_object":
            return self._sock if isinstance(self._sock, ssl.SSLSocket) else None
        if info == "client_addr":
            return self._sock.getsockname()
        if info == "server_addr":
            return self._sock.getpeername()
        if info == "socket":
            return self._sock
        if info == "is_readable":
            # Readable with nothing sent means the server closed the idle
            # connection, so the pool does not reuse it.
            return self._sock.fileno() < 0 or bool(select.select([self._sock], [], [], 0)[0])
        return None


class _Backend(httpcore.NetworkBackend):
    def __init__(self, connections: Connections) -> None:
        self._connections = connections

    def connect_tcp(
        self,
        host: str,
        port: int,
        timeout: float | None = None,
        local_address: str | None = None,
        # httpcore's SOCKET_OPTION tuples, each the arguments of one setsockopt.
        socket_options: Iterable[Any] | None = None,
    ) -> httpcore.NetworkStream:
        last_error: Exception = httpcore.ConnectError(f"no address for {host}")
        with _mapped(httpcore.ConnectTimeout, httpcore.ConnectError):
            addresses = socket.getaddrinfo(host, port, type=socket.SOCK_STREAM)
        for family, kind, proto, _, address in addresses:
            sock = socket.socket(family, kind, proto)
            self._connections.add(sock)
            try:
                with _mapped(httpcore.ConnectTimeout, httpcore.ConnectError):
                    sock.settimeout(timeout)
                    if local_address is not None:
                        sock.bind((local_address, 0))
                    for option in socket_options or ():
                        sock.setsockopt(*option)
                    sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
                    sock.connect(address)
            except (httpcore.ConnectError, httpcore.ConnectTimeout) as exc:
                sock.close()
                last_error = exc
                continue
            return _Stream(sock, self._connections)
        raise last_error

    def sleep(self, seconds: float) -> None:
        time.sleep(seconds)


class _ResponseBytes(httpx.SyncByteStream):
    def __init__(self, stream: Iterable[bytes]) -> None:
        self._stream = stream

    def __iter__(self) -> Iterator[bytes]:
        yield from self._stream

    def close(self) -> None:
        close = getattr(self._stream, "close", None)
        if close is not None:
            close()


class CuttableTransport(httpx.BaseTransport):
    """httpx's default transport behaviour over a pool whose sockets ``connections`` holds."""

    def __init__(self, connections: Connections) -> None:
        self._pool = httpcore.ConnectionPool(
            ssl_context=httpx.create_ssl_context(), network_backend=_Backend(connections)
        )

    def handle_request(self, request: httpx.Request) -> httpx.Response:
        assert isinstance(request.stream, httpx.SyncByteStream)
        response = self._pool.handle_request(
            httpcore.Request(
                method=request.method,
                url=httpcore.URL(
                    scheme=request.url.raw_scheme,
                    host=request.url.raw_host,
                    port=request.url.port,
                    target=request.url.raw_path,
                ),
                headers=request.headers.raw,
                content=request.stream,
                extensions=request.extensions,
            )
        )
        assert isinstance(response.stream, Iterable)
        # httpcore types a response's extensions partly as an untyped dict.
        extensions = cast("Mapping[str, Any]", getattr(response, "extensions"))
        return httpx.Response(
            status_code=response.status,
            headers=response.headers,
            stream=_ResponseBytes(response.stream),
            extensions=dict(extensions),
        )

    def close(self) -> None:
        self._pool.close()
