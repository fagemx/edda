# Proactive Operation

## Intent

The loop should not behave like a passive question-answer bot. It should infer safe next actions, route to relevant skills/tools, execute reversible work, and stop only at meaningful checkpoints or real approval gates.

## Default Behavior

When given a broad request:

1. Inspect supplied files or project state.
2. Identify profile, scale tier, and likely missing decisions.
3. Choose installed skills/tools that fit the task.
4. Create or update control files.
5. Execute the safest useful work package.
6. Write evidence, state, index, and next command.

Do not ask the user to tell you obvious next steps that can be inferred from local context.

## Autonomy Budget

Use a work package, not a tiny task, as the execution unit.

A work package can include:

- reading relevant files
- creating or updating one artifact
- running tests or checks
- fixing issues found by those checks
- updating state and ledger
- preparing the next command

A work package must not cross:

- stage boundaries
- approval gates
- architecture freeze decisions
- production or public-release boundaries
- unrelated backlog items

## Command Selection

If the skill is registered, generate:

```text
active goal run Use the verifiable-goal-loop skill for this project...
```

If the skill is standalone and unregistered, generate:

```text
active goal run Read [skill path]/SKILL.md, then follow the project files in [workdir]...
```

Always include:

- files to read
- current work package
- evidence to produce
- checks to run
- files to update
- stop condition

## Skill Routing

Choose tools from the current environment, not from wishful thinking. If a tool is unavailable, record it as optional and continue.

Routing examples:

- Product uncertainty: product/architecture gate first.
- UI/frontend: Product Design if available; Browser for local verification.
- Data work: Data Analytics.
- GitHub PR/CI: GitHub plugin skills.
- Documents/spreadsheets/PDF/slides: corresponding document plugin skills.
- Missing specialized capability: find-skills, then recommend or record fallback.
- Lulin project: Lulin project files are authority; this skill becomes adapter.
- superpowers/gstack: use only if provided; otherwise record them as optional engines.

## Stop Policy

Stop only when:

- the current work package reaches its checkpoint
- verifier or oracle fails and repair is not obvious
- the same failure repeats three times
- Red approval is needed
- the user-facing direction changed
- stage review is due

Do not stop merely because one file was edited or one note was written.
