# Fleet Orchestration Playbook

## Contents

1. Authority and preflight
2. Truth, doorbell, and isolation
3. Formation and scaling
4. Two-rail operating model
5. Review campaign and drift control
6. Finding promotion
7. Delivery graph and ownership
8. Role contracts and briefs
9. Rulings and cross-session traffic
10. Monitoring and recovery
11. Scoped verification and merge
12. Carrier selection
13. Worked example
14. Common mistakes

## 1. Authority and preflight

Write an authority contract before dispatch:

| Field | Required content |
|---|---|
| Goal | Observable end state, not an activity |
| In scope | Repositories, issues, modules, branches |
| Out of scope | Explicit exclusions and optional work |
| Evidence | Issue/spec-required facts and safety proof; do not invent acceptance fields |
| External writes | Whether the fleet may file/close issues, comment, push, or open PRs |
| Merge authority | Who may merge, which PRs/SHAs, and ordering constraints |
| Stop conditions | Complete, blocked, coverage/cost limit, diminishing returns, or operator decision |

A terminal instruction such as “do not stop” requires persistence toward the
goal. It does not authorize new products, repositories, issue spam, scope
expansion, or merge.

Then inspect:

- repository `AGENTS.md`, `CLAUDE.md`, equivalent instructions, and relevant
  skills;
- base branch, full HEAD SHA, dirty files, remotes, CI, and test commands;
- active sessions, task rail, claims, requests, decisions, issues, and PRs;
- candidate issues for age, current relevance, duplicates, existing fixes,
  dependencies, and shared files.

Preserve unrelated dirty work. Never initialize coordination infrastructure,
create a remote, or change repository policy unless that mutation is within
the operator’s authority.

## 2. Truth, doorbell, and isolation

Keep three layers separate:

| Layer | Examples | Rule |
|---|---|---|
| Truth | edda tasks/decisions/notes, issue bodies/comments, Git commits | Must survive session death |
| Doorbell | host thread messages, `edda request` notification | Carries pointers, not the only copy |
| Isolation | worktree, branch, path/symbol claim | Prevents concurrent writes |

Record a load-bearing fact in the truth layer first, then notify the affected
session. A queued or crossed message cannot reorder durable rulings.

## 3. Formation and scaling

Default formation:

- **Controller:** owns goal, graph, authority, board, arbitration, and final
  acceptance matrix. Stay out of implementation while multiple lanes run.
- **Workers:** each owns one cohesive write bundle and produces a frozen
  candidate plus evidence. Workers do not merge.
- **Verifier:** starts read-only before implementation, checks the baseline,
  builds acceptance criteria, and independently reviews frozen candidates.

From two concurrent workers onward, reserve one independent verifier. If only
one writer exists, obtain an independent review before integration rather than
self-approval.

Choose concurrency by the graph, not by available Session count:

`worker WIP = min(independent ready bundles, available writers, verifier capacity)`

Add a worker only when a ready bundle has disjoint ownership and can be
verified without starving the review gate. Same code chain, shared invariant,
or unresolved dependency means serial work even when sessions are idle.

One controller can manage review and delivery simultaneously because they are
separate rails with compressed state. Split into two fleets only when the
control plane is demonstrably saturated—for example, the board is falling
behind, rulings wait unhandled, or a long-running review campaign must continue
while delivery consumes all verifier capacity. Never put two controllers over
the same rail; both report to one merge authority.

## 4. Two-rail operating model

### Review/discovery rail — read-only

`campaign cell -> candidate -> verified finding -> issue draft/issue`

Reviewers inspect, reproduce, compare against base, and attach evidence. They
do not repair what they discover. If filing issues is not authorized, stop at
a durable verified draft.

### Delivery rail — write-enabled

`ready issue -> bundle -> claimed worktree -> frozen SHA -> independent review -> merge candidate`

Only promoted issues with acceptance criteria enter delivery. Workers may
report adjacent suspicions back to discovery but must not silently widen their
bundle.

These are two queues, not automatically two armies. Preserve role independence
for an artifact: the session that authored a change does not verify or approve
that same change.

## 5. Review campaign and drift control

Create a review charter before scanning:

