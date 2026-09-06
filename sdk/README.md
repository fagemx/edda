# Edda SDKs

Thin TypeScript and Python clients for the
[edda client contract](../docs/reference/client-contract.md): MCP for agent
writes, HTTP read-only (until write authorization lands, GH-609), types
**generated** from the v1 event spec (#608) at a pinned commit.

```
sdk/
  generator/            type generator + spec pinning (no deps)
  ts/                   TypeScript client (@fagem/edda-sdk)
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
| Publication | packages are publishable; **the publish itself needs the operator's registry accounts** — see below |

See `SDK_HANDOFF.md` at the repo root for open controller decisions.

## Publishing the first version (operator)

Everything a repository can settle is settled: both packages build, carry
dual MIT/Apache-2.0 licences, and pass their registries' own validators
(`npm pack`, `twine check`). What is left needs credentials this repository
does not hold, so it is the operator's to run.

The npm package is `@fagem/edda-sdk` — the maintainer's own npm user scope,
which exists by virtue of the account and needs no organisation to be created
first. It deliberately does not match the GitHub org (`fagemx`): matching that
would mean creating an npm organisation and waiting on it, and the scope is
transferable to one later if the project ever wants that. PyPI has no scopes,
so the Python package is plain `edda-sdk`.

`@fagem/edda-sdk` and PyPI `edda-sdk` were both unclaimed when this was
written; verify before assuming.

```sh
# TypeScript — npm ci first: prepublishOnly runs tsc, which a fresh clone
# does not have until devDependencies are installed. publishConfig sets
# public access.
cd sdk/ts && npm ci && npm login && npm publish

# Python
cd sdk/python && python -m build && python -m twine upload dist/*
```

`0.1.0` is deliberate: the SDKs track **spec v1** at the commit pinned in
`SPEC_PIN.json`, and stay `0.x` until the client contract is frozen
(`docs/reference/client-contract.md`).

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
