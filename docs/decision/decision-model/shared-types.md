# shared-types.md — Cross-Spec Type Contracts

> Status: `canonical`
>
> Rule: When other specs or other arch-spec stacks (Intake, Injection, Governance) reference these types, reference THIS file — don't redefine.

---

## 1. Base Enums

### 1.1 DecisionStatus

The lifecycle state of a decision. See `canonical-form.md` for the state machine.

```typescript
type DecisionStatus =
  | "proposed"       // candidate in inbox, awaiting review
  | "active"         // in force, governs behavior
  | "experimental"   // trial run, visible but not authoritative
  | "frozen"         // intentionally paused, not superseded
  | "superseded"     // replaced by a newer decision
  | "rejected";      // reviewed and explicitly declined
```

**`is_active` mapping:**
```typescript
const IS_ACTIVE_STATUSES: DecisionStatus[] = ["active", "experimental"];
// is_active = status IN IS_ACTIVE_STATUSES
```

### 1.2 DecisionAuthority

Who made this decision and with what weight.

```typescript
type DecisionAuthority =
  | "human"             // explicitly typed by a human via CLI/UI
  | "agent_approved"    // proposed by agent, promoted by human
  | "agent_proposed"    // proposed by agent, not yet reviewed
  | "system";           // auto-generated (import, sync, migration)
```

### 1.3 PropagationScope

How far a decision travels across projects. **Not** the same as affected paths.

```typescript
type PropagationScope =
  | "local"    // this project only (default)
  | "shared"   // projects in the same group
  | "global";  // all registered projects
```

**Note:** In SQLite, this maps to the existing `scope` column. In the API payload, it's called `propagation` to disambiguate from `affected_paths`.

### 1.4 Reversibility

How difficult it is to undo this decision.

```typescript
type Reversibility =
  | "low"      // hard to undo — database choice, auth strategy
  | "medium"   // moderate effort — error handling pattern, module structure
  | "high";    // easy to undo — log format, naming convention
```

---

## 2. Core Object Types

### 2.1 DecisionPayload

The structured payload stored inside a decision event. This is what gets written to the event log.

```typescript
type DecisionPayload = {
  key: string;                          // "db.engine" — dotted domain.aspect
  value: string;                        // "sqlite"
  reason?: string;                      // why this choice
  propagation?: PropagationScope;       // cross-project reach
  authority?: DecisionAuthority;        // who decided
  affected_paths?: string[];            // glob patterns: ["crates/edda-ledger/**"]
  tags?: string[];                      // ["architecture", "storage"]
  review_after?: string | null;         // ISO 8601 date
  reversibility?: Reversibility;        // how hard to undo
};
```

### 2.2 DecisionRow

The materialized view in the `decisions` table. Superset of `DecisionPayload` with computed/derived fields.

```typescript
type DecisionRow = {
  // Identity
  event_id: string;                     // ULID: "evt_01JE7X..."
  key: string;                          // "db.engine"
  domain: string;                       // auto-extracted: "db"
  branch: string;                       // "main"

  // Content
  value: string;                        // "sqlite"
  reason: string;                       // "" if not provided

  // Lifecycle
  status: DecisionStatus;               // "active"
  is_active: boolean;                   // derived from status
  supersedes_id: string | null;         // "evt_..." or null

  // Metadata
  authority: DecisionAuthority;         // "human"
  affected_paths: string;               // JSON: '["crates/edda-ledger/**"]'
  tags: string;                         // JSON: '["architecture"]'
  review_after: string | null;          // ISO 8601 or null
  reversibility: Reversibility;         // "low"

  // Propagation
  scope: string;                        // "local" (SQLite column name)
  source_project_id: string | null;     // for imports
  source_event_id: string | null;       // for imports

  // Timestamps
  ts: string | null;                    // event timestamp
};
```

**Note:** `affected_paths` and `tags` are stored as JSON strings in SQLite. Callers parse them as arrays.

### 2.3 DecisionView

The read-side projection consumed by Injection. **Injection must never depend on `DecisionRow` directly** — `DecisionRow` is a storage noun; `DecisionView` is a delivery noun.

```typescript
type DecisionView = {
  // What
  key: string;                          // "db.engine"
  value: string;                        // "sqlite"
  reason: string;                       // why this choice
  domain: string;                       // "db"

  // Governance state (read-only for Injection)
  status: DecisionStatus;               // "active" | "experimental"
  authority: DecisionAuthority;         // who decided
  reversibility: Reversibility;         // how hard to undo

  // Scope
  affected_paths: string[];             // parsed, not JSON string
  tags: string[];                       // parsed, not JSON string
  propagation: PropagationScope;        // "local" | "shared" | "global"

  // Identity (for linking, not for storage)
  event_id: string;
  branch: string;
  ts: string | null;

  // Graph (optional, populated on demand)
  supersedes_id?: string;
  depends_on?: string[];
};
```

