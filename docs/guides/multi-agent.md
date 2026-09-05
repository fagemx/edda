---
title: Multi-Agent Coordination
---

# Multi-Agent Coordination

When multiple Claude Code agents work on the same repo simultaneously, they have no awareness of each other. Edda adds a lightweight coordination layer that runs entirely through local files — no server, no configuration beyond `edda init`.

## The problem

Two agents on the same repo will:

1. **Edit the same file** — one overwrites the other
2. **Do redundant work** — both solve the same problem independently
3. **Make contradictory decisions** — one chooses Postgres, the other adds SQLite migrations

These failures are silent. You only discover them after the damage is done.

## How it works

Edda adds three coordination primitives, all running through Claude Code hooks:

### 1. Peer discovery

Every session writes a heartbeat file with its current activity — what files it's editing, what tasks it's working on, and what git branch it's on.

When a new prompt is submitted, Edda checks for other active heartbeats. If peers are found, it injects their status into the agent's context:

```
## Peers (1 active)
- billing (30s ago) [branch: feat/billing]: editing src/billing/service.rs
```

The agent now knows it's not alone.

### 2. Scope claims

Agents can claim ownership of file paths:

```bash
edda claim "auth" --paths "src/auth/*"
```

Once claimed, other agents see those paths as off-limits:

```
### Off-limits (other agents active)
- src/auth/* -> Agent auth (30s ago)
```

Claims are advisory — the coordination protocol tells agents not to edit off-limits files, and in practice they follow the instruction.

### 3. Binding decisions

When one agent makes a decision that affects everyone:

```bash
edda decide "db.engine=postgres" --reason "JSONB support needed"
```

All other active sessions see this immediately:

```
### Binding Decisions
- db.engine: postgres (backend)
```

If another agent tries to decide differently on the same key, Edda detects the conflict and warns.

## What the agent sees

At session start, if peers are detected, Edda injects the full coordination protocol:

```markdown
## Coordination Protocol
You are one of 2 agents working simultaneously.
Claim your scope: `edda claim "label" --paths "src/scope/*"`
Message a peer: `edda request "peer-label" "your message"`

### Peers Working On
- billing (30s ago) [branch: feat/billing]: editing src/billing/service.rs

### Off-limits (other agents active)
- src/billing/* -> Agent billing (30s ago)

### Binding Decisions
- db.engine: postgres (backend)
```

On subsequent prompts, Edda injects lightweight peer updates so the agent stays aware without repeating the full protocol.

## Peer messaging

Agents can send requests to each other:

```bash
edda request "billing" "Please expose the invoice total as a public method"
```

The target agent sees the request at its next prompt. If no active session
answers to `billing`, the send fails rather than silently going nowhere — pass
`--force` to queue it for a peer that has not started yet.

The request keeps appearing until the receiving agent acknowledges it:

```bash
edda request-ack "auth"
```

That ack covers the messages outstanding at that moment, so a later request
from the same peer is delivered normally. Requests left unacked for 7 days
expire and are reported as dead letters.

## Coordination discipline

Edda's primitives — heartbeats, claims, requests — are carriers, and every
carrier here is unreliable by design: requests deliver at the peer's next hook
event, host cross-session messages deliver at turn boundaries, sessions die,
and the log replays whatever was never released. Running several sessions on
real work needs a discipline **on top of** the carriers. This one is distilled
from a live multi-session run on this repository (the session formation that
landed GH-442 through GH-445); every rule below exists because its absence
cost that formation a concrete incident. The `coord-orchestrate` skill
(installed by `edda init`) is the executable form of this section.

### Three layers, never mixed

| Layer | Carrier | Property |
|-------|---------|----------|
| Truth | ledger: `edda task` / `edda decide` / issue comments | survives any session's death |
| Doorbell | `edda request`, host cross-session messaging | instant-ish, may drop — never the only copy |
| Isolation | one git worktree per work bundle | conflicts impossible, not merely discouraged |

The core rule: **messages may drop; state may not.** Every load-bearing fact
travels dual-track — fixate it in the truth layer first, then ring the
doorbell. A fact that exists only in a message dies with the session that
received it.

### Formation

