# Demo Path — Prove the Decision Model closes the loop

> Status: `working draft`
>
> Purpose: Walk through one complete decision lifecycle end-to-end, proving that create → query → supersede → freeze all work through the mutation contract.
>
> Shared types: see `./shared-types.md`

---

## 1. One-Liner

> **One decision, six state transitions, zero direct SQL — if this walkthrough works, the model works.**

---

## 2. What It's NOT

### NOT a test plan

This is a conceptual walkthrough to prove closure, not a list of test cases. Tests come during implementation.

### NOT a UI demo

We're testing the mutation contract, not how things look in the CLI. All operations are function calls.

### NOT an integration test with other specs

This demo uses only Decision Model operations. Intake/Injection/Governance are simulated as "callers" but their internal logic is not tested here.

---

## 3. Setup

```text
Project: edda (this repo)
Branch: main
Schema: v10 (migration applied)
Existing decisions: none (clean slate)
```

---

## 4. Demo Walkthrough

### Phase 1: Agent extracts a candidate (Intake → Model)

**Action:** Intake calls `create_candidate()` with agent authority

```typescript
const r1 = create_candidate({
  key: "db.engine",
  value: "sqlite",
  reason: "extracted from transcript: 'we chose SQLite because embedded'",
  branch: "main",
  authority: "agent_proposed",
  affected_paths: ["crates/edda-ledger/**"],
  tags: ["architecture", "storage"],
  reversibility: "low",
});
```

**Expected state:**

```json
{
  "event_id": "evt_A",
  "key": "db.engine",
  "value": "sqlite",
  "status": "proposed",
  "is_active": false,
  "authority": "agent_proposed"
}
```

**Verify:**
- `find_active_by_key("main", "db.engine")` → `null` (proposed is not active)
- Event written to log with `tags: ["decision"]`

---

### Phase 2: Human approves from inbox (Governance → Model)

**Action:** Governance calls `promote()`

```typescript
const r2 = promote("evt_A");
```

**Expected state:**

```json
{
  "event_id": "evt_A",
  "status": "active",
  "is_active": true,
  "authority": "agent_approved"
}
```

**Verify:**
- `find_active_by_key("main", "db.engine")` → returns `evt_A`
- `is_active` flipped to `true`
- `authority` changed from `agent_proposed` to `agent_approved`
- Promotion event written

---

### Phase 3: Query works (Injection → Model read via DecisionView)

**Action:** Injection calls `to_view()` — never touches `DecisionRow` directly

```typescript
const row = find_active_by_key("main", "db.engine");
const view = to_view(row!);
// → { key: "db.engine", value: "sqlite", status: "active",
//    affected_paths: ["crates/edda-ledger/**"],  // parsed array, not JSON string
//    tags: ["architecture", "storage"],
//    propagation: "local" }                      // not "scope"

const conflicts = find_conflicts("db.engine", "postgres");
// → [{ existing_event_id: "evt_A", existing_value: "sqlite",
//      conflict_type: "value_divergence", ... }]
```

**Verify:** Active decision is findable by key, `to_view()` produces clean delivery model, and conflict detection returns structural judgment.

---

### Phase 4: Human re-decides (CLI → Intake create → Governance promote+supersede)

**Action:** Human runs `edda decide "db.engine=postgres" --reason "need multi-user"`

Step 4a — Intake creates candidate (existing active found, so status = `proposed`):
```typescript
const r3 = create_candidate({
  key: "db.engine",
  value: "postgres",
  reason: "need multi-user access",
  branch: "main",
  authority: "human",
  affected_paths: ["crates/edda-ledger/**"],
  tags: ["architecture", "storage"],
  reversibility: "low",
});
// Status: "proposed" because existing active decision for db.engine exists
```

Step 4b — Governance promotes, which triggers supersede:
```typescript
const r4 = promote("evt_B");
// Governance detects existing active evt_A for same key → supersede(evt_A, evt_B)
```

**Expected state after promote+supersede:**

Decision evt_A (old):
```json
{ "event_id": "evt_A", "value": "sqlite", "status": "superseded", "is_active": false }
```

Decision evt_B (new):
```json
{
  "event_id": "evt_B",
  "value": "postgres",
  "status": "active",
  "is_active": true,
  "supersedes_id": "evt_A"
}
```

**Verify:**
- `find_active_by_key("main", "db.engine")` → returns `evt_B`
- `get_decision("evt_A")` → `status: "superseded"`
- Supersede provenance link: `evt_B.refs.provenance` contains `{ target: "evt_A", rel: "supersedes" }`
- Three events written: candidate creation + supersede + promotion

---

### Phase 5: Governance freezes a decision

**Action:** Release approaching, governance freezes the DB decision

```typescript
const r4 = transition("evt_B", "frozen", {
  reason: "release freeze — no infra changes until v2.0 ships",
});
```

**Expected state:**

```json
{ "event_id": "evt_B", "status": "frozen", "is_active": false }
```

**Verify:**
- `find_active_by_key("main", "db.engine")` → `null` (frozen ≠ active)
- `get_decision("evt_B")` → `status: "frozen"`
- Freeze event written with reason

---

### Phase 6: After release, governance unfreezes

**Action:**

```typescript
const r5 = transition("evt_B", "active", { reason: "v2.0 shipped, unfreeze" });
// This is the unfreeze() transition: frozen → active
```

**Expected state:**

```json
{ "event_id": "evt_B", "status": "active", "is_active": true }
```

**Verify:**
- `find_active_by_key("main", "db.engine")` → returns `evt_B` again
- Unfreeze event written

---

### Phase 7: Illegal transition rejected

**Action:** Try to move superseded decision back to active

```typescript
const r6 = transition("evt_A", "active");
```

**Expected result:**

```json
{ "ok": false, "error": "illegal transition: superseded → active" }
```

**Verify:** No event written, no state change.

---

## 5. Closure Proof

| Capability | Demonstrated in Phase | Owner |
|-----------|----------------------|-------|
| Create candidate with `proposed` status | Phase 1 | Intake |
| Promote to `active` | Phase 2 | **Governance** |
| Read via `DecisionView` (not `DecisionRow`) | Phase 3 | Injection |
| Conflict detection (judgment, not explanation) | Phase 3 | **Governance** |
| Supersede on promote (Governance-triggered) | Phase 4 | **Governance** |
| Provenance link on supersede | Phase 4 | Model |
| `is_active` stays in sync with `status` | Phase 2, 4, 5, 6 | Model |
| Freeze | Phase 5 | **Governance** |
| Unfreeze | Phase 6 | **Governance** |
| Illegal transition blocked | Phase 7 | Model |
| Every mutation writes event | All phases | Model |

---

## 6. What This Demo Does NOT Cover

| Gap | Covered By |
|-----|-----------|
| How candidates get extracted from transcripts | Intake spec |
| How decisions surface in session start | Injection spec |
| How drift/conflict triggers warnings | Governance spec |
| `experimental` / `trial()` flow | Future iteration |
| Cross-project propagation (shared/global) | Existing edda-ledger sync |
| Dependency graph (`depends_on`) | Existing edda-ledger deps |

---

## Closing Line

> **7 phases, 5 state transitions, 1 rejection — Intake creates, Governance transitions, Injection reads views. Nobody crossed the line.**
