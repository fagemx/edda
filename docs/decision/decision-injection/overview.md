# Decision Injection — When and where do decisions surface?

> Status: `v0 design spec` — the implementation shipped via the [decision-deepening tracks](../../archive/plans/decision-deepening/00_OVERVIEW.md); where details differ, the code is authoritative.
> Purpose: Define retrieval, ranking, pack generation, and hook-triggered delivery of decisions to agents and humans.
> Shared types: see `../decision-model/shared-types.md`

---

## 1. One-Liner

> **Injection reads decisions and delivers them at the right time, in the right form, to the right context — it never creates, modifies, or judges them.**

---

## 2. What It's NOT / Common Mistakes

### NOT a mutation layer

Injection never calls `create_candidate()`, `promote()`, `reject()`, `supersede()`, or any other write/transition operation. It consumes `DecisionView` via `to_view()` and formats for delivery. If you need to change a decision's state, that is Governance.

### NOT a search engine

Injection uses the existing query infrastructure (`edda-ask`, `edda-search-fts`) but it is not a general-purpose search service. It answers one question: "which decisions matter for this context right now?" General search (`edda ask`) remains a separate user-facing command.

### NOT a conflict resolver

Injection may receive `ConflictInfo` from Governance and format it for human consumption, but it never judges whether a conflict is real, which side wins, or what action to take. **Governance judges, Injection explains.**

### NOT the same as the hot pack

The hot memory pack (`edda-pack`, `hot.md`) contains recent conversation turns. Decision packs are a separate concern: they contain relevant decisions for a context. Both may be injected at session start, but they are different data structures with different lifecycles.

---

## 3. Core Concepts

### DecisionView (the ONLY input type)

Injection works exclusively with `DecisionView` (see `../decision-model/shared-types.md` section 2.3). It never touches `DecisionRow` or parses storage-layer JSON. The `to_view()` function is the sole bridge between storage and delivery.

### Retrieval Context

The set of signals that determine which decisions are relevant:

| Signal | Source | Example |
|--------|--------|---------|
| File paths | Hook payload, tool use | `crates/edda-ledger/src/lib.rs` |
| Task/issue | Karvi brief, issue URL | `GH-319: optimize query path` |
| Domain | Extracted from file path or query | `db`, `auth` |
| Tags | User-specified or inferred | `architecture`, `error-handling` |
| Branch | Git HEAD | `main`, `feat/auth` |
| Stage | Hook context | `plan`, `implement`, `review`, `dispatch` |

### Decision Pack

A ranked, size-bounded collection of `DecisionView` items relevant to a retrieval context. Packs are the unit of delivery.

### Stage-Aware Filtering

Different workflow stages need different decision subsets:

| Stage | Filter Logic | Rationale |
|-------|-------------|-----------|
| `plan` | All active + experimental; include `reversibility: low` | Planning needs full picture, especially hard-to-undo choices |
| `implement` | Active only; filter by `affected_paths` match | Implementation needs precise, actionable decisions |
| `review` | Active + experimental; include conflict info | Review validates decisions are followed |
| `dispatch` | Active only; minimal top-3 | Dispatch is lightweight; avoid context bloat |

---

## 4. Position in the Overall System

```text
                    ┌──────────────────┐
                    │  Decision Model  │
                    │  (source of      │
                    │   truth)         │
                    └────────┬─────────┘
                             │ to_view()
                             ▼
┌───────────┐      ┌──────────────────┐       ┌────────────┐
│           │      │                  │       │            │
│  Intake   │      │   Injection      │       │ Governance │
│  (create) │      │                  │       │ (lifecycle)│
│           │      │  ┌────────────┐  │       │            │
└───────────┘      │  │ Retrieval  │  │       └────────────┘
                   │  │ Engine     │  │              │
                   │  └─────┬──────┘  │              │
                   │        │         │   ConflictInfo│
                   │  ┌─────▼──────┐  │◄─────────────┘
                   │  │ Relevance  │  │  (read-only)
                   │  │ Ranking    │  │
                   │  └─────┬──────┘  │
                   │        │         │
                   │  ┌─────▼──────┐  │
                   │  │ Pack       │  │
                   │  │ Generator  │  │
                   │  └─────┬──────┘  │
                   │        │         │
                   └────────┼─────────┘
                            │
            ┌───────────────┼───────────────┐
            ▼               ▼               ▼
    ┌──────────────┐ ┌─────────────┐ ┌────────────┐
    │ SessionStart │ │ PreToolUse  │ │ UserPrompt │
    │ Hook         │ │ Hook        │ │ Submit     │
    │ (full pack)  │ │ (file warn) │ │ (light)    │
    └──────────────┘ └─────────────┘ └────────────┘
```

