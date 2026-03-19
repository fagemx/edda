# Schema v0 — What does a Decision object look like?

> Status: `working draft`
>
> Purpose: Define the complete Decision object type, including new fields, and the SQLite schema migration from v9 → v10.
>
> Shared types: see `./shared-types.md`

---

## 1. One-Liner

> **If `Event` is Edda's append-only fact, then `Decision` is Edda's governed opinion — a structured object with identity, lifecycle, authority, and scope.**

---

## 2. What It's NOT / Common Mistakes

### NOT a flat key-value pair

`db.engine=sqlite` is the *content* of a decision, not the decision itself. The decision is the content plus status, authority, affected paths, tags, review schedule, and graph edges.

### NOT the event itself

The event is immutable and lives in the `events` table. The decision is a mutable projection that lives in the `decisions` table and gets updated by the mutation contract. One event creates a decision; subsequent events transition its state.

### NOT JSON Schema

We use TypeScript types for spec clarity. The Rust implementation uses `serde` structs. The SQLite schema uses SQL DDL. All three must agree but this spec defines the canonical shape in TypeScript.

### NOT a breaking change

Schema v10 adds columns with defaults. Existing code reading `is_active` continues to work. No data loss, no migration rewrite.

---

## 3. Current Schema (v1 — what exists today)

```typescript
// Current: edda-core/src/types.rs:117-126
type DecisionPayload_V1 = {
  key: string;           // "db.engine"
  value: string;         // "sqlite"
  reason?: string;       // "embedded, zero-config"
  scope?: "local" | "shared" | "global";  // propagation scope
};

// Current: edda-ledger/src/sqlite_store.rs:168-186
type DecisionRow_V1 = {
  event_id: string;
  key: string;
  value: string;
  reason: string;
  domain: string;          // auto-extracted: "db" from "db.engine"
  branch: string;
  supersedes_id?: string;
  is_active: boolean;
  ts?: string;
  scope: string;           // "local" | "shared" | "global"
  source_project_id?: string;
  source_event_id?: string;
};
```

---

## 4. Proposed Schema (v2)

### 4.1 Decision Payload (event-side)

```typescript
// Canonical definition: see ./shared-types.md §2.1
type DecisionPayload = {
  key: string;                      // "db.engine" — dotted domain.aspect
  value: string;                    // "sqlite"
  reason?: string;                  // why this choice
  propagation?: PropagationScope;   // cross-project reach (renamed from "scope")
  authority?: DecisionAuthority;    // who decided
  affected_paths?: string[];        // glob patterns: ["crates/edda-ledger/**"]
  tags?: string[];                  // ["architecture", "storage"]
  review_after?: string;            // ISO 8601: "2026-06-01"
  reversibility?: Reversibility;    // how hard to undo
};
```

**Key rename: `scope` → `propagation`** in the payload to disambiguate from `affected_paths`. The `scope` column in SQLite keeps its name for backward compat but maps to `propagation` in the API.

### 4.2 Decision Row (SQLite-side)

```typescript
// Canonical definition: see ./shared-types.md §2.2
type DecisionRow = {
  // --- existing fields (unchanged) ---
  event_id: string;
  key: string;
  value: string;
  reason: string;
  domain: string;
  branch: string;
  supersedes_id?: string;
  is_active: boolean;               // kept for backward compat, derived from status
  ts?: string;
  scope: string;                    // maps to propagation in API
  source_project_id?: string;
  source_event_id?: string;

  // --- new fields (schema v10) ---
  status: DecisionStatus;           // replaces semantic meaning of is_active
  authority: DecisionAuthority;     // who/what made this decision
  affected_paths: string;           // JSON array of glob patterns
  tags: string;                     // JSON array of strings
  review_after?: string;            // ISO 8601 or NULL
  reversibility: Reversibility;     // "low" | "medium" | "high"
};
```

### 4.3 Enums

