# When to Use Architecture Spec Stack

## Use When

- The problem is still at the "what even is this" stage
- The concept cannot be directly decomposed into tracks/tasks
- You need to compress the motif (母題) into architecture language
- You need to produce canonical forms before discussing engineering
- You need to align product language, architecture language, and engineering language
- Multiple people/agents have different understandings of "what this system looks like"
- You are defining a new runtime / layer / protocol / world model

### Typical Trigger Phrases

- "What even is this thing?"
- "Where's the boundary between this and X?"
- "What does one cycle look like?"
- "What is a first-class citizen?"
- "Which repo should this live in?"

---

## Do NOT Use When

- The concept is already clear, you just need task decomposition → use `project-plan`
- It's a single feature / bug fix / refactor → just do it
- You only need a one-page RFC-level decision doc → write an RFC
- You just want to brainstorm → don't pretend you're doing a spec

---

## How It Differs from Other Formats

| Format | What It Asks | What It Produces | When |
|--------|-------------|------------------|------|
| **Architecture Spec Stack** | What is this system really | Concepts / schema / canonical form / slice | Concepts not yet stable |
| **RFC** | Should we do X | One-page decision doc | Decision point |
| **PRD** | What do users want | Requirements spec | Product definition |
| **project-plan** | How to decompose into doable work | tracks / tasks / DoD | Concepts already stable |
| **ADR** | Why choose A over B | Decision record | Decision already made |

### Core Distinction

> Architecture Spec Stack is more continuous than an RFC, more technical than a PRD, and more upstream than a project-plan.
> It's like a chain: motif (母題) → layers → canonical form → schema → judgment → API → exemplar → demo path.
