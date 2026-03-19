# Schema v0 — Intake-owned types and storage

> Status: `working draft`
> Purpose: Define types that Intake owns — extraction results, draft files, inbox state. Shared types (DecisionPayload, MutationResult, etc.) are NOT redefined here.
> Shared types: see `../decision-model/shared-types.md`

---

## 1. One-Liner

> **Intake owns extraction artifacts and inbox state. Decision identity and lifecycle types belong to Decision Model.**

---

## 2. What It's NOT / Common Mistakes

### NOT a redefinition of shared types

`DecisionPayload`, `DecisionStatus`, `DecisionAuthority`, `MutationResult` are defined in `../decision-model/shared-types.md`. This file defines ONLY Intake-specific types that don't exist in the shared model.

### NOT storage schema for decisions

Decisions are stored in the `decisions` table via `DecisionRow` (shared-types.md §2.2). Intake's storage is the draft decisions directory and extraction state files — ephemeral artifacts that exist only during the triage phase.

### NOT the inbox UI model

These types describe what gets persisted. How the inbox CLI renders them (columns, colors, sorting) is a presentation concern.

---

## 3. Intake-Owned Types

### 3.1 ExtractedDecision

A single decision found by the LLM extraction pipeline. This is the raw output before it becomes a `DecisionPayload`.

```typescript
type ExtractedDecision = {
  key: string;                    // "error.pattern" — dotted domain.aspect
  value: string;                  // "thiserror+anyhow"
  reason?: string;                // LLM-generated reason from transcript context
  confidence: number;             // 0.0–1.0, LLM's certainty this is a real decision
  evidence: string;               // transcript quote supporting the extraction
  source_turn: number;            // which turn in the transcript (0-indexed)
  status: DraftStatus;            // triage state within Intake
  kind: DecisionKind;             // extraction or enhancement
  original_reason?: string;       // for enhancements: the vague reason being replaced
};
```

### 3.2 DraftStatus

Intake's internal triage state. NOT the same as `DecisionStatus` from the shared model.

> **Phase note:** `DraftStatus` is Phase 0 only. In Phase 1 (Schema V10), `DraftStatus` is replaced by `DecisionStatus` from `../decision-model/shared-types.md`. Candidates are created via `create_candidate()` with `DecisionStatus: "proposed"`, and draft files become an optional preview cache — not the source of truth. See `overview.md` §8 for the phased implementation plan.

```typescript
// Intake-internal, Phase 0 only — DO NOT confuse with DecisionStatus (shared-types.md §1.1)
type DraftStatus =
  | "pending"      // awaiting human review in inbox
  | "accepted"     // human approved → will be sent to Governance for promotion
  | "rejected";    // human declined → will be sent to Governance for rejection
```

**Mapping to Decision Model:**

| DraftStatus | Action | DecisionStatus after |
|------------|--------|---------------------|
| `pending` | (no action yet) | remains `proposed` |
| `accepted` | Intake calls `request-promote` → Governance calls `promote()` | `active` |
| `rejected` | Intake calls `request-reject` → Governance calls `reject()` | `rejected` |

### 3.3 DecisionKind

Distinguishes new extractions from reason enhancements.

```typescript
type DecisionKind =
  | "extraction"     // a new decision found by the extractor
  | "enhancement";   // an improved reason for an already-recorded decision
```

### 3.4 ExtractionResult

The full output of one extraction run against a session transcript.

```typescript
type ExtractionResult = {
  session_id: string;             // which session was analyzed
  decisions: ExtractedDecision[]; // all candidates (before confidence filter)
  transcript_hash: string;        // blake3 hash for idempotency
  extracted_at: string;           // ISO 8601
  model_used: string;             // "claude-3-5-haiku-20241022"
  input_tokens: number;           // for cost tracking
  output_tokens: number;
  cost_usd: number;               // computed from token counts
};
```

### 3.5 DraftDecisionFile

The persisted file for one session's draft candidates. Stored at `state/draft_decisions/{session_id}.json`.

```typescript
type DraftDecisionFile = {
  session_id: string;
  extracted_at: string;           // ISO 8601
  model: string;                  // LLM model used
  decisions: ExtractedDecision[]; // only those above confidence threshold
};
```

### 3.6 ExtractionState

Idempotency marker. Stored at `state/bg_extract.{session_id}.json`.

```typescript
type ExtractionState = {
  status: "completed" | "pending" | "failed";
  extracted_at: string;           // ISO 8601
  transcript_hash: string;        // blake3 hash of transcript at extraction time
  decisions_count: number;        // how many candidates were found
};
```

### 3.7 DailyCost

Daily budget tracker. Stored at `state/bg_daily_cost.json`.

```typescript
type DailyCost = {
  date: string;                   // "2026-03-19" (UTC)
  total_usd: number;              // cumulative cost today
  calls: number;                  // number of LLM calls today
};
```

### 3.8 AuditEntry

Append-only log for extraction history. Stored at `state/bg_audit.jsonl`.

