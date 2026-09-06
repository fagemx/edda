# Writing an Edda bridge

**Contract:** `adapter-contract/0.1` · **normalized protocol:** `adapter-normalized-protocol/0.1`

A bridge is a fail-open adapter between an agent host and Edda. It translates
host events; it does not make policy. This contract covers five hook bridges
and a non-CLI agent reference adapter.

## Hook obligations

| Responsibility | Requirement |
|---|---|
| Unknown/malformed host input | **MUST** exit 0 permissively; never block an agent. |
| Context | **MUST** inject bounded context between `<!-- edda:start -->` / `<!-- edda:end -->`; preserve the `edda decide` write-back tail when budgeted. |
| Lifecycle | **MUST** create/refresh heartbeat on start, remove it and release claims on end; resumed sessions continue the same Edda session, while a host fork is a new Edda session that records its parent only when the host supplies it. |
| Durability | **MUST** record activity before consumption and emit at most one digest per completed session. **SHOULD** dedupe prompt injection, clean state, and rate-limit nudges. |
| Safety | Rules are advisory unless a host explicitly supports opt-in enforcement. **SHOULD** redact before any durable write. |

A MUST failure is a gap, never a waived capability. A host may report an
unsupported SHOULD as `SKIP` with a reason.

## Launcher obligations

A launcher receipt **MUST** report `model_requested`, `model_observed`,
`session_observed`, `tools_requested`, `tools_applied`, and `heartbeat_owner`.
Requested values are caller intent; observed values come from the backend
stream or are `unknown`, never inferred from configuration. Unsupported tool
or thinking policy is refused rather than silently dropped. The launcher owns
heartbeats while its process runs; a hook owns the interactive host session.

The portable conformance harness exercises each supported launcher profile
(Claude, pi, Codex) with `fixtures/fake_launcher.py`. This is deterministic
receipt-protocol evidence — it never starts a provider, reads global config,
or claims an arbitrary installed `edda dispatch` binary has the fields. A
production launcher missing any field is non-conformant and must be tested
against its own provider-free backend shim before it can be accepted.

`docs/reference/cli.md` documents current public dispatch resume behavior:
repeat a backend session id for pi/Codex; Claude requires `--resume`. A fork is
not a resume and must receive a distinct Edda session id.

## Version and signing boundary

This contract is independent of the event-spec version but consumes its public
fixture types. `adapter-contract/0.1` is compatible with the currently frozen
normalized fixtures. #609 signing is design work: no signature is required or
claimed here. When a public signed-event API lands, an adapter must sign only
through that API and report unavailable signing honestly.

## Source-blind reference profile

The reference adapter is a non-CLI **agent runtime** (Python, shell, or a
GitHub Actions step), not a reuse of `edda hook` or any bridge source/crate. It
may use the public `edda` CLI as its data plane; that is the smallest currently
published write API and avoids inventing a store engine or requiring a private
store layout.

The fixture is `tests/adapter-conformance/fixtures/normalized-events.json`.
For every `--adapter-cmd` call the harness writes an envelope to stdin:

```json
{"event":"session.start","payload":{},"conformance":{"contract_version":"adapter-contract/0.1","protocol_version":"adapter-normalized-protocol/0.1","session_id":"conf…","workspace":"…/project","store_root":"…/store","edda_bin":"…/edda"}}
```

The command inherits `EDDA_STORE_ROOT`, has the supplied workspace as cwd, and
must return one permissive JSON object. Context may be `context`,
`additionalContext`, `additional_context`, `prependContext`, or
`hookSpecificOutput.additionalContext`.

For independently observable lifecycle evidence, the reference profile writes
these exact public CLI notes (using `conformance.edda_bin note <text>`):

```text
adapter-contract/0.1 session=<id> action=heartbeat.start
adapter-contract/0.1 session=<id> action=activity.append
adapter-contract/0.1 session=<id> action=digest.complete
adapter-contract/0.1 session=<id> action=heartbeat.end
```

It writes `digest.complete` once across repeated end delivery, and never puts
the secret fixture sentinel in a CLI note. The harness reads the public ledger
with `edda log`; a JSON response alone is not acceptance evidence. This is the
minimal portable equivalent of native private heartbeat/session-ledger files.

Run a clean-room implementation only from this guide, the fixture, and the
harness:

```text
python tests/adapter-conformance/harness/conformance.py --edda PATH_TO_EDDA --adapter-cmd "YOUR_COMMAND" --out reference-report.json
```

The existing five-bridge mode remains an integration observation of native
bridges. `tests/adapter-conformance/RESULTS.md` preserves its pinned-binary
provenance, #869/#870 gaps, and its limits. A separate source-blind worker must
supply reference evidence; this author does not claim that proof or close #610.