| Charter field | Meaning |
|---|---|
| Object | Repo, subsystem, diff, workflow, or invariant under review |
| Basis | Base branch and full SHA |
| Lenses | Fixed set such as correctness, security, concurrency, data loss, UX, tests |
| Exclusions | What this campaign will not inspect |
| Blocking surface | Changed behavior/paths, direct consumers, acceptance, introduced/exposed safety regressions, current-base integration |
| Evidence bar | Reproducer, failing test, trace/log, or direct code proof |
| Coverage | Matrix of scope units × lenses |
| Stop rule | Coverage/budget complete, blockers resolved, cost stop, or operator decision |

Assign each review task one coverage cell. Rotate lenses across cells instead
of repeating one familiar bug pattern. Mark each cell with reviewer, basis SHA,
evidence, candidates, and disposition.

Prevent drift:

- Keep the object and exclusions fixed for the campaign.
- Route adjacent, pre-existing, or speculative defects that do not invalidate
  the blocking surface to evidenced follow-up issues. Expand the charter only
  through a durable operator-approved ruling.
- Compare suspected regressions against the basis revision before escalating.
- Have the verifier sample both positive findings and “clean” cells; a campaign
  that only checks its hits trains itself toward confirmation bias.
- Stop by the charter’s coverage rule, not by fatigue and not by an endless
  demand to find more issues.

## 6. Finding promotion

Use these states:

1. **Candidate:** plausible observation; never scheduled for repair.
2. **Verified finding:** evidence establishes actual behavior on a named full
   SHA and distinguishes inherited behavior from a regression.
3. **Issue draft:** actionable scope and acceptance contract exist.
4. **Ready issue:** issue creation/priority is authorized and dependencies are
   explicit.

Require this evidence before promotion:

- full basis SHA, path and symbol;
- expected versus actual observable behavior;
- minimal reproduction, failing test, log/trace, or direct proof;
- impact and severity without speculation;
- base comparison and duplicate/existing-fix check;
- smallest repair boundary, dependencies, and likely overlapping files;
- `doneWhen` and exact verification commands.

Do not file vague “investigate X” issues merely to inflate the queue. If the
direction is useful but evidence is incomplete, retain the candidate with the
attempts already made and the next bounded experiment.

Finding priority and current-PR disposition are separate. A serious adjacent
defect can be a high-priority follow-up without becoming a blocking P0/P1 in
the current PR. Direct caller/consumer regressions, introduced or exposed
security/data-loss regressions, and current-base integration conflicts remain
blocking.

## 7. Delivery graph and ownership

Build a graph over ready issues:

- Join issues that traverse the same code chain or invariant.
- Add dependency edges for API/schema/order constraints.
- Record overlapping files and, when unavoidable, exact symbol/section
  ownership.
- Prefer one cohesive serial bundle over two workers editing the same path.
- Do not brief both overlapping bundles as active and merely tell workers to
  serialize themselves. Keep the later bundle blocked and unassigned until
  the current owner freezes and releases the overlapping scope.
- Assign each bundle one worktree, branch, claim, owner, verifier criteria,
  and done gate.

Every bundle brief contains:

```text
Intent and issue/spec references
Basis branch and full SHA
Owned paths/symbols; forbidden paths
Dependencies and highest durable d-NNN ruling
Required worktree/branch and claim label
Test-first acceptance criteria and exact gates
Build lane: <assigned name from the fixed pool> at <absolute lane root>
  (the worker resolves its build directory as <lane root>/<lane name> and never
  invents one; if this line is missing, ask before building)
Verification budget: focused gates on touched units while iterating; the full
  gate set once per frozen full SHA with a gate receipt; READ receipts and
  exact-head CI before any RAN; name the coverage the project's CI genuinely
  lacks, since that is the reviewer's legitimate reason to run it
Cleanup authority: lane build cache is disposable; worktrees, branches, and
  sources are never deleted
Drift rule: if HEAD differs, satisfy intent against HEAD and report full SHA
Done: pushed candidate/PR, frozen full SHA, receipt, no merge
```

Line numbers are hints, not durable anchors. Code-touching instructions use
intent + basis SHA + symbol anchor + drift rule.

## 8. Role contracts and briefs

### Worker contract

1. Read repository rules, task/spec, board, decisions, and claims.
2. Claim only the approved paths and create/use the assigned worktree.
3. Reproduce the failure or write the failing acceptance test first.
4. Implement the smallest in-scope repair; surface adjacent findings instead
   of taking them.
5. Run focused gates on touched units while iterating; run the full gate set
   once on the frozen full SHA in the assigned build lane and record the gate
   receipt (SHA, gate set, toolchain, lane, result). Never build in an ad-hoc
   directory.
