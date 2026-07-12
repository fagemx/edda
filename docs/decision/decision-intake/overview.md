# Decision Intake — How do candidates enter the system?

> Status: `v0 design spec` — the implementation shipped via the [decision-deepening tracks](../../archive/plans/decision-deepening/00_OVERVIEW.md); where details differ, the code is authoritative.
> Purpose: Define how decision candidates are created, triaged, and prepared for Governance review.
> Shared types: see `../decision-model/shared-types.md`

---

## 1. One-Liner

> **Intake creates candidates, not truth. It extracts, triages, and requests — it never promotes.**

Intake is the funnel through which raw signal (transcripts, plans, PRs, human input) becomes structured `proposed` candidates in the decision inbox. Everything after that — promote, reject, freeze — belongs to Governance.

---

## 2. What It's NOT / Common Mistakes

### NOT a decision engine

Intake does not decide whether a candidate should become active. In Phase 1, it calls `create_candidate()` and stops. In Phase 0 (current), it saves draft files and stops. Either way, the word "proposed" / "pending" means exactly that. Governance calls `promote()`.

### NOT a query/retrieval layer

Intake does not answer "what decisions apply to this file?" That is Injection. Intake's read operations are limited to listing and editing inbox candidates.

### NOT auto-promotion

Even when confidence is 1.0, Intake still creates `proposed` status. There is no auto-promote path. A human must approve via Governance. The only exception is the legacy `edda decide` CLI shortcut where a human directly creates an `active` decision with no prior conflict.

### NOT the lifecycle owner

Intake never calls `promote()`, `reject()`, `transition()`, `freeze()`, or `supersede()`. It may REQUEST a transition (e.g., `request-promote`), but Governance executes it. See `../decision-model/canonical-form.md` for the state machine.

---

## 3. Core Concepts

### Candidate

A decision object with `status: "proposed"` and `authority: "agent_proposed"`. Created by extraction pipelines or the MCP `edda_draft_inbox` tool. Lives in the inbox until Governance acts.

### Extraction Source

Where candidates come from:

| Source | Trigger | Extractor |
|--------|---------|-----------|
| Session transcript | `SessionEnd` hook | `bg_extract.rs` (LLM via Haiku) |
| Plan text | Manual / hook | Future: plan parser |
| PR discussion | PR merge hook | Future: PR decision extractor |
| Issue conclusion | Issue close hook | Future: issue extractor |
| Human CLI | `edda decide` | Direct `create_candidate()` (Phase 1) / direct write (Phase 0) |
| MCP tool | Agent calls `edda_draft_inbox` | Reads existing drafts |

### Inbox

The collection of `proposed` candidates awaiting human review. Surfaced via:
- CLI: `edda inbox list`, `edda inbox edit <id>`, `edda inbox request-promote <id>`
- MCP: `edda_draft_inbox` tool (read-only listing today, will grow)

### Confidence Threshold

Extraction pipeline filter. Candidates below threshold are discarded before reaching the inbox. Default: `0.7` (`EDDA_BG_CONFIDENCE_THRESHOLD`). This is a pre-inbox quality gate, not a governance decision.

### Daily Budget

Cost control for LLM extraction. Default: `$0.50/day` (`EDDA_BG_DAILY_BUDGET_USD`). When exhausted, extraction skips. Budget resets at midnight UTC.

---

## 4. Canonical Form / Flow

```text
 ┌──────────────────────────────────────────────────────────┐
 │                   INTAKE BOUNDARY                        │
 │                                                          │
 │  Sources                 Pipeline              Inbox     │
 │  ─────────              ──────────            ─────────  │
 │                                                          │
 │  transcript ──┐                                          │
 │  plan text  ──┤     ┌───────────────┐    ┌───────────┐  │
 │  PR discuss ──┼────▶│  Extraction   │───▶│ Candidate │  │
 │  issue close──┤     │  (LLM edge)   │    │ proposed  │  │
 │  human CLI  ──┘     └───────┬───────┘    └─────┬─────┘  │
 │                             │                  │         │
 │                    confidence < 0.7?           │         │
 │                        ▼ discard               │         │
 │                                                │         │
 │                                   ┌────────────┴──────┐  │
 │                                   │  Inbox Triage     │  │
 │                                   │  list / edit /    │  │
 │                                   │  set-tags /       │  │
 │                                   │  set-paths        │  │
 │                                   └────────┬──────────┘  │
 │                                            │             │
 ├────────────────────────────────────────────┼─────────────┤
 │              INTAKE → GOVERNANCE           │             │
 │              (request only)                │             │
 │                                            ▼             │
 │                                   request-promote(id)    │
 │                                   request-reject(id)     │
 └──────────────────────────────────────────────────────────┘
                                              │
                                              ▼
                                    ┌──────────────────┐
                                    │   GOVERNANCE     │
                                    │   promote(id)    │
                                    │   reject(id)     │
                                    └──────────────────┘
```

