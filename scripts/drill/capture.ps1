$ErrorActionPreference = 'Stop'
$script:DrillUtf8 = [Text.UTF8Encoding]::new($false)
$script:DrillClassifications = @('PASS', 'PRODUCT_RED', 'SAFETY_RED', 'HARNESS_RED')

function ConvertTo-DrillExitHex {
    param([Parameter(Mandatory = $true)][int32]$ExitCode)
    $bits = [BitConverter]::ToUInt32([BitConverter]::GetBytes($ExitCode), 0)
    '0x{0:X8}' -f $bits
}

function Get-DrillSha256 {
    param([Parameter(Mandatory = $true)][string]$Path)
    $stream = [IO.File]::OpenRead($Path)
    $sha = [Security.Cryptography.SHA256]::Create()
    try { ([BitConverter]::ToString($sha.ComputeHash($stream))).Replace('-', '') }
    finally {
        $sha.Dispose()
        $stream.Dispose()
    }
}

function Write-DrillAtomicNewText {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [AllowEmptyString()][Parameter(Mandatory = $true)][string]$Text
    )
    $temporary = "$Path.$([guid]::NewGuid().ToString('N')).tmp"
    try {
        [IO.File]::WriteAllText($temporary, $Text, $script:DrillUtf8)
        [IO.File]::Move($temporary, $Path)
    }
    finally {
        if ([IO.File]::Exists($temporary)) { [IO.File]::Delete($temporary) }
    }
}

function Write-DrillRecord {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)]$Record
    )
    $json = $Record | ConvertTo-Json -Depth 12 -Compress
    Write-DrillAtomicNewText -Path $Path -Text ($json + [Environment]::NewLine)
}

function Get-DrillSpecValue {
    param($Spec, [Parameter(Mandatory = $true)][string]$Name)
    if ($Spec -is [Collections.IDictionary]) { return $Spec[$Name] }
    $Spec.$Name
}

function Assert-DrillSpec {
    param([Parameter(Mandatory = $true)]$Spec)
    $captureId = [string](Get-DrillSpecValue $Spec 'capture_id')
    $owner = [string](Get-DrillSpecValue $Spec 'owner')
    $executable = [string](Get-DrillSpecValue $Spec 'executable')
    $cwd = [string](Get-DrillSpecValue $Spec 'cwd')
    $classification = [string](Get-DrillSpecValue $Spec 'classification')
    if ([string]::IsNullOrWhiteSpace($captureId)) { throw 'capture_id is required before process start' }
    if ([string]::IsNullOrWhiteSpace($owner)) { throw 'owner is required before process start' }
    if ([string]::IsNullOrWhiteSpace($executable)) { throw 'executable is required before process start' }
    if ([string]::IsNullOrWhiteSpace($cwd)) { throw 'cwd is required before process start' }
    if (-not [IO.File]::Exists($executable)) { throw "executable must be a literal file before process start: $executable" }
    if (-not [IO.Directory]::Exists($cwd)) { throw "cwd must exist before process start: $cwd" }
    if ([string]::IsNullOrWhiteSpace($classification)) { $classification = 'PASS' }
    if ($classification -notin $script:DrillClassifications) { throw "invalid drill classification: $classification" }
    [pscustomobject]@{
        capture_id = $captureId
        safe_id = $captureId -replace '[^A-Za-z0-9._-]', '_'
        owner = $owner
        executable = [IO.Path]::GetFullPath($executable)
        cwd = [IO.Path]::GetFullPath($cwd)
        argv = @((Get-DrillSpecValue $Spec 'argv'))
        classification = $classification
    }
}

function Get-DrillBoundedText {
    param([AllowNull()][string]$Text, [Parameter(Mandatory = $true)][int]$MaxChars)
    if ($null -eq $Text) { $Text = '' }
    $length = $Text.Length
    [ordered]@{
        text = if ($length -le $MaxChars) { $Text } else { $Text.Substring(0, $MaxChars) }
        total_chars = $length
        truncated = $length -gt $MaxChars
    }
}

