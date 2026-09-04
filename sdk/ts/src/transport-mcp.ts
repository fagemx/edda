// MCP transport: JSON-RPC 2.0 over stdio against `edda mcp serve`
// (newline-delimited JSON, rmcp stdio framing).
//
// Safety: the child process is spawned from a FIXED argv array supplied by
// the caller (binary path + fixed args). This module never interpolates
// user input into a shell command and never uses a shell.

import { spawn, type ChildProcess } from "node:child_process";
import {
  CancelledError,
  ProtocolError,
  RpcError,
  TimeoutError,
  TransportError,
} from "./errors.ts";

export interface McpSpawnSpec {
  /** Absolute or resolvable path to the edda binary. Never a shell string. */
  command: string;
  /** Fixed argument list, e.g. ["mcp", "serve"]. No shell interpolation. */
  args: string[];
  cwd?: string;
  env?: Record<string, string>;
}

export interface CallOptions {
  /** Deadline in milliseconds; the child is killed and TimeoutError raised. */
  timeoutMs?: number;
  /** Cancellation signal; the child is killed and CancelledError raised. */
  signal?: AbortSignal;
}

interface JsonRpcRequest {
  jsonrpc: "2.0";
  id: number;
  method: string;
  params?: unknown;
}

interface JsonRpcResponse {
  jsonrpc: "2.0";
  id: number | null;
  result?: unknown;
  error?: { code: number; message: string; data?: unknown };
}

export interface McpToolInfo {
  name: string;
  description?: string;
}

export class McpTransport {
  private child: ChildProcess | null = null;
  private nextId = 1;
  private pending = new Map<
    number,
    { resolve: (v: unknown) => void; reject: (e: Error) => void; timer: NodeJS.Timeout | null }
  >();
  private buffer = "";
  private toolsCache: McpToolInfo[] | null = null;
  private initDone: Promise<void> | null = null;
  private spec: McpSpawnSpec;

  constructor(spec: McpSpawnSpec) {
    this.spec = spec;
  }

  private ensureChild(): ChildProcess {
    if (this.child && this.child.exitCode === null) return this.child;
    const child = spawn(this.spec.command, this.spec.args, {
      cwd: this.spec.cwd,
      env: this.spec.env ? { ...process.env, ...this.spec.env } : process.env,
      shell: false, // never a shell — fixed argv only
      stdio: ["pipe", "pipe", "pipe"],
      windowsHide: true,
    });
    child.on("error", (err) => {
      this.failAll(new TransportError(`failed to spawn ${this.spec.command}`, err));
    });
    child.stdout!.setEncoding("utf8");
    child.stdout!.on("data", (chunk: string) => {
      this.buffer += chunk;
      let idx: number;
      while ((idx = this.buffer.indexOf("\n")) >= 0) {
        const line = this.buffer.slice(0, idx).trim();
        this.buffer = this.buffer.slice(idx + 1);
        if (line) this.handleLine(line);
      }
    });
    child.stderr!.setEncoding("utf8");
    child.stderr!.on("data", (chunk: string) => {
      // stderr is diagnostic only; rmcp logs there. Kept for debugging.
      this.lastStderr = (this.lastStderr + chunk).slice(-8192);
    });
    child.on("exit", (code) => {
      this.failAll(
        new TransportError(`edda mcp exited with code ${code}${this.lastStderr ? `: ${this.lastStderr.trim()}` : ""}`),
      );
    });
    this.child = child;
    return child;
  }

  private lastStderr = "";

  private failAll(err: Error): void {
    for (const [, p] of this.pending) {
      if (p.timer) clearTimeout(p.timer);
      p.reject(err);
    }
    this.pending.clear();
  }

  private handleLine(line: string): void {
    let msg: JsonRpcResponse;
    try {
      msg = JSON.parse(line) as JsonRpcResponse;
    } catch {
      // Non-JSON lines are ignored (rmcp may emit benign output).
      return;
    }
    if (msg.id === null || msg.id === undefined) return; // notification
    const entry = this.pending.get(msg.id as number);
    if (!entry) return;
    this.pending.delete(msg.id as number);
    if (entry.timer) clearTimeout(entry.timer);
    if (msg.error) {
      entry.reject(new RpcError(msg.error.code, msg.error.message, msg.error.data));
    } else {
      entry.resolve(msg.result);
    }
  }

