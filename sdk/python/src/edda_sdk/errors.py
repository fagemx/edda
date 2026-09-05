"""Typed transport/protocol errors for the edda client contract.

Every operation maps transport failures to one of these — no raw exceptions
across the boundary.
"""

from __future__ import annotations


class EddaError(Exception):
    """Base class for all SDK errors."""

    kind = "EddaError"

    def __init__(self, message: str) -> None:
        super().__init__(f"[{self.kind}] {message}")


class TransportError(EddaError):
    """Underlying transport failed (spawn error, socket error, bad framing)."""

    kind = "TransportError"

    def __init__(self, message: str, cause: BaseException | None = None) -> None:
        super().__init__(message)
        self.__cause__ = cause


class TimeoutError_(EddaError):
    """Operation exceeded its deadline."""

    kind = "Timeout"

    def __init__(self, message: str) -> None:
        super().__init__(message)


class CancelledError_(EddaError):
    """Operation was cancelled via its cancellation token."""

    kind = "Cancelled"

    def __init__(self, message: str = "operation cancelled") -> None:
        super().__init__(message)


class CapabilityNotAvailable(EddaError):
    """The connected server does not expose the contracted tool/route
    (contract §5 capability gap)."""

    kind = "CapabilityNotAvailable"

    def __init__(self, operation: str, surface: str) -> None:
        super().__init__(
            f"operation '{operation}' is contracted but not exposed by this {surface} "
            "(see docs/reference/client-contract.md §5)"
        )
        self.operation = operation


class HttpWriteRefused(EddaError):
    """Write attempted over the read-only HTTP surface (contract §4)."""

    kind = "HttpWriteRefused"

    def __init__(self, operation: str) -> None:
        super().__init__(
            f"operation '{operation}' is a write; the SDK HTTP transport is read-only "
            "until HTTP write authorization lands (GH-609)"
        )


class ProtocolError(EddaError):
    """Server returned a malformed JSON-RPC/HTTP response."""

    kind = "ProtocolError"


class RpcError(EddaError):
    """JSON-RPC error object returned by the server."""

    kind = "RpcError"

    def __init__(self, code: int, message: str, data: object | None = None) -> None:
        super().__init__(f"rpc error {code}: {message}")
        self.code = code
        self.rpc_message = message
        self.data = data
