# Promotion Rules — When to Upgrade from arch-spec to project-plan

## Core Principle

> **arch-spec asks "what even is this thing." project-plan asks "how to decompose it into doable work."**
>
> If you're still answering the first question, don't enter project-plan.
> If the first question is already answered, don't stay in arch-spec.

---

## Promotion Checklist

ALL must be true:

### 1. Core Terminology Is Stable
- [ ] First-class entities have names, and no name changes in the last 2 discussions
- [ ] Nobody is still debating "should it be called X or Y"

### 2. Canonical Form Exists
- [ ] An ASCII diagram shows the system's core cycle / flow / pipeline
- [ ] Someone can point to one document and say "this is what one unit looks like"

### 3. Schema Types Are Concrete
- [ ] TypeScript types exist (not prose descriptions)
- [ ] Field names are finalized
- [ ] shared-types.md exists and covers all cross-file types

### 4. Boundaries Are Explicit
- [ ] Each spec states what it does NOT do
- [ ] Handoff contracts with adjacent systems exist
- [ ] No single spec is answering two core questions

### 5. Canonical Slice Exists
- [ ] At least one concrete instance has been fully specced
- [ ] The slice has real data (zones, gates, chiefs, metrics — not placeholders)

### 6. At Least One Closure Can Be Demonstrated
- [ ] A demo path exists, showing the system's minimum closed loop
- [ ] The closed loop touches all first-class artifacts

---

## Red Flags — Not Ready for project-plan

| Signal | Meaning |
|--------|---------|
| Names keep changing | Concepts not stable |
| "We might still need X" | Scope still expanding |
| No canonical slice | Concepts still abstract |
| shared-types.md has conflicts | Types haven't converged |
| Can't write a demo path | System can't close the loop yet |
| Spec references non-existent things | Stack has gaps |

---

## Transitions During Upgrade

```text
arch-spec stack (concept definition)
  ↓
project-plan (execution decomposition)
  ├── 00_OVERVIEW.md     ← derived from spec stack's overview
  ├── CONTRACT.md        ← derived from spec stack's rules/invariants
  ├── TRACKS.md          ← decompose spec stack's modules into tracks
  ├── VALIDATION.md      ← derived from spec stack's canonical slice + demo path
  └── TRACK_X/tasks      ← each task implements a part of the spec
```

The spec stack becomes the **input** to project-plan, not replaced by it.

---

## Partial Promotion Is OK

Not all specs need to be ready at the same time.

Example:
- `canonical-cycle.md` + `change-proposal-schema-v0.md` + `judgment-rules-v0.md` → can enter project-plan
- `intent-router.md` + `path-check.md` → still evolving, stay in arch-spec

You can promote one subsystem while another continues speccing.

---

## Strongest Signal

> **"Names are stable" is the single strongest signal.**
> If first-class citizen names haven't changed in 2 discussions, you can almost certainly start decomposing work.
