# Review Checklists

Use this file when applying the `spec-review-modes` skill.

---

## Mode A — Quick Overview Checklist

Ask:
- What is this spec stack trying to build?
- Which files appear canonical?
- What are the main bounded contexts?
- What feels clean immediately?
- What feels ambiguous immediately?
- What would most likely break first during implementation?

Suggested output:

```markdown
## Quick read
[what the stack is doing]

## Strong parts
- ...
- ...

## Biggest risks
- ...
- ...

## What I would read/fix next
- ...
```

---

## Mode B — Strict Spec Audit Checklist

### Canonical source questions
- Is there one source of truth for shared types?
- Is there one source of truth for lifecycle/state transitions?
- Is there one source of truth for API ownership?

### Type questions
- Are the same nouns used consistently across files?
- Are any modules consuming storage-shaped types where they should consume view models?
- Do examples, schemas, and state tables agree?

### Ownership questions
- Which context owns creation?
- Which context owns promotion/activation?
- Which context owns rejection?
- Which context owns conflict detection?
- Which context owns retrieval and pack generation?

### Separation questions
- Is read-side coupled to write/storage shape?
- Is governance leaking into intake?
- Is injection implicitly redefining lifecycle?

### Output template

```markdown
## Overall judgment
[1 paragraph]

## Must fix
1. ...
2. ...

## Should fix
1. ...
2. ...

## Future concerns
- ...
- ...

## Ownership summary
- Intake owns ...
- Injection owns ...
- Governance owns ...
```

---

## Mode C — Product / Architecture Lens Checklist

Ask:
- What user pain becomes meaningfully easier if this spec works?
- What part of the stack would users actually feel?
- Which parts create leverage in real workflows?
- Which parts are completeness theater?
- Is this creating first-order value or only second-order optimization?
- What should stay hidden substrate versus become visible surface?

### Look for product thickness
Signals of thickness:
- changes workflow behavior
- creates meaningful pressure at the right moment
- reduces repeated labor
- improves judgment or execution quality in a felt way

### Look for thinness
Signals of thinness:
- architecture neatness without user pull
- lifecycle complexity with no workflow effect
- abstractions that only future internal developers care about
- naming sophistication without leverage

### Output template

```markdown
## What feels real
- ...
- ...

## What feels thin
- ...
- ...

## Actual wedge
[what the real entry value is]

## What should stay substrate
- ...

## Conclusion
[build / narrow / hide / refactor]
```

---

## Escalation rule

When in doubt:
- If the confusion is about structure → go more toward Mode B.
- If the confusion is about usefulness → go more toward Mode C.
- If the user just wants orientation → stay in Mode A.