6. Push/open the delivery candidate if authorized, report the full SHA, freeze
   the branch, and leave a receipt tagged `ran` versus `read` that cites the
   gate receipt.
7. Never merge or rewrite another worker’s branch.

### Verifier contract

1. Remain read-only for every artifact under review.
2. Establish the baseline once on the basis SHA before workers code: READ
   exact-head CI and any existing gate receipt, RAN only what they do not
   cover in the assigned verifier lane, and classify existing red checks.
3. Translate each issue into observable acceptance criteria.
4. Audit tests for false greens and tests that encode behavior being removed.
5. Review the exact frozen full SHA, complete the whole scoped audit, compare
   against the basis, and batch every blocking P0/P1 with path/symbol and
   failure scenario before requesting changes. READ the frozen SHA's gate
   receipt and exact-head CI; RAN only focused or adversarial checks they do
   not cover, and state the reason for any full rerun.
6. Acknowledge that any push voids the verdict; a status, label, or draft flip
   is not a push and reruns nothing.
7. Keep one verifier identity per delivery candidate: rounds resume the same
   session and lane; a replacement reads receipts and CI before running
   anything.

### Controller contract

1. Maintain authority, graph, board, rulings, and WIP.
2. Send zero-context briefs and relay only verified cross-lane evidence.
3. Avoid reading full diffs mid-flight; consume status, receipts, blocker
   reports, and verifier criteria until formal review time.
4. Never reinterpret a receipt as acceptance.
5. Assign build lanes from the fixed pool, put the verification budget and
   cleanup authority in every brief, and route over-verification (a second RAN
   for an already-receipted SHA without a reason, full gates for a docs-only
   push, an ad-hoc build directory) as a process finding — cost line,
   `FOLLOW-UP ISSUE`, corrected brief — never as a reason to block a
   product-green candidate.

## 9. Rulings and cross-session traffic

Give normative rulings monotonic identifiers: `d-001`, `d-002`, and so on.
Changing one requires `SUPERSEDES d-NNN` in the same durable carrier. Every
brief includes this tie-break:

> Obey the highest durable `d-NNN`, not the latest message. If a message
> conflicts with the ledger, report current state instead of executing it.
> Never discard pushed work unless the ruling names the exact commit.

Agent-authored edda decisions are recorded guidance until an operator ratifies
them. Use `edda ratify` only when the operator actually grants that authority;
do not self-ratify an operator choice.

When crossing ownership, send a durable request first and a host message that
only points to it second. The owner either acknowledges and changes its scope,
or the controller records a new ruling before reassignment.

## 10. Monitoring and recovery

Maintain one machine-findable fleet board line per transition:

```text
<rail>/<bundle> | <owner> | task <id> | <branch>@<full-sha> |
ACTIVE|BLOCKED|FROZEN | review:<state> | next:<action>
```

Use event-driven waits or compact snapshots. Do not interrupt busy workers for
status already available from the rail, PR, branch, or receipt. A worker update
should contain only: state, full SHA, gates run, blocker, next action.

If a session dies:

1. Read the authority contract, board, task, claims, decisions, and Git state.
2. Check for an unseen push before expiring or replacing ownership.
3. Assign a replacement with a complete brief and the last known full SHA.
4. Never depend on the dead session’s chat history.

If work is blocked, record the reason and evidence, release/reassign ownership
using the runtime’s current mechanism, and continue other independent ready
bundles. Do not spin or broaden scope to appear active.

## 11. Scoped verification and merge

Every review handoff freezes one blocking contract:

This is a bounded complete review, never a minimal review. The reviewer audits
the entire frozen surface. Every failure in changed behavior/paths, a direct
caller/consumer, explicit acceptance, introduced/exposed security or data
loss, or current-base integration is mandatory. Only findings genuinely
outside that surface qualify for follow-up.

This workflow applies when the coord review/orchestration skills are invoked;
it is not an Edda runtime rule imposed on every project.

**`IN SCOPE`**

- changed behavior and changed paths;
- directly affected callers and consumers;
- explicit issue/spec acceptance;
- security or data-loss regressions introduced or exposed by the change; and
- current-base integration conflicts.

**`FOLLOW-UP ISSUE`**

- adjacent or pre-existing defects that do not invalidate requested behavior;
- speculative improvements; and
- extra evidence/format preferences beyond the acceptance ceiling.

