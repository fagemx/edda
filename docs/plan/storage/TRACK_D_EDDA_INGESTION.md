# Track D: Edda Ingestion Engine

> Batch 3（依賴 Track B: Cross-Layer IDs，可與 Track C 並行）
> Repo: `C:\ai_agent\edda`
> Language: **Rust**
> Spec: `thyra/docs/storage/edda-ingestion-triggers-v0.md`

## 核心設計

Build the trigger evaluation engine that decides which decision events auto-write to Edda's ledger, which get queued for human review, and which are silently dropped.

Edda is a Rust project with crate-based architecture (`crates/`). This track adds ingestion infrastructure as a new crate or extends an existing one.

---

## Step 1: Trigger Tables + Evaluator

**Crate**: `crates/edda-ingestion/` (new crate) or extend `crates/edda-store/`

**Reference**: `thyra/docs/storage/edda-ingestion-triggers-v0.md` §4-6

**Key changes**:

1. Define trigger classification types:
```rust
pub enum TriggerResult {
    AutoIngest,
    SuggestIngest { reason: String },
    Skip,
}

pub struct SourceRef {
    pub layer: String,   // "L0" | "L1" | "L2" | "L3" | "L4" | "L5"
    pub kind: String,    // e.g. "decision-session", "commit-memo"
    pub id: String,      // e.g. "ds_abc123"
    pub note: Option<String>,
}
```

2. Define ingestion record:
```rust
pub struct IngestionRecord {
    pub id: String,              // prec_... or dec_...
    pub trigger_type: String,    // "auto" | "suggested" | "manual"
    pub event_type: String,      // from trigger tables
    pub source_layer: String,    // "L1" | "L2" | "L3" | "L4"
    pub source_refs: Vec<SourceRef>,
    pub summary: String,
    pub detail: serde_json::Value,
    pub tags: Vec<String>,
    pub created_at: String,
}
```

3. Implement `evaluate_trigger(event_type: &str, source_layer: &str) -> TriggerResult`:

**Auto-ingest triggers** (9):
| Source | Event Type | Trigger |
|--------|-----------|---------|
| L1 | `decision.commit` | Commit memo created with verdict=commit |
| L1 | `decision.discard` | Candidate discarded with reason |
| L1 | `decision.promotion` | Promotion check completed |
| L1 | `decision.rollback` | Promotion rollback triggered |
| L4 | `outcome.harmful` | Outcome verdict = harmful |
| L4 | `runtime.rollback` | Change rolled back |
| L4 | `governance.patch.v1` | Governance adjustment applied |
| L4 | `safety.violation` | Safety invariant violation |
| L2 | `design.type_change` | shared-types.md major revision |

**Suggest-ingest triggers** (8):
| Source | Event Type | Reason |
|--------|-----------|--------|
| L1 | route changed | May indicate routing anti-pattern |
| L1 | ambiguous probe signal | Possible false positive |
| L1 | 3+ candidates pruned | Space-builder quality issue |
| L4 | inconclusive outcome | Metrics may be wrong |
| L4 | same change kind 3+ times | Change not solving problem |
| L4 | chief permission escalation | Privilege creep worth reviewing |
| L2/L3 | spec patched 3+ times | Concept instability |
| L2/L3 | planning track suspended | Downstream instability signal |

**Never-ingest** (8): follow-up drafts, snapshot updates, ranking iterations, probe draft iterations, spec typo fixes, task status changes, individual pulse frames, normal cycle completions.

4. Implement `write_ingestion_record(record: IngestionRecord)` → append to Edda ledger.

### Acceptance Criteria
```bash
cargo build -p edda-ingestion
cargo test -p edda-ingestion
# evaluate_trigger returns correct TriggerResult for all 25 cases (9 auto + 8 suggest + 8 never)
# write_ingestion_record appends to ledger
# SourceRef validation rejects invalid layers
```

---

## Step 2: Suggestion Queue

**Crate**: Same as Step 1

**Reference**: `edda-ingestion-triggers-v0.md` §8 (suggestion flow)

**Key changes**:

1. Define suggestion type:
```rust
pub struct Suggestion {
    pub id: String,              // sug_...
    pub event_type: String,
    pub source_refs: Vec<SourceRef>,
    pub summary: String,
    pub suggested_because: String,
    pub status: SuggestionStatus,
    pub reviewed_at: Option<String>,
}

pub enum SuggestionStatus {
    Pending,
    Accepted,
    Rejected,
}
```

2. Implement `SuggestionQueue`:
```rust
impl SuggestionQueue {
    pub fn enqueue(suggestion: Suggestion) -> Result<String>;
    pub fn list_pending() -> Result<Vec<Suggestion>>;
    pub fn accept(id: &str) -> Result<IngestionRecord>;  // → writes to ledger
    pub fn reject(id: &str) -> Result<()>;                // → discards, no write
}
```

3. Accepted suggestions become normal `IngestionRecord`s (trigger_type = "suggested").
   Rejected suggestions are marked rejected but NOT written to the ledger.

### Acceptance Criteria
```bash
cargo test -p edda-ingestion
# enqueue creates pending suggestion
# accept converts to ingestion record + writes to ledger
# reject marks rejected, does NOT write to ledger
# list_pending returns only status=Pending
```

---

## Step 3: API Routes + Tests

**Crate**: `crates/edda-serve/` (extend existing HTTP layer) or `crates/edda-ingestion/`

**Reference**: Existing Edda API patterns

**Key changes**:

1. API routes:
```
POST   /api/ingestion/evaluate    ← evaluate trigger for an event
POST   /api/ingestion/records     ← manual ingestion (bypass trigger)
GET    /api/ingestion/records     ← list ingestion records (with filters)
GET    /api/ingestion/suggestions ← list pending suggestions
POST   /api/ingestion/suggestions/:id/accept
POST   /api/ingestion/suggestions/:id/reject
```

2. Ingestion flow integration:
```
event arrives → POST /api/ingestion/evaluate
  → auto: write record immediately, return { action: "ingested", record_id }
  → suggest: create suggestion, return { action: "queued", suggestion_id }
  → skip: return { action: "skipped" }
```

3. Integration tests covering the full flow:
- Auto-ingest: commit memo event → record appears in ledger
- Suggest-ingest: route change event → suggestion queued → accept → record in ledger
- Never-ingest: follow-up draft event → skipped, nothing written

### Acceptance Criteria
```bash
cargo build
cargo test -p edda-ingestion
# All API routes respond correctly
# Full flow: event → evaluate → ingest/queue/skip
# Integration test: auto event → record exists
# Integration test: suggest event → queue → accept → record exists
# Integration test: never event → nothing written
```

### Git Commit
```
feat(edda-ingestion): add decision event ingestion engine with auto/suggest/never triggers

Implements edda-ingestion-triggers-v0: trigger evaluation for 25 event types,
suggestion queue with accept/reject workflow, and ingestion API routes.
Auto-ingest for critical decisions, suggest-ingest for ambiguous signals,
silent skip for noise events.
```

---

## Track Completion Checklist
- [ ] D1: Trigger tables + evaluator + ingestion writer (9 auto, 8 suggest, 8 never)
- [ ] D2: Suggestion queue with accept/reject workflow
- [ ] D3: API routes + integration tests
- [ ] `cargo build` zero errors
- [ ] `cargo test -p edda-ingestion` all pass
- [ ] `cargo clippy -p edda-ingestion -- -D warnings` zero warnings
