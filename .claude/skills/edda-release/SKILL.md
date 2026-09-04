---
name: edda-release
description: "Release Edda across crates.io, GitHub assets, install.sh, and Homebrew without channel drift"
---

# Edda Release

You are Edda's release operator. A release is complete only when every install
path advertised in `README.md` gives users the intended version and its
release-critical commands. A GitHub Release alone is not success.

Since GH-648 (PR #847) the repository automates crates.io publication inside
`.github/workflows/release.yml`: pushing the `v<VERSION>` tag makes CI publish
every workspace crate in dependency order, verify registry provenance against
the tag SHA, and only then create the GitHub Release. Your job shifts from
uploading crates to proving the release **before** the tag push and proving the
public channels **after** CI is green.

## Usage

```text
edda-release preflight <version>
edda-release publish <version>
edda-release verify <version>
edda-release recover <version>
```

Parse `args` as an operation plus a bare semantic version such as `0.4.1`.
Reject a leading `v` in the version argument; derive the tag as `v<version>`.

- `preflight`: read-only release readiness and exact-SHA package proof
- `publish`: run the preflight, push the tag, watch CI, then prove every
  advertised channel
- `verify`: read-only public consumer canaries and parity report
- `recover`: inspect a partial release, then resume only missing safe steps

If the operation or version is missing, ask for it. Never guess a version.

## Non-negotiable invariant

The public contract is the `README.md` Install section. At present it advertises
four surfaces: `install.sh`, Homebrew, crates.io, and GitHub release assets.
Treat any version or behavior mismatch among them as a release blocker.

For crates.io, “published” means all publishable workspace crates at the exact
version are visible, unyanked, carry the frozen tag SHA in
`.cargo_vcs_info.json`, and a plain unversioned `cargo install edda` works.

The crates.io-before-GitHub-Release ordering is enforced by CI: the
`create-release` job runs `python3 scripts/publish-crates.py verify --tag` and
fails unless every publishable crate exists unyanked at the tag version. Do not
re-implement that gate manually — verify it ran.

## Authority boundary

- `preflight` and `verify` are read-only except for disposable build/canary
  directories.
- An explicit `publish <version>` request authorizes pushing the
  `v<VERSION>` tag — which triggers CI publication to crates.io and the
  release workflow — plus the follow-up public canaries.
- It does not authorize yanking a crate, deleting or replacing a tag/release,
  force-pushing, editing or creating repository secrets, copying credentials,
  or merging a PR. Ask separately before any of those actions.
- Never print or read credential contents. Check only whether the
  `CARGO_REGISTRY_TOKEN` secret is configured and whether Cargo authentication
  is available locally.

## Common ground truth

### Step 1: Read repository policy and distribution reality

Read `AGENTS.md` and `.claude/CLAUDE.md` completely, then inspect the actual
install promises and release automation:

```bash
rg -n "cargo install|brew install|releases|install.sh" README.md
git status --short --branch
git fetch origin main --tags
git rev-parse HEAD
git ls-remote --tags origin "refs/tags/v<VERSION>"
gh release view "v<VERSION>" --repo fagemx/edda
gh secret list --repo fagemx/edda
```

`gh secret list` must show `CARGO_REGISTRY_TOKEN`. Without that secret,
`prepare-crates` skips crates publication **and** every downstream GitHub
Release job while the workflow run still looks green — a silent no-op release.
Treat an absent secret as BLOCKED before pushing anything.

Preserve unrelated changes. Release from a clean commit on current `origin/main`
unless the operator explicitly names another full SHA.

### Step 2: Freeze version, SHA, docs, and verification evidence

Require all workspace packages, `Cargo.lock`, `CHANGELOG.md`, and the CLI docs
to describe the same version. `release.yml` extracts release notes from a
CHANGELOG heading that starts exactly with `## [<VERSION>]` — a missing section
fails `create-release` after publication and cannot be repaired without moving
the tag, so it must exist before the push.

```bash
rg -n '^version = "' Cargo.toml          # workspace version == <VERSION>
rg -n "^## \[<VERSION>\]" CHANGELOG.md    # release-notes section exists
EDDA_BIN=<built-edda-binary> bash scripts/check-cli-docs.sh
```

- `check-cli-docs.sh` (GH-650/GH-795) is the CLI reference drift gate: it
  verifies every verb and long flag in `docs/reference/cli.md` against the
  built binary. The old `check_cli_reference.py` no longer exists.
- Package-version parity against the tag is enforced mechanically by
  `publish-crates.py plan` in Step 3, once the local tag exists.
- Follow the L0/L1/L2 verification ladder in `.claude/CLAUDE.md`. Reuse a valid
  L1 receipt and exact-head CI for the frozen full SHA. Do not rerun the full
  workspace merely to feel safer; run only uncovered focused checks and state
  why.

### Step 3: Prove packaging, then create a local-only immutable release point

Mirror the `publish-crates-check.yml` dry-run gate locally:

```bash
cargo package --workspace --locked --no-verify
cargo publish --dry-run --workspace --locked
```

Optionally verify the local archives would carry the tag SHA (skill-local
helper, kept only for this check):

```bash
python scripts/crates_release_plan.py \
  --version <VERSION> --package-dir target/package --expected-sha <FULL_SHA>
```

Only after packaging is green, create an annotated tag locally. Do not push it,
then run the plan gate — it requires HEAD to be the tag's exact commit on a
clean tree, validates the tag shape, and prints the dependency-first publish
order (`python` instead of `python3` on Windows):

```bash
git tag -a "v<VERSION>" <FULL_SHA> -m "Release v<VERSION>"
git rev-list -n 1 "v<VERSION>"
python3 scripts/publish-crates.py plan --tag "v<VERSION>"
python3 scripts/publish-crates-test.py
```

Create a detached worktree from that tag for any local re-verification (for
example `publish-crates.py verify` after CI, which requires HEAD == tag on a
clean tree). Use the worktree's default `target/`; do not invent timestamped
Cargo target lanes.

```bash
git worktree add --detach <TEMP_WORKTREE> "v<VERSION>"
```

`preflight` stops here and reports evidence. Remove only the exact disposable
worktree after verifying its resolved path.

## Operation: publish

### Step 1: Push the tag — the single mutation point

The tag push is what publishes crates.io. Everything before it (preflight) is
the only chance to catch package defects; publication itself is irreversible.

```bash
git push origin "refs/tags/v<VERSION>"
gh run list --workflow release.yml --branch "v<VERSION>" --limit 5
gh run watch <RUN_ID> --exit-status
```

### Step 2: Verify every CI job did its job

Require all five jobs green — a green run with skipped jobs is BLOCKED:

| Job | Requirement |
|---|---|
| `prepare-crates` | `enabled=true` (secret present); tag/plan validation passed |
| `publish-crates` | `SUCCESS: all <N> workspace versions verified` |
| `create-release` | parity gate `verify --tag` passed; draft release created from the CHANGELOG section |
| `build-release` | all five platform archives + `.sha256` uploaded to the draft |
| `publish-release` | exact 10-asset set, no empty assets, checksums verified, native binary canary without credentials, release published `--latest` |

Never repair a failed run by moving the tag.

### Step 3: Prove the README Cargo path

Use a fresh Cargo home and install root. Run exactly the public command without
`--version`, `--git`, `--path`, or `--locked`:

```text
cargo install edda --root <FRESH_ROOT>
<FRESH_ROOT>/bin/edda --version
<FRESH_ROOT>/bin/edda dispatch --help
<FRESH_ROOT>/bin/edda verdict --help
```

The version must equal `<VERSION>`. On Windows, retry once with `--jobs 1` only
when rustc itself exits with an OS crash/resource signature such as
`0xc0000005`. A Rust compiler diagnostic, test failure, missing native library,
or wrong CLI behavior is a product failure and must not be relabeled flaky.

Do not declare DONE until this unversioned install canary passes.

### Step 4: Prove install.sh and Homebrew

Run the one-line installer twice in disposable directories: once pinned
(`--version v<VERSION>`) and once through its default “latest” path. Both
binaries must report the same version and expose `dispatch` and `verdict`.

After the release is published, generate the Homebrew formula from the release
checksums:

```bash
./scripts/update-homebrew.sh <VERSION> <HOME_BREW_TAP_CHECKOUT>
brew audit --strict fagemx/tap/edda
brew reinstall fagemx/tap/edda
edda --version
edda dispatch --help
edda verdict --help
```

Commit and push the tap formula only after its diff names the intended version
and hashes. If no macOS/Linux Homebrew verifier is available, the release cannot
claim full `DONE`; report `DONE_WITH_CONCERNS` with the missing public canary.

### Step 5: Record the release receipt

Pin every claim to the tag's full SHA. Use this exact output structure:

```markdown
## Edda Release v<VERSION>

Status: DONE | DONE_WITH_CONCERNS | BLOCKED
Tag SHA: <40-hex>

| Surface | Evidence | Result |
|---|---|---|
| Workspace packages | <N>, version parity (plan ORDER) | PASS/FAIL |
| Preflight package proof | dry-run publish + local provenance SHA | PASS/FAIL |
| Release workflow | run URL, all 5 jobs green, none skipped | PASS/FAIL |
| crates.io | <N> visible and unyanked | PASS/FAIL |
| crates.io provenance | CI VERIFIED lines / verify --tag SHA match | PASS/FAIL |
| cargo install edda | resolved version + critical help canaries | PASS/FAIL |
| GitHub Release | isLatest, 10 assets, checksums, native canary | PASS/FAIL |
| install.sh | pinned + latest version canaries | PASS/FAIL |
| Homebrew | formula commit + install canary | PASS/FAIL/NOT RUN |

Failures/retries: <exact command, classification, result>
Remaining action: <none or one concrete next action>
```

## Operation: verify

Run the public half of the workflow without changing remote state:

1. From the tag worktree, `python3 scripts/publish-crates.py verify --tag
   "v<VERSION>"` — read-only all-crate registry and provenance proof.
2. Repeat the fresh unversioned Cargo canary.
3. Inspect the exact GitHub Actions run (all five jobs, none skipped), release
   assets, and checksums; require `isLatest=true`.
4. Repeat pinned/latest `install.sh` and Homebrew canaries.
5. Emit the release receipt. Do not say DONE when any README channel is stale.

## Operation: recover

First inventory immutable public state; do not repeat successful mutations.
CI publication is idempotent — `publish-crates.py publish` prints `NO-OP` for
every already-verified crate — so resuming is usually just rerunning failed
jobs:

```bash
git ls-remote --tags origin "refs/tags/v<VERSION>"
gh run list --workflow release.yml --branch "v<VERSION>" --limit 5
gh run view <RUN_ID> --json jobs,conclusion
gh release view "v<VERSION>" --repo fagemx/edda --json isDraft,isLatest,assets
# from the detached tag worktree:
python3 scripts/publish-crates.py verify --tag "v<VERSION>"
```

Use the decision table below, then resume at the earliest missing safe step.

| Observed state | Action |
|---|---|
| No tag pushed, nothing published | Return to preflight |
| `prepare-crates` reports `enabled=false` | BLOCKED until the operator configures `CARGO_REGISTRY_TOKEN`, then rerun failed jobs |
| Tag pushed, run failed at `publish-crates` | Rerun failed jobs (`gh run rerun <id> --failed`); verified crates are NO-OP |
| Run failed at `create-release` (parity) | Inspect registry with `verify --tag`; rerun only after the cause is gone |
| Run failed at `create-release` (CHANGELOG section missing) | BLOCKED; repair means a new commit and tag — operator decision, never move the tag |
| Run failed at `build-release`/`publish-release` | Rerun failed jobs; draft uploads are clobber-safe and re-verified |
| Registry provenance differs from tag SHA | BLOCKED; do not yank or move tag without operator decision |
| Public release exists but a channel is stale | Repair that channel without changing immutable crate/tag identity |

Registry propagation is handled inside `publish-crates.py` (bounded retries and
a 12 × 10 s per-crate poll), so a missing-crate failure there is real, not
lag. After three failures with the same stable cause, stop and report
`BLOCKED` with the exact attempts.

## Anti-patterns

1. **GitHub-only success**: assets existing does not satisfy `cargo install edda`.
2. **Push tag, then hope**: preflight is the only gate before an irreversible
   publication; CI publishes and verifies, but a bad package at the tag is
   unfixable without a new release identity.
3. **Skipped-jobs green run**: `prepare-crates` with no token skips publication
   and the release silently; a green run means nothing unless every job ran.
4. **Publish from a moving checkout**: publication happens in CI from the tag;
   local re-verification only from the exact detached tag worktree.
5. **Root crate only**: all publishable workspace crates must reach crates.io in
   dependency order — enforced by `publish-crates.py` and the parity gate.
6. **Local artifact trust**: verify downloaded registry provenance and public
   install behavior, not only `target/release/edda`.
7. **Retry laundering**: distinguish registry delay or rustc OS crash from a
   deterministic product/compiler failure.
8. **Credential improvisation**: never echo tokens or edit repository secrets
   without explicit authorization.
9. **Destructive recovery**: never yank, delete, recreate, or force-move public
   release identity as an automatic recovery step.
10. **Manual publication drift**: do not run `cargo publish` locally; CI owns
    ordering, provenance, and verification. Local helpers only prove packaging
    and verify public state.

## References

- crates.io publication mechanics: `references/crates-io.md`
- release automation: `.github/workflows/release.yml` (five jobs, tag-triggered)
- publication dry-run gate: `.github/workflows/publish-crates-check.yml`
- publish plan/publish/verify script: `scripts/publish-crates.py` (+ its test
  `scripts/publish-crates-test.py`)
- consumer promises: `README.md`
- Homebrew formula generator: `scripts/update-homebrew.sh`
- CLI reference drift gate: `scripts/check-cli-docs.sh` (GH-650/GH-795)
- verification ladder and build lanes: `.claude/CLAUDE.md`
