# Edda SDKs

Thin TypeScript and Python clients for the
[edda client contract](../docs/reference/client-contract.md): MCP for agent
writes, HTTP read-only (until write authorization lands, GH-609), types
**generated** from the v1 event spec (#608) at a pinned commit.

```
sdk/
  generator/            type generator + spec pinning (no deps)
  ts/                   TypeScript client (@edda/sdk draft)
  python/               Python client (edda-sdk draft)
  run-contract-tests.mjs  cross-language contract runner
  spec-pin/             pinned spec checkout (created by pin-spec.sh; gitignored)
```

## Status

| Piece | Status |
|---|---|
| Client contract doc | written (`docs/reference/client-contract.md`) |
| Generators (TS + Py) | ready — consume pinned spec on controller handoff |
| Canon/hash (TS + Py) | implemented, independently; verify all golden fixtures |
| Transports (MCP/HTTP) | implemented; MCP writes, HTTP read-only, typed timeout/cancel |
| Contract tests | both languages + cross-language equivalence runner (task/receipt/claim/verify included) |
| Spec pin | pinned: `9e3f6ddb8660e730be2cee631aa1eff7dd208a18` (sdk/SPEC_PIN.json) |
| Publication | **blocked: no npm/PyPI namespace/account authorization** (no NPM_TOKEN, `npm whoami` → ENEEDAUTH, no TWINE_PASSWORD) |

See `SDK_HANDOFF.md` at the repo root for open controller decisions.

## Running the contract tests

```sh
# the pinned spec is recorded in SPEC_PIN.json; materialize it locally:
bash generator/pin-spec.sh "$(node -p "require('./SPEC_PIN.json').spec_sha")"

# with a built edda binary:
EDDA_BIN=/path/to/edda node run-contract-tests.mjs
```

The runner generates both type modules, runs golden fixture tests
(independent canon + SHA-256 recomputation against pinned digests), runs both
contract suites against a real edda on temp repos with isolated stores
(`EDDA_STORE_ROOT`), and requires structural equivalence of the TS and Python
scenario transcripts.

## Ten-line example (TypeScript)

```ts
import { EddaClient } from "./sdk/ts/src/index.ts";
const client = new EddaClient({ mcp: { command: "edda", args: ["mcp", "serve"], cwd: "." } });
const caps = await client.capabilities();          // honest probe — missing tools reported
await client.call("note", { note: "hello from the SDK" });
await client.call("decide", { key: "db.engine", value: "sqlite" });
const found = await client.call("ask", { query: "db.engine" });
console.log(found);
await client.close();
```

## Ten-line example (Python)

```python
from edda_sdk import EddaClient, McpSpawnSpec
client = EddaClient(mcp=McpSpawnSpec(command="edda", args=["mcp", "serve"], cwd="."))
caps = client.capabilities()                # honest probe — missing tools reported
client.call("note", {"note": "hello from the SDK"})
client.call("decide", {"key": "db.engine", "value": "sqlite"})
found = client.call("ask", {"query": "db.engine"})
print(found)
client.close()
```