### Step-by-step

1. **Extract** — Source material (transcript, plan, PR) enters the extraction pipeline. LLM (Haiku) runs at the edge, producing structured `ExtractedDecision` objects with confidence scores.
2. **Filter** — Candidates below the confidence threshold (`0.7`) are discarded. Budget check prevents runaway cost.
3. **Create** — Surviving candidates are persisted. In Phase 0 (current): saved to `state/draft_decisions/{session_id}.json` with `DraftStatus::Pending`. In Phase 1: passed to `create_candidate()` with `authority: "agent_proposed"`, status set to `proposed`.
4. **Triage** — Human reviews inbox via CLI or MCP. Can edit reason, set affected paths, set tags.
5. **Request** — Human runs `edda inbox request-promote <id>`. Intake sends a transition request to Governance. Governance calls `promote()` and enforces preconditions.

---

## 5. Mutation Contract Surface

### Phase 0 (current)

Intake writes directly to draft files in `state/draft_decisions/`. It does NOT call any Decision Model API functions.

| Operation | When | Caller |
|-----------|------|--------|
| Write draft file | New candidate from extraction | bg_extract |
| Update draft file | Human edits candidate (paths, tags, reason) | inbox CLI |
| Mark draft accepted/rejected | Human triages candidate | inbox CLI |

### Phase 1 (target — requires Schema V10)

Intake calls exactly 3 write operations from the Decision Model API (`../decision-model/api.md`):

| Operation | When | Caller |
|-----------|------|--------|
| `create_candidate()` | New candidate from extraction or CLI | bg_extract, inbox CLI, MCP |
| `set_affected_paths(id, paths)` | Human edits candidate scope | inbox CLI |
| `set_tags(id, tags)` | Human edits candidate tags | inbox CLI |

In both phases, Intake does NOT call: `promote()`, `reject()`, `transition()`, `supersede()`, `freeze()`, `set_review_after()`.

### Transition Requests

Intake exposes request verbs that delegate to Governance:

```typescript
// These are Intake CLI commands, NOT direct mutations
// Phase 0: id is session_id + index; Phase 1: id is event_id
function request_promote(id: string): void;
function request_reject(id: string): void;
// Internally: calls Governance.promote(id) / Governance.reject(id)
```

---

## 6. Position in the Overall System

```text
┌──────────────┐     ┌──────────────┐     ┌──────────────┐
│   INTAKE     │────▶│  DECISION    │◀────│  GOVERNANCE  │
│              │     │   MODEL      │     │              │
│ create only  │     │ (contract)   │     │ ALL state    │
│              │     │              │     │ transitions  │
└──────────────┘     └──────┬───────┘     └──────────────┘
                            │
                            ▼
                     ┌──────────────┐
                     │  INJECTION   │
                     │  read only   │
                     └──────────────┘
```

| Boundary | Rule |
|----------|------|
| Intake → Model | `create_candidate()`, `set_affected_paths()`, `set_tags()` only |
| Intake → Governance | Request verbs only (Governance executes) |
| Intake → Injection | No direct interaction |
| Injection → Intake | Never. Injection reads `DecisionView`, not inbox |

---

## 7. Canonical Examples

### Example 1: bg_extract finds a decision in a session transcript

```json
// LLM extraction output (raw)
{
  "key": "error.pattern",
  "value": "thiserror+anyhow",
  "reason": "consistent pattern across 5 crates in this workspace",
  "confidence": 0.85,
  "evidence": "User: 'let's standardize on thiserror for library crates'"
}

// After confidence filter (0.85 >= 0.7 threshold), Intake calls:
// create_candidate({
//   key: "error.pattern",
//   value: "thiserror+anyhow",
//   reason: "consistent pattern across 5 crates in this workspace",
//   branch: "main",
//   authority: "agent_proposed",
//   tags: ["architecture", "error-handling"],
//   reversibility: "medium"
// })

// Result: MutationResult
{
  "ok": true,
  "event_id": "evt_01JF3Q...",
  "decision_id": "evt_01JF3Q..."
}
// Decision now in inbox with status "proposed"
```

