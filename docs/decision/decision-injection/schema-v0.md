# Schema v0 — Read-Side View Models and Pack Types

> Status: `working draft`
> Purpose: Define the types that Injection owns: retrieval context, ranked results, pack formats, and hook output shapes.
> Shared types: see `../decision-model/shared-types.md`

---

## 1. One-Liner

> **Injection owns the read-side types: context in, ranked pack out. Storage types belong to Decision Model.**

---

## 2. What It's NOT

### NOT storage schema

Injection never defines storage tables or row types. Those belong to Decision Model (`DecisionRow`). Injection's types are all transient — computed per request, never persisted.

### NOT a redefinition of DecisionView

`DecisionView` is defined in `../decision-model/shared-types.md` section 2.3. Injection consumes it. This file defines the types that wrap, rank, and deliver `DecisionView`.

---

## 3. Input Types

### 3.1 RetrievalContext

The parsed context from a hook payload. This is what the pipeline operates on.

```typescript
type RetrievalContext = {
  // Files being edited or referenced
  file_paths: string[];                // ["crates/edda-ledger/src/lib.rs"]

  // Inferred or explicit domains
  domains: string[];                   // ["db", "query"]

  // Tags to match against
  tags: string[];                      // ["architecture"]

  // Git branch
  branch: string;                      // "main"

  // Workflow stage
  stage: InjectionStage;              // "implement"

  // Task context (optional — from karvi brief or issue)
  task_context: TaskContext | null;

  // Pack size limit (overridable, default from stage)
  max_items?: number;
};
```

### 3.2 InjectionStage

The workflow stage determines filtering behavior.

```typescript
type InjectionStage =
  | "plan"       // broad view: active + experimental, all domains
  | "implement"  // focused: active only, path match preferred
  | "review"     // validation: active + experimental, include conflicts
  | "dispatch";  // minimal: active only, top 3
```

### 3.3 TaskContext

Optional task metadata that enriches retrieval.

```typescript
type TaskContext = {
  // Task or issue identifier
  task_id: string;                     // "GH-319"

  // Free-text description
  description: string;                 // "optimize decision query hot path"

  // Extracted keywords (for domain/tag matching)
  keywords: string[];                  // ["query", "optimize", "hot-path"]
};
```

---

## 4. Scoring Types

### 4.1 ScoredDecisionView

A `DecisionView` annotated with relevance score and match signals.

```typescript
type ScoredDecisionView = {
  // The decision itself (see ../decision-model/shared-types.md §2.3)
  decision: DecisionView;

  // Composite relevance score (0.0 - 1.0)
  score: number;

  // Which signals contributed to the score
  match_signals: MatchSignal[];
};

type MatchSignal =
  | { type: "path_match"; path: string; glob: string }
  | { type: "domain_match"; domain: string }
  | { type: "tag_match"; tags: string[] }
  | { type: "keyword_match"; keyword: string }
  | { type: "recency"; days_old: number }
  | { type: "authority"; authority: DecisionAuthority }
  | { type: "reversibility"; reversibility: Reversibility };
```

### 4.2 RankingWeights

Configurable weights for the scoring formula. Defaults shown.

```typescript
type RankingWeights = {
  path_match: number;        // 0.35
  domain_match: number;      // 0.25
  tag_overlap: number;       // 0.15
  recency: number;           // 0.10
  authority: number;         // 0.10
  reversibility: number;     // 0.05
};
```

---

## 5. Pack Types

### 5.1 DecisionPack

The output of the pipeline: a bounded set of ranked decisions for a context.

```typescript
type DecisionPack = {
  // Metadata
  stage: InjectionStage;
  branch: string;
  generated_at: string;                // ISO 8601
  context_summary: string;             // "3 file paths, domain: db"

  // Ranked decisions (highest score first)
  items: ScoredDecisionView[];

  // How many candidates were considered vs included
  total_candidates: number;
  included: number;

  // Conflict explanations (review stage only)
  conflicts: ConflictExplanation[];
};
```

### 5.2 ConflictExplanation

Human-readable formatting of `ConflictInfo` (defined in `../decision-model/shared-types.md` section 3.2). Injection owns the explanation, Governance owns the judgment.

```typescript
type ConflictExplanation = {
  // The structural conflict (from Governance)
  conflict: ConflictInfo;

  // Human-readable summary
  summary: string;
  // e.g. "db.engine has conflicting values: sqlite (main) vs postgres (feat/migration)"

  // Suggested action (informational only — Injection does not enforce)
  suggested_action: string;
  // e.g. "Review with team before proceeding"
};
```

---

## 6. Hook Output Types

### 6.1 SessionStartOutput

Full decision pack injected at session start.

```typescript
type SessionStartOutput = {
  hookSpecificOutput: {
    hookEventName: "SessionStart";
    additionalContext: string;         // Rendered markdown of DecisionPack
  };
};
```

**Rendered markdown format:**

