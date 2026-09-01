# Promotion Handoff — Human-Readable Package for arch-spec → project-plan

> This file defines the **human-readable promotion package** for `arch-spec -> project-plan`.
> It is a rendering/profile of the canonical engineering schema, not an alternative schema.
>
> **Canonical schema**: `docs/storage/promotion-handoff-schema-v0.md`
> **Field mapping**: Promotion Memo ≈ `summary + whyNow + knownGaps` in the schema.
> Required/Optional items ≈ `requiredSpecs + stableObjects + sourceLinks` in the schema.

## Core Question

> **When promoting to project-plan, what human-readable package should be assembled?**

`promotion-rules.md` tells you "when you can promote."
This file tells you "what to hand off, in a format humans and agents can read."

---

## Promotion Package

When a spec stack (or a subsystem within it) passes the promotion checklist,
assemble the following handoff package for `project-plan`:

### Required

| Item | Source | How project-plan Uses It |
|------|--------|--------------------------|
| **Overview** | overview.md | Becomes the Goal section of `00_OVERVIEW.md` |
| **Canonical Form** | canonical-form.md | Identifies track decomposition boundaries |
| **Shared Types** | shared-types.md | Becomes the type rules in `CONTRACT.md` |
| **Rules / Invariants** | rules-v0.md | Becomes the architecture constraints in `CONTRACT.md` |
| **Canonical Slice** | canonical-slice.md | Becomes the golden path scenario in `VALIDATION.md` |
| **Demo Path** | demo-path.md | Becomes the end-to-end scenario in `VALIDATION.md` |

### Optional (if available)

| Item | Source | How project-plan Uses It |
|------|--------|--------------------------|
| API spec | api.md | Drives task definitions in the routes track |
| Regime docs | *-regime-v0.md | May each become an independent track |
| Handoff contract | handoff-contract.md | Becomes the spec for the integration track |
| Test cases | test-cases.md | Becomes input for validation tests |

---

## Promotion Memo

When promoting, produce a short memo to include in the package:

```markdown
## Promotion Memo: <system name>

**From**: arch-spec stack at `docs/<name>/`
**To**: project-plan

### What's stable
<list stabilized first-class terms, canonical form, core types>

### What's intentionally not specced
<list deliberately unspecified parts — project-plan should not assume these are defined>

### Recommended track cuts
<suggested track decomposition, based on the spec stack's module boundaries>

### Open risks
<known but unresolved risks at time of promotion>
```

---

## Transition Mapping

```text
arch-spec                      →    project-plan
─────────────────────────────────────────────────
overview.md                    →    00_OVERVIEW.md (Goal + DAG)
shared-types.md + rules-v0.md  →    CONTRACT.md
canonical-form module boundaries →  TRACKS.md (track decomposition)
canonical-slice + demo-path    →    VALIDATION.md (golden paths)
each module                    →    TRACK_X/ (task files)
```

---

## Partial Promotion

Not all specs need to be promoted at the same time.

Example:
- Thyra's canonical-cycle + change-proposal + judgment → can enter project-plan first
- Volva's intent-router + path-check → not yet stable, stays in arch-spec

When promoting a subsystem, explicitly state in the promotion memo "what's still in arch-spec."

---

## After project-plan Receives the Package

project-plan should:
1. Read the promotion memo
2. Read overview + canonical form → design the DAG
3. Read shared-types + rules → write CONTRACT.md
4. Read canonical slice + demo path → write VALIDATION.md
5. Decompose along module boundaries into tracks → write TRACKS.md + task files

project-plan should NOT:
- Redefine concepts that arch-spec has already finalized
- Assume that intentionally unspecced parts are defined
- Rename types in shared-types (if renaming is needed, go back to arch-spec first)
