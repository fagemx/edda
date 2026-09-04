# GH-610 five-bridge result matrix

## Frozen inputs

| Item | SHA-256 |
|---|---|
| `../../docs/guides/writing-a-bridge.md` | `1f1794f30abeb9e4b794fba5a0d09f57f7d2a01f1cc9b4c5b2beb86bd2b6a336` |
| `fixtures/normalized-events.json` | `166c1ba25db29ca4506c8c978352da846973999f6ebc54f81edf91ec9d431d7f` |
| `harness/conformance.py` | `1013f80daa4c66d096ad7475acfb6777aeec94bf74332f9047533f6bd32cdbf5` |
| report | `69d255c788cda8aec3c9fdda97092845c3225229f69b27c737cddc6420dd5b9c` |

Command (no Cargo/build lane):

```text
python tests/adapter-conformance/harness/conformance.py --edda C:/Users/fagem/AppData/Local/fleet-workstation/lanes/verifier/debug/edda.exe --skip-launcher --out tests/adapter-conformance/results-sdk-binary-0ad2b0c5.json
```

The command exited 3. The mutation-negative control was detected (four
violations), so this run distinguishes the observed failures from a harness
that passes a do-nothing adapter.

## Binary provenance limit

The read-only binary was SHA-256
`0ad2b0c5e236e1cd078f2de2c0bd2f3a171567163caa2574dd030bc9f4b55833`.
The supplied lane provenance says SDK source
`03d0f6ee7d06442f7d72f899de0e8c2fc0b68d4f`, base `9de7662`, with inherited
bridge code. Its embedded version instead reports `467e8be02eb9-dirty`.
Therefore this is a pinned-binary observation only: it is **not** proof about
that supplied source, current `main`, or an installed `0.4.0` binary.

## Matrix

