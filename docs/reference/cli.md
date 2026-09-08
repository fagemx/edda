---
title: CLI Reference
---

# CLI Reference

Complete reference for all `edda` commands.

> Documented for edda 0.5 — the surface below is re-derived from the built binary by
> binary, not copied from an older release. `scripts/check-cli-docs.sh`
> enforces that every verb the binary exposes is documented here, either as a
> full section or as a row in the [Internal / experimental](#internal--experimental-commands)
> table.

## Getting started

### `edda init`

Initialize a new `.edda/` workspace in the current directory.

```bash
edda init [--no-hooks]
```

| Option | Description |
|--------|-------------|
| `--no-hooks` | Skip auto-detection and installation of bridge hooks |

Creates `.edda/` with an empty ledger. If `.claude/` is detected, automatically installs Claude Code hooks and adds decision-tracking instructions to `CLAUDE.md`.

### `edda status`

Show workspace status — head branch, last commit, and how many events sit on
the branch since it.

```bash
edda status
edda status --json
```

`--json` emits one object with `branch`, `last_commit` (null until the branch
has one) and `uncommitted_events`. It is a stable contract: within 0.x keys
may be added, never deleted, renamed, or retyped. The exact shape is in
COMPATIBILITY.md § "Stable `--json` contracts".

### `edda --version`

Show installed CLI identity, including git commit and build date.

```bash
edda --version
```

Expected format:

```text
edda <semver> (<12-hex sha>[-dirty] <YYYY-MM-DD>)
```

The date is UTC. `-dirty` denotes tracked worktree or index changes present
when the binary was built; untracked files do not change the build identity.

In a build without git metadata (for example, from a source distribution), it prints:

```text
edda <semver> (unknown)
```

### `edda doctor`

Health check for bridge integration.

```bash
edda doctor claude     # check Claude Code hooks
edda doctor cursor     # check Cursor native hooks
edda doctor codex      # check Codex hooks
edda doctor openclaw   # check OpenClaw hooks
```

---

## Memory & querying

### `edda ask`

Query past decisions, history, and conversations.

```bash
edda ask [QUERY] [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `QUERY` | Keyword, domain, or exact key (e.g. `"db.engine"`) |
| `--limit N` | Max results per section (default: 20) |
| `--json` | Output as JSON |
| `--all` | Include superseded decisions |
| `--branch NAME` | Filter by branch |

```bash
edda ask "cache"             # keyword search
edda ask "db.engine"         # exact key lookup
edda ask                     # all active decisions
edda ask --all "auth"        # include superseded
```

### `edda context`

Output the context snapshot — what the agent sees at session start.

```bash
edda context [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--branch NAME` | Branch name (defaults to HEAD) |
| `--depth N` | Number of recent commits/signals to show (default: 5) |

### `edda log`

Query events from the ledger with filters.

```bash
edda log [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--type TYPE` | Filter by event type: `note`, `cmd`, `commit`, `merge`, etc. |
| `--family FAMILY` | Filter by family: `signal`, `milestone`, `admin`, `governance` |
| `--tag TAG` | Filter by tag (matches `payload.tags` array) |
| `--keyword TEXT` | Case-insensitive payload text search |
| `--after DATE` | Events after this date (ISO 8601, e.g. `2026-02-13`) |
| `--before DATE` | Events before this date |
| `--branch NAME` | Filter by branch |
| `--limit N` | Max events to show (default: 50, `0` = unlimited) |
| `--json` | Output as JSON lines |

```bash
edda log                           # recent events
edda log --tag decision            # decisions only
edda log --type cmd                # command events
edda log --after 2026-02-20        # events this week
edda log --keyword "auth" --json   # search + JSON output
```

### `edda search`

Full-text search across transcripts and events (powered by Tantivy).

```bash
edda search index          # build/update search index
edda search query "auth"   # search for text
edda search show TURN_ID   # show full turn content
```

---

## Recording

### `edda note`

Record a note event.

```bash
edda note <TEXT> [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--role ROLE` | `user`, `assistant`, or `system` (default: `user`) |
| `--tag TAG` | Tags for the note (repeatable) |

```bash
edda note "completed auth refactor; next: rate limiting" --tag session
edda note "switching to Redis for pub/sub support" --tag decision
```

### `edda decide`

Record a binding decision. Writes to both the workspace ledger and the coordination layer.

```bash
edda decide <DECISION> [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `DECISION` | Key=value format (e.g. `"db.engine=postgres"`) |
| `--reason TEXT` | Reason for the decision |
| `--cite CITATION` | Authority this decision rests on, repeatable: `operator:<when>`, `issue:#<n>`, or `decision:<key>`. Read by `edda ratify --by-rule` (GH-761). Anything else exits 2 |
| `--session ID` | Explicit session attribution; otherwise uses process-carried `EDDA_SESSION_ID` |

```bash
edda decide "db.engine=sqlite" --reason "embedded, zero-config"
edda decide "auth.strategy=JWT" --reason "stateless, scales horizontally"
edda decide "review.engine=opus" --reason "window day 0" --cite issue:#888
```

A decision written without `--cite` still works everywhere; the ratify rule
falls back to reading the `--reason` text for an issue number, a binding
decision key, or the word "operator".

### `edda ratify`

Ratify a decision — confer authority (GH-401). An agent-authored decision from
`edda decide` is unratified; ratification is what makes it binding.

Three forms, mutually exclusive. Each writes a different, permanently
distinguishable `ratified_by` prefix, so `edda log` and `edda ask` always say
which authority conferred binding status:

| Form | `ratified_by` | Who is asserting |
|------|---------------|------------------|
| `edda ratify <KEY> [--by WHO]` | `<WHO>` or the session label | a person |
| `edda ratify <KEY> --evidence pr#<N>@<sha>` | `evidence:pr#N@sha` | a merged PR (GH-764) |
| `edda ratify --by-rule <RULE>` | `rule:<RULE>` | a rule in the binary (GH-761) |

`edda log --type decision_ratify` prints `<key> by <ratified_by>` in its detail
column, so the three forms are told apart there without `--json`.

```bash
edda ratify [OPTIONS] <KEY>
edda ratify --by-rule <RULE> [--dry-run] [OPTIONS]
```

| Argument / Option | Description |
|-------------------|-------------|
| `KEY` | Decision key to ratify (e.g. `"db.engine"`). Omit it with `--by-rule` |
| `--note TEXT` | Optional note recorded with the ratification |
| `--by TEXT` | Who ratified — recorded for audit; self-asserted, not verified (identity enforcement is a policy-layer concern). Defaults to the resolved session label |
| `--evidence pr#N@SHA` | The merged PR that made this decision binding. The SHA is a full 40-hex commit; an abbreviated one is refused |
| `--by-rule RULE` | Sweep every active, unratified decision with a named rule. Today: `cited-authority` |
| `--dry-run` | With `--by-rule`: print the table and write nothing |
| `--session ID` | Session ID (uses `EDDA_SESSION_ID`; `--session` required when identity is ambiguous) |

Exit codes (`claim-check.exit-codes=0/1/2`): **0** success, including the no-op
when the key is already binding; **1** no active decision for that key; **2**
malformed input — a bad evidence string, an unknown rule, or a key given
together with `--by-rule`.

```bash
edda ratify "demo.engine" --note "confirmed after load test" --by operator
edda ratify "demo.engine" --evidence "pr#764@03c604ffea4b2a1731b7866e7f701374eb03b156"
edda ratify --by-rule cited-authority --dry-run
```

Output:

```
Ratified 'demo.engine' (by operator) — now binding.
  note: confirmed after load test
```

#### Rule `cited-authority`

Ratifies an active, unratified decision when it names the authority it rests
on — a `--cite` value, or failing that an issue number, a binding decision key,
or the word "operator" in its reason. It holds:

- keys under `product.`, `commercial.` and `spend.` — these bind money or
  product promises, and a cited issue is not authority for either;
- anything a later decision's reason names, which has already been overtaken;
- anything with no citation at all.

`--dry-run` prints the same table it would act on, so the sweep can be read
before it runs:

```
key                action  why
db.engine          ratify  issue:#742
product.tier       hold    held domain 'product.' — operator ratifies these
review.engine      hold    superseded — a later decision 'review.pool' names it
cache.ttl          hold    no citation — add --cite operator:<when> | issue:#<n> | decision:<key>
dry run — would ratify 1, hold 3 (rule: cited-authority); nothing written.
```

Ratification is per decision event, not per key: re-deciding a key resets it to
unratified, so a sweep run after a re-decide judges the new value on its own
merits.

### `edda checkpoint`

Record a vendor-neutral reasoning checkpoint — current hypotheses, rejected
hypotheses with reasons, open questions, and the next action. Use it to make
an investigation resumable by any agent, not only the one that wrote it.

```bash
edda checkpoint [OPTIONS] --next <NEXT>
```

| Option | Description |
|--------|-------------|
| `--next TEXT` | Next checkpoint action (required) |
| `--hypothesis TEXT` | Current hypotheses (repeatable) |
| `--rejected HYPOTHESIS\|REASON` | Rejected hypothesis and reason, separated by `\|` (repeatable) |
| `--open TEXT` | Open questions (repeatable) |
| `--role ROLE` | Author role (default: `agent`) |

```bash
edda checkpoint \
  --hypothesis "lock contention on the ledger" \
  --rejected "sqlite busy_timeout|already configured" \
  --open "does fs2 lock survive fork?" \
  --next "profile the write path under 4 workers"
```

Output (the event id differs per run):

```
Wrote CHECKPOINT evt_01m1h59pn7nn115e3pqvz9cpc8
```

### `edda commit`

Create a commit event in the ledger.

```bash
edda commit --title "Add JWT middleware" [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--title TEXT` | Commit title (required) |
| `--purpose TEXT` | Purpose of this commit |
| `--contrib TEXT` | Contribution description (defaults to title) |
| `--evidence REF` | Evidence refs: `evt_*` or `blob:sha256:*` (repeatable) |
| `--label LABEL` | Labels (repeatable) |
| `--auto` | Enable auto-evidence collection |
| `--dry-run` | Preview without writing to ledger |

### `edda run`

Run a command and record its output in the ledger.

```bash
edda run -- cargo test
edda run -- npm run build
```

---

## Coordination (multi-agent)

Hook integrations carry their session ID into shell commands as
`EDDA_SESSION_ID`; an explicit `--session` overrides it. Without either, the
CLI uses a deterministic `cli-<label>` identity only when no live session is
present. Beside one or more live sessions it refuses before mutating state and
asks for `--session`, because a heartbeat cannot prove which process owns it.
These IDs provide attribution, not authentication or authorization.

### `edda claim`

Claim a scope for coordination. Other agents see claimed paths as off-limits.

```bash
edda claim <LABEL> [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `LABEL` | Short label (e.g. `"auth"`, `"billing"`) |
| `--paths PATTERN` | One file path pattern; repeat for multiple patterns |
| `--session ID` | Explicit session attribution; otherwise uses process-carried `EDDA_SESSION_ID` |

```bash
edda claim "auth" --paths "src/auth/*"
edda claim "billing" --paths "src/billing/*" --paths "src/invoice/*"
```

A session holds one active claim. Running `edda claim` again from the same
session replaces its previous label and complete path list; it does not add a
second claim. Pass each path pattern with its own `--paths` flag; comma-separated
values are not split.

A claim keeps refusing writers while **either** of two things is true, so a
`cli-*` claim can outlive its own stale heartbeat:

| A claim stands while | Window |
|---|---|
| its session has a fresh heartbeat | 120 s (`EDDA_PEER_STALE_SECS`), or 15× that for a sub-agent whose heartbeat records a parent — no hook events fire during a sub-agent's run, so a heartbeat written once at spawn would otherwise age out mid-run. A running session keeps refreshing it, so it is protected for as long as it runs. |
| **or** it is a bare-CLI claim (`cli-*`) young enough by its own timestamp | 24 h (`EDDA_CLAIM_TTL_SECS`). Nothing refreshes a heartbeat for a one-shot process, so the claim's own age is what is judged. |

Neither true, and the claim stops refusing. That has an edge worth knowing:
a claim from a session that is **not** `cli-*` and has never written a
heartbeat refuses nobody, from the moment it is recorded — the board entry
alone is not evidence such a session exists (GH-617).

The second window is deliberately not the first. A heartbeat is refreshed every
30 seconds; a claim is written once, so measuring it against a
refresh-calibrated window would open a surface two minutes after its owner
claimed it. Before the bare-CLI case was bounded at all, claims from
months-gone sessions refused every new lane and no `unclaim` could reach them
(GH-1018).

`edda claim check` and the `edda dispatch --owns` admission guard read one
rule, so they cannot answer differently about the same board.

### `edda request`

Send a request to another active session.

```bash
edda request <TO> <MESSAGE> [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `TO` | Target session label |
| `MESSAGE` | Request message |
| `--session ID` | Explicit session attribution; otherwise uses process-carried `EDDA_SESSION_ID` |
| `--force` | Send even when no active session answers to `TO` |

```bash
edda request "billing" "Please expose invoice total as a public method"
```

`TO` is resolved against active sessions before the request is recorded. A
label nobody answers to is an error — usually a typo — and `--force` queues it
anyway for a peer that has not started yet. A label held by more than one
session is a warning: all of them will see the message.

Unacked requests expire after 7 days (`EDDA_REQUEST_TTL_SECS`), after which
they are reported as expired and dropped by the next `edda gc`.

### `edda request-ack`

Acknowledge requests from a peer.

```bash
edda request-ack <FROM>
```

Rendering a request into an agent's context is delivery, not acknowledgement:
the request keeps appearing until it is acked here, and the ack covers only the
messages outstanding at that moment — later ones from the same peer still
arrive.

### `edda peers`

Show active peer sessions — the read-only view of who else is live, what
each session has claimed, and which requests are outstanding. Shortcut for
`bridge claude peers`.

```bash
edda peers [--json]
```

| Option | Description |
|--------|-------------|
| `--json` | Output sessions, claims, requests, and acknowledgements as JSON |

```bash
edda peers
```

Output (empty workspace):

```
No active sessions.
```

### `edda watch`

Launch the real-time TUI showing active sessions, events, and coordination state.

```bash
edda watch
```

### `edda reconcile`

Recover unfinished Task Rail attempts and dispatch ready work. The ledger,
claims, leases, attempts, and receipts remain authoritative; reconciliation is
safe to invoke repeatedly.

```bash
edda reconcile
edda reconcile --install-scheduler
edda reconcile --uninstall-scheduler
```

| Option | Description |
|--------|-------------|
| `--max-workers N` | Maximum concurrent workers (default: 3) |
| `--max-attempts N` | Retry cap per task (default: 3) |
| `--lease-ttl-s N` | Runner lease lifetime in seconds (default: 300) |
| `--codex-bin PATH` | Codex executable used for runners |
| `--install-scheduler` | On Windows, create or replace this project's one-minute scheduler task |
| `--uninstall-scheduler` | On Windows, remove this project's exact scheduler task if present |

The lifecycle flags are explicit machine mutations and never run from
`edda init` or ordinary reconciliation. They are mutually exclusive and return
without dispatching workers. Windows registration calls `schtasks.exe`
directly under the current user with `LIMITED`; it does not use `SYSTEM`,
`HIGHEST`, a password, shell wrapper, or background daemon.

Each project uses the exact name `Edda-Reconcile-<32-lowercase-hex-project-id>`.
Install uses `/F`, so repeating it replaces only that name. Uninstall queries
and deletes only that exact name and is idempotent; it never terminates a
running reconciler or worker. The compact scheduled command contains canonical
Edda and manifest paths; the repository path lives in the validated manifest,
so linked worktrees share one task and execution does not depend on the
scheduler's working directory.

Scheduler lifecycle is Windows-only. A local exact-name missing-task HRESULT
lifecycle run reported the expected signed and hexadecimal result codes, but
its raw command, output, and Query XML artifacts were not preserved. D1 stopped
`RED / BLOCKED` on a scheduler re-entry defect, and D2–D8 were not run. The
incomplete evidence and rerun requirements are tracked in
[`P2_DRILL_2026-08-16.md`](../plan/task-rail/P2_DRILL_2026-08-16.md); neither the
lifecycle nor the controller-loss drills are release-accepted.

---

## Branches & drafts

### `edda branch`

Branch operations.

```bash
edda branch create <NAME>
```

### `edda switch`

Switch to another branch.

```bash
edda switch <NAME>
```

### `edda merge`

Merge a source branch into a destination branch.

```bash
edda merge <SRC> <DST> --reason "feature complete"
```

### `edda draft`

Draft commit operations — propose changes for review before writing to ledger.

```bash
edda draft propose --title "Add caching layer" [OPTIONS]
edda draft list
edda draft show <DRAFT_ID>
edda draft apply <DRAFT_ID>
edda draft approve <DRAFT_ID>
edda draft reject <DRAFT_ID>
edda draft delete <DRAFT_ID>
edda draft inbox              # show pending approval items
```

---

## Integration

### `edda bridge`

Install or uninstall bridge hooks.

```bash
edda bridge claude install      # install Claude Code hooks
edda bridge claude uninstall    # remove hooks
edda bridge cursor install      # install native Cursor hooks
edda bridge cursor uninstall
edda bridge codex install       # install Codex hooks
edda bridge codex uninstall
edda bridge openclaw install    # install OpenClaw plugin
edda bridge openclaw uninstall
```

### `edda mcp`

Start MCP server (stdio transport, JSON-RPC 2.0).

```bash
edda mcp serve
```

Exposes 7 tools: `edda_status`, `edda_note`, `edda_decide`, `edda_ask`, `edda_log`, `edda_context`, `edda_draft_inbox`.

---

## Maintenance

### `edda config`

Read or write workspace config (`.edda/config.json`).

```bash
edda config list
edda config get <KEY>
edda config set <KEY> <VALUE>
```

### `edda pattern`

Manage classification patterns (`.edda/patterns/`).

```bash
edda pattern add <NAME> --glob "*.test.ts" --class test
edda pattern remove <NAME>
edda pattern list
edda pattern test <FILE_PATH>
```

### `edda rebuild`

Rebuild derived views from the ledger.

```bash
edda rebuild                  # rebuild HEAD branch
edda rebuild --all            # rebuild all branches
edda rebuild --branch main
```

### `edda gc`

Garbage collect expired blobs and transcripts.

```bash
edda gc                          # interactive
edda gc --dry-run                # preview only
edda gc --force                  # skip confirmation
edda gc --keep-days 30           # override retention
edda gc --global                 # also clean global transcript store
edda gc --include-sessions       # also clean session ledgers and stale files
edda gc --archive                # archive instead of delete
edda gc --purge-archive          # purge expired archived blobs
```

### `edda blob`

Manage blob metadata.

```bash
edda blob info <HASH>
edda blob stats
edda blob classify <HASH> --class artifact
edda blob pin <HASH>
edda blob unpin <HASH>
edda blob tombstones
```

### `edda index`

Index operations.

```bash
edda index verify    # verify index entries match store records
```

### `edda verify`

Verify the ledger hash chain — the tamper-evidence check over all events in
`.edda/ledger.db` (parent linkage + canonical hashes). Read-only: the command
never creates, migrates, or writes to the ledger — a missing or unreadable
`.edda/ledger.db` is reported, never silently rebuilt as an empty one.

```bash
edda verify          # human-readable one-line report
edda verify --json   # {"ok": ..., "events": ..., "first_bad_event": ...}
```

Exit codes (same convention as `edda claim check`):

- `0` — chain intact (an empty ledger is OK, not an error)
- `1` — chain broken; the report names the first broken event (including a
  row whose payload is no longer valid JSON — the unreadable row is named)
- `2` — the ledger could not be opened or read (not an edda workspace, or
  `.edda/ledger.db` missing/unreadable)

Example output on a tampered ledger:

```
ledger chain BROKEN at event evt_01J… (3 event(s) scanned): event evt_01J… has invalid hash or digest
```

---

## Task rail, dispatch & gates

### `edda task`

Task rail — create, hand off, and track tasks on the ledger. Agent verbs
(`new`, `start`, `done`, `fail`) mutate tasks; user verbs (`list`, `show`)
are read-only.

```bash
edda task new <TITLE> [OPTIONS]          # create a task (agent verb)
edda task start <ID> [--lease-ttl S]     # take the lease, mark running (agent verb)
edda task done <ID> --receipt TEXT       # complete: done + receipt; successors become ready
edda task fail <ID> --reason TEXT        # mark failed (agent verb)
edda task list [--status S] [--assignee L] [--json] [--fleet]
edda task show <ID> [--json]
```

`edda task new` options:

| Option | Description |
|--------|-------------|
| `--assignee LABEL` | Agent label this task is assigned to (e.g. `worker-1`) |
| `--agent KIND` | Agent transport kind (e.g. `claude-acp`, `codex-acp`) |
| `--after ID` | Task id that must be done first (repeatable — dependencies) |
| `--path PATTERN` | Paths this task may write (repeatable — scope) |
| `--plan PLAN` | Plan this task belongs to |
| `--work-unit UNIT` | Work unit this task delivers |
| `--brief REF` | Brief reference (path or free text) for whoever picks this up |
| `--key KEY` | Idempotency key — the same key never creates a twin task |

`edda task done` requires `--receipt` ("no receipt, no done") and accepts
repeatable `--evidence` paths. `edda task start` records a lease (default
TTL 3600 s, enforced by the P2 reconciler).

```bash
edda task new "Wire up rate limiter" --assignee worker-1 \
  --brief "docs/briefs/rate-limit.md" --key demo-001
```

Output:

```
Created task #1 'Wire up rate limiter' [ready]
```

```bash
edda task list
```

Output:

```
#1 [ready] Wire up rate limiter (assignee: worker-1)
```

```bash
edda task show 1
```

Output:

```
Task #1: Wire up rate limiter
  status:   ready
  assignee: worker-1
  brief:    docs/briefs/rate-limit.md
  created:  2026-09-02T13:35:22.9345246Z
  updated:  2026-09-02T13:35:22.9345246Z
```

### `edda dispatch`

Run one agent turn with no plan file, DAG, or state machine. Reads the
prompt from `--prompt-file` and runs exactly one turn through the selected
backend; loop control stays with the caller.

```bash
edda dispatch --agent <AGENT> --prompt-file <FILE> [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--agent AGENT` | Backend that runs the turn: `claude` (default), `pi`, or `codex` |
| `--task-id ID` | Task-rail id whose brief the ACP prompt is derived from: an ACP dispatch takes prompt, scope, and resume id from the task instead of a prompt file. Only valid with an ACP agent (`acp:*`); paired with a non-ACP agent it is an error (`--task-id is only valid with an ACP agent`), not a silent no-op. Requires the task to be running |
| `--prompt-file FILE` | Path to the file containing the prompt, read verbatim (required) |
| `--session-id ID` | Session id passed to the backend verbatim; generated and printed when omitted so the caller can reuse it on the next call. pi and codex resume a prior conversation by repeating the id; claude refuses an id that already exists (`Session ID <id> is already in use`) and needs `--resume` |
| `--resume` | Continue the conversation `--session-id` names instead of starting a new one (`claude --resume <id>`). claude only — pi and codex resume by repeating `--session-id` alone and refuse this flag. Requires `--session-id` |
| `--cwd DIR` | Working directory for the agent (default: current directory); must exist. With `--issue`, this is also the repository context the claim guard's GitHub reads and writes run in |
| `--budget-usd N` | Per-turn budget in USD (codex cannot enforce budgets) |
| `--timeout-sec S` | Turn timeout in seconds (default: 1800, like a conduct phase) |
| `--permission-mode MODE` | Permission mode carried on the synthetic phase verbatim (default `bypassPermissions`); only the claude backend consumes it today, pi and codex ignore it |
| `--issue N` | Check and claim the GitHub issue before launch; refuses another owner, an open PR, or a merged delivery |
| `--machine MACHINE/ROLE` | Full claim identity, e.g. `4090/worker-1` (also via `EDDA_MACHINE`; requires `--issue`). Bare machine names are invalid |
| `--owns PATHS` | Comma-separated paths this dispatch will write. A live overlapping writer is refused before the agent starts; the claim is released when the foreground worker exits. |
| `--detach` | Start the dispatch outside the caller's process group or Windows Job Object and return a durable receipt immediately. |
| `--build-lane NAME` | Optional Cargo lane for a detached worker: `worker-1`, `worker-2`, `verifier`, or `verifier-2`. Requires `--detach`. |
| `--detach-log-dir DIR` | Directory for detached logs, manifests, and the prompt snapshot (default: system temp `edda-dispatch` directory). Requires `--detach`. |
| `--json` | Print exactly one JSON object to stdout instead of text lines |

A `codex` agent that must reach the network — posting a PR comment with
`gh`, pushing a branch — should be launched through
`edda dispatch --agent codex` (GH-565): the backend sets no sandbox or
approval flags, so the user's global Codex configuration is inherited
unchanged. The Claude Code codex plugin is not a substitute for that role;
it defaults to a `read-only` sandbox that overrides `~/.codex/config.toml`,
and a read-only sandbox has no network access.

With `--json` the object has the shape
`{"outcome":"done\|crash\|timeout\|max_turns\|budget_exceeded\|claim_refused", "result_text":string\|null, "cost_usd":number\|null, "session_id":string, "error":string\|null, "model_requested":string, "model_observed":string, "session_observed":string}`.
With `--issue`, dispatch reads `taking: <machine>/<role>` comments and PR
history before starting the agent. Ownership compares the full token; routing
labels (`lane:*`) are ignored. A matching open or merged PR blocks dispatch
(title `GH-N` or branch `ghN`, case-insensitive). Closed, unmerged PRs do not
block a retry. GitHub failures fail closed. The prompt file and the working
directory are validated before any claim write, so an invalid local
prerequisite produces no GitHub mutation; a malformed `--machine` identity is
refused with exit 2 before any GitHub call.

For an unclaimed issue, dispatch writes `taking: <machine>/<role> at <time>`,
adds `fleet:claimed`, removes `fleet:ready`, and assigns `@me` before launch.
Self-claims add no duplicate comment and repair the queue labels/assignee after
a partial write. The check and writes are sequential GitHub calls, not a
cross-session lock. Release/reassignment is handled separately.

`--detach` uses a Scheduled Task on Windows and a new process group on Unix.
It copies the prompt into the receipt directory, explicitly sets `HOME`, and
sets `CARGO_TARGET_DIR` only when `--build-lane` is named. Its JSON result is a
receipt rather than a turn result:

```json
{"handle":"dispatch-…","log":"C:\\…\\dispatch-….log","manifest":"C:\\…\\dispatch-….json","task":"edda-dispatch-…"}
```

The manifest starts as `launching`, then records the worker PID and terminal
`completed`, `timeout`, or `failed` state. A restarted controller can use the
returned handle, log path, and manifest path without reconstructing a task
name. On Windows, the generated task wrapper carries the controller PID and
its actual creation time for safe stale-task recovery.
When refused by the GitHub claim guard (`outcome` is `"claim_refused"`), a reduced shape is emitted:
`{"outcome":"claim_refused", "error":string, "issue":number, "machine":string}`.
`session_id` is the id edda asked for; `session_observed` is the one the
backend reported in-band, or `"unknown"`. They differ when a `--resume` forked
instead of continuing, which is the only way to see that from outside.

Exit codes:

| Code | Meaning |
|------|---------|
| `0` | agent done |
| `1` | agent crash or any other failure (including pre-dispatch errors) |
| `2` | timeout or GitHub claim refusal/read/write failure, including a malformed `--machine` identity (distinguished by outcome) |
| `3` | budget exceeded |
| `4` | max turns |

A one-turn dispatch against the `pi` backend, where `prompt.txt` contains a
trivial instruction (real transcript, edda 0.4.0):

```bash
edda dispatch --agent pi --prompt-file prompt.txt
```

```
pong
Cost: $0.00
Session: 86929af4-8f4c-58d1-9742-1ba96c1eba94
```

The same turn with `--json` prints exactly one object (real transcript):

```
{"cost_usd":0.000341235,"error":null,"outcome":"done","result_text":"ok","session_id":"37b8267b-e87b-56a4-bbe6-cff142f1f427"}
```

A pre-dispatch failure exits `1` per the table above (real transcript; the
OS-error line is locale-dependent):

```bash
edda dispatch --agent pi --prompt-file missing.txt
```

```
Error: --prompt-file not readable: missing.txt

Caused by:
    系統找不到指定的檔案。 (os error 2)
```

### `edda verdict`

Issue a verdict on a gated subject (approve/reject) — GH-519. The subject is
free-form; for conductor gates it is `<plan-name>/<phase-id>`. Approving a
gate may resume the waiting agent; rejecting feeds the comment back into the
gated agent session as its next turn.

```bash
edda verdict approve [OPTIONS] --sha <SHA> <SUBJECT>
edda verdict reject --sha <SHA> --comment <COMMENT> <SUBJECT>
```

| Argument / Option | Description |
|-------------------|-------------|
| `SUBJECT` | Gated subject (argument); `<plan>/<phase>` for conductor gates |
| `--sha SHA` | Full 40-hex git SHA the verdict applies to |
| `--comment TEXT` | Optional for approve; **required** for reject (fed back to the agent) |
| `--session ID` | Session ID (uses `EDDA_SESSION_ID`; `--session` required when identity is ambiguous) |

```bash
edda verdict approve "demo/phase-1" \
  --sha 0000000000000000000000000000000000000000 --comment "sanity"
```

Output:

```
Verdict recorded: approved demo/phase-1 @ 0000000000000000000000000000000000000000
  event: evt_01m1h59q2exp6xm7h2jyv9ks4r
  comment: sanity
```

> **Freshness (GH-519 D6):** a verdict only satisfies a waiting gate if it
> postdates the gate's `gate_entered_at`. Pre-recording a verdict — approving
> a known SHA *before* the gate opens — does not work: the gate ignores any
> verdict recorded before it entered `AWAITING_VERDICT`, even for the
> matching SHA. Wait for the gate to open, then run the `edda verdict`
> command the conductor prints.

### `edda phase`

Agent phase map, plus approve/reject sugar over `edda verdict` (GH-547).
The status view shows per-plan/per-phase agent state; `phase approve` and
`phase reject` resolve `gate_sha` and session from the persisted conductor
state instead of requiring `--sha` (both remain available as explicit
overrides).

```bash
edda phase [--json]                          # status view
edda phase approve <plan>/<phase> [--comment TEXT]
edda phase reject <plan>/<phase> --comment TEXT
```

`phase reject --comment` is mandatory: the comment becomes the redispatch
prompt for the gated agent session.

```bash
edda phase
```

Output (no conductor state in this workspace):

```
No agent phase data found.
Phase detection runs automatically during Claude Code hook dispatch.
```

---

## Orchestration

### `edda plan`

Plan scaffolding and templates.

```bash
edda plan init     # generate plan.yaml from template
edda plan scan     # scan codebase and suggest a plan
```

### `edda conduct`

Multi-phase AI plan conductor.

```bash
edda conduct run <PLAN.yaml>     # run a plan
edda conduct status              # show running/completed plans
edda conduct retry <PLAN>        # reset a failed phase
edda conduct skip <PLAN>         # skip a phase
edda conduct abort <PLAN>        # abort a running plan
```

---

## Internal / experimental commands

The verbs below exist in the binary but are not part of the recommended
daily surface. Each is either internal plumbing — meant to be invoked by
hooks, schedulers, or other verbs rather than by hand — or experimental,
with semantics that may still change. They are listed here so the
documented surface cannot silently drift from the binary;
`scripts/check-cli-docs.sh` enforces the same invariant.

| Command | What it does | Why not for direct use |
|---------|--------------|------------------------|
| `actor` | Manage project actors (add, remove, list, grant, revoke) | Identity grants are policy-layer plumbing; manage access through operator workflow, not ad-hoc CLI calls |
| `group` | Manage project groups for cross-project sync | Experimental multi-repo grouping; semantics not settled |
| `sync` | Pull shared decisions from group members | Depends on the experimental `group` setup |
| `unclaim` | Release this session's coordination scope | Counterpart of `edda claim`; hooks and reconciliation release scopes, so manual use is rarely needed |
| `coord` | Show coordination state | Shortcut for `bridge claude render-coordination`; rendering plumbing meant for hooks |
| `setup` | Setup a bridge integration | Shortcut for `bridge <platform> install`; use `edda init` or `edda bridge` instead |
| `recap` | Chronicle synthesis — cognitive zoom across sessions; --digest prints the deterministic operator digest (exceptions, cost, ready tasks) for the last window | Experimental; output shape may change |
| `export` | Export the ledger as human-readable Markdown (read-only projection; SQLite stays authoritative) | A convenience projection; automation should query the ledger or `edda log`, not parse exports |
| `hook` | Hook entrypoint (called by supported coding-agent hooks) | Internal: expects a hook payload on stdin; hand-invocation writes events with wrong attribution |
| `intake` | Task intake — ingest external tasks into the ledger | Experimental ingest surface |
| `prs` | Scan and record PR events from GitHub | Needs network and a token; normally driven by the scheduler or `edda watch` |
| `pipeline` | Auto-execution pipeline — skill chain with approval gates | Experimental orchestration layer; prefer `edda plan` / `edda conduct` for reviewed plans |
| `bundle` | Create and manage review bundles for rapid approval | Deprecated; use `edda review` |
| `brief` | View task engineering briefs (materialized from ledger events) | Read-only viewer normally consumed via `edda task` workflows |
| `policy` | Approval policy management (show, check, init) | Changes gate semantics; edit policy deliberately, not ad hoc |
| `notify` | Push notification management; notify send --title --file pushes free text as event "digest" | Plumbing for other verbs; needs a configured channel |
| `pair` | Device pairing and token management | Security-sensitive: tokens grant access; manage from the pairing device |
| `serve` | Start HTTP API server | Long-running process; run it as a managed service, not ad hoc in a shell |
| `user` | User-level aggregation (cross-repo queries, rollup, config) | Experimental cross-project surface |
| `rules` | L3 post-mortem learned rules management | Written by post-mortem runs; hand edits can break TTL-decay semantics |
| `scan` | Capability scanner — identify gaps via LLM analysis | Costs tokens; experimental |
| `propose-issue` | Issue proposal workflow — draft, review, and create GitHub issues | Experimental; requires `gh` authentication |
| `propose-patch` | Controls patch workflow — evaluate quality rules and propose Karvi controls adjustments | Niche governance surface, experimental (references the retired Karvi workflow) |
| `skill` | Manage skill registry (scan, list, show, search) | Experimental registry |
| `tool-tier` | Tool tier governance — query and manage tool risk classifications | Governance plumbing consumed by other tools |

### edda fleet

#### edda fleet health

Measure the path-classified mix of recent work. `edda fleet
health` reads the merged PRs and opened issues of the last `--window` days
through `gh` (server-side date filters `merged:>=` / `created:>=`), and
classifies each by changed paths (merged PRs) or by the backticked paths of
the issue's `## Predicted surface` section (issues): `crates/`, `sdk/` →
product; `scripts/`, `docs/fleet/`, `.github/`, `REVIEW.md` → mechanism;
anything else → other. A PR or issue takes the majority class of its paths;
ties resolve product over mechanism over other. The report states two
numbers against ledger thresholds: the product share of merged PRs and the
mechanism issues opened per day.

```bash
edda fleet health --window 7 --json
edda fleet health --line
```

Flags:

- `--window <N>` — rolling window in days; default 7; must be at least 1.
- `--json` — emit the full health report as JSON.
- `--line` — emit one digest line, intended for the digest adapter (#1025).
- `--json` and `--line` are mutually exclusive (usage error, exit 2).

Thresholds come from ledger decisions: `fleet.health.product-share-floor`
(default 50) and `fleet.health.mech-issues-per-day-ceiling` (default 9).
`thresholds.source` is `ledger` when both keys resolved, `default` when
neither did, and `mixed` otherwise.

Sampling: each query fetches at most 200 PRs / 300 issues.
`merged_prs.fetched`, `merged_prs.truncated`, `issues_opened.fetched` and
`issues_opened.truncated` record how many rows came back and whether a cap
was hit. A truncated sample must not be trusted for the freeze decision.

Status: RED when the product share is below the floor or mechanism issues
per day exceed the ceiling; YELLOW when within 20% of either; GREEN
otherwise. RED sets `mechanism_dispatch: freeze`, otherwise `open`; the
ordering layer that consumes it is #1015.

Exit codes:

| Code | Meaning |
|------|---------|
| 0 | GREEN |
| 3 | YELLOW |
| 4 | RED |
| 1 | error (a `gh` failure or a failed stdout write) |
| 2 | usage |

#### edda fleet order

Deterministic, health-gated ready queue with lane routing. `edda fleet order`
ranks the open issues so ordering is a re-runnable computation instead of a
hand-written plan: the same input yields the same output byte-for-byte, and
every row carries the score components that produced its rank, so the
operator's remaining move is a veto at the queue head.

```bash
edda fleet order                                   # text report
edda fleet order --markdown                        # board-comment table
edda fleet order --issues queue.json --json        # offline, from a fixture
```

Flags:

- `--issues <PATH>` — read open issues from a JSON fixture with the shape of
  `gh issue list --json number,title,body,labels` instead of querying `gh`.
- `--health-status <GREEN|YELLOW|RED>` — use this status instead of computing
  it; any casing is accepted, anything else is a usage error (exit 2).
- `--window <N>` — rolling window in days for the health computation;
  default 7, must be at least 1.
- `--json` — emit the queue as JSON.
- `--markdown` — emit the board-comment table.
- `--limit <N>` — keep only the top N rows; ranks are assigned before
  truncation.
- `--json` and `--markdown` are mutually exclusive (usage error, exit 2).

Score components, which sum to the row's total:

| Component | Value |
|-----------|-------|
| `class` | product +40, other +10, mechanism +0, from the `## Predicted surface` paths |
| `readiness` | `fleet:ready` +25 |
| `priority` | P0 +30, P1 +15, P2 +5 |
| `blocker` | +50 — the RED-freeze exception, below |
| `collision` | −10 per peer issue declaring a shared surface path |
| `freshness` | −100 when the freshness re-check FAILed |
| `frozen` | the negation of everything above, when the row is frozen |

Rows sort by total descending, then by issue number ascending, so equal scores
always fall back to the older issue.

Health coupling: when the status is RED, `mechanism_dispatch` is `freeze` and
every mechanism-class row is zeroed (`status: frozen`). The one exception is a
row whose body cites a red run or gate link — a URL containing `/actions/runs/`
or `/checks` — which instead earns the `blocker` bonus. Without a link the
issue is not a pipeline blocker.

Collision: computed from the pairwise intersection of `## Predicted surface`
paths, never from labels (#1005). Labels are issue-level and conflicts are
file-level, so a shared label neither predicts nor excludes one — #671 and #685
are the live example: they share two labels (`enhancement`, `lane:feature`),
which says nothing either way, and they really do collide, on
`crates/edda-ledger/src/sync.rs` and `docs/guides/multi-agent.md`. Every
colliding peer costs a penalty, and walking the ranked queue puts the head on
`ready` and the rest on `hold` naming the owner and the shared path.

Freshness (#970) is re-derived here rather than trusted from a label. It
resolves paths against the pinned tree (`git ls-tree -r HEAD`) and commands
against this binary's own verb table — not against the `edda` on `PATH`, which
may predate the tree and produced the false `FAIL edda fleet --help exited
nonzero` on #1015 itself. Findings are WARN or FAIL only; a reference that
resolves is silent. A path or command declared by a section that describes what
the issue will build (`doneWhen`, any `… surface` heading, `改哪裡`) WARNs, as
does an evidence-cited command; a reference in an observation section to a path
or verb that does not exist FAILs and marks the row `stale`. `doneWhen`
headings match case-insensitively. Cited line ranges (`lib.rs:288-296`) resolve
to their file, and crate- or module-relative references resolve as a suffix of
the tracked paths.

Lane routing, in evaluation order. A `governance` or `fleet:goal` label or a
`[判斷]` marker in the body routes to `controller`. Then the four flash criteria,
any one of which failing routes to `strong`:

1. the surface is non-empty and no wider than the flash cap — the ledger key
   `fleet.order.flash-max-surface-files` (default 3), with `flash_cap_source`
   recording whether it resolved from the ledger or the built-in default;
2. no `scripts/` path in the surface;
3. `brief-render` — `scripts/fleet/brief-from-issue.sh <issue>` exits 0 (#885);
4. `dispatch-dry-run` — `scripts/fleet/brief-validate.sh` reports VALID for the
   issue's authored brief under `$EDDA_FLEET_SCRATCH` (default `~/.edda/fleet`),
   which is the same brief `next-issue.sh` would launch with (#945).

Criteria 3 and 4 need a subprocess, so they are run once at collection time
alongside `gh` and `git ls-tree` and handed to the ranker as data; ranking
itself stays pure and network-free. Only issues that criteria 1 and 2 have not
already routed away are spent a process on, and a failed render short-circuits
the dry-run. Each result is reported on the row under `flash_checks`.

**A criterion that was not evaluated is not a pass.** An unwritten authored
brief, a missing `sh`, or a `--issues` fixture run (where the issue numbers are
synthetic and the scripts are not run at all) routes the row to `strong`, and
`lane_reasons` names the check that decided it.

Statuses: `ready`, `hold` (behind a higher-ranked row on a shared path),
`frozen` (mechanism under a RED freeze), `stale` (freshness FAILed), `claimed`
(`fleet:claimed`). Only `ready` rows claim paths, so an in-flight or frozen row
never blocks anything behind it.

Exit codes:

| Code | Meaning |
|------|---------|
| 0 | queue rendered |
| 1 | error (a `gh` or `git` failure, an unreadable fixture, a failed stdout write) |
| 2 | usage |

### edda review

Review a committed branch using an independent read-only agent and record a
`review_verdict/0` event in the author's ledger. The payload and `--json`
output are unstable. `edda log --type review_verdict` reads the events.

```bash
edda review --agent pi --model openai-codex/gpt-5.6-sol --spec acceptance.md --gate 'cargo test -p mycrate'
edda review --pr 123 --resume --json
```

The default agent is pi, with its inherited model. The base resolves through
origin/HEAD, origin/main, origin/master, main, then master; use `--base` to
override, and `--head` to select a committed subject. Empty diffs fail before
launch. `--pr` resolves immutable head/base and the first closing issue as the
spec; an explicit `--spec <path|#issue>` takes precedence.

The reviewer receives the base version of REVIEW.md, scoped decisions,
evidence, and the diff in a unique detached checkout. pi and Claude use a
read-only tool allowlist without a shell. Requested and observed model/session
identities remain distinct. Same-author sessions are refused; model diversity
is optional with `--require-model-diversity`. `--session-id` sets the reviewer
UUID explicitly; it must be a UUID and cannot name an author session. When
resuming, an explicit `--session-id` must match the prior ledger-recorded
reviewer session. `--resume` requires that prior review and reuses its
reviewer session; a backend fork disqualifies it. `--thinking` selects pi's
thinking level; Claude and Codex reject the option rather than silently
ignoring it.

The brief also carries an `ENGINE QUALIFICATION (R22)` section naming the PR's
R22 surface (`review`, `gate`, `shipping` or `internal-tool`, strictest first)
with the changed path that decided it, the canonical requested model id, and
whether R22's engine table makes that engine authoritative for the surface. An
authoritative engine decides the specification's judgment item itself; any other
engine escalates it, as REVIEW.md §6.1 requires of a checklist-type engine. An
engine the table does not name — including a round that passes no `--model`,
since R22 names model ids — is never authoritative: it records the
`engine-not-authoritative` disqualifier and cannot exit 0. `--json` and the
ledger event carry the same statement under `engine_qualification`.

`--require-model-diversity` is unchanged by that section and remains
independent of it. Without the flag the independence policy is `session`: an
author and reviewer on the same model are recorded in the receipt as
`independence: same-model` and stated in the brief, but do not disqualify the
round. With the flag the policy is `model` and any independence other than
`verified` disqualifies it.

Gates are READ from clean exact-SHA command receipts and required exact-SHA CI.
Missing checks remain unverified; any red evidence wins. `--run-gates` opts in
to execution of trusted declared commands, bounded by `--max-ran-sec` (300 by
default). Cargo gates require an existing `CARGO_TARGET_DIR` build lane.
`--trust-spec` authorizes issue verify commands; explicit issue selection by
itself does not. Local specification paths are operator-trusted.

| Exit | Meaning |
|---|---|
| 0 | LGTM with all qualification requirements satisfied |
| 1 | Changes requested |
| 2 | Unable to review: refusal, empty diff, provider failure or invalid verdict |
| 3 | LGTM with unmet qualification requirements |

Output lists findings, SHA, reviewer, round, gate evidence, cost and actionable
disqualifiers. Unmeasured cost is never represented as zero. No PR comment,
label change, merge, or conductor gate approval occurs automatically.
`--timeout-sec` defaults to 900; `--budget-usd` is passed to supported backends.
`--keep-worktree` preserves the review checkout for inspection.

#### edda review gate

Answer whether a reviewed SHA has passed. Read-only: it launches no review,
writes no event, and never touches GitHub.

```bash
edda review gate <sha> --base origin/main
edda review gate <sha> --json
printf 'LGTM\t0\t0\n' | edda review gate <sha> --verdicts -
```

Two rules decide it, and this verb is their only implementation (REVIEW.md §8):

- **Union.** `PASS` requires at least one qualifying `LGTM` at P0=0 P1=0 with
  nothing else standing on the SHA. A later LGTM never overrides an earlier
  Changes Requested (GH-742). An unqualified LGTM at P0=P1=0 is *provisional*:
  never a pass on its own, but it does not hold a later qualified LGTM at
  fail; with any P0/P1 it stands like any other non-qualifying verdict. A
  missing or non-numeric count reads as non-zero.
- **Window** (only with `--base`). If the base has advanced over any file the
  subject changed, the reviewed tree is not the tree that would merge, and the
  gate fails with reason `window`.

Verdicts come from the ledger's `review_verdict` events for that SHA;
`unreviewed` events are not verdicts and are skipped. `--verdicts <path|->`
reads them from the caller instead, one `verdict<TAB>p0<TAB>p1` record per
line — the shape a caller derives from §7 comments while the ledger does not
yet carry verdicts across machines. The SHA must be a full lowercase 40-hex
value, the shape REVIEW.md R5 requires.

Stdout is one line — `PASS <sha> verdicts=<n>`, `FAIL <sha> <reason>` or
`NONE <sha>`. `--json` adds each verdict's `reviewer_model`, `round` and
`cost_usd`; a cost nobody measured prints `unmeasured`, never `0`.

| Exit | Meaning |
|---|---|
| 0 | Pass: the union rule is satisfied and the window is clear |
| 1 | Fail: a non-qualifying verdict stands (`union`), or the base moved (`window`) |
| 2 | No verdict on the SHA — an inability to judge, not a judgment |

#### edda review due

Is this PR worth reviewing again? Read-only: it launches nothing, writes no
event, and never touches GitHub.

```bash
edda review due --head <sha> --pr 42 --pushed-at 2026-09-07T12:00:00Z
edda review due --head <sha> --pr 42 --draft
```

This is the main cost switch in the review system. A round-1 Opus review
measured $1.28-$2.57 on #754; the resumed delta round of the same PR measured
$0.22 and then $0.02. Reviewing once per push therefore buys a round-1 price
per push, which is what this verb exists to stop.

The policy, in order — the first matching row wins:

| Condition | Answer |
|---|---|
| `--draft` | `SKIP draft` |
| `--unreviewed-label` and the head has not demonstrably moved | `SKIP review-unreviewed` |
| `--ready` (the PR left draft this cycle) | `REVIEW ready` |
| `--response-at` newer than every record of having reviewed | `REVIEW response` |
| head moved and the push has settled | `REVIEW push` |
| head moved, still inside the debounce | `SKIP debounce <n>s` |
| head moved, push time unknown | `SKIP debounce push-time-unknown` |
| otherwise | `SKIP reviewed` |

`ready` and `response` are one-time events the operator is waiting on, so
neither is debounced; only `push` is, because only `push` repeats. A draft is
refused first and cannot be enabled by any trigger.

Two of those rows exist because `response` is *not* debounced, which makes a
wrong "unanswered" answer cost one round per poll rather than one round. So it
fires only when the response is newer than **both** records of having reviewed
that may exist — the ledger verdict (`review_verdict`, which is absent for a
round published through the §7 comment path, and does not cross machines) and
the caller's own `--last-reviewed-at`. With neither, there is nothing to be
newer than and the PR falls through to the debounced push rule.

`SKIP debounce push-time-unknown` is the same caution: the caller cannot
distinguish "this PR has no commit date" from "the forge call failed", and the
second would otherwise open the switch for every PR at once.

Facts come from the caller, and rounds from the ledger. `--last-reviewed`
matters: a round published through the §7 comment path writes no
`review_verdict` event, so the daemon's own record is the only evidence it
happened, and without it every cycle would read as never-reviewed.

Stdout is two lines — `REVIEW <reason>` or `SKIP <reason>`, then
`review cost so far: $X over N rounds`. A round whose cost was never measured
makes the whole total read `unmeasured` rather than contributing zero: a sum
that silently drops a round reads cheaper than the truth. A second or later
round appends ` --resume <session id>` naming round 1's reviewer session.

Settings live in `.edda/review/due.json`, and absent means defaults:

```json
{ "debounce_seconds": 600, "triggers": ["ready", "response", "push"] }
```

A file that exists but cannot be parsed is an error, not a silent fallback —
an operator who wrote a debounce and got the default one would be paying for a
switch they believe they threw.

| Exit | Meaning |
|---|---|
| 0 | Due: start a round |
| 1 | Not due, for the printed reason |
| 2 | Cannot judge — an unreadable ledger or a malformed argument, never a decision |

#### `edda review deliver`

Reads the §7 verdict comments GitHub holds for a pull request, reports what
they amount to on one reviewed SHA, and delivers the writes that follow: the
`review:lgtm` / `review:changes-requested` label, the `Independent Review`
commit status, and — for a malformed verdict comment — a one-shot notice.
This is `edda review`'s only subcommand that touches GitHub.

```bash
edda review deliver --pr 1030
edda review deliver --pr 1030 --sha "$reviewed" --json
```

`--pr` names the pull request. `--sha` is the reviewed commit, as a full
40-character lowercase hex SHA; without it the PR's current head is used. A
verdict comment counts only when it is pinned to that SHA.

A comment is a verdict when its **first** line is the §7 heading. A heading
anywhere else in the body is a transcript dump, not a verdict: it contributes
nothing to the union, and its comment id gets a one-shot `review: malformed
verdict comment <id>` notice — posted once (a later run recognizes the
existing notice text and does not repeat it). Posting a *new* notice
withholds status and label for that run, regardless of whether a real
verdict also stands on the SHA (R23, #917); a withheld write reports outcome
`withheld` (not `skipped`) and the run's exit code says so too (see the exit
code table below), so a poller comes back rather than treating the round as
settled. A round whose heading carries ` (SHADOW)` — in either recorded
position — is well formed and contributes nothing to the union, because a
SHADOW round is calibration evidence, never a gate (REVIEW.md §8, rules.md
R22); when it is the only signal standing on the SHA, deliver performs **zero
GitHub writes** rather than reporting the SHA unreviewed.

The verdict word is read in order: `Changes Requested` first, then the
`Provisional — …` line written for an unqualified LGTM, then plain `LGTM`. A
`P0=`/`P1=` count that is absent stays absent, and the union rule reads an
unstated count as non-zero. A later LGTM never overrides an earlier Changes
Requested standing on the same SHA (GH-742).

The `Independent Review` status is written for the union's state —
`success`, `failure`, or `error` when no verdict stands at all — regardless
of whether the PR's current head has since moved past the reviewed SHA. The
`review:*` label is different: it is applied **only** while the current head
still equals the reviewed SHA. A moved head still gets the status; it never
gets the label — and a withheld run (a new malformed notice) reports the
label the same way, `skipped (head moved)` rather than `withheld`, when the
head has moved. Whenever the desired label stands (freshly applied, or
already applied from an earlier run) and its sibling is still standing too,
the sibling is removed — a later Changes Requested on the same SHA removes a
standing `review:lgtm`, and vice versa — so the two never stand together;
this cleanup retries on every run until it succeeds, and a failed removal is
its own reported write (`label_removed`), not swallowed into the label's own
outcome.

Every write is idempotent by construction: deliver reads the label already on
the PR, the latest `Independent Review` state already posted for the SHA, and
the comments already present, before writing anything, so re-running deliver
for an already-delivered verdict performs zero new GitHub calls.

Stdout is one line — the union state, the SHA, and the number of verdicts
standing — followed by one `malformed <id>` line per malformed comment, one
`notice <id> <outcome>` line per notice attempted, and a `status`/`label`/
`label_removed` line for each of those writes. `--json` emits the same as an
object, with an `outcome` of `done`, `skipped`, `withheld`, or `failed` (with
a `reason`) for each write.

| Exit | Meaning |
|---|---|
| 0 | Delivered — every write this round called for succeeded, or none was due |
| 1 | Partially delivered — some writes succeeded, at least one failed |
| 2 | Failed — no write succeeded, the comment list was unreadable, or `--sha` was invalid |
| 3 | Withheld — a new malformed-comment notice posted this run held back a status/label that was due (R23, #917); rerun to retry |

Launching reviews (`edda review`), the trigger policy (`edda review due`,
GH-763), verdict semantics (`edda review gate`, GH-769), and merging
(`edda prs check-merge`; GATE-01 — deliver never merges) stay out of scope.