One **controller** (assigns, adjudicates, reviews — never reads
implementation diffs mid-flight), one **read-only verifier**, N **workers**
in separate worktrees. The verifier starts *before* any code is written:
baseline gates on main, flake hunt, per-issue review criteria, and a sweep
for two test poisons — tests asserting the exact behavior being removed
(invert and rename, never delete) and single-case tests that pass whether or
not the bug is fixed. In the source run, that pre-work audit caught a
parallel-execution flake workers would have blamed themselves for, three
false-green tests, and a planned fix that would have silently killed request
delivery.

### Normative traffic rules

Messages cross during any busy stretch. Don't try to prevent it — make it
harmless by pinning everything normative:

- **Nothing normative travels in a message.** Rulings live in the truth layer
  with monotonic ids (`d-001, d-002, …`); a message is only a pointer
  ("d-007 posted on #NN"). Numbering is the ordering, so arrival order stops
  mattering.
- **Supersession duty.** Changing or accepting a deviation from a prior
  ruling requires a `SUPERSEDES d-NNN` entry in the same durable place the
  old ruling lives, before any doorbell. An acceptance that exists only in a
  PR comment or a message is not in force.
- **Receiver tie-break.** Obey the highest `d-NNN` in the truth layer, not
  the latest message; on conflict, reply with your current state instead of
  executing. Never discard pushed work unless the ruling names the exact
  commit being discarded.
- **Reviews pin a SHA, never a PR number.** The branch freezes from
  assignment to verdict; any push voids the verdict. Claims in reports are
  tagged ran-vs-read — a test count from the harness you executed is
  evidence; a test described in someone's message is not.
- **Code-touching instructions carry intent + basis SHA + a drift branch**
  ("if your HEAD differs, satisfy the intent and reply with your SHA").
  Transit delay makes "current state" language false by construction.
- **Coordination board.** The controller writes one state line at every
  transition (per lane: SHA, frozen?, review status) — e.g.
  `edda note --tag fleet-board`. Whoever wakes up confused reads the board,
  not the chat history.

### What is edda's and what is not

The discipline is carrier-neutral. The truth layer can be edda or plain
issue comments; the doorbell can be `edda request` or the host's
cross-session messaging; the rules survive substituting either. What edda
adds over a bare issue tracker: local zero-network coordination, hash-chained
auditability, task receipts that unlock successors (`edda task done
--receipt`), and cross-repo group queries.

One boundary worth stating: a worker's `--receipt` is **execution evidence**
— proof of what was done and how it was verified. It is not acceptance. The
formation (controller and verifier included) produces delivery candidates
and evidence; sign-off belongs to whoever holds merge authority outside it,
unless that authority was explicitly delegated.

## Monitoring

```bash
edda watch
```

Opens a terminal UI showing all active sessions, their activity, and coordination state in real time:

```
┌─ Sessions ──────────────────────┐┌─ Events ────────────────────────┐
│ auth (2s ago) [main]            ││ cmd:fail $ cargo test [exit:2]  │
│   editing: src/auth/jwt.rs      ││ note    session checkpoint      │
│   task: Add JWT middleware      ││ decide  db.engine=postgres      │
│                                 ││                                 │
│ billing (5s ago) [feat/billing] ││                                 │
│   editing: src/billing/api.rs   ││                                 │
│   task: Implement invoice API   ││                                 │
└─────────────────────────────────┘└─────────────────────────────────┘
```

## Storage

The coordination layer runs entirely on local files:

- **Heartbeats**: JSON files in the per-user store, one per session
- **Claims and bindings**: append-only `coordination.jsonl` log
- **Peer discovery**: filesystem scan at hook time

No central server, no network communication. Two Claude Code sessions on the same repo coordinate automatically.

## Cross-machine decision mirror (GH-671)

Everything above coordinates sessions on one machine. Decisions recorded in a
ledger are machine-local by design (`#613` §2: `.edda/` is gitignored), so
`edda ask` on machine B cannot see a ruling made on machine A. The binding
ruling is:

`ledger.cross-machine-projection=committed-mirror-stamped-at-wave-close-quote-never-paraphrase`

The cross-machine projection of decisions is a **git-committed mirror**, not a
network protocol:

