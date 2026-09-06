# Edda Client Contract

Status: **v1-draft** — the operations and interface roles below are the client
contract for GH-611. The type source is the v1 event spec (#608); this page
pins which interface is canonical for which caller class and how versions move.
Compatibility policy background: `COMPATIBILITY.md`.

## 1. Canonical interfaces

| Interface | Role | Writes | Notes |
|---|---|---|---|
| **MCP** (`edda mcp serve`, stdio JSON-RPC 2.0) | **Canonical for agents** | yes | The tool list is the agent-facing capability surface. |
| **HTTP** (`edda serve`, `/api/*`) | **Read-only for SDK clients** (today) | SDK: no | Writes exist in the server but are gated on the signing ticket (#609); until that lands the SDK refuses HTTP writes by design (see §4). |
| **CLI `--json`** | Fallback / escape hatch | yes | Output shapes for stable verbs are pinned by golden fixtures (COMPATIBILITY.md §2). |

Thin SDKs (TypeScript, Python) sit on top of MCP and HTTP. They do transport
and types only — no business logic, no state rules. Event/decision state
machines live in the Rust core; the SDK never re-implements them.

## 2. Operations

The contracted operation set:

| Operation | MCP tool | HTTP (SDK use) | CLI fallback |
|---|---|---|---|
| `ask` (decision query) | `edda_ask` | `GET /api/decisions` (list) | `edda ask --json` |
| `note` | `edda_note` | — | `edda note` |
| `decide` | `edda_decide` | — | `edda decide` |
| `task new / start / done` | `edda_task_new` / `edda_task_start` / `edda_task_done` | — | `edda task new/start/done` |
| `claim` (scope claim; `claim check` remains CLI-only) | `edda_claim` | — | `edda claim` |
| `receipt` (task receipt on the ledger) | `edda_receipt` | — | `edda task done` (writes receipt event) |
| `verify` (ledger/hash verification) | `edda_verify` | — | `edda verify` |
| read: status / log / context | `edda_status`, `edda_log`, `edda_context` | `GET /api/status`, `GET /api/log`, `GET /api/context` | `edda status`, `edda log --json` |

## 3. Version and compatibility policy

- SDK packages are **0.x**; their minor versions track the event spec major
  they consume. SDK `0.1.x` targets **spec v1 (Layer 1)**.
- **Layer 1** event types (registry `stability: "stable-v1"`) and the event
  envelope are **stable**: a generated type may gain optional fields, never
  lose or retype contracted fields. Breaking a Layer 1 type requires a spec
  version bump and a matching SDK minor bump.
- **Layer 2** event types (registry `stability: "unstable"` — e.g. task,
  approval, verdict families) are **explicitly experimental**: they may change
  in any release. Generated Layer 2 types live in a separate module/namespace
  and are documented as unstable at the point of export.
- `schema_version` in the envelope is the event payload version (currently
  `1`); readers accept values `<=` the version they were generated against and
  must refuse newer ones loudly, mirroring the store policy in
  COMPATIBILITY.md §1. The envelope field is a `u32` — SDKs must preserve
  numeric fidelity when hashing/canonicalizing (see §6).

## 4. Write authorization

- Today all SDK **writes go through MCP** (or the CLI fallback). HTTP write
  endpoints (`POST /api/note`, `POST /api/decide`, …) exist but the SDK's HTTP
  transport is **read-only by construction** and returns a typed error for any
  write operation.
- This is deliberate: HTTP write authorization depends on the signed actor
  identity work (#609), which is still design/spike. When signing lands, the
  contract gains an authorized HTTP write path and this section is updated in
  the same release. The SDK will not pretend the current unauthenticated HTTP
  writes are an authorized surface.

## 5. MCP capability notes and remaining gap

The contracted operations are exposed as MCP tools backed by the shared,
validated service paths (the task state machine lives in
`edda_ledger::task_actions`, used by both CLI and MCP; claim writes go
through the bridge peers board; verify calls the ledger's chain verifier —
no state rules are duplicated):

- `edda_task_new` / `edda_task_start` / `edda_task_done` / `edda_task_fail`
  — task rail (idempotency keys, start/done pairing, receipt requirement are
  enforced by the shared state machine, not by the SDK or the tool layer).
- `edda_claim` — writes a coordination-scope claim on the session board
  (one claim per session; a lost write is never reported as success). The
  glob-intersection `claim check` remains a CLI-only surface for now; it is
  the sole operation-table gap and is not represented as SDK capability.
- `edda_receipt` — reads the receipt recorded by `task done` (receipts have
  no separate write path: "done without a receipt does not exist").
- `edda_verify` — read-only hash-chain verification, same payload as
  `edda verify --json`.

SDKs probe `tools/list` and fail with a typed `CapabilityNotAvailable` for
any contracted operation a given server does not expose.

## 6. Types, canonicalization and fidelity

- SDK types are **generated** from the canonical JSON Schema published by the
  event spec (#608) at a **pinned commit**. Hand-written duplicates of spec
  types are not allowed in the SDK packages.
- Canonical form (`edda-canon-v1`): recursive lexicographic key sort (byte
  order), arrays in order, compact separators. Event hash =
  SHA-256 of the canonical JSON of the event **excluding** the top-level
  `hash`, `digests` and `schema_version` keys.
- SDKs compute this independently (no Rust code, no shelling out to `edda`)
  and must preserve the raw numeric lexeme (integers up to `u32`/`u64`,
  float spelling) when re-serializing for hashing. Golden fixtures from the
  spec repo pin the digests; both SDKs recompute and must match them.
- Transport errors, timeouts and cancellation are part of the contract: every
  operation accepts a deadline/cancellation token and maps transport failures
  to typed errors (`Timeout`, `Cancelled`, `TransportError`,
  `CapabilityNotAvailable`).

## 7. Conformance testing

Contract tests run both SDKs against a **real built `edda`** on a temp repo
with an isolated store (`EDDA_STORE_ROOT`) and require structural equivalence
of results across the two languages. CI runs this on one OS as a focused job;
existing coverage is unchanged.
