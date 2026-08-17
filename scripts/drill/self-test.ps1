param()

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'capture.ps1')

function Assert-Drill([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw "self-test: $Message" }
}

$utf8 = [Text.UTF8Encoding]::new($false)
$testRoot = Join-Path ([IO.Path]::GetTempPath()) "edda drill self-test $([guid]::NewGuid().ToString('N'))"
$unicodeRoot = Join-Path $testRoot '路徑 with spaces'
$pwsh = (Get-Command pwsh -ErrorAction Stop).Source
$childScript = Join-Path $unicodeRoot 'child probe.ps1'

try {
    [IO.Directory]::CreateDirectory($unicodeRoot) | Out-Null
    [IO.File]::WriteAllText($childScript, @'
param([string]$Mode, [string]$Text)
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
[Console]::InputEncoding = [Text.UTF8Encoding]::new($false)
if ($Mode -eq 'slow') { Start-Sleep -Milliseconds 700 }
if ($Mode -eq 'noisy') {
    [Console]::Out.Write(('o' * 80))
    [Console]::Error.Write(('e' * 80))
    exit 7
}
[Console]::Out.Write("$($PWD.Path)|$Text")
'@, $utf8)

    Assert-Drill ((ConvertTo-DrillExitHex -ExitCode -1) -eq '0xFFFFFFFF') 'negative exit conversion lost Int32 bits'

    $fastDir = Join-Path $unicodeRoot 'fast result'
    $fast = Invoke-DrillCapture `
        -CaptureId 'fast' `
        -Owner 'self-test' `
        -Executable $pwsh `
        -WorkingDirectory $unicodeRoot `
        -ArgumentList @('-NoProfile', '-File', $childScript, '-Mode', 'fast', '-Text', 'hello 世界 with spaces') `
        -OutputDirectory $fastDir
    Assert-Drill ($fast.Terminal.exit_signed -eq 0) 'fast child exit was not captured'
    Assert-Drill ($fast.Terminal.pid -gt 0) 'fast child PID is missing'
    Assert-Drill ($fast.Terminal.owner -eq 'self-test') 'ownership field is missing'
    Assert-Drill ($fast.Terminal.cwd -eq $unicodeRoot) 'cwd was not recorded exactly'
    Assert-Drill ($fast.Terminal.stdout.text -eq "$unicodeRoot|hello 世界 with spaces") "ArgumentList or non-ASCII path was altered: '$($fast.Terminal.stdout.text)'"
    Assert-Drill (Test-Path -LiteralPath (Join-Path $fastDir 'fast.00-planned.json')) 'planned record was not preserved'
    Assert-Drill (Test-Path -LiteralPath (Join-Path $fastDir 'fast.20-terminal.json')) 'terminal record was not preserved'

    $startText = $fast.Terminal.utc_start
    $endText = $fast.Terminal.utc_end
    $start = [DateTimeOffset]::ParseExact($startText, 'o', [Globalization.CultureInfo]::InvariantCulture)
    $end = [DateTimeOffset]::ParseExact($endText, 'o', [Globalization.CultureInfo]::InvariantCulture)
    Assert-Drill ($start.Offset -eq [TimeSpan]::Zero -and $end.Offset -eq [TimeSpan]::Zero) 'timestamps are not UTC round-trippable'
    $jsonRoundTrip = @{ utc_start = $startText; utc_end = $endText } | ConvertTo-Json -Compress | ConvertFrom-Json
    Assert-Drill ($jsonRoundTrip.utc_start -eq $startText -and $jsonRoundTrip.utc_end -eq $endText) 'JSON timestamp round-trip changed the value'

    $noisy = Invoke-DrillCapture `
        -CaptureId 'bounded' `
        -Owner 'self-test' `
        -Executable $pwsh `
        -WorkingDirectory $unicodeRoot `
        -ArgumentList @('-NoProfile', '-File', $childScript, '-Mode', 'noisy') `
        -OutputDirectory (Join-Path $unicodeRoot 'bounded result') `
        -MaxChars 32
    Assert-Drill ($noisy.Terminal.exit_signed -eq 7) 'signed exit was not retained'
    Assert-Drill ($noisy.Terminal.stdout.truncated -and $noisy.Terminal.stderr.truncated) 'output was not bounded'
    Assert-Drill ($noisy.Terminal.stdout.text.Length -eq 32 -and $noisy.Terminal.stderr.text.Length -eq 32) 'bounded output length is wrong'
    Assert-Drill ($noisy.Terminal.stdout.sha256 -eq (Get-DrillSha256 -Path $noisy.Terminal.stdout.path)) 'stdout hash mismatch'
    Assert-Drill ($noisy.Terminal.stderr.sha256 -eq (Get-DrillSha256 -Path $noisy.Terminal.stderr.path)) 'stderr hash mismatch'

    $concurrentDir = Join-Path $unicodeRoot 'concurrent result'
    $specs = @(
        @{ capture_id = 'A'; owner = 'self-test'; executable = $pwsh; cwd = $unicodeRoot; argv = @('-NoProfile', '-File', $childScript, '-Mode', 'slow', '-Text', 'A') },
        @{ capture_id = 'B'; owner = 'self-test'; executable = $pwsh; cwd = $unicodeRoot; argv = @('-NoProfile', '-File', $childScript, '-Mode', 'slow', '-Text', 'B') }
    )
    $concurrent = Invoke-DrillCaptureSet -SetId 'overlap' -Spec $specs -OutputDirectory $concurrentDir
    Assert-Drill ($concurrent.Overlap.overlap) 'two-process OS intervals did not overlap'
    Assert-Drill ($concurrent.Overlap.latest_start_utc -lt $concurrent.Overlap.earliest_exit_utc) 'overlap proof interval is invalid'
    Assert-Drill (($concurrent.Terminals | Where-Object { -not $_.process_creation_utc }).Count -eq 0) 'slow-process creation time is missing'

    $fastExitSpecs = @(
        @{ capture_id = 'gone'; owner = 'self-test'; executable = $env:ComSpec; cwd = $unicodeRoot; argv = @('/d', '/c', 'exit 0') },
        @{ capture_id = 'delay'; owner = 'self-test'; executable = $pwsh; cwd = $unicodeRoot; argv = @('-NoProfile', '-File', $childScript, '-Mode', 'slow', '-Text', 'delay') }
    )
    $fastExit = Invoke-DrillCaptureSet -SetId 'fast-exit' -Spec $fastExitSpecs -OutputDirectory (Join-Path $unicodeRoot 'fast exit result')
    $gone = $fastExit.Terminals | Where-Object capture_id -eq 'gone'
    Assert-Drill ($gone.exit_signed -eq 0) 'process exiting before identity query was not completed honestly'
    $identityProbe = [Diagnostics.Process]::Start($env:ComSpec, '/d /c exit 0')
    $identityProbe.WaitForExit()
    $exitedIdentity = Get-DrillProcessIdentity -Process $identityProbe
    Assert-Drill ($exitedIdentity.identity_status -match '^exited_before_identity_query') 'fast exit was not identified honestly'

    $optionalDir = Join-Path $unicodeRoot 'optional failure result'
    $optional = Invoke-DrillCapture `
        -CaptureId 'optional' `
        -Owner 'self-test' `
        -Executable $env:ComSpec `
        -WorkingDirectory $unicodeRoot `
        -ArgumentList @('/d', '/c', 'exit 0') `
        -OutputDirectory $optionalDir `
        -OptionalMetadata { throw 'deliberate optional formatter failure' }
    Assert-Drill ($optional.Terminal.pid -gt 0 -and $optional.Terminal.utc_start -and $optional.Terminal.utc_end) 'optional failure erased ownership or timing'
    Assert-Drill ($optional.Optional.classification -eq 'OPTIONAL_METADATA') 'optional failure classification is wrong'
    Assert-Drill (Test-Path -LiteralPath (Join-Path $optionalDir 'optional.20-terminal.json')) 'optional failure erased terminal record'

    $marker = Join-Path $unicodeRoot 'must-not-run.txt'
    $requiredFailed = $false
    try {
        Invoke-DrillCapture `
            -CaptureId 'required-failure' `
            -Owner '' `
            -Executable $pwsh `
            -WorkingDirectory $unicodeRoot `
            -ArgumentList @('-NoProfile', '-Command', "[IO.File]::WriteAllText('$marker','ran')") `
            -OutputDirectory (Join-Path $unicodeRoot 'required failure result') | Out-Null
    }
    catch { $requiredFailed = $true }
    Assert-Drill $requiredFailed 'missing required ownership did not fail'
    Assert-Drill (-not (Test-Path -LiteralPath $marker)) 'required ownership failure acted on a PID'

    [pscustomobject]@{ status = 'PASS'; checks = 8; root = $testRoot }
}
finally {
    if (Test-Path -LiteralPath $testRoot) { Remove-Item -LiteralPath $testRoot -Recurse -Force }
}
