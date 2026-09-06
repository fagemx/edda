"""edda-sdk (Python) — thin client for the edda client contract.

Types module is GENERATED from the pinned event spec; see ../generator/.
Canonicalization (edda-canon-v1) is implemented independently here — see
canon.py.
"""

from .canon import (
    EVENT_HASH_EXCLUDED_KEYS,
    canonicalize,
    canonicalize_text,
    compute_event_hash,
    sha256_hex_of_text,
)
from .client import EddaClient
from .errors import (
    CancelledError_,
    CapabilityNotAvailable,
    EddaError,
    HttpWriteRefused,
    ProtocolError,
    RpcError,
    TimeoutError_,
    TransportError,
)
from .transport_http import HttpTransport
from .transport_mcp import CallOptions, McpSpawnSpec, McpTransport

__version__ = "0.1.0"

__all__ = [
    "EddaClient",
    "HttpTransport",
    "McpSpawnSpec",
    "McpTransport",
    "CallOptions",
    "EddaError",
    "TransportError",
    "TimeoutError_",
    "CancelledError_",
    "CapabilityNotAvailable",
    "HttpWriteRefused",
    "ProtocolError",
    "RpcError",
    "canonicalize",
    "canonicalize_text",
    "compute_event_hash",
    "sha256_hex_of_text",
    "EVENT_HASH_EXCLUDED_KEYS",
]
