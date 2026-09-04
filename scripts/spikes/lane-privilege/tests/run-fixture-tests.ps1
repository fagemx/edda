# run-fixture-tests.ps1 — fixture tests for the GH-690 lane privilege spike harness.
#
# Scope: syntax-level validation of the no-op preflight and the refusal branches,
# plus execution-path tests of the publication/metadata logic with injected safe
# synthetic dependencies (no real git pushes, no gh auth operations, no network).
# Deliberately does NOT touch real credentials, change ACLs or accounts, build,
# or push. Run anywhere (operator principal is fine — the tests assert the refusal
# paths, not the positive path).
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
    $aArgs.Add('ProtectedCredentialFiles', @((Join-Path $fxRoot 'operator-profile\.claude\.credentials.json')))
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
    # Calls run in-process (proper argument passing — no source interpolation,
    # Round1 F6), including a quote-bearing fixture path.
    $quoteDir = Join-Path $fxRoot "quote'dir"
    New-Item -ItemType Directory -Path $quoteDir -Force | Out-Null
    $canaryFile = Join-Path $quoteDir 'readable-secret.txt'
    $canary = 'CANARY-gho_FAKEFIXTURETOKEN0000000-DO-NOT-PRINT'
    Set-Content -LiteralPath $canaryFile -Value $canary -NoNewline

    $probeResult = Get-CredentialFileProbe -Path $canaryFile
    Assert-True -Name 'T6a probe of readable file reports Readable' `
        -Condition ("$probeResult" -eq 'Readable') -Detail "$probeResult"
    Assert-True -Name 'T6a probe of a quote-bearing path works (paths are data, not code)' `
        -Condition ("$probeResult" -eq 'Readable') -Detail $canaryFile

    $missing = Get-CredentialFileProbe -Path (Join-Path $fxRoot 'does-not-exist.json')
    Assert-True -Name 'T6b probe of missing file reports NotFound (inconclusive)' `
        -Condition ("$missing" -eq 'NotFound') -Detail "$missing"

    # T6c (Round1 F5): an exclusive lock held by this test process must yield
    # INCONCLUSIVE — a sharing violation is not access control.
    $lockedFile = Join-Path $fxRoot 'locked-fixture.txt'
    Set-Content -LiteralPath $lockedFile -Value 'fixture-bytes' -NoNewline
    $lockStream = [System.IO.File]::Open($lockedFile, [System.IO.FileMode]::Open,
        [System.IO.FileAccess]::Read, [System.IO.FileShare]::None)
    try {
        $locked = Get-CredentialFileProbe -Path $lockedFile
        Assert-True -Name 'T6c sharing violation reports Inconclusive, NOT AccessDenied' `
            -Condition ("$locked" -eq 'Inconclusive') -Detail "$locked"
    }
    finally {
        $lockStream.Dispose()
    }
    $afterLock = Get-CredentialFileProbe -Path $lockedFile
    Assert-True -Name 'T6c same file after lock release reports Readable (lock was the blocker)' `
        -Condition ("$afterLock" -eq 'Readable') -Detail "$afterLock"

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

    # --- T10 (Round1 F1): metadata reducer tested on EXECUTION, with synthetic
    # status lines — not by searching function source.
    $synthetic = @(
        'github.com',
        '  ✓ Logged in to github.com account someuser (keyring)',
        '  - Active account: true',
        '  - Token: gho_************ (keyring)'
    )
    $m = Get-GhTokenSourceMetadata -StatusText $synthetic
    Assert-True -Name 'T10 keyring line parsed: token present with source keyring' `
        -Condition ($m.TokenPresent -and $m.TokenSource -eq 'keyring') -Detail "$($m.TokenPresent)/$($m.TokenSource)"
    Assert-True -Name 'T10 login line parsed' -Condition ($m.LoggedIn -and $m.Account -eq 'someuser') -Detail $m.Account
    Assert-True -Name 'T10 reducer returns no token VALUE (masked form absent from object)' `
        -Condition (-not ("$m" -match 'gho_[A-Za-z0-9]'))

    $mEnv = Get-GhTokenSourceMetadata -StatusText @('  - Token: ghp_************ (env)')
    Assert-True -Name 'T10 env source parsed' -Condition ($mEnv.TokenPresent -and $mEnv.TokenSource -eq 'env')
    $mInfile = Get-GhTokenSourceMetadata -StatusText @('  - Token: ghs_************ (infile)')
    Assert-True -Name 'T10 infile source parsed' -Condition ($mInfile.TokenPresent -and $mInfile.TokenSource -eq 'infile')
    $mUnknown = Get-GhTokenSourceMetadata -StatusText @('  - Token: redacted-but-unparsable')
    Assert-True -Name 'T10 unparsable token line => present with source unknown' `
        -Condition ($mUnknown.TokenPresent -and $mUnknown.TokenSource -eq 'unknown')
    $mNone = Get-GhTokenSourceMetadata -StatusText @('  - no token detected')
    Assert-True -Name 'T10 explicit absence line => no token' -Condition (-not $mNone.TokenPresent)

    # Verdict adjudication across all cases (fail closed on uncertainty).
    $v1 = Get-GhMetadataVerdict -Metadata $m            # keyring token
    $v2 = Get-GhMetadataVerdict -Metadata $mUnknown     # unknown source token
    $v3 = Get-GhMetadataVerdict -Metadata $mNone        # not logged in
    $v4 = Get-GhMetadataVerdict -Metadata @{ LoggedIn = $true; TokenPresent = $false }  # ambiguous
    $v5 = Get-GhMetadataVerdict -Metadata $mNone -GhExitCode 1                          # gh error
    Assert-True -Name 'T10 verdict: keyring token => deny' -Condition ($v1 -eq 'deny-token-present')
    Assert-True -Name 'T10 verdict: unknown-source token => deny (fail closed)' -Condition ($v2 -eq 'deny-token-present')
    Assert-True -Name 'T10 verdict: no login => ok' -Condition ($v3 -eq 'ok-no-login')
    Assert-True -Name 'T10 verdict: logged-in-without-token => inconclusive' -Condition ($v4 -eq 'inconclusive-ambiguous')
    Assert-True -Name 'T10 verdict: gh error exit => inconclusive' -Condition ($v5 -eq 'inconclusive-error')

    # --- T11 (Round1 F2): push destination binding, pure matcher + repo-level.
    $t11 = 0
    foreach ($bad in @(
        'git@github.com:evil/repo.git',
        'https://github.com/evil/repo',
        'ssh://git@github.com/fagemx/edda-other.git',
        'git@github.com:../traversal.git',
        'not-a-url',
        'https://gitlab.com/fagemx/edda.git'
    )) {
        if (-not (Test-PushUrlAllowed -Url $bad -RepoAllowList @('fagemx/edda'))) { $t11++ }
    }
    Assert-True -Name 'T11 matcher refuses all 6 disallowed/malformed destinations' -Condition ($t11 -eq 6) -Detail "refused=$t11"
    Assert-True -Name 'T11 matcher accepts ssh form of allowlisted repo' `
        -Condition (Test-PushUrlAllowed -Url 'git@github.com:fagemx/edda.git' -RepoAllowList @('fagemx/edda'))
    Assert-True -Name 'T11 matcher accepts https form of allowlisted repo' `
        -Condition (Test-PushUrlAllowed -Url 'https://github.com/fagemx/edda' -RepoAllowList @('fagemx/edda'))

    # Repo-level resolution against a real local git config (no network, no push).
    git init -q (Join-Path $fxRoot 'wsrepo') 2>$null
    git -C (Join-Path $fxRoot 'wsrepo') remote add origin 'git@github.com:evil/repo.git' 2>$null
    $t11Repo = $false
    try { Get-EffectivePushUrl -WorkspacePath (Join-Path $fxRoot 'wsrepo') -RepoAllowList @('fagemx/edda') | Out-Null }
    catch { $t11Repo = ($_.Exception.Message -match 'does not resolve to an allowlisted') }
    Assert-True -Name 'T11 repo-level guard refuses disallowed configured origin' -Condition $t11Repo
    git -C (Join-Path $fxRoot 'wsrepo') remote set-url origin 'git@github.com:fagemx/edda.git' 2>$null
    $t11Ok = $false
    try { $t11Ok = (Get-EffectivePushUrl -WorkspacePath (Join-Path $fxRoot 'wsrepo') -RepoAllowList @('fagemx/edda')) -match 'fagemx/edda' } catch {}
    Assert-True -Name 'T11 repo-level guard accepts allowlisted configured origin' -Condition ([bool]$t11Ok)

    # --- T12 (Round1 F3): publication path with an INJECTED synthetic git invoker.
    # Records every invocation (args + stdin) and returns scripted results.
    $fakeToken = 'gho_FAKEFIXTURETOKEN0000000'
    $secure = ConvertTo-SecureString $fakeToken -AsPlainText -Force
    $calls = [System.Collections.Generic.List[object]]::new()
    $synthInvoker = {
        param([string[]]$GitArgs, [string]$StdinInput, [string]$WorkingDirectory)
        $calls.Add([pscustomobject]@{ Args = $GitArgs; Stdin = $StdinInput })
        # Scripted behavior by subcommand.
        $joined = $GitArgs -join ' '
        if ($joined -match 'credential approve') { return [pscustomobject]@{ ExitCode = 0; Output = @(); Args = $GitArgs; Stdin = $StdinInput } }
        if ($joined -match 'credential reject')  { return [pscustomobject]@{ ExitCode = 0; Output = @(); Args = $GitArgs; Stdin = $StdinInput } }
        if ($joined -match 'credential-cache exit') { return [pscustomobject]@{ ExitCode = 0; Output = @(); Args = $GitArgs; Stdin = $null } }
        if ($joined -match ' push ') { return [pscustomobject]@{ ExitCode = 0; Output = @('synthetic push ok'); Args = $GitArgs; Stdin = $null } }
        return [pscustomobject]@{ ExitCode = 1; Output = @("unexpected git invocation: $joined"); Args = $GitArgs; Stdin = $StdinInput }
    }

    # T12a: happy path — approve, push, cleanup all succeed; token NEVER in argv.
    $calls.Clear()
    $r = Invoke-SpikePublication -Token $secure -WorkspacePath (Join-Path $fxRoot 'ws') -RemoteName 'origin' `
        -RefSpec 'HEAD:refs/heads/spike/lane-privilege-fixture' -GitInvoker $synthInvoker
    Assert-True -Name 'T12a publication verdict published' -Condition ($r.Verdict -eq 'published') -Detail "$($r.Verdict); $($r.Notes -join '; ')"
    $allArgs = ($calls | ForEach-Object { $_.Args -join ' ' }) -join "`n"
    Assert-True -Name 'T12a token NEVER appears in any git argv' -Condition (-not $allArgs.Contains($fakeToken))
    $approveStdin = (@($calls | Where-Object { ($_.Args -join ' ') -match 'credential approve' })[0]).Stdin
    Assert-True -Name 'T12a token reaches git ONLY via credential-approve stdin' `
        -Condition ($null -ne $approveStdin -and $approveStdin.Contains($fakeToken))
    $pushCall = @($calls | Where-Object { ($_.Args -join ' ') -match ' push ' })[0]
    Assert-True -Name 'T12a helper list is isolated (credential.helper= reset before cache)' `
        -Condition (($pushCall.Args -contains 'credential.helper=') -and
                    (@($pushCall.Args | Where-Object { $_ -match '^credential\.helper=cache' }).Count -ge 1))
    Assert-True -Name 'T12a push uses one explicit refspec and disables tag-following' `
        -Condition (($pushCall.Args -join ' ') -match 'push\.followTags=false' -and
                    ($pushCall.Args -join ' ') -match 'HEAD:refs/heads/spike/lane-privilege-fixture')
    Assert-True -Name 'T12a cleanup runs after push (reject + cache exit recorded)' `
        -Condition ((@($calls | Where-Object { ($_.Args -join ' ') -match 'credential reject' }).Count -eq 1) -and
                    (@($calls | Where-Object { ($_.Args -join ' ') -match 'credential-cache exit' }).Count -eq 1))
    Assert-True -Name 'T12a no gh auth invocation exists in the publication path' `
        -Condition (-not ($allArgs -match 'gh auth| auth |auth login'))

    # T12b: approve fails => auth-failed, push NEVER invoked, cleanup still attempted.
    $calls.Clear()
    $failInvoker = {
        param([string[]]$GitArgs, [string]$StdinInput, [string]$WorkingDirectory)
        $calls.Add([pscustomobject]@{ Args = $GitArgs; Stdin = $StdinInput })
        $joined = $GitArgs -join ' '
        if ($joined -match 'credential approve') { return [pscustomobject]@{ ExitCode = 1; Output = @('synthetic approve failure'); Args = $GitArgs; Stdin = $StdinInput } }
        if ($joined -match 'credential-cache exit') { return [pscustomobject]@{ ExitCode = 0; Output = @(); Args = $GitArgs; Stdin = $null } }
        return [pscustomobject]@{ ExitCode = 0; Output = @(); Args = $GitArgs; Stdin = $StdinInput }
    }
    $r2 = Invoke-SpikePublication -Token $secure -WorkspacePath (Join-Path $fxRoot 'ws') -RemoteName 'origin' `
        -RefSpec 'HEAD:refs/heads/spike/lane-privilege-fixture' -GitInvoker $failInvoker
    Assert-True -Name 'T12b approve failure => auth-failed' -Condition ($r2.Verdict -eq 'auth-failed')
    Assert-True -Name 'T12b push never invoked when approve fails' `
        -Condition ((@($calls | Where-Object { ($_.Args -join ' ') -match ' push ' }).Count -eq 0))

    # T12c: push fails => push-failed, cleanup still attempted and observable.
    $calls.Clear()
    $pushFailInvoker = {
        param([string[]]$GitArgs, [string]$StdinInput, [string]$WorkingDirectory)
        $calls.Add([pscustomobject]@{ Args = $GitArgs; Stdin = $StdinInput })
        $joined = $GitArgs -join ' '
        if ($joined -match ' push ') { return [pscustomobject]@{ ExitCode = 128; Output = @('synthetic push failure'); Args = $GitArgs; Stdin = $null } }
        return [pscustomobject]@{ ExitCode = 0; Output = @(); Args = $GitArgs; Stdin = $StdinInput }
    }
    $r3 = Invoke-SpikePublication -Token $secure -WorkspacePath (Join-Path $fxRoot 'ws') -RemoteName 'origin' `
        -RefSpec 'HEAD:refs/heads/spike/lane-privilege-fixture' -GitInvoker $pushFailInvoker
    Assert-True -Name 'T12c push failure => push-failed' -Condition ($r3.Verdict -eq 'push-failed')
    Assert-True -Name 'T12c cleanup still attempted after push failure' `
        -Condition ((@($calls | Where-Object { ($_.Args -join ' ') -match 'credential reject' }).Count -eq 1))

    # T12d: cleanup failure is observable, not silently swallowed.
    $cleanupFailInvoker = {
        param([string[]]$GitArgs, [string]$StdinInput, [string]$WorkingDirectory)
        $joined = $GitArgs -join ' '
        if ($joined -match 'credential reject') { return [pscustomobject]@{ ExitCode = 1; Output = @(); Args = $GitArgs; Stdin = $StdinInput } }
        if ($joined -match 'credential-cache exit') { return [pscustomobject]@{ ExitCode = 1; Output = @(); Args = $GitArgs; Stdin = $null } }
        if ($joined -match ' push ') { return [pscustomobject]@{ ExitCode = 0; Output = @(); Args = $GitArgs; Stdin = $null } }
        return [pscustomobject]@{ ExitCode = 0; Output = @(); Args = $GitArgs; Stdin = $StdinInput }
    }
    $r4 = Invoke-SpikePublication -Token $secure -WorkspacePath (Join-Path $fxRoot 'ws') -RemoteName 'origin' `
        -RefSpec 'HEAD:refs/heads/spike/lane-privilege-fixture' -GitInvoker $cleanupFailInvoker
    Assert-True -Name 'T12d failed cleanup => cleanup-incomplete (observable)' -Condition ($r4.Verdict -eq 'cleanup-incomplete' -and $r4.CleanupExit -eq 1)

    # T12e: token moves as SecureString; plaintext helper exists in memory only.
    $plainRoundtrip = ConvertFrom-SecureStringInMemory -SecureValue $secure
    Assert-True -Name 'T12e SecureString roundtrip yields the synthetic token in memory' -Condition ($plainRoundtrip -eq $fakeToken)

}
finally {
    Remove-Item -LiteralPath $fxRoot -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Output ("fixture tests: {0} passed, {1} failed" -f $script:pass, $script:fail)
if ($script:fail -gt 0) { exit 1 }
exit 0