function Get-DrillProcessIdentity {
    param([Parameter(Mandatory = $true)][Diagnostics.Process]$Process)
    $exitedBeforeQuery = $Process.HasExited
    try {
        [ordered]@{
            process_creation_utc = $Process.StartTime.ToUniversalTime().ToString('o')
            identity_status = if ($exitedBeforeQuery) { 'exited_before_identity_query_recovered' } else { 'captured' }
            identity_error = $null
        }
    }
    catch {
        [ordered]@{
            process_creation_utc = $null
            identity_status = if ($exitedBeforeQuery) { 'exited_before_identity_query_unavailable' } else { 'identity_query_failed' }
            identity_error = $_.Exception.Message
        }
    }
}

function Invoke-DrillCaptureSet {
    param(
        [Parameter(Mandatory = $true)][string]$SetId,
        [Parameter(Mandatory = $true)][object[]]$Spec,
        [Parameter(Mandatory = $true)][string]$OutputDirectory,
        [ValidateRange(1, 1048576)][int]$MaxChars = 65536,
        [scriptblock]$OptionalMetadata
    )

    if ([string]::IsNullOrWhiteSpace($SetId)) { throw 'SetId is required before process start' }
    if ($Spec.Count -eq 0) { throw 'at least one process spec is required' }
    $validated = @($Spec | ForEach-Object { Assert-DrillSpec $_ })
    $safeIds = @($validated.safe_id)
    if (($safeIds | Select-Object -Unique).Count -ne $safeIds.Count) { throw 'capture IDs collide after filename sanitization' }

    [IO.Directory]::CreateDirectory($OutputDirectory) | Out-Null
    $setSafeId = $SetId -replace '[^A-Za-z0-9._-]', '_'
    $destinations = @()
    foreach ($item in $validated) {
        foreach ($suffix in @('00-planned.json', '10-started.json', '20-terminal.json', '30-optional.json', '30-optional-error.json', 'stdout.txt', 'stderr.txt')) {
            $destinations += Join-Path $OutputDirectory "$($item.safe_id).$suffix"
        }
    }
    $overlapPath = Join-Path $OutputDirectory "$setSafeId.20-overlap.json"
    $destinations += $overlapPath
    foreach ($path in $destinations) {
        if ([IO.File]::Exists($path)) { throw "append-only capture destination already exists: $path" }
    }

    $states = @()
    foreach ($item in $validated) {
        $planned = [ordered]@{
            schema = 'edda-drill-process-v1'
            phase = 'planned'
            capture_id = $item.capture_id
            owner = $item.owner
            executable = $item.executable
            argv = @($item.argv)
            cwd = $item.cwd
            utc_planned = [DateTimeOffset]::UtcNow.ToString('o')
        }
        Write-DrillRecord -Path (Join-Path $OutputDirectory "$($item.safe_id).00-planned.json") -Record $planned
        $states += [pscustomobject]@{
            spec = $item
            planned = $planned
            process = $null
            stdout_task = $null
            stderr_task = $null
            utc_start = $null
            process_id = $null
            process_creation_utc = $null
            identity_status = 'not_started'
            identity_error = $null
            start_error = $null
        }
    }

    # Launch every child before asking the OS for either child's identity.
    foreach ($state in $states) {
        $state.utc_start = [DateTimeOffset]::UtcNow.ToString('o')
        try {
            $psi = [Diagnostics.ProcessStartInfo]::new()
            $psi.FileName = $state.spec.executable
            $psi.WorkingDirectory = $state.spec.cwd
            $psi.UseShellExecute = $false
            $psi.CreateNoWindow = $true
            $psi.RedirectStandardOutput = $true
            $psi.RedirectStandardError = $true
            $psi.StandardOutputEncoding = $script:DrillUtf8
            $psi.StandardErrorEncoding = $script:DrillUtf8
            foreach ($argument in $state.spec.argv) { [void]$psi.ArgumentList.Add([string]$argument) }
            $process = [Diagnostics.Process]::new()
            $process.StartInfo = $psi
            [void]$process.Start()
            $state.process = $process
            $state.process_id = $process.Id
            $state.stdout_task = $process.StandardOutput.ReadToEndAsync()
            $state.stderr_task = $process.StandardError.ReadToEndAsync()
        }
        catch {
            $state.start_error = $_.Exception.ToString()
            $state.identity_status = 'start_failed'
        }
    }

    foreach ($state in $states) {
        if ($null -ne $state.process) {
            $identity = Get-DrillProcessIdentity -Process $state.process
            $state.process_creation_utc = $identity.process_creation_utc
            $state.identity_status = $identity.identity_status
            $state.identity_error = $identity.identity_error
        }
        $started = [ordered]@{
            schema = 'edda-drill-process-v1'
            phase = 'started'
            capture_id = $state.spec.capture_id
            owner = $state.spec.owner
            executable = $state.spec.executable
            argv = @($state.spec.argv)
            cwd = $state.spec.cwd
            utc_start = $state.utc_start
            pid = $state.process_id
            process_creation_utc = $state.process_creation_utc
            identity_status = $state.identity_status
            identity_error = $state.identity_error
            start_error = $state.start_error
        }
        Write-DrillRecord -Path (Join-Path $OutputDirectory "$($state.spec.safe_id).10-started.json") -Record $started
    }

    $terminals = @()
    $optionalRecords = @()
    foreach ($state in $states) {
        $stdoutText = ''
        $stderrText = if ($state.start_error) { $state.start_error } else { '' }
        $exitSigned = $null
        $processExitUtc = $null
        if ($null -ne $state.process) {
            $state.process.WaitForExit()
            $stdoutText = $state.stdout_task.GetAwaiter().GetResult()
            $stderrText = $state.stderr_task.GetAwaiter().GetResult()
            $exitSigned = [int32]$state.process.ExitCode
            try { $processExitUtc = $state.process.ExitTime.ToUniversalTime().ToString('o') } catch { $processExitUtc = $null }
        }
        $utcEnd = [DateTimeOffset]::UtcNow.ToString('o')
        $stdout = Get-DrillBoundedText -Text $stdoutText -MaxChars $MaxChars
        $stderr = Get-DrillBoundedText -Text $stderrText -MaxChars $MaxChars
        $stdoutPath = Join-Path $OutputDirectory "$($state.spec.safe_id).stdout.txt"
        $stderrPath = Join-Path $OutputDirectory "$($state.spec.safe_id).stderr.txt"
        Write-DrillAtomicNewText -Path $stdoutPath -Text $stdout.text
        Write-DrillAtomicNewText -Path $stderrPath -Text $stderr.text
        $stdout.path = $stdoutPath
        $stdout.sha256 = Get-DrillSha256 -Path $stdoutPath
        $stderr.path = $stderrPath
        $stderr.sha256 = Get-DrillSha256 -Path $stderrPath
        $terminal = [ordered]@{
            schema = 'edda-drill-process-v1'
            phase = 'terminal'
            classification = $state.spec.classification
            capture_id = $state.spec.capture_id
            owner = $state.spec.owner
            executable = $state.spec.executable
            argv = @($state.spec.argv)
            cwd = $state.spec.cwd
            utc_start = $state.utc_start
            process_creation_utc = $state.process_creation_utc
            utc_end = $utcEnd
            process_exit_utc = $processExitUtc
            pid = $state.process_id
            identity_status = $state.identity_status
            identity_error = $state.identity_error
            exit_signed = $exitSigned
            start_error = $state.start_error
            stdout = $stdout
            stderr = $stderr
        }
        Write-DrillRecord -Path (Join-Path $OutputDirectory "$($state.spec.safe_id).20-terminal.json") -Record $terminal
        $terminals += $terminal

        try {
            $metadata = if ($OptionalMetadata) { & $OptionalMetadata $terminal } else { $null }
            $optional = [ordered]@{
                schema = 'edda-drill-process-v1'
                phase = 'optional'
                classification = 'OPTIONAL_METADATA'
                status = 'captured'
                capture_id = $state.spec.capture_id
                exit_u32_hex = if ($null -eq $exitSigned) { $null } else { ConvertTo-DrillExitHex -ExitCode $exitSigned }
                metadata = $metadata
            }
            Write-DrillRecord -Path (Join-Path $OutputDirectory "$($state.spec.safe_id).30-optional.json") -Record $optional
        }
        catch {
            $optional = [ordered]@{
                schema = 'edda-drill-process-v1'
                phase = 'optional'
                classification = 'OPTIONAL_METADATA'
                status = 'failed'
                capture_id = $state.spec.capture_id
                error = $_.Exception.Message
            }
            Write-DrillRecord -Path (Join-Path $OutputDirectory "$($state.spec.safe_id).30-optional-error.json") -Record $optional
        }
        $optionalRecords += $optional
    }

    $intervals = @($terminals | Where-Object { $_.process_creation_utc -and $_.process_exit_utc })
    $latestStart = $null
    $earliestExit = $null
    $overlap = $false
    if ($intervals.Count -ge 2) {
        $latestStart = @($intervals | ForEach-Object { [DateTimeOffset]::ParseExact($_.process_creation_utc, 'o', [Globalization.CultureInfo]::InvariantCulture) } | Sort-Object)[-1]
        $earliestExit = @($intervals | ForEach-Object { [DateTimeOffset]::ParseExact($_.process_exit_utc, 'o', [Globalization.CultureInfo]::InvariantCulture) } | Sort-Object)[0]
        $overlap = $latestStart -lt $earliestExit
    }
    $overlapRecord = [ordered]@{
        schema = 'edda-drill-overlap-v1'
        phase = 'terminal'
        set_id = $SetId
        capture_ids = @($terminals.capture_id)
        overlap = $overlap
        latest_start_utc = if ($null -eq $latestStart) { $null } else { $latestStart.ToString('o') }
        earliest_exit_utc = if ($null -eq $earliestExit) { $null } else { $earliestExit.ToString('o') }
    }
    Write-DrillRecord -Path $overlapPath -Record $overlapRecord
    [pscustomobject]@{ Terminals = $terminals; Optional = $optionalRecords; Overlap = $overlapRecord }
}

function Invoke-DrillCapture {
    param(
        [Parameter(Mandatory = $true)][string]$CaptureId,
        [Parameter(Mandatory = $true)][AllowEmptyString()][string]$Owner,
        [Parameter(Mandatory = $true)][string]$Executable,
        [Parameter(Mandatory = $true)][string]$WorkingDirectory,
        [string[]]$ArgumentList = @(),
        [Parameter(Mandatory = $true)][string]$OutputDirectory,
        [ValidateSet('PASS', 'PRODUCT_RED', 'SAFETY_RED', 'HARNESS_RED')][string]$Classification = 'PASS',
        [ValidateRange(1, 1048576)][int]$MaxChars = 65536,
        [scriptblock]$OptionalMetadata
    )
    $spec = @{
        capture_id = $CaptureId
        owner = $Owner
        executable = $Executable
        cwd = $WorkingDirectory
        argv = @($ArgumentList)
        classification = $Classification
    }
    $result = Invoke-DrillCaptureSet -SetId $CaptureId -Spec @($spec) -OutputDirectory $OutputDirectory -MaxChars $MaxChars -OptionalMetadata $OptionalMetadata
    [pscustomobject]@{ Terminal = @($result.Terminals)[0]; Optional = @($result.Optional)[0] }
}
