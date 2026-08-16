# Scheduler launch manifest design

**Status:** Approved design for GH-466; not an implementation or drill receipt

**Ratified decision:** `gh466.scheduler-launch-config=machine-local-content-addressed-manifest`

**Observed blocker:** the fresh Windows lifecycle run rendered a 356-UTF-16-code-unit
`/TR`; `schtasks.exe /Create` rejected it because `/TR` may not exceed 261.

## Scope and constraints

Replace the long scheduler command with a direct Edda invocation that names one
immutable machine-local manifest. The ledger remains truth; the scheduled task
is only a one-minute doorbell.

The implementation must keep these GH-466 constraints:

- one exact `Edda-Reconcile-<32-lowercase-hex-project-id>` task;
- direct `edda.exe` and `schtasks.exe` execution, with no shell, wrapper, daemon,
  XML registration, new dependency, principal change, or task enumeration;
- canonical main-repository identity, a canonical native Codex `.exe`, and the
  existing `max-workers`, `max-attempts`, and `lease-ttl-s` semantics;
- fail closed before any file or scheduler mutation when validation or the
  `/TR` length preflight fails.

## Manifest and command

The manifest location is:

```text
<canonical edda_store::store_root()>/scheduler-launch/v1/<sha256>.json
```

`<sha256>` is the full 64-character lowercase SHA-256 of the exact manifest
bytes. Version 1 uses compact UTF-8 JSON with a fixed field order and no trailing
newline:

```json
{"schema_version":1,"project_id":"<32hex>","repo":"<canonical absolute root>","codex_bin":"<canonical absolute native .exe>","max_workers":3,"max_attempts":3,"lease_ttl_s":300}
```

There is no timestamp or mutable `current` pointer, so reinstalling identical
configuration produces the same bytes, digest, and path. The manifest contains
no credentials or session state.

The installed `/TR` is exactly this shape:

```text
"<canonical absolute edda.exe>" reconcile --scheduler-manifest "<canonical absolute manifest>"
```

`--scheduler-manifest` is hidden and is the sole configuration source on this
re-entry path. It conflicts with scheduler install/uninstall, `--repo`,
`--codex-bin`, `--run-task`, `--attempt`, and explicitly supplied reconcile
limits. It does not dispatch during install.

The renderer uses the existing Windows argument quoting rules. Before creating
a directory or file and before invoking `schtasks.exe`, install renders the full
`/TR` and requires `task_run.encode_utf16().count() <= 261`. The count excludes
the terminating NUL. A 262-code-unit command is an error naming the observed and
maximum lengths; no shortening, lossy conversion, relative path, or fallback
transport is allowed.

## Trust and schema validation

Both install and scheduled re-entry validate the same typed version-1 payload.
Re-entry additionally performs all checks before opening the ledger or launching
Codex:

1. Require an absolute Unicode manifest path with no NUL or quote.
2. Canonicalize the file and its parent. The parent must equal the canonical
   `store_root()/scheduler-launch/v1`; symlink or reparse escape is rejected.
3. Require a regular file no larger than 16 KiB and a filename exactly
   `<64-lowercase-hex>.json`.
4. Read bounded bytes, parse a `deny_unknown_fields` version-1 struct, serialize
   it back to the canonical byte form, and require byte-for-byte equality.
5. Recompute SHA-256 and require it to equal the filename. Unknown versions,
   unknown fields, duplicate/noncanonical JSON, and digest mismatch fail closed.
6. Validate `repo` through the existing authoritative `EddaPaths::find_root`
   and canonical-main-root logic. Recompute `project_id_for_root(repo)` and
   require both the payload id and exact task name to match.
7. Revalidate `codex_bin` with the existing absolute, canonical, regular native
   `.exe` checks. Parse the three numeric fields into their existing Rust types
   and apply the same reconcile semantics; the manifest must not introduce a
   second set of limits.

The trust boundary is the current user's canonical Edda store. A manifest from
another directory is rejected even when its digest is internally consistent.
Changing any field creates a different artifact rather than mutating one in
place.

## Lifecycle and cleanup

### Install and reinstall

1. Canonicalize and validate Edda, repo, Codex, project id, config, store root,
   canonical manifest bytes, prospective path, and the complete `/TR` in memory.
2. Perform the UTF-16 preflight. Until it passes, make no filesystem or scheduler
   mutation.
3. Acquire one store-local `scheduler-launch/manifest.lock`, using the existing
   Edda file-lock helper. Create the version directory and atomically publish a
   missing content-addressed file with the existing atomic-write helper. If it
   already exists, never rewrite it; accept it only after the complete trust
   checks and exact-byte comparison pass.
