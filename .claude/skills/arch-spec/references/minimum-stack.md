# Minimum Stack — Don't Default to Full Dossier

## Core Principle

> **Don't generate a full stack by default. First determine whether this round needs minimal / standard / full.**

Most of the time minimal is enough. Only use full when defining a complete system.

---

## Three Modes

### Minimal Stack (concept just starting)

```text
docs/<name>/
├── overview.md            ← motif (母題) + one-liner + what it's NOT
├── canonical-form.md      ← what one cycle / one flow looks like
└── shared-types.md        ← core types (if there are cross-file concepts)
```

**Use when**:
- Just starting to discuss "what even is this thing"
- Still defining the core cycle
- Terminology not yet stable

**Don't use when**:
- There are schemas ready to be landed
- Already need to interface with other systems

---

### Standard Stack (core has taken shape)

```text
docs/<name>/
├── overview.md
├── canonical-form.md
├── schema-v0.md           ← first-class citizen types
├── api.md                 ← external interface
├── demo-path.md           ← run through once to prove closure
└── shared-types.md
```

**Use when**:
- Core cycle is stable
- Need to start defining schemas and APIs
- Need to prove the system can close the loop

---

### Full Stack (complete system definition)

```text
docs/<name>/
├── overview.md
├── canonical-form.md
├── schema-v0.md
├── rules-v0.md            ← judgment / invariants
├── api.md
├── canonical-slice.md     ← minimum concrete instance
├── demo-path.md
├── shared-types.md
├── <regime>-v0.md         ← regime/variant deep-dives (0-N files)
├── comparison-matrix.md   ← cross-regime comparison (if 3+ regimes)
├── test-cases.md          ← routing / classification test cases
└── handoff-contract.md    ← cross-system boundary handoff
```

**Use when**:
- A complete system needs to be defined
- There is judgment logic / invariants
- There are multiple modes / regimes
- Need to interface with another system

---

## How to Decide Which Mode

| Signal | Mode |
|--------|------|
| "I'm not sure what this thing is yet" | minimal |
| "I know the core cycle, but schema isn't defined yet" | standard |
| "The whole system needs to be defined — judgment, API, slice" | full |
| Terminology still changing | minimal (don't rush) |
| Already have a canonical slice to write | at least standard |
| Need cross-system handoff | full |

## You Can Upgrade

Start with minimal, add more once things stabilize.
Add a new spec file with `arch-spec add` — no need to start over.
