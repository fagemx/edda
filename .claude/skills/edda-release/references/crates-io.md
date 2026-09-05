# crates.io release protocol

This reference contains the detailed mechanics behind the crates.io gate in
`edda-release`. Read it before `publish` or `recover` touches anything.

Since GH-648 (PR #847) crates.io publication is automated in CI by
`scripts/publish-crates.py`, a tracked script tested on every relevant PR by
`.github/workflows/publish-crates-check.yml`. The release operator no longer
uploads crates manually; the operator proves readiness before the tag push and
proves public state after CI.

## Why the root crate is not enough

The `edda` binary depends on path crates in the workspace. crates.io resolves
those as registry dependencies, so each publishable internal dependency must be
available before a dependant can be accepted. Publishing only `edda` either
fails or leaves `cargo install edda` resolving an older release.

The script derives the order from `cargo metadata --locked --no-deps`, which is
the source of truth:

- packages with `publish = false` / `publish = []` are excluded;
- every workspace package version must equal the tag version;
- normal and build path-dependency edges (including target/optional ones) must
  precede their reader; unversioned dev edges are dropped by Cargo and ignored;
- the `edda` CLI is forced last by construction, not alphabetically;
- cycles or a crate depending on `edda` abort the plan.

The CLI is also deliberately published last so that a plain
`cargo install edda` immediately after the run cannot resolve a stale root.

## The three modes

All modes share the same entry gate: the tag must match `vMAJOR.MINOR.PATCH`,
`HEAD` must be the tag's exact commit (full 40-hex SHA), and the tree must have
no tracked modifications.

```bash
python3 scripts/publish-crates.py plan    --tag "v<VERSION>"   # validate + print ORDER
python3 scripts/publish-crates.py publish --tag "v<VERSION>"   # CI only: upload loop
python3 scripts/publish-crates.py verify  --tag "v<VERSION>"   # read-only all-crate proof
```

- `plan` — validation only; prints `ORDER: a -> b -> ...`. This is the
  preflight ordering gate and runs locally.
- `publish` — requires `CARGO_REGISTRY_TOKEN` in the environment; refuses to
  run without it. For each package in order: skip (`NO-OP`) if the registry
  already has a verified match, otherwise `cargo publish --locked --registry
  crates-io -p <name>`, then poll registry verification (12 × 10 s). A timed-out
  upload or an already-uploaded race is judged by registry evidence, not by
  Cargo's error wording. After all uploads it rechecks every version to catch
  yanks and stragglers. Idempotent and resumable by design.
- `verify` — read-only: every package must pass registry verification.
  This is the parity gate CI runs in `create-release` before any GitHub Release
  exists, and the operator's local inventory tool in `verify`/`recover` (run it
  from the detached tag worktree so the HEAD gate is satisfied).

## What "verified" means

For every publishable workspace crate the script requires, from the official
registry:

- the exact version endpoint exists (`crates.io/api/v1/crates/<name>/<version>`);
- `yanked` is false and the returned identity matches name and version;
- the downloaded `.crate` archive's SHA256 equals the registry checksum;
- the archive contains `.cargo_vcs_info.json` whose `git.sha1` equals the
  frozen tag SHA and whose `dirty` flag is false;
- `path_in_vcs` matches the crate's real workspace-relative path.

Archive members are read in memory; registry-controlled paths are never
extracted to disk. Each verified crate prints `VERIFIED <name>@<version>
source=<sha>`.

## Local preflight proof

Before the tag push, mirror CI's dry-run gate:

```bash
cargo package --workspace --locked --no-verify
cargo publish --dry-run --workspace --locked
```

`--no-verify` is allowed only for packaging/dry-run preflight: it avoids
rebuilding packaged crates whose new internal dependency versions do not exist
on the registry yet. It is never allowed on a real `cargo publish` — and the
operator does not run real publishes locally at all.

Optionally, the skill-local helper `scripts/crates_release_plan.py` (shipped
with this skill) can check that the
local archives in `target/package` would carry the tag SHA:

```bash
python scripts/crates_release_plan.py \
  --version <VERSION> --package-dir target/package --expected-sha <FULL_SHA>
```

That helper is legacy: its `--commands`/manual-upload modes are superseded by
CI publication and must not be used to upload anything. Only its local
`--package-dir` provenance check remains meaningful. The canonical ordering and
registry mechanics live in the tracked `scripts/publish-crates.py`.

## Authentication safety

CI publishes with the repository secret `CARGO_REGISTRY_TOKEN`. If the secret
is absent, `prepare-crates` skips publication **and** every downstream GitHub
Release job while the run still reports green — treat that as BLOCKED, not as
success. Check secret presence with `gh secret list --repo fagemx/edda`; only
the operator may create or edit secrets.

Locally, accept either Cargo's normal credential provider/configuration or a
`CARGO_REGISTRY_TOKEN` already supplied by the operator environment. Check only
presence and ownership:

```bash
cargo owner --list edda
```

Never display credential files or token values. Never copy a developer token to
repository files, CI logs, or GitHub secrets without a separate explicit request.

## Recovery

crates.io versions cannot be overwritten, but the CI publication loop is
idempotent: verified crates are `NO-OP`, missing crates are uploaded, and the
final recheck proves the whole set. Recovery is therefore:

1. Inventory with `verify --tag` from the tag worktree, plus
   `gh run view <id> --json jobs` and `gh release view --json isDraft,isLatest,assets`.
2. Rerun only the failed workflow jobs (`gh run rerun <id> --failed`).
3. Draft-release reuse and clobber-safe asset uploads make build reruns safe;
   `publish-release` re-verifies the exact 10-asset set and checksums before
   flipping the release public.
4. Re-run the all-crate `verify --tag` proof and the plain public install
   canary after recovery.

If an existing crate has the wrong source SHA, stop. Automatic yanking does not
repair provenance and may break dependants or users.

## Public canary classification

| Failure | Classification | Response |
|---|---|---|
| crates.io 404 during the script's own poll | handled internally | bounded retries + 120 s per-crate poll; a final failure is real |
| crates.io/API network errors | environment | bounded retry, keep release state unchanged |
| Windows rustc exits `0xc0000005` | environment candidate | retry once with `--jobs 1` |
| Cargo compiler diagnostic | product/package | stop and fix; do not call flaky |
| `prepare-crates` enabled=false | missing secret | BLOCKED; operator configures the secret, then rerun |
| Installed version is older | channel drift | stop; investigate channel before declaring DONE |
| `dispatch` or `verdict` help missing | release contents wrong | stop; BLOCKED with exact evidence |
| Downloaded package SHA differs | provenance violation | block and request operator decision |
| CHANGELOG section missing at tag | source defect | BLOCKED; repair needs a new tag — operator decision, never move the tag |
