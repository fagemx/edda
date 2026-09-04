# Invoke-SpikeAction.ps1 — GH-690 lane privilege spike, fail-closed action.
#
# Runs ONLY under the restricted principal. Refuses (exit 3) under any other
# principal, including the operator — and the principal is taken from the process
# token (WindowsIdentity), not from the environment, so faking one cannot pass.
#
# Order of operations, each gated on the previous:
#   1. Preflight (no-secrets) must pass.
#   2. Principal assertion: current principal must equal -RestrictedPrincipal.
#   3. Protected-path target validation: the probed credential files must be the
#      OPERATOR's (explicitly passed), never the current (restricted) profile —
#      probing our own profile proves nothing about the boundary (Round1 F4).
#   4. Negative test A: probe protected credential files (open/dispose only —
#      never reads content). AccessDenied is the only continuation evidence;
#      Readable => exit 4; NotFound/Inconclusive/ProbeError => exit 6 (fail
#      closed — a sharing violation is NOT access control, Round1 F5).
#   5. Negative test B: gh identity/token SOURCE metadata (token values dropped,
#      source parsed before the line is discarded). Any reachable token => exit 4;
#      error exits and login-without-token ambiguity => exit 6 (Round1 F1).
#   6. Broker token resolution: real or explicitly UNSUPPORTED (exit 5). No mocks.
#   7. In-memory credential-cache availability: UNSUPPORTED (exit 5) without it —
#      there is deliberately no persistent fallback (Round1 F3).
#   8. Build/test under the restricted principal in the assigned build lane.
#   9. Publication: bind the EFFECTIVE push URL to the repo allowlist (Round1 F2),
#      then push via git's in-memory credential cache only — token moves through a
#      stdin pipe, never argv/env/files, no `gh auth login` (Round1 F3).
#
# Exit codes: 0 ok; 2 preflight/destination failed; 3 principal refused;
#             4 protection fail; 5 provider unsupported; 6 inconclusive;
#             7 publication failed or cleanup incomplete.

