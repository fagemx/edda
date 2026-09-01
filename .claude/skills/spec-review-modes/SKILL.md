---
name: spec-review-modes
description: "Review architecture specs, RFCs, API docs, design docs, decision-system docs, or any multi-file technical spec stack using three explicit reading modes: (A) quick overview, (B) strict spec audit, and (C) product/architecture critique. Use when the user asks to 看 spec, review docs, 掃一遍架構, 幫我抓矛盾, 做 spec 審查, or wants structured feedback on boundaries, type consistency, lifecycle ownership, API seams, product thickness, or whether a spec feels complete vs actually useful."
---

# Spec Review Modes

Use this skill to review technical specs without collapsing into one vague style of feedback.

## Core rule

Always choose **one primary mode** first. You may add a short secondary pass later, but do not blur all three into one undifferentiated review.

The three modes are:

- **Mode A — Quick Overview**
- **Mode B — Strict Spec Audit**
- **Mode C — Product / Architecture Lens**

If the user does not choose, infer from intent:
- Wants a fast scan / first impression / "先看有沒有大問題" → **Mode A**
- Wants contradictions / ownership / API / type rigor → **Mode B**
- Wants product depth / market sense / "這樣會不會太薄" → **Mode C**

For detailed checklists and output scaffolds, read `references/review-checklists.md`.

## Before reading

1. Identify the root folder or top-level doc set.
2. Read directory structure first if the spec is multi-file.
3. Read `overview.md` and `schema-v0.md` first when present.
4. If there is a shared type file (`shared-types.md` or equivalent), read it before criticizing type drift.
5. Do not pretend to have read everything if you only sampled part of the stack.

## Mode A — Quick Overview

Use this when the goal is speed and orientation.

Focus on:
- what the system is trying to do
- whether the boundaries make sense
- the most obvious strengths
- the most dangerous ambiguities
- what to read next

Do:
- summarize the stack in plain language
- name the 2-5 biggest risks, not every nit
- point out where the docs feel clean vs muddy
- say what is missing only if it blocks understanding

Do not:
- enumerate every schema concern
- nitpick naming unless it causes real confusion
- drift into implementation planning unless asked

## Mode B — Strict Spec Audit

Use this when the goal is rigor.

Focus on:
- type consistency
- noun drift
- lifecycle ownership
- API boundary clarity
- read/write model separation
- duplicated truth sources
- unresolved canonical source questions

Default audit questions:
- Are canonical nouns defined in one place?
- Do shared types and downstream specs agree?
- Is lifecycle ownership centralized or leaking across contexts?
- Are APIs placed in the correct bounded context?
- Is any read-side module coupled to storage shape?
- Are there hidden transitions or implicit authority assumptions?
- Are there contradictory examples, tables, or state diagrams?

When you find problems, classify them:
- **Must fix** — will cause implementation drift or broken ownership
- **Should fix** — clarity/maintainability problem, not fatal
- **Future concern** — acceptable deferral, but note it

## Mode C — Product / Architecture Lens

Use this when the question is not just "is the spec coherent?" but "does this system become valuable, thick, and usable?"

Focus on:
- whether the spec creates real product pressure or only completeness
- whether the system will change behavior in workflows
- whether the abstractions are too early or too internal
- whether the spec is building something that feels alive/useful vs merely tidy
- where the user-facing leverage actually comes from

Default product/architecture questions:
- What real pain does this spec solve?
- Which parts create product thickness versus internal neatness?
- What would a user actually feel from this system?
- Which parts are entry points, and which should stay buried as substrate?
- Is this creating first-order value or second-order optimization?
- Does the spec over-index on architecture completeness?

Do not reduce this mode to market fluff. Stay anchored to the documents.

## Review method for multi-file stacks

For a spec tree like:
- `decision-model/`
- `decision-intake/`
- `decision-injection/`
- `decision-governance/`

Use this order unless the user says otherwise:
1. model/shared types
2. write/intake
3. read/injection
4. governance/lifecycle

Reason: you need canonical nouns before you criticize downstream seams.

## Good output shape

### For Mode A
- 1 short paragraph: what this stack is
- strengths
- biggest risks
- what to read/fix next

### For Mode B
- overall judgment
- must-fix list
- should-fix list
- future concerns
- boundary/ownership summary

### For Mode C
- what feels product-thick
- what feels internally tidy but externally thin
- what the actual wedge is
- what should remain hidden substrate
- conclusion

## Important discipline

- Do not say "everything looks good" unless it truly does.
- Do not inflate minor wording issues into architectural crises.
- Do not confuse a complete spec with a useful product.
- Do not review all files at the same level of detail if some are canonical and others derivative.
- Ground criticism in concrete files, nouns, tables, or transitions.

## Optional combined workflow

If the user wants a deeper pass, use this sequence:
1. **Mode A** to orient
2. **Mode B** to identify structural faults
3. **Mode C** to judge whether the resulting system is worth building as framed

Do not start with Mode C before you understand the structure.
