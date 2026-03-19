# Query API & Hook Integration Points

> Status: `working draft`
> Purpose: Define the function-level API that hooks and HTTP endpoints call into Injection, plus the hook integration contract.
> Shared types: see `../decision-model/shared-types.md`

---

## 1. One-Liner

> **5 query functions, 3 hook entry points, 1 HTTP wrapper layer. All read-only, all return `DecisionPack` or its subsets.**

---

## 2. What It's NOT

### NOT the mutation contract

Decision Model's `api.md` defines write operations (`create_candidate`, `promote`, etc.). This file defines Injection's read-only query API. They are complementary, not competing.

### NOT an HTTP spec

This defines the library-level Rust API in a new `edda-injection` module (or crate). It is a **parallel query path** alongside `edda-ask`, not an extension of it. HTTP routes in `edda-serve` wrap these functions. The HTTP mapping is documented here for reference but the authoritative contract is the function signatures.

### NOT the full `edda ask` command

`edda ask` is a user-facing CLI command with its own output format. Injection's API is programmatic — consumed by hooks and HTTP endpoints, not directly by users.

---

## 3. Query Functions

### 3.1 `query_for_context`

The primary entry point. Takes a `RetrievalContext` and returns a `DecisionPack`.

```typescript
function query_for_context(
  ctx: RetrievalContext,
): DecisionPack;
```

**Behavior:**
1. Runs the five-step pipeline (see `canonical-form.md`)
2. Returns a bounded, ranked, stage-filtered `DecisionPack`
3. Applies dedup by `event_id`

**Callers:** All three hooks (SessionStart, PreToolUse, UserPromptSubmit)

### 3.2 `query_by_paths`

File-aware retrieval: find decisions whose `affected_paths` globs match the given file paths.

```typescript
function query_by_paths(
  paths: string[],           // file paths to match against
  branch: string,            // git branch filter
  limit: number,             // max results (default: 10)
): ScoredDecisionView[];
```

**Behavior:**
1. For each active `DecisionView`, evaluate each `affected_paths` glob against each input path
2. Score by glob specificity: exact file match > directory match > wildcard match
3. Return sorted by score, deduped, limited

**Implementation note:** Uses `glob::Pattern` matching. The existing `affected_paths` field stores glob patterns like `crates/edda-ledger/**`.

**Callers:** PreToolUse hook (file edit warning), `query_for_context` step 2

### 3.3 `query_by_domain`

Domain-aware retrieval: find active decisions in a given domain.

```typescript
function query_by_domain(
  domain: string,            // "db", "auth", "error"
  branch: string,
  status_filter: DecisionStatus[],  // default: ["active"]
  limit: number,             // default: 10
): ScoredDecisionView[];
```

**Behavior:**
1. Filter `DecisionView` items where `domain == domain` (or `key` starts with `domain.`)
2. Apply status filter
3. Score by recency + authority
4. Return sorted

**Callers:** `query_for_context` step 2, HTTP `/api/decisions?domain=db`

### 3.4 `query_by_tags`

Tag-aware retrieval: find decisions matching a set of tags.

```typescript
function query_by_tags(
  tags: string[],            // ["architecture", "storage"]
  branch: string,
  limit: number,             // default: 10
): ScoredDecisionView[];
```

**Behavior:**
1. Filter `DecisionView` items where `tags ∩ input_tags` is non-empty
2. Score by Jaccard similarity of tag sets + recency
3. Return sorted

**Callers:** `query_for_context` step 2

### 3.5 `build_pack`

Assemble a `DecisionPack` from scored results. Applies stage-aware filtering and size limits.

```typescript
function build_pack(
  candidates: ScoredDecisionView[],
  stage: InjectionStage,
  branch: string,
  conflicts?: ConflictInfo[],  // from Governance, optional
): DecisionPack;
```

**Behavior:**
1. Apply stage-specific status filter (see `canonical-form.md` section 4)
2. Truncate to stage-specific `max_items`
3. If stage == `review` and conflicts provided, build `ConflictExplanation` items
4. Assemble `DecisionPack` with metadata

**Callers:** `query_for_context` steps 4-5

---

## 4. Hook Integration Points