```markdown
## Active Decisions (N)

- **db.engine**=sqlite — embedded, zero-config for CLI tool
  `[human | LOW reversibility | affects: crates/edda-ledger/**]`
- **error.pattern**=thiserror+anyhow — axum idiomatic, typed errors
  `[agent_approved | MEDIUM reversibility]`
...

## Experimental Decisions (M)  <!-- only in plan/review stages -->

- **cache.strategy**=lru — trial run for query caching
  `[experimental | HIGH reversibility]`
```

### 6.2 FileWarningOutput

Single-decision warning for PreToolUse (Edit/Write).

```typescript
type FileWarningOutput = {
  hookSpecificOutput: {
    hookEventName: "PreToolUse";
    additionalContext: string;         // Single-line warning
  };
};
```

**Rendered format:**

```text
[Decision] db.engine=sqlite governs this file (crates/edda-ledger/**). Ensure changes align.
```

### 6.3 LightweightOutput

Compact list for UserPromptSubmit.

```typescript
type LightweightOutput = {
  hookSpecificOutput: {
    hookEventName: "UserPromptSubmit";
    additionalContext: string;         // Compact markdown
  };
};
```

**Rendered format:**

```text
Relevant decisions: db.engine=sqlite, error.pattern=thiserror+anyhow, auth.strategy=JWT
```

---

## 7. Canonical Examples

### Example 1: DecisionPack (implement stage)

```json
{
  "stage": "implement",
  "branch": "main",
  "generated_at": "2026-03-19T14:30:00Z",
  "context_summary": "1 file path (crates/edda-ledger/src/lib.rs), domain: db",
  "items": [
    {
      "decision": {
        "key": "db.engine",
        "value": "sqlite",
        "reason": "embedded, zero-config for CLI tool",
        "domain": "db",
        "status": "active",
        "authority": "human",
        "reversibility": "low",
        "affected_paths": ["crates/edda-ledger/**"],
        "tags": ["architecture", "storage"],
        "propagation": "local",
        "event_id": "evt_01JE7X...",
        "branch": "main",
        "ts": "2025-12-01T10:00:00Z"
      },
      "score": 0.82,
      "match_signals": [
        { "type": "path_match", "path": "crates/edda-ledger/src/lib.rs", "glob": "crates/edda-ledger/**" },
        { "type": "domain_match", "domain": "db" },
        { "type": "authority", "authority": "human" },
        { "type": "reversibility", "reversibility": "low" }
      ]
    },
    {
      "decision": {
        "key": "db.schema",
        "value": "jsonl",
        "reason": "append-only event log, human-readable",
        "domain": "db",
        "status": "active",
        "authority": "human",
        "reversibility": "low",
        "affected_paths": ["crates/edda-ledger/**"],
        "tags": ["architecture", "storage"],
        "propagation": "local",
        "event_id": "evt_01JE8Y...",
        "branch": "main",
        "ts": "2025-12-01T10:05:00Z"
      },
      "score": 0.78,
      "match_signals": [
        { "type": "path_match", "path": "crates/edda-ledger/src/lib.rs", "glob": "crates/edda-ledger/**" },
        { "type": "domain_match", "domain": "db" }
      ]
    }
  ],
  "total_candidates": 12,
  "included": 2,
  "conflicts": []
}
```

### Example 2: ScoredDecisionView with conflict (review stage)

```json
{
  "decision": {
    "key": "db.engine",
    "value": "sqlite",
    "domain": "db",
    "status": "active",
    "authority": "human",
    "event_id": "evt_01JE7X..."
  },
  "score": 0.82,
  "match_signals": [
    { "type": "path_match", "path": "crates/edda-ledger/src/lib.rs", "glob": "crates/edda-ledger/**" }
  ]
}
```

With a `ConflictExplanation`:

```json
{
  "conflict": {
    "existing_event_id": "evt_01JE7X...",
    "existing_value": "sqlite",
    "existing_branch": "main",
    "existing_authority": "human",
    "existing_ts": "2025-12-01T10:00:00Z",
    "conflict_type": "cross_branch"
  },
  "summary": "db.engine has conflicting values: sqlite (main) vs postgres (feat/migration)",
  "suggested_action": "Resolve before merging feat/migration into main"
}
```

---

## 8. Boundaries / Out of Scope

### In Scope
- `RetrievalContext`, `InjectionStage`, `TaskContext` (input types)
- `ScoredDecisionView`, `MatchSignal`, `RankingWeights` (scoring types)
- `DecisionPack`, `ConflictExplanation` (pack types)
- Hook output shapes for SessionStart, PreToolUse, UserPromptSubmit
- Rendered markdown formats

### Out of Scope
- **`DecisionView` definition** -> `../decision-model/shared-types.md` section 2.3
- **`ConflictInfo` definition** -> `../decision-model/shared-types.md` section 3.2
- **Storage types** (`DecisionRow`, mutations) -> Decision Model
- **HTTP API routes** -> `api.md`

---

## Closing Line

> **3 input types, 3 scoring types, 3 pack types, 3 hook output shapes. All transient, all read-only, all built from `DecisionView`. If you're persisting these types, you're doing it wrong.**
