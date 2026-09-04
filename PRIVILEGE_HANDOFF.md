# PRIVILEGE_HANDOFF.md — GH-690 restricted-lane spike: what is delivered, what is blocked

**Session:** infra-controller-gh690 · task #27 · worktree
`C:\ai_agent\edda-wt-infra-privilege` · branch `codex/infra-privilege-690`
· basis `467e8be02eb98fb47d0eca82e9dd400d91d67e6a` · 2026-09-04

## Delivered (reviewable in the PR)

1. **Spike harness, prepared and fixture-validated** —
   `scripts/spikes/lane-privilege/` (`LanePrivilegeSpike.psm1`,
   `Invoke-Preflight.ps1`, `Invoke-SpikeAction.ps1`,
   `tests/run-fixture-tests.ps1`, `README.md`).
   - No-secrets metadata preflight is a separate script from the action.
   - Principal is read from the process token (WindowsIdentity); a spoofed
     `USERNAME`/`USERPROFILE` cannot pass (fixture T5), and the action refuses
     any principal except the configured restricted one (fixture T4, exit 3).
   - Negative tests only open/dispose protected credential files — content is
     never read, stored, or printed (fixture T6 uses a canary to prove this).
   - `gh` checks expose identity/token-SOURCE metadata only; token values
     (even masked) are dropped before output.
   - Push is structurally limited to the exact allowlisted `spike/…` branch;
     `main`/`master`/`HEAD` are refused by the branch guard (fixture T7).
   - Token provider resolution is real-or-explicitly-UNSUPPORTED; no mocks
     (fixture T9). `file:`/`env:` refs are refused by design (fixture T3).
   - Fixture run: **19/19 pass** (syntax-checked; no credentials touched, no
     ACL/account changes, no network, no build, no push).
2. **Threat model updated** — `docs/architecture/lane-privilege-threat-model.md`:
   §7 fixed (PR #686 merged; obsolete wait-for-#686 text replaced) plus new
   §7.0 cross-reference table to the node design doc
   (`docs/superpowers/specs/2026-09-02-edda-node-agent-transport-design.md`);
   §8 points at the prepared harness and states plainly that the restricted
   run is NOT RUN; `docs/architecture/actor-signing.md` referenced as inline
   code only, marked as a forthcoming cross-PR proposal — deliberately no
   tracked link until that file lands (it is being authored by a peer session).

## Evidence status

| Item | Status |
|---|---|
| Baseline FAIL (lane can read operator credentials + org token) | READ from GH-690 issue body (2026-09-02 22:5x, 4090, basis `a1dd3d8`) — not re-measured |
| Harness fixture tests (preflight no-op + refusal branches) | RAN, 19/19 pass |
| Restricted-account negative tests (AccessDenied proof) | **NOT RUN** |
| Restricted-account positive test (broker token → build/test/push of spike branch) | **NOT RUN** |

The two NOT RUN rows are the remaining GH-690 doneWhen item 3. They cannot be
produced from this session: the current process is not elevated
(`WindowsPrincipal` check: not admin), no obvious restricted lane user exists
(`Get-LocalUser` review found none), and no GitHub App installation-token
source was provided. Per instruction, no accounts, ACLs, scheduled tasks, or
auth settings were changed, no credential contents were read, and no mock
tokens were substituted.

## Exact setup the operator must provide (nothing else assumed)

1. **Windows standard account** for the restricted lane, e.g. `edda-lane`
   (one per machine, per decision `fleet.lane-privilege`). Created by the
   operator; account creation is an implementation action outside agent
   authority.
2. **Explicit write grants** for that account on (a) the spike worktree and
   (b) the assigned build-lane directory (e.g.
   `%LOCALAPPDATA%\fleet-workstation\lanes\worker-N`).
3. **A token provider** reachable from that account:
   - `edda-node://…` — requires the node credential-broker, which node v0 does
     not ship (explicitly UNSUPPORTED in the harness; see threat model §7.0); or
   - `credman://<name>` — a per-lane fine-grained PAT stored in the **restricted
     account's** Windows Credential Manager (needs the CredentialManager
     PowerShell module) — the documented degradation path.
4. **One scheduled task / runas invocation** under the restricted principal
   running `Invoke-SpikeAction.ps1` with the exact parameters shown in
   `scripts/spikes/lane-privilege/README.md` (operator principal, restricted
   principal, workspace, build lane, repo allowlist, single spike branch
   allowlist entry, token provider ref).

Once provided, the run produces the missing evidence directly:
negative tests must show `AccessDenied` and no reachable token (exit path 0),
and the positive path pushes only `refs/heads/spike/…`. Any `Readable`
credential or keyring token stops the run with exit 4 — that is the
fail-closed contract, not a harness defect.

## Handoff

The controller should present the PR for review and ask the user **only
once**, with this file and the harness README as the reviewable artifact:
provide items 1–4 above (or explicitly descope the spike to its own issue).
The PR carries `Issue: #690` **without** a closes keyword: the issue stays
open until the restricted-account run actually happens.