- **Write (source machine).** `scripts/fleet/ledger-sync.sh` runs
  `edda export md --out docs/ledger` and commits only that directory
  (path-limited commit — unrelated WIP is never swept). Trigger choice:
  `fleet.ledger-sync-trigger=scheduled-on-4090` — Windows Task Scheduler runs
  the script periodically on the 4090; it is behavior-tested with a stubbed
  `edda` against throwaway repos (`scripts/fleet/test-ledger-sync.sh`), never
  against the production ledger, and the script itself never registers a
  production Scheduled Task.
- **Read (target machine).** After pulling, `edda sync --from-mirror
  docs/ledger` imports the mirror into the local ledger. Same rule as sqlite
  sync (#394): same key with a different value imports **inactive** — merge,
  never overwrite. A decision whose original event already exists locally is
  skipped, so a machine importing its own mirror is a no-op. Ratified
  decisions arrive ratified: the mirror's ratification is replayed as an
  append-only `decision_ratify` event.
- **Values are quoted, never paraphrased.** The mirror carries the verbatim
  value and reason of every decision; the import must never mint a value from
  an INDEX gloss (INDEX.md carries counts and freshness only). The
  `fleet.lane-profile` acceptance is the worked example: the verbatim value
  `agent-actor-is-the-profile` with its six-point reason must survive the
  round trip — the design-doc gloss `actor-is-profile` must not win.
- **Freshness (death visibility).** `INDEX.md` is stamped on every export
  with `- **Exported at**:` (RFC 3339) and `- **Exporting machine**:`. If the
  stamp is older than **24 hours** (default,
  `edda-ledger::sync::DEFAULT_MIRROR_STALE_HOURS`) — or unreadable — `edda
  sync --from-mirror` prints a visible `⚠ STALE MIRROR` line naming the
  threshold, stamp and machine before importing. Unknown freshness is treated
  as stale, never silently fresh.
- **Doorbell boundary.** The mirror is truth-layer replication: it rides git
  and delivers whenever the clone pulls, with no resident process. There is
  deliberately **no cross-platform doorbell** in this issue — live push over
  Tailscale (`edda node` / `edda inbox`) is #685, as is session identity
  (`label@machine`, collision warning). The mirror carries the *decisions*;
  it does not implement them.

### Auditing a doneWhen that says 「決策 X 已在帳本」(`audit.ledger-donewhen`)

This is the named home of the audit rule. When an issue's doneWhen claims a
decision of the form 「決策 X 已在帳本」("decision X is in the ledger"), audit it
by **citing the decision key and the machine**:

1. Name the exact key (e.g. `fleet.merge-authority`) and the machine whose
   ledger is claimed to hold it (e.g. `4090`).
2. Check the committed mirror of that machine: look up the key under
   `docs/ledger/decisions/<domain>.md` in a checkout that carries machine's
   mirror push, or run `edda ask <key>` on the machine itself. The INDEX
   stamp tells you how fresh the evidence is (see the 24h threshold above).
3. **not-found is ledger locality, not an absent ruling.** If the key is not
   in *your* ledger or mirror, that means the ruling lives on another
   machine's ledger, or your mirror checkout is stale — re-export or
   `edda sync --from-mirror docs/ledger` before concluding anything. It does
   **not** mean the decision was never made. Only after the machine named in
   the claim provably lacks the key (fresh mirror, key absent) is the claim
   refuted.

## Typical workflow

```bash
# Terminal 1
claude   # "Refactor the auth module"

# Terminal 2
claude   # "Add the billing API"
```

Each agent will:
1. See the other's activity at each prompt
2. Respect claimed scopes
3. Share binding decisions
4. Send requests when coordination is needed

## Limitations

- **Advisory enforcement** — off-limits paths are communicated but not hard-blocked
- **Same machine only** — peer discovery uses local filesystem
- **Bash bypass** — scope claims apply to Edit/Write tools; `sed` and `mv` in Bash are not checked
- **Stale heartbeats** — heartbeats older than 120 seconds are considered inactive
- **Decision mirrors** — the committed mirror under `docs/ledger/` is generated
  by `edda export md` (see the section above); hand-edits to it are lost on the
  next export, and the SQLite ledger stays the single source of truth
