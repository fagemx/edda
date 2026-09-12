# B1 — Adopt a usable continuity release cut with the existing owner

> Owner: existing continuity controller (task #113), not a replacement implementation team.
> Depends on: none. Unblocks: B2 after recorded owner adoption.
> Contract: DF-08/09. Delivery: one program amendment, not new feature code.

## Read first / confirm current ownership

```bash
edda task show 113
edda task show 140
edda task show 141
edda task show 76
git show f8e91dfa27deeee238e7cd4d6ff2a907f7478187:docs/superpowers/continuity/program.md
gh issue view 1141 --repo fagemx/edda --comments
```

Use `MSYS_NO_PATHCONV=1` for Git object path commands if Git Bash rewrites them.
Read the current owner plan, not only the older inspected hash in GAPS. A `running` task
is ownership evidence, not proof a process is alive; check current owner response/state.

## Scope and permission

Read-only preparation may start now. Only the current owner (or an explicitly agreed
handoff) edits GH1141's program/plan and issue metadata. This pack grants no cross-scope
write permission and does not itself cancel the prior final-carve approval condition.
No SDK writes, S5 fix takeover, new node implementation or changes to R6.

## Amendment to adopt

1. Preserve all S0–S10 product promises and actual security/compatibility acceptance.
2. Replace universal S10-before-carve with per-usable-slice proof/carve/landing.
   Whole-program completion still requires the remaining cross-machine/control proof.
3. Choose first slice: local save/show/list/restore + portable identity/offline bundle +
   native continuity-save/restore skills from the existing accepted candidate. Prefer
   one coherent PR unless consumers reveal a real independently useful split.
4. Explicitly exclude live node sync, task-prepare/S5 execution, Flash controller,
   manager retirement and cross-machine process takeover from this first PR.
5. Refresh stale #1135/#1136 prerequisite text against main/closed issue facts; don't
   duplicate their completed fixes. Re-measure #1134 if any first-slice caller depends on it.
6. Replace repeated final-carve prompts with one documented adoption of this specific
   release cut under existing operator authority. If the owner determines the earlier
   plan still requires an operator decision, present this single scoped amendment once;
   do not manufacture authority or ask again at every normal R6 merge.
7. Confirm the first slice's SDK/event consumer owner and one handoff interval; no
   assumption that #141 is unrelated merely because it names S5. If unavailable, prepare
   a scoped compatibility diff for that owner, not a competing SDK implementation.

## Output

An update in the existing durable program carrier containing:

- exact candidate/source SHAs and included behavior;
- exclusions and remaining program promises;
- owner acceptance of changed landing order and authority reference;
- affected consumer/schema paths, named owner and pending checks;
- B2 integration path and receipt links.

Use `SliceReadiness` semantics from CONTRACT as optional prose shape. Do not create
another mandatory JSON artifact, six child issues or a new controlling board.
Update the owner plan and its program/issue references coherently, not a second authority.

## Acceptance / verification

V4 in [VALIDATION](../VALIDATION.md): a ready local feature can land without S3/S6/S10,
but cannot claim sync/whole program done. The exact prior carve condition is explicitly
amended, not merely contradicted by a new note. No peer work is altered before permission.

```bash
sh scripts/lint-markdown-content.sh
sh scripts/lint-doc-citations.sh --tree
git diff --check
```

Docs-only, no Cargo and no build lane. No claim that local accepted candidates have
exact-head CI; the current program explicitly says they do not.

## Delivery / return

Intent: `docs(continuity): separate usable slice landing from program completion`.
An unavailable owner pauses B's write/integration only. A/C continue. Do not delete old
branches or rewrite unmerged candidate history; preserve the full program acceptance.
