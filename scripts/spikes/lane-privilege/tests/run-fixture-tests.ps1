# run-fixture-tests.ps1 — fixture tests for the GH-690 lane privilege spike harness.
#
# Scope: syntax-level validation of the no-op preflight and the refusal branches.
# Deliberately does NOT touch real credentials, change ACLs or accounts, reach the
# network, build, or push. Run anywhere (operator principal is fine — the tests
# assert the refusal paths, not the positive path).
#
# Usage:  pwsh -NoProfile -File tests/run-fixture-tests.ps1
# Exit:   0 = all pass; 1 = at least one failure.

[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 3.0

$testsDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$spikeDir = Split-Path -Parent $testsDir
Import-Module (Join-Path $spikeDir 'LanePrivilegeSpike.psm1') -Force

$script:pass = 0
$script:fail = 0

function Assert-True {
    param([string]$Name, [bool]$Condition, [string]$Detail = '')
    if ($Condition) {
        $script:pass++
        Write-Output "PASS  $Name"
    } else {
        $script:fail++
        Write-Output "FAIL  $Name  $Detail"
    }
}

# Fixture workspace/lane: throwaway temp dirs, no repo, no cargo.
$fxRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("gh690-fixture-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path (Join-Path $fxRoot 'ws') -Force | Out-Null
New-Item -ItemType Directory -Path (Join-Path $fxRoot 'lane') -Force | Out-Null
try {

    $baseArgs = @{
        OperatorPrincipal   = Get-SpikePrincipal   # fixture: we ARE the operator
        RestrictedPrincipal = 'FIXTURE\spike-lane-user'
        WorkspacePath       = (Join-Path $fxRoot 'ws')
        BuildLanePath       = (Join-Path $fxRoot 'lane')
        RepoAllowList       = @('fagemx/edda')
        BranchAllowList     = @('spike/lane-privilege-fixture')
        TokenProviderRef    = 'edda-node://fixture-host:7777/credentials/gh-installation-token/fagemx-edda'
    }

    # --- T1: preflight happy path (no-op, metadata only) → exit 0, no token shapes.
    $out = & (Join-Path $spikeDir 'Invoke-Preflight.ps1') @baseArgs 2>&1
    Assert-True -Name 'T1 preflight happy path exits 0' -Condition ($LASTEXITCODE -eq 0) -Detail "exit=$LASTEXITCODE"
    $outText = ($out | ForEach-Object { "$_" }) -join "`n"
    $tokenLeak = $false
    foreach ($p in @('gh[pousr]_[A-Za-z0-9]{16,}', 'github_pat_[A-Za-z0-9_]{20,}')) {
        if ([regex]::IsMatch($outText, $p)) { $tokenLeak = $true }
    }
    Assert-True -Name 'T1 preflight output contains no token-shaped strings' -Condition (-not $tokenLeak)
    Assert-True -Name 'T1 preflight reports principal classification' `
        -Condition ($outText -match "PrincipalClass") -Detail ($outText.Substring(0, [Math]::Min(400, $outText.Length)))

    # --- T2: preflight refuses a main-targeting branch allowlist.
    $mainArgs = $baseArgs.Clone(); $mainArgs.BranchAllowList = @('main')
    $null = & (Join-Path $spikeDir 'Invoke-Preflight.ps1') @mainArgs 2>&1
    Assert-True -Name 'T2 preflight refuses branch allowlist [main]' -Condition ($LASTEXITCODE -eq 2) -Detail "exit=$LASTEXITCODE"

    # --- T3: preflight refuses forbidden provider schemes (file:/env:).
    foreach ($ref in @('file://C:/no/secrets.txt', 'env:GITHUB_TOKEN')) {
        $pArgs = $baseArgs.Clone(); $pArgs.TokenProviderRef = $ref
        $null = & (Join-Path $spikeDir 'Invoke-Preflight.ps1') @pArgs 2>&1
        Assert-True -Name "T3 preflight refuses provider '$ref'" -Condition ($LASTEXITCODE -eq 2) -Detail "exit=$LASTEXITCODE"
    }

    # --- T4: action refuses under the operator principal (the cannot-fake-pass gate).
    $aArgs = $baseArgs.Clone(); $aArgs.Add('SkipBuildTest', $true)
    $aOut = & (Join-Path $spikeDir 'Invoke-SpikeAction.ps1') @aArgs 2>&1
    Assert-True -Name 'T4 action refuses when run as operator (exit 3)' -Condition ($LASTEXITCODE -eq 3) -Detail "exit=$LASTEXITCODE"
    $aText = ($aOut | ForEach-Object { "$_" }) -join "`n"
    Assert-True -Name 'T4 refusal names the process-token rationale' `
        -Condition ($aText -match 'not the restricted principal') -Detail $aText.Substring(0, [Math]::Min(300, $aText.Length))

    # --- T5: identity helper ignores spoofed environment variables.
    $realPrincipal = Get-SpikePrincipal
    $savedUser = $env:USERNAME; $savedProfile = $env:USERPROFILE
    $env:USERNAME = 'FIXTURE\spoofed-lane-user'
    $env:USERPROFILE = Join-Path $fxRoot 'fake-profile'
    $spoofedReading = Get-SpikePrincipal
    $env:USERNAME = $savedUser; $env:USERPROFILE = $savedProfile
    Assert-True -Name 'T5 spoofed USERNAME/USERPROFILE cannot change the reported principal' `
        -Condition (Test-SpikePrincipalMatch -Expected $realPrincipal -Actual $spoofedReading) `
        -Detail "reading=$spoofedReading"

    # --- T6: credential probe semantics without touching real credentials.
    #   a. readable fixture file → Readable (fail signal), and its CONTENT never appears
    #      in the harness output (open/dispose only).
    $canaryFile = Join-Path $fxRoot 'readable-secret.txt'
    $canary = 'CANARY-gho_FAKEFIXTURETOKEN0000000-DO-NOT-PRINT'
    Set-Content -LiteralPath $canaryFile -Value $canary -NoNewline
    $modPath = Join-Path $spikeDir 'LanePrivilegeSpike.psm1'
    $probeCmd = "& { param(`$m,`$p) Import-Module `$m -Force; Get-CredentialFileProbe -Path `$p } '$modPath' '$canaryFile'"
    $probeOut = & pwsh -NoProfile -Command $probeCmd 2>&1
    $probeOutText = ($probeOut | ForEach-Object { "$_" }) -join "`n"
    Assert-True -Name 'T6a probe of readable file reports Readable' `
        -Condition ($probeOutText.Trim() -eq 'Readable') -Detail $probeOutText
    Assert-True -Name 'T6a probe output never contains file content (canary absent)' `
        -Condition (-not $probeOutText.Contains('CANARY-gho_'))
    #   b. nonexistent path → NotFound (inconclusive; must not count as AccessDenied).
    $missing = & pwsh -NoProfile -Command ("& { param(`$m,`$p) Import-Module `$m -Force; Get-CredentialFileProbe -Path `$p } '$modPath' '" + (Join-Path $fxRoot 'does-not-exist.json') + "'") 2>&1
    Assert-True -Name 'T6b probe of missing file reports NotFound (inconclusive)' `
        -Condition ("$missing".Trim() -eq 'NotFound') -Detail "$missing"

    # --- T7: branch guard refusals and the single allowlisted acceptance.
    $t7fail = 0
    foreach ($bad in @('main', 'master', 'HEAD', 'origin/main', 'feature/x', 'spike/other-branch', 'spike/../traversal')) {
        try { Assert-SafeSpikeBranch -Branch $bad -BranchAllowList @('spike/lane-privilege-fixture') | Out-Null }
        catch { $t7fail++ }
    }
    Assert-True -Name 'T7 branch guard refuses all 7 forbidden targets' -Condition ($t7fail -eq 7) -Detail "refused=$t7fail"
    $t7ok = $false
    try { $t7ok = Assert-SafeSpikeBranch -Branch 'spike/lane-privilege-fixture' -BranchAllowList @('spike/lane-privilege-fixture') } catch {}
    Assert-True -Name 'T7 branch guard accepts the exact allowlisted spike branch' -Condition ([bool]$t7ok)

    # --- T8: secret scrubber.
    $dirty = 'token gho_ABCDEFGHIJKLMNOPqrst and github_pat_ABCDEFGHIJKLMNOPQRSTUVWXYZ123456 in text'
    Assert-True -Name 'T8 scrubber removes token shapes' `
        -Condition (-not [regex]::IsMatch((Hide-SecretPatterns -Text $dirty), 'gh[o]_[A-Za-z0-9]|github_pat_'))

    # --- T9: provider classification is real-or-unsupported, never mock.
    $t9unsupported = $false
    try { Resolve-BrokerToken -ProviderRef 'madeup://whatever' | Out-Null } catch { $t9unsupported = ($_.Exception.Message -match 'UNSUPPORTED') }
    Assert-True -Name 'T9 unknown scheme resolves to explicit UNSUPPORTED (no mock)' -Condition $t9unsupported
    $t9node = $false
    try { Resolve-BrokerToken -ProviderRef $baseArgs.TokenProviderRef | Out-Null } catch { $t9node = ($_.Exception.Message -match 'UNSUPPORTED') }
    Assert-True -Name 'T9 edda-node:// scheme resolves to explicit UNSUPPORTED on v0 (no mock)' -Condition $t9node
    $t9forbidden = $false
    try { Resolve-BrokerToken -ProviderRef 'env:GITHUB_TOKEN' | Out-Null } catch { $t9forbidden = ($_.Exception.Message -match 'UNSUPPORTED') }
    Assert-True -Name 'T9 env: scheme refuses outright' -Condition $t9forbidden

    # --- T10: gh token metadata drops token lines. (No gh auth is invoked here; we
    # test the reducer directly on fixture lines, including a masked token line.)
    $reducerSrc = (Get-Command Get-GhTokenSourceMetadata).ScriptBlock.ToString()
    Assert-True -Name 'T10 metadata reducer never surfaces raw Token lines (drops them)' `
        -Condition ($reducerSrc -match "match 'Token:'" -and $reducerSrc -match 'continue')

}
finally {
    Remove-Item -LiteralPath $fxRoot -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Output ("fixture tests: {0} passed, {1} failed" -f $script:pass, $script:fail)
if ($script:fail -gt 0) { exit 1 }
exit 0
