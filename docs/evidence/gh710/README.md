# GH-710: captured hook stdout

This directory supplies the missing observation from GH-678 / PR #698.
On 2026-09-04 the installed `edda 0.4.0` executable was invoked through its
real `edda hook claude` stdin/stdout interface in an isolated scratch Git
repository. Two session IDs were started through `SessionStart`; peer
discovery reported both as live. These are controlled hook invocations with
empty transcript files, not a transcript of an interactive Claude conversation.

The executable SHA-256 is
`3b930e658613ea2e6f3e0130bd41d90a4751a8aed52444be16fb943e5eb0678e`.
Its version output does not identify a source commit, so this capture makes
no claim that the binary was rebuilt from this evidence-only branch.

## Raw observation

Every file under `captured/` is an unmodified subprocess stream or the exact
stdin sent to it. Empty stdout files are deliberately committed as zero-byte
files. `manifest.json` records commands, exit codes, timestamps, byte counts,
and stdout hashes; the streams themselves are the evidence.

| Call | Raw stdout | Observed bytes |
| --- | --- | ---: |
| Initial full coordination injection | [04-first-contact.stdout](captured/04-first-contact.stdout) | 804 |
| Lightweight peer baseline | [05-baseline.stdout](captured/05-baseline.stdout) | 565 |
| Same session, unchanged peer | [06-dedup.stdout](captured/06-dedup.stdout) | 0 |
| Same session, unchanged peer again | [07-dedup.stdout](captured/07-dedup.stdout) | 0 |
| Peer label changed through heartbeat CLI | [09-changed.stdout](captured/09-changed.stdout) | 577 |
| Both live sessions | [10-peers.stdout](captured/10-peers.stdout) | 787 |

All twelve commands exited zero. The baseline contains `## Peers (1 active)`.
The dedup calls are separated by two-second waits; the subsequent changed
peer output contains `changed-peer`. This control checks that the empty
result is conditional on unchanged context rather than a disabled hook.

## Reproduce

From the repository root, using Python 3 and an installed edda executable:

```powershell
python docs/evidence/gh710/capture.py --edda edda --output C:/scratch/gh710-new-capture
```

The output directory must not exist. The script creates a new scratch repo
and store, clears inherited `EDDA_*` and `GIT_*` variables in child processes,
and disables automatic digest. It never redirects the parent process or
uses the operator's real store. The scratch runtime is retained at the path
printed by the script and recorded in the manifest; the committed capture
remains usable even after that runtime is removed. The script asserts the
nonempty baseline, two empty calls, and changed-peer reinjection.

No durable hook-log facility was added. This evidence closes the missing
captured-output criterion of GH-710; it does not retroactively recover the
missing temporary files cited by PR #698.
