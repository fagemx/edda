// Read-only HTTP transport against `edda serve` (/api/*).
//
// Writes are REFUSED by construction (contract §4): the HTTP write path is
// unauthenticated today and its authorization model depends on the signing
// ticket (GH-609), which is still design/spike. The SDK will not pretend
// otherwise.

import {
  CancelledError,
  HttpWriteRefused,
  ProtocolError,
  TimeoutError,
  TransportError,
} from "./errors.ts";

const WRITE_METHODS = new Set(["POST", "PUT", "PATCH", "DELETE"]);

export class HttpTransport {
  private baseUrl: string;
  private defaultTimeoutMs: number;

  constructor(baseUrl: string, defaultTimeoutMs = 30_000) {
    this.baseUrl = baseUrl;
    this.defaultTimeoutMs = defaultTimeoutMs;
  }

  private async request(
    method: string,
    path: string,
    opts: { timeoutMs?: number; signal?: AbortSignal } = {},
  ): Promise<unknown> {
    if (WRITE_METHODS.has(method.toUpperCase())) {
      throw new HttpWriteRefused(`${method} ${path}`);
    }
    const timeoutMs = opts.timeoutMs ?? this.defaultTimeoutMs;
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(new TimeoutError(`${method} ${path} exceeded ${timeoutMs}ms`)), timeoutMs);
    const onOuterAbort = () => controller.abort(new CancelledError());
    opts.signal?.addEventListener("abort", onOuterAbort, { once: true });

    try {
      const res = await fetch(`${this.baseUrl.replace(/\/$/, "")}${path}`, {
        method,
        signal: controller.signal,
        headers: { accept: "application/json" },
      });
      if (!res.ok) {
        throw new TransportError(`HTTP ${res.status} on ${path}`);
      }
      const text = await res.text();
      try {
        return JSON.parse(text);
      } catch {
        throw new ProtocolError(`non-JSON response on ${path}`);
      }
    } catch (err) {
      // fetch aborts surface as generic AbortError; re-surface the typed reason.
      if (controller.signal.aborted && controller.signal.reason instanceof Error) {
        throw controller.signal.reason;
      }
      throw err;
    } finally {
      clearTimeout(timer);
      opts.signal?.removeEventListener("abort", onOuterAbort);
    }
  }

  // ── Read operations (contract §2) ──

  /** GET /api/status */
  status(opts?: { timeoutMs?: number; signal?: AbortSignal }): Promise<unknown> {
    return this.request("GET", "/api/status", opts);
  }

  /** GET /api/decisions — decision query surface (list; filtered variants are server-side). */
  decisions(query = "", opts?: { timeoutMs?: number; signal?: AbortSignal }): Promise<unknown> {
    const qs = query ? `?${query}` : "";
    return this.request("GET", `/api/decisions${qs}`, opts);
  }

  /** GET /api/log */
  log(query = "", opts?: { timeoutMs?: number; signal?: AbortSignal }): Promise<unknown> {
    const qs = query ? `?${query}` : "";
    return this.request("GET", `/api/log${qs}`, opts);
  }

  /** GET /api/context */
  context(opts?: { timeoutMs?: number; signal?: AbortSignal }): Promise<unknown> {
    return this.request("GET", "/api/context", opts);
  }

  /** GET /api/health */
  health(opts?: { timeoutMs?: number; signal?: AbortSignal }): Promise<unknown> {
    return this.request("GET", "/api/health", opts);
  }
}
