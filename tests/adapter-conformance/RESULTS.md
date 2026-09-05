# GH-610 adapter conformance result matrix

## Task66 frozen evidence

Every digest below is SHA-256 over the **exact committed Git blob bytes** of
the artifact — the byte stream `git show <rev>:<path>` (equivalently
`git cat-file blob <rev>:<path>`) emits.  They are content digests, not Git
object IDs.  Blob bytes are what the repository actually stores, so the values
are independent of `core.autocrlf`, EOL attributes, and checkout platform;
no checkout-rendered byte stream is hashed.  These values supersede the
earlier digests recorded at this head, which mixed CRLF-rendered checkout
bytes with raw blob bytes and were not reproducible by any single procedure.

`repair-report-task66.json` is separately generated and immutable: the harness
writes it once to a fresh output file, it is committed unmodified, and it
contains no digest of itself or of this file.  This table does not list
`RESULTS.md` itself, so the binding is acyclic; each digest is bound to the
stable committed blob at the frozen head and is meaningful and repeatable.
(The harness embeds a fresh random store-isolation nonce in each run's
evidence strings, so a rerun matches the committed report in verdict and
results but not byte-for-byte; the commit — not regeneration — is what fixes
the bound bytes.)

Frozen evidence head: `197b7af18065033071a6eb1c0fc1c22a50c30e52`

| Bound artifact (repo-relative) | SHA-256 of committed blob bytes |
|---|---|
| `docs/guides/writing-a-bridge.md` | `97acb42d7ea51d58b42551c35f260f0ed046bd687ad8fa4659aa808223d1cd89` |
| `tests/adapter-conformance/harness/conformance.py` | `6cf15434c078f8180082b8414693012a311e3fc8c11ef13ce5a4c9d287853867` |
| `tests/adapter-conformance/fixtures/fake_launcher.py` | `c883d95fbf7ac7be6d0f0f1a23fc4ecf94a6839cdbeebccc234684ca8b7ee61f` |
| `tests/adapter-conformance/reference/repair-report-task66.json` | `6f790f1fba6fb969a568b966356e47fd4ffe8bc07d1bcf23469030f8487def4e` |

Reproduce all four values with one uniform method (Git Bash on Windows ships
both tools; the commands read only repository objects and never touch the
working tree, so no checkout state can affect them):

```text
H=197b7af18065033071a6eb1c0fc1c22a50c30e52
for p in docs/guides/writing-a-bridge.md \
         tests/adapter-conformance/harness/conformance.py \
         tests/adapter-conformance/fixtures/fake_launcher.py \
         tests/adapter-conformance/reference/repair-report-task66.json; do
  git show "$H:$p" | sha256sum
done
```

The four artifacts are byte-identical at the head that carries this table
(it changes only this file), so the same commands yield these values there
too.  If any bound artifact ever changes on this branch, recompute its row
with the same procedure at the new head and update the table.

The report was produced without a provider or global configuration:

```text
python tests/adapter-conformance/harness/conformance.py --edda C:/Users/fagem/.cargo/bin/edda.exe --adapter-cmd "python <ABS_REPO>/tests/adapter-conformance/reference/edda_reference_adapter.py" --out tests/adapter-conformance/reference/repair-report-task66.json
```

It exited 0 (`contract_violations: 0`). It proves the reference profile's
malformed/unknown permissive JSON, <=300-character tiny-budget context,
exactly one session digest, private-state redaction and cleanup, and isolation
both before and after events. `L-RECEIPT-*` and `L-EXIT-CLASS-*` pass for all
three supported launcher profiles (Claude, pi, Codex), and
`L-RECEIPT-NEGATIVE` rejects an incomplete receipt. Those launcher checks use
the committed deterministic fixture; they are receipt-protocol evidence, not
an assertion about a provider-backed installed binary.

Focused portable gates:

```text
python -m unittest tests/adapter-conformance/harness/test_conformance.py -v  # 3/3 OK
EDDA_BIN=C:/Users/fagem/.cargo/bin/edda.exe python -m unittest tests/adapter-conformance/reference/test_reference_adapter.py -v  # 11/11 OK
python -m py_compile tests/adapter-conformance/harness/conformance.py tests/adapter-conformance/fixtures/fake_launcher.py tests/adapter-conformance/reference/edda_reference_adapter.py
```

## Historical five-bridge observation (preserved)

The original pinned-binary five-bridge observation remains in
`results-sdk-binary-0ad2b0c5.json` and its stdout receipt. It used a different,
unattested binary and is historical only; its documented Hermes/OpenClaw gaps
[#869](https://github.com/fagemx/edda/issues/869) and
[#870](https://github.com/fagemx/edda/issues/870) are unchanged and outside
this repair. The clean-room files under `reference/` remain preserved as the
original proof; `repair-report-task66.json` is additional repair evidence, not
a rewritten authorship claim. No `Closes #610` claim is made.