| Bridge | MUST | SHOULD | Result |
|---|---:|---:|---|
| Claude | 8 pass | 2 pass, 2 skip | observed conformant within skips |
| Codex | 8 pass | 3 pass, 1 skip | observed conformant within skips |
| Cursor | 8 pass | 2 pass, 3 skip | observed conformant within skips |
| Hermes | 7 pass, 1 fail | 2 pass, 2 skip | **H-HEARTBEAT fail** — [#869](https://github.com/fagemx/edda/issues/869) |
| OpenClaw | 7 pass, 1 fail | 2 pass, 1 fail, 2 skip | **H-HEARTBEAT** and **H-REDACT-STORE** — [#870](https://github.com/fagemx/edda/issues/870) |

`SKIP` is capability/coverage evidence, not a pass. The JSON report contains
per-check evidence and paths in its isolated temporary store.

## Pending acceptance

- A separate source-blind worker must implement the non-CLI reference adapter
  from only `writing-a-bridge.md`, the fixture, and harness, then run the
  documented command. This author makes no clean-room claim.
- Re-run the matrix on an attested binary built from the cited source before
  treating #869 or #870 as source defects.
- No `Closes #610` is warranted until the source-blind proof and all required
  acceptance evidence are delivered.

# Source-blind reference adapter — integration (task56)

Integrated verbatim from the completed fresh clean-room package
`C:/ai_agent/edda-cleanroom-610-20260904` (fresh GLM implementation from public
inputs only). Historical receipts are preserved byte-exact with their original
paths/provenance:

- `reference/CLEANROOM_HANDOFF.md` — the implementer's raw receipt (authorship,
  attestation, original commands and cost estimate preserved as written).
- `reference/INPUT_MANIFEST.json` — the manifest the implementer verified
  before and after work (cleanroom-absolute paths are historical record).
- `reference/README.md` — the implementer's own README, byte-exact; its
  commands use the original cleanroom-absolute paths and are historical.
- `reference/edda_reference_adapter.py` (SHA-256
  `36dd3c7bff56562c0b88818363816171a220912bce254602db1e585621b118b7`),
  `reference/test_reference_adapter.py`, `reference/control_stub.py` — copied
  byte-exact; never rewritten to make tests pass.
- `reference/reference-report.json`, `reference/negative-control-report.json`,
  `reference/evidence/implementationSHA256.txt` — frozen run evidence.

Not copied, deliberately: the two `evidence/*-run-stdout.json` files
(content-identical to the committed report JSONs apart from a trailing
newline), the pinned `tools/edda.exe` binary (SHA-256
`0ad2b0c5e236e1cd078f2de2c0bd2f3a171567163caa2574dd030bc9f4b55833` — recorded,
never committed), and no pycache/provider logs.

## Repeatable repository-relative commands

Run from the repository root. Invariant discovered during relocation: the
harness launches `--adapter-cmd` with `cwd` set to its isolated temporary
project, so the adapter path inside `--adapter-cmd` must be **absolute**,
derived from the repo root at invocation time — a plain repo-relative path
fails with `can't open file`. Replace `<EDDA_BIN>` with a pinned `edda`
binary (frozen evidence used the SHA-256 above; do not assume the original
cleanroom path still exists).

Reference command (bash / Git Bash; expect exit 0, `contract_violations: 0`,
verdict `CONFORMANT (within documented gaps)`):

```bash
REPO=$(cygpath -m "$PWD" 2>/dev/null || pwd)
python tests/adapter-conformance/harness/conformance.py \
  --edda <EDDA_BIN> \
  --adapter-cmd "python $REPO/tests/adapter-conformance/reference/edda_reference_adapter.py" \
  --skip-launcher \
  --out tests/adapter-conformance/reference/reference-report.relocated.json
```

Negative control (same command, `control_stub.py` as adapter; expect exit 4
with `H-INJECT-START, H-INJECT-BUDGET, H-LEDGER-APPEND, H-HEARTBEAT`):

```bash
python tests/adapter-conformance/harness/conformance.py \
  --edda <EDDA_BIN> \
  --adapter-cmd "python $REPO/tests/adapter-conformance/reference/control_stub.py" \
  --skip-launcher \
  --out tests/adapter-conformance/reference/negative-control-report.relocated.json
```

Own tests (expect `Ran 9 tests ... OK`):

```bash
cd tests/adapter-conformance/reference
EDDA_BIN=<EDDA_BIN> python -m unittest test_reference_adapter -v
```

## Relocated-command smoke (RAN, 2026-09-04, task56)

One relocation smoke was required and run: relocation changes launch behavior
(the `--adapter-cmd` cwd invariant above and the test file's `EDDA_BIN` default
path). With the pinned binary used in place (not committed):

- Reference harness command (repo-relative pattern above): exit 0,
  `contract_violations: 0`, `CONFORMANT (within documented gaps)` — 8/8 MUST
  PASS, H-REDACT-STORE / H-PROMPT-DEDUP / H-NUDGE-RATE PASS, H-END-CLEANUP and
  H-PRETOOL-IDENTITY SKIP — identical to the frozen cleanroom result.
- Own tests relocated: 9/9 OK.
- Syntax gates: `python -m py_compile` on all three relocated `.py` files.

## Cost correction (integrator summary; original receipt untouched)

The original handoff estimated "≈ 40 minutes of agent work". The controller
observed helper start 11:10 UTC and completion ≈ 11:20 UTC (~10 minutes);
observed cost `usd 0.025800885`. The raw historical receipt above is preserved
as written; this correction does not alter authorship or attestation. The
post-completion pi ingestion failure was expected (the cleanroom has no
`.edda/`); the controller recorded the task52 receipt centrally.

## Scope limits (unchanged)

This integration is not a new reference implementation or a new source-blind
claim: it delivers the completed fresh proof. The five-bridge binary matrix
above, including the [#869](https://github.com/fagemx/edda/issues/869) and
[#870](https://github.com/fagemx/edda/issues/870) gaps, is unchanged; source
provenance remains limited and no claim is made that all five bridges' MUST
checks pass. #609 remains merged design only (no production sign API); no
signature is produced or claimed.
