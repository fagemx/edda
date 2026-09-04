"""Read-only HTTP transport against ``edda serve`` (``/api/*``)."""
from __future__ import annotations

import http.client
import json
import queue
import socket
import threading
import time
from urllib.parse import urlsplit

from .errors import CancelledError_, HttpWriteRefused, ProtocolError, TimeoutError_, TransportError

_WRITE_METHODS = {"POST", "PUT", "PATCH", "DELETE"}


class HttpTransport:
    """Read-only service client; ``bearer_token`` is sent only as an HTTP header."""

    def __init__(self, base_url: str, default_timeout_s: float = 30.0, bearer_token: str | None = None) -> None:
        parsed = urlsplit(base_url)
        if parsed.scheme not in {"http", "https"} or not parsed.netloc:
            raise ValueError("base_url must be an absolute http(s) URL")
        self._base = base_url.rstrip("/")
        self._parsed = parsed
        self._default_timeout_s = default_timeout_s
        self._bearer_token = bearer_token

    def _request(self, method: str, path: str, timeout_s: float | None = None, cancel: threading.Event | None = None) -> object:
        if method.upper() in _WRITE_METHODS:
            raise HttpWriteRefused(f"{method} {path}")
        if cancel is not None and cancel.is_set():
            raise CancelledError_()
        timeout = timeout_s if timeout_s is not None else self._default_timeout_s
        if timeout <= 0:
            raise TimeoutError_(f"{method} {path} exceeded {timeout}s")

        # http.client exposes the connection so the controlling thread can
        # close its socket immediately on cancellation/deadline. urllib's
        # urlopen does not, which made an in-flight request uninterruptible.
        connection_type = http.client.HTTPSConnection if self._parsed.scheme == "https" else http.client.HTTPConnection
        # A cancellation-aware request uses a short socket wait so a platform
        # that cannot interrupt a blocking recv still reaches a deterministic
        # cleanup checkpoint. Ordinary reads retain their caller deadline.
        socket_timeout = min(timeout, 0.25) if cancel is not None else timeout
        conn = connection_type(self._parsed.hostname, self._parsed.port, timeout=socket_timeout)
        result: queue.Queue[tuple[str, object]] = queue.Queue(maxsize=1)
        active_socket: list[socket.socket | None] = [None]
        stop = threading.Event()
        response_ref: list[http.client.HTTPResponse | None] = [None]
        headers = {"Accept": "application/json"}
        if self._bearer_token is not None:
            headers["Authorization"] = f"Bearer {self._bearer_token}"
        target = (self._parsed.path.rstrip("/") + path) or path
        if self._parsed.query:
            target = f"{target}?{self._parsed.query}"

        def run() -> None:
            try:
                conn.request(method, target, headers=headers)
                active_socket[0] = conn.sock
                if stop.is_set():
                    return
                response = conn.getresponse()
                response_ref[0] = response
                body = response.read().decode("utf-8")
                result.put(("response", (response.status, body)))
            except Exception as exc:  # converted below without exposing headers/token
                result.put(("error", exc))
            finally:
                # HTTPResponse owns a buffered socket file; close it before
                # closing the connection so cancellation cannot leave it for GC.
                if response_ref[0] is not None:
                    response_ref[0].close()
                conn.close()
                active_socket[0] = None

        worker = threading.Thread(target=run, name="edda-sdk-http", daemon=False)
        worker.start()
        deadline = time.monotonic() + timeout
        reason: Exception | None = None
        item: tuple[str, object] | None = None
        while item is None:
            if cancel is not None and cancel.is_set():
                reason = CancelledError_()
            elif time.monotonic() >= deadline:
                reason = TimeoutError_(f"{method} {path} exceeded {timeout}s")
            if reason is not None:
                stop.set()
                # Explicit shutdown wakes a thread blocked in getresponse/read;
                # close() alone is not sufficient on every platform.
                if active_socket[0] is not None:
                    try:
                        active_socket[0].shutdown(socket.SHUT_RDWR)
                    except OSError:
                        pass
                conn.close()
                # Do not hide a lingering thread behind a timeout: surface it
                # as transport failure rather than returning a fake success.
                worker.join(timeout=1)
                if worker.is_alive():
                    raise TransportError(f"{method} {path} did not stop after cancellation/deadline")
                raise reason
            try:
                item = result.get(timeout=min(0.05, max(0.0, deadline - time.monotonic())))
            except queue.Empty:
                continue
        worker.join()
        kind, value = item
        if kind == "error":
            raise TransportError(f"{method} {path} failed") from value  # type: ignore[arg-type]
        status, body = value  # type: ignore[misc]
        if not 200 <= status < 300:
            raise TransportError(f"HTTP {status} on {path}")
        try:
            return json.loads(body)
        except json.JSONDecodeError as exc:
            raise ProtocolError(f"non-JSON response on {path}") from exc

    def status(self, **kw: object) -> object:
        return self._request("GET", "/api/status", **kw)  # type: ignore[arg-type]

    def decisions(self, query: str = "", **kw: object) -> object:
        return self._request("GET", f"/api/decisions{'?' + query if query else ''}", **kw)  # type: ignore[arg-type]

    def log(self, query: str = "", **kw: object) -> object:
        return self._request("GET", f"/api/log{'?' + query if query else ''}", **kw)  # type: ignore[arg-type]

    def context(self, **kw: object) -> object:
        return self._request("GET", "/api/context", **kw)  # type: ignore[arg-type]

    def health(self, **kw: object) -> object:
        return self._request("GET", "/api/health", **kw)  # type: ignore[arg-type]
