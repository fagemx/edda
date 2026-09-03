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
  `schema.rs:318-335`). This is the number a binary compares against itself
  when opening a ledger. The recorded history is a migration ladder from v1
  to v13 (`schema.rs:238-308`): v5 added cross-project sync fields, v6 the
  `task_briefs` view, v7 `device_tokens`, v8 `decide_snapshots`, v9 hot-path
  indexes, v10 decision deepening columns, v11 `village_id`, v12 the
  suggestions queue, v13 further decision columns.
- **Event payload version** — `edda_core::SCHEMA_VERSION`
  (`crates/edda-core/src/types.rs:4`, stamped into every event payload at
  `crates/edda-core/src/event.rs:186` and siblings). This has been `1` for
  the project's entire history and is part of the Layer 1 event format, whose
  stability is governed by the v1 event spec (#608), not by this page.
- Decide-snapshot rows additionally carry a per-row
  `"schema_version": "snapshot.v1"` string
  (`crates/edda-ledger/src/sqlite_store/schema.rs:151`); it identifies the
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
  `crates/edda-ledger/src/sqlite_store/mod.rs:44-52`,
  `schema.rs:238-308`).
- **Refuse newer.** A ledger with a store version **newer** than the binary
  is refused to open (exit 2, with a message naming both version numbers).
  Refusing is deliberate: the alternative is silently misinterpreting a
  ledger we cannot correctly read.
  **Implementation status (honest note):** this gate is declared policy but
  not yet enforced — `apply_schema` only walks `current < N` upward
  (`schema.rs:238-308`) and a newer ledger currently opens without a version
  check. The gap is tracked as #729; consumers must not rely on
  the current lenient behavior, and the gate may land in any 0.x release.
- **Minor bump, announced.** A `schema_version` jump is a **0.x minor bump**
  (e.g. 0.4 → 0.5), not a major-version event. The release that bumps the
  store version ships an updated version of this page documenting the
  migration path — the v5–v13 entries in §1.1 are the pattern. The JSONL
  ledger export and `edda export` (`crates/edda-cli/src/cmd_export.rs`) are
  the escape hatches: data can always be extracted in the form it was written.
- **Read-only consumers never migrate.** `edda verify` opens the ledger
  `query_only` and never applies schema or migrations
  (`crates/edda-ledger/src/sqlite_store/mod.rs:62-82`); an unreadable ledger
  is reported at exit 2 (`crates/edda-cli/src/cmd_verify.rs:55-59`), never
  repaired by a verification command.

## 2. Stable `--json` contracts

Ledger decision: `compat.stable-json-surfaces=dispatch-verify-ask-status-mcp`

Exactly the surfaces enumerated below are stable. **Everything not listed
here is unstable by default** — including every other `--json` flag in the
CLI (e.g. `edda phase --json`, `edda ask --fleet --json`
(`crates/edda-cli/src/cmd_ask.rs:131-137`)): it may change shape in any
release without notice.

Within 0.x, a stable surface may have keys **added**. Keys are never
**deleted, renamed, or retyped**. Consumers must tolerate unknown keys (that
is what "additive" means for them).

### `edda dispatch --json`

One JSON object with exactly these keys (emitted at
`crates/edda-cli/src/cmd_dispatch.rs:259-268`; mirrored in the long help,
`crates/edda-cli/src/main.rs:483-489`):

| Key | Type | Notes |
|---|---|---|
| `outcome` | string | one of `done`, `crash`, `timeout`, `max_turns`, `budget_exceeded` (`cmd_dispatch.rs:113-123`) |
| `result_text` | string \| null | agent summary; null except `done` |
| `cost_usd` | number \| null | honest cost; null when the backend reported no usage |
| `session_id` | string | id to reuse for continuity |
| `error` | string \| null | crash detail; null except `crash` |
| `model_requested` | string | what edda passed to the backend, or `inherited` |
| `model_observed` | string | what the backend reported in-band, or `unknown` |

Exit-code table (contract, mirrors the long help; mapping at
`cmd_dispatch.rs:127-135`): `0` done · `1` crash or any other failure ·
`2` timeout · `3` budget exceeded · `4` max turns.

Golden fixture: `crates/edda-cli/src/cmd_dispatch.rs` →
`compat_golden_fixture_dispatch_json_keys_types_and_exit_code_table`
(crate `edda`).

### `edda verify --json`

One JSON object with exactly these keys (emitted at
`crates/edda-cli/src/cmd_verify.rs:63-69`):

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
(`crates/edda-ask/src/lib.rs:52-74`), printed at
`crates/edda-cli/src/cmd_ask.rs:51-53`. Keys always present: `query`
(string), `input_type` (string), `decisions`, `timeline`, `related_commits`,
`related_notes`, `conversations` (arrays). Keys `tasks` (array, GH-404),
`dependents` (array) and `override_risk` (object) are present **only when
non-empty / Some** — they are serialized with `skip_serializing_if`
(`lib.rs:66-71`) and their absence means "none", not "removed".

A `DecisionHit` (element of `decisions`/`timeline`;
`crates/edda-ask/src/lib.rs:84-107`) always carries: `event_id`, `key`,
`value`, `reason`, `domain`, `branch`, `ts` (strings), `is_active` (bool);
`tags` (array), `village_id` (string), and `staleness` (object) appear only
when non-empty / Some.

Golden fixture: `crates/edda-cli/src/cmd_ask.rs` →
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

Declared stable by the ruling, **not yet implemented**. Today `edda status`
is text-only and takes no `--json` flag
(`crates/edda-cli/src/cmd_status.rs:5-10`, dispatched at
`crates/edda-cli/src/main.rs:669-671,1224`); passing `--json` is rejected at
argument parsing. There is no key set to pin, so no golden fixture exists for
it yet: the flag must land together with its fixture in the same commit, and
this page updates the same day. Tracked as #730.

## 3. Layer 2/3 events are unstable

All Layer 2/3 objects — task, claim, receipt, verdict, plan, and phase
events, plus the `edda review` review-verdict events — are **unstable** until
the v1 event spec (#608) lands and declares otherwise.

That is not re-declared here: it is the recorded decision
`spec.v1-scope=layer1-ledger-events-only-review-verdict-unstable`, cited
rather than restated.