**Dependency direction:** Injection depends on Decision Model (via `to_view()`). Injection may receive `ConflictInfo` from Governance for formatting, but never calls Governance mutations. Intake is completely independent of Injection.

---

## 5. What Injection Owns

| Capability | Description |
|------------|-------------|
| Retrieval | Given context signals, find matching `DecisionView` items |
| Relevance ranking | Score and rank decisions by contextual relevance |
| Pack generation | Build bounded, stage-aware decision packs |
| Hook delivery | SessionStart, PreToolUse, UserPromptSubmit injection |
| File-aware matching | Match `affected_paths` globs against edited file paths |
| Task-aware matching | Match decisions by domain/tags against task context |
| Stage-aware filtering | Different pack contents for plan/implement/review/dispatch |
| Conflict explanation | Format `ConflictInfo` (from Governance) for human readability |

---

## 6. Relationship to Other Specs

| Spec | Injection's Relationship | Contract Surface |
|------|-------------------------|-----------------|
| **Decision Model** | Consumes `DecisionView` via `to_view()` | Read-only: `to_view()`, query functions |
| **Intake** | No direct relationship | Independent — Intake creates, Injection reads |
| **Governance** | Receives `ConflictInfo` for formatting | Read-only: never calls transitions |
| **edda-ask** (existing) | Extends with file/task/stage-aware retrieval | Existing TF-IDF + new context signals |
| **edda-pack** (existing) | Separate concern: hot pack = turns, decision pack = decisions | Both injected at session start |
| **edda-bridge-claude** | Hook dispatch calls Injection's pack API | SessionStart, PreToolUse, UserPromptSubmit |
| **edda-serve** | HTTP endpoints wrap Injection queries | `/api/decisions`, `/api/decisions/batch` |

---

## 7. Canonical Examples

### Example 1: Session start — full decision pack

Agent starts a new session working on `crates/edda-ledger/`. Injection:

1. Detects cwd = `crates/edda-ledger/`
2. Queries active decisions where `affected_paths` matches `crates/edda-ledger/**`
3. Also queries by domain `db` (inferred from path context)
4. Ranks by relevance: scope match > recency > tag match
5. Builds a pack of top 5 decisions
6. Injects via SessionStart hook as `additionalContext`

```json
{
  "hookSpecificOutput": {
    "hookEventName": "SessionStart",
    "additionalContext": "## Active Decisions (5)\n\n- db.engine=sqlite — embedded, zero-config for CLI tool [LOW reversibility]\n- db.schema=jsonl — append-only event log [LOW reversibility]\n- error.pattern=thiserror+anyhow — axum idiomatic [MEDIUM reversibility]\n..."
  }
}
```

### Example 2: PreToolUse — file edit warning

Agent is about to edit `crates/edda-ledger/src/sqlite_store.rs`. Injection:

1. Matches file path against all active decisions' `affected_paths`
2. Finds `db.engine=sqlite` with `affected_paths: ["crates/edda-ledger/**"]`
3. Injects a lightweight warning

```json
{
  "hookSpecificOutput": {
    "hookEventName": "PreToolUse",
    "decision": "db.engine=sqlite — embedded, zero-config for CLI tool",
    "warning": "This file is governed by an active decision. Ensure changes align with: db.engine=sqlite"
  }
}
```

---

## 8. Boundaries / Out of Scope

### In Scope

- Retrieval: query decisions by file, task, domain, tag, branch, stage
- Ranking: score decisions by contextual relevance
- Pack generation: bounded, formatted decision sets
- Hook integration: SessionStart, PreToolUse, UserPromptSubmit delivery
- Conflict explanation: format `ConflictInfo` for humans
- Stage-aware filtering: different content per workflow stage

### Out of Scope

- **Creating decisions** -> Intake spec
- **Lifecycle transitions** (promote, reject, freeze) -> Governance spec
- **Conflict detection and judgment** -> Governance spec
- **Hot memory pack** (conversation turns) -> edda-pack, separate concern
- **Full-text search UI** -> edda-ask / edda-search-fts, user-facing
- **Decision storage schema** -> Decision Model spec

---

## Closing Line

> **Injection is the delivery layer: it reads `DecisionView`, ranks by context, and delivers packs through hooks. It never writes, never judges, never transitions. Read-only, context-aware, stage-sensitive.**
