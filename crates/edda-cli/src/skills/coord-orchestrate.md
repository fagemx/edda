---
name: coord-orchestrate
description: Use when coordinating two or more sessions on parallel implementation work as the controller — assigning bundles, briefing workers and a verifier, adjudicating mid-flight, and closing with dual review
---

# Coordination Orchestrate

You are the controller of a multi-session formation. The other coord skills
teach a session to be a good peer (coord-sync/request/handoff/review); this
one teaches the seat that runs the whole formation. The companion prose is
the "Coordination discipline" section of edda's multi-agent guide.

This policy applies when this skill is invoked. It guides the coordinating
session; it is not an Edda runtime rule imposed on every project.

## Layers — never mixed

| Layer | Carrier | Property |
|---|---|---|
| Truth | `edda task` / `edda decide` / issue comments | survives any session's death |
| Doorbell | `edda request`, host cross-session messaging | may drop — never the only copy |
| Isolation | one git worktree per work bundle | conflicts impossible, not discouraged |

Core rule: **messages may drop; state may not.** Every load-bearing fact is
fixated in the truth layer FIRST, then doorbelled.

## Formation

1 controller (you) + 1 read-only verifier + N workers. The verifier is
mandatory from 2 workers up — it is the highest-leverage seat, not a wasted
worker: your review inherits your own spec blind spots, and end-only review
is reactive.

The whole formation — you included — produces delivery candidates and
**execution evidence**, not acceptance. A worker's `edda task done --receipt`
proves what was done and how it was verified; sign-off belongs to whoever
holds merge authority outside the formation, unless explicitly delegated.

## Review scope contract

Before every review, freeze `IN SCOPE`: changed behavior/paths, directly
affected callers/consumers, explicit issue/spec acceptance, security or
data-loss regressions introduced or exposed by the change, and current-base
integration conflicts. Adjacent, pre-existing, or speculative findings that
do not invalidate the requested behavior become evidenced `FOLLOW-UP ISSUE`s;
they do not extend the PR or require a response/current-round fix.

This is a bounded complete review, never a minimal review. Audit every item in
the frozen surface; any failure there is mandatory. Only findings genuinely
outside that surface qualify for follow-up.

The issue/spec is the acceptance ceiling. Extra evidence is advisory unless
needed to prove a required fact or safety boundary. Before `Changes Requested`,
the reviewer completes the whole scoped audit and batches all blocking P0/P1.
A later round adds a blocker only when the fix caused it or made it previously
unobservable; otherwise it is follow-up.

Select gates proportionally. Code/product-blob, base, or toolchain changes run
the relevant code gates. A docs/evidence-only push with those inputs unchanged
reuses still-applicable code results as `READ` with source SHA, then runs only
relevant diff/docs/evidence checks and exact-head CI as `RAN`.

Verify once per frozen artifact. The implementer runs the full gate set once
per frozen full SHA — in the assigned build lane where the session builds
locally — and records a gate receipt (SHA, gate set, toolchain, lane or n/a,
result). The reviewer READs that receipt and exact-head CI, RANs only focused
or adversarial checks they do not cover, and states the reason for any full
rerun (no receipt, red or absent CI, or grounds to distrust the receipt). Know
what the project's CI genuinely lacks before citing it as independent evidence;
a gap earns a focused check of the uncovered surface, not a full rerun.
Deterministically red CI already blocks the artifact: audit and request changes
rather than spending a full run, and re-run only the failed job when the red is
environmental. Focused
gates on touched units while iterating, never the full set per edit. A status,
label, or draft flip is not a push and reruns nothing.

Each handoff records available elapsed/token/tool cost. Stop after two
consecutive non-product evidence/docs or harness-only cycles without improved
required behavior/proof, or at clear diminishing returns; route the finding to
follow-up or ask the operator to expand scope.

Over-verification is a process finding, not a product blocker: a second RAN
for an already-receipted SHA without a reason, full gates for a docs-only push,
or an ad-hoc build directory goes into the cost line, routes as a `FOLLOW-UP
ISSUE`, and corrects the next brief.

Use this request template; it is not a verdict:

```text
## Code Review Handoff: Round N
Full SHA: <full SHA>
Base full SHA: <full SHA>
IN SCOPE: <frozen blocking surface>
FOLLOW-UP ISSUE: <links or none>
Blocking counts entering review: P0=<n>, P1=<n>
Evidence:
- RAN: <commands/checks run on this SHA>
- READ: <reused results and source SHAs>
- Lane: <assigned build lane, or n/a when nothing was built locally>
- Receipt: <gate receipt for this SHA (SHA, gate set, toolchain, lane, result), or none>
Cost: elapsed=<available/unknown>, tokens=<available/unknown>, tools=<available/unknown>
Request: audit the whole scoped surface; publish no self-verdict from the implementer
```

## Protocol

1. **Decompose.** Bundle by code-chain cohesion (same chain → same worker,
   serial). Ownership per file, per SECTION where bundles share a file.
2. **Ledger first.** `edda task new "<bundle>" --assignee <label>` per
   bundle. Specs live in issues, never only in messages.
