# Intake API — How do callers interact with the Intake subsystem?

> Status: `working draft`
> Purpose: Define the CLI commands, MCP tools, and internal functions that constitute the Intake API surface.
> Shared types: see `../decision-model/shared-types.md`

---

## 1. One-Liner

> **Intake exposes 3 surfaces: CLI commands for humans, MCP tools for agents, and Rust functions for hooks. All three funnel into the same pipeline.**

---

## 2. What It's NOT / Common Mistakes

### NOT the Decision Model mutation contract

The Decision Model mutation contract (`../decision-model/api.md`) defines `create_candidate()`, `set_affected_paths()`, etc. Intake CALLS those functions. This file defines Intake's own API — the CLI commands and MCP tools that sit above the mutation contract.

### NOT an HTTP API

Intake operations run locally. There is no HTTP endpoint for extraction or inbox. The MCP server (`edda-mcp`) exposes tools over stdio transport, not HTTP.

### NOT a CRUD interface for decisions

Intake cannot update arbitrary decision fields. It can set paths and tags on `proposed` candidates, edit the reason text, and request transitions. That's it.

---

## 3. API Surface Overview

```text
┌─────────────────────────────────────────────────────────────┐
│                    INTAKE API SURFACE                        │
│                                                             │
│  CLI Commands                MCP Tools                      │
│  ────────────               ──────────                      │
│  edda inbox list            edda_draft_inbox                │
│  edda inbox show <id>       (future: edda_inbox_triage)     │
│  edda inbox edit <id>                                       │
│  edda inbox request-promote <id>                            │
│  edda inbox request-reject <id>                             │
│  edda inbox stats                                           │
│                                                             │
│  Hook Triggers              Internal Functions              │
│  ─────────────              ──────────────────              │
│  SessionEnd                 should_run()                    │
│  (bridge hook)              run_extraction()                │
│                             extract_decisions()             │
│                             list_pending_drafts()           │
│                             accept_decisions()              │
│                             reject_decisions()              │
│                                                             │
├─────────────────────────────────────────────────────────────┤
│  Phase 0: ALL writes go to draft files in                   │
│           state/draft_decisions/{session_id}.json            │
│  Phase 1: ALL paths call into Decision Model contract:      │
│           create_candidate(), set_affected_paths(), set_tags()│
└─────────────────────────────────────────────────────────────┘
```

---

## 4. CLI Commands

### 4.1 `edda inbox list`

List all pending candidates in the inbox.

```text
Usage: edda inbox list [OPTIONS]

Options:
  --all          Show accepted/rejected too (default: pending only)
  --session <id> Filter by session
  --sort <field> Sort by: confidence (default), key, date

Output:
  #  key                value              confidence  source           status
  1  error.pattern      thiserror+anyhow   0.85        ses_abc123       pending
  2  logging.format     structured_json    0.72        ses_def456       pending
```

**Phase 0 implementation:** Calls `list_pending_drafts(project_id)` → reads `state/draft_decisions/*.json`. Candidates are addressed by session_id + array index.

**Phase 1 implementation:** Queries decisions table for `status: "proposed"`. Candidates are addressed by `event_id`.

### 4.2 `edda inbox show <id>`

Show detailed information about one candidate.

```text
Usage: edda inbox show <index_or_event_id>

Output:
  Key:          error.pattern
  Value:        thiserror+anyhow
  Reason:       consistent pattern across 5 crates
  Confidence:   0.85
  Evidence:     User: 'standardize on thiserror for library crates'
  Source:       ses_abc123, turn 12
  Kind:         extraction
  Status:       pending
  Paths:        (none set)
  Tags:         (none set)
```

### 4.3 `edda inbox edit <id>`

Edit a candidate's metadata before requesting promotion.

```text
Usage: edda inbox edit <index_or_event_id> [OPTIONS]

Options:
  --reason <text>     Update the reason text
  --paths <globs>     Set affected paths (comma-separated globs)
  --tags <tags>       Set tags (comma-separated)
  --value <value>     Correct the value
```

