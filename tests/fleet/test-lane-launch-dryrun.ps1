# test-lane-launch-dryrun.ps1 — regression test for GH-822.
#
# Defect: lane-launch.ps1 -DryRun derived $Log/$Done as $Name.log/$Name.done
# and never redirected them inside the -DryRun branch, so the dry-run wrapper
# teed its output into the REAL lane's log and wrote the REAL lane's
# done-file. A real launch after a dry run then started with a foreign
# "=== EXIT code=0 ===" line already in $Name.log: a running lane looked
# already-exited and a killed lane looked cleanly finished (defeating the
# GH-672 contract that a log with START but no EXIT means "killed").
#
# What this test proves (issue #822 doneWhen):
#   1. No foreign EXIT in the real log (AC1): dry-run first, then a real
#      launch whose wrapper is left to finish NATURALLY — no lane-stop — so
#      the end record proven is the real pipeline's own: $Name.log contains
#      EXACTLY ONE "=== EXIT" line, it matches "=== EXIT code=<n> ==="
#      (numeric — never lane-stop's "stopped" sentinel), and the done-file
#      holds exactly that numeric code. Pre-fix this fails: the dry-run's
#      own EXIT line is already in $Name.log, so the count is 2.
#   2. Clean LogDir after dry-run (AC2): a dry-run on a clean -LogDir leaves
#      no $Name.log / $Name.done; every file it creates matches $Name.dryrun*.
#   3. Printed summary transparency (AC3): the dry-run summary prints
#      "dry-run log=<path>" and that path differs from the real log path.
#   4. The dry-run wrapper embeds the dryrun log/done paths (mechanism check).
#   5. lane-status.ps1 does not surface dry-run artifacts as lane status
#      (AC5): sampled while the real lane is live (the row is reported
#      exactly because the task exists) and again after the task is
#      unregistered — `lane-status -Name X` exits nonzero even though
#      X.log / X.done / X.dryrun* files all exist in -LogDir; status is
#      derived from scheduled tasks only, never from artifact files, so a
#      leftover dry-run cannot look like a lane.
#   6. Guard rail (P1-1): a -Name containing the dryrun segment (e.g.
#      'X.dryrun', 'X_dryrun', 'X-dryrun') is rejected, so a real lane can
#      never resolve its $Name.log / $Name.done onto another lane's dry-run
#      artifacts.
#
# The real-launch leg runs one trivial `edda dispatch` and polls up to 45s
# for the wrapper to finish on its own (bounded by -TimeoutSec 90); if
# `edda` or the agent is unavailable the dispatch fails fast and the wrapper
# still writes its EXIT record, so the assertion does not depend on agent
# spend. Pass -SkipRealLaunch to run only the dry-run legs.
#
# Cleanup never uses raw Stop-/Unregister-ScheduledTask (P1-2): they kill
# only the wrapper and can orphan the agent process tree, so cleanup runs
# lane-stop.ps1 — the sanctioned full-tree kill, in a child pwsh because its
# Fail calls exit — before unregistering and removing the temp directories.
#
# usage:
#   pwsh -NoProfile -File tests/fleet/test-lane-launch-dryrun.ps1 [-SkipRealLaunch]
#
# Exit codes: 0 = all assertions passed; 1 = at least one failed (details on
# stdout; the scratch -LogDir is kept for inspection and its path printed).

param([switch]$SkipRealLaunch)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..\..')).Path
$launch = Join-Path $repoRoot 'scripts\fleet\lane-launch.ps1'
$status = Join-Path $repoRoot 'scripts\fleet\lane-status.ps1'
$stop = Join-Path $repoRoot 'scripts\fleet\lane-stop.ps1'

$script:failures = [System.Collections.Generic.List[string]]::new()
function Assert-True([bool]$Cond, [string]$What) {
  if ($Cond) { "PASS: $What" }
  else { "FAIL: $What"; $script:failures.Add($What) | Out-Null }
}