[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$OperatorPrincipal,
    [Parameter(Mandatory)][string]$RestrictedPrincipal,
    [Parameter(Mandatory)][string]$WorkspacePath,
    [Parameter(Mandatory)][string]$BuildLanePath,
    [Parameter(Mandatory)][string[]]$RepoAllowList,
    [Parameter(Mandatory)][string[]]$BranchAllowList,
    [Parameter(Mandatory)][string]$TokenProviderRef,
    [Parameter(Mandatory)][string[]]$ProtectedCredentialFiles,
    [switch]$SkipBuildTest   # run negative tests + resolution + availability only (no build, no auth, no push)
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

# --- 3. Protected-path target validation (Round1 F4). --------------------------
# The probed files must be the OPERATOR's protected credentials, passed explicitly.
# Defaults derived from the current USERPROFILE would probe the RESTRICTED profile
# under the restricted account and prove nothing; a target inside the current
# profile is refused outright.
$currentProfile = $env:USERPROFILE
$targetOk = $true
foreach ($p in $ProtectedCredentialFiles) {
    if ([string]::IsNullOrWhiteSpace($p)) { $targetOk = $false; continue }
    $full = [System.IO.Path]::GetFullPath($p)
    if (-not [System.IO.Path]::IsPathRooted($full)) {
        Write-SpikeSafe -Text "FAIL: protected credential path is not absolute: '$p'."
        $targetOk = $false
    }
    elseif ($currentProfile -and $full.StartsWith($currentProfile, [System.StringComparison]::OrdinalIgnoreCase)) {
        Write-SpikeSafe -Text ("FAIL: protected path '{0}' is inside the CURRENT (restricted) profile. " +
            "Pass the operator's credential paths explicitly; probing our own profile proves nothing." -f $p)
        $targetOk = $false
    }
}
if (-not $targetOk) { exit $EXIT_PREFLIGHT_FAILED }
Write-SpikeSafe -Text ("OK: protected-path targets validated (operator profile, explicitly passed): {0}" -f
    ($ProtectedCredentialFiles -join '; '))

# --- 4. Negative test A: protected credential files. ---------------------------
# Open/dispose only. Expected after implementation: every probe is AccessDenied.
# Anything else is either a baseline failure (Readable) or inconclusive.
$protectionMet = $true
$inconclusiveSeen = $false
foreach ($path in $ProtectedCredentialFiles) {
    $result = Get-CredentialFileProbe -Path $path
    $detail = switch ("$result") {
        'AccessDenied'  { 'access denied (protection property MET)' }
        'Readable'      { 'READABLE — protection property NOT met (baseline failure)' }
        'NotFound'      { 'not found — inconclusive, nothing proven' }
        'Inconclusive'  { 'open failed on a non-ACL state (e.g. sharing violation) — inconclusive, NOT access-control evidence' }
        default         { 'probe error — inconclusive' }
    }
    Write-SpikeSafe -Text ("probe '{0}': {1}" -f $path, $detail)
    switch ("$result") {
        'AccessDenied' { }
        'Readable'     { $protectionMet = $false }
        default        { $inconclusiveSeen = $true }
    }
}
if (-not $protectionMet) {
    Write-SpikeSafe -Text ("FAIL: protected credentials reachable from the restricted lane. " +
        "This is the expected BASELINE result (GH-690 issue body, 2026-09-02 measurement: all three " +
        "vendor credential files readable as the operator). Stopping before build/push — no positive " +
        "evidence may be produced while the negative property fails.")
    exit $EXIT_PROTECTION_FAIL
}
if ($inconclusiveSeen) {
    Write-SpikeSafe -Text ("STOP: one or more probes were inconclusive (not access-denied evidence). " +
        "The negative property is unproven; fail closed before build/push.")
    exit $EXIT_INCONCLUSIVE
}

# --- 5. Negative test B: gh token SOURCE metadata (Round1 F1). ------------------
# Token values are never printed; only presence, parsed source class, and the
# account name. Error exits and ambiguous states are inconclusive, never a pass.
try {
    $ghMeta = Get-GhTokenSourceMetadata
    $ghVerdict = Get-GhMetadataVerdict -Metadata $ghMeta -GhExitCode $ghMeta.GhExit
} catch {
    Write-SpikeSafe -Text ("STOP: {0}" -f $_.Exception.Message)
    exit $EXIT_INCONCLUSIVE
}
Write-SpikeSafe -Text ("gh metadata: loggedIn={0} account={1} tokenPresent={2} tokenSource={3} verdict={4}" -f
    $ghMeta.LoggedIn, $ghMeta.Account, $ghMeta.TokenPresent, $ghMeta.TokenSource, $ghVerdict)
switch ($ghVerdict) {
    'ok-no-login' {
        Write-SpikeSafe -Text "OK: no gh login visible to the restricted lane (the transient cache credential is the only path)."
    }
    'deny-token-present' {
        Write-SpikeSafe -Text ("FAIL: a token is reachable under the restricted principal " +
            "(source class '{0}'; unparsable source also fails closed). No build/push may proceed." -f $ghMeta.TokenSource)
        exit $EXIT_PROTECTION_FAIL
    }
    default {
        Write-SpikeSafe -Text ("STOP: gh metadata verdict '{0}' is inconclusive; fail closed." -f $ghVerdict)
        exit $EXIT_INCONCLUSIVE
    }
}

# --- 6. Broker token resolution — real or explicitly unsupported. ---------------
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

# --- 7. In-memory credential-cache availability (Round1 F3). --------------------
# The publication path uses git's credential-cache (daemon memory, short timeout)
# as the ONLY transient store. No persistent credential helper, no `gh auth login`.
if (-not (Test-GitCredentialCacheAvailable)) {
    Write-SpikeSafe -Text ("UNSUPPORTED: git-credential-cache is not available on this host; " +
        "there is deliberately no persistent fallback (no gh auth login, no plaintext file). " +
        "Provision git with credential-cache support or resolve the broker on a host that has it.")
    exit $EXIT_PROVIDER_UNSUPPORTED
}

# --- 8/9. Build, test, publish — restricted principal only. ---------------------
# NOTE: with no broker provider available today, step 6 always exits 5 before this
# point on this host. The code below is the executed-in-full path once an operator
# provisions the account + broker; it is fail-closed, not decorative.
$spikeBranch = $BranchAllowList[0]
try {
    Assert-SafeSpikeBranch -Branch $spikeBranch -BranchAllowList $BranchAllowList | Out-Null
} catch {
    Write-SpikeSafe -Text ("FAIL: {0}" -f $_.Exception.Message)
    exit $EXIT_PREFLIGHT_FAILED
}

if ($SkipBuildTest) {
    Write-SpikeSafe -Text "SKIP: -SkipBuildTest set; build/test/publication not run."
    exit $EXIT_OK
}

try {
    $env:CARGO_TARGET_DIR = $BuildLanePath
    Write-SpikeSafe -Text "build/test: cargo test (restricted principal, assigned build lane)…"
    & cargo test --workspace 2>&1 | ForEach-Object { Write-SpikeSafe -Text "$_" }
    if ($LASTEXITCODE -ne 0) {
        Write-SpikeSafe -Text "FAIL: cargo test exited nonzero; nothing will be published and no credential is cached."
        exit $EXIT_PROTECTION_FAIL
    }
}
finally {
    Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
}

# Bind the ACTUAL publication destination to the repo allowlist (Round1 F2),
# immediately before publication: the effective push URL is resolved from the
# workspace's git config and must belong to an allowlisted repository.
try {
    $pushUrl = Get-EffectivePushUrl -WorkspacePath $WorkspacePath -RepoAllowList $RepoAllowList -RemoteName 'origin'
    Write-SpikeSafe -Text ("OK: effective push destination bound and allowlisted: {0}" -f $pushUrl)
} catch {
    Write-SpikeSafe -Text ("FAIL: {0}" -f $_.Exception.Message)
    exit $EXIT_PREFLIGHT_FAILED
}

# Publication: token moves only through stdin into git's in-memory credential
# cache (isolated helper list), push uses one explicit refspec with tag-following
# disabled, and cleanup (cache entry rejection + daemon exit) is attempted
# unconditionally and reported. No gh auth login/logout anywhere in this path.
$publication = Invoke-SpikePublication -Token $token -WorkspacePath $WorkspacePath -RemoteName 'origin' -RefSpec "HEAD:refs/heads/$spikeBranch"
$token = $null
foreach ($note in $publication.Notes) { Write-SpikeSafe -Text ("publication note: {0}" -f $note) }
Write-SpikeSafe -Text ("publication verdict: {0} (pushExit={1} cleanupExit={2}). " +
    "A published verdict proves only that git published the ref — it does not by itself prove " +
    "the token was broker-scoped; that requires the broker's own audit trail." -f
    $publication.Verdict, $publication.PushExit, $publication.CleanupExit)

switch ($publication.Verdict) {
    'published' { exit $EXIT_OK }
    'auth-failed' { exit $EXIT_PROVIDER_UNSUPPORTED }
    'cleanup-incomplete' { exit $EXIT_PUBLICATION_FAILED }
    default { exit $EXIT_PUBLICATION_FAILED }
}