**Phase 0 implementation:**
- `<id>` is an array index (e.g., `1`, `2`) resolved via session_id + index
- `--paths`, `--tags`, `--reason`, `--value` → all update the draft JSON file directly
- Does NOT call `set_affected_paths()` or `set_tags()` on the Decision Model API
- **Precondition:** Candidate must have `status: "pending"` in the draft file

**Phase 1 implementation:**
- `<id>` is an `event_id` (e.g., `evt_01JF3Q...`)
- `--paths` → calls `set_affected_paths(event_id, parsed_paths)` (Decision Model API)
- `--tags` → calls `set_tags(event_id, parsed_tags)` (Decision Model API)
- `--reason` and `--value` → calls mutation contract on the decisions table
- **Precondition:** Candidate must have `DecisionStatus: "proposed"`

### 4.4 `edda inbox request-promote <id>`

Request Governance to promote a candidate.

```text
Usage: edda inbox request-promote <index_or_event_id>

Output:
  Promotion requested for evt_01JF3Q...
  → Governance promoted: status active, authority agent_approved
```

**Implementation:**
1. Marks draft as `accepted`
2. Calls Governance `promote(event_id)`
3. Reports result

**Note:** Despite the name, this is a synchronous call to Governance in the current implementation. The "request" framing preserves the boundary — Intake requests, Governance executes.

### 4.5 `edda inbox request-reject <id>`

Request Governance to reject a candidate.

```text
Usage: edda inbox request-reject <index_or_event_id>

Output:
  Rejection requested for evt_01JF3Q...
  → Governance rejected: status rejected
```

**Implementation:** Same pattern as request-promote, but calls `reject(event_id)`.

### 4.6 `edda inbox stats`

Show extraction statistics.

```text
Usage: edda inbox stats

Output:
  Today:     3 extractions, $0.012 spent, $0.488 remaining
  Pending:   5 candidates across 2 sessions
  Accepted:  12 (lifetime)
  Rejected:  8 (lifetime)
  Discarded: 23 (below confidence threshold)
```

**Implementation:** Reads `bg_daily_cost.json` and `bg_audit.jsonl`.

---

## 5. MCP Tools

### 5.1 `edda_draft_inbox` (existing)

List pending draft approval items. Read-only.

```typescript
// Current implementation: reads drafts_dir for governance drafts
// Enhancement: also list decision candidates from state/draft_decisions/

// Input: (none)
// Output: text list of pending items with draft_id, title, stage, approvals
```

**Current state:** Lists governance draft approvals only. Enhancement plan: merge decision candidates into the same listing or add a separate `edda_decision_inbox` tool.

### 5.2 `edda_decision_inbox` (planned)

List and triage decision candidates from the extraction inbox.

```typescript
// Input
type DecisionInboxParams = {
  action?: "list" | "show" | "request-promote" | "request-reject";
  candidate_id?: string;   // required for show/request-promote/request-reject
};

// Output: structured text with candidate details
```

**Note:** The actions use `request-promote` / `request-reject` (not `accept` / `reject`) to match the boundary rule: Intake requests, Governance executes. See `overview.md` §2 "NOT the lifecycle owner".

---

## 6. Internal Functions (Rust API)

### 6.1 Extraction Pipeline

```typescript
// Precondition check — should extraction run for this session?
function should_run(project_id: string, session_id: string): boolean;

// Main extraction entry point — called from background thread
function run_extraction(project_id: string, session_id: string): Result<void>;

// Core extraction — reads transcript, calls LLM, returns structured results
function extract_decisions(
  project_id: string,
  session_id: string,
  api_key: string
): Result<ExtractionResult>;

// Parse LLM JSON output into structured decisions
function parse_llm_decisions(text: string): ExtractedDecision[];
```

### 6.2 Inbox Management