### Example 2: Human triages inbox and requests promotion

```text
$ edda inbox list
  #  key                value              confidence  source
  1  error.pattern      thiserror+anyhow   0.85        session abc123
  2  logging.format     structured_json    0.72        session def456

$ edda inbox edit 1 --paths "crates/*/src/lib.rs" --tags architecture,error-handling
  Updated evt_01JF3Q... affected_paths and tags.

$ edda inbox request-promote 1
  Promotion requested for evt_01JF3Q...
  → Governance promoted: status active, authority agent_approved
```

### Example 3: Low confidence candidate is discarded

```json
// LLM extraction output
{
  "key": "test.framework",
  "value": "proptest",
  "reason": "mentioned in passing",
  "confidence": 0.45,
  "evidence": "maybe we should try proptest sometime"
}
// 0.45 < 0.7 threshold → discarded, never reaches inbox
```

---

## 8. Phased Implementation

The Intake spec describes a target architecture, but the current codebase implements only Phase 0. This section makes the gap explicit.

### Phase 0 (current) — Draft File System

This is what `bg_extract.rs` implements TODAY.

- **Storage:** `state/draft_decisions/{session_id}.json` (flat JSON files)
- **Addressing:** session_id + array index (e.g., `edda inbox edit 1`)
- **Status type:** `DraftStatus` (`pending` / `accepted` / `rejected`) — Intake-internal only
- **No Decision Model integration:** Extraction does NOT call `create_candidate()`. Draft files are the sole source of truth for inbox candidates.
- **Triage operations** (`--paths`, `--tags`, `--reason`) update the draft JSON file directly. They do NOT call `set_affected_paths()` or `set_tags()` on the Decision Model API.

> **Known boundary violation:** In Phase 0, `edda inbox request-promote` creates the decision via `edda decide` (bootstrap path), going directly to `active` without Governance mediation. This means Phase 0 Intake effectively bypasses the "Governance owns all transitions" rule. This is an accepted transitional compromise — Phase 1 replaces this with a proper Governance request that routes through `gov_promote()`.

### Phase 1 (target) — Decision Model Bridge

Requires Schema V10 and the Decision Model mutation contract to be implemented.

- **Storage:** Extraction calls `create_candidate()` → candidates stored in the decisions table with `status: "proposed"`, `authority: "agent_proposed"`.
- **Addressing:** `event_id` (e.g., `edda inbox edit evt_01JF3Q... --reason "..."`)
- **Status type:** `DecisionStatus` (`proposed`) from `../decision-model/shared-types.md`
- **Draft files become optional:** They may persist as a preview cache / offline fallback, but the decisions table is the source of truth.
- **Triage operations** call `set_affected_paths()` and `set_tags()` on the Decision Model API.

### Migration path

Phase 0 → Phase 1 is NOT automatic. It requires:
1. Schema V10 (decisions table with `create_candidate()` support)
2. A bridge layer in `bg_extract.rs` that calls `create_candidate()` instead of (or in addition to) writing draft files
3. CLI commands updated to resolve `event_id` instead of session_id + index
4. Draft files retained as cache or removed entirely (design decision TBD)

> **When reading other sections of this spec:** Sections that reference `create_candidate()`, `event_id`, or `DecisionStatus` describe Phase 1 behavior. The current implementation is Phase 0.

---

## 9. Boundaries / Out of Scope

### In Scope

- Candidate extraction from session transcripts (bg_extract.rs)
- Candidate extraction from plan text, PR discussion, issue conclusions (future)
- Confidence filtering and daily budget control
- Inbox triage workflow: list, edit, set paths, set tags
- Transition request verbs: request-promote, request-reject
- MCP tool: `edda_draft_inbox` (listing and future enhancements)
- Reason enhancement for vague decisions (DecisionKind::Enhancement)

### Out of Scope

- **Lifecycle transitions** (promote, reject, freeze, supersede) → Governance spec
- **Decision querying** (query_by_path, query_active, packs) → Injection spec
- **Decision schema** (what fields exist, types) → Decision Model spec
- **Conflict detection** → Governance spec
- **Decision rendering** in UI/CLI → presentation layer

---

## Closing Line

> **Intake is a funnel, not a judge. It catches decisions at the edge, structures them, and hands them to Governance for verdict. If Intake ever calls `promote()`, something is deeply broken.**
