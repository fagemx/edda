# Source-Blind Reference Adapter (GH610 task52, clean-room)

A small independent Python reference adapter for the Edda adapter contract
(`adapter-contract/0.1`, protocol `adapter-normalized-protocol/0.1`), written
exclusively from this clean-room's public documents and fixtures:

- `docs/guides/writing-a-bridge.md`
- `docs/reference/cli.md`
- `tests/adapter-conformance/fixtures/normalized-events.json`
- `tests/adapter-conformance/harness/conformance.py`
- public CLI help of `tools/edda.exe`

No Edda Rust/bridge/SDK source, no other worktree, and no prior implementation
was read. The adapter **never invokes `edda hook`** and imports nothing from
any bridge implementation.

## Non-CLI runtime vs CLI data plane (explicit distinction)

- This adapter is a **non-CLI agent runtime**: a Python program that speaks the
  normalized protocol on stdin/stdout (one envelope in, one permissive JSON
  object out). It is *not* a CLI tool and *not* a reuse of `edda hook`.
- It uses the **public `edda` CLI as its data plane only** — the smallest
  published write API per the guide — namely `edda note` (durable lifecycle
  notes), `edda log` (read-back), and `edda context` (context snapshot).
  No private store layout, no store engine, no bridge internals.

## Files

| File | Purpose |
|---|---|
| `edda_reference_adapter.py` | The reference adapter (Python 3.8+ stdlib only) |
| `test_reference_adapter.py` | Own meaningful tests (unittest, isolated temp store/workspace) |
| `control_stub.py` | Verbatim copy of the harness's built-in do-nothing CONTROL_STUB, used only for a mutation-negative control run |
| `reference-report.json` | Full harness report of the documented reference command |
| `negative-control-report.json` | Harness report proving the control stub is flagged non-conformant |
| `evidence/` | Raw run stdout, implementation SHA-256, edda binary version |

## Repeatable commands

Full reference command (from the clean-room root):

```text
python tests/adapter-conformance/harness/conformance.py --edda C:/ai_agent/edda-cleanroom-610-20260904/tools/edda.exe --adapter-cmd "python C:/ai_agent/edda-cleanroom-610-20260904/reference/edda_reference_adapter.py" --skip-launcher --out reference-report.json
```

Negative control (same command, control stub as adapter — must FAIL with >= 4
violations; the harness only auto-runs its built-in control in vendor mode, so
the verbatim stub is supplied here without modifying the harness):

```text
python tests/adapter-conformance/harness/conformance.py --edda C:/ai_agent/edda-cleanroom-610-20260904/tools/edda.exe --adapter-cmd "python C:/ai_agent/edda-cleanroom-610-20260904/reference/control_stub.py" --skip-launcher --out negative-control-report.json
```

Own tests (each run uses an isolated temp workspace + `EDDA_STORE_ROOT`):

```text
python -m unittest reference.test_reference_adapter -v
```

## Observed results (2026-09-04, Windows, Python 3.12)

Reference run: exit 0, `{"contract_violations": 0, "verdict": "CONFORMANT (within documented gaps)"}`.

| Check | Severity | Status |
|---|---|---|
| H-STORE-ISOLATION | MUST | PASS |
| H-FAIL-OPEN | MUST | PASS |
| H-UNKNOWN-EVENT | MUST | PASS |
| H-INJECT-START (boundaries + `edda decide` tail) | MUST | PASS |
| H-INJECT-BUDGET (body truncated, tail preserved) | MUST | PASS |
| H-LEDGER-APPEND (public CLI notes) | MUST | PASS |
| H-HEARTBEAT (start/end markers) | MUST | PASS |
| H-DIGEST-IDEMPOTENT (exactly one digest) | MUST | PASS |
| H-REDACT-STORE (sentinel absent) | SHOULD | PASS |
| H-PROMPT-DEDUP | SHOULD | PASS |
| H-NUDGE-RATE (2 nudges over 6 signals) | SHOULD | PASS |
| H-END-CLEANUP | SHOULD | SKIP (harness-design: "portable profile has no private state-file layout"; public lifecycle is covered by H-HEARTBEAT) |
| H-PRETOOL-IDENTITY | SHOULD | SKIP (reference has no session-identity rewrite capability; stays advisory-allow) |

Launcher checks were skipped by design (`--skip-launcher`, documented in the
task): the reference adapter is a non-CLI runtime, not a launcher.

Negative control: exit 4, violations `H-INJECT-START, H-INJECT-BUDGET,
H-LEDGER-APPEND, H-HEARTBEAT` — the harness detects a non-conformant adapter.
Own tests: 9/9 pass.

## Implementation behavior

- **Fail-open**: any malformed/unparseable stdin yields `{"continue": true}`
  and exit 0; unknown future events yield `{}` and exit 0.
- **Lifecycle / heartbeat**: `session.start` writes the exact public note
  `adapter-contract/0.1 session=<id> action=heartbeat.start`; `session.end`
  writes `action=heartbeat.end` (once, idempotently, before digest). The
  portable profile holds no `edda claim`, so there is nothing to unclaim.
- **Bounded context injection**: `session.start` injects
  `<!-- edda:start -->` + body (public `edda context --depth 5`) +
  write-back tail ("record ... with `edda decide` ...") + `<!-- edda:end -->`.
  Under `EDDA_MAX_CONTEXT_CHARS` / `EDDA_WORKSPACE_BUDGET_CHARS` the body is
  elided whole; the write-back tail is never truncated.
- **Durability / digest idempotency**: every tool post-event writes
  `action=activity.append` before consumption. The first `session.end` writes
  `action=digest.complete`; a per-session digest watermark (private adapter
  state under `<workspace>/.edda-reference-adapter/`) survives repeated end
  delivery so exactly one digest is ever written per completed session.
- **Dedup / rate limiting**: identical consecutive `prompt.submit` injections
  are deduped by content hash; commit-signal nudges are capped at 2/session.
- **Redaction**: tool payloads are never persisted; lifecycle notes are
  fixed-format and contain only the session id and action word, so the
  fixture sentinel can never reach the ledger.
- **Session identity**: the harness supplies the Edda session id per call;
  resumed deliveries of the same id continue the same Edda session, and a
  host fork delivering a distinct id is a distinct Edda session (the parent
  linkage is only recorded when the host supplies it — the normalized
  protocol here supplies none, so none is claimed).
- **Signing**: no signature is produced or claimed (`adapter-contract/0.1`
  requires none; #609 signing is future design work).

## Honest gaps / notes

- The two SHOULD SKIPs above are harness-designed adapter-mode skips, not
  failures; they are reported with the harness's own reasons.
- The binary reports `edda 0.4.0 (467e8be02eb9-dirty 2026-09-04)`; per the
  harness provenance note this is a pinned-binary observation, not evidence
  about current main or a supplied frozen source. No source attestation is
  invented for the binary; its SHA-256 is recorded in `evidence/`.
- The digest watermark and dedupe state are private adapter bookkeeping in
  the workspace; the durable public record remains the CLI ledger.