# Run a script file in a child pwsh with a timeout; captures stdout, stderr,
# and the exit code without depending on the caller's profile.
function Invoke-ChildScript([string]$File, [string[]]$ScriptArgs, [int]$TimeoutSec = 240) {
  $psi = [System.Diagnostics.ProcessStartInfo]::new()
  $psi.FileName = (Get-Command pwsh.exe).Source
  $psi.Arguments = "-NoProfile -NonInteractive -ExecutionPolicy Bypass -File `"$File`" $($ScriptArgs -join ' ')"
  $psi.RedirectStandardOutput = $true
  $psi.RedirectStandardError = $true
  $psi.UseShellExecute = $false
  $p = [System.Diagnostics.Process]::Start($psi)
  if (-not $p.WaitForExit($TimeoutSec * 1000)) {
    $p.Kill($true)
    throw "child script timed out after ${TimeoutSec}s: $File"
  }
  [pscustomobject]@{
    ExitCode = $p.ExitCode
    StdOut   = $p.StandardOutput.ReadToEnd()
    StdErr   = $p.StandardError.ReadToEnd()
  }
}

$name = "gh822-test-$PID-$(Get-Date -Format 'HHmmss')"
$logDir = Join-Path $env:TEMP "gh822-lane-test-$PID"
$cwd = Join-Path $env:TEMP "gh822-lane-test-cwd-$PID"
$realLog = Join-Path $logDir "$name.log"
$realDone = Join-Path $logDir "$name.done"

New-Item -ItemType Directory -Force -Path $logDir, $cwd | Out-Null
& git -C $cwd init -q 2>$null
if ($LASTEXITCODE -ne 0) { throw "git init failed in $cwd" }

# Cleanup goes through lane-stop.ps1 (P1-2): Stop-ScheduledTask /
# Unregister-ScheduledTask terminate only the wrapper and can orphan the
# agent's child process tree, while lane-stop.ps1 kills the whole tree. It
# runs in a child pwsh (Invoke-ChildScript) because its Fail calls exit,
# which must not take this test process down.
$cleanup = {
  if (Get-ScheduledTask -TaskName "edda-lane-$name" -ErrorAction SilentlyContinue) {
    $stopped = Invoke-ChildScript $stop @("-Name", $name, "-LogDir", "`"$logDir`"")
    if ($stopped.ExitCode -ne 0) {
      "--- cleanup lane-stop stdout ---"; $stopped.StdOut
      "--- cleanup lane-stop stderr ---"; $stopped.StdErr
    }
    Unregister-ScheduledTask -TaskName "edda-lane-$name" -Confirm:$false -ErrorAction SilentlyContinue
  }
}
& $cleanup
try {
  # --- leg 1: dry-run on a clean LogDir (AC2, AC3, AC4) ----------------------

  "=== leg 1: dry-run on a clean LogDir ==="
  $dry = Invoke-ChildScript $launch @("-Name", $name, "-Cwd", "`"$cwd`"", "-LogDir", "`"$logDir`"", "-DryRun")
  if ($dry.ExitCode -ne 0) {
    "--- dry-run stdout ---"; $dry.StdOut; "--- dry-run stderr ---"; $dry.StdErr
    throw "lane-launch -DryRun exited $($dry.ExitCode)"
  }

  $leftovers = @(Get-ChildItem -LiteralPath $logDir -File |
    Where-Object { $_.Name -notmatch ('^' + [regex]::Escape($name) + '\.dryrun') })
  Assert-True ($leftovers.Count -eq 0) "AC2: dry-run creates only $name.dryrun* files (violators: $($leftovers.Name -join ', '))"
  Assert-True (-not (Test-Path -LiteralPath $realLog)) "AC2: dry-run does not create $name.log"
  Assert-True (-not (Test-Path -LiteralPath $realDone)) "AC2: dry-run does not create $name.done"

  $printedLog = [regex]::Match($dry.StdOut, '(?m)^dry-run log=(.+)\r?$').Groups[1].Value.Trim()
  Assert-True ($printedLog -ne '') "AC3: dry-run summary prints 'dry-run log=<path>'"
  Assert-True ($printedLog -eq (Join-Path $logDir "$name.dryrun.log")) "AC3: printed dry-run log path is $name.dryrun.log (got: $printedLog)"
  Assert-True ($printedLog -ne $realLog) "AC3: printed dry-run log path differs from the real log path"

  $wrapperPath = Join-Path $logDir "$name.dryrun-wrapper.ps1"
  $wrapperText = Get-Content -LiteralPath $wrapperPath -Raw
  Assert-True ($wrapperText.Contains((Join-Path $logDir "$name.dryrun.log"))) "AC4: dry-run wrapper embeds the $name.dryrun.log path"
  Assert-True ($wrapperText.Contains((Join-Path $logDir "$name.dryrun.done"))) "AC4: dry-run wrapper embeds the $name.dryrun.done path"
  Assert-True (-not $wrapperText.Contains($realLog)) "AC4: dry-run wrapper never references the real log path"
  Assert-True (-not $wrapperText.Contains($realDone)) "AC4: dry-run wrapper never references the real done path"

  # --- leg 2: real launch after the dry run (AC1, AC5) ------------------------

  if (-not $SkipRealLaunch) {
    "=== leg 2: real launch after the dry run ==="
    $brief = Join-Path $logDir "$name.dryrun-brief.md"  # dry-run's own brief, reused as the real brief
    Set-Content -LiteralPath $brief -Encoding utf8 -Value @(
      '# regression-test brief (GH-822)'
      'Trivial turn: reply with the single word ok and do nothing else.'
    )
    $real = Invoke-ChildScript $launch @("-Name", $name, "-Brief", "`"$brief`"", "-Cwd", "`"$cwd`"", "-LogDir", "`"$logDir`"", "-TimeoutSec", "90")
    if ($real.ExitCode -ne 0) {
      "--- real-launch stdout ---"; $real.StdOut; "--- real-launch stderr ---"; $real.StdErr
      throw "lane-launch (real) exited $($real.ExitCode)"
    }

    # Sampled while the lane is live: the row exists exactly because the
    # task exists, and the only name it can print is the real lane's task.
    $st = Invoke-ChildScript $status @("-Name", $name, "-LogDir", "`"$logDir`"")
    Assert-True ($st.ExitCode -eq 0) "AC5: lane-status reports the live real-launch lane"
    Assert-True ($st.StdOut -match "^edda-lane-$name ") "AC5: the reported row is the real lane's task, not a dry-run artifact"

    # P0-1: let the real wrapper finish NATURALLY — no lane-stop — so the
    # end record proven below is the real pipeline's own, never lane-stop's
    # "stopped" sentinel. The wrapper writes the done-file in its finally
    # block together with "=== EXIT code=<n> ===", so the done-file's
    # appearance marks natural completion.
    $deadline = (Get-Date).AddSeconds(45)
    $doneValue = $null
    while ((Get-Date) -lt $deadline) {
      Start-Sleep -Seconds 2
      if (Test-Path -LiteralPath $realDone) {
        $doneValue = (Get-Content -LiteralPath $realDone -Raw).Trim()
        break
      }
    }
    Assert-True ($null -ne $doneValue) "AC1: real wrapper finished naturally within 45s and wrote $name.done"

    Assert-True (Test-Path -LiteralPath $realLog) "AC1: real launch creates $name.log"
    $exitLines = @(Select-String -LiteralPath $realLog -Pattern '=== EXIT')
    Assert-True ($exitLines.Count -eq 1) "AC1: $name.log contains exactly one '=== EXIT' line after dry-run + real launch (found: $($exitLines.Count))"
    $exitMatch = if ($exitLines.Count -gt 0) { [regex]::Match($exitLines[0].Line, '^=== EXIT code=(\d+) ===$') } else { $null }
    Assert-True ($null -ne $exitMatch -and $exitMatch.Success) "AC1: the EXIT line is the real wrapper's own record '=== EXIT code=<n> ===' (got: '$($exitLines[0].Line)')"
    Assert-True ($null -ne $exitMatch -and $exitMatch.Success -and $exitMatch.Groups[1].Value -eq $doneValue) "AC1: done-file holds the EXIT line's exact numeric code (exit line: '$($exitMatch.Groups[1].Value)'; done file: '$doneValue')"

    if ($null -ne $doneValue) {
      # The wrapper finished on its own — unregister the completed task. If
      # it did not, leave the task for $cleanup (lane-stop) to take down
      # with its whole process tree.
      Unregister-ScheduledTask -TaskName "edda-lane-$name" -Confirm:$false -ErrorAction SilentlyContinue
    }
  }

  # --- leg 3: the guard rail reserves the dryrun namespace (P1-1) -------------

  "=== leg 3: -Name containing 'dryrun' is rejected ==="
  $collision = Invoke-ChildScript $launch @("-Name", "$name.dryrun", "-Cwd", "`"$cwd`"", "-LogDir", "`"$logDir`"")
  Assert-True ($collision.ExitCode -ne 0) "guard rail: lane-launch rejects -Name '$name.dryrun' (GH-822 P1-1)"
  Assert-True ($collision.StdErr -match "may not contain 'dryrun'") "guard rail: the rejection names the dryrun reservation (stderr: $($collision.StdErr.Trim()))"

  # The reservation must hold regardless of the delimiter that glues the
  # dryrun segment onto the lane name — dot, underscore, and hyphen all end
  # in a distinct $Name.dryrun.log/$Name.dryrun.done-style artifact family,
  # so each must be rejected too.
  $collisionUnder = Invoke-ChildScript $launch @("-Name", "${name}_dryrun", "-Cwd", "`"$cwd`"", "-LogDir", "`"$logDir`"")
  Assert-True ($collisionUnder.ExitCode -ne 0) "guard rail: lane-launch rejects underscore delimiter -Name '${name}_dryrun' (GH-822 P1-1)"

  $collisionHyphen = Invoke-ChildScript $launch @("-Name", "$name-dryrun", "-Cwd", "`"$cwd`"", "-LogDir", "`"$logDir`"")
  Assert-True ($collisionHyphen.ExitCode -ne 0) "guard rail: lane-launch rejects hyphen delimiter -Name '$name-dryrun' (GH-822 P1-1)"

  # --- leg 4: lane-status after unregister (AC5) ------------------------------

  "=== leg 4: lane-status after the task is unregistered ==="
  & $cleanup
  $st2 = Invoke-ChildScript $status @("-Name", $name, "-LogDir", "`"$logDir`"")
  Assert-True ($st2.ExitCode -ne 0) "AC5: lane-status reports no lane once the task is gone (dry-run artifacts alone are not status)"
}
finally {
  & $cleanup
  if ($script:failures.Count -gt 0) {
    "kept for inspection: $logDir"
  } else {
    Remove-Item -LiteralPath $logDir -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $cwd -Recurse -Force -ErrorAction SilentlyContinue
  }
}

""
if ($script:failures.Count -gt 0) {
  "RESULT: FAIL ($($script:failures.Count) assertion(s)):"
  $script:failures | ForEach-Object { "  - $_" }
  exit 1
}
"RESULT: PASS"
exit 0