```typescript
// Canonical definition: see ./shared-types.md §1

type DecisionStatus =
  | "proposed"       // candidate, in inbox
  | "active"         // in force
  | "experimental"   // trial run
  | "frozen"         // intentionally paused
  | "superseded"     // replaced
  | "rejected";      // declined

type DecisionAuthority =
  | "human"             // explicitly typed by human
  | "agent_approved"    // proposed by agent, approved by human
  | "agent_proposed"    // proposed by agent, not yet approved
  | "system";           // auto-generated (import, sync)

type PropagationScope =
  | "local"    // this project only (default)
  | "shared"   // same group
  | "global";  // all projects

type Reversibility =
  | "low"      // hard to undo (e.g., choosing a database)
  | "medium"   // moderate effort (e.g., error handling pattern)
  | "high";    // easy to undo (e.g., log format)
```

---

## 5. SQLite Migration: V9 → V10

```sql
-- Schema V10: Decision object v2
ALTER TABLE decisions ADD COLUMN status TEXT NOT NULL DEFAULT 'active';
ALTER TABLE decisions ADD COLUMN authority TEXT NOT NULL DEFAULT 'human';
ALTER TABLE decisions ADD COLUMN affected_paths TEXT NOT NULL DEFAULT '[]';
ALTER TABLE decisions ADD COLUMN tags TEXT NOT NULL DEFAULT '[]';
ALTER TABLE decisions ADD COLUMN review_after TEXT;
ALTER TABLE decisions ADD COLUMN reversibility TEXT NOT NULL DEFAULT 'medium';

-- Backfill status from is_active + supersedes_id
UPDATE decisions SET status = 'active' WHERE is_active = TRUE;
UPDATE decisions SET status = 'superseded' WHERE is_active = FALSE AND supersedes_id IS NOT NULL;
UPDATE decisions SET status = 'superseded' WHERE is_active = FALSE AND supersedes_id IS NULL;

-- Index for status-based queries
CREATE INDEX IF NOT EXISTS idx_decisions_status ON decisions(status);
CREATE INDEX IF NOT EXISTS idx_decisions_status_domain
    ON decisions(status, domain) WHERE status = 'active';

-- Index for affected_paths queries (JSON)
-- Note: SQLite JSON functions for path matching will be used at query time
-- No specialized index needed yet — affected_paths cardinality is low
```

### Backward Compatibility Contract

```text
Rule: is_active must always agree with status.

Mutation contract enforces:
  status IN ('active', 'experimental') → is_active = TRUE
  status IN ('proposed', 'rejected', 'frozen', 'superseded') → is_active = FALSE

Existing code that reads `WHERE is_active = TRUE` continues to work correctly.
Existing code that sets `is_active` directly → must migrate to use mutation contract.
```

---

## 5a. Implementation Prerequisites

> **Schema V10 is the foundation. No other spec (Intake, Injection, Governance) can be implemented until these columns exist in SQLite.**

All four spec directories (`decision-model`, `decision-injection`, `decision-governance`, `decision-intake`) assume that `status`, `authority`, `affected_paths`, `tags`, `review_after`, and `reversibility` columns exist. They do not. This section makes the dependency explicit and defines the rollout order.

### Phased Rollout

#### Phase 0: Add columns with defaults (no code changes needed)

Run the V9 → V10 migration SQL from §5 above. This is a pure DDL change:

- All new columns have `NOT NULL DEFAULT` values (except `review_after` which is nullable)
- Existing queries (`WHERE is_active = TRUE`) continue to work unchanged
- No Rust code changes required — the new columns simply exist with defaults
- **This phase unblocks all other specs** because the columns are now queryable

```text
Prerequisite:  nothing (can run on any v9 database)
Artifact:      schema_version = 10 in SQLite
Risk:          zero — additive DDL only, no data loss
Verification:  SELECT status, authority FROM decisions LIMIT 1; -- should return 'active', 'human'
```

#### Phase 1: Mutation contract enforces new fields

After Phase 0, update the Rust mutation contract to:

1. Set `status` on every insert/update (instead of relying on `is_active` alone)
2. Maintain the `is_active ↔ status` invariant (see §5 Backward Compatibility Contract)
3. Accept `authority`, `affected_paths`, `tags`, `review_after`, `reversibility` in `create_candidate()`
4. Validate enum values at write time (`DecisionStatus`, `DecisionAuthority`, `Reversibility`)

```text
Prerequisite:  Phase 0 complete
Artifact:      mutation contract in edda-ledger enforces new fields
Risk:          low — existing callers pass no new fields, defaults apply
Verification:  cargo test -p edda-ledger
```

