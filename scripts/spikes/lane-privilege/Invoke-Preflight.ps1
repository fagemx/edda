# Invoke-Preflight.ps1 — GH-690 lane privilege spike, no-secrets metadata preflight.
#
# This script performs NO secret resolution, NO credential probing, NO network I/O,
# and NO build/push. It validates the spike's parameter set and records the current
# principal as metadata. The action script (Invoke-SpikeAction.ps1) runs this first
# and refuses to proceed on any failure.
#
# Exit codes: 0 = all checks pass; 2 = one or more checks failed.

[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$OperatorPrincipal,     # e.g. MACHINE\fagem — the principal that must NEVER run the action
    [Parameter(Mandatory)][string]$RestrictedPrincipal,   # e.g. MACHINE\edda-lane — the only principal allowed to run the action
    [Parameter(Mandatory)][string]$WorkspacePath,         # worktree the restricted lane builds in
    [Parameter(Mandatory)][string]$BuildLanePath,         # CARGO_TARGET_DIR the restricted lane reuses (no cold builds)
    [Parameter(Mandatory)][string[]]$RepoAllowList,       # exact repos, e.g. 'fagemx/edda' — wildcards refused
    [Parameter(Mandatory)][string[]]$BranchAllowList,     # exact spike branches, e.g. 'spike/lane-privilege-20260904' — main refused
    [Parameter(Mandatory)][string]$TokenProviderRef       # e.g. 'edda-node://<host>:<port>/credentials/gh-installation-token/<app>'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 3.0

# Normalize collection parameters: a single-element array passed by splatting may
# arrive as a scalar string, which breaks .Count and foreach under StrictMode.
$RepoAllowList = @($RepoAllowList)
$BranchAllowList = @($BranchAllowList)

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
Import-Module (Join-Path $scriptDir 'LanePrivilegeSpike.psm1') -Force

$checks = [System.Collections.Generic.List[object]]::new()

function Add-Check {
    param([string]$Name, [bool]$Passed, [string]$Detail)
    $checks.Add([pscustomobject]@{ Check = $Name; Passed = $Passed; Detail = $Detail })
}

# 1. Principals: present, distinct, non-degenerate.
$principalOk = -not [string]::IsNullOrWhiteSpace($OperatorPrincipal) `
    -and -not [string]::IsNullOrWhiteSpace($RestrictedPrincipal)
Add-Check -Name 'principals-present' -Passed:$principalOk -Detail 'both principals non-empty'

$distinctOk = -not (Test-SpikePrincipalMatch -Expected $OperatorPrincipal -Actual $RestrictedPrincipal)
Add-Check -Name 'principals-distinct' -Passed:$distinctOk `
    -Detail 'operator and restricted principals differ (operator==restricted would void the boundary)'

# 2. Current principal — metadata only. This does NOT pass or fail here: preflight is
#    expected to run as the operator. The action script enforces the refusal.
$actualPrincipal = Get-SpikePrincipal
$isOperator  = Test-SpikePrincipalMatch -Expected $OperatorPrincipal   -Actual $actualPrincipal
$isRestricted = Test-SpikePrincipalMatch -Expected $RestrictedPrincipal -Actual $actualPrincipal
$classification = if ($isOperator) { 'operator' }
    elseif ($isRestricted) { 'restricted' }
    else { 'other' }
Add-Check -Name 'current-principal-recorded' -Passed:$true `
    -Detail "current principal classified as '$classification' (metadata only; refusal is enforced by the action script)"

# 3. Paths: workspace and build lane exist (metadata; no writes, no build).
$workspaceOk = Test-Path -LiteralPath $WorkspacePath -PathType Container
Add-Check -Name 'workspace-exists' -Passed:$workspaceOk -Detail $WorkspacePath
$laneOk = Test-Path -LiteralPath $BuildLanePath -PathType Container
Add-Check -Name 'build-lane-exists' -Passed:$laneOk -Detail $BuildLanePath

# 4. Allowlists: exact names only; a main-targeting allowlist is refused outright.
$repoOk = $true
$repoDetail = 'ok'
try {
    foreach ($repo in $RepoAllowList) { Assert-SafeRepoTarget -Repo $repo -RepoAllowList $RepoAllowList | Out-Null }
} catch { $repoOk = $false; $repoDetail = $_.Exception.Message }
Add-Check -Name 'repo-allowlist-valid' -Passed:$repoOk -Detail $repoDetail

$branchOk = $true
$branchDetail = 'ok'
try {
    if ($BranchAllowList.Count -ne 1) {
        throw "Branch allowlist must contain exactly one spike branch (got $($BranchAllowList.Count))."
    }
    Assert-SafeSpikeBranch -Branch $BranchAllowList[0] -BranchAllowList $BranchAllowList | Out-Null
} catch { $branchOk = $false; $branchDetail = $_.Exception.Message }
Add-Check -Name 'branch-allowlist-valid' -Passed:$branchOk -Detail $branchDetail

# 5. Token provider reference: syntax/classification only — NO resolution here.
$providerOk = $true
$providerDetail = 'ok'
try {
    $kindText = [string](Get-TokenProviderKind -ProviderRef $TokenProviderRef)
    if ($kindText -eq 'Forbidden') {
        throw "Forbidden provider scheme (file:/env:) — secrets never on disk, never in env."
    }
    if ($kindText -eq 'Unsupported') {
        throw "Unknown provider scheme — explicitly unsupported."
    }
    $providerDetail = "classified as '$kindText' (resolution happens only in the action script, fail-closed)"
} catch {
    $providerOk = $false; $providerDetail = $_.Exception.Message
    $kindText = 'invalid'
}
Add-Check -Name 'token-provider-classified' -Passed:$providerOk -Detail $providerDetail

# 6. Tool availability (metadata only).
foreach ($tool in @('git', 'gh', 'cargo')) {
    $cmd = Get-Command $tool -ErrorAction SilentlyContinue
    Add-Check -Name "tool-$tool" -Passed:($null -ne $cmd) `
        -Detail $(if ($null -ne $cmd) { $cmd.Source } else { 'not found in PATH' })
}

# Emit the metadata record, scrubbed of anything token-shaped.
$report = [pscustomobject]@{
    Harness             = 'gh690-lane-privilege-preflight'
    TimestampUtc        = (Get-Date).ToUniversalTime().ToString('o')
    CurrentPrincipal    = $actualPrincipal
    PrincipalClass      = $classification
    OperatorPrincipal   = $OperatorPrincipal
    RestrictedPrincipal = $RestrictedPrincipal
    WorkspacePath       = $WorkspacePath
    BuildLanePath       = $BuildLanePath
    RepoAllowList       = $RepoAllowList
    BranchAllowList     = $BranchAllowList
    TokenProviderKind   = $null
    Checks              = $checks
    AllPassed           = (@($checks | Where-Object { -not $_.Passed }).Count -eq 0)
}
try { $report.TokenProviderKind = $kindText }
catch { $report.TokenProviderKind = 'invalid' }

$json = $report | ConvertTo-Json -Depth 4
foreach ($line in ($json -split "`r?`n")) { Write-SpikeSafe -Text $line }

if (-not $report.AllPassed) { exit $EXIT_PREFLIGHT_FAILED }
exit $EXIT_OK
