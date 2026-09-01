# Stack Shape — Standard Skeleton

## File Order

Discussion and output should follow this order — don't skip ahead:

```text
1. motif (母題) / overview   ← What problem does this system solve?
2. canonical form            ← What does one cycle / one flow look like?
3. primitives / schemas      ← What types are first-class citizens?
4. rules / judgment          ← Decision logic / invariants
5. APIs / interfaces         ← How does it connect externally?
6. canonical slice           ← Minimum concrete instance
7. demo path                 ← Run through once to prove closure
8. promotion check           ← Is it stable? Can it enter project-plan?
```

This order prevents the most common failure: jumping to API design before the canonical form is stable.

---

## Internal Structure of Each Spec File

```markdown
# <topic>.md

> Status: `working draft` | `canonical`
>
> Purpose: <one sentence — what question does this file answer>
>
> Shared types: see `./shared-types.md` (if applicable)

---

## 1. One-Liner
> **<core definition, one sentence>**

---

## 2. What It's NOT / Most Common Mistakes
<2-4 concrete failure modes, with examples>

---

## 3. Core Definition
<first-class concepts, each with a clear definition>

---

## 4. Canonical Form / Flow
<the system's repeating unit, ASCII diagram or step list>

---

## 5. Schema / Types
<TypeScript types, with field annotations>

---

## 6. Position in the Overall System
<connections to other specs>

---

## 7. Canonical Examples
<2-3 concrete walkthroughs>

---

## 8. Boundaries / What's Out of Scope
<explicit scope boundaries>

---

## Closing Line
> **<quotable convergence sentence — read just this to get the core>**
```

---

## Naming Convention

```text
<topic>.md                    — main spec
<topic>-v0.md                 — first version, explicitly not final
<topic>-<variant>-v0.md       — regime/variant deep-dive
shared-types.md               — cross-file type single source of truth
```

---

## shared-types.md Structure

When a type exists as a **cross-file contract in 2+ files**, consider extracting it to shared-types.md.
The decision is not based on file count, but on **concept centrality**.

```markdown
# shared-types.md

> Status: `canonical`
> Rule: When other files reference types, reference this file — don't redefine.

## 1. Base Types
<enums, unions, primitives>

## 2. <Layer 1>
<types for this layer>

## 3. <Layer 2>
...

## N. Cross-Layer Shared
<cross-layer types, with positioning notes>
```

---

## Quality Self-Check

Must verify before output:

| Check | How |
|-------|-----|
| Each spec answers only one core question | Title is a question, not a topic dump |
| Each spec says "what it's NOT" | Section 2 has concrete failure modes |
| Each type is defined in only one place | Inline (if local) or shared-types (if shared) |
| Field names don't conflict across files | Same concept = same name |
| Canonical examples are concrete | Has JSON / TypeScript, not "imagine X" |
| Boundaries are explicit | Each spec states in-scope and out-of-scope |
| Adjacent specs cross-reference each other | "See X.md" links, no dangling references |
| v0 doesn't pretend to be final | Status says `working draft` |

---

## Review Output Format

```markdown
## Spec Stack Review: <name>

### Per-File Health
| File | Internal | Types Aligned | Examples Valid | Boundaries Clear |
|------|----------|---------------|----------------|-----------------|

### Cross-File Issues
| # | Type | Files | Detail |
|---|------|-------|--------|

### Verdict
**<HEALTHY / NEEDS FIXES / MAJOR GAPS>** — <one sentence>
```