File follow-ups with evidence and a basis SHA. They do not extend the current
PR, require an implementer response, or create another review round unless the
operator intentionally pulls their fix into scope. The issue/spec is the
acceptance ceiling; evidence beyond it becomes mandatory only when its absence
prevents proving a required fact or safety boundary.

Before posting `Changes Requested`, finish the whole scoped audit and batch all
blocking P0/P1. A later round may add a blocker only if the fix caused it or
made it previously unobservable; otherwise route it to follow-up.

Select gates proportionally:

| Delta | Required treatment |
|---|---|
| Code/product blob, base SHA, or toolchain changed | Run the relevant code gates for the changed risk surface. |
| Only docs/evidence changed; code/product blobs, base, and toolchain unchanged | Reuse still-applicable code gates as `READ` with source SHA; `RAN` only relevant diff/docs/evidence checks and exact-head CI. |
| Code changed; a gate receipt for this exact SHA, gate set, and toolchain exists and exact-head CI is green | READ the receipt and CI; `RAN` only focused or adversarial checks they do not cover. Full rerun only with a stated reason (no receipt, red or absent CI, grounds to distrust the receipt). |
| Status, label, or draft flip without a push | Not a new artifact; nothing reruns. |
| Exact-head CI is deterministically red | The SHA is already blocked; finish the scoped audit and request changes instead of spending a full local run that cannot change the verdict. Rerun only failed jobs when logs show a transient failure, and run the missing full set only once the artifact is otherwise acceptable. |

Never label a reused gate `RAN`. Exact-head CI is still required because the
review verdict binds the new full SHA.

Verify once per frozen artifact. The implementer's full gate run leaves a gate
receipt (SHA, gate set, toolchain, lane, result); the reviewer cites it. Runs
happen in the assigned build lane, never in an ad-hoc directory.
Over-verification found in evidence is a process finding — cost line,
`FOLLOW-UP ISSUE`, corrected brief — not a product blocker.

Record available elapsed time, token use, and tool cost in every handoff. Stop
after two consecutive cycles that change only non-product evidence/docs or
harness material without improving required behavior/proof, or sooner when
returns clearly diminish. Classify and route instead of continuing: open a
follow-up issue for out-of-scope work, or ask the operator to expand scope.

Use this handoff request before review; it is not an implementer verdict:

```text
## Code Review Handoff: Round N
Full SHA: <full SHA>
Base full SHA: <full SHA>
IN SCOPE: <frozen blocking surface>
FOLLOW-UP ISSUE: <links or none>
Blocking counts entering review: P0=<n>, P1=<n>
Evidence:
- RAN: <commands/checks executed on this SHA>
- READ: <reused results and source SHAs>
Cost: elapsed=<available/unknown>, tokens=<available/unknown>, tools=<available/unknown>
Request: complete the scoped audit and publish an independent verdict
```

For each candidate:

1. Worker freezes and reports a full SHA with a receipt.
2. Controller checks scope and issue acceptance coverage.
3. Independent verifier reviews that SHA and runs the proportional required
   gates.
4. Any push voids the verdict and triggers a delta review from the new SHA.
5. When the delivery artifact is a GitHub PR, publish the review on the PR;
   internal reports and chat are supporting evidence, not a substitute:
   - `## Code Review: Round N` with reviewed full SHA, frozen `IN SCOPE`,
     blocking P0/P1 counts/findings, `FOLLOW-UP ISSUE` links, `RAN` versus
     `READ` evidence, cost, and `Changes Requested` or `LGTM`;
   - after `Changes Requested`, the implementer publishes
     `## Review Response: Round N`, answering every blocking finding with
     disposition, changed paths/tests, ran evidence, and the new full SHA;
   - the reviewer starts Round N+1 on that new frozen SHA. Repeat until the PR
     has a final current-head `LGTM` comment stating P0=0, P1=0 and exact gates.
6. Controller reads `headRefOid`, PR comments/reviews, and CI immediately before
   merge. A final comment for an older SHA, an unanswered finding, or an
   internal-only verdict keeps the candidate blocked.
7. Controller builds a final matrix: issue/criterion, evidence, reviewed SHA,
   CI/local gate, PR round/response/final-verdict links, and remaining risk.
8. Merge only when the current reviewed SHA is green, the visible review loop
   is closed, and merge authority is explicit. Preserve required order and
   recheck state immediately before each merge.