**Key differences from `DecisionRow`:**
- `affected_paths` and `tags` are parsed arrays, not JSON strings
- `scope` renamed to `propagation` (API language, not storage language)
- No `is_active` — Injection filters by `status` directly
- No `source_project_id` / `source_event_id` — storage concerns, not delivery concerns
- Graph fields optional — only populated when Injection needs them

**Conversion:** Decision Model provides a `to_view(row: DecisionRow) → DecisionView` function. Injection never parses `DecisionRow` itself.

---

## 3. Operation Types

### 3.1 MutationResult

Returned by all mutation contract operations.

```typescript
type MutationResult = {
  ok: boolean;
  event_id?: string;                    // the audit event written
  decision_id?: string;                 // the decision affected
  superseded_id?: string;              // if auto-supersede occurred
  conflicts?: ConflictInfo[];          // non-blocking conflicts detected during mutation
  error?: string;                       // if ok == false
};
```

**`conflicts` field:** Some mutations (e.g., `gov_unfreeze`) may succeed but detect conflicts that the caller should be aware of. When present, `ok` is still `true` — the mutation completed — but the caller should surface these conflicts to the user. If no conflicts are detected, the field is omitted.

### 3.2 ConflictInfo

Returned by `find_conflicts()` — a **judgment result**, not an explanation. Contains only the structural facts of the conflict. Human-readable explanations ("why this matters", "what you should do") belong to the Injection or UI layer, not here.

```typescript
type ConflictInfo = {
  existing_event_id: string;
  existing_value: string;
  existing_branch: string;
  existing_authority: DecisionAuthority;
  existing_ts: string | null;
  conflict_type: ConflictType;         // structural classification
};

type ConflictType =
  | "value_divergence"    // same key, different value
  | "scope_overlap"       // affected_paths overlap with different conclusion
  | "cross_branch";       // same key decided differently on another branch
```

> **Naming collision warning:** `edda_ledger::sync` module has its own `ConflictInfo` type used for cross-project sync conflict resolution. That is a **different type** from the `ConflictInfo` defined here (§3.2). During implementation, the sync module's type should be renamed to `SyncConflict` to avoid ambiguity. Until then, always qualify: `shared_types::ConflictInfo` vs `sync::ConflictInfo`.

### 3.3 TransitionError

Possible errors from `transition()`.

```typescript
type TransitionError =
  | "decision_not_found"
  | "illegal_transition"                // e.g., superseded → active
  | "missing_superseded_by"             // supersede without specifying new decision
  | "precondition_failed";              // status doesn't match expected
```

---

## 4. Cross-Spec Contracts

These types will be referenced by the other three arch-spec stacks. Each type is defined here once; other specs reference as:

```typescript
// In Intake spec:
// See decision-model/shared-types.md §1.1 for DecisionStatus
// See decision-model/shared-types.md §3.1 for MutationResult
```

### 4.1 Types consumed by Intake spec

Intake creates candidates. It does NOT own lifecycle transitions.

| Type | Section | Usage |
|------|---------|-------|
| `DecisionAuthority` | §1.2 | Set `agent_proposed` on extraction |
| `DecisionPayload` | §2.1 | Build payload for `create_candidate()` |
| `MutationResult` | §3.1 | Handle result of `create_candidate()`, `edit_candidate()` |

### 4.2 Types consumed by Injection spec

Injection reads delivery models. It does NOT read storage rows.

| Type | Section | Usage |
|------|---------|-------|
| `DecisionView` | §2.3 | Query results for pack building — **never `DecisionRow`** |
| `DecisionStatus` | §1.1 | Filter: only `active` and `experimental` |
| `PropagationScope` | §1.3 | Filter cross-project decisions |

### 4.3 Types consumed by Governance spec

Governance owns ALL lifecycle transitions. It is the sole state machine owner.

| Type | Section | Usage |
|------|---------|-------|
| `DecisionStatus` | §1.1 | Transition targets |
| `ConflictInfo` | §3.2 | Conflict detection judgment (not explanation) |
| `ConflictType` | §3.2 | Conflict classification |
| `MutationResult` | §3.1 | Handle transition results |
| `TransitionError` | §3.3 | Error handling |

---

## 5. Version History

| Version | Date | Changes |
|---------|------|---------|
| v0 | 2026-03-19 | Initial: 4 enums, 2 object types, 3 operation types |
| v0.1 | 2026-03-19 | Add DecisionView (read model), ConflictType enum, boundary ownership rules |

---

## Closing Line

> **5 enums, 3 object types, 3 operation types — defined once here, referenced everywhere else. If you're redefining these in another file, you're creating drift.**