3. **Brief workers** — self-contained (they have zero context): issues to
   read, worktree + branch command, `edda claim` label and paths, files
   owned/forbidden, quality gates verbatim, verification budget (focused gates
   on touched units while iterating; the full gate set once per frozen SHA with
   a receipt; READ receipts before any RAN), cleanup authority (build cache is
   disposable and stale cache should be reclaimed by age; worktrees, branches,
   and sources are never deleted), and — only when the session compiles locally
   — an assigned build lane from the fixed pool **with its absolute lane root**
   (a worker who cannot resolve `<lane root>/<lane name>` will invent a
   directory, the exact failure the lane rule exists to prevent; a session that
   builds nothing needs no lane), done = GitHub PR when available, otherwise a
   frozen local branch plus durable review carrier (never invent a PR); never
   merge + `edda task done --receipt`. Include the receiver tie-break verbatim
   (see Traffic rules).
4. **Brief the verifier — read-only, starts BEFORE code:** baseline on the
   basis SHA by READing exact-head CI and any existing gate receipt, RANning
   only what they do not cover in the assigned verifier lane, and classifying
   existing red checks; flake hunt; observable-behavior criteria per issue;
   sweep for two test poisons — tests asserting the behavior being removed
   (invert and rename, never delete) and single-case tests that pass either
   way (demand the second case). One verifier identity per delivery
   candidate: rounds resume the same session and lane; a replacement reads
   receipts and CI before running anything.
5. **Relay loop:** verifier intel → spot-check the load-bearing claims
   yourself → adjudicate → fixate as issue comment → doorbell affected
   workers. Never fixate an unverified claim.
6. **Track without interrupting:** workers' done-bells + background poll on
   `edda task list` / PR state + read-only peeks. "Queued" means busy — fine.
7. **Close:** receipt on the rail → publish the review handoff above → your
   review + verifier's adversarial review, independently → for GitHub delivery,
   publish `Code Review: Round N` on the PR pinned to the full SHA with
   `IN SCOPE`, blocking P0/P1, `FOLLOW-UP ISSUE`, and `RAN`/`READ` evidence.
   `Changes Requested` requires the implementer's point-by-point `Review
   Response: Round N` for blocking findings, a new frozen SHA, and another
   review round. Publish final current-head LGTM with P0=0, P1=0 and exact
   required gates → the merge authority integrates. For local-only delivery,
   record the same round/response/verdict fields in the strongest durable local
   carrier; do not invent a PR. Internal reports do not replace the durable
   visible loop.

## Traffic rules (messages WILL cross)

- Rulings live in the truth layer with monotonic ids (d-001, d-002, …);
  messages carry only pointers. Numbering is the ordering.
- Changing or accepting a deviation from a prior ruling requires a
  SUPERSEDES entry in the same durable place, before any doorbell.
- Receiver tie-break (verbatim in every brief): obey the highest d-NNN in
  the ledger, not the latest message; on conflict reply with your state
  instead of executing; never discard pushed work unless the ruling names
  the exact commit.
- Reviews pin a full SHA, never a PR number; the branch freezes from
  assignment to verdict; any push voids the verdict. Report claims are
  tagged ran-vs-read — a harness count you executed is evidence, a test
  described in someone's message is not.
- Code-touching instructions carry intent + basis SHA + a drift branch
  ("if your HEAD differs, satisfy the intent and reply with your SHA").
- Write a board line at every transition: `edda note --tag fleet-board`
  ("<lane>@<sha> FROZEN review-pending | ..."). The confused read the
  board, not chat history.

## Doorbell vs registered letter

`edda request` is a registered letter to a **role**: durable, acknowledged,
survives session replacement — and structurally unable to wake an idle peer.
Host cross-session messaging is the bell: it wakes, but binds to one session
and may not exist. Both present → letter first, bell second. Terminal-only →
letters at the peers' natural cadence. Bell-only → ring, ledger as backstop.

## Common mistakes

| Mistake | Fix |
|---|---|
| All sessions implement, review at end | verifier from 2 workers up, audit before code |
| Truth only in messages | fixate to ledger/issue, then doorbell |
| Accepting a deviation without revoking the old ruling | SUPERSEDES in the same place, then bell |
| Review assigned to a PR, not a SHA | pin + freeze + void-on-push |
| Verifier accepted only in chat/report | publish the numbered SHA-pinned review round on the PR |
| Requested changes fixed without a response | implementer replies point-by-point, then reviewer opens the next round |
| Worker obeys latest message over ledger | tie-break: highest d-NNN wins, reply don't execute |
| Controller reads worker diffs mid-flight | compressed signals only until review time |
| Verifier fixes things | read-only, always |
| Treating a worker receipt as acceptance | receipts are execution evidence; sign-off lives outside the formation |
| Adjacent finding becomes another blocking round | file an evidenced follow-up issue unless it is in the frozen blocking surface |
| Docs-only push restarts full code gates | reuse applicable code gates as `READ`; run delta checks plus exact-head CI |
| Review drips one blocker per round | audit the whole scope and batch P0/P1 before requesting changes |
| Fresh verifier reruns every gate "to be safe" | READ the frozen SHA's receipt and exact-head CI; RAN only what they do not cover, or state the reason |
| New build directory per round, SHA, or timestamp | one assigned lane per session for its lifetime; lane cache is disposable, sources are not |
| Status/label/draft flip treated as a push | not a push; nothing reruns |