### Dependency Graph

```text
Phase 0 (DDL)
    │
    ├──▶ decision-model/canonical-form.md  (status transitions need the column)
    ├──▶ decision-injection/*              (query_by_paths needs affected_paths)
    ├──▶ decision-governance/*             (transitions need status + authority)
    │
    └──▶ Phase 1 (mutation contract)
              │
              └──▶ decision-intake/*       (create_candidate needs to set authority)
```

---

## 6. Layered Field Responsibilities

Following the schema-example pattern, fields are grouped by layer:

### Layer 1: Identity
| Field | Type | Source |
|-------|------|--------|
| `event_id` | `string` | ULID, created at write time |
| `key` | `string` | User-provided, dotted notation |
| `domain` | `string` | Auto-extracted from key |
| `branch` | `string` | Git branch at decision time |

### Layer 2: Content
| Field | Type | Source |
|-------|------|--------|
| `value` | `string` | User-provided |
| `reason` | `string` | User-provided or extracted |

### Layer 3: Lifecycle
| Field | Type | Source |
|-------|------|--------|
| `status` | `DecisionStatus` | Mutation contract |
| `is_active` | `boolean` | Derived from status |
| `supersedes_id` | `string?` | Set by `supersede()` |

### Layer 4: Metadata
| Field | Type | Source |
|-------|------|--------|
| `authority` | `DecisionAuthority` | Set at creation |
| `affected_paths` | `string[]` (JSON) | Set at creation or promote |
| `tags` | `string[]` (JSON) | Set at creation or promote |
| `review_after` | `string?` | Set at creation or update |
| `reversibility` | `Reversibility` | Set at creation |

### Layer 5: Propagation
| Field | Type | Source |
|-------|------|--------|
| `scope` | `PropagationScope` | Set at creation |
| `source_project_id` | `string?` | Set by import |
| `source_event_id` | `string?` | Set by import |

### Layer 6: Timestamps
| Field | Type | Source |
|-------|------|--------|
| `ts` | `string?` | Event timestamp |

---

## 7. Canonical Examples

### Example 1: Full decision row after creation

```json
{
  "event_id": "evt_01JMWX8K4V3QNXRR8BZCFH5T00",
  "key": "db.engine",
  "value": "sqlite",
  "reason": "embedded, zero-config for CLI tool",
  "domain": "db",
  "branch": "main",
  "supersedes_id": null,
  "is_active": true,
  "ts": "2025-12-01T10:00:00Z",
  "scope": "local",
  "source_project_id": null,
  "source_event_id": null,
  "status": "active",
  "authority": "human",
  "affected_paths": "[\"crates/edda-ledger/**\"]",
  "tags": "[\"architecture\", \"storage\"]",
  "review_after": "2026-06-01",
  "reversibility": "low"
}
```

### Example 2: Migrated legacy row (no new fields set)

```json
{
  "event_id": "evt_01JE7XOLD...",
  "key": "auth.method",
  "value": "none",
  "reason": "CLI tool, no auth needed",
  "domain": "auth",
  "branch": "main",
  "supersedes_id": null,
  "is_active": true,
  "ts": "2025-10-15T08:00:00Z",
  "scope": "local",
  "source_project_id": null,
  "source_event_id": null,
  "status": "active",
  "authority": "human",
  "affected_paths": "[]",
  "tags": "[]",
  "review_after": null,
  "reversibility": "medium"
}
```

All new fields have sensible defaults — legacy data works without manual backfill.

---

## 8. Boundaries / Out of Scope

### In Scope
- Decision object field definitions (all layers)
- SQLite schema migration V9 → V10
- Backward compatibility rules for `is_active`
- Enum value definitions
- Field responsibility assignment (which layer owns what)

### Out of Scope
- **Query API** — how to search/filter decisions → `api.md`
- **State transitions** — which operations move between states → `canonical-form.md`
- **Rust struct definitions** — implementation detail, derived from this spec
- **Index tuning** — performance optimization, not schema design
- **UI rendering** — how decisions appear in CLI output

---

## Closing Line

> **Schema v2 adds 6 columns with defaults, breaks nothing, and gives the Decision object enough structure to support lifecycle, governance, and scope-aware retrieval.**
