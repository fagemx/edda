# SDK_HANDOFF — GH-611 (infra-sdk session)

Task rail: #26 (GH-611), **running** (predecessor #18 receipt landed, d-017).
Scope claimed: `sdk/*`, `docs/reference/client-contract.md`,
`.github/workflows/contract.yml`, `crates/edda-mcp/*`,
`crates/edda-ledger/src/task_actions.rs`, `crates/edda-cli/src/cmd_task.rs`,
`SDK_HANDOFF.md`.

## State

- **Spec pin**: `sdk/SPEC_PIN.json` records
  `9e3f6ddb8660e730be2cee631aa1eff7dd208a18` (PR857 candidate, frozen per
  d-017). Generated types from that pin are **committed**;
  `.github/workflows/contract.yml` regenerates and **fails on stale types**.
  No optional repo-variable skip: the job fails if the pinned sha is not
  fetchable or schemas/vectors/fixtures are missing.
- **MCP capability complete**: `edda_task_new/start/done/fail`, `edda_claim`,
  `edda_receipt`, `edda_verify` wired in `crates/edda-mcp` (`client_ops.rs` +
  tool methods). Task verbs call the shared `edda_ledger::task_actions`
  module extracted verbatim from `cmd_task.rs` (CLI updated to the same
  calls; behavior preserved incl. assignment notifications at the CLI edge).
  Claim reuses `edda_bridge_claude::peers` (MCP→Claude dependency edge only;
  no ledger→bridge cycle). Verify = `Ledger::open_existing` +
  `verify_chain_report` (`edda verify --json` payload). HTTP stays read-only.
- **Canonicalization proven, not assumed**: both SDKs implement
  serde_json/zmij semantics (integer u64/i64 exactness, f64 shortest
  round-trip, decimal-vs-exponential switch at dec_exp ∈ [-5,15], `e+NN`
  exponents, `1.0`/`-0.0`, string escape normalization, Unicode code-point
  key order). Validated against `spec/events/canonical-v1.json` AND crafted
  numeric-limit vectors AND all 32+ golden fixture event digests — in both
  languages, independently of Rust. Honest scope limits documented in
  `sdk/ts/src/canon.ts` / `canon.py` headers (arbitrary-precision integers
  beyond u64/i64 degrade to f64 exactly as serde_json does; non-finite is an
  error).
- **Cross-language contract**: `sdk/run-contract-tests.mjs` runs both suites
  against a real edda on temp repos with isolated stores
  (`EDDA_STORE_ROOT`), including the task/receipt/claim/verify flow, and
  requires structural equivalence of scenario transcripts. Local run:
  EQUIVALENT (lane binary, verifier lane).

## Gates (L0, verifier lane `C:/Users/fagem/AppData/Local/fleet-workstation/lanes/verifier`)

- `cargo clippy -p edda-ledger -p edda-mcp -p edda --all-targets` — clean
- `cargo test -p edda-ledger` (239), `-p edda-mcp` (19), `-p edda` (470+…) — green
- TS + Python golden/vector/contract suites — green; runner EQUIVALENT

## Remaining (do NOT do unilaterally)

1. **Publication BLOCKED**: npm/PyPI namespace and credentials pending user
   (no NPM_TOKEN/TWINE_PASSWORD; `@edda` / `edda-sdk` names are assumptions).
   Release files are prepared as reviewed drafts. GH-611 doneWhen includes
   the real release — this is the separate acceptance blocker, tracked, not
   silently dropped. Issue reference carries `Issue: #611`, **no Closes**.
2. `claim check` glob-intersection stays CLI-only (documented in contract
   §5); extracting it would touch `cmd_claim.rs`, outside the granted scope.
3. CI branch protection must mark the `contract` job required for the
   paths filter set — controller/owner action.
