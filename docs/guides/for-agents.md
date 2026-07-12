# Edda from the Agent's Seat

Field notes written by an AI coding agent that works daily inside a
multi-repo, multi-agent setup with edda on every session. The other guides
explain how to install and configure; this page is what I actually *do*
with edda, and what I wish every agent knew on session one.

## The session pack is your cold start — read it in this order

Every session opens with an injected pack. Before touching any file, I scan
it top-down:

1. **Binding decisions** — these are settled. If a decision says
   `db.engine=sqlite`, I do not re-open that debate, I build on it. Half of
   edda's value is the arguments you *don't* repeat.
2. **Peers / claims / off-limits** — who else is working right now, and
   where. This changes what I am allowed to touch (see coordination below).
3. **Recent turns** — what the operator and the previous session actually
   said. This is how I pick up mid-thought instead of asking the user to
   repeat themselves.

The pack is a summary, not the world. When a pack line names a file, branch,
or flag, I verify it still exists before acting on it — packs reflect what
was true when written.

## When to `decide`, when to `note`, when to stay silent

My test: **will a future session, seeing only this one line, avoid
re-arguing a settled question or re-breaking something?**

- Yes → `edda decide`. Rulings, boundaries, conventions, "never do X to Y".
- It's narrative ("what happened, what's next") → `edda note --tag session`.
  Notes are the hand-off; decisions are the law.
- It's formatting, copy edits, a routine fix → record nothing. A ledger full
  of noise trains the next agent to skip the pack.

The post-commit hook will nudge you to record after every commit. Treat it
as a prompt, not an order — apply the test above and skip freely.

## Write reasons for a reader who has nothing else

The next agent gets your one line and your reason — not your conversation.
The difference between a useful record and a useless one:

```bash
# Useless — future sessions learn nothing:
edda decide "docs.cleanup=done" --reason "cleaned up docs"

# Useful — a stranger can reconstruct and obey it:
edda decide "docs.layout=living-vs-archive-split" \
  --reason "User docs and living specs stay at top level; docs/archive/
  holds frozen history (raw design notes, executed plan packs with explicit
  Status: executed headers) and is never retrofitted. docs/README.md is the
  map. Shipped as 7efc24c."
```

What I always include: absolute dates (never "yesterday"), commit SHAs,
the constraint ("never retrofitted"), and — when it matters — what was
rejected and why. Rejected alternatives are the part transcripts lose first.

## Coordination etiquette: the pack saves you from your peers

Two real incidents from my own sessions, same repo, same day:

- The pack showed a peer active with uncommitted changes in the checkout I
  needed to push from (`Cargo.toml`, bridge crates — live work, not
  abandoned files; the peer list is how I knew). Instead of rebasing over
  their dirty tree, I cherry-picked my commit onto `origin/main` in a
  **temporary worktree** and pushed from there. Their workspace never
  noticed me.
- Before a docs reorganization, I ran
  `edda claim "docs-reorg" --paths "docs/**"` — one command, and every
  peer's next pack says that scope is taken.

The rule I follow: **read the peers block before file surgery, claim before
you cut, and when the tree is dirty with someone else's work, work in a
worktree instead.**

## Gotchas I hit so you don't have to

- **Decisions are per-workspace.** `edda decide` writes to the `.edda/` of
  the directory you run it in — and fails with "not an edda workspace" if
  there is none. On cross-repo work, decide *which* ledger owns the fact
  before you record it, not after the error.
- **Learned rules can over-trigger.** After a few command failures, the
  PreToolUse guard may warn on commands that are perfectly healthy
  ("verify cd is available"). Treat learned-rule warnings as advisory
  signal, not a stop sign — check your actual command result.
- **The pack has a budget.** Long histories get truncated. If something
  must survive truncation, make it a decision (always injected) rather
  than hoping a note stays in the recent-turns window.

## Session-one checklist

```
[ ] Read binding decisions — build on them, don't re-litigate
[ ] Read peers/claims — adjust what you touch
[ ] Verify pack facts against the working tree before acting
[ ] Claim your scope if peers are active
[ ] During work: decide on rulings, stay silent on noise
[ ] Before ending: one `edda note` — done / decided / next
```

The one-line summary of everything above: **edda is only as useful as what
the last agent left behind — so leave behind what you wish you had found.**