```typescript
// List all sessions with pending drafts
function list_pending_drafts(project_id: string): Result<DraftDecisionFile[]>;

// Accept specific decisions by index
function accept_decisions(
  project_id: string,
  session_id: string,
  indices: number[]
): Result<ExtractedDecision[]>;

// Accept all pending decisions for a session
function accept_all_decisions(
  project_id: string,
  session_id: string
): Result<ExtractedDecision[]>;

// Reject specific decisions by index
function reject_decisions(
  project_id: string,
  session_id: string,
  indices: number[]
): Result<void>;
```

### 6.3 Cost Control

```typescript
// Check if daily budget allows another extraction
function check_daily_budget(project_id: string): Result<boolean>;

// Update daily cost after an extraction
function update_daily_cost(project_id: string, cost_usd: number): Result<void>;
```

---

## 7. Error Handling

| Error | Cause | Recovery |
|-------|-------|----------|
| `EDDA_LLM_API_KEY not set` | Missing API key env var | Set the env var |
| `Transcript not found` | Session has no stored transcript | Expected for short sessions, skip |
| `Daily budget exhausted` | `total_usd >= EDDA_BG_DAILY_BUDGET_USD` | Wait for next day or increase budget |
| `No draft decisions for session` | Accept/reject called on missing file | Session already triaged or never extracted |
| `Anthropic API request failed` | Network or API error | Logged, extraction marked as failed |
| `candidate not found` | Edit/promote called on invalid ID | Show error, list valid candidates |

---

## 8. Canonical Examples

### Example 1: Full CLI triage workflow

```text
# 1. List pending candidates
$ edda inbox list
  #  key                value              confidence  source
  1  error.pattern      thiserror+anyhow   0.85        ses_abc123
  2  logging.format     structured_json    0.72        ses_def456
  3  api.versioning     url_prefix         0.91        ses_abc123

# 2. Show details for candidate 1
$ edda inbox show 1
  Key:          error.pattern
  Value:        thiserror+anyhow
  Reason:       consistent pattern across 5 crates
  Confidence:   0.85
  Evidence:     User: 'standardize on thiserror for library crates'
  ...

# 3. Edit before promoting
$ edda inbox edit 1 --paths "crates/*/src/lib.rs,crates/*/src/error.rs" --tags architecture,error-handling
  Updated evt_01JF3Q... affected_paths and tags.

# 4. Promote
$ edda inbox request-promote 1
  Promotion requested for evt_01JF3Q...
  → Governance promoted: status active, authority agent_approved

# 5. Reject candidate 2
$ edda inbox request-reject 2
  Rejection requested for evt_01JG4R...
  → Governance rejected: status rejected
```

### Example 2: MCP agent interaction

```json
// Agent calls edda_draft_inbox tool
// Request: {}
// Response:
{
  "content": [
    {
      "type": "text",
      "text": "Decision candidates:\n  1  error.pattern = thiserror+anyhow (0.85) [pending]\n  2  api.versioning = url_prefix (0.91) [pending]\n\nGovernance drafts:\n  drf_xyz | Add auth module | stage: lead (lead) | approvals: 0/1"
    }
  ]
}
```

---

## 9. Boundaries / Out of Scope

### In Scope

- CLI command definitions (inbox list/show/edit/request-promote/request-reject/stats)
- MCP tool definitions (edda_draft_inbox, planned edda_decision_inbox)
- Internal Rust function signatures (extraction, inbox management, cost control)
- Error handling patterns
- Relationship to Decision Model mutation contract

### Out of Scope

- **Decision Model mutation contract** (`create_candidate()`, etc.) → `../decision-model/api.md`
- **Governance transition execution** (`promote()`, `reject()`) → Governance spec
- **HTTP endpoints** → `edda-serve`, not Intake
- **Query API** (query_by_path, packs) → Injection spec
- **CLI rendering** (colors, table formatting) → presentation layer

---

## Closing Line

> **6 CLI commands, 1 MCP tool (soon 2), 8 internal functions. In Phase 0, writes go to draft files. In Phase 1, every path funnels through `create_candidate()` for writes and Governance for transitions.**
