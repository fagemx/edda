// EddaClient: the contracted operation surface (docs/reference/client-contract.md §2).
// Thin by design — transport + types only. No state rules.

import { McpTransport, type McpSpawnSpec, type CallOptions } from "./transport-mcp.ts";
import { HttpTransport } from "./transport-http.ts";
import { CapabilityNotAvailable } from "./errors.ts";

// Contracted operations. `capability` names the MCP tool that must exist for
// the operation to be available; the SDK probes tools/list and fails with a
// typed error rather than pretending a different tool covers it.
export const OPERATIONS = [
  "ask",
  "note",
  "decide",
  "task.new",
  "task.start",
  "task.done",
  "claim",
  "receipt",
  "verify",
  "status",
  "log",
  "context",
] as const;

export type OperationName = (typeof OPERATIONS)[number];

interface OpSpec {
  tool: string;
  args: (input: Record<string, unknown>) => Record<string, unknown>;
}

// Expected MCP tool names for contracted operations. Tools that do not exist
// on today's server (task/claim/receipt/verify) are still modeled here — the
// capability probe decides availability at runtime (contract §5).
const OPS: Record<OperationName, OpSpec> = {
  ask: { tool: "edda_ask", args: (i) => ({ query: i.query, ...opt(i, "domain") }) },
  note: { tool: "edda_note", args: (i) => ({ text: i.note, ...opt(i, "tags", "role") }) },
  decide: {
    tool: "edda_decide",
    args: (i) => ({
      decision: `${i.key}=${i.value}`,
      ...opt(i, "reason"),
    }),
  },
  "task.new": { tool: "edda_task_new", args: (i) => i },
  "task.start": { tool: "edda_task_start", args: (i) => i },
  "task.done": { tool: "edda_task_done", args: (i) => i },
  claim: { tool: "edda_claim", args: (i) => i },
  receipt: { tool: "edda_receipt", args: (i) => i },
  verify: { tool: "edda_verify", args: (i) => i },
  status: { tool: "edda_status", args: () => ({}) },
  log: { tool: "edda_log", args: (i) => opt(i, "event_type", "keyword", "after", "before", "limit") },
  context: { tool: "edda_context", args: () => ({}) },
};

function opt(i: Record<string, unknown>, ...keys: string[]): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const k of keys) if (i[k] !== undefined) out[k] = i[k];
  return out;
}

export class EddaClient {
  private readonly mcp: McpTransport | null;
  private readonly http: HttpTransport | null;

  constructor(opts: { mcp?: McpSpawnSpec; http?: string }) {
    if (!opts.mcp && !opts.http) {
      throw new Error("EddaClient requires an MCP spawn spec and/or an HTTP base URL");
    }
    this.mcp = opts.mcp ? new McpTransport(opts.mcp) : null;
    this.http = opts.http ? new HttpTransport(opts.http) : null;
  }

  /** Probe which contracted operations the server actually exposes. */
  async capabilities(opts?: CallOptions): Promise<Record<OperationName, boolean>> {
    const out = Object.fromEntries(OPERATIONS.map((op) => [op, false])) as Record<OperationName, boolean>;
    if (this.mcp) {
      const tools = await this.mcp.listTools(opts);
      const names = new Set(tools.map((t) => t.name));
      for (const op of OPERATIONS) out[op] = names.has(OPS[op].tool);
    }
    return out;
  }

  /** Run a contracted operation over MCP. Fails honestly if the tool is absent. */
  async call(
    op: OperationName,
    input: Record<string, unknown> = {},
    opts?: CallOptions,
  ): Promise<unknown> {
    if (!this.mcp) throw new CapabilityNotAvailable(op, "client (no MCP transport configured)");
    const spec = OPS[op];
    const tools = await this.mcp.listTools(opts);
    if (!tools.some((t) => t.name === spec.tool)) {
      throw new CapabilityNotAvailable(op, "server tool list");
    }
    return this.mcp.callTool(spec.tool, spec.args(input), opts);
  }

  // ── Read-only HTTP accessors ──
  get httpTransport(): HttpTransport {
    if (!this.http) throw new Error("no HTTP transport configured");
    return this.http;
  }

  async close(): Promise<void> {
    await this.mcp?.close();
  }
}

export { McpTransport, HttpTransport };
export * from "./errors.ts";
export * from "./canon.ts";
