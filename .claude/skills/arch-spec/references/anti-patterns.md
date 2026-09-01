# Anti-Patterns — Mistakes to Avoid When Building a Spec Stack

---

## Structural Mistakes

### 1. Spec Has No "What It's NOT"
If you can't say what it's not, you don't know what it is.
Every spec must have Section 2 listing 2-4 concrete failure modes.

### 2. Same Type Defined in 3 Places
shared-types.md exists precisely for this.
Once a type appears in 2+ files, move it to shared-types.

### 3. Prose Description Instead of Schema
"A proposal has a target and a kind" is not a spec.
Write TypeScript types with fields and annotations.

### 4. God Spec
One file trying to define everything. Split by core question into multiple files.

### 5. Numeric Scores in v0
`personFit: 72` is false precision. Use `"low" | "medium" | "high"`.

### 6. No Canonical Examples
A spec without examples is a wish, not a specification. At least 2 concrete walkthroughs.

---

## Process Mistakes

### 7. Entering project-plan Too Early
Force-decomposing into tracks/tasks when concepts aren't stable yet.
Result: tracks look reasonable, but the underlying concepts fall apart under pressure.

### 8. Infinite Renaming
Names are already good enough but still chasing something better.
Once stable, stop renaming — go nail down interfaces instead.

### 9. Boundary Overflow
Module A doing Module B's job.
Example: router doing path-check's judgment, pre-world logic leaking into live-world runtime.

### 10. Generic Selection Replacing Regime-Specific Logic
If a concept means completely different things in different modes, don't flatten it into one.
Example: `probe-commit` signal interpretation is completely different under economic vs governance regimes.

---

## Lessons Learned from world-design-v0

### 11. Treating Topics as Regimes
"Video generation" is not a regime. "I want to make money" is.
A regime is a classification of end-goal intent, not subject matter.

### 12. Treating Dashboards as Worlds
Something without state / change / judgment is not a world.
Having a UI does not equal having governance.

### 13. Treating Brainstorms as Selection
Listing 20 ideas is not making a decision.
Decisions require kill filters → probe → signal → commit.

### 14. Mixing Execution/Build with Intent Regime
"How fixed is the path" and "what do you want to change" are two different dimensions.
The former belongs to path-check, the latter to intent-router. Don't mix them into the same layer.

### 15. Putting Edda at the Last Box of the Pipeline
It's not the tail — it's the decision spine, running alongside from the first step to the last.
Drawing it as the last box creates the misconception that it only records final results.
