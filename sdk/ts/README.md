# @edda/sdk (draft name — publication pending namespace authorization)

Thin TypeScript client for the edda client contract. Zero runtime
dependencies. Types are **generated** from the v1 event spec — do not
hand-edit `src/types.gen.ts` (generated on demand; see `../generator/`).

- Writes go over **MCP** (`edda mcp serve`, stdio JSON-RPC 2.0).
- The HTTP transport is **read-only by construction** (write authorization
  pending, GH-609 — see the client contract §4).
- All contracted operations (task new/start/done, claim, receipt, verify)
  have MCP tools; `capabilities()` probes `tools/list` and
  `CapabilityNotAvailable` is raised for any operation a given server does
  not expose (contract §5).
- All operations accept `timeoutMs` / `AbortSignal` and map failures to
  typed errors (`TimeoutError`, `CancelledError`, `TransportError`, …).

## Ten-line example

```ts
import { EddaClient } from "@edda/sdk";
const client = new EddaClient({ mcp: { command: "edda", args: ["mcp", "serve"], cwd: "." } });
const caps = await client.capabilities();
await client.call("note", { note: "hello from the SDK" });
await client.call("decide", { key: "db.engine", value: "sqlite" });
const found = await client.call("ask", { query: "db.engine" });
console.log(found);
await client.close();
```

## Tests

```sh
EDDA_BIN=/path/to/edda npm test          # contract tests (skipped without EDDA_BIN)
EDDA_SPEC_FIXTURES=<dir> npm run test:golden   # golden fixture hash tests
```

Version 0.1.0 targets spec v1 (Layer 1 stable types); Layer 2 types are
experimental (`Layer2Payload`). Version policy: client contract §3.
