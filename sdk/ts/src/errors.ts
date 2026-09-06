// Typed transport/protocol errors for the edda client contract.
// Every operation maps transport failures to one of these — no raw throws
// across the boundary.

export class EddaError extends Error {
  readonly kind: string;
  constructor(kind: string, message: string) {
    super(`[${kind}] ${message}`);
    this.kind = kind;
    this.name = "EddaError";
  }
}

/** Underlying transport failed (spawn error, socket error, bad framing). */
export class TransportError extends EddaError {
  readonly cause?: unknown;
  constructor(message: string, cause?: unknown) {
    super("TransportError", message);
    this.name = "TransportError";
    this.cause = cause;
  }
}

/** Operation exceeded its deadline. */
export class TimeoutError extends EddaError {
  constructor(message: string) {
    super("Timeout", message);
    this.name = "TimeoutError";
  }
}

/** Operation was cancelled via an abort signal. */
export class CancelledError extends EddaError {
  constructor(message = "operation cancelled") {
    super("Cancelled", message);
    this.name = "CancelledError";
  }
}

/** The connected server does not expose the contracted tool/route (capability gap, contract §5). */
export class CapabilityNotAvailable extends EddaError {
  readonly operation: string;
  readonly surface: string;
  constructor(operation: string, surface: string) {
    super(
      "CapabilityNotAvailable",
      `operation '${operation}' is contracted but not exposed by this ${surface} (see docs/reference/client-contract.md §5)`,
    );
    this.name = "CapabilityNotAvailable";
    this.operation = operation;
    this.surface = surface;
  }
}

/** Write attempted over the read-only HTTP surface (contract §4). */
export class HttpWriteRefused extends EddaError {
  readonly operation: string;
  constructor(operation: string) {
    super(
      "HttpWriteRefused",
      `operation '${operation}' is a write; the SDK HTTP transport is read-only until HTTP write authorization lands (GH-609)`,
    );
    this.name = "HttpWriteRefused";
    this.operation = operation;
  }
}

/** Server returned a malformed JSON-RPC/HTTP response. */
export class ProtocolError extends EddaError {
  constructor(message: string) {
    super("ProtocolError", message);
    this.name = "ProtocolError";
  }
}

/** JSON-RPC error object returned by the server. */
export class RpcError extends EddaError {
  readonly code: number;
  readonly rpcMessage: string;
  readonly data?: unknown;
  constructor(code: number, rpcMessage: string, data?: unknown) {
    super("RpcError", `rpc error ${code}: ${rpcMessage}`);
    this.name = "RpcError";
    this.code = code;
    this.rpcMessage = rpcMessage;
    this.data = data;
  }
}
