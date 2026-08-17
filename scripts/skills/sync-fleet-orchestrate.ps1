[CmdletBinding()]
param(
    [ValidateSet('Status', 'Sync')]
    [string]$Action = 'Status',

    [ValidateSet('Codex', 'Claude', 'All')]
    [string]$Target = 'All',

    [switch]$DryRun,
    [switch]$Force,
    [switch]$Json,

    [string]$CanonicalPath = (Join-Path $PSScriptRoot '../../skills/fleet-orchestrate'),
    [string]$CodexPath = (Join-Path $env:USERPROFILE '.agents/skills/fleet-orchestrate'),
    [string]$ClaudePath = (Join-Path $env:USERPROFILE '.claude/skills/fleet-orchestrate'),

    [string]$StagingPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$Utf8NoBom = [Text.UTF8Encoding]::new($false)

function Get-FullPath {
    param([string]$Path)

    [IO.Path]::GetFullPath($Path).TrimEnd([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar)
}

function Get-Sha256Hex {
    param([byte[]]$Bytes)

    $sha = [Security.Cryptography.SHA256]::Create()
    try {
        [Convert]::ToHexString($sha.ComputeHash($Bytes)).ToLowerInvariant()
    }
    finally {
        $sha.Dispose()
    }
}

function Get-HistoricalComparisonBytes {
    param([byte[]]$Bytes)

    if ([Array]::IndexOf($Bytes, [byte]0) -ge 0) { return $Bytes }
    try {
        $strictUtf8 = [Text.UTF8Encoding]::new($false, $true)
        $text = $strictUtf8.GetString($Bytes)
        if (-not $text.Contains("`r`n")) { return $Bytes }
        $Utf8NoBom.GetBytes($text.Replace("`r`n", "`n"))
    }
    catch {
        $Bytes
    }
}

function New-Manifest {
    param([object[]]$Entries)

    $byPath = [Collections.Generic.Dictionary[string, object]]::new([StringComparer]::Ordinal)
    foreach ($entry in $Entries) {
        $byPath.Add($entry.RelativePath, $entry)
    }
    [string[]]$paths = $byPath.Keys
    [Array]::Sort($paths, [StringComparer]::Ordinal)
    $sorted = foreach ($path in $paths) { $byPath[$path] }
    $lines = foreach ($entry in $sorted) {
        $encodedPath = [Convert]::ToBase64String($Utf8NoBom.GetBytes($entry.RelativePath))
        "$encodedPath`t$($entry.Length)`t$($entry.Sha256)"
    }
    $historicalLines = foreach ($entry in $sorted) {
        $encodedPath = [Convert]::ToBase64String($Utf8NoBom.GetBytes($entry.RelativePath))
        "$encodedPath`t$($entry.HistoricalLength)`t$($entry.HistoricalSha256)"
    }
    $serialized = if ($lines.Count -gt 0) { ($lines -join "`n") + "`n" } else { '' }
    $historicalSerialized = if ($historicalLines.Count -gt 0) { ($historicalLines -join "`n") + "`n" } else { '' }
    [pscustomobject]@{
        Entries          = @($sorted)
        Digest           = Get-Sha256Hex $Utf8NoBom.GetBytes($serialized)
        HistoricalDigest = Get-Sha256Hex $Utf8NoBom.GetBytes($historicalSerialized)
    }
}

function Get-DirectoryManifest {
    param([string]$Path)

    $entries = foreach ($file in Get-ChildItem -LiteralPath $Path -Recurse -File -Force) {
        $relative = [IO.Path]::GetRelativePath($Path, $file.FullName).Replace('\', '/')
        $bytes = [IO.File]::ReadAllBytes($file.FullName)
        $historicalBytes = Get-HistoricalComparisonBytes $bytes
        [pscustomobject]@{
            RelativePath     = $relative
            Length           = [long]$bytes.Length
            Sha256           = Get-Sha256Hex $bytes
            HistoricalLength = [long]$historicalBytes.Length
            HistoricalSha256 = Get-Sha256Hex $historicalBytes
        }
    }
    New-Manifest @($entries)
}

function Invoke-GitBytes {
    param([string]$WorkingDirectory, [string[]]$Arguments)

    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = 'git'
    $startInfo.UseShellExecute = $false
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $startInfo.CreateNoWindow = $true
    $startInfo.ArgumentList.Add('-C')
    $startInfo.ArgumentList.Add($WorkingDirectory)
    foreach ($argument in $Arguments) {
        $startInfo.ArgumentList.Add($argument)
    }

    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    try {
        if (-not $process.Start()) {
            throw 'git did not start'
        }
        $stderr = $process.StandardError.ReadToEndAsync()
        $output = [IO.MemoryStream]::new()
        try {
            $process.StandardOutput.BaseStream.CopyTo($output)
            $process.WaitForExit()
            $errorText = $stderr.GetAwaiter().GetResult()
            if ($process.ExitCode -ne 0) {
                throw "git exited $($process.ExitCode): $errorText"
            }
            $output.ToArray()
        }
        finally {
            $output.Dispose()
        }
    }
    finally {
        $process.Dispose()
    }
}

function Invoke-GitText {
    param([string]$WorkingDirectory, [string[]]$Arguments)

    $Utf8NoBom.GetString((Invoke-GitBytes $WorkingDirectory $Arguments)).Trim()
}

function Get-GitTreeManifest {
    param([string]$RepoRoot, [string]$Commit, [string]$CanonicalRelativePath)

    $treeBytes = Invoke-GitBytes $RepoRoot @('ls-tree', '-r', '-z', '--full-tree', $Commit, '--', $CanonicalRelativePath)
    $records = $Utf8NoBom.GetString($treeBytes).Split([char]0, [StringSplitOptions]::RemoveEmptyEntries)
    $entries = foreach ($record in $records) {
        $tab = $record.IndexOf("`t", [StringComparison]::Ordinal)
        if ($tab -lt 0) { continue }
        $header = $record.Substring(0, $tab).Split(' ', [StringSplitOptions]::RemoveEmptyEntries)
        if ($header.Count -lt 3 -or $header[1] -ne 'blob') { continue }
        $repoPath = $record.Substring($tab + 1).Replace('\', '/')
        $relative = $repoPath.Substring($CanonicalRelativePath.Length).TrimStart('/')
        $content = Invoke-GitBytes $RepoRoot @('cat-file', 'blob', $header[2])
        $historicalContent = Get-HistoricalComparisonBytes $content
        [pscustomobject]@{
            RelativePath     = $relative
            Length           = [long]$content.Length
            Sha256           = Get-Sha256Hex $content
            HistoricalLength = [long]$historicalContent.Length
            HistoricalSha256 = Get-Sha256Hex $historicalContent
        }
    }
    if (@($entries).Count -eq 0) { return $null }
    New-Manifest @($entries)
}

function Test-HistoricalCanonical {
    param([string]$Canonical, [object]$TargetManifest)

    try {
        $repoRoot = Get-FullPath (Invoke-GitText $Canonical @('rev-parse', '--show-toplevel'))
        $relative = [IO.Path]::GetRelativePath($repoRoot, $Canonical).Replace('\', '/')
        if ($relative -eq '..' -or $relative.StartsWith('../', [StringComparison]::Ordinal)) {
            return $false
        }
        $history = Invoke-GitText $repoRoot @('log', '--format=%H', '--all', '--', $relative)
        foreach ($commit in $history.Split("`n", [StringSplitOptions]::RemoveEmptyEntries)) {
            $manifest = Get-GitTreeManifest $repoRoot $commit.Trim() $relative
            if ($null -ne $manifest -and $manifest.HistoricalDigest -eq $TargetManifest.HistoricalDigest) {
                return $true
            }
        }
    }
    catch {
        # No usable repository history means the target is not safe to overwrite.
    }
    $false
}

function Get-Marker {
    param([string]$MarkerPath)

    if (-not (Test-Path -LiteralPath $MarkerPath -PathType Leaf)) {
        return [pscustomobject]@{ Exists = $false; Valid = $false; Schema = $null; CanonicalDigest = $null; InstalledDigest = $null }
    }
    try {
        $data = Get-Content -Raw -LiteralPath $MarkerPath | ConvertFrom-Json
        $schema = $data.PSObject.Properties['Schema']
        $canonicalDigest = $data.PSObject.Properties['CanonicalDigest']
        $installedDigest = $data.PSObject.Properties['InstalledDigest']
        $valid = $null -ne $schema -and $null -ne $canonicalDigest -and $null -ne $installedDigest -and $schema.Value -eq 1
        [pscustomobject]@{
            Exists = $true
            Valid = $valid
            Schema = if ($null -ne $schema) { $schema.Value } else { $null }
            CanonicalDigest = if ($null -ne $canonicalDigest) { $canonicalDigest.Value } else { $null }
            InstalledDigest = if ($null -ne $installedDigest) { $installedDigest.Value } else { $null }
        }
    }
    catch {
        [pscustomobject]@{ Exists = $true; Valid = $false; Schema = $null; CanonicalDigest = $null; InstalledDigest = $null }
    }
}

function Get-TargetState {
    param([string]$Name, [string]$Path, [object]$CanonicalManifest, [string]$Canonical)

    $fullPath = Get-FullPath $Path
    $markerPath = "$fullPath.edda-provenance.json"
    if (-not (Test-Path -LiteralPath $fullPath -PathType Container)) {
        return [pscustomobject]@{
            Target = $Name; Path = $fullPath; MarkerPath = $markerPath; Status = 'missing'
            Digest = $null; CanonicalDigest = $CanonicalManifest.Digest; MarkerCurrent = $false
        }
    }

    $manifest = Get-DirectoryManifest $fullPath
    $marker = Get-Marker $markerPath
    $markerCurrent = $marker.Valid -and
        $marker.CanonicalDigest -eq $CanonicalManifest.Digest -and
        $marker.InstalledDigest -eq $manifest.Digest

    if ($manifest.Digest -eq $CanonicalManifest.Digest) {
        $status = 'current'
    }
    elseif ($marker.Exists) {
        $status = if ($marker.Valid -and $marker.InstalledDigest -eq $manifest.Digest) { 'stale' } else { 'locally-modified' }
    }
    elseif (Test-HistoricalCanonical $Canonical $manifest) {
        $status = 'stale'
    }
    else {
        $status = 'locally-modified'
    }

    [pscustomobject]@{
        Target = $Name; Path = $fullPath; MarkerPath = $markerPath; Status = $status
        Digest = $manifest.Digest; CanonicalDigest = $CanonicalManifest.Digest; MarkerCurrent = $markerCurrent
    }
}

function Assert-TargetPath {
    param([string]$Candidate, [string]$Requested)

    if (-not [string]::Equals((Get-FullPath $Candidate), (Get-FullPath $Requested), [StringComparison]::OrdinalIgnoreCase)) {
        throw "Mutation path is not the exact requested target: $Candidate"
    }
}

function Assert-UniqueSibling {
    param([string]$Candidate, [string]$Requested, [ValidateSet('staging', 'backup')][string]$Kind)

    $candidateFull = Get-FullPath $Candidate
    $requestedFull = Get-FullPath $Requested
    $parent = [IO.Path]::GetDirectoryName($requestedFull)
    $candidateParent = [IO.Path]::GetDirectoryName($candidateFull)
    $prefix = [IO.Path]::GetFileName($requestedFull) + ".edda-$Kind-"
    if (-not [string]::Equals($parent, $candidateParent, [StringComparison]::OrdinalIgnoreCase) -or
        -not [IO.Path]::GetFileName($candidateFull).StartsWith($prefix, [StringComparison]::Ordinal) -or
        [IO.Path]::GetFileName($candidateFull).Length -le $prefix.Length) {
        throw "Mutation path is not a unique $Kind sibling of the requested target: $Candidate"
    }
}

function Write-Provenance {
    param([object]$State, [string]$Canonical, [string]$Digest)

    $markerPath = Get-FullPath $State.MarkerPath
    if (-not [string]::Equals($markerPath, "$($State.Path).edda-provenance.json", [StringComparison]::OrdinalIgnoreCase)) {
        throw "Invalid provenance path: $markerPath"
    }
    $temporary = Join-Path ([IO.Path]::GetDirectoryName($State.Path)) "$([IO.Path]::GetFileName($State.Path)).edda-staging-marker-$([guid]::NewGuid().ToString('N'))"
    Assert-UniqueSibling $temporary $State.Path staging
    $payload = [ordered]@{
        Schema = 1
        CanonicalDigest = $Digest
        InstalledDigest = $Digest
        CanonicalPath = $Canonical
        SyncedAtUtc = [DateTimeOffset]::UtcNow.ToString('O')
    } | ConvertTo-Json -Compress
    try {
        [IO.File]::WriteAllText($temporary, $payload + "`n", $Utf8NoBom)
        [IO.File]::Move($temporary, $markerPath, $true)
    }
    finally {
        if (Test-Path -LiteralPath $temporary -PathType Leaf) { [IO.File]::Delete($temporary) }
    }
}

function Copy-ToStage {
    param([string]$Canonical, [object]$CanonicalManifest, [string]$Stage)

    [IO.Directory]::CreateDirectory($Stage) | Out-Null
    foreach ($entry in $CanonicalManifest.Entries) {
        $source = Join-Path $Canonical $entry.RelativePath
        $destination = Get-FullPath (Join-Path $Stage $entry.RelativePath)
        $stagePrefix = (Get-FullPath $Stage) + [IO.Path]::DirectorySeparatorChar
        if (-not $destination.StartsWith($stagePrefix, [StringComparison]::OrdinalIgnoreCase)) {
            throw "Canonical relative path escapes staging: $($entry.RelativePath)"
        }
        [IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($destination)) | Out-Null
        [IO.File]::WriteAllBytes($destination, [IO.File]::ReadAllBytes($source))
    }
    $stagedManifest = Get-DirectoryManifest $Stage
    if ($stagedManifest.Digest -ne $CanonicalManifest.Digest) {
        throw "Staging parity failed: expected $($CanonicalManifest.Digest), got $($stagedManifest.Digest)"
    }
}

function Install-Target {
    param([object]$State, [string]$Canonical, [object]$CanonicalManifest, [bool]$KeepBackup, [string]$RequestedStage)

    Assert-TargetPath $State.Path $State.Path
    $parent = [IO.Path]::GetDirectoryName($State.Path)
    if (-not (Test-Path -LiteralPath $parent -PathType Container)) {
        throw "Target parent does not exist: $parent"
    }
    $stage = if ($RequestedStage) { Get-FullPath $RequestedStage } else {
        Join-Path $parent "$([IO.Path]::GetFileName($State.Path)).edda-staging-$([guid]::NewGuid().ToString('N'))"
    }
    Assert-UniqueSibling $stage $State.Path staging
    if (Test-Path -LiteralPath $stage) { throw "Staging path already exists: $stage" }

    $stageCreated = $false
    $oldTarget = $null
    $swapped = $false
    try {
        Copy-ToStage $Canonical $CanonicalManifest $stage
        $stageCreated = $true
        if (Test-Path -LiteralPath $State.Path -PathType Container) {
            $kind = if ($KeepBackup) { 'backup' } else { 'backup-transient' }
            $oldTarget = Join-Path $parent "$([IO.Path]::GetFileName($State.Path)).edda-$kind-$([guid]::NewGuid().ToString('N'))"
            Assert-UniqueSibling $oldTarget $State.Path backup
            [IO.Directory]::Move($State.Path, $oldTarget)
        }
        [IO.Directory]::Move($stage, $State.Path)
        $stageCreated = $false
        $swapped = $true
        Write-Provenance $State $Canonical $CanonicalManifest.Digest

        $installed = Get-DirectoryManifest $State.Path
        $marker = Get-Marker $State.MarkerPath
        if ($installed.Digest -ne $CanonicalManifest.Digest -or -not $marker.Valid -or
            $marker.CanonicalDigest -ne $CanonicalManifest.Digest -or $marker.InstalledDigest -ne $installed.Digest) {
            throw 'Post-sync parity or provenance verification failed'
        }
        if ($null -ne $oldTarget -and -not $KeepBackup) {
            Assert-UniqueSibling $oldTarget $State.Path backup
            [IO.Directory]::Delete($oldTarget, $true)
            $oldTarget = $null
        }
        return $oldTarget
    }
    catch {
        if ($swapped -and $null -ne $oldTarget -and (Test-Path -LiteralPath $oldTarget -PathType Container)) {
            if (Test-Path -LiteralPath $State.Path -PathType Container) {
                Assert-UniqueSibling $stage $State.Path staging
                [IO.Directory]::Move($State.Path, $stage)
                $stageCreated = $true
            }
            [IO.Directory]::Move($oldTarget, $State.Path)
            $oldTarget = $null
        }
        throw
    }
    finally {
        if ($stageCreated -and (Test-Path -LiteralPath $stage -PathType Container)) {
            Assert-UniqueSibling $stage $State.Path staging
            [IO.Directory]::Delete($stage, $true)
        }
    }
}

function New-Result {
    param([object]$State, [string]$Operation, [string]$PreviousStatus, [string]$BackupPath, [string]$ErrorMessage)

    [pscustomobject][ordered]@{
        Target = $State.Target
        Path = $State.Path
        Status = $State.Status
        PreviousStatus = $PreviousStatus
        Operation = $Operation
        CanonicalDigest = $State.CanonicalDigest
        InstalledDigest = $State.Digest
        MarkerPath = $State.MarkerPath
        MarkerCurrent = $State.MarkerCurrent
        BackupPath = $BackupPath
        Error = $ErrorMessage
    }
}

$results = @()
$exitCode = 0
try {
    $canonical = Get-FullPath $CanonicalPath
    if (-not (Test-Path -LiteralPath $canonical -PathType Container)) {
        throw "Canonical skill directory does not exist: $canonical"
    }
    $canonicalManifest = Get-DirectoryManifest $canonical
    if ($canonicalManifest.Entries.Count -eq 0) { throw "Canonical skill directory is empty: $canonical" }

    $selected = switch ($Target) {
        'Codex' { @([pscustomobject]@{ Name = 'Codex'; Path = $CodexPath }) }
        'Claude' { @([pscustomobject]@{ Name = 'Claude'; Path = $ClaudePath }) }
        'All' {
            @(
                [pscustomobject]@{ Name = 'Codex'; Path = $CodexPath }
                [pscustomobject]@{ Name = 'Claude'; Path = $ClaudePath }
            )
        }
    }
    if ($StagingPath -and $selected.Count -ne 1) { throw '-StagingPath requires a single target' }

    $states = foreach ($item in $selected) {
        $fullTarget = Get-FullPath $item.Path
        if ([string]::Equals($canonical, $fullTarget, [StringComparison]::OrdinalIgnoreCase)) {
            throw "Canonical and target paths must differ: $canonical"
        }
        Get-TargetState $item.Name $fullTarget $canonicalManifest $canonical
    }

    foreach ($state in $states) {
        $previousStatus = $state.Status
        if ($Action -eq 'Status') {
            $results += New-Result $state 'None' $previousStatus $null $null
            continue
        }
        if ($state.Status -eq 'locally-modified' -and -not $Force) {
            $results += New-Result $state 'Refused' $previousStatus $null 'Use -Force to preserve a backup and replace this locally modified target.'
            $exitCode = 1
            continue
        }
        if ($DryRun) {
            $operation = if ($state.Status -eq 'current') { if ($state.MarkerCurrent) { 'None' } else { 'WouldMark' } } else { 'WouldSync' }
            $results += New-Result $state $operation $previousStatus $null $null
            continue
        }

        try {
            $backup = $null
            if ($state.Status -eq 'current') {
                Write-Provenance $state $canonical $canonicalManifest.Digest
                $operation = if ($state.MarkerCurrent) { 'None' } else { 'Marked' }
            }
            else {
                $backup = Install-Target $state $canonical $canonicalManifest ($state.Status -eq 'locally-modified') $StagingPath
                $operation = 'Synced'
            }
            $current = Get-TargetState $state.Target $state.Path $canonicalManifest $canonical
            if ($current.Status -ne 'current' -or -not $current.MarkerCurrent) {
                throw 'Post-sync target is not current'
            }
            $results += New-Result $current $operation $previousStatus $backup $null
        }
        catch {
            $failed = Get-TargetState $state.Target $state.Path $canonicalManifest $canonical
            $results += New-Result $failed 'Failed' $previousStatus $null $_.Exception.Message
            $exitCode = 1
        }
    }
}
catch {
    $results = @([pscustomobject][ordered]@{
        Target = $Target; Path = $null; Status = 'error'; PreviousStatus = $null; Operation = 'Failed'
        CanonicalDigest = $null; InstalledDigest = $null; MarkerPath = $null; MarkerCurrent = $false
        BackupPath = $null; Error = $_.Exception.Message
    })
    $exitCode = 1
}

if ($Json) {
    ConvertTo-Json -InputObject @($results) -Depth 5 -Compress
}
else {
    $results
}
exit $exitCode
