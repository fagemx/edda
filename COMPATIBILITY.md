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

## 1. `schema_version` upgrade policy

Ledger decision: `compat.schema-version-policy=read-older-refuse-newer-minor-bump-announced`

### 1.1 Which `schema_version` this policy governs

Two version namespaces exist in the tree; the policy governs the **ledger
store version**:

- **Ledger store version** — `schema_meta.version` in the SQLite ledger
  (`schema_meta` defined at
  `crates/edda-ledger/src/sqlite_store/schema.rs:75-79`, read/written at
  `schema.rs:342-356`). This is the number a binary compares against itself
  when opening a ledger. The recorded history is a migration ladder from v1
  to v13 (`schema.rs:249-341`): v5 added cross-project sync fields, v6 the
  `task_briefs` view, v7 `device_tokens`, v8 `decide_snapshots`, v9 hot-path
  indexes, v10 decision deepening columns, v11 `village_id`, v12 the
  suggestions queue, v13 the `task_leases` table
  (`schema.rs:216-226`).
- **Event payload version** — `edda_core::SCHEMA_VERSION`
  (`crates/edda-core/src/types.rs:4`, stamped into every event payload at
  `crates/edda-core/src/event.rs:186` and siblings). This has been `1` for
  the project's entire history and is part of the Layer 1 event format, whose
  stability is governed by the v1 event spec (#608), not by this page.
- Decide-snapshot rows additionally carry a per-row
  `"schema_version": "snapshot.v1"` string
  (`crates/edda-ledger/src/sqlite_store/schema.rs:152`); it identifies the
  snapshot shape, not the ledger.

### 1.2 The policy

- **Read older.** A binary may open any ledger whose store version is **≤ its
  own**. Old ledgers are migrated forward on open, never rewritten in place at
  the event level: the raw events in the ledger are immutable, and changes to
  derived views go through `edda rebuild`
  (`crates/edda-cli/src/cmd_rebuild.rs`). This matches the ladder as it
  actually works today — `Ledger::open` → `SqliteStore::open_or_create` →
  `apply_schema` runs every missing migration upward
  (`crates/edda-ledger/src/ledger.rs:32-41`,
  `crates/edda-ledger/src/sqlite_store/mod.rs:45-53`,
  `schema.rs:249-341`).
- **Refuse newer.** A ledger with a store version **newer** than the binary
  is refused to open (exit 2, with a message naming both version numbers).
  Refusing is deliberate: the alternative is silently misinterpreting a
  ledger we cannot correctly read.
  **Enforced as of #735 (GH-729, closed):** opening a ledger checks the
  store version against `MAX_KNOWN_SCHEMA_VERSION` (13) and fails with
  `UnsupportedSchemaVersionError` (`schema.rs:228-247`); the message names
  both the stored and the maximum supported version, and `edda verify`
  maps it to exit 2 (`crates/edda-cli/src/cmd_verify.rs:40-43`). The
  contract is pinned by `crates/edda-cli/tests/schema_refusal_contract.rs`.
- **Minor bump, announced.** A `schema_version` jump is a **0.x minor bump**
  (e.g. 0.4 → 0.5), not a major-version event. The release that bumps the
  store version ships an updated version of this page documenting the
  migration path — the v5–v13 entries in §1.1 are the pattern. The JSONL
  ledger export and `edda export` (`crates/edda-cli/src/cmd_export.rs`) are
  the escape hatches: data can always be extracted in the form it was written.
- **Read-only consumers never migrate.** `edda verify` opens the ledger
  `query_only` and never applies schema or migrations
  (`crates/edda-ledger/src/sqlite_store/mod.rs:63-83`); an unreadable ledger
  is reported at exit 2 (`crates/edda-cli/src/cmd_verify.rs:55-63`), never
  repaired by a verification command.

## 2. Stable `--json` contracts

Ledger decision: `compat.stable-json-surfaces=dispatch-verify-ask-status-mcp`

Exactly the surfaces enumerated below are stable. **Everything not listed
here is unstable by default** — including every other `--json` flag in the
CLI (e.g. `edda phase --json`, `edda ask --fleet --json`
(`crates/edda-cli/src/cmd_ask.rs:167-173`)): it may change shape in any
release without notice.

Within 0.x, a stable surface may have keys **added**. Keys are never
**deleted, renamed, or retyped**. Consumers must tolerate unknown keys (that
is what "additive" means for them).

### `edda dispatch --json`

One JSON object with exactly these keys (emitted at
`crates/edda-cli/src/cmd_dispatch.rs:298-310`; mirrored in the long help,
`crates/edda-cli/src/main.rs:492-497`):

| Key | Type | Notes |
|---|---|---|
| `outcome` | string | one of `done`, `crash`, `timeout`, `max_turns`, `budget_exceeded` (`cmd_dispatch.rs:129-135`) |
| `result_text` | string \| null | agent summary; null except `done` |
| `cost_usd` | number \| null | honest cost; null when the backend reported no usage |
| `session_id` | string | id to reuse for continuity |
| `error` | string \| null | crash detail; null except `crash` |
| `model_requested` | string | what edda passed to the backend, or `inherited` |
| `model_observed` | string | what the backend reported in-band, or `unknown` |
| `session_observed` | string | the session id the backend reported in-band, or `unknown` |

Exit-code table (contract, mirrors the long help; mapping at
`cmd_dispatch.rs:151-159`): `0` done · `1` crash or any other failure ·
`2` timeout · `3` budget exceeded · `4` max turns.

Golden fixture: `crates/edda-cli/src/cmd_dispatch.rs` →
`compat_golden_fixture_dispatch_json_keys_types_and_exit_code_table`
(crate `edda`).

### `edda verify --json`

One JSON object with exactly these keys (emitted at
`crates/edda-cli/src/cmd_verify.rs:68-72`):

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
(`crates/edda-ask/src/lib.rs:52-75`), printed at
`crates/edda-cli/src/cmd_ask.rs:80-82`. Keys always present: `query`
(string), `input_type` (string), `decisions`, `timeline`, `related_commits`,
`related_notes`, `conversations` (arrays). Keys `tasks` (array, GH-404),
`dependents` (array), `override_risk` (object), `workspace_event_count`
(integer, total events in the workspace ledger) and
`workspace_decision_count` (integer, total decisions) are present **only
when non-empty / Some** — they are serialized with `skip_serializing_if`
(`lib.rs:62-73`; the two count keys were added by #728) and their absence
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

Of the MCP tools, two return JSON payloads, and their shapes are stable:

- `edda_ask` (`crates/edda-mcp/src/lib.rs:263-287`) — returns the same
  `AskResult` envelope as `edda ask --json` (same key set, same optionality
  rules).
- `edda_tool_tier` (`crates/edda-mcp/src/lib.rs:407-417`) — one JSON object,
  the `ToolTierResult` shape (`crates/edda-core/src/tool_tier.rs:103-108`):
  `tool` (string), `tier` (string, `T0`–`T4`), `approval` (string,
  `none`/`lazy`/`required`/`blocked`), `description` (string).

Other MCP tools return human-readable text; their output is not a contract.

Golden fixtures: `crates/edda-mcp/src/lib.rs` →
`compat_golden_fixture_ask_tool_response_keys_and_types` and
`compat_golden_fixture_tool_tier_response_keys_and_types`.

### `edda status --json`

One JSON object with exactly these keys (emitted at
`crates/edda-cli/src/cmd_status.rs:10-29`; flag declared at
`crates/edda-cli/src/main.rs:290-295`, dispatched at `:1263`):

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
| `ts` | string | RFC 3339 UTC, as written by `now_rfc3339` (`crates/edda-core/src/event.rs:19-22`); sub-second precision is platform-dependent and not part of the contract |
| `title` | string | the commit title |

**Failure side.** `--json` adds no failure mode of its own: with and without
it, `edda status` produces the same exit code and the same stderr in every
state (measured). Success is exit `0` with the object on stdout. Failures do
not emit JSON — a newer ledger schema exits `2` (§1.2), and every other
failure (no `.edda/`, unreadable database) takes the CLI's shared error path
and exits `1` (`crates/edda-cli/src/main.rs:1103-1110`).

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
events, plus `edda review` events (`review_verdict/0`) and its JSON output
(delivered in #652) — are **unstable** until
the v1 event spec (#608) lands and declares otherwise.

That is not re-declared here: it is the recorded decision
`spec.v1-scope=layer1-ledger-events-only-review-verdict-unstable`, cited
rather than restated.
