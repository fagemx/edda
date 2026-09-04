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