4. Run the unchanged exact `/Create ... /F /HRESULT` vector and then the exact
   `/Query ... /XML /HRESULT`. Success requires the query to show the rendered
   direct manifest command for the exact task.
5. Reinstalling the same configuration reuses one artifact. A successful
   changed-config reinstall retains the prior immutable artifact: a previously
   triggered invocation may still need it, and exact-task-only policy forbids a
   machine-wide reference scan.

There is no pre-delete, suffix task, task-list scan, or content-directory sweep.

### Failure

- A manifest write failure occurs before scheduler mutation.
- After a Create or post-Create Query failure, query only the exact task again.
  Remove a manifest created by this attempt only when that query proves the task
  missing or proves its strict command references a different manifest.
- If scheduler state, XML, path trust, or artifact ownership is uncertain,
  retain the artifact and report the exact operation/task plus bounded scheduler
  output. Never delete the task or a prior manifest merely to make rollback look
  clean.

### Uninstall

Query the exact task first and, when present, recover a manifest candidate only
from its bounded XML's strict direct-command shape. This narrow extraction adds
no shell, XML-registration path, or generic parser abstraction; failure to prove
the exact shape simply disables artifact deletion. Delete the exact task,
accept only the locally proved missing HRESULT race, and require an exact
post-Delete Query to show absence. After absence is proved, delete the recovered
artifact under the store lock only if it passes all path, schema, digest, and
project-id checks. Missing, malformed, or untrusted artifact state does not
block exact-task removal and is retained for manual inspection. An
already-missing task is success and performs no artifact sweep.

This is intentionally conservative: bounded orphan files are preferable to
deleting an artifact whose ownership is not proved. General garbage collection
is outside GH-466.

## Acceptance-matrix amendments

These rows replace the corresponding rows in the Task 4 verifier matrix; all
other rows remain in force.

| Row | Replacement |
|---|---|
| S2 — absolute re-entry | `/TR` contains a hidden absolute `--scheduler-manifest` path. The validated manifest contains the canonical authoritative repo root. Re-entry from an unrelated cwd targets that root; invalid manifest/repo paths fail before ledger access. |
| S4 — quoting | The single `/TR` string quotes only the canonical Edda executable and canonical manifest path. Spaces and terminal backslashes round-trip; quotes, NUL, non-Unicode input, and length over 261 UTF-16 code units are rejected. Repo and Codex paths are not repeated in `/TR`. |
| S5 — exact create argv | The ordered vector remains `/Create /SC MINUTE /MO 1 /TN <exact-name> /TR <single-direct-manifest-command> /RL LIMITED /F /HRESULT`. No principal option, shell, wrapper, XML registration, or extra scheduler option is added. |
| S14 — minimal scope | Expected product work remains `crates/edda-cli/src/cmd_reconcile.rs`, using existing `edda_store::store_root`, `lock_file`, `write_atomic`, `sha2`, Serde, and standard-library file APIs. `main.rs`, Cargo files, a generic scheduler/config layer, a new crate, and a new dependency remain out of scope unless a new reviewed scope decision proves them unavoidable. |

## Tests and gates

Implementation starts with focused failing tests for:

- the preserved 356-code-unit configuration rendering through a manifest at
  no more than 261, plus exact 261-accept and 262-reject/no-mutation boundaries;
- deterministic canonical bytes/digest/path, identical reinstall reuse, changed
  config producing a new digest, and no-replace immutability;
- unknown version/field, duplicate or noncanonical JSON, oversize input, digest
  mismatch, store-root escape, symlink/reparse escape, wrong project id, invalid
  repo, and Codex `.exe` disappearance or alias-to-non-`.exe`;
- exact `/Create`, Query, and Delete argv; unrelated-cwd re-entry; install not
  dispatching; strict uninstall-command recovery; failure retention/cleanup;
  reinstall reuse/retention; idempotent uninstall; and untrusted artifacts being
  retained.

Required gates are the focused scheduler and CLI suites, `cargo fmt --check`,
`cargo clippy --workspace --all-targets -- -D warnings`,
`cargo test --workspace`, and exact-head GitHub Actions.

## Drill restart boundary

The lifecycle RED in `C:\ai_agent\edda-drills\20260816T163456Z` is preserved;
it is not a PASS and D1-D8 did not start. No scheduler or PID drill resumes from
this design commit.

After implementation, all gates, and a PR-visible numbered review with
current-head LGTM (P0=0, P1=0), restart Task #50 in a new preserved run
directory with a fresh exact-head binary and raw capture artifacts. Prove the
missing-HRESULT lifecycle from scratch, then D1, and only after each accepted boundary continue
serially through D2-D8. Any product RED stops drills and returns to a separately
scoped fix and review round.
