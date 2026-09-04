# GH-693 — D1 claim-subject evidence

This evidence distinguishes the historical installed runtime from the source
claim that it was incorrectly used to judge.

## Historical runtime

The preserved official Windows `edda 0.3.0` artifact has SHA-256:

```text
a303e1a5042c6a112c430920e1a7bd0257f3dccc1a7dc9a44909aeef4ae33214
```

The checked artifact reports `edda 0.3.0`, and its `edda dispatch --help`
probe exits `2`: that release does not expose a `dispatch` command. This is the
actual official artifact, not a rebuilt old workspace. The earlier development
0.3 binary used during PR #684 instead listed eight pre-GH-574 dispatch flags.
Those two observations differ and are not interchangeable.

## Same-input regression

The source side is PR #684 commit
`f94960306377e01e5e395704bc0876ec1fb4257b`, whose document cites
`crates/edda-cli/src/cmd_dispatch.rs` at lines 58, 64, 68, 77, 83, and 87. The historical command probe is a
runtime observation; the citation is a source claim. Under the old D1 wording,
the unavailable/stale runtime was sufficient to report a false P0 against that
source claim. Under the GH-693 routing, the runtime result is recorded but the
claim is checked from the cited source:

```sh
git show f94960306377e01e5e395704bc0876ec1fb4257b:crates/edda-cli/src/cmd_dispatch.rs
```

That source contains the six cited GH-574 options: `--permission-mode`,
`--model`, `--thinking`, `--tools`, `--exclude-tools`, and `--session-dir`.
The new rule therefore produces no P0 for the same source citation, while it
would still report a P1 if those cited source lines were absent.

The current workstation's installed `edda --version` is recorded separately
when a review begins. It is diagnostic context and never substitutes for a
SHA-pinned source read.

## Reproduction

Run `verify.ps1` with the official release ZIP. It verifies the archive digest,
executes the official binary's version and missing-dispatch probes, and asserts
that the full PR #684 source SHA contains every cited field. The script fails
on any unexpected runtime result or missing source field; it does not print a
hardcoded pass.
