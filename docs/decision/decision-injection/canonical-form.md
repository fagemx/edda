# Canonical Form — How does Injection flow from context to delivery?

> Status: `working draft`
> Purpose: Define the canonical retrieval-to-delivery pipeline and the stage-aware filtering logic.
> Shared types: see `../decision-model/shared-types.md`

---

## 1. One-Liner

> **Context in, ranked pack out — every injection follows the same five-step pipeline.**

---

## 2. What It's NOT

### NOT a request-response API

The pipeline describes the internal flow. External callers (hooks, HTTP) invoke entry points that trigger the pipeline, but the pipeline itself is not an API. See `api.md` for the external contract.

### NOT a batch ETL process

Injection runs on-demand per hook event. There is no background indexing pass, no scheduled rebuild. The pipeline executes synchronously within the hook dispatch latency budget (~100ms for PreToolUse, ~500ms for SessionStart).

### NOT a learning/feedback loop

Injection does not learn from past deliveries. It does not track "did the agent follow this decision?" That is an Observability concern, not Injection.

---

## 3. The Five-Step Pipeline

```text
┌──────────────────────────────────────────────────────────────┐
│                    Injection Pipeline                         │
│                                                              │
│  Step 1          Step 2          Step 3        Step 4        │
│  ┌──────────┐   ┌──────────┐   ┌─────────┐   ┌──────────┐  │
│  │ Context  │──▶│ Retrieve │──▶│  Rank   │──▶│  Filter  │  │
│  │ Extract  │   │          │   │         │   │ (stage)  │  │
│  └──────────┘   └──────────┘   └─────────┘   └────┬─────┘  │
│                                                    │        │
│                                              Step 5│        │
│                                              ┌─────▼─────┐  │
│                                              │  Format   │  │
│                                              │  & Pack   │  │
│                                              └───────────┘  │
└──────────────────────────────────────────────────────────────┘
```

### Step 1: Context Extract

Parse the hook payload into a `RetrievalContext`:

```text
Hook Payload                          RetrievalContext
─────────────                         ────────────────
tool_name: "Edit"         ──▶         file_paths: ["crates/edda-ledger/src/lib.rs"]
file_path: "crates/..."              domains: ["db"]  (inferred)
cwd: "/project"                       branch: "main"  (from git HEAD)
session_id: "s1"                      stage: "implement"  (from hook type)
                                      tags: []  (none inferred)
                                      task_context: null
```

**Domain inference rules:**
- From decision key: the `domain` column in the `decisions` table is auto-extracted from the decision key at write time (e.g., `db.engine` -> domain `db`). This already exists in the codebase (`edda-ledger/sqlite_store.rs`). **No path-to-domain mapping is needed** — domain matching queries `WHERE domain = ?` against this existing column.
- From task context: `GH-319: optimize query` -> domain `query`, `perf` (keyword extraction from task brief)
- From explicit tag: user tags in the hook payload

### Step 2: Retrieve

Query `DecisionView` items matching the context signals. Uses multiple retrieval strategies in parallel:

```text
                    RetrievalContext
                         │
              ┌──────────┼──────────┐
              ▼          ▼          ▼
        ┌──────────┐ ┌────────┐ ┌────────┐
        │ Path     │ │ Domain │ │ Tag    │
        │ Match    │ │ Match  │ │ Match  │
        │          │ │        │ │        │
        │ glob     │ │ key    │ │ tags   │
        │ against  │ │ prefix │ │ inter- │
        │ affected │ │ match  │ │ section│
        │ _paths   │ │        │ │        │
        └────┬─────┘ └───┬────┘ └───┬────┘
             │            │          │
             └────────────┼──────────┘
                          ▼
                   Union + Dedup
                   (by event_id)
```

**Path matching** uses glob evaluation: `affected_paths: ["crates/edda-ledger/**"]` matches `crates/edda-ledger/src/lib.rs`.

**Domain matching** uses the `domain` field: context domain `db` matches decisions with `key` starting with `db.`.

**Tag matching** uses set intersection: context tags `["architecture"]` matches decisions with `tags` containing `"architecture"`.

### Step 3: Rank

Score each retrieved `DecisionView` by contextual relevance:

```text
Score = w_path * path_match_score
      + w_domain * domain_match_score
      + w_tag * tag_overlap_score
      + w_recency * recency_score
      + w_authority * authority_weight
      + w_reversibility * reversibility_weight
```

| Factor | Weight | Scoring |
|--------|--------|---------|
| `path_match` | 0.35 | 1.0 if glob matches file being edited, 0.5 if matches cwd, 0.0 otherwise |
| `domain_match` | 0.25 | 1.0 if exact domain match, 0.0 otherwise |
| `tag_overlap` | 0.15 | `|context_tags ∩ decision_tags| / |context_tags ∪ decision_tags|` (Jaccard) |
| `recency` | 0.10 | Exponential decay: `e^(-days_old / 90)` |
| `authority` | 0.10 | human=1.0, agent_approved=0.8, agent_proposed=0.3, system=0.5 |
| `reversibility` | 0.05 | low=1.0, medium=0.5, high=0.2 (hard-to-undo decisions rank higher) |

### Step 4: Filter (Stage-Aware)

Apply stage-specific filters to the ranked list:

```text
                    Ranked List
                         │
          ┌──────────────┼──────────────┐
          ▼              ▼              ▼
    ┌──────────┐  ┌──────────┐  ┌──────────┐
    │  plan    │  │implement │  │  review  │
    │          │  │          │  │          │
    │ active + │  │ active   │  │ active + │
    │ experi-  │  │ only     │  │ experi-  │
    │ mental   │  │          │  │ mental   │
    │          │  │ path     │  │          │
    │ all      │  │ match    │  │ include  │
    │ domains  │  │ required │  │ conflict │
    │          │  │          │  │ info     │
    │ max: 7   │  │ max: 5   │  │ max: 7   │
    └──────────┘  └──────────┘  └──────────┘
```

