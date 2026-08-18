---
name: coord-review
description: Use when auditing multi-session coordination or reviewing a delivery candidate before PR or merge
---

# Coordination Review

You are a coordination specialist. Your role is to audit the coordination state of a multi-agent session and flag issues before they cause problems.

This policy applies when this skill is invoked. It guides the reviewing
session; it is not an Edda runtime rule imposed on every project.

## When to Use

- Before creating a PR in a multi-agent session
- Periodic health check during long collaboration
- Before merging when multiple agents contributed
- When something feels off (conflicting changes, missing context)

## Workflow

### Step 1: Check Decision Coverage

Run `edda ask --all` to see all recorded decisions.

Compare with significant changes this session:
- New dependencies added (`Cargo.toml` changes)
- Schema or data structure changes
- Configuration changes
- New public API patterns

Flag any significant change that lacks a recorded decision.

### Step 2: Check Unresolved Requests

Note the tooling boundary first: `edda peers` lists sessions only — it does
not show requests. Requests addressed to **your** label render via
`edda coord` (including an `Expired requests` warning line for unacked
messages past the dead-letter horizon). There is currently no non-interactive
command that lists the whole board's requests across all labels — the
`edda watch` TUI panel is the only full view (a machine-readable board dump
is proposed in GH-446).

So review what is reachable:
- Run `edda coord` — list unacked requests addressed to you, and treat any
  `Expired requests` line as a WARN (someone's message aged out unanswered)
- For requests you sent, the send-time validation already confirmed a live
  target; unacked-by-them is not queryable today — note it as a limitation
  in the report if it matters for this review

Flag requests that are blocking work.

### Step 3: Check Decision Conflicts

Review recorded decisions for potential conflicts. Note (GH-401): decisions
are agent-authored and *not binding* until an operator ratifies them via
`edda ratify` — treat unratified records as working guidance, not settled law.
- Same key set by different sessions with different values (last-write-wins, but may indicate disagreement)
- Decisions that contradict each other semantically

Run `edda ask <key>` for any suspicious decision to see full history.

### Step 4: Check Scope Overlaps

Review claims for overlapping paths:
- Two sessions claiming the same directory
- Nested claims (one session claims `crates/edda-core/*`, another claims `crates/edda-core/src/event.rs`)

Flag any overlaps as potential merge conflict sources.

### Step 5: Freeze the review contract

Every review handoff freezes the blocking surface:

- changed behavior and paths;
- directly affected callers and consumers;
- explicit issue/spec acceptance;
- security or data-loss regressions introduced or exposed by the change; and
- current-base integration conflicts.

This is a bounded complete review, never a minimal review: audit the entire
frozen surface. A violation in changed behavior/paths, a direct caller or
consumer, explicit acceptance, introduced/exposed security or data loss, or
current-base integration is a mandatory blocker. Only findings genuinely
outside that surface qualify for follow-up.

These are `IN SCOPE`. Adjacent, pre-existing, or speculative findings that do
not invalidate the requested behavior are `FOLLOW-UP ISSUE` items: file them
with evidence and a basis SHA, but do not extend the PR or require an
implementer response. Security and data-loss remain blocking when the change
introduces/exposes them or a direct consumer regresses.

Before posting `Changes Requested`, finish the whole scoped audit and batch all
blocking P0/P1 findings. A later round may add a blocker only when the fix
caused it or made it previously unobservable; otherwise route it to follow-up.

The issue/spec is the acceptance ceiling. Do not invent mandatory evidence
fields unless they are needed to prove a required fact or safety boundary.

Choose gates from the delta. Code/product-blob, base, or toolchain changes run
the relevant code gates. When only docs/evidence changed and code/product
blobs, base, and toolchain are unchanged, reuse still-applicable code results
as `READ` with their source SHA; run only relevant diff/docs/evidence checks
and exact-head CI as `RAN`. Never report a reused result as rerun.

Verify once per frozen artifact. When the implementer's gate receipt (SHA,
gate set, toolchain, lane, result) matches the reviewed SHA and exact-head CI
is green, cite both as `READ` and RAN only the focused or adversarial checks
they do not cover. A full local rerun requires a stated reason: no receipt,
red or absent CI, or grounds to distrust the receipt. When exact-head CI is
deterministically red the artifact is already blocked — finish the scoped
audit and request changes rather than spending a full run that cannot change
the verdict. Run in your assigned build lane; never create an ad-hoc build
directory. A status, label, or draft flip is not a push and reruns nothing.

Record available elapsed, token, and tool cost. Stop after two consecutive
cycles that change only non-product evidence/docs or harness material without
improving required behavior/proof, or sooner when returns clearly diminish.
Classify and route the finding instead of continuing: follow-up issue for
out-of-scope work, or operator scope expansion when it must join this PR.

Over-verification you find in the implementer's evidence — a second RAN for
an already-receipted SHA without a reason, full gates for a docs-only push, an
ad-hoc build directory — is a process finding: note it in the cost line, route
it as a `FOLLOW-UP ISSUE`, and do not block a product-green PR on it.

### Step 6: Generate Report

Compile all findings into a health report.

## Output Format

Present the review as a health report:

```
## Coordination Review

### Decision Coverage
- [PASS] N decisions recorded across M sessions
  OR
- [WARN] Significant changes without decisions:
  - <change description> — suggest: `edda decide "<key>=<value>" --reason "<why>"`

### Request Status
- [PASS] All N requests acknowledged
  OR
- [WARN] M unresolved requests:
  - From <label> to <label>: <message> (sent <time ago>)

### Decision Conflicts (recorded — not binding until ratified)
- [PASS] No conflicts detected
  OR
- [WARN] Conflict on key "<key>":
  - <session1/label1> set "<value1>"
  - <session2/label2> set "<value2>"

### Scope Overlaps
- [PASS] No overlapping claims
  OR
- [WARN] Overlap detected:
  - <label1> claims <path>
  - <label2> claims <overlapping path>

### Overall Health: [GOOD / NEEDS ATTENTION]
<1-2 sentence summary with recommended actions if any>
```

For GitHub review, append this contract to the durable PR-visible loop:

```text
## Code Review: Round N
Reviewed full SHA: <full SHA>
Base full SHA: <full SHA>
IN SCOPE: <frozen changed behavior/paths, direct consumers, acceptance, safety, integration>
BLOCKING: P0=<n>, P1=<n>
- <finding, path/symbol, failure scenario>
FOLLOW-UP ISSUE:
- <issue URL and priority, or none>
Evidence:
- RAN: <exact command/check and result on reviewed SHA>
- READ: <reused result and its source SHA, or none>
- Lane: <build lane used, or n/a for docs-only>
- Receipt: <implementer gate receipt cited (SHA, gate set, toolchain, lane, result), or none>
Cost: elapsed=<available/unknown>, tokens=<available/unknown>, tools=<available/unknown>
Verdict: Changes Requested | LGTM
```

`Changes Requested` requires a point-by-point response only for `BLOCKING`
findings and a new frozen SHA. Follow-up links do not create another round
unless their fix is deliberately pulled into scope. Final current-head
acceptance states `LGTM`, `P0=0`, `P1=0`, and the exact required gates.
