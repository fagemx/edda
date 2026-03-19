# Canonical Form — What does one candidate's journey through Intake look like?

> Status: `working draft`
> Purpose: Define the extraction-to-inbox pipeline, the triage workflow, and the handoff to Governance.
> Shared types: see `../decision-model/shared-types.md`

---

## 1. One-Liner

> **Every candidate follows the same path: extract → filter → create → triage → request. No shortcuts.**

---

## 2. What It's NOT / Common Mistakes

### NOT a decision lifecycle

The decision lifecycle (proposed → active → superseded → ...) is owned by Governance. See `../decision-model/canonical-form.md`. Intake's canonical form is the candidate pipeline — what happens BEFORE Governance takes over.

### NOT a batch import

Intake processes candidates one at a time. Even if the LLM extracts 5 decisions from one transcript, each one is individually filtered, created (Phase 0: saved to draft file; Phase 1: via `create_candidate()`), and triaged.

### NOT a guaranteed path to `active`

Most extracted candidates will be rejected or ignored. The funnel narrows at every stage: LLM extraction → confidence filter → dedup check → human triage → Governance approval. Each stage can drop a candidate.

### NOT synchronous

Extraction runs in a background thread (`std::thread::spawn` at `SessionEnd`). The human triage step happens hours or days later. There is no "extract and promote in one call."

---

## 3. Pipeline Stages

### Stage 1: Signal Detection

Before extraction runs, the system checks preconditions:

```text
SessionEnd hook fires
  │
  ├── EDDA_BG_ENABLED == "0"?          → skip
  ├── EDDA_LLM_API_KEY missing?        → skip
  ├── signal_count == 0?               → skip (no nudge signals)
  ├── already_extracted(session_id)?    → skip (idempotent)
  ├── daily_budget exhausted?           → skip
  │
  └── All checks pass → spawn background extraction thread
```

### Stage 2: LLM Extraction

The extraction pipeline:

```text
┌──────────────────────────────────────────────────┐
│  Background Thread                               │
│                                                  │
│  1. Read transcript JSONL                        │
│  2. Assemble user/assistant turns into text      │
│  3. Truncate to max chars (30K default)          │
│  4. Find recorded decisions with vague reasons   │
│  5. Build extraction prompt                      │
│  6. Call Haiku LLM                               │
│  7. Parse JSON response                          │
│  8. Return Vec<ExtractedDecision>                │
└──────────────────────────────────────────────────┘
```

LLM output per candidate:

```typescript
// See ../decision-model/shared-types.md §1.2 for DecisionAuthority
type ExtractedDecision = {
  key: string;           // "error.pattern"
  value: string;         // "thiserror+anyhow"
  reason?: string;       // "consistent pattern across 5 crates"
  confidence: number;    // 0.0–1.0
  evidence: string;      // transcript quote
  source_turn: number;   // which turn in the transcript
  status: DraftStatus;   // always "pending" at creation
  kind: DecisionKind;    // "extraction" or "enhancement"
  original_reason?: string; // for enhancements: the vague reason being replaced
};

type DraftStatus = "pending" | "accepted" | "rejected";
type DecisionKind = "extraction" | "enhancement";
```

### Stage 3: Confidence Filter

```text
for each ExtractedDecision:
  if confidence < EDDA_BG_CONFIDENCE_THRESHOLD (default 0.7):
    discard — never reaches inbox
  else:
    proceed to Stage 4
```

### Stage 4: Candidate Creation

> **Phase 0 vs Phase 1:** This stage differs depending on the implementation phase. See `overview.md` §8 for the full phased implementation plan.

#### Phase 0 (current) — Draft file storage

Each surviving candidate is saved to a draft decision file:

```text
state/draft_decisions/{session_id}.json
```

The file contains a `DraftDecisionFile` with all candidates for that session. Each candidate has `status: DraftStatus::Pending`. No Decision Model API is called. The draft file is the sole source of truth.

