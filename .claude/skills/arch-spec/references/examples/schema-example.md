# Example: Schema Spec

> Source: `thyra/docs/world-design-v0/change-proposal-schema-v0.md`
> Type: Schema — structural definition of a first-class citizen

## What makes this good

### 1. A metaphor makes the schema's role instantly clear
> If `world` is Thyra's ontology, then `change proposal` is Thyra's verb.

### 2. Explicitly says "what it's NOT"
- A change proposal is not a task
- A change proposal is not a prompt
- A change proposal is always world-scoped
- A proposal must be traceable against judgment / outcome

### 3. Canonical lifecycle with ASCII state machine
```text
draft → proposed → judged → approved | rejected | simulation_required | escalated
→ applied | cancelled → outcome_window_open → outcome_closed → archived
```

### 4. Schema divided into 7 layers, each with clear responsibility
1. Identity — who proposed it, what type
2. Target — what it touches
3. Intent — why the change
4. Diff — what changes
5. Governance — constraints
6. Expected Outcome — anticipated results
7. Traceability — tracking links

### 5. TypeScript types with inline annotations
```typescript
type ChangeProposal = {
  id: string;           // cp_...
  worldId: string;      // world_midnight_market_001
  cycleId: string;      // cycle_2026_03_18_2000
  status: ChangeProposalStatus;
  kind: ChangeKind;
};
```

### 6. Enums use typed unions, values are self-explanatory
```typescript
type ChangeKind =
  | "adjust_stall_capacity"
  | "throttle_entry"
  | "pause_event";
```
No codes ("S1", "S2"), no generic `string`.

---

> **Pattern notes:**
> - A schema spec must open by explaining WHY this schema is a first-class citizen
> - Canonical lifecycle uses ASCII state machine
> - TypeScript types, not JSON Schema
> - Complex types split into named layers
> - Enums are always typed unions with self-explanatory values
> - Schema should be followed by concrete JSON examples