  private async request(method: string, params: unknown, opts: CallOptions = {}): Promise<unknown> {
    const child = this.ensureChild();
    if (opts.signal?.aborted) throw new CancelledError();

    const id = this.nextId++;
    const req: JsonRpcRequest = { jsonrpc: "2.0", id, method, params };
    return new Promise<unknown>((resolve, reject) => {
      const timer =
        opts.timeoutMs != null
          ? setTimeout(() => {
              this.pending.delete(id);
              reject(new TimeoutError(`${method} exceeded ${opts.timeoutMs}ms`));
            }, opts.timeoutMs)
          : null;
      this.pending.set(id, { resolve, reject, timer });
      const onAbort = () => {
        if (this.pending.has(id)) {
          this.pending.delete(id);
          if (timer) clearTimeout(timer);
          reject(new CancelledError());
        }
      };
      opts.signal?.addEventListener("abort", onAbort, { once: true });
      child.stdin!.write(JSON.stringify(req) + "\n", (err) => {
        if (err) {
          this.pending.delete(id);
          if (timer) clearTimeout(timer);
          reject(new TransportError("write to edda mcp stdin failed", err));
        }
      });
    });
  }

  /** MCP initialize handshake (idempotent; a failed handshake is retried). */
  async initialize(opts: CallOptions = {}): Promise<void> {
    if (!this.initDone) {
      this.initDone = (async () => {
        await this.request(
          "initialize",
          {
            protocolVersion: "2025-03-26",
            capabilities: {},
            clientInfo: { name: "edda-sdk-ts", version: "0.1.0" },
          },
          opts,
        );
        this.notify("notifications/initialized");
      })();
      // A failed handshake must not poison the client: clear the cache so the
      // next call retries (e.g. after a timeout or cancellation probe).
      const p = this.initDone;
      this.initDone.catch(() => {
        if (this.initDone === p) this.initDone = null;
      });
    }
    return this.initDone;
  }

  /** Fire-and-forget JSON-RPC notification (no id, no response). */
  private notify(method: string, params: unknown = {}): void {
    const child = this.ensureChild();
    const msg = { jsonrpc: "2.0", method, params };
    child.stdin!.write(JSON.stringify(msg) + "\n");
  }

  /** List tools (used to probe capabilities honestly; result cached). */
  async listTools(opts: CallOptions = {}): Promise<McpToolInfo[]> {
    await this.initialize(opts);
    if (!this.toolsCache) {
      const result = (await this.request("tools/list", {}, opts)) as { tools?: McpToolInfo[] };
      this.toolsCache = result?.tools ?? [];
    }
    return this.toolsCache;
  }

  /** Invoke an MCP tool by name; returns the parsed result content. */
  async callTool(
    name: string,
    args: Record<string, unknown>,
    opts: CallOptions = {},
  ): Promise<unknown> {
    await this.initialize(opts);
    const result = (await this.request("tools/call", { name, arguments: args }, opts)) as {
      content?: Array<{ type: string; text?: string }>;
      isError?: boolean;
    };
    if (result?.isError) {
      const text = result.content?.find((c) => c.type === "text")?.text ?? "tool error";
      throw new RpcError(-32000, text);
    }
    // Tool results carry a JSON payload as text content; parse when present.
    const text = result?.content?.find((c) => c.type === "text")?.text;
    if (text == null) return result;
    try {
      return JSON.parse(text);
    } catch {
      return text;
    }
  }

  /** Close the child process. */
  async close(): Promise<void> {
    const child = this.child;
    this.child = null;
    if (child && child.exitCode === null) {
      const exited = new Promise<void>((resolve) => child.on("exit", () => resolve()));
      child.stdin!.end();
      child.kill();
      await Promise.race([exited, new Promise<void>((r) => setTimeout(r, 2000))]);
    }
  }
}
