# Compatibility Policy

This page is the compatibility contract for edda in 0.x. It transcribes the
operator ruling of 2026-09-02 on
[GH-651](https://github.com/fagemx/edda/issues/651) — held in the decision
ledger as `compat.schema-version-policy=read-older-refuse-newer-minor-bump-announced`
and `compat.stable-json-surfaces=dispatch-verify-ask-status-mcp` — into policy
form. The lane does not re-decide; this page records.

It is not prose backed by nothing: every stable `--json` surface declared in
§2 has a golden-fixture test that pins its key set and types. Renaming or
retyping a contracted key turns the fixture red, so this page cannot silently
go stale — if a fixture fails, the contract moved and this page must ship
updated in the same release.

Source citations are checked by the [documentation citation gate](docs/guides/doc-citations.md).
Literal anchors detect when a cited range no longer contains its named source.

## 1. `schema_version` upgrade policy

Ledger decision: `compat.schema-version-policy=read-older-refuse-newer-minor-bump-announced`

### 1.1 Which `schema_version` this policy governs

Two version namespaces exist in the tree; the policy governs the **ledger
store version**:

- **Ledger store version** — `schema_meta.version` in the SQLite ledger
  (`schema_meta` defined at
  `crates/edda-ledger/src/sqlite_store/schema.rs:76-79#CREATE TABLE IF NOT EXISTS schema_meta (`, read/written at
  `crates/edda-ledger/src/sqlite_store/schema.rs:343-356#pub(super) fn schema_version(&self) -> anyhow::Result<u32> {`). This is the number a binary compares against itself
  when opening a ledger. The recorded history is a migration ladder from v1
  to v13 (`crates/edda-ledger/src/sqlite_store/schema.rs:250-341#pub(super) fn apply_schema(&self) -> anyhow::Result<()> {`): v5 added cross-project sync fields, v6 the
  `task_briefs` view, v7 `device_tokens`, v8 `decide_snapshots`, v9 hot-path
  indexes, v10 decision deepening columns, v11 `village_id`, v12 the
  suggestions queue, v13 the `task_leases` table
  (`crates/edda-ledger/src/sqlite_store/schema.rs:217-226#pub(super) const SCHEMA_V13_SQL: &str = "`).
- **Event payload version** — `edda_core::SCHEMA_VERSION`
  (`crates/edda-core/src/types.rs:4#pub const SCHEMA_VERSION: u32 = 1;`, stamped into every event payload at
  `crates/edda-core/src/event.rs:186#schema_version: SCHEMA_VERSION,` and siblings). This has been `1` for
  the project's entire history and is part of the Layer 1 event format, whose
  stability is governed by the v1 event spec (#608), not by this page.
- Decide-snapshot rows additionally carry a per-row
  `"schema_version": "snapshot.v1"` string
  (`crates/edda-ledger/src/sqlite_store/schema.rs:152#schema_version  TEXT NOT NULL DEFAULT 'snapshot.v1',`); it identifies the
  snapshot shape, not the ledger.

### 1.2 The policy

- **Read older.** A binary may open any ledger whose store version is **≤ its
  own**. Old ledgers are migrated forward on open, never rewritten in place at
  the event level: the raw events in the ledger are immutable, and changes to
  derived views go through `edda rebuild`
  (`crates/edda-cli/src/cmd_rebuild.rs`). This matches the ladder as it
  actually works today — `Ledger::open` → `SqliteStore::open_or_create` →
  `apply_schema` runs every missing migration upward
  (`crates/edda-ledger/src/ledger.rs:32-41#/// Open an existing workspace. Fails if`,
  `crates/edda-ledger/src/sqlite_store/mod.rs:45-53#pub fn open_or_create(db_path: &Path) -> anyhow::Result<Self> {`,
  `crates/edda-ledger/src/sqlite_store/schema.rs:250-341#pub(super) fn apply_schema(&self) -> anyhow::Result<()> {`).
- **Refuse newer.** A ledger with a store version **newer** than the binary
  is refused to open (exit 2, with a message naming both version numbers).
  Refusing is deliberate: the alternative is silently misinterpreting a
  ledger we cannot correctly read.
  **Enforced as of #735 (GH-729, closed):** opening a ledger checks the
  store version against `MAX_KNOWN_SCHEMA_VERSION` (13) and fails with
  `UnsupportedSchemaVersionError` (`crates/edda-ledger/src/sqlite_store/schema.rs:228-247#/// The maximum schema version known and supported by this binary.`); the message names
  both the stored and the maximum supported version, and `edda verify`
  maps it to exit 2 (`crates/edda-cli/src/cmd_verify.rs:40-43#if let Some(e) = err.downcast_ref::<edda_ledger::UnsupportedSchemaVersionError>() {`). The
  contract is pinned by `crates/edda-cli/tests/schema_refusal_contract.rs`.
- **Minor bump, announced.** A `schema_version` jump is a **0.x minor bump**
  (e.g. 0.4 → 0.5), not a major-version event. The release that bumps the
  store version ships an updated version of this page documenting the
  migration path — the v5–v13 entries in §1.1 are the pattern. The JSONL
  ledger export and `edda export` (`crates/edda-cli/src/cmd_export.rs`) are
  the escape hatches: data can always be extracted in the form it was written.
- **Read-only consumers never migrate.** `edda verify` opens the ledger
  `query_only` and never applies schema or migrations
  (`crates/edda-ledger/src/sqlite_store/mod.rs:63-83#pub fn open_existing(db_path: &Path) -> anyhow::Result<Self> {`); an unreadable ledger
  is reported at exit 2 (`crates/edda-cli/src/cmd_verify.rs:55-63#let report = match ledger.verify_chain_report() {`), never
  repaired by a verification command.

## 2. Stable `--json` contracts

Ledger decision: `compat.stable-json-surfaces=dispatch-verify-ask-status-mcp`

Exactly the surfaces enumerated below are stable. **Everything not listed
here is unstable by default** — including every other `--json` flag in the
CLI (e.g. `edda phase --json`, `edda ask --fleet --json`
(`crates/edda-cli/src/cmd_ask.rs:172-173#let payload = crate::fleet::json_envelope(projects, &misses);`)): it may change shape in any
release without notice.

Within 0.x, a stable surface may have keys **added**. Keys are never
**deleted, renamed, or retyped**. Consumers must tolerate unknown keys (that
is what "additive" means for them).

### `edda dispatch --json`

One JSON object with exactly these keys (emitted at
`crates/edda-cli/src/cmd_dispatch.rs#pub fn to_json(&self) -> String {`; mirrored in the long help,
`crates/edda-cli/src/main.rs:480-485#With --json, exactly one object is printed to stdout:`):

| Key | Type | Notes |
|---|---|---|
| `outcome` | string | one of `done`, `crash`, `timeout`, `max_turns`, `budget_exceeded` (`crates/edda-cli/src/cmd_dispatch.rs#pub enum Outcome {`) |
| `result_text` | string \| null | agent summary; null except `done` |
| `cost_usd` | number \| null | honest cost; null when the backend reported no usage |
| `session_id` | string | id to reuse for continuity |
| `error` | string \| null | crash detail; null except `crash` |
| `model_requested` | string | what edda passed to the backend, or `inherited` |
| `model_observed` | string | what the backend reported in-band, or `unknown` |
| `session_observed` | string | the session id the backend reported in-band, or `unknown` |

Exit-code table (contract, mirrors the long help; mapping at
`crates/edda-cli/src/cmd_dispatch.rs#pub fn exit_code_for(outcome: Outcome) -> i32 {`): `0` done · `1` crash or any other failure ·
`2` timeout · `3` budget exceeded · `4` max turns.

Golden fixture: `crates/edda-cli/src/cmd_dispatch.rs` →
`compat_golden_fixture_dispatch_json_keys_types_and_exit_code_table`
(crate `edda`).

### `edda verify --json`

One JSON object with exactly these keys (emitted at
`crates/edda-cli/src/cmd_verify.rs:68-72#let payload = serde_json::json!({`):

| Key | Type | Notes |
|---|---|---|
| `ok` | bool | chain intact |
| `events` | integer | events scanned |
| `first_bad_event` | string \| null | first broken event id, null when intact |

Golden fixture: `crates/edda-cli/src/cmd_verify.rs` →
`compat_golden_fixture_verify_json_keys_and_types` (crate `edda`; covers
both the intact and the broken side, through the real binary).

### `edda ask --json`

One JSON object — the `AskResult` envelope
(`crates/edda-ask/src/lib.rs:52-75#pub struct AskResult {`), printed at
`crates/edda-cli/src/cmd_ask.rs:81-82#println!("{}", serde_json::to_string_pretty(&result)?);`. Keys always present: `query`
(string), `input_type` (string), `decisions`, `timeline`, `related_commits`,
`related_notes`, `conversations` (arrays). Keys `tasks` (array, GH-404),
`dependents` (array), `override_risk` (object), `workspace_event_count`
(integer, total events in the workspace ledger) and
`workspace_decision_count` (integer, total decisions) are present **only
when non-empty / Some** — they are serialized with `skip_serializing_if`
(`crates/edda-ask/src/lib.rs:62-73##[serde(skip_serializing_if = "Vec::is_empty")]`; the two count keys were added by #728) and their absence
means "none", not "removed".

A `DecisionHit` (element of `decisions`/`timeline`;
`crates/edda-ask/src/lib.rs`) always carries: `event_id`, `key`,
`value`, `reason`, `domain`, `branch`, `ts` (strings), `is_active` (bool),
and `governance` (object with `status`, optional `ratified_by`, `ratified_at`;
GH-806); `tags` (array), `village_id` (string), and `staleness` (object) appear
only when non-empty / Some.

Golden fixture: `crates/edda-cli/tests/ask_compat_contract.rs` →
`compat_golden_fixture_ask_json_keys_and_types` (crate `edda`).

### edda-mcp tool responses

Stable JSON MCP responses are a client contract for the following tools:

- `edda_ask` returns the `AskResult` envelope used by `edda ask --json`.
- `edda_tool_tier` returns `ToolTierResult`: `tool`, `tier` (`T0`–`T4`),
  `approval` (`none`/`lazy`/`required`/`blocked`), and `description`.
- `edda_task_new`, `edda_task_start`, `edda_task_done`, `edda_task_fail`,
  `edda_receipt`, and `edda_verify` return the JSON payloads consumed by the
  shared task state-machine and ledger verification paths.
- `edda_claim` returns the persisted claim-board result.

These response shapes are covered by the live cross-language SDK contract
scenario (`sdk/run-contract-tests.mjs`), which calls the real MCP server and
requires TypeScript/Python structural equivalence. Other MCP tool text remains
human-readable and is not a stable response contract.

### `edda status --json`

One JSON object with exactly these keys (emitted at
`crates/edda-cli/src/cmd_status.rs:18-29#let payload = serde_json::json!({`; flag declared at
`crates/edda-cli/src/main.rs:278-283#Status {`, dispatched at `crates/edda-cli/src/main.rs#Command::Status { json } =>`):

| Key | Type | Notes |
|---|---|---|
| `branch` | string | the ledger's head branch |
| `last_commit` | object \| null | null until the branch has a commit; keys below |
| `uncommitted_events` | integer | events on the branch since that commit |

`last_commit`, when present, has exactly these keys — they are part of the
same contract:

| Key | Type | Notes |
|---|---|---|
| `event_id` | string | the commit event's id |
| `ts` | string | RFC 3339 UTC, as written by `now_rfc3339` (`crates/edda-core/src/event.rs:19-22#fn now_rfc3339() -> String {`); sub-second precision is platform-dependent and not part of the contract |
| `title` | string | the commit title |

**Failure side.** `--json` adds no failure mode of its own: with and without
it, `edda status` produces the same exit code and the same stderr in every
state (measured). Success is exit `0` with the object on stdout. Failures do
not emit JSON — a newer ledger schema exits `2` (§1.2), and every other
failure (no `.edda/`, unreadable database) takes the CLI's shared error path
and exits `1` (`crates/edda-cli/src/main.rs:1091-1098#if let Err(err) = run(cli) {`).

That last row differs from `edda verify --json`, which answers `2` to the
same questions. The split is pre-existing and outside #730, but it is
recorded here rather than left for a consumer to discover: do not assume one
exit-code convention across these five surfaces.

The text form (no `--json`) is unchanged and is not a contract.

Golden fixture: `crates/edda-cli/src/cmd_status.rs` →
`compat_golden_fixture_status_json_keys_and_types` (crate `edda`), which pins
the key set and per-key types on both sides of `last_commit` — null before a
commit exists, object after — including `ts` being RFC-3339-shaped rather
than merely a string. `status_json_adds_no_failure_mode` pins the paragraph
above: on a missing workspace both forms exit alike and neither prints JSON.

## 3. Layer 2/3 events are unstable

All Layer 2/3 objects — task, claim, receipt, verdict, plan, and phase
events, plus the `edda review` review-verdict events (the verb itself is a
designed future verb, tracked as #652) — are **unstable** until
the v1 event spec (#608) lands and declares otherwise.

That is not re-declared here: it is the recorded decision
`spec.v1-scope=layer1-ledger-events-only-review-verdict-unstable`, cited
rather than restated.