```typescript
type AuditEntry = {
  ts: string;                     // ISO 8601
  session_id: string;
  decisions_found: number;        // after confidence filter
  cost_usd: number;
  model: string;
  status: "completed" | "failed";
};
```

---

## 4. Storage Layout

```text
.edda/
└── state/
    ├── draft_decisions/
    │   ├── ses_abc123.json         ← DraftDecisionFile
    │   └── ses_def456.json
    ├── bg_extract.ses_abc123.json  ← ExtractionState (idempotency)
    ├── bg_extract.ses_def456.json
    ├── bg_daily_cost.json          ← DailyCost (resets daily)
    └── bg_audit.jsonl              ← AuditEntry (append-only)
```

### Lifecycle of storage artifacts

| Artifact | Created | Updated | Deleted |
|----------|---------|---------|---------|
| DraftDecisionFile | extraction completes | triage (accept/reject) | after all decisions triaged |
| ExtractionState | extraction completes | never (immutable) | manual cleanup |
| DailyCost | first extraction of the day | each extraction | auto-reset next day |
| AuditEntry | each extraction | never (append-only) | manual cleanup |

---

## 5. Configuration

All configuration via environment variables with defaults:

```typescript
type IntakeConfig = {
  EDDA_BG_ENABLED: "0" | "1";                    // default: "1"
  EDDA_LLM_API_KEY: string;                       // required for extraction
  EDDA_BG_MODEL: string;                          // default: "claude-3-5-haiku-20241022"
  EDDA_BG_CONFIDENCE_THRESHOLD: number;           // default: 0.7
  EDDA_BG_DAILY_BUDGET_USD: number;               // default: 0.50
  EDDA_BG_MAX_TRANSCRIPT_CHARS: number;           // default: 30000
};
```

---

## 6. Relationship to Shared Types

| Intake Type | Related Shared Type | Relationship |
|-------------|-------------------|-------------|
| `ExtractedDecision` | `DecisionPayload` (§2.1) | Intake converts extracted → payload for `create_candidate()` |
| `DraftStatus` | `DecisionStatus` (§1.1) | `DraftStatus` is Intake-internal triage; `DecisionStatus` is lifecycle |
| `ExtractionResult.cost_usd` | (none) | Intake-only cost tracking |
| `DraftDecisionFile` | (none) | Intake-only storage artifact |

### Conversion: ExtractedDecision → create_candidate params

```typescript
function to_candidate_params(
  extracted: ExtractedDecision,
  branch: string,
  session_id: string
): CreateCandidateParams {
  return {
    key: extracted.key,
    value: extracted.value,
    reason: extracted.reason,
    branch: branch,
    authority: "agent_proposed",  // see shared-types.md §1.2
    tags: infer_tags(extracted.key),
    reversibility: "medium",      // default; can be overridden during triage
    session_id: session_id,
  };
}
```

---

## 7. Canonical Examples

### Example 1: DraftDecisionFile on disk

```json
{
  "session_id": "ses_abc123",
  "extracted_at": "2026-03-19T14:30:00Z",
  "model": "claude-3-5-haiku-20241022",
  "decisions": [
    {
      "key": "error.pattern",
      "value": "thiserror+anyhow",
      "reason": "consistent pattern across 5 crates",
      "confidence": 0.85,
      "evidence": "User: 'standardize on thiserror for library crates'",
      "source_turn": 12,
      "status": "pending",
      "kind": "extraction"
    },
    {
      "key": "db.engine",
      "value": "sqlite",
      "original_reason": "for now",
      "reason": "embedded DB, zero-config, single-file deployment for CLI tool",
      "confidence": 0.90,
      "evidence": "User: 'SQLite is perfect because we want zero dependencies'",
      "source_turn": 5,
      "status": "pending",
      "kind": "enhancement"
    }
  ]
}
```

### Example 2: ExtractionState (idempotency marker)

```json
{
  "status": "completed",
  "extracted_at": "2026-03-19T14:30:00Z",
  "transcript_hash": "blake3:a1b2c3d4e5f6...",
  "decisions_count": 2
}
```

### Example 3: DailyCost at end of day

```json
{
  "date": "2026-03-19",
  "total_usd": 0.487,
  "calls": 14
}
```

---

## 8. Boundaries / Out of Scope

### In Scope

- All Intake-owned types (ExtractedDecision, DraftStatus, DecisionKind, etc.)
- Storage layout and file locations
- Configuration variables and defaults
- Conversion from Intake types to shared model types

### Out of Scope

- **DecisionPayload, DecisionRow, DecisionView** → shared-types.md (do not redefine)
- **MutationResult, ConflictInfo** → shared-types.md (do not redefine)
- **SQLite schema for decisions table** → Decision Model schema-v0.md
- **Inbox CLI rendering** → presentation layer

---

## Closing Line

> **8 types, 4 storage files, 6 config vars. Intake owns the extraction artifacts. The decision itself — its identity, status, and lifecycle — belongs to Decision Model.**
