# Search Auto-Index v1 (GH-403)

Status: design approved, pending implementation
Issue: https://github.com/fagemx/edda/issues/403
Depends on: GH-402 (CJK tokenizer) — landed as #410; this work must respect its
`INDEX_VERSION` / `IndexLock` / `created_fresh` rebuild path.

## Problem

The Tantivy index is only ever built by a manual `edda search index`. Nothing in
the hook lifecycle builds or updates it, so the flagship retrieval feature ships
cold: on 2026-07-15 the fleet's daily-driver workspace (2147 events, months of
transcripts) had never been indexed at all. `edda search` answered "No search
index found" since forever and nobody noticed, because the SessionStart pack
covered daily recall.

Even after a manual build the index rots immediately — every new event is
invisible until someone remembers to re-run the command.

### The measurement that reframes the issue

The issue assumed an incremental step would be cheap. Measured on the real
corpus (project `043a8e81…`, 2227 events):

| Run | Result | Time |
|-----|--------|------|
| 1st | 2227 events + 35 turns | 24.661s |
| 2nd, immediately after, nothing changed | 2227 events + **0** turns | **24.729s** |

A re-index with zero new work still costs 24.7s. Cause: `indexer::index_events`
issues `delete_term(doc_type="event")` — deleting every event doc — then re-adds
all of them, on every run. **Turns are already incremental** (`index_watermark`
holds a per-session transcript byte offset; the 2nd run indexed 0 turns). Events
are not incremental at all, and they are the entire cost.

Consequence: wiring the *current* indexer to SessionEnd would add ~25s to every
session exit — over half the 45s background-join budget, and a direct violation
of the issue's own "must never block a session's exit". **Events must become
incremental first**; that is in scope here, not a follow-up.

`Ledger::events_after_rowid(after_rowid)` already exists (edda-serve uses it for
streaming) but the search indexer never adopted it. There is no event cursor:
`index_watermark` is `(project_id, session_id, last_offset)` and is turns-only.

Related cleanup: `indexer.rs` claims `add_event_doc` is "used both by bulk
`index_events` and by append-time indexing". That is stale — no append-time
caller exists.

## Decisions

Ruled by the operator during design:

1. **Scope**: make events incremental *and* wire auto-indexing in one change.
   The issue's acceptance criteria demand sub-second incremental, so splitting
   would leave GH-403 blocked on its own prerequisite.
