---
title: CLI Reference
---

# CLI Reference

Complete reference for all `edda` commands.

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

Show workspace status — ledger stats, active branches, hook status.

```bash
edda status
```

### `edda doctor`

Health check for bridge integration.

```bash
edda doctor claude     # check Claude Code hooks
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
| `--session ID` | Session ID (auto-inferred from active heartbeats) |

```bash
edda decide "db.engine=sqlite" --reason "embedded, zero-config"
edda decide "auth.strategy=JWT" --reason "stateless, scales horizontally"
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

### `edda claim`

Claim a scope for coordination. Other agents see claimed paths as off-limits.

```bash
edda claim <LABEL> [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `LABEL` | Short label (e.g. `"auth"`, `"billing"`) |
| `--paths PATTERN` | File path patterns (e.g. `"src/auth/*"`) |
| `--session ID` | Session ID (auto-inferred) |

```bash
edda claim "auth" --paths "src/auth/*"
edda claim "billing" --paths "src/billing/*,src/invoice/*"
```

### `edda request`

Send a request to another active session.

```bash
edda request <TO> <MESSAGE> [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `TO` | Target session label |
| `MESSAGE` | Request message |
| `--session ID` | Session ID (auto-inferred) |

```bash
edda request "billing" "Please expose invoice total as a public method"
```

### `edda watch`

Launch the real-time TUI showing active sessions, events, and coordination state.

```bash
edda watch
```

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
