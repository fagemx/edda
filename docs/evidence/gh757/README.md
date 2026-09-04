# GH-757 parallel test isolation receipt

Source SHA: `16e5e46574424caf99b78d615583141a9dcc9334`.

Twenty consecutive `cargo test -p edda-bridge-claude` runs passed on Windows,
each with **638 passed, 0 failed**, using default parallelism. The sequence
took 443.651 seconds including Cargo overhead. `RUST_TEST_THREADS` was unset;
there was no test filter, thread-count argument, or retry inside the sequence.
The capture checked that the source SHA and clean working tree remained fixed.

`manifest.json` records the toolchain, assigned worker-1 lane, every exit code,
test summary, duration, stdout/stderr SHA-256, and fallback-store snapshot.
`raw-evidence.zip` contains the byte-preserved raw logs and manifests:

- `successful-attempt/`: the complete twenty-run sequence.
- `first-attempt-and-l0/`: the prior failed sequence and CLI/Clippy receipts.
- `repair-gates/`: the repair checks, including intermediate failures and the
  final successful full indexed file-length check.

Verify the archive against `SHA256SUMS`, then verify the twenty raw stdout and
stderr entries against `manifest.json`. `run20.py` is the capture harness;
its repository and lane constants record this workstation's paths and must
be adapted when reproducing elsewhere. Do not overwrite an existing manifest.

## Isolation boundary

Every run received a private process fallback `EDDA_STORE_ROOT` containing a
sentinel. Its complete file/hash snapshot was unchanged after every run.
Per-test stores resolve through thread-local RAII guards; bridge configuration
overrides also avoid process-global environment mutation. Background SessionEnd
closures explicitly install the captured test-store root. The CLI child-process
contract exercises an explicit child root independently of the parent's guard.

The snapshot proves no writes reached the controlled fallback store. It is not
a snapshot of the live operator store, which other sessions may legitimately
modify. Static inspection found no executable process-global `set_var` or
`remove_var` in bridge/store sources. Production env/default lookup remains
the fallback when no test override is installed.

## Earlier failure and gate reuse

At `2ffa58509fc3c876213f30c30237b87d7455a960`, the first sequence stopped on
run 2: 637 tests passed and one peer-render equivalence assertion compared
`2s ago` with `3s ago`. Both runs left the fallback unchanged. The repair
normalizes only the displayed seconds-age field in the two equivalence tests.
Complete test modules/functions were extracted to satisfy file-length ceilings;
the final suite still contains 638 tests.

READ: store's 45 passing tests (worker receipt) and full CLI tests at `2ffa585`,
plus touched-crate Clippy; the later source change touches bridge tests only.
The CLI raw log includes the child-process isolation contract.
RAN: final bridge Clippy, formatting, the focused equivalence tests, full
indexed file-length validation, and the twenty full bridge runs above.
The evidence-only commit reuses these source gates; exact-head CI is recorded
on the PR. No local workspace-wide Cargo gate was run.

## GH-731 and GH-734

Merged [PR #719](https://github.com/fagemx/edda/pull/719), squash
`4075ab53063437bf12a04b3be7043490ce09cbc0`, addressed bounded workspace
discovery and anchored fixtures. This change completes their remaining shared
store/environment race. The CLI receipt covers the named reconcile/verify
cases; the twenty bridge runs cover heartbeat age, idempotency, audit log,
draft storage, and workspace-render fixtures. The ledger's former unbounded
discovery test was replaced by the bounded-discovery coverage in #719.
