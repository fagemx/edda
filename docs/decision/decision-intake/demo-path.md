# Demo Path — How to validate Intake end-to-end

> Status: `working draft`
> Purpose: Define a concrete, step-by-step demonstration path that proves Intake works correctly from extraction through triage to Governance handoff.
> Shared types: see `../decision-model/shared-types.md`

---

## 1. One-Liner

> **Three demos prove Intake works: extraction finds real decisions, the inbox lets you triage them, and Governance actually promotes/rejects on request.**
>
> **Phase note:** All demos below show Phase 0 behavior (draft files, index addressing, `DraftStatus`). Phase 1 demos (event_id addressing, `DecisionStatus`, `create_candidate()` bridge) require Schema V10 and the Decision Model mutation contract. See `overview.md` §8.

---

## 2. What It's NOT / Common Mistakes

### NOT a test plan

This is a demo path — a sequence of commands a human can run to see the system working. Unit tests and integration tests are implementation concerns, not architecture.

### NOT exhaustive

This covers the happy path and one error case. Edge cases (budget exhaustion, concurrent extraction, network failures) are tested in code, not demo'd.

### NOT a tutorial

This assumes familiarity with Edda's CLI and architecture. For onboarding, see the project README.

---

## 3. Prerequisites

```text
Required:
  - edda workspace initialized (`edda init`)
  - EDDA_LLM_API_KEY set (Anthropic API key)
  - At least one session with a transcript in state/transcripts/

Optional (for cost control demo):
  - EDDA_BG_DAILY_BUDGET_USD=0.50 (default)
  - EDDA_BG_CONFIDENCE_THRESHOLD=0.7 (default)
```

---

## 4. Demo 1: Extraction Pipeline

**Goal:** Prove that bg_extract finds decisions in a session transcript.

### Step 1: Create a mock transcript

```bash
# Create a transcript with clear decision signals
mkdir -p .edda/state/transcripts

cat > .edda/state/transcripts/demo_session.jsonl << 'EOF'
{"type":"human","message":{"content":"What database should we use for this project?"}}
{"type":"assistant","message":{"content":"For a CLI tool like edda, SQLite is the best choice. It's embedded, requires zero configuration, and stores everything in a single file. Let me record this decision."}}
{"type":"assistant","message":{"content":"I'll also use thiserror for error types in library crates and anyhow in binary crates. This is the standard Rust error handling pattern."},"tool_input":{"command":"edda decide \"error.pattern=thiserror+anyhow\" --reason \"for now\""}}
{"type":"human","message":{"content":"Sounds good. Let's also make sure we use structured JSON logging."}}
{"type":"assistant","message":{"content":"Agreed. I'll configure tracing with JSON output for production builds. This makes log aggregation much easier."}}
EOF
```

### Step 2: Run extraction manually

```bash
# Trigger extraction (normally happens at SessionEnd)
EDDA_BG_ENABLED=1 edda extract --session demo_session

# Expected output:
# Extracted 3 candidates from demo_session:
#   1. db.engine = sqlite (confidence: 0.92)
#   2. error.pattern = thiserror+anyhow [enhancement] (confidence: 0.88)
#   3. logging.format = structured_json (confidence: 0.78)
```

### Step 3: Verify artifacts (Phase 0: draft files)

```bash
# Check draft decisions file (Phase 0 source of truth)
cat .edda/state/draft_decisions/demo_session.json
# Should contain 3 decisions with DraftStatus "pending"

# Check extraction state (idempotency marker)
cat .edda/state/bg_extract.demo_session.json
# Should show status "completed" with transcript hash

# Check daily cost
cat .edda/state/bg_daily_cost.json
# Should show today's date with cost > 0

# Check audit log
cat .edda/state/bg_audit.jsonl
# Should have one entry for this extraction
```

### Step 4: Verify idempotency

```bash
# Run extraction again on same session
EDDA_BG_ENABLED=1 edda extract --session demo_session

# Expected: "Session demo_session already extracted, skipping"
# No new cost incurred, no duplicate candidates
```

---

## 5. Demo 2: Inbox Triage

**Goal:** Prove that humans can list, edit, and manage candidates through the inbox CLI.

### Step 1: List candidates

```bash
edda inbox list

# Expected:
#   #  key                value              confidence  source          status
#   1  db.engine          sqlite             0.92        demo_session    pending
#   2  error.pattern      thiserror+anyhow   0.88        demo_session    pending
#   3  logging.format     structured_json    0.78        demo_session    pending
```

### Step 2: Show details

```bash
edda inbox show 1

# Expected:
#   Key:          db.engine
#   Value:        sqlite
#   Reason:       embedded, zero-config, single-file for CLI tool
#   Confidence:   0.92
#   Evidence:     "For a CLI tool like edda, SQLite is the best choice..."
#   Source:       demo_session, turn 1
#   Kind:         extraction
#   Status:       pending
#   Paths:        (none set)
#   Tags:         (none set)
```

### Step 3: Edit candidate before promoting

```bash
# Phase 0: edits update the draft JSON file directly.
# --paths and --tags do NOT call set_affected_paths() / set_tags() on the Decision Model API.
edda inbox edit 1 \
  --paths "crates/edda-ledger/**,crates/edda-store/**" \
  --tags "architecture,storage" \
  --reason "embedded DB, zero-config, single-file deployment — ideal for CLI tool distribution"

# Expected:
#   Updated candidate 1 (db.engine) in draft file:
#     affected_paths: ["crates/edda-ledger/**", "crates/edda-store/**"]
#     tags: ["architecture", "storage"]
#     reason: "embedded DB, zero-config, single-file deployment — ideal for CLI tool distribution"
```