Use the same round/response/verdict fields in the strongest durable review
carrier for local-only delivery. Do not invent a PR requirement when no PR
exists.

The formation produces delivery candidates and execution evidence. Acceptance
and merge remain outside it unless the operator explicitly delegates them.

## 12. Carrier selection

Use the strongest available durable carrier without assuming a specific host:

| Environment | Truth and queue | Delivery artifact |
|---|---|---|
| edda initialized | `edda task`, `decide`, `note`, `claim`, `request` | branch/worktree and optional PR |
| GitHub remote, no edda | issue/PR bodies and comments | PR pinned to full SHA |
| Local Git only | operator-approved local board plus Git commits | local branch pinned to full SHA |

Useful edda verbs; run `--help` because installed versions may differ:

```text
edda peers
edda coord
edda task new "<bundle>" --assignee "<role>" --brief "<self-contained brief>"
edda task start <id>
edda task done <id> --receipt "<ran evidence and full SHA>"
edda task fail <id> --reason "<blocker>"
edda claim "<label>" --paths "<path>"
edda decide "d-001.topic=value" --reason "<why>"
edda request "<owner-label>" "<scope request or ruling pointer>"
edda request-ack "<sender-label>"
edda note "<board line>" --tag fleet-board
```

`edda request` is a durable registered letter, not necessarily a wake-up
mechanism. Ring the host’s thread/session message after writing the letter.

## 13. Worked example

Suppose seven issues form three independent chains:

- A1 → A2 both edit the parser;
- B1 → B2 both edit persistence;
- C1 and C2 are independent UI fixes;
- S is an unverified review suspicion.

With three writer slots and one verifier, start A1, B1, and C1. Keep A2 and B2
blocked by their predecessors; schedule C2 when its verifier capacity is
available. Assign S to the read-only discovery rail. Promote S only after
reproduction and base comparison; then place it in the graph according to its
actual file overlap. More sessions do not make A1 → A2 parallel.

## 14. Common mistakes

| Rationalization | Correction |
|---|---|
| “More Sessions always means more speed.” | Only independent ready bundles create useful concurrency. |
| “Review and delivery need two armies immediately.” | Start with two logical rails under one controller; split only when control saturates. |
| “The finding is obvious; file/fix it now.” | Candidate first; promote on named-SHA evidence and authority. |
| “The controller can review its own plan at the end.” | Reserve an independent read-only verifier before code. |
| “The user said keep going, so nearby work is allowed.” | Persistence does not broaden scope or external-write authority. |
| “The latest message is newest truth.” | Highest durable `d-NNN` wins; messages are doorbells. |
| “The PR barely changed after approval.” | Any push changes the artifact and voids the SHA-bound verdict. |
| “This adjacent defect is real, so it must block.” | Preserve the evidence in a follow-up issue unless it invalidates the frozen blocking surface. |
| “A new preference deserves another review round.” | The issue/spec is the acceptance ceiling; only required facts and safety proof can raise it. |
| “Docs changed, so rerun every code gate.” | Reuse still-applicable code gates as `READ`; run delta checks and exact-head CI as `RAN`. |
| “We can reveal the next blocker after this fix.” | Complete the scoped audit and batch P0/P1; later blockers must be fix-caused or previously unobservable. |
| “One more harness/evidence cycle might help.” | After two non-product cycles without useful progress or at diminishing returns, stop and route. |
| “The verifier report is enough; PR comments are clerical.” | A GitHub PR is the durable collaboration surface. Publish every review round, response, and final current-head verdict there. |
| “The final LGTM implies earlier findings were handled.” | Changes Requested requires an implementer point-by-point response before re-review. |
| “Receipt says tests pass, so merge.” | Receipt is worker evidence; the verifier READs it against exact-head CI, RANs what they do not cover, and merge authority accepts. |
| “A fresh session should build in its own directory to be safe.” | Use the assigned lane; lanes are the isolation. Ad-hoc build directories are forbidden. |
| “The verifier must rerun everything to be independent.” | Exact-head CI and the gate receipt are independent evidence; RAN only what they do not cover, or state the reason. |
| “The PR flipped to ready, so rerun the gates.” | Not a push; nothing reruns. |
| “Cleanup is destructive, so leave every build directory.” | Lane build cache is disposable by design; worktrees, branches, and sources are not. |
