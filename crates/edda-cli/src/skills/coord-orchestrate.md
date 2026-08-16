---
name: coord-orchestrate
description: Use when coordinating two or more sessions on parallel implementation work as the controller — assigning bundles, briefing workers and a verifier, adjudicating mid-flight, and closing with dual review
---

# Coordination Orchestrate

You are the controller of a multi-session formation. The other coord skills
teach a session to be a good peer (coord-sync/request/handoff/review); this
one teaches the seat that runs the whole formation. The companion prose is
the "Coordination discipline" section of edda's multi-agent guide.

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

## Protocol

1. **Decompose.** Bundle by code-chain cohesion (same chain → same worker,
   serial). Ownership per file, per SECTION where bundles share a file.
2. **Ledger first.** `edda task new "<bundle>" --assignee <label>` per
   bundle. Specs live in issues, never only in messages.
3. **Brief workers** — self-contained (they have zero context): issues to
   read, worktree + branch command, `edda claim` label and paths, files
   owned/forbidden, quality gates verbatim, done = GitHub PR when available,
   otherwise a frozen local branch plus durable review carrier (never invent a
   PR); never merge + `edda task done --receipt`. Include the receiver tie-break
   verbatim (see Traffic rules).
4. **Brief the verifier — read-only, starts BEFORE code:** baseline gates on
   main; flake hunt; observable-behavior criteria per issue; sweep for two
   test poisons — tests asserting the behavior being removed (invert and
   rename, never delete) and single-case tests that pass either way (demand
   the second case).
5. **Relay loop:** verifier intel → spot-check the load-bearing claims
   yourself → adjudicate → fixate as issue comment → doorbell affected
   workers. Never fixate an unverified claim.
6. **Track without interrupting:** workers' done-bells + background poll on
   `edda task list` / PR state + read-only peeks. "Queued" means busy — fine.
7. **Close:** receipt on the rail → your review + verifier's adversarial
   review, independently → for GitHub delivery, publish `Code Review: Round N`
   on the PR pinned to the full SHA. `Changes Requested` requires the
   implementer's point-by-point `Review Response: Round N`, a new frozen SHA,
   and another review round. Publish final current-head LGTM with P0=0, P1=0
   and ran gates → the merge authority integrates. For local-only delivery,
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
