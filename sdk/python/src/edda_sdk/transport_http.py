"""Read-only HTTP transport against ``edda serve`` (``/api/*``).

Writes are REFUSED by construction (contract §4): the HTTP write path is
unauthenticated today and its authorization model depends on the signing
ticket (GH-609), which is still design/spike. The SDK will not pretend
otherwise.
"""

from __future__ import annotations

import json
import threading
import urllib.error
import urllib.request

from .errors import CancelledError_, HttpWriteRefused, ProtocolError, TimeoutError_, TransportError

_WRITE_METHODS = {"POST", "PUT", "PATCH", "DELETE"}


class HttpTransport:
    def __init__(self, base_url: str, default_timeout_s: float = 30.0) -> None:
        self._base = base_url.rstrip("/")
        self._default_timeout_s = default_timeout_s

    def _request(
        self,
        method: str,
        path: str,
        timeout_s: float | None = None,
        cancel: threading.Event | None = None,
    ) -> object:
        if method.upper() in _WRITE_METHODS:
            raise HttpWriteRefused(f"{method} {path}")
        timeout = timeout_s if timeout_s is not None else self._default_timeout_s
        req = urllib.request.Request(f"{self._base}{path}", method=method, headers={"Accept": "application/json"})
        try:
            with urllib.request.urlopen(req, timeout=timeout) as resp:  # noqa: S310 - fixed scheme
                body = resp.read().decode("utf-8")
        except urllib.error.HTTPError as exc:
            raise TransportError(f"HTTP {exc.code} on {path}") from exc
        except urllib.error.URLError as exc:
            if cancel is not None and cancel.is_set():
                raise CancelledError_() from exc
            raise TransportError(f"{method} {path} failed: {exc.reason}") from exc
        except TimeoutError as exc:
            raise TimeoutError_(f"{method} {path} exceeded {timeout}s") from exc
        try:
            return json.loads(body)
        except json.JSONDecodeError as exc:
            raise ProtocolError(f"non-JSON response on {path}") from exc

    # ── Read operations (contract §2) ──

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
