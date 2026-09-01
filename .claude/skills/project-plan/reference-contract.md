# Reference: CONTRACT.md

> This is a real architecture constraint document from a production project.
> Use this as the quality bar when generating CONTRACT.md.
> Note the pattern: summary table first, then detailed sections per rule.

---

# gctx — Architecture Constraints

> These rules cannot be violated during development.
> Any task that violates these rules is considered incomplete.

## Rules

| Rule ID | Description | Verification | Affected Tracks |
|---------|------------|--------------|-----------------|
| LEDGER-01 | Ledger is the single source of truth; all views must be rebuildable from ledger | `rm -rf .gctx/branches && gctx rebuild --all` | All |
| LEDGER-02 | events.jsonl is append-only — no modify, no delete | Code review: no truncate/rewrite logic | All |
| HASH-01 | Every event must have sha256 hash chain (parent_hash → hash) | Hash chain verification script | All |
| HASH-02 | Hash uses Canonical JSON (object keys sorted lexicographically) | Rust/TS produce same hash | A, H |
| VIEW-01 | Derived views are deterministic | Same ledger → rebuild twice → diff = 0 | B, C |
| DRAFT-01 | Drafts don't enter events.jsonl (don't pollute raw trace) | `grep "draft" events.jsonl` = 0 results | E |
| LOCK-01 | Any write to ledger must acquire exclusive `.gctx/LOCK` | Code review of all write paths | All |
| RUST-01 | `cargo clippy -- -D warnings` zero warnings | CI check | All |
| EVIDENCE-01 | Commit without evidence must auto-add `label=claim` | Code review of commit event builder | B |

---

## Detailed Rules

### LEDGER-01: Ledger is the Single Source of Truth

**Description**: `.gctx/ledger/events.jsonl` is the only source of truth in the entire system. All branches/*/commit.md, log.md, main.md, metadata.yaml are derived views.

**Rationale**: Guarantees replayability — at any time, the complete system state can be rebuilt from the ledger.

**Verification**:
```bash
gctx rebuild --all
# Views before and after should be identical
```

**Consequence of violation**: Views and ledger become inconsistent; system state is untrustworthy.

---

### HASH-01: sha256 Hash Chain

**Description**: Every event must contain `hash` and `parent_hash` fields. `parent_hash` points to the previous event's `hash` (first event is null). `hash` = sha256(canonical_json(event_without_hash)).

**Rationale**: Tamper-evident — if any intermediate event is modified, all subsequent hashes break.

**Verification**:
```bash
# Verify hash chain line by line
# Each event.parent_hash == previous event.hash
```

**Consequence of violation**: Hash chain cannot be verified; ledger integrity is untrustworthy.

---

### VIEW-01: Derived Views are Deterministic

**Description**: The same events.jsonl must always produce identical commit.md, log.md, main.md, metadata.yaml. No randomness, timestamps, or non-deterministic sorting in derive logic.

**Rationale**: Rebuildability — `gctx rebuild` results must be predictable.

**Verification**:
```bash
gctx rebuild --all
cp -r .gctx/branches /tmp/b1
gctx rebuild --all
diff -r .gctx/branches /tmp/b1
# Expected: no differences
```

**Consequence of violation**: Different rebuild times produce different results; views are untrustworthy.

---

> **Pattern notes for generation:**
> - Start with a summary table (Rule ID, Description, Verification, Affected Tracks)
> - Then detailed sections for each rule with exactly 4 parts:
>   1. **Description** — what the rule says
>   2. **Rationale** — WHY this rule exists (one sentence)
>   3. **Verification** — concrete bash command(s)
>   4. **Consequence of violation** — what breaks if violated
> - Each rule has a unique ID with a category prefix (LEDGER-, HASH-, VIEW-, LOCK-, etc.)
> - Verification must be a runnable command, not "manual review" (exception: code review for logic checks)
> - Rules are derived FROM the actual project, not generic best practices
