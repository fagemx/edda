# Gotchas — Common Failures When Agents Execute arch-spec

> This is not about "design mistakes" (that's in anti-patterns.md).
> This is about "operational mistakes agents actually make when running this skill."
> Updated continuously as usage experience grows.

---

## During Generation

### G1: Defaulting to Full Stack
Agent sees `arch-spec` and produces 8-12 files.
Most of the time minimal (3 files) or standard (5 files) is enough.
**Read `minimum-stack.md` first to determine stack size.**

### G2: Turning Overview into a God Spec
Stuffing all concepts, all schemas, all APIs into the overview.
The overview does one thing only: define the motif (母題) + draw the global map. Details go into individual specs.

### G3: Skipping "What It's NOT"
Agents tend to jump straight to "what it is" and then into the schema.
But Section 2 "what it's NOT" is the key to preventing scope creep.
**Every spec must have Section 2. Without it = not finished.**

### G4: Describing Schema in Prose
"A proposal has a target and a kind" is not a spec.
Write TypeScript types with field names, types, and annotations.

### G5: Defining Types Without Reading shared-types First
Result: the same concept uses different field names in different files.
**Before writing any new spec, read shared-types.md first.**

### G6: Canonical Examples Too Hand-Wavy
"For example a market might have stalls" is not an example.
Use concrete JSON: zone_a, stallCapacity: 8, spotlightWeight: 0.6.

### G7: ASCII Diagrams Omitted
Listing steps in text without drawing a diagram.
Canonical form / state machine / pipeline must have ASCII diagrams.

---

## During Review

### G8: Surface-Level Lint Only
Only checking "does the file exist" and "does it have frontmatter."
Must do cross-file type comparison, field name consistency checks, boundary checks.
**Use the full checklist from `review-checklist.md`.**

### G9: Not Catching Type Drift
Same concept called `missingFields` in A.md and `keyUnknowns` in B.md.
This is the most common cross-file bug.

### G10: Not Checking Promotion Readiness
Finishing the review with just "looks OK" without assessing whether it can enter project-plan.
**The last step of every review is always a promotion status check.**

---

## During Shared-Types Work

### G11: Using "3+ Files" as a Hard Threshold
The decision isn't based on file count — it's based on concept centrality.
If a type is a core contract in 2 files, it should be extracted.

### G12: Extracting shared-types but Not Updating References
shared-types was created, but the original spec files still have the old inline definitions.
**After extraction, update all files to add `// canonical definition: see ./shared-types.md section X.Y`.**

---

## During Promotion

### G13: Saying "Ready to Promote" Without Producing a Promotion Memo
Promotion isn't just a judgment call — it also requires outputting a handoff package.
**Use the format from `promotion-handoff.md`.**

### G14: Assuming All Specs Must Be Promoted Simultaneously
Partial promotion is OK. Once a subsystem is stable, promote it. Others can continue speccing.
