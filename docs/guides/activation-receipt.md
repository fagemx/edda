# Activation and the revision receipt

One merged revision has to travel through four carriers before it is real on a
machine, and each carrier historically had its own install step:

| Carrier | Surface | Installed identity |
|---|---|---|
| repository | `C:/ai_agent/edda` `main` | 40-hex merge revision |
| shipping binary | `edda` on `PATH` | `edda --version` build commit (12-hex) |
| Pi package | `@edda/pi-session-channel` | digest-addressed runtime release id (content, not semver) |
| agent-manager | the local console service | `release.json` configured revision + the running owner |

The failure this guide removes is concrete: a green, merged PR could leave the
installed `edda`, the global `edda-pi` package, the agent-manager release metadata
and the running service on **different** revisions, and the Pi package semantic
version stayed `0.8.0` even when its installed **content** changed. Controllers had
to pack/install/restart by hand to make a merge real.

## The read-only receipt

`edda-pi` carries the receipt in `integrations/pi/activation-receipt.mjs`. It is
read-only: it writes nothing anywhere, starts nothing, calls no model, and never
prints a credential.

From a checkout (the installed `cli.mjs` registration is sequenced behind a sibling
controller, so the source entry is the runnable one today):

```text
node integrations/pi/activation-receipt.mjs --json
node integrations/pi/activation-receipt.mjs --check     # exit 2 unless coherent
```

The receipt distinguishes exactly the four revisions and reports a fail-closed
`coherence`:

- `coherent` — all four legs observed and consistent;
- `drift` — at least one observed inconsistency (`edda_revision_mismatch`,
  `pi_content_drift`, `manager_configured_revision_mismatch`, `manager_not_healthy`,
  `manager_absent_owner`);
- `partial` — no inconsistency found, but at least one leg could not be observed;
- `unknown` — no leg could be observed at all.

The Pi leg is the part that semver cannot answer: `installedReleaseId` is the
content id of the installed package directory and `repoReleaseId` is the same id
computed for the checkout's `integrations/pi`, both derived exactly as
`installRuntime()` derives the digest-addressed release. Old releases are listed in
`pinnedReleaseIds` and are never hidden or deleted. The receipt never claims a
running session was upgraded in place; per-run pinned releases stay visible through
`edda-pi run-status`.

## The activation route

`scripts/activation/activate-revision.sh` is the one supported route. It adds no
installer, state store, scheduler or daemon — it sequences the existing primitives
and gates on the receipt:

```text
sh scripts/activation/activate-revision.sh --dry-run
sh scripts/activation/activate-revision.sh --repo C:/ai_agent/edda
```

Steps, in order, each also valid to run alone:

1. **shipping binary** — `cargo install --path crates/edda-cli --force` (or copy a
   verified prebuilt artifact with `--edda-from`), then verify `edda --version`
   matches the target revision. A running process holding the old file installs by
   copy instead of rebuild (GH #1133).
2. **Pi package** — `npm pack ./integrations/pi --ignore-scripts`,
   `npm install --global --ignore-scripts <tarball>`, `edda-pi runtime-install`,
   then verify the installed release id equals the checkout's.
3. **agent-manager** — `scripts/activation/manager-release.mjs` builds the console,
   stops only the proven-current owned instance through the console's own `stop`
   (never a PID kill), writes `release.json` with a timestamped backup, starts it
   again, and reports status.
4. **receipt gate** — print the receipt and exit `2` unless it is `coherent`.

Safety: `--dry-run` mutates nothing; a dirty tracked tree or a `HEAD` that is not
the requested revision is refused; old runtime releases and release metadata
backups are kept; unrelated running work is never stopped or overwritten.

## Relationship to GH #1133

GH #1133 owns the installed-binary freshness check ("nothing checks the PATH binary
is new enough for the verb"). The receipt's `edda` leg is that same measurement, not
a second mechanism: it compares `edda --version` to the checkout revision and fails
closed. This guide does not add a per-verb probe.