2. **Build-if-missing**: `edda search query` against a missing index prints one
   notice, builds, then answers. Not silent (a 25s stall with no explanation
   reads as a hang, and contradicts the issue's own "staleness honesty").
3. **Cold build on SessionEnd**: no. SessionEnd only tops up an index that
   already exists; a missing index is left for query-time build-if-missing. The
   session-exit budget is never spent on a cold build.

### Approach: sqlite rowid cursor + idempotent per-event replace

Rejected alternatives:

- **Sentinel row in `index_watermark`** (store the event rowid in `last_offset`
  with `session_id="__events__"`): saves a table, but overloads a column that
  means "transcript byte offset" to sometimes mean "ledger rowid". A semantic
  lie every future reader must decode.
- **Derive the cursor from Tantivy** (store rowid as a fast field, take
  `max(rowid)` per run): a genuinely cleaner single source of truth with no
  cursor to drift — but it adds a field, which bumps `INDEX_VERSION` 2→3 and
  forces *every user through a second full rebuild* days after #410 forced one.
  The drift it prevents is already rendered harmless by commit-then-cursor
  ordering (below). Paying a forced rebuild for a property we already have.

The chosen approach adds **no Tantivy field**, so `INDEX_VERSION` stays at 2 and
no forced rebuild is triggered.

## Architecture

`edda-search-fts` deliberately does not depend on `edda-ledger` — that is why
`index_events` takes an injected closure. This design keeps that inversion.

The sync orchestration gets exactly one home, because it has three callers
(manual `search index`, query-time cold build, SessionEnd hook) and duplicating
it across crates is the cross-crate coupling class already flagged in #410
review.

```rust
// edda-search-fts::sync (new module)
pub struct SyncStats {
    pub events: usize,
    pub turns: usize,
    pub indexed_through: Option<String>, // ts of last indexed event
}

pub fn sync<F>(proj_dir: &Path, project_id: &str, events_after: F) -> anyhow::Result<SyncStats>
where
    F: FnOnce(i64) -> anyhow::Result<Vec<(i64, edda_core::Event)>>;
```

`sync` owns: `IndexLock` acquisition, open-or-create, outdated-schema wipe,
cursor read, incremental event indexing, turn indexing, `commit`, cursor write,
version-marker write. Callers supply only `|cursor| ledger.events_after_rowid(cursor)`.

**Cursor ownership**: callers never pass or reset the cursor — `sync` decides.
It resets to 0 (full rebuild) when `open_or_create_index` reports
`created_fresh`, or when the stored cursor is ahead of the ledger's max rowid.
This matters because a missing Tantivy dir does not imply a missing cursor:
`meta.sqlite` can outlive a deleted or wiped index, and a caller that assumed
"no index dir ⇒ start from 0" would read the stale cursor and skip every
existing event — the GH-403 bug, rebuilt. `created_fresh` (from #410) is the
single signal that governs this, for every fresh path at once.

Dependency direction is safe: `edda-bridge-claude` already has `edda-ledger` and
gains `edda-search-fts`; `edda-search-fts` depends on neither, so no cycle.

## Components

### edda-search-fts

- `schema.rs`
  - new table `events_watermark(project_id TEXT PRIMARY KEY, last_rowid INTEGER NOT NULL, last_ts TEXT)`
    in both `ensure_meta_db` and `ensure_meta_db_memory`. All table creation is
    `CREATE TABLE IF NOT EXISTS`, so no migration is needed.
  - `read_events_cursor(conn, project_id) -> anyhow::Result<i64>` (0 when absent)
  - `write_events_cursor(conn, project_id, rowid, ts) -> anyhow::Result<()>`
  - extend `clear_index_watermark` to also clear `events_watermark`, so the
    existing fresh-index reset keeps covering the whole cursor set.
- `indexer.rs`
  - new `index_events_since(writer, schema, project_id, events: &[(i64, Event)]) -> anyhow::Result<usize>`:
    per event, `delete_term(Term::from_field_text(doc_id, id))` then
    `add_event_doc`. `doc_id` uses the same `string_opts` as `doc_type`, which
    `index_events` already deletes by term, so no schema change is required.
  - **remove `index_events`** — its delete-all is precisely the behaviour being
    eliminated, and the full-rebuild path is expressed as `sync` with cursor 0.
  - fix the stale "append-time indexing" doc comment on `add_event_doc`.
- `sync.rs` — new, as above.

### edda-cli (`cmd_search.rs`)

- `index()` delegates to `sync`, supplying the ledger closure.
- `query()` takes `repo_root` (already available in `run_cmd`) so it can reach
  the ledger for build-if-missing and for the staleness count.
  - missing index → print `No search index — building now (one-time)…`, run
    `sync`, then answer. (The caller never passes a cursor; `sync` resets it
    itself — see "cursor ownership" below.)
  - present index → answer, then print `indexed through <last_ts>` and, when
    newer events exist, `N newer events not yet indexed`.
  - `N` is `ledger.events_after_rowid(cursor).len()` — the same cursor call the
    sync path uses, no new ledger API. It deserializes the un-indexed tail only,
    which is small in the steady state (SessionEnd keeps the cursor current); a
    cold or long-stale index pays a one-off read that is still far below the
    Tantivy commit cost it is reporting on.

### edda-bridge-claude

- `Cargo.toml`: add `edda-search-fts`.
- `bg_index.rs` (new): `should_run(project_id) -> bool` (true iff the index dir
  exists — cold build is not SessionEnd's job) and `run_index(project_id, cwd)`
  calling `sync`. Deliberately **no cooldown or interval gate**, unlike
  `bg_scan`/`bg_detect`: those are gated because they cost LLM calls, whereas an
  incremental sync with nothing new is a cursor read and a no-op commit. Gating
  it would only reintroduce staleness — the thing this issue exists to remove.
- `dispatch/session.rs`: spawn in `dispatch_session_end` following the existing
  `bg_extract`/`bg_digest` shape — gated spawn, `tx.send("bg_index")`, counted
  into the mpsc join with the `EDDA_BG_JOIN_TIMEOUT_SECS` (default 45s) budget.

## Data flow

**SessionEnd (hot path, must be sub-second)**
`should_run` → index missing? skip → else spawn thread → `sync` → read cursor →
`events_after_rowid(cursor)` → replace-add only new docs → `commit` → write
cursor → `tx.send` → main thread joins within budget.

**query**
Index missing → notice → `sync` (cursor 0, full build) → answer.
Index present → answer → `indexed through <ts>` (+ `N newer…` when N > 0).

**manual `edda search index`**
The same `sync`, ungated. `sync` always creates a missing index; what differs
between the three callers is only the gate in front of it — SessionEnd skips a
missing index (`bg_index::should_run`), query announces before building, and the
manual command just runs.

## Error handling and edge cases

- **Killed mid-reindex**: `commit` happens *before* the cursor write. A crash
  between them leaves the cursor stale, so the next run re-does that batch;
  `delete_term(doc_id)` makes re-adding idempotent. Never duplicates, never
  misses. This is the issue's "idempotent by watermark" criterion.
- **Cursor ahead of the ledger** (ledger rebuilt / rowids reset): if
  `cursor > max_rowid`, reset to 0 and rebuild. Otherwise events would be
  invisible forever — the exact bug shape GH-403 exists to kill; we must not
  reintroduce it in the fix.
- **Schema upgrade**: `created_fresh` (from #410) → cursor reset to 0 → full
  build. Reuses the existing path, no new mechanism.
- **Lock contention**: `sync` always takes `IndexLock`; a SessionEnd sync racing
  a manual `search index` means one waits.
- **Hook read paths must not build**: `cmd_ask`'s transcript callback stays
  read-only (`open_index`) and must not build-if-missing — otherwise a
  background hook could silently stall ~25s.
- **SessionEnd failure**: best-effort. `tracing::warn!` and move on; never block
  exit, consistent with the sibling background tasks.

## Testing

Test-driven; the RED test comes first.

1. **RED**: a second `sync` over an unchanged ledger reports 0 events indexed.
   This is the executable form of the 24.7s measurement above and must be seen
   failing before the incremental path exists — a straight port of today's
   `index_events` re-adds every event and fails it.
2. **Idempotence**: syncing the same batch twice leaves doc count unchanged, no
   duplicates.
3. **Crash recovery**: interrupt between `commit` and cursor write → next run
   completes the batch without duplicates.
4. **Cursor ahead of ledger** → reset to 0 and full rebuild.
5. **`created_fresh`** → cursor reset to 0.
6. **Cold build**: empty index + `query` → auto-builds and returns results.
7. **Watermark output**: `N newer events not yet indexed` appears iff N > 0.
8. **Budget (live)**: an empty `sync` (no new events) against the real 2227-event
   corpus must complete in < 1s. This is the number that decides whether the
   SessionEnd hook is shippable, so it is verified on the real corpus, not a
   fixture.

## Out of scope

- Turns are already incremental; untouched.
- No new Tantivy field → `INDEX_VERSION` stays 2 → no second forced rebuild.
- `query` does not auto-catch-up (only cold-builds). Freshness is SessionEnd's
  job; query's job is to be honest about staleness.

## Acceptance (from the issue)

- Fresh workspace: first `search query` works without a manual index step.
- After a session writes events, a new session's `search query` finds them with
  no manual reindex.
- Query output shows the indexed-through watermark.
- Kill the process mid-reindex: next run recovers, no corrupt index.
