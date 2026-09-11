# B2 — Integrate and deliver the existing usable continuity slice

> Owner: existing continuity owner/integrator; SDK changes by the agreed SDK owner.
> Depends on: B1 adopted release amendment and source handoff, not A/C or S5–S10.
> Contract: DF-05/06/08/09. Delivery: one usable candidate/PR, not a new implementation.

## Input / existing commits

Local reference candidate: `f8e91dfa27deeee238e7cd4d6ff2a907f7478187`.
Check these objects and current owner history before using them:

| Commit | Existing behavior |
|---|---|
| `8bda180e54db221f3b54c3155264bd6842d71238` | portable local continuity implementation |
| `2c0e0f2b5a53cf639eaf6eb6863b307f8efdecd5` | first-round integrity corrections |
| `7ef7dfba0ba8dbce45fd8e5335baf883269b3353` | exact restore/alias corrections |
| `e0e25416d6a4b9cab367ad6c98a1886448fe3847` | native continuity skills |
| `b46d4bceb44b042614d633a73f320ae358962b28` | bounded metadata skill correction |

S1/S2 local receipt chain: tasks #114/#117/#118/#120/#121/#122.
Skills local receipts: #124/#126/#127/#129. Local acceptance is execution/review evidence,
not current-main integration approval or exact-head CI. Objects may be local-only;
if missing, ask owner for source publication rather than reconstructing a feature from prose.

## Scope

Existing continuity modules in `edda-core`, `edda-ledger`, `edda-store`, CLI
`cmd_continuity`, CLI main/init, two native continuity skill sources, existing continuity
process tests, continuity event schema/registry/fixture, and their direct documented
compatibility/SDK consumers. Only integration-caused fixes within this behavior belong here.
No S5 prompt authority changes, node, coordinator state machine or manager migration.

## Steps

1. Bind latest base full SHA; obtain owner permission and use a new integration worktree
   or the owner-approved candidate worktree. Never reset/rebase pushed source stacks.
2. Inspect exact source commit diffs and dependencies; adopt the existing implementation
   commits in order onto current base, resolving only actual integration conflicts.
   Do not blindly cherry-pick program-wide docs that restore S10-before-landing.
3. Confirm usable CLI/save/import/restore behavior and embedded skill projection. It is
   acceptable to ship local+bundle+skills in one PR; do not create artificial API-only PRs
   that force users to wait again for the usable entry point.
4. Reconcile event spec registry, fixtures, SDK generated types and SPEC_PIN through the
   assigned owner. If a spec pin must advance, ensure the pinned full commit is fetchable
   from origin before client CI depends on it; preserve generator/consumer ordering.
5. Run the slice's focused checks once. Reuse earlier reasoning/receipts for unchanged
   behavior, but run necessary current-base integration and exact-head checks.
6. Create the smallest externally useful delivery issue(s) only now, using observed code
   boundaries, or reference the existing tracking issue for partial delivery. No
   `Closes #1141` while node/guided/control promises remain incomplete.
7. Publish PR with usable behavior/exclusions, exact-head CI and prior evidence links.
   Same independent reviewer can READ local audits and examine the integrated surface;
   final current-head PR verdict still required, no self-verdict.
8. Controller with existing R6 authority uses canonical merge once all existing conditions
   hold. Do not wait for Flash pilot, node, manager cutover or every program slice.

## Focused checks (bound worktree and lane only)

```bash
cargo fmt --all --check
cargo clippy -p edda-core -p edda-ledger -p edda-store -p edda --all-targets -- -D warnings
cargo test -p edda-core -p edda-ledger -p edda-store -p edda
sh scripts/lint-file-length.sh --tree
sh scripts/lint-markdown-content.sh
sh scripts/lint-doc-citations.sh --tree
git diff --check
```

No default full workspace rerun. Verify installed executable provenance; do not test a
stale PATH binary instead of the candidate. Core is outside Windows CI's 7-crate subset:
verifier runs the focused Windows core gap once at frozen SHA in its assigned lane.

The SDK owner uses the actual current contract workflow and, with a fresh candidate
binary, `EDDA_BIN=<absolute-candidate-binary> node sdk/run-contract-tests.mjs` from repo
root. Materialize the pinned spec per sdk README in the owner's scope first. The runner
can regenerate outputs, so it is not a reviewer read-only check.

## Acceptance

V4 cases: exact local save/readback; new session restores goal/next action; bundle imports
once to a fresh isolated store in a second clone deriving the same portable repository
identity (same canonical remote or explicit approved mapping); duplicate import visible;
unrelated repository import correctly refuses. Corrupt/conflicting content
refuses; dirty/missing SHA are warnings; old binary feature detection; imported commands
remain data. Live sync unavailable is truthful, not silently successful.

Record actual candidate SHA, runtime path/version, source commit map, CI run, Windows gap,
SDK parity and independent verdict. Feature availability is not whole program completion.

## Return / rollback

One scoped consumer blocker pauses B2, not A/C. A fix needing node/control changes means
the cut is not independent: return to B1 with evidence rather than grow this card.
Rollback by new fix/revert commits, never delete ledger history, peer branches or sources.
