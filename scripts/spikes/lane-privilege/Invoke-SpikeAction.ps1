# Invoke-SpikeAction.ps1 — GH-690 lane privilege spike, fail-closed action.
#
# Runs ONLY under the restricted principal. Refuses (exit 3) under any other
# principal, including the operator — and the principal is taken from the process
# token (WindowsIdentity), not from the environment, so faking one cannot pass.
#
# Order of operations, each gated on the previous:
#   1. Preflight (no-secrets) must pass.
#   2. Principal assertion: current principal must equal -RestrictedPrincipal.
#   3. Negative test A: probe protected credential files (open/dispose only —
#      never reads content). Expected POST-implementation result: AccessDenied.
#      Readable => exit 4 (protection property not met; the baseline evidence).
#   4. Negative test B: gh identity/token SOURCE metadata (token values dropped,
#      output scrubbed). An operator keyring token present under the restricted
#      principal => exit 4.
#   5. Broker token resolution: real or explicitly UNSUPPORTED (exit 5). No mocks.
#   6. Build/test under the restricted principal in the assigned build lane.
#   7. Push to the exact allowlisted spike branch only (never main).
#
# Exit codes: 0 ok; 2 preflight failed; 3 principal refused; 4 protection fail;
#             5 provider unsupported; 6 inconclusive.

