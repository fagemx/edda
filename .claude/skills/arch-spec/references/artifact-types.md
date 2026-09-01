# Artifact Types — Spec File Type Catalog

Each spec stack selects the types it needs from this catalog. Not every type is required.

---

## Core Types

| Type | Purpose | When Needed | Naming Example |
|------|---------|-------------|----------------|
| **Motif (母題) / Overview** | One-liner definition + global map | Always | `overview.md`, `intent-router-and-space-builder.md` |
| **Canonical Form** | The system's core repeating unit | When there's a loop or pipeline | `canonical-cycle.md` |
| **Schema** | TypeScript types, data structures | When there are structured entities | `change-proposal-schema-v0.md` |
| **Rules / Judgment** | Decision logic, invariants, constraints | When the system makes judgments | `judgment-rules-v0.md` |
| **API** | REST routes, request/response shapes | When the system exposes endpoints | `world-cycle-api.md` |
| **Metrics / Pulse** | How to measure health, outcomes, state | When the system has observable state | `pulse-and-outcome-metrics-v0.md` |

## Grounding Types

| Type | Purpose | When Needed | Naming Example |
|------|---------|-------------|----------------|
| **Canonical Slice** | Minimum concrete instance (the system's hello world) | When concepts need grounding | `midnight-market-canonical-slice.md` |
| **Demo Path** | Step-by-step walkthrough proving closure | When you need to prove closure | `midnight-market-demo-path.md` |
| **Gap Analysis** | What's missing now, what must be added | When retrofitting or evolving | `5-gaps.md` |

## Multi-Mode Types

| Type | Purpose | When Needed | Naming Example |
|------|---------|-------------|----------------|
| **Regime / Variant** | Deep-dive into a specific mode | When the system has different modes | `economic-regime-v0.md`, `governance-regime-v0.md` |
| **Comparison Matrix** | Structural comparison across modes | When 3+ variants exist | `regime-comparison-matrix.md` |
| **Test Cases** | input → expected output pairs | When there's classification/routing logic | `router-test-cases.md` |

## Cross-System Types

| Type | Purpose | When Needed | Naming Example |
|------|---------|-------------|----------------|
| **Shared Types** | Cross-file type single source of truth | When 3+ specs share types | `shared-types.md` |
| **Handoff Contract** | System boundary handoff format | When 2+ systems interact | `T0_VOLVA_HANDOFF.md` |

---

## Quality Standard for Each Spec

A spec must have at minimum:

1. **Why this file exists** — an opening paragraph
2. **One-liner definition** — a core sentence that can be quoted
3. **What it's NOT** — 2-4 failure modes
4. **Canonical form / schema / flow** — structured content
5. **Canonical examples** — at least 2 concrete walkthroughs
6. **Boundaries** — in-scope and out-of-scope
7. **Relationship to other files** — reference links

Missing any of these = not finished.

---

## Decision Table — How to Select Artifact Types for This Round

Not a menu, but a selector. Choose by problem shape:

| If the problem looks like this | Prioritize producing |
|---|---|
| Still defining the core cycle | overview + canonical-form |
| Have first-class citizens but types are messy | schema + shared-types |
| Have external interfaces / system boundaries | api + handoff contract |
| Too abstract, needs grounding | canonical slice + demo path |
| Has rules, judgment, risk classification | rules / judgment |
| Has multiple modes (3+) | regime docs + comparison matrix |
| Has classification / routing logic | test cases |
| Retrofitting an existing system | gap analysis |

### Recommended Combinations

| Scenario | Recommended Combination |
|----------|------------------------|
| Brand new system v0 | overview → canonical-form → schema → demo-path |
| Existing system, need to add boundaries | overview → rules → api → handoff |
| World/governance system | overview → canonical-form → schema → rules → slice → demo-path |
| Router/dispatcher system | overview → canonical-form → schema → regimes → matrix → test-cases |
