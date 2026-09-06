# Ledger event specification v1

This is the written contract for the existing Layer 1 ledger format (#608).
It specifies the current unsigned envelope and hash algorithm, and inventories
all current ledger event types. Presence in the inventory does **not** make a
Layer 2/3 payload stable. The operator's v1 scope ruling is recorded on
[GH-608](https://github.com/fagemx/edda/issues/608#issuecomment-5506324281).
The compatibility policy is [COMPATIBILITY.md](../../COMPATIBILITY.md),
especially §1's separate store/event version namespaces and §3's unstable
coordination objects. This document does not redefine the store migration or
0.x release policy.

Machine-readable input for SDK generation is
[`spec/events/registry.json`](../../spec/events/registry.json), the shared
[`envelope.schema.json`](../../spec/events/envelope.schema.json), and one
`<type>.schema.json` per payload. Schemas use JSON Schema draft 2020-12, allow
unknown properties, and describe read shapes rather than impose new runtime
validation. Payload schemas are not runtime validators in `append_event`.

## Envelope

The Rust read/write type is `Event` in
[`crates/edda-core/src/types.rs:358`](../../crates/edda-core/src/types.rs#L358).
One complete event is a JSON object with these fields:

| Field | JSON type | Meaning / read default |
|---|---|---|
| `event_id` | string | Identifier. Core normally emits `evt_` plus a lowercase ULID. Execution and telemetry callers can supply external IDs; a ULID regex is not a universal constraint. |
| `ts` | string | Producer time, normally UTC RFC 3339; external execution/telemetry timestamps are passed through. Ordering uses insertion order, not timestamp. |
| `type` | string | Payload discriminator; Rust name is `event_type`. Registry below. |
| `branch` | string | Logical ledger branch. It does not define a separate hash chain. |
| `parent_hash` | string or null | Previous global event's hash. Missing deserializes as `None`; writers serialize null for the first event. |
| `hash` | string | Lowercase 64-hex SHA-256 of the projection in [Canonical hash](#canonical-hash). |
| `payload` | any JSON value | Type-specific content. Most producers emit objects; generic execution/telemetry constructors accept any JSON. |
| `refs` | object | References; absent reads as `{}`. Serialization always emits the object. |
| `schema_version` | unsigned 32-bit integer | Event envelope version, currently 1; absent reads as 0. This is **not** SQLite `schema_meta.version`. Excluded from hash. |
| `digests` | array | Digest records; absent/empty reads as `[]`, empty is omitted on serialization. Excluded from hash. |
| `event_family` | string or null | Optional taxonomy; absent/null omitted on serialization. Included when present in hash input. |
| `event_level` | string or null | Optional taxonomy; same serialization rule. |

`refs.blobs` and `refs.events` are arrays of string references.
`refs.provenance` is an array of `{target, rel, note?}` objects. All three
arrays default to empty and are omitted when empty. `note` defaults to absent.
Targets are references, not necessarily rows in this ledger (for example
`session:<id>` or an imported event ID). Common relations include `based_on`,
`supersedes`, `continues`, `reviews`, `depends_on`, and `imported_from`;
`rel` remains a string. References are integrity-protected as event content;
chain verification does not fetch referenced objects or prove their existence.

Each current digest is `{alg: "sha256", canon: "edda-canon-v1", value: hash}`.
Current finalization replaces the whole digest array with exactly one such
record. `digests` is algorithm metadata, not an independent signature.

Taxonomy is deterministically assigned by `classify_event_type` in
[`types.rs:94`](../../crates/edda-core/src/types.rs#L94): signal, milestone,
admin or governance family; trace, info, milestone or governance level.
`execution_event` and `ingestion` currently have no taxonomy. An unknown type
also has none. Finalization overwrites caller taxonomy before hashing;
verification requires stored taxonomy to equal that result.

## Event types and payloads

Every row links the actual payload schema. The registry is checked against
production Rust constructors, assignments and task-builder calls. The table
and registry must change in the same patch when a new producer is added.

| Type | Stability | Payload semantics / schema |
|---|---|---|
| `note` | Layer 1 v1 | [Readable text, role and tags; optional structured decision or session digest](../../spec/events/note.schema.json) |
| `checkpoint` | Layer 1 v1 | [Hypotheses, rejected hypotheses with reasons, open questions and next action](../../spec/events/checkpoint.schema.json) |
| `decision_ratify` | Layer 1 v1 | [Operator ratification of a decision key](../../spec/events/decision_ratify.schema.json) |
| `decision_import` | Layer 1 v1 | [Decision plus source project and source event provenance](../../spec/events/decision_import.schema.json) |
| `cmd` | Layer 1 v1 | [argv, cwd, exit code, duration and stdout/stderr blob references](../../spec/events/cmd.schema.json) |
| `commit` | Layer 1 v1 | [Ledger milestone title, purpose, summary, contribution, evidence and labels](../../spec/events/commit.schema.json) |
| `rebuild` | Layer 1 v1 | [View rebuild scope, branch and reason](../../spec/events/rebuild.schema.json) |
| `branch_create` | Layer 1 v1 | [Name, purpose, source branch and source event](../../spec/events/branch_create.schema.json) |
| `branch_switch` | Layer 1 v1 | [Previous and next branch](../../spec/events/branch_switch.schema.json) |
| `merge` | Layer 1 v1 | [Source/destination branch, reason and adopted commit IDs](../../spec/events/merge.schema.json) |
| `approval` | unstable | [Draft hash, actor, decision, role and stage](../../spec/events/approval.schema.json) |
| `approval_request` | unstable | [Draft hash, routing rule, stage and assignees](../../spec/events/approval_request.schema.json) |
| `approval_policy_match` | unstable | [Task/step policy evaluation result](../../spec/events/approval_policy_match.schema.json) |
| `task_intake` | unstable | [External source, title, intent, priority and constraints](../../spec/events/task_intake.schema.json) |
| `agent_phase_change` | unstable | [Session phase transition, confidence and signals](../../spec/events/agent_phase_change.schema.json) |
| `review_bundle` | unstable | [Changes, tests, risk and suggested review action](../../spec/events/review_bundle.schema.json) |
| `review_verdict` | unstable | [SHA-pinned independent review outcome and evidence](../../spec/events/review_verdict.schema.json) |
| `pr` | unstable | [Pull request status and review/merge metadata](../../spec/events/pr.schema.json) |
| `verdict.recorded` | unstable | [Subject, approved/rejected decision, full SHA and actor](../../spec/events/verdict.recorded.schema.json) |
| `device_pair` | unstable | [Device name, pairing address and token hash prefix](../../spec/events/device_pair.schema.json) |
| `device_revoke` | unstable | [Device name or revoke-all flag](../../spec/events/device_revoke.schema.json) |
| `task.created` | unstable | [Task ID, title, dependencies and optional routing/scope](../../spec/events/task.created.schema.json) |
| `task.started` | unstable | [Task ID, lease TTL and attempt](../../spec/events/task.started.schema.json) |
| `task.session` | unstable | [Legacy ACP session or host-neutral session ID with agent kind and attempt](../../spec/events/task.session.schema.json) |
| `task.done` | unstable | [Task ID, nonblank receipt and evidence paths](../../spec/events/task.done.schema.json) |
| `task.failed` | unstable | [Task ID and failure reason](../../spec/events/task.failed.schema.json) |
| `task.requeued` | unstable | [Task ID and next attempt](../../spec/events/task.requeued.schema.json) |
| `execution_event` | unstable | [External execution envelope; HTTP writer uses Karvi fields](../../spec/events/execution_event.schema.json) |
| `decide_snapshot` | unstable | [Context hash, engine version and inline/blob data](../../spec/events/decide_snapshot.schema.json) |
| `cycle_telemetry` | unstable | [Cycle duration, operations, usage and cost](../../spec/events/cycle_telemetry.schema.json) |
| `ingestion` | unstable | [Camel-case ingestion record with source layer and references](../../spec/events/ingestion.schema.json) |

`decide` is a command, not an event type: it writes a `note` with a `decision`
object and a `decision` tag. That object contains required `key` and `value`,
optional reason, scope, authority, affected paths, tags, review date,
reversibility and village ID. Authority is descriptive provenance, not proof
of ratification; `decision_ratify` is a separate fact. `session_digest` is
also a `note` tag, with `source: bridge:session_digest`, `session_id`,
`session_stats`, and optional replay watermark. Plain note and these variants
share one schema; optional fields do not make the variants new types.

The registry inventories the `Event` envelope only. `coordination.jsonl`
claims, requests, bindings and acks use `CoordEvent`, a different protocol.
Conductor plan/phase logs, telemetry's nested `event_type`, and proposed
finding/strategy/campaign events are not additional current ledger types.
Future designs must not be added as producers until implemented.

## Canonical hash

The algorithm is `edda-canon-v1`, implemented by
[`finalize` in event.rs:32](../../crates/edda-core/src/event.rs#L32),
[`canonical_json_bytes` in canon.rs:5](../../crates/edda-core/src/canon.rs#L5),
and [`sha256_hex` in hash.rs:4](../../crates/edda-core/src/hash.rs#L4).

1. Populate taxonomy from the type. Serialize the typed `Event` as a JSON
   value using the omission/default rules above. Do not hash the original
   whitespace or the original key order from a JSONL line.
2. Remove exactly the **top-level** keys `hash`, `digests`, `schema_version`.
   Identically named keys inside `payload` are retained. `refs`, `branch`,
   `parent_hash`, ID, timestamp and present taxonomy remain covered.
3. Recursively sort every object by Rust string lexicographic order (Unicode
   scalar/UTF-8 order); arrays preserve element order. There is no Unicode
   normalization. This differs from UTF-16 key sorting and is not RFC 8785 JCS.
4. Serialize with `serde_json::to_vec`: compact UTF-8 JSON, no BOM, no
   whitespace separators and **no terminal newline**. Strings escape quote,
   backslash and U+0000–U+001F; the short escapes are `\b`, `\t`, `\n`, `\f`,
   `\r`, and other control characters use lowercase `\u00xx`. Slash and
   other Unicode remain unescaped. Null and booleans use JSON lowercase.
5. Numbers follow `serde_json::Value`'s number representation: signed/unsigned
   64-bit integers serialize as decimal integers; floating values use the
   serializer's finite f64 representation, preserving floating `.0` and
   negative zero. Exponent spelling and f64 parsing/rounding matter. This is
   **not** arbitrary-precision decimal canonicalization; an independent
   implementation must match the canonical byte vectors, not substitute JCS
   or blindly rely on its default JSON serializer. Byte vectors are in
   [`canonical-v1.json`](../../spec/events/canonical-v1.json).
6. Compute SHA-256 of those bytes and encode the 32 bytes as 64 lowercase hex
   digits. Store as `hash` and as the single digest's `value`.

`compute_event_hash` hashes its supplied JSON value directly: field removal
is performed by `finalize`, not by that helper. To verify an event, clone it
and call `finalize_event`; retain the original for comparisons.

## Chain verification

<a id="chain-verification"></a>

[`Ledger::verify_chain` in ledger.rs:205](../../crates/edda-ledger/src/ledger.rs#L205)
delegates to [`SqliteStore::verify_chain` in events.rs:715](../../crates/edda-ledger/src/sqlite_store/events.rs#L715).
Both reference this algorithm:

1. Read all events in SQLite insertion (`rowid`) order. Empty is valid.
2. For each event, compute the canonical hash above and compare original
   `hash`, the complete digest array, and taxonomy against finalization.
3. The first event must have `parent_hash = null`. Each later event must
   reference the immediately preceding event's `hash`, even across branch
   switches and merges. Branches label content; they do not fork the chain.

Append checks the current global tail inside a SQLite transaction, rejecting
stale parents and invalid hashes/digests/taxonomy. `verify_chain_report`
reports the first bad event; the older `verify_chain` checks hashes before
linkage and its first returned error need not follow that reporting order.
Successful verification proves internal integrity, not author identity or
truth of payload assertions. The readable-payload boundary additionally
rejects native reasoning content, permitting a readable summary or a pointer
plus content hash; this policy is separate from canonicalization.

## Persistence and JSONL layout

The current authority is `.edda/ledger.db` (`events` rows). SQLite is **not
merely an index over events.jsonl**: `sqlite_store/mod.rs` explicitly replaced
the old file-backed event store. Event columns reconstruct the typed
envelope; query indexes and materialized decisions/bundles/task views are
derived from it. Rebuildable branch Markdown, search indexes and human
exports do not supersede the ledger.

Interchange fixtures use UTF-8 JSONL: one complete JSON event object per
nonempty line, LF terminated, in insertion order when the file represents a
chain. JSON strings escape embedded newlines. Fixture files in
[`tests/fixtures/events`](../../tests/fixtures/events) each begin with an
independent first event unless subsequent lines explicitly link back.
Do not concatenate independent fixture files and call the result a chain.
The newline framing byte is never part of an event hash.

`.edda/ledger/blobs/` stores content-addressed blobs. Tombstones and
`coordination.jsonl` are distinct JSONL protocols. `edda export` is a
human-readable Markdown projection, not a lossless JSONL ledger export.
This specification defines interchange shape; it does not introduce a new
import/export command or promise that a live `events.jsonl` is maintained.

## Compatibility and extension boundary

For version/release and stable CLI JSON policies, use
[COMPATIBILITY.md](../../COMPATIBILITY.md). Event version 1 is independent of
the store version. Current Rust deserialization defaults missing event
`schema_version` to 0 and does not itself reject a larger event version.
The store's refuse-newer rule must not be misreported as an event validator.
Readers may parse legacy events lacking optional fields, but parsing alone
does not imply that current strict digest/taxonomy verification will pass.

Current `Event` deserialization ignores unknown envelope fields, and a typed
round trip drops them; unknown `payload` properties are retained and hashed.
Unknown types remain strings and may be stored with no taxonomy; generic
readers must retain their opaque payload and avoid interpreting them as a
known type. The source registration test prevents accidental **repository
producer** additions, not external unknown-type storage. A relay that needs
lossless forwarding of an extended envelope must preserve the raw object and
negotiate that version instead of claiming typed v1 forwarding is lossless.

V1 remains unsigned. Proposed `actor_id`, `sig`, and `key_id` fields belong
to the explicit **v1.1 proposal** tracked in
[issue #609](https://github.com/fagemx/edda/issues/609), not the v1 hash
projection or current schemas. No production signing
or authentication behavior is enabled by this specification.

## Conformance and proof status

`cargo test -p edda-core event_conformance` parses each registered fixture,
validates schemas and payloads, recomputes hashes/digests/taxonomy, and checks
the registry against production Rust syntax across `crates/`. Source scanning
recognizes direct/qualified envelope literals, type-field assignments and
the shared task builder. An unclassified dynamic producer fails closed;
audited SQLite reconstruction functions are explicit exceptions. This is an
authoring guard, not a complete Rust semantic analysis of arbitrary macros.
Custom macro-generated event writers require the same registration audit.

The fixtures are language-neutral. A second implementation must parse and
round-trip them, reproduce canonical bytes/hashes, preserve payload fields,
and verify linked branches before claiming independent interoperability.
Rust's own tests alone do not prove the v1 contract across implementations;
#611 or an external adapter supplies that independent evidence.