#### Phase 1 (target) — Decision Model bridge (requires Schema V10)

Each surviving candidate is passed to `create_candidate()`:

```typescript
// See ../decision-model/api.md §4.1
create_candidate({
  key: extracted.key,
  value: extracted.value,
  reason: extracted.reason,
  branch: current_branch,
  authority: "agent_proposed",      // always agent_proposed for extraction
  tags: inferred_tags(extracted),
  reversibility: inferred_reversibility(extracted),
  session_id: session_id,
});
// Result: DecisionStatus = "proposed", addressed by event_id
```

Draft files may still be written as a preview cache, but the decisions table is the source of truth.

### Stage 5: Inbox Triage (Human)

#### Phase 0 (current) — draft file addressing

```text
$ edda inbox list
  → reads state/draft_decisions/*.json, shows candidates with DraftStatus "pending"
  → addressed by session_id + array index (e.g., #1, #2)

$ edda inbox edit 1 --reason "better reason" --paths "crates/foo/**"
  → updates the draft JSON file directly (reason, paths, tags fields)
  → does NOT call set_affected_paths() or set_tags() on the Decision Model API

$ edda inbox request-promote 1
  → marks draft as DraftStatus::Accepted
  → creates the decision via edda decide (or future Governance bridge)

$ edda inbox request-reject 1
  → marks draft as DraftStatus::Rejected
```

#### Phase 1 (target) — event_id addressing (requires Schema V10)

```text
$ edda inbox list
  → queries decisions table for status "proposed"
  → addressed by event_id (e.g., evt_01JF3Q...)

$ edda inbox edit evt_01JF3Q... --reason "better reason" --paths "crates/foo/**"
  → calls set_affected_paths(event_id, paths) on Decision Model API
  → calls set_tags(event_id, tags) on Decision Model API

$ edda inbox request-promote evt_01JF3Q...
  → sends transition request to Governance
  → Governance calls promote(event_id)

$ edda inbox request-reject evt_01JF3Q...
  → sends transition request to Governance
  → Governance calls reject(event_id)
```

---

## 4. Full Pipeline Diagram

```text
 SessionEnd
     │
     ▼
 ┌─────────────┐
 │ should_run? │──── no ──▶ (exit)
 └──────┬──────┘
        │ yes
        ▼
 ┌─────────────┐
 │ read        │
 │ transcript  │
 └──────┬──────┘
        │
        ▼
 ┌─────────────┐
 │ call Haiku  │──── error ──▶ log failure, exit
 │ (LLM edge)  │
 └──────┬──────┘
        │
        ▼
 ┌─────────────┐
 │ parse JSON  │──── invalid ──▶ empty result, exit
 └──────┬──────┘
        │
        ▼
 ┌─────────────────┐
 │ filter by       │
 │ confidence ≥ 0.7│──── below ──▶ discard
 └──────┬──────────┘
        │ above
        ▼
 ┌─────────────────┐
 │ save draft      │
 │ decisions file  │
 └──────┬──────────┘
        │
        ▼
 ┌─────────────────┐
 │ update daily    │
 │ cost tracker    │
 └──────┬──────────┘
        │
        ▼
 ┌─────────────────┐
 │ append audit    │
 │ log entry       │
 └─────────────────┘

 ─── (asynchronous gap: hours/days) ───

 Human runs:
     │
     ▼
 ┌─────────────────┐
 │ edda inbox list │
 └──────┬──────────┘
        │
        ▼
 ┌─────────────────────┐
 │ edda inbox edit <id>│  (optional: set paths, tags, reason)
 └──────┬──────────────┘
        │
        ▼
 ┌──────────────────────────┐
 │ edda inbox request-      │
 │ promote/reject <id>      │
 └──────────┬───────────────┘
            │
            ▼
     ┌──────────────┐
     │  GOVERNANCE  │  ← executes promote() or reject()
     └──────────────┘
```

---

## 5. Enhancement Pipeline (Vague Reason Upgrade)

