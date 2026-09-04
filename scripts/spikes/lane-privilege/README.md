# Lane privilege spike harness (GH-690)

**Status: PREPARED, NOT RUN.** The real restricted-account build/test/push has not
been executed: this host has no restricted lane account and no token broker.
Everything below is the prepared harness plus fixture-level validation of its
no-op and refusal branches. No positive spike evidence exists yet. Do not quote
this harness as proof that the privilege boundary holds.

## What this harness is

A fail-closed PowerShell harness that, once the operator provisions the missing
setup (§ Operator setup below), executes the GH-690 spike:

- **Negative test A** — from the restricted principal, attempt to open each
  protected credential file (`~/.claude/.credentials.json`, `~/.codex/auth.json`,
  `~/.pi/agent/auth.json`) and immediately dispose the handle. It **never reads,
  stores, or prints file content**. Expected after implementation: `AccessDenied`
  for all. Any `Readable` result is the baseline failure signal (exit 4) and
  stops the run before build/push.
- **Negative test B** — collect `gh` identity and token **SOURCE** metadata only
  (login name, whether a token is present, source class). Token values, including
  the masked forms `gh` prints, are dropped before output. A keyring token
  visible under the restricted principal → exit 4.
- **Positive path** — resolve a short-lived broker token (real resolution or an
  explicit UNSUPPORTED; never a mock), run `cargo test --workspace` in the
  assigned build lane, and push to the **exact allowlisted spike branch only**
  (`spike/…`; `main`/`master`/`HEAD` are structurally refused by the branch guard).

## Files

| File | Role |
|---|---|
| `LanePrivilegeSpike.psm1` | shared fail-closed helpers (principal, probe, scrubber, branch/repo guards, provider classification/resolution) |
| `Invoke-Preflight.ps1` | **no-secrets** metadata preflight: parameter shape, principal classification, path existence, allowlist validity, provider scheme classification, tool availability |
| `Invoke-SpikeAction.ps1` | the gated action: preflight → principal assertion → negative tests → token resolution → build/test → push |
| `tests/run-fixture-tests.ps1` | fixture tests of the no-op preflight and refusal branches (no real credentials, no ACL/account changes, no network, no build/push) |

## Fail-closed properties (verified by the fixture tests)

1. **Principal comes from the process token** (`WindowsIdentity`), never from the
   environment — a spoofed `USERNAME`/`USERPROFILE` cannot make an unrestricted
   caller pass (T5).
2. **The action refuses any principal other than the restricted one**, including
   the operator (exit 3, T4).
3. **Probes never read content** — open/dispose only; a canary string in a fixture
   file never appears in output (T6a). A missing file is `NotFound` =
   inconclusive, not a pass (T6b).
4. **Branch guard refuses** `main`, `master`, `HEAD`, `origin/main`,
   `origin/master`, any non-`spike/` branch, and any name not exactly in the
   allowlist (T7).
5. **All output is scrubbed** through a token-pattern redactor before printing (T8).
6. **Token resolution is real or explicitly UNSUPPORTED** — `edda-node://`
   (node v0 has no credential-broker endpoint), `credman://` without the
   CredentialManager module, and unknown schemes all fail with an explicit
   UNSUPPORTED, never a mock value (T9). `file:`/`env:` refs are refused by
   design (T3): secrets never land on disk and never travel through the
   environment (decision `fleet.lane-privilege`).

## Exit codes

| Code | Meaning |
|---|---|
| 0 | success (or `-SkipBuildTest` completed) |
| 2 | preflight failed |
| 3 | principal refusal (not running as the restricted principal) |
| 4 | protection property not met — credential readable or operator token visible; baseline-expected, run stopped |
| 5 | token provider explicitly unsupported |
| 6 | inconclusive (e.g. `gh` unavailable) |
| 7 | publication failed or credential cleanup was incomplete |

## Evidence status (honest ledger)

| Item | Status |
|---|---|
| Baseline FAIL evidence (lane can read operator credentials + org token) | **READ, not re-measured** — GH-690 issue body, 2026-09-02 22:5x on 4090, basis `a1dd3d8` (threat model §1). Rerunning equals re-touching credentials; the conclusion does not change by rerunning. |
| Fixture tests of preflight + refusal branches | **RAN** — 64/64 pass (safe synthetic fixtures; see run receipt in the PR). |
| Restricted-account negative tests (AccessDenied observed) | **NOT RUN** — no restricted account exists on this host. |
| Restricted-account positive test (broker token → build/test/push of the spike branch) | **NOT RUN** — no restricted account, no GitHub App installation-token broker. |

## Operator setup — the exact minimum to run the spike

The harness needs exactly these four things; nothing else was assumed or faked:

1. **A dedicated Windows standard account** for the restricted lane
   (decision `fleet.lane-privilege`: one per machine, e.g. `edda-lane`).
   Created and provisioned by the operator — creating accounts is an
   implementation action outside this session's authority.
2. **Write access for that account** to (a) the spike worktree and (b) the
   assigned build-lane directory — explicit grants, not inherited, per threat
   model §6.1.
3. **A token provider** reachable by the restricted account: either the node
   credential-broker (GH-685 follow-up; `edda-node://…` — currently
   UNSUPPORTED because node v0 ships no credential endpoint) or, as the
   documented degradation path, a per-lane fine-grained PAT stored under
   `credman://<name>` in Windows Credential Manager **of the restricted
   account** (requires the CredentialManager PowerShell module).
4. **The spike invocation itself**, as a scheduled task or runas session under
   the restricted principal, e.g.:

   ```powershell
   # Use edda-node://… instead of credman:// once the broker exists.
   pwsh -NoProfile -File scripts/spikes/lane-privilege/Invoke-SpikeAction.ps1 `
     -OperatorPrincipal      'MACHINE\fagem' `
     -RestrictedPrincipal    'MACHINE\edda-lane' `
     -WorkspacePath          'C:\ai_agent\edda-wt-infra-privilege' `
     -BuildLanePath          '<lane-root>\worker-N' `
     -RepoAllowList          'fagemx/edda' `
     -BranchAllowList        'spike/lane-privilege-<date>' `
     -TokenProviderRef       'credman://gh-lane-spike' `
     -ProtectedCredentialFiles @('C:\Users\fagem\.claude\.credentials.json', `
                              'C:\Users\fagem\.codex\auth.json', `
                              'C:\Users\fagem\.pi\agent\auth.json')
   ```

   The spike branch is created by the run itself (`HEAD:refs/heads/spike/…`);
   no other ref may appear in the allowlist.

Nothing in this harness creates accounts, changes ACLs, touches scheduled tasks,
or modifies authentication settings — all of that is operator-side, listed above.