### 4.1 SessionStart

**Trigger:** Claude Code session begins (via `edda hook claude --event SessionStart`)

**Current integration point:** `edda-bridge-claude/src/dispatch/session.rs` :: `dispatch_session_start()`

**Injection behavior:**
1. Build `RetrievalContext` from: cwd, branch, task brief (if available)
2. Infer stage from task context
3. Call `query_for_context(ctx)` -> `DecisionPack`
4. Render pack as markdown
5. Append to `additionalContext` alongside hot memory pack

**Budget:** ~2000 chars for decision pack (separate from hot pack budget)

**Latency target:** < 500ms

```text
dispatch_session_start()
  │
  ├── read_hot_pack()           // existing: conversation turns
  ├── query_for_context(ctx)    // NEW: decision pack
  │     ├── query_by_paths([])
  │     ├── query_by_domain(inferred)
  │     └── build_pack(results, stage)
  ├── render_decision_pack_md() // NEW: markdown formatting
  └── combine into additionalContext
```

### 4.2 PreToolUse

**Trigger:** Agent is about to use a tool (Edit, Write, Bash)

**Current integration point:** `edda-bridge-claude/src/dispatch/` :: PreToolUse handler

**Injection behavior:**
1. Extract `file_path` from tool input (Edit/Write) or parse from command (Bash)
2. If no file path extractable, skip (return empty)
3. Call `query_by_paths([file_path], branch, 1)`
4. If match found with score > threshold (0.5), render `FileWarningOutput`
5. If no match, return empty (no injection)

**Budget:** ~200 chars (single-line warning)

**Latency target:** < 100ms (critical — blocks tool execution)

```text
PreToolUse handler
  │
  ├── extract file_path from tool input
  ├── query_by_paths([path], branch, 1)
  │     └── glob match against affected_paths
  ├── IF score > 0.5:
  │     └── render FileWarningOutput
  └── ELSE: return empty
```

### 4.3 UserPromptSubmit

**Trigger:** User submits a prompt

**Current integration point:** `edda-bridge-claude/src/dispatch/session.rs` :: `dispatch_user_prompt_submit()`

**Injection behavior:**
1. Extract keywords from user prompt
2. Infer stage from prompt content
3. Build lightweight `RetrievalContext` (domains from keywords, no file paths)
4. Call `query_for_context(ctx)` with `max_items = 3`
5. Render as compact one-liner
6. Apply dedup: skip if identical to last injection in this session

**Budget:** ~500 chars (compact list)

**Latency target:** < 200ms

```text
dispatch_user_prompt_submit()
  │
  ├── extract keywords from prompt
  ├── IF post_compact: full pack (existing behavior)
  ├── ELSE:
  │     ├── query_for_context(ctx, max=3)
  │     ├── render_compact_list()
  │     └── dedup check (hash comparison)
  └── combine with workspace context
```

---

## 5. HTTP Endpoint Mapping

Injection's query functions are exposed via `edda-serve`. These wrap the library API.

| Endpoint | Method | Injection Function | Notes |
|----------|--------|-------------------|-------|
| `/api/decisions` | GET | `query_by_domain` / `query_for_context` | Existing; add `?paths=` and `?stage=` params (routed to Injection) |
| `/api/decisions/batch` | POST | `query_for_context` (per query) | Existing; add context-aware queries (routed to Injection) |
| `/api/decisions/pack` | GET | `query_for_context` + `build_pack` | **New**: returns rendered `DecisionPack` |
| `/api/decisions/file-match` | GET | `query_by_paths` | **New**: `?paths=a.rs,b.rs` |

### 5.1 New: GET `/api/decisions/pack`

```text
GET /api/decisions/pack?stage=implement&paths=crates/edda-ledger/src/lib.rs&branch=main

Response:
{
  "stage": "implement",
  "branch": "main",
  "generated_at": "2026-03-19T14:30:00Z",
  "items": [...],
  "total_candidates": 12,
  "included": 5,
  "conflicts": []
}
```

### 5.2 New: GET `/api/decisions/file-match`

