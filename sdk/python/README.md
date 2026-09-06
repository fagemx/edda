# edda-sdk (draft name — publication pending namespace authorization)

Thin Python client for the edda client contract. Zero runtime dependencies
(stdlib only). Types are **generated** from the v1 event spec — do not
hand-edit `src/edda_sdk/types_gen.py` (generated on demand; see
`../generator/`).

- Writes go over **MCP** (`edda mcp serve`, stdio JSON-RPC 2.0).
- The HTTP transport is **read-only by construction** (write authorization
  pending, GH-609 — see the client contract §4).
- All contracted operations (task new/start/done, claim, receipt, verify)
  have MCP tools; `capabilities()` probes `tools/list` and
  `CapabilityNotAvailable` is raised for any operation a given server does
  not expose (contract §5).
- All operations accept `CallOptions(timeout_s=…, cancel=threading.Event())`
  and map failures to typed errors.

## Ten-line example

```python
from edda_sdk import EddaClient, McpSpawnSpec
client = EddaClient(mcp=McpSpawnSpec(command="edda", args=["mcp", "serve"], cwd="."))
caps = client.capabilities()
client.call("note", {"note": "hello from the SDK"})
client.call("decide", {"key": "db.engine", "value": "sqlite"})
found = client.call("ask", {"query": "db.engine"})
print(found)
client.close()
```

## Tests

```sh
EDDA_BIN=/path/to/edda python -m unittest discover -s tests   # contract tests
EDDA_SPEC_FIXTURES=<dir> python -m unittest tests.test_golden # golden fixtures
```

Version 0.1.0 targets spec v1 (Layer 1 stable types); Layer 2 types are
experimental (`Layer2Payload`). Version policy: client contract §3.
