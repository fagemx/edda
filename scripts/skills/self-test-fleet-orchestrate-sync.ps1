[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$ToolPath = Join-Path $PSScriptRoot 'sync-fleet-orchestrate.ps1'
$Utf8NoBom = [Text.UTF8Encoding]::new($false)

function Assert-True {
    param([bool]$Condition, [string]$Message)

    if (-not $Condition) {
        throw "ASSERTION FAILED: $Message"
    }
}

function Write-FixtureFile {
    param([string]$Path, [string]$Content)

    [IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($Path)) | Out-Null
    [IO.File]::WriteAllText($Path, $Content, $Utf8NoBom)
}

function Copy-FixtureTree {
    param([string]$Source, [string]$Destination)

    [IO.Directory]::CreateDirectory($Destination) | Out-Null
    foreach ($file in Get-ChildItem -LiteralPath $Source -Recurse -File) {
        $relative = [IO.Path]::GetRelativePath($Source, $file.FullName)
        $copy = Join-Path $Destination $relative
        [IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($copy)) | Out-Null
        [IO.File]::WriteAllBytes($copy, [IO.File]::ReadAllBytes($file.FullName))
    }
}

function Get-FixtureDigest {
    param([string]$Path)

    if (-not (Test-Path -LiteralPath $Path -PathType Container)) {
        return $null
    }

    $lines = [Collections.Generic.List[string]]::new()
    foreach ($file in Get-ChildItem -LiteralPath $Path -Recurse -File) {
        $relative = [IO.Path]::GetRelativePath($Path, $file.FullName).Replace('\', '/')
        $bytes = [IO.File]::ReadAllBytes($file.FullName)
        $sha = [Security.Cryptography.SHA256]::Create()
        try {
            $hash = [Convert]::ToHexString($sha.ComputeHash($bytes)).ToLowerInvariant()
        }
        finally {
            $sha.Dispose()
        }
        $lines.Add("$relative`t$($bytes.Length)`t$hash")
    }
    $sorted = $lines.ToArray()
    [Array]::Sort($sorted, [StringComparer]::Ordinal)
    $manifestBytes = $Utf8NoBom.GetBytes(($sorted -join "`n") + "`n")
    $manifestSha = [Security.Cryptography.SHA256]::Create()
    try {
        return [Convert]::ToHexString($manifestSha.ComputeHash($manifestBytes)).ToLowerInvariant()
    }
    finally {
        $manifestSha.Dispose()
    }
}

function Invoke-SyncTool {
    param([string[]]$Arguments)

    $output = & (Get-Process -Id $PID).Path -NoProfile -NonInteractive -File $ToolPath @Arguments 2>&1 | Out-String
    $exitCode = $LASTEXITCODE
    $data = $null
    if ($output.Trim()) {
        try {
            $data = ConvertFrom-Json $output
        }
        catch {
            throw "Tool did not return JSON. Exit=$exitCode Output=$output"
        }
    }
    [pscustomobject]@{ ExitCode = $exitCode; Data = @($data); Raw = $output }
}

function Remove-FixtureTree {
    param([string]$Path, [string]$FixtureRoot)

    $fullPath = [IO.Path]::GetFullPath($Path)
    $fullRoot = [IO.Path]::GetFullPath($FixtureRoot).TrimEnd('\') + '\'
    if (-not [string]::Equals($fullPath.TrimEnd('\'), $fullRoot.TrimEnd('\'), [StringComparison]::OrdinalIgnoreCase) -and
        -not $fullPath.StartsWith($fullRoot, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing fixture cleanup outside $fullRoot"
    }
    if (Test-Path -LiteralPath $fullPath) {
        Remove-Item -LiteralPath $fullPath -Recurse -Force
    }
}

Assert-True (Test-Path -LiteralPath $ToolPath -PathType Leaf) 'sync tool exists'

$fixtureRoot = Join-Path ([IO.Path]::GetTempPath()) "edda fleet sync 測試 $([guid]::NewGuid().ToString('N'))"
[IO.Directory]::CreateDirectory($fixtureRoot) | Out-Null

try {
    $repo = Join-Path $fixtureRoot 'tiny history repo'
    $canonical = Join-Path $repo 'skills/fleet-orchestrate'
    $codex = Join-Path $fixtureRoot 'Codex 安裝/skills/fleet-orchestrate'
    $claude = Join-Path $fixtureRoot 'Claude install/skills/fleet-orchestrate'
    [IO.Directory]::CreateDirectory($repo) | Out-Null

    & git -C $repo init --quiet
    & git -C $repo config user.email 'fixture@example.invalid'
    & git -C $repo config user.name 'Fixture'
    & git -C $repo config core.autocrlf false
    Write-FixtureFile (Join-Path $canonical 'SKILL.md') "# Fleet Orchestrate`n`nOld policy fixture.`n"
    Write-FixtureFile (Join-Path $canonical 'agents/openai.yaml') "name: fleet-orchestrate`n"
    Write-FixtureFile (Join-Path $canonical 'references/playbook.md') "# Old playbook`n"
    & git -C $repo add --all
    & git -C $repo commit --quiet -m 'old canonical'
    Copy-FixtureTree $canonical $codex
    Copy-FixtureTree $canonical $claude
    foreach ($install in @($codex, $claude)) {
        $agentFile = Join-Path $install 'agents/openai.yaml'
        $checkoutText = [IO.File]::ReadAllText($agentFile).Replace("`n", "`r`n")
        [IO.File]::WriteAllText($agentFile, $checkoutText, $Utf8NoBom)
    }
    $oldDigest = Get-FixtureDigest $codex

    Write-FixtureFile (Join-Path $canonical 'SKILL.md') "# Fleet Orchestrate`n`nThis is a bounded complete review, never a minimal review.`n"
    Write-FixtureFile (Join-Path $canonical 'references/playbook.md') "# Current playbook`n`nBounded scope.`n"
    Write-FixtureFile (Join-Path $canonical 'references/review-scope-pressure-tests.md') "# Pressure tests`n"
    & git -C $repo add --all
    & git -C $repo commit --quiet -m 'current canonical'
    $canonicalDigest = Get-FixtureDigest $canonical

    $common = @('-CanonicalPath', $canonical, '-CodexPath', $codex, '-ClaudePath', $claude, '-Json')

    $status = Invoke-SyncTool (@('-Action', 'Status', '-Target', 'Codex') + $common)
    Assert-True ($status.ExitCode -eq 0) 'status succeeds for historical install'
    Assert-True ($status.Data[0].Status -eq 'stale') 'old unmarked canonical copy is stale'
    Assert-True ((Get-FixtureDigest $codex) -eq $oldDigest) 'status writes nothing'
    Assert-True (-not (Test-Path -LiteralPath "$codex.edda-provenance.json")) 'status does not create provenance'

    $beforeDryRun = @(Get-ChildItem -LiteralPath ([IO.Path]::GetDirectoryName($codex)) -Force | ForEach-Object Name | Sort-Object)
    $dryRun = Invoke-SyncTool (@('-Action', 'Sync', '-Target', 'Codex', '-DryRun') + $common)
    $afterDryRun = @(Get-ChildItem -LiteralPath ([IO.Path]::GetDirectoryName($codex)) -Force | ForEach-Object Name | Sort-Object)
    Assert-True ($dryRun.ExitCode -eq 0) 'dry-run succeeds'
    Assert-True ($dryRun.Data[0].Operation -eq 'WouldSync') 'dry-run reports intended sync'
    Assert-True ((Get-FixtureDigest $codex) -eq $oldDigest) 'dry-run preserves target bytes'
    Assert-True (($beforeDryRun -join '|') -eq ($afterDryRun -join '|')) 'dry-run creates no sibling files'

    $env:EDDA_FLEET_SYNC_TEST_ROOT = $fixtureRoot
    $env:EDDA_FLEET_SYNC_TEST_FAULT = 'TargetChanged'
    try {
        $boundaryRefused = Invoke-SyncTool (@('-Action', 'Sync', '-Target', 'Codex') + $common)
    }
    finally {
        Remove-Item Env:EDDA_FLEET_SYNC_TEST_FAULT, Env:EDDA_FLEET_SYNC_TEST_ROOT -ErrorAction SilentlyContinue
    }
    Assert-True ($boundaryRefused.ExitCode -ne 0) 'mutation-boundary change is refused without Force'
    Assert-True ($boundaryRefused.Data[0].Operation -eq 'Refused') 'mutation-boundary refusal is reported'
    $changedBeforeForce = [IO.File]::ReadAllText((Join-Path $codex 'SKILL.md'))

    $env:EDDA_FLEET_SYNC_TEST_ROOT = $fixtureRoot
    $env:EDDA_FLEET_SYNC_TEST_FAULT = 'TargetChanged'
    try {
        $boundaryForced = Invoke-SyncTool (@('-Action', 'Sync', '-Target', 'Codex', '-Force') + $common)
    }
    finally {
        Remove-Item Env:EDDA_FLEET_SYNC_TEST_FAULT, Env:EDDA_FLEET_SYNC_TEST_ROOT -ErrorAction SilentlyContinue
    }
    Assert-True ($boundaryForced.ExitCode -eq 0) 'Force accepts a mutation-boundary change'
    Assert-True (Test-Path -LiteralPath $boundaryForced.Data[0].BackupPath -PathType Container) 'Force preserves a mutation-boundary backup'
    $expectedChangedBytes = $changedBeforeForce + "`nTEST BOUNDARY MUTATION`n"
    Assert-True ([IO.File]::ReadAllText((Join-Path $boundaryForced.Data[0].BackupPath 'SKILL.md')) -eq $expectedChangedBytes) 'Force backup preserves the changed bytes'

    $canonicalBeforeOverlap = Get-FixtureDigest $canonical
    $targetBelowCanonical = Join-Path $canonical 'nested target'
    $belowResult = Invoke-SyncTool @('-Action', 'Sync', '-Target', 'Codex', '-CanonicalPath', $canonical, '-CodexPath', $targetBelowCanonical, '-Json')
    Assert-True ($belowResult.ExitCode -ne 0) 'target below canonical is rejected'
    Assert-True (-not (Test-Path -LiteralPath $targetBelowCanonical)) 'target-below-canonical rejection writes nothing'
    Assert-True ((Get-FixtureDigest $canonical) -eq $canonicalBeforeOverlap) 'target-below-canonical rejection preserves canonical'

    $targetAboveCanonical = Join-Path $fixtureRoot 'ancestor target'
    $canonicalBelowTarget = Join-Path $targetAboveCanonical 'canonical child'
    Copy-FixtureTree $canonical $canonicalBelowTarget
    $aboveBefore = Get-FixtureDigest $targetAboveCanonical
    $aboveResult = Invoke-SyncTool @('-Action', 'Sync', '-Target', 'Codex', '-Force', '-CanonicalPath', $canonicalBelowTarget, '-CodexPath', $targetAboveCanonical, '-Json')
    Assert-True ($aboveResult.ExitCode -ne 0) 'canonical below target is rejected'
    Assert-True ((Get-FixtureDigest $targetAboveCanonical) -eq $aboveBefore) 'canonical-below-target rejection writes nothing'
    Assert-True (Test-Path -LiteralPath $canonicalBelowTarget -PathType Container) 'canonical-below-target rejection preserves canonical'

    $claudeBeforeStageMove = Get-FixtureDigest $claude
    $env:EDDA_FLEET_SYNC_TEST_ROOT = $fixtureRoot
    $env:EDDA_FLEET_SYNC_TEST_FAULT = 'StageMove'
    try {
        $stageMoveFailure = Invoke-SyncTool (@('-Action', 'Sync', '-Target', 'Claude') + $common)
    }
    finally {
        Remove-Item Env:EDDA_FLEET_SYNC_TEST_FAULT, Env:EDDA_FLEET_SYNC_TEST_ROOT -ErrorAction SilentlyContinue
    }
    Assert-True ($stageMoveFailure.ExitCode -ne 0) 'stage move failure is reported'
    Assert-True (Test-Path -LiteralPath $claude -PathType Container) 'stage move failure leaves a live target'
    Assert-True ((Get-FixtureDigest $claude) -eq $claudeBeforeStageMove) 'stage move failure restores the complete old target'

    $env:EDDA_FLEET_SYNC_TEST_ROOT = $fixtureRoot
    $env:EDDA_FLEET_SYNC_TEST_FAULT = 'Cleanup'
    try {
        $cleanupFailure = Invoke-SyncTool (@('-Action', 'Sync', '-Target', 'Claude') + $common)
    }
    finally {
        Remove-Item Env:EDDA_FLEET_SYNC_TEST_FAULT, Env:EDDA_FLEET_SYNC_TEST_ROOT -ErrorAction SilentlyContinue
    }
    Assert-True ($cleanupFailure.ExitCode -ne 0) 'cleanup failure is reported'
    Assert-True ($cleanupFailure.Data[0].Operation -eq 'CleanupFailed') 'cleanup failure has a distinct operation'
    Assert-True ((Get-FixtureDigest $claude) -eq $canonicalDigest) 'cleanup failure leaves the complete new target live'
    Assert-True (Test-Path -LiteralPath $cleanupFailure.Data[0].BackupPath -PathType Container) 'cleanup failure reports the remaining backup'
    Assert-True ($cleanupFailure.Data[0].Error.Contains('injected cleanup failure')) 'cleanup error is explicit'

    $syncAll = Invoke-SyncTool (@('-Action', 'Sync', '-Target', 'All') + $common)
    Assert-True ($syncAll.ExitCode -eq 0) 'normal sync succeeds for both targets'
    Assert-True ($syncAll.Data.Count -eq 2) 'all returns both target results'
    Assert-True ((Get-FixtureDigest $codex) -eq $canonicalDigest) 'Codex target has exact parity'
    Assert-True ((Get-FixtureDigest $claude) -eq $canonicalDigest) 'Claude target has exact parity'
    $boundedReviewText = 'This is a bounded complete review, never a minimal review.'
    Assert-True ((Get-Content -Raw -LiteralPath (Join-Path $codex 'SKILL.md')).Contains($boundedReviewText)) 'Codex load sees bounded review text'
    Assert-True ((Get-Content -Raw -LiteralPath (Join-Path $claude 'SKILL.md')).Contains($boundedReviewText)) 'Claude load sees bounded review text'
    foreach ($result in $syncAll.Data) {
        Assert-True ($result.Status -eq 'current') 'post-sync status is current'
        Assert-True ($result.MarkerCurrent -eq $true) 'post-sync provenance is current'
    }

    $otherCanonical = Join-Path $repo 'other/fleet-orchestrate'
    Copy-FixtureTree $canonical $otherCanonical
    & git -C $repo add --all
    & git -C $repo commit --quiet -m 'other canonical lineage path'
    $beforeForeignCanonical = Get-FixtureDigest $codex
    $foreignArgs = @('-CanonicalPath', $otherCanonical, '-CodexPath', $codex, '-Json')
    $foreignStatus = Invoke-SyncTool (@('-Action', 'Status', '-Target', 'Codex') + $foreignArgs)
    Assert-True ($foreignStatus.Data[0].Status -eq 'locally-modified') 'marker from another canonical identity is locally modified'
    $foreignRefused = Invoke-SyncTool (@('-Action', 'Sync', '-Target', 'Codex') + $foreignArgs)
    Assert-True ($foreignRefused.ExitCode -ne 0) 'another canonical identity is refused without Force'
    Assert-True ($foreignRefused.Data[0].Operation -eq 'Refused') 'another canonical refusal is reported'
    Assert-True ((Get-FixtureDigest $codex) -eq $beforeForeignCanonical) 'another canonical refusal preserves target bytes'

    Add-Content -LiteralPath (Join-Path $codex 'SKILL.md') -Value 'local edit' -Encoding utf8
    $modifiedDigest = Get-FixtureDigest $codex
    $modified = Invoke-SyncTool (@('-Action', 'Status', '-Target', 'Codex') + $common)
    Assert-True ($modified.Data[0].Status -eq 'locally-modified') 'changed marked target is locally modified'
    $refused = Invoke-SyncTool (@('-Action', 'Sync', '-Target', 'Codex') + $common)
    Assert-True ($refused.ExitCode -ne 0) 'normal sync refuses local modifications'
    Assert-True ($refused.Data[0].Operation -eq 'Refused') 'refusal is reported'
    Assert-True ((Get-FixtureDigest $codex) -eq $modifiedDigest) 'refused sync preserves target bytes'

    $forced = Invoke-SyncTool (@('-Action', 'Sync', '-Target', 'Codex', '-Force') + $common)
    Assert-True ($forced.ExitCode -eq 0) 'forced sync succeeds'
    Assert-True ($forced.Data[0].Status -eq 'current') 'forced sync reports current'
    Assert-True ((Get-FixtureDigest $codex) -eq $canonicalDigest) 'forced sync restores parity'
    Assert-True (Test-Path -LiteralPath $forced.Data[0].BackupPath -PathType Container) 'forced sync reports a preserved backup'
    Assert-True ((Get-FixtureDigest $forced.Data[0].BackupPath) -eq $modifiedDigest) 'forced backup preserves modified bytes'

    Remove-FixtureTree $claude $fixtureRoot
    Remove-Item -LiteralPath "$claude.edda-provenance.json" -Force
    $missing = Invoke-SyncTool (@('-Action', 'Status', '-Target', 'Claude') + $common)
    Assert-True ($missing.Data[0].Status -eq 'missing') 'absent target is missing'
    $missingSync = Invoke-SyncTool (@('-Action', 'Sync', '-Target', 'Claude') + $common)
    Assert-True ($missingSync.ExitCode -eq 0) 'missing target sync succeeds'
    Assert-True ((Get-FixtureDigest $claude) -eq $canonicalDigest) 'missing target reaches parity'

    $outsideCanonical = Join-Path $fixtureRoot '沒有 git canonical'
    $outsideTarget = Join-Path $fixtureRoot 'unmarked custom target'
    Copy-FixtureTree $canonical $outsideCanonical
    Copy-FixtureTree $canonical $outsideTarget
    Add-Content -LiteralPath (Join-Path $outsideTarget 'SKILL.md') -Value 'unknown old tree' -Encoding utf8
    $noHistory = Invoke-SyncTool @('-Action', 'Status', '-Target', 'Codex', '-CanonicalPath', $outsideCanonical, '-CodexPath', $outsideTarget, '-Json')
    Assert-True ($noHistory.ExitCode -eq 0) 'status survives unavailable history'
    Assert-True ($noHistory.Data[0].Status -eq 'locally-modified') 'unavailable history fails safe as locally modified'

    Add-Content -LiteralPath (Join-Path $canonical 'SKILL.md') -Value 'next canonical' -Encoding utf8
    & git -C $repo add --all
    & git -C $repo commit --quiet -m 'next canonical'
    $beforeStageFailure = Get-FixtureDigest $codex
    $blockedStage = Join-Path ([IO.Path]::GetDirectoryName($codex)) 'fleet-orchestrate.edda-staging-fixed-fixture'
    Write-FixtureFile $blockedStage 'blocks staging directory creation'
    $stageFailure = Invoke-SyncTool (@('-Action', 'Sync', '-Target', 'Codex', '-StagingPath', $blockedStage) + $common)
    Assert-True ($stageFailure.ExitCode -ne 0) 'staging failure is reported'
    Assert-True ((Get-FixtureDigest $codex) -eq $beforeStageFailure) 'staging failure retains the target'

    Write-Output 'PASS: fleet-orchestrate sync self-test'
}
finally {
    Remove-FixtureTree $fixtureRoot $fixtureRoot
}
