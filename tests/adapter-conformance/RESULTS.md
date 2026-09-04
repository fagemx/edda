# GH-610 adapter conformance result matrix

## Task66 frozen evidence

This repair report is bound to the commit that contains it.  These are
content SHA-256 values (not Git object IDs); `repair-report-task66.json` is the
non-self-referential report artifact, so its digest is meaningful and
repeatable.

| Input / report artifact | SHA-256 |
|---|---|
| `../../docs/guides/writing-a-bridge.md` | `a6231f273f44601aee46de9903b83c564695c221c13ccd8906e8063cf1647184` |
| `harness/conformance.py` | `24a89c4d95b286862d269e3dfb664f3fae7ee3a0dfa123136bc85977f7474caa` |
| `fixtures/fake_launcher.py` | `c883d95fbf7ac7be6d0f0f1a23fc4ecf94a6839cdbeebccc234684ca8b7ee61f` |
| `reference/repair-report-task66.json` | `2841a8faa96fffe9630cd315dcd8feb60880333e617119bd0bd7a865e87b63f8` |

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
