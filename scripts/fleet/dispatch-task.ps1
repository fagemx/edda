# Embedded in edda: standalone Windows detached process supervisor (GH-605).
param(
  [ValidateSet('Launch', 'Run')][string]$Mode,
  [Parameter(Mandatory)][string]$Config
)
$ErrorActionPreference = 'Stop'
$c = Get-Content -Raw -LiteralPath $Config | ConvertFrom-Json
if ($Mode -eq 'Launch') {
  $pwsh = (Get-Process -Id $PID).Path
  # This is deliberately the controller process's actual creation time, not
  # launch time.  The lane reaper uses the pair to avoid matching a recycled
  # Windows PID after a controller has died.
  $controller = Get-Process -Id ([int]$c.controller_pid) -ErrorAction Stop
  $controllerStarted = $controller.StartTime.ToUniversalTime().ToString('o')
  $description = "# lane-reap: controller-pid=$($c.controller_pid) controller-started=$controllerStarted"
  # The reaper reads the generated wrapper, which survives a dead supervisor.
  # Append the canonical metadata before registering the task so a PID can
  # never be confused with a later process that reused its numeric value.
  Add-Content -LiteralPath $PSCommandPath -Value $description -Encoding utf8
  $quotedScript = $PSCommandPath.Replace('"', '\"')
  $quotedConfig = $Config.Replace('"', '\"')
  $action = New-ScheduledTaskAction -Execute $pwsh -Argument "-NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -File `"$quotedScript`" -Mode Run -Config `"$quotedConfig`"" -WorkingDirectory $c.cwd
  $settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Seconds ($c.timeout + 30)) -MultipleInstances IgnoreNew -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
  Register-ScheduledTask -TaskName $c.task -Action $action -Settings $settings -Description $description -RunLevel Limited | Out-Null
  try { Start-ScheduledTask -TaskName $c.task }
  catch { Unregister-ScheduledTask -TaskName $c.task -Confirm:$false; throw }
  exit 0
}

$m = Get-Content -Raw -LiteralPath $c.manifest | ConvertFrom-Json
$p = $null
$outFile = $null
$errFile = $null
$code = 1
function Write-Manifest([object]$Value) {
  $temporary = "$($c.manifest).new"
  $Value | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $temporary -Encoding utf8
  Move-Item -LiteralPath $temporary -Destination $c.manifest -Force
}
try {
  $start = [Diagnostics.ProcessStartInfo]::new($c.executable)
  $start.WorkingDirectory = $c.cwd
  $start.UseShellExecute = $false
  $start.CreateNoWindow = $true
  $start.RedirectStandardOutput = $true
  $start.RedirectStandardError = $true
  $start.RedirectStandardInput = $true
  foreach ($arg in $c.argv) { $start.ArgumentList.Add([string]$arg) }
  foreach ($property in $c.environment.PSObject.Properties) { $start.Environment[$property.Name] = [string]$property.Value }
  $start.Environment['HOME'] = $c.home
  if ($c.cargo) { $start.Environment['CARGO_TARGET_DIR'] = $c.cargo }
  else { [void]$start.Environment.Remove('CARGO_TARGET_DIR') }
  $p = [Diagnostics.Process]::Start($start)
  $p.StandardInput.Close()
  $m.worker_pid = $p.Id
  $m.state = 'running'
  Write-Manifest $m
  $outFile = [IO.FileStream]::new($c.log, [IO.FileMode]::Create, [IO.FileAccess]::Write, [IO.FileShare]::ReadWrite, 1, $true)
  $errFile = [IO.FileStream]::new(($c.log + '.err'), [IO.FileMode]::Create, [IO.FileAccess]::Write, [IO.FileShare]::ReadWrite, 1, $true)
  $outCopy = $p.StandardOutput.BaseStream.CopyToAsync($outFile)
  $errCopy = $p.StandardError.BaseStream.CopyToAsync($errFile)
  if (-not $p.WaitForExit([int]($c.timeout * 1000))) {
    $p.Kill($true)
    $p.WaitForExit()
    $m.state = 'timeout'
    $code = 2
  } else { $code = $p.ExitCode; $m.state = 'completed' }
  [Threading.Tasks.Task]::WaitAll(@($outCopy, $errCopy))
} catch {
  $m.state = 'failed'
  $m.error = $_.Exception.Message
  if ($p -and -not $p.HasExited) { $p.Kill($true); $p.WaitForExit() }
} finally {
  if ($outFile) { $outFile.Dispose() }
  if ($errFile) { $errFile.Dispose() }
  $m.exit_code = $code
  Write-Manifest $m
  Unregister-ScheduledTask -TaskName $c.task -Confirm:$false -ErrorAction Continue
}
exit $code