```text
GET /api/decisions/file-match?paths=crates/edda-ledger/src/lib.rs&branch=main&limit=3

Response:
[
  {
    "decision": { "key": "db.engine", "value": "sqlite", ... },
    "score": 0.82,
    "match_signals": [
      { "type": "path_match", "path": "crates/edda-ledger/src/lib.rs", "glob": "crates/edda-ledger/**" }
    ]
  }
]
```

---

## 6. Relationship to `edda-ask`

Injection is a **parallel query path**, not an extension of `edda-ask`. The two serve different consumers:

| | `edda-ask` | `edda-injection` |
|---|---|---|
| **Consumer** | Human CLI (`edda ask`), MCP tool | Hooks (SessionStart, PreToolUse), HTTP pack API |
| **Input** | Free-form query string | Structured `RetrievalContext` (paths, domains, stage) |
| **Output** | `AskResult` (decisions + timeline + commits + notes + conversations) | `DecisionPack` (ranked decisions only) |
| **Scoring** | TF-IDF semantic search | Composite (scope match + domain + tag + recency) |
| **Latency** | No constraint | <100ms (PreToolUse), <500ms (SessionStart) |

**Why not extend `edda-ask`?**
- `AskResult` is structurally incompatible with `DecisionPack` — it carries timeline, commits, notes, conversations that Injection doesn't need
- Adding `PathMatch` / `ContextQuery` to `InputType` would force `ask()` to return a `DecisionPack` for some variants and `AskResult` for others — a type-level mess
- Injection's scoring formula (composite weights) is fundamentally different from `edda-ask`'s TF-IDF

**What IS reused:**
- `edda-ask`'s `semantic_decision_search()` TF-IDF scorer can be called as one signal within Injection's composite ranking
- The `Ledger` query methods (`active_decisions`, `active_decisions_limited`) are shared infrastructure
- `edda-ask` continues to serve the CLI and MCP — Injection does not replace it

---

## 7. Canonical Examples

### Example 1: Hook dispatch flow (SessionStart)

```rust
// In edda-bridge-claude/src/dispatch/session.rs
fn dispatch_session_start(...) -> anyhow::Result<HookResult> {
    // ... existing hot pack logic ...

    // NEW: Decision injection
    let ctx = RetrievalContext {
        file_paths: vec![],
        domains: infer_domains_from_task(task_brief),
        tags: vec![],
        branch: git_branch.clone(),
        stage: infer_stage(task_brief),
        task_context: parse_task_context(task_brief),
        max_items: None,  // use stage default
    };

    let pack = query_for_context(&ledger, &ctx)?;
    let decision_md = render_decision_pack_md(&pack);

    // Append to existing content
    content = Some(match content {
        Some(c) => format!("{c}\n\n{decision_md}"),
        None => decision_md,
    });
}
```

### Example 2: HTTP endpoint (file-match)

```rust
// In edda-serve
async fn get_decision_file_match(
    State(state): State<Arc<AppState>>,
    Query(params): Query<FileMatchParams>,
) -> Result<Json<Vec<ScoredDecisionView>>, AppError> {
    let ledger = state.open_ledger()?;
    let paths: Vec<String> = params.paths.split(',').map(|s| s.to_string()).collect();
    let branch = params.branch.unwrap_or("main".into());
    let limit = params.limit.unwrap_or(5);

    let results = query_by_paths(&ledger, &paths, &branch, limit)?;
    Ok(Json(results))
}
```

---

## 8. Boundaries / Out of Scope

### In Scope
- 5 query function signatures with behavior specs
- 3 hook integration points with dispatch flow
- HTTP endpoint mapping (existing + new)
- Relationship to `edda-ask` (parallel, not extension)
- Latency targets and budget constraints

### Out of Scope
- **Mutation operations** -> Decision Model `api.md`
- **Type definitions** -> `schema-v0.md` (this spec) and `../decision-model/shared-types.md`
- **Pipeline internals** (ranking formula, stage filter rules) -> `canonical-form.md`
- **Rust trait design** -> implementation detail
- **CLI command changes** -> separate concern

---

## Closing Line

> **5 read-only query functions, 3 hook entry points, 2 new HTTP endpoints. Injection's API surface is small by design — it does one thing: deliver the right decisions to the right context.**