### Step 4: Verify edit persisted

```bash
edda inbox show 1

# Expected: paths and tags now populated, reason updated
```

---

## 6. Demo 3: Governance Handoff

**Goal:** Prove that Intake correctly delegates to Governance for lifecycle transitions.

### Step 1: Promote a candidate

```bash
# Phase 0: uses index addressing, marks draft as accepted,
# then creates the decision via edda decide bridge
edda inbox request-promote 1

# Expected:
#   Promotion requested for candidate 1 (db.engine)
#   → Draft marked as accepted
#   → Decision db.engine=sqlite created as ACTIVE
```

### Step 2: Verify promotion in decisions

```bash
edda ask db.engine

# Expected: shows db.engine=sqlite with status "active"
# The decision is now live and will appear in Injection packs
```

### Step 3: Reject a candidate

```bash
edda inbox request-reject 3

# Expected:
#   Rejection requested for candidate 3 (logging.format)
#   → Draft marked as rejected
#   Decision logging.format=structured_json has been REJECTED
```

### Step 4: Verify inbox state

```bash
edda inbox list

# Expected:
#   #  key                value              confidence  source          status
#   2  error.pattern      thiserror+anyhow   0.88        demo_session    pending
#
# Candidate 1 promoted (no longer in inbox)
# Candidate 3 rejected (no longer in inbox)
# Candidate 2 still pending
```

### Step 5: Check stats

```bash
edda inbox stats

# Expected:
#   Today:     1 extraction, $0.003 spent, $0.497 remaining
#   Pending:   1 candidate across 1 session
#   Accepted:  1 (this session)
#   Rejected:  1 (this session)
```

---

## 7. Demo 4: Confidence Filter

**Goal:** Prove that low-confidence candidates are filtered out.

```bash
# Set a high threshold temporarily
EDDA_BG_CONFIDENCE_THRESHOLD=0.95 edda extract --session demo_session_2

# If LLM returns candidates with confidence 0.78, 0.85, 0.92:
# Only the 0.92 candidate survives (if ≥ 0.95, maybe none survive)

edda inbox list
# Shows only candidates above threshold
```

---

## 8. Demo 5: Enhancement Pipeline

**Goal:** Prove that vague reasons get upgraded.

### Setup

```bash
# Record a decision with a vague reason
edda decide "cache.strategy=redis" --reason "for now"

# Create a transcript discussing the decision in depth
cat > .edda/state/transcripts/enhance_session.jsonl << 'EOF'
{"type":"human","message":{"content":"Why did we pick Redis for caching?"}}
{"type":"assistant","message":{"content":"Redis gives us pub/sub for cache invalidation across multiple worker processes, TTL-based expiry without custom code, and the sorted sets are perfect for our leaderboard feature. It's also the de facto standard for Rust web services via the redis crate."},"tool_input":{"command":"edda decide \"cache.strategy=redis\" --reason \"for now\""}}
EOF
```

### Run extraction

```bash
EDDA_BG_ENABLED=1 edda extract --session enhance_session

# Expected: an enhancement candidate appears
edda inbox show 1
#   Key:             cache.strategy
#   Value:           redis
#   Kind:            enhancement
#   Original Reason: "for now"
#   New Reason:      "pub/sub for cache invalidation, TTL-based expiry, sorted sets for leaderboard, standard redis crate"
#   Confidence:      0.90
```

---

## 9. Verification Checklist

| # | What to Verify | How | Expected |
|---|---------------|-----|----------|
| V1 | Extraction finds decisions | Demo 1, Step 2 | 2+ candidates with confidence > 0.7 |
| V2 | Idempotency works | Demo 1, Step 4 | Second run skips, no duplicates |
| V3 | Daily cost tracked | Demo 1, Step 3 | bg_daily_cost.json shows cost > 0 |
| V4 | Inbox lists candidates | Demo 2, Step 1 | All pending candidates shown |
| V5 | Edit persists metadata | Demo 2, Step 3-4 | Paths and tags saved |
| V6 | Promote delegates to Governance | Demo 3, Step 1-2 | Status becomes `active` |
| V7 | Reject delegates to Governance | Demo 3, Step 3 | Status becomes `rejected` |
| V8 | Confidence filter works | Demo 4 | Low-confidence candidates excluded |
| V9 | Enhancement pipeline works | Demo 5 | Vague reason upgraded with context |
| V10 | Audit log records all extractions | Any demo, check bg_audit.jsonl | Entry per extraction with cost |

---

## 10. Boundaries / Out of Scope

### In Scope

- End-to-end demo of extraction → triage → Governance handoff
- Verification of idempotency, cost tracking, confidence filtering
- Enhancement pipeline demo
- Verification checklist

### Out of Scope

- **Performance benchmarks** → separate concern
- **Concurrent extraction testing** → integration test, not demo
- **MCP tool demo** → requires MCP client setup, separate walkthrough
- **Governance internals** (how promote/reject works) → Governance spec
- **Injection packs** (how promoted decisions appear in context) → Injection spec

---

## Closing Line

> **5 demos, 10 verification points. If extraction finds decisions, the inbox lets you triage them, and Governance promotes on request — Intake is working.**
