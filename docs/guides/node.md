---
title: Cross-Machine Node
---

# Cross-Machine Node (`edda node`)

`edda node` moves allowlisted coordination events between machines over the
Tailscale network (GH-685). It transports and records. It does **not** execute a
remote process, migrate or kill anything, wake a model, schedule work, or become
a second task system. The authoritative wire contract is
`proof/cross-machine/node-v0-wire-contract.md`; this guide is the operator view.

## What you get

- **Lane delivery** — a request sent to `<machine>/<role>` is durably queued and
  delivered to that peer's node, with a `sent → delivered → acked` state on the
  sender and `dead` after a TTL.
- **`edda inbox`** — a receiving agent can `wait` for an unacked request, `ack`
  exactly one request id, and `send`/`status`.
- **Owner-return projection** — imported returns stay unclaimable until this
  machine binds the owner; `edda return status` names ambiguity
  (`holder_unknown`, `handover_pending`) instead of reporting a healthy zero.

## Configure `node.json`

`node.json` lives at `<store root>/node.json` (i.e. `~/.edda/node.json`) and is
**never** committed:

```json
{
  "version": 1,
  "node": { "alias": "4090", "bind": "100.105.187.119", "port": 6850 },
  "peers": [
    { "alias": "docs", "host": "100.116.144.116", "port": 6850, "tokenEnv": "EDDA_NODE_TOKEN" }
  ]
}
```

- `alias` is this machine's `<machine>` label (`^[a-z0-9._-]{1,64}$`). A session
  id, run id, host display name or path is never an address.
- `bind` must be one of this host's Tailscale `100.x` IPv4 addresses. Any other
  bind refuses to start unless the test-only `--insecure-bind` flag is passed.
- `tokenEnv` names an environment variable holding the shared token. The value
  is read at request time and is never stored, printed, logged or replicated.
  Unknown keys anywhere in `node.json` fail closed and name the offending key.

## Run the node

```bash
EDDA_NODE_TOKEN=... edda node start
edda node status --json
edda node peers --json
```

`start` runs the existing HTTP server with the node routes wired plus a bounded
replicator that flushes each peer's durable outbound queue. `status` and `peers`
are **local observations**: an unreachable or unobserved peer reports
`reachable: false` with a reason and a freshness timestamp, never a silent zero,
and the revision is a local fact (from `EDDA_PI_RELEASE_ID` or git HEAD), not a
global claim.

## Send a request across machines

Address the peer as `<machine>/<role>`:

```bash
edda request "docs/reviewer" "Please review the frozen contract"
edda inbox send --to "docs/reviewer" --message "Please review the frozen contract"
```

- A target whose machine is **this** machine's own alias stays local.
- Any other machine's target writes the local `request` (same shape as ever, so
  `edda coord` and the board keep working) and enqueues a `lane_request` into
  that peer's durable queue (`<store root>/node/queue/<peer>.jsonl`). Nothing is
  dead-lettered, and the queue survives a restart.
- The wire carries labels and a machine alias only — never a session id, run id,
  path, root, lease, worktree or credential.

## Receive and acknowledge

On the receiving machine an agent answers to its role label:

```bash
edda inbox wait --actor "reviewer" --timeout 120   # read-only; exit 2 on timeout
edda inbox ack  --actor "reviewer" --id "$REQUEST_ID"
```

`wait` blocks until an unacked request for `--actor` exists, then returns it. It
polls no faster than 250 ms, has no scheduler, and **never** acks. A
`request_delivered` marker is transport provenance, not an acknowledgment: a
delivered-but-unacked request is still pending and still returned by `wait`.

`ack` covers **exactly** the named id; an unknown id is a named error. When the
request arrived over the wire, `ack` also enqueues
`receipt{state:"acked"}` back to the origin machine.

## Read delivery state

```bash
edda request --status "$REQUEST_ID" --json
edda inbox status --id "$REQUEST_ID" --json
```

The state is read from the sender's durable queue:

| State | Meaning |
|-------|---------|
| `pending` | Queued durably and (re)sent, no 2xx yet. |
| `delivered` | The peer node accepted the envelope (2xx). |
| `acked` | The receiving agent recorded consumption. |
| `dead` | No ack within `EDDA_NODE_REQUEST_TTL_SECS` (default 24 h). A real state, never a silent zero. |

> **Spelling note.** GH-685's issue text wrote `edda request status <id>`; that
> would collide with the existing `edda request <to> <message>` positional form,
> so the additive `--status` flag is used instead. `edda inbox status --id` is
> an alias.

## Ambiguity, not healthy-zero

Cross-machine consistency is deliberately honest. There is no global
exactly-once claim: the bound is exactly-once on the holder machine, at-most-once
elsewhere, and at-least-once during a partition or handover. Those gaps surface
as named states, never as `dead` or a zero:

- `edda return status --owner <ref> --json` reports `holder_unknown` (with a
  reason string) when this machine has no local binding and no replicated
  handover names a holder, and `handoverPending: true` when a replicated
  `handover` envelope names this owner and is still pending.
- An imported `owner_return` stays **unclaimable** until this machine binds the
  owner (`edda return bind`); import creates only `messages/`, never `owners/` or
  `claims/`.

## Security-relevant behaviour

- Every envelope is `deny_unknown_fields`. Unknown fields, and secret-shaped
  names (`sessionId`, `path`, `ownerRoot`, `registry`, `lease`, `token`, ...),
  are refused by name and never partially applied.
- Auth is a shared bearer token checked before the body is read; a missing, empty
  or wrong token is a 401 that writes nothing. A token absent from both the
  request and the server config is a 401, not an open door.
- The listener binds only the machine's Tailscale `100.x` address.
- The node never executes anything remote, wakes a model, schedules, or keeps a
  second task state.