[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$OperatorPrincipal,
    [Parameter(Mandatory)][string]$RestrictedPrincipal,
    [Parameter(Mandatory)][string]$WorkspacePath,
    [Parameter(Mandatory)][string]$BuildLanePath,
    [Parameter(Mandatory)][string[]]$RepoAllowList,
    [Parameter(Mandatory)][string[]]$BranchAllowList,
    [Parameter(Mandatory)][string]$TokenProviderRef,
    [string[]]$ProtectedCredentialFiles = @(
        "$env:USERPROFILE\.claude\.credentials.json",
        "$env:USERPROFILE\.codex\auth.json",
        "$env:USERPROFILE\.pi\agent\auth.json"
    ),
    [switch]$SkipBuildTest   # run negative tests + token resolution only (no build, no push)
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 3.0

# Normalize collection parameters (single-element arrays may arrive as scalars).
$RepoAllowList = @($RepoAllowList)
$BranchAllowList = @($BranchAllowList)
$ProtectedCredentialFiles = @($ProtectedCredentialFiles)

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
Import-Module (Join-Path $scriptDir 'LanePrivilegeSpike.psm1') -Force

# --- 1. Preflight (no secrets). -----------------------------------------------
# Only pass parameters the preflight script declares (drop action-only ones).
$preflightParams = @{
    OperatorPrincipal   = $OperatorPrincipal
    RestrictedPrincipal = $RestrictedPrincipal
    WorkspacePath       = $WorkspacePath
    BuildLanePath       = $BuildLanePath
    RepoAllowList       = $RepoAllowList
    BranchAllowList     = $BranchAllowList
    TokenProviderRef    = $TokenProviderRef
}
$preflightJson = & (Join-Path $scriptDir 'Invoke-Preflight.ps1') @preflightParams
if ($LASTEXITCODE -ne $EXIT_OK) {
    Write-SpikeSafe -Text "FAIL: preflight did not pass; refusing to run the action."
    $preflightJson | ForEach-Object { Write-SpikeSafe -Text "$_" }
    exit $EXIT_PREFLIGHT_FAILED
}

# --- 2. Principal assertion — the fail-closed gate. ----------------------------
$currentPrincipal = Get-SpikePrincipal
if (-not (Test-SpikePrincipalMatch -Expected $RestrictedPrincipal -Actual $currentPrincipal)) {
    Write-SpikeSafe -Text ("REFUSED: current principal '{0}' is not the restricted principal. " +
        "The action never runs from the operator or any other account, and a spoofed " +
        "environment cannot change this verdict (identity is read from the process token)." -f
        $currentPrincipal)
    exit $EXIT_PRINCIPAL_REFUSED
}
Write-SpikeSafe -Text "OK: running as restricted principal."

# --- 3. Negative test A: protected credential files. ---------------------------
# Open/dispose only. Expected after implementation: every probe is AccessDenied.
$protectionMet = $true
foreach ($path in $ProtectedCredentialFiles) {
    $result = Get-CredentialFileProbe -Path $path
    $detail = switch ($result) {
        'AccessDenied' { 'access denied (protection property MET)' }
        'Readable'     { 'READABLE — protection property NOT met (baseline failure)' }
        'NotFound'     { 'not found — inconclusive, nothing proven' }
        default        { 'probe error — inconclusive' }
    }
    Write-SpikeSafe -Text ("probe '{0}': {1}" -f $path, $detail)
    if ([string]$result -ne 'AccessDenied') { $protectionMet = $false }
}
if (-not $protectionMet) {
    Write-SpikeSafe -Text ("FAIL: protected credentials reachable from the restricted lane. " +
        "This is the expected BASELINE result (GH-690 issue body, 2026-09-02 measurement: all three " +
        "vendor credential files readable as the operator). Stopping before build/push — no positive " +
        "evidence may be produced while the negative property fails.")
    exit $EXIT_PROTECTION_FAIL
}

# --- 4. Negative test B: gh token SOURCE metadata. ------------------------------
# Token values are never printed; only presence, source class, and account name.
try {
    $ghMeta = Get-GhTokenSourceMetadata
} catch {
    Write-SpikeSafe -Text ("STOP: {0}" -f $_.Exception.Message)
    exit $EXIT_INCONCLUSIVE
}
Write-SpikeSafe -Text ("gh metadata: loggedIn={0} account={1} tokenPresent={2} tokenSource={3}" -f
    $ghMeta.LoggedIn, $ghMeta.Account, $ghMeta.TokenPresent, $ghMeta.TokenSource)
if ($ghMeta.TokenPresent -and $ghMeta.TokenSource -eq 'keyring') {
    Write-SpikeSafe -Text "FAIL: a keyring token is reachable under the restricted principal (org-level operator token must not be)."
    exit $EXIT_PROTECTION_FAIL
}
if (-not $ghMeta.LoggedIn) {
    Write-SpikeSafe -Text "OK: no gh login visible to the restricted lane (short-lived broker token is the only path)."
}

# --- 5. Broker token resolution — real or explicitly unsupported. ---------------
try {
    $token = Resolve-BrokerToken -ProviderRef $TokenProviderRef
} catch [System.Management.Automation.RuntimeException] {
    Write-SpikeSafe -Text ("STOP: {0}" -f $_.Exception.Message)
    exit $EXIT_PROVIDER_UNSUPPORTED
}
if ($null -eq $token) {
    Write-SpikeSafe -Text "STOP: broker token resolution returned nothing (fail-closed)."
    exit $EXIT_PROVIDER_UNSUPPORTED
}

# --- 6/7. Build, test, push — restricted principal only. ------------------------
# NOTE: with no broker provider available today, step 5 always exits 5 before this
# point on this host. The code below is the executed-in-full path once an operator
# provisions the account + broker; it is fail-closed, not decorative.
if ($SkipBuildTest) {
    Write-SpikeSafe -Text "SKIP: -SkipBuildTest set; build/test/push not run."
    exit $EXIT_OK
}

$spikeBranch = $BranchAllowList[0]
try {
    Assert-SafeSpikeBranch -Branch $spikeBranch -BranchAllowList $BranchAllowList | Out-Null
} catch {
    Write-SpikeSafe -Text ("FAIL: {0}" -f $_.Exception.Message)
    exit $EXIT_PREFLIGHT_FAILED
}

Push-Location $WorkspacePath
try {
    $env:CARGO_TARGET_DIR = $BuildLanePath
    Write-SpikeSafe -Text "build/test: cargo test (restricted principal, assigned build lane)…"
    & cargo test --workspace 2>&1 | ForEach-Object { Write-SpikeSafe -Text "$_" }
    if ($LASTEXITCODE -ne 0) {
        Write-SpikeSafe -Text "FAIL: cargo test exited nonzero; nothing will be pushed."
        exit $EXIT_PROTECTION_FAIL
    }

    # Token reaches git only through gh's own stdin-driven login, never argv/env/files.
    $token | & gh auth login --with-token 2>&1 | ForEach-Object { Write-SpikeSafe -Text (Hide-SecretPatterns -Text "$_") }
    try {
        & git push origin "HEAD:refs/heads/$spikeBranch" 2>&1 |
            ForEach-Object { Write-SpikeSafe -Text (Hide-SecretPatterns -Text "$_") }
        if ($LASTEXITCODE -ne 0) {
            Write-SpikeSafe -Text "FAIL: git push exited nonzero."
            exit $EXIT_PROTECTION_FAIL
        }
        Write-SpikeSafe -Text ("OK: pushed HEAD to refs/heads/{0} (exact spike branch; main is guarded)." -f $spikeBranch)
    }
    finally {
        # Revoke the short-lived login on the restricted account immediately.
        & gh auth logout --hostname github.com 2>&1 |
            ForEach-Object { Write-SpikeSafe -Text (Hide-SecretPatterns -Text "$_") } | Out-Null
    }
}
finally {
    Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
    Pop-Location
}

exit $EXIT_OK