| Stage | Status Filter | Path Required | Conflict Info | Max Items |
|-------|--------------|---------------|---------------|-----------|
| `plan` | active, experimental | No | No | 7 |
| `implement` | active only | Yes (at least cwd match) | No | 5 |
| `review` | active, experimental | No | Yes (if available) | 7 |
| `dispatch` | active only | No | No | 3 |

### Step 5: Format & Pack

Render the filtered, ranked list into the output format appropriate for the hook:

- **SessionStart**: Full markdown pack with headers and details
- **PreToolUse**: Single-decision warning (if path match found)
- **UserPromptSubmit**: Compact one-liner list (lightweight)

See `schema-v0.md` for the exact output types.

---

## 4. Hook-to-Pipeline Mapping

```text
┌─────────────────────┬────────────────────────────────────────┐
│ Hook                │ Pipeline Behavior                      │
├─────────────────────┼────────────────────────────────────────┤
│ SessionStart        │ stage=infer from task context           │
│                     │ context=cwd + task brief + branch       │
│                     │ output=full DecisionPack markdown       │
│                     │ budget=~2000 chars                      │
├─────────────────────┼────────────────────────────────────────┤
│ PreToolUse (Edit)   │ stage="implement"                      │
│                     │ context=file_path from tool input       │
│                     │ output=FileWarning (single decision)    │
│                     │ budget=~200 chars                       │
│                     │ latency: < 100ms                        │
├─────────────────────┼────────────────────────────────────────┤
│ PreToolUse (Bash)   │ stage="implement"                      │
│                     │ context=command text (parse file refs)  │
│                     │ output=FileWarning or skip              │
│                     │ budget=~200 chars                       │
├─────────────────────┼────────────────────────────────────────┤
│ UserPromptSubmit    │ stage=infer from prompt keywords        │
│                     │ context=prompt text + cwd               │
│                     │ output=compact list or skip (dedup)     │
│                     │ budget=~500 chars                       │
└─────────────────────┴────────────────────────────────────────┘
```

---

## 5. Stage Inference

When the stage is not explicit, Injection infers it:

```text
IF hook == PreToolUse           → "implement"
ELIF hook == SessionStart:
  IF task_brief contains "plan" → "plan"
  ELIF task_brief exists        → "implement"
  ELSE                          → "plan"  (new session default)
ELIF hook == UserPromptSubmit:
  IF prompt contains review/PR keywords → "review"
  ELIF prompt contains plan keywords    → "plan"
  ELSE                                  → "implement"
```

---

## 6. Dedup and Caching

Injection reuses the existing dedup mechanism from `edda-bridge-claude`:

- **Session-scoped hash**: After each injection, store a hash of the delivered content. On subsequent hooks in the same session, skip if the hash matches.
- **No cross-session cache**: Each session starts fresh. Decision state may have changed between sessions.

---

## 7. Canonical Examples

### Example 1: Full pipeline for SessionStart

```text
Input:
  hook = SessionStart
  cwd = "C:\ai_agent\edda"
  branch = "main"
  task_brief = "GH-319: optimize decision query hot path"

Step 1 (Context Extract):
  file_paths = []  (no specific file yet)
  domains = ["query", "perf"]  (from task keywords)
  branch = "main"
  stage = "implement"  (task brief exists)
  tags = []

Step 2 (Retrieve):
  Domain match "query" → finds: query.engine=tantivy, query.cache=lru
  Domain match "perf"  → finds: perf.budget_ms=100
  No path match (no files yet)

Step 3 (Rank):
  1. perf.budget_ms=100      (domain=0.25, recency=0.09, authority=0.10) = 0.44
  2. query.engine=tantivy    (domain=0.25, recency=0.07, authority=0.10) = 0.42
  3. query.cache=lru         (domain=0.25, recency=0.05, authority=0.08) = 0.38

Step 4 (Filter):
  stage=implement, status=active only → all 3 pass
  max=5, have 3 → no truncation

Step 5 (Format):
  ## Active Decisions (3)
  - perf.budget_ms=100 — hot path must stay under 100ms [human]
  - query.engine=tantivy — full-text search via Tantivy [agent_approved]
  - query.cache=lru — LRU cache for repeated queries [human]
```

### Example 2: PreToolUse file warning

```text
Input:
  hook = PreToolUse
  tool = Edit
  file_path = "crates/edda-ledger/src/sqlite_store.rs"

Step 1: file_paths = ["crates/edda-ledger/src/sqlite_store.rs"]
Step 2: Path match → db.engine=sqlite (affected_paths: ["crates/edda-ledger/**"])
Step 3: Score = 0.35 (path) + 0.25 (domain) = 0.60
Step 4: stage=implement, 1 result → pass
Step 5: "Active decision: db.engine=sqlite — embedded, zero-config. Ensure edits align."
```

---

## 8. Boundaries / Out of Scope

### In Scope
- Five-step pipeline: extract, retrieve, rank, filter, format
- Stage-aware filtering logic and stage inference
- Hook-to-pipeline mapping (SessionStart, PreToolUse, UserPromptSubmit)
- Ranking weights and scoring formula
- Dedup/caching strategy

### Out of Scope
- **Query API signatures** -> `api.md`
- **Output type definitions** -> `schema-v0.md`
- **Decision lifecycle** -> Governance spec
- **Conflict detection** -> Governance spec
- **Hot memory pack** (conversation turns) -> edda-pack

---

## Closing Line

> **Five steps, three hooks, four stages. Context in, ranked pack out. The pipeline is deterministic, stage-aware, and always read-only.**