A special Intake subflow that improves existing decisions' reasons:

```text
 During extraction (Stage 2):
     │
     ▼
 ┌───────────────────────────────┐
 │ scan transcript for           │
 │ `edda decide` commands        │
 └──────────────┬────────────────┘
                │
                ▼
 ┌───────────────────────────────┐
 │ filter: is_vague_reason()?   │
 │ (< 15 chars, or exact match  │
 │  to vague patterns)          │
 └──────────────┬────────────────┘
                │ vague decisions found
                ▼
 ┌───────────────────────────────┐
 │ include in LLM prompt with   │
 │ "enhancement" section         │
 └──────────────┬────────────────┘
                │
                ▼
 LLM returns items with kind: "enhancement"
 and original_reason preserved
```

Enhancement candidates follow the same confidence filter and triage workflow as extractions.

---

## 6. Idempotency Guards

| Guard | Where | How |
|-------|-------|-----|
| Transcript hash | `save_extraction_state()` | Same transcript content → skip re-extraction |
| Session already extracted | `already_extracted()` | State file with `status: "completed"` exists → skip |
| Duplicate key detection | `create_candidate()` | Agent-proposed candidate for key with existing active → skip (no duplicate inbox entry) |

---

## 7. Canonical Examples

### Example 1: Complete extraction cycle

```text
1. SessionEnd fires for session "ses_abc123"
2. should_run() checks: enabled=true, API key present, signal_count=3, not extracted, budget=$0.42 remaining
3. Background thread reads transcript (18,000 chars)
4. Finds 1 recorded decision with vague reason: db.engine=sqlite, reason="for now"
5. Calls Haiku with extraction + enhancement prompt
6. LLM returns:
   [
     { "key": "error.pattern", "value": "thiserror+anyhow",
       "reason": "consistent across all crates", "confidence": 0.85,
       "evidence": "User: standardize on thiserror" },
     { "kind": "enhancement", "key": "db.engine", "value": "sqlite",
       "original_reason": "for now",
       "reason": "embedded DB, zero-config, single-file deployment for CLI tool",
       "confidence": 0.90, "evidence": "User: SQLite is perfect for CLI" },
     { "key": "test.style", "value": "integration_first",
       "reason": "maybe try integration tests", "confidence": 0.45,
       "evidence": "vague mention" }
   ]
7. Confidence filter: error.pattern (0.85 pass), db.engine enhancement (0.90 pass), test.style (0.45 DISCARD)
8. Saves 2 draft decisions to state/draft_decisions/ses_abc123.json
9. Updates daily cost: $0.42 + $0.003 = $0.423
10. Audit log entry written
```

### Example 2: Budget exhaustion

```text
1. SessionEnd fires for session "ses_def456"
2. should_run() checks budget: $0.498 spent today, limit $0.50
3. $0.498 < $0.50 → budget OK, extraction proceeds
4. After extraction: cost was $0.004 → $0.502 total
5. Next session ses_ghi789: should_run() checks budget: $0.502 >= $0.50 → SKIP
6. Tomorrow: budget resets to $0.00
```

---

## 8. Boundaries / Out of Scope

### In Scope

- The 5-stage pipeline: signal → extract → filter → create → triage
- Enhancement subflow for vague reasons
- Idempotency guards (hash, state file, dedup)
- Cost control (daily budget, per-call tracking)
- CLI triage workflow (list, edit, request-promote, request-reject)
- Transition request handoff to Governance

### Out of Scope

- **What happens after promote/reject** → Governance lifecycle (`../decision-model/canonical-form.md`)
- **How promoted decisions are queried** → Injection spec
- **Conflict detection during promotion** → Governance spec
- **LLM prompt engineering details** → implementation concern, not architecture
- **Future extraction sources** (PR, issue) — acknowledged as planned, not specified here

---

## Closing Line

> **Five stages, one direction: signal in, candidate out, Governance decides. Intake's pipeline is a funnel, and the narrow end always points at Governance.**
