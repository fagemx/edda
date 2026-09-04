# lane-reap.ps1 — reap finished fleet scheduled-task registrations (GH-772).
#
# The fleet accumulates scheduled-task registrations that outlived their work:
# a worker lane whose issue was closed, a review lane whose PR was merged, a
# task whose controller session is provably gone. Each registration is small,
# but the pile hides the live lanes (lane-status lists them all) and can
# re-fire stale work. This script reports and, with explicit -Apply, removes
# ONLY the registration — never the wrapper file, logs, done-files, worktrees
# or sources, never a running process.
#
# Known fleet task families (everything else is invisible to this script by
# design — it cannot unregister a task it never enumerates):
#   edda-b-*            worker lanes (e.g. edda-b-lane-gh648)
#   edda-lane-*         worker lanes
#   edda-review-pr*     PR review rounds (e.g. edda-review-pr123-r1)
#   edda-dispatch-*     dispatch lanes
# edda-pr-review-watcher is persistent infrastructure and is NEVER a candidate,
# even under explicit -TaskName.
#
# A candidate is removed when any of these holds (one reason row per rule):
#   issue-closed      the associated GitHub issue is CLOSED
#   pr-merged         the associated GitHub PR is MERGED
#   controller-gone   the controller is PROVABLY gone: the recorded PID has no
#                     process, or a live process owns the PID but was created
#                     at a different time (PID reuse). Both the PID and the
#                     creation time must be recorded to prove anything. The
#                     controller is the scheduled task's own wrapper process,
#                     which records `# lane-reap: controller-pid=...
#                     controller-started=...` for itself when it starts; its
#                     death means the completion unregister can never run, so
#                     the registration is stale. Removal is STILL unregister-
#                     only: a detached worker child that outlived its wrapper
#                     keeps running to its own completion and is never
#                     killed — controller death justifies dropping the stale
#                     REGISTRATION, never touching a live worker, and a
#                     worker whose recorded controller is alive (or whose
#                     identity is partial) is retained.
# Association evidence, strongest first:
#   1. `# lane-reap:` metadata in the task action's wrapper file:
#        # lane-reap: issue=772 pr=850 controller-pid=4242 controller-started=2026-02-14T10:00:00.0000000+08:00
#      (record controller-started with Get-Date -Format o; UTC 'Z' values are
#      also understood — instants are compared, not wall clocks. Malformed or
#      unreadable metadata fails visibly, retains the task, and costs the run
#      its exit 0.) A task whose registered state is Running is never removed
#      regardless of its evidence — the registration of a live worker is not
#      stale by definition.
#   2. The task name itself: ghNNN -> issue, prNNN -> PR (the fleet's own
#      naming convention).
#   3. ghNNN / prNNN tokens in the wrapper text — but only when unambiguous;
#      several distinct numbers are reported and ignored rather than guessed.
# A task with no wrapper and no name evidence is reported retained/unknown —
# a legacy registration is never assumed stale. GitHub read failures are
# unknown too: the task is preserved and the failure is reported (nonzero exit).
#
# Safety properties:
#   - Default is a dry-run report; nothing is changed without explicit -Apply.
#   - Removal is Unregister-ScheduledTask ONLY. The script never calls
#     Stop-ScheduledTask, never kills a process, never deletes a file or tree.
#     A task reporting State = Running is never a removal candidate at all:
#     unregistering under a live worker could stop it, which this script must
#     never do.
#   - Wrapper, logs, done-files, worktrees and sources are never touched.
#
# usage:
#   pwsh -NoProfile -File scripts/fleet/lane-reap.ps1                        # dry-run report, all families
#   pwsh -NoProfile -File scripts/fleet/lane-reap.ps1 -Apply                 # unregister what the report proved dead
#   pwsh -NoProfile -File scripts/fleet/lane-reap.ps1 -TaskName 'edda-b-*'   # narrow candidates (-like pattern)
#   pwsh -NoProfile -File scripts/fleet/lane-reap.ps1 -LogDir "$env:TEMP\edda-lanes"
#                                                                            # search wrapper files here when the
#                                                                            # task action names none
#   pwsh -NoProfile -File scripts/fleet/lane-reap.ps1 -Repo fagemx/edda      # pin the gh repository
#
# Output: structured key=value rows on stdout, one per candidate reason /
# action / error, plus a final summary row.
# Exit codes: 0 = report complete, nothing failed; 1 = at least one action,
# GitHub read, or metadata read/parse failure — the report is then incomplete
# and every affected task was preserved (fail closed).
#
# Testability: Get-FleetScheduledTasks, Get-FleetTaskByName, Remove-FleetTaskRegistration,
# Invoke-FleetGh and Test-FleetControllerAlive are the seams into the Windows
# task service, gh and the process table. test-lane-reap.ps1 dot-sources this
# file in a child pwsh ($env:LANE_REAP_LIBRARY='1' suppresses the main run)
# and overrides those seams with injected function mocks — no real scheduled
# task or process is ever touched by a test. Read-FleetTextFile is left real
# so the metadata path validation is exercised against real files.
#
# Limitations (candid):
#   - Wrapper controller-pid/controller-started metadata is recorded by new
#     lane wrappers with the GH-772 wrapper change; until that lands (it is
#     being integrated with #702), current tasks carry only issue/PR evidence
#     from task names and wrapper text, so controller-pid /
#     controller-started rows appear only for freshly launched lanes.
#   - gh resolves the repository from its own auth/default unless -Repo is
#     given; run from the repo or pass -Repo, or every read errors and the
#     run fails closed, preserving everything.
#   - Creation-time comparison allows 2s tolerance for serialization loss;
#     offset-bearing and UTC timestamps are compared as instants, but a
#     controller-started recorded WITHOUT any offset is assumed to be local
#     wall time.
#   - Removal decisions are only as good as the associations: a wrong ghNNN
#     baked into a task name would reap it when that issue closes. Names are
#     the fleet's own convention, and every removal row names its evidence so
#     the dry-run output can be reviewed before -Apply.

param(
  [switch]$Apply,
  [string]$TaskName = '',
  [string]$LogDir = '',
  [string]$Repo = ''
)

$ErrorActionPreference = 'Stop'

$script:FleetTaskFamilies = @('edda-b-*', 'edda-lane-*', 'edda-review-pr*', 'edda-dispatch-*')
$script:PersistentTaskNames = @('edda-pr-review-watcher')
$script:ControllerTimeToleranceSec = 2

function Fail([string]$Msg) {
  [Console]::Error.WriteLine("lane-reap: $Msg")
  exit 1
}

# --- seams (overridable by the test harness; see header) ---------------------

function Get-FleetScheduledTasks {
  # All tasks in the known families, deduplicated, in family order.
  $seen = @{}
  $out = @()
  foreach ($fam in $script:FleetTaskFamilies) {
    foreach ($t in @(Get-ScheduledTask -TaskName $fam -ErrorAction SilentlyContinue)) {
      if (-not $seen.ContainsKey($t.TaskName)) {
        $seen[$t.TaskName] = $true
        $out += $t
      }
    }
  }
  return ,$out
}

function Get-FleetTaskByName([string]$TaskName) {
  # Re-read immediately before unregistering.  The first enumeration is only
  # evidence for the report; a scheduler action can replace or start that
  # registration before -Apply reaches it.
  return @(Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue | Select-Object -First 1)
}

function Get-FleetTaskIdentity($Task) {
  # MSFT_ScheduledTask has no Xml property.  Export-ScheduledTask is the real
  # Scheduler API for the registration document; hash it at each observation.
  if (-not $Task) { return $null }
  try { $xml = [string](Export-ScheduledTask -TaskName $Task.TaskName -TaskPath $Task.TaskPath -ErrorAction Stop) } catch { return $null }
  if (-not $xml) { return $null }
  $bytes = [Text.Encoding]::UTF8.GetBytes($xml)
  return [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($bytes))
}

function Remove-FleetTaskRegistration([string]$TaskName) {
  # The ONLY mutation this script ever performs: drop the registration.
  # The task's wrapper, logs and any running worker are untouched.
  Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction Stop
}

function Invoke-FleetGh([string]$Kind, [int]$Number, [string]$Repo) {
  # Read-only gh query. Returns @{ Ok = $true; State = <string> } or
  # @{ Ok = $false; Error = <string> } — never throws, so a read failure can
  # be reported per candidate and treated as "unknown" (preserve the task).
  $repoArgs = @()
  if ($Repo) { $repoArgs = @('--repo', $Repo) }
  $json = $null
  try {
    if ($Kind -eq 'issue') {
      $json = & gh issue view $Number @repoArgs --json state 2>$null
    } else {
      $json = & gh pr view $Number @repoArgs --json state 2>$null
    }
  } catch {
    return @{ Ok = $false; Error = $_.Exception.Message }
  }
  if ($LASTEXITCODE -ne 0 -or -not $json) {
    return @{ Ok = $false; Error = "gh $Kind view $Number failed (exit $LASTEXITCODE, no output)" }
  }
  try {
    $parsed = $json | ConvertFrom-Json
  } catch {
    return @{ Ok = $false; Error = "gh $Kind view $Number returned unparseable JSON" }
  }
  return @{ Ok = $true; State = [string]$parsed.state }
}

function Test-FleetControllerAlive([int]$ControllerPid, [datetime]$StartedAt) {
  # Returns @{ Found = <bool>; CreationTime = <datetime | $null> }. Whether
  # the found creation time matches $StartedAt is decided by the caller, so
  # the PID-reuse test can be mocked without a real process table.
  $proc = Get-CimInstance Win32_Process -Filter "ProcessId=$ControllerPid" -ErrorAction SilentlyContinue
  if (-not $proc) { return @{ Found = $false; CreationTime = $null } }
  return @{ Found = $true; CreationTime = [datetime]$proc.CreationDate }
}

function Read-FleetTextFile([string]$Path) {
  Get-Content -LiteralPath $Path -Raw -ErrorAction Stop
}

# --- metadata parsing --------------------------------------------------------

function Read-FleetWrapperMetadata([string]$Text) {
  # Parses `# lane-reap:` marker lines. Anything on such a line that is not a
  # known key=value pair is malformed (visible failure, never a guess).
  # Returns @{ Issue; Pr; ControllerPid; ControllerStarted; Malformed }.
  $res = @{ Issue = $null; Pr = $null; ControllerPid = $null; ControllerStarted = $null; Malformed = @() }
  $vals = @{}
  foreach ($m in [regex]::Matches($Text, '(?m)^\s*#\s*lane-reap:\s*(.+?)\s*$')) {
    foreach ($token in ($m.Groups[1].Value -split '\s+')) {
      if (-not $token) { continue }
      if ($token -notmatch '^(?i)(issue|pr|controller-pid|controller-started)=(.+)$') {
        $res.Malformed += "unrecognized token '$token'"
        continue
      }
      $key = $Matches[1].ToLowerInvariant()
      $val = $Matches[2]
      if ($vals.ContainsKey($key)) {
        if ("$($vals[$key])" -ne $val) {
          $res.Malformed += "conflicting values for $key ('$($vals[$key])' vs '$val')"
        }
        continue
      }
      switch ($key) {
        'issue' {
          $n = 0
          if ($val -match '^\d+$' -and [int]::TryParse($val, [ref]$n)) { $vals[$key] = $n }
          else { $res.Malformed += "issue value '$val' is not a number" }
        }
        'pr' {
          $n = 0
          if ($val -match '^\d+$' -and [int]::TryParse($val, [ref]$n)) { $vals[$key] = $n }
          else { $res.Malformed += "pr value '$val' is not a number" }
        }
        'controller-pid' {
          $n = 0
          if ($val -match '^\d+$' -and [int]::TryParse($val, [ref]$n)) { $vals[$key] = $n }
          else { $res.Malformed += "controller-pid value '$val' is not a number" }
        }
        'controller-started' {
          try {
            $vals[$key] = [datetime]::Parse($val, [System.Globalization.CultureInfo]::InvariantCulture,
              [System.Globalization.DateTimeStyles]::RoundtripKind)
          } catch {
            $res.Malformed += "controller-started value '$val' is not a parseable timestamp"
          }
        }
      }
    }
  }
  if ($vals.ContainsKey('issue')) { $res.Issue = $vals['issue'] }
  if ($vals.ContainsKey('pr')) { $res.Pr = $vals['pr'] }
  if ($vals.ContainsKey('controller-pid')) { $res.ControllerPid = $vals['controller-pid'] }
  if ($vals.ContainsKey('controller-started')) { $res.ControllerStarted = $vals['controller-started'] }
  return $res
}

function Get-FleetNameAssociation([string]$TaskName) {
  # ghNNN -> issue, prNNN -> PR. The leading letter of the token must not be
  # part of a longer word (weight2, preview123 do not match).
  $issue = $null; $pr = $null
  $mi = [regex]::Match($TaskName, '(?i)gh(\d+)')
  if ($mi.Success) { $issue = [int]$mi.Groups[1].Value }
  $mp = [regex]::Match($TaskName, '(?i)(?<![A-Za-z])pr(\d+)(?!\d)')
  if ($mp.Success) { $pr = [int]$mp.Groups[1].Value }
  return @{ Issue = $issue; Pr = $pr }
}

function Get-FleetLogDirWrapperCandidates([string]$TaskName, [string]$LogDir) {
  # lane-stop.ps1-style fallback: when the task action names no wrapper file,
  # look for it beside the logs under the task's own name variants.
  $base = $TaskName -replace '^edda-b-', '' -replace '^edda-lane-', '' -replace '^edda-review-', '' -replace '^edda-dispatch-', ''
  $noLane = $base -replace '-lane$', ''
  $names = @(
    "$base.wrapper.ps1", "$base.ps1", "$noLane.wrapper.ps1", "$noLane.ps1",
    "$noLane-lane.wrapper.ps1", "$noLane-lane.ps1",
    "$TaskName.wrapper.ps1", "$TaskName.ps1"
  )
  foreach ($n in $names) {
    $p = Join-Path $LogDir $n
    if (Test-Path -LiteralPath $p -PathType Leaf) { return $p }
  }
  return $null
}

function Get-FleetTaskAssociation($Task, [string]$LogDir) {
  # Gathers every scrap of association evidence for one task. Read or parse
  # failures are collected in .Errors (fail visibly, retain the task);
  # absence of evidence is collected in .Notes (retained/unknown, not an error).
  $res = @{ WrapperPath = $null; Issue = $null; Pr = $null; ControllerPid = $null; ControllerStarted = $null; Notes = @(); Errors = @() }
  $text = $null

  $path = $null
  $actionArg = if ($Task.Actions -and $Task.Actions.Count -gt 0) { [string]$Task.Actions[0].Arguments } else { $null }
  if ($actionArg -and $actionArg -match '-File\s+("([^"]+)"|''([^'']+)''|([^\s]+))') {
    $path = if ($Matches[2]) { $Matches[2] } elseif ($Matches[3]) { $Matches[3] } else { $Matches[4] }
  }
  if (-not $path -and $LogDir) {
    $path = Get-FleetLogDirWrapperCandidates -TaskName $Task.TaskName -LogDir $LogDir
    if ($path) { $res.Notes += "wrapper resolved from -LogDir: $path" }
  }
  if ($path) {
    if ([string]::IsNullOrWhiteSpace($path)) {
      $res.Errors += 'task action names an empty wrapper path'
    } elseif (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
      $res.Errors += "metadata path does not exist: $path"
    } else {
      try {
        $text = Read-FleetTextFile -Path $path
        $res.WrapperPath = $path
      } catch {
        $res.Errors += "metadata path unreadable: $path ($($_.Exception.Message))"
      }
    }
  } else {
    $res.Notes += 'no wrapper file resolvable from the task action'
  }

  if ($null -ne $text) {
    $meta = Read-FleetWrapperMetadata -Text $text
    foreach ($m in $meta.Malformed) { $res.Errors += "malformed lane-reap metadata: $m" }
    if ($null -ne $meta.Issue) { $res.Issue = $meta.Issue }
    if ($null -ne $meta.Pr) { $res.Pr = $meta.Pr }
    if ($null -ne $meta.ControllerPid) { $res.ControllerPid = $meta.ControllerPid }
    if ($null -ne $meta.ControllerStarted) { $res.ControllerStarted = $meta.ControllerStarted }
  }

  $nameAssoc = Get-FleetNameAssociation -TaskName $Task.TaskName
  if ($null -eq $res.Issue -and $null -ne $nameAssoc.Issue) {
    $res.Issue = $nameAssoc.Issue
    $res.Notes += "issue from task name (gh$($nameAssoc.Issue))"
  }
  if ($null -eq $res.Pr -and $null -ne $nameAssoc.Pr) {
    $res.Pr = $nameAssoc.Pr
    $res.Notes += "pr from task name (pr$($nameAssoc.Pr))"
  }

  if ($null -ne $text) {
    if ($null -eq $res.Issue) {
      $hits = @([regex]::Matches($text, '(?i)gh(\d+)') | ForEach-Object { [int]$_.Groups[1].Value } | Select-Object -Unique)
      if ($hits.Count -eq 1) { $res.Issue = $hits[0]; $res.Notes += "issue from wrapper text (gh$($hits[0]))" }
      elseif ($hits.Count -gt 1) { $res.Notes += "multiple ghNNN tokens in wrapper text ($($hits -join ',')); ambiguous, association ignored" }
    }
    if ($null -eq $res.Pr) {
      $hits = @([regex]::Matches($text, '(?i)(?<![A-Za-z])pr(\d+)(?!\d)') | ForEach-Object { [int]$_.Groups[1].Value } | Select-Object -Unique)
      if ($hits.Count -eq 1) { $res.Pr = $hits[0]; $res.Notes += "pr from wrapper text (pr$($hits[0]))" }
      elseif ($hits.Count -gt 1) { $res.Notes += "multiple prNNN tokens in wrapper text ($($hits -join ',')); ambiguous, association ignored" }
    }
  }
  return $res
}

# --- decision ----------------------------------------------------------------

function New-LaneReapRow([hashtable]$Fields) {
  $o = [ordered]@{}
  foreach ($k in @('row', 'task', 'rule', 'decision', 'issue', 'pr', 'controller', 'verb', 'mode', 'result', 'what', 'detail')) {
    if ($Fields.ContainsKey($k)) { $o[$k] = $Fields[$k] }
  }
  foreach ($k in $Fields.Keys) { if (-not $o.Contains($k)) { $o[$k] = $Fields[$k] } }
  return [pscustomobject]$o
}

function Format-LaneReapRow($Row) {
  $parts = foreach ($p in $Row.PSObject.Properties) {
    if ($null -ne $p.Value -and "$($p.Value)" -ne '') { "$($p.Name)=$($p.Value)" }
  }
  return ($parts -join ' ')
}

function Invoke-LaneReap {
  # Returns @{ Rows; Applied; CandidateCount; RemoveDecisions; ErrorCount; ExitCode }.
  # Rows are also the reviewable artifact of a dry run: every removal the
  # script would make names its evidence before -Apply is ever given.
  param(
    [switch]$Apply,
    [string]$TaskName = '',
    [string]$LogDir = '',
    [string]$Repo = ''
  )

  $rows = New-Object System.Collections.Generic.List[object]
  $applied = New-Object System.Collections.Generic.List[string]
  $errorCount = 0
  $removeDecisions = 0

  # Candidates only: known families, never the persistent watcher, never an
  # off-family task — the narrowing pattern cannot widen this set.
  $candidates = @()
  foreach ($t in @(Get-FleetScheduledTasks)) {
    $tn = [string]$t.TaskName
    if ($script:PersistentTaskNames -contains $tn) { continue }
    $inFamily = $false
    foreach ($fam in $script:FleetTaskFamilies) { if ($tn -like $fam) { $inFamily = $true; break } }
    if (-not $inFamily) { continue }
    if ($TaskName -and -not ($tn -like $TaskName)) { continue }
    $candidates += $t
  }

  foreach ($t in $candidates) {
    $tn = [string]$t.TaskName
    # Capture identity before any gh/process work.  Those reads are the race
    # window in which a same-name registration can be replaced.
    $initialIdentity = Get-FleetTaskIdentity $t
    $assoc = Get-FleetTaskAssociation -Task $t -LogDir $LogDir

    foreach ($err in $assoc.Errors) {
      $rows.Add((New-LaneReapRow @{ row = 'error'; task = $tn; what = 'metadata'; detail = $err }))
      $errorCount++
    }

    # Metadata failures mean the evidence is not trustworthy: report and
    # retain, never decide on partial or malformed input.
    if ($assoc.Errors.Count -gt 0) {
      $rows.Add((New-LaneReapRow @{ row = 'reason'; task = $tn; rule = 'metadata-unreadable'; decision = 'retain'; issue = $assoc.Issue; pr = $assoc.Pr }))
      continue
    }

    # A Running worker's registration is not stale by definition, whatever
    # its issue/PR state says: unregistering under it could stop the worker,
    # and this script never stops anything. Revisit when the worker exits.
    if ([string]$t.State -eq 'Running') {
      $rows.Add((New-LaneReapRow @{ row = 'reason'; task = $tn; rule = 'task-running'; decision = 'retain'; issue = $assoc.Issue; pr = $assoc.Pr; detail = 'Running workers are never removal candidates (GH-772)' }))
      continue
    }

    $removeRules = New-Object System.Collections.Generic.List[string]
    $retainRules = New-Object System.Collections.Generic.List[string]
    $ctrl = $null

    if ($null -ne $assoc.Issue) {
      $r = Invoke-FleetGh -Kind 'issue' -Number $assoc.Issue -Repo $Repo
      if ($r.Ok) {
        if ($r.State -eq 'CLOSED') { $removeRules.Add('issue-closed') } else { $retainRules.Add('issue-open') }
      } else {
        # A gh read error is "unknown", never "stale": preserve the task.
        $rows.Add((New-LaneReapRow @{ row = 'error'; task = $tn; what = 'gh-issue'; detail = "issue $($assoc.Issue): $($r.Error)" }))
        $errorCount++
        $retainRules.Add('issue-state-unknown')
      }
    }

    if ($null -ne $assoc.Pr) {
      $r = Invoke-FleetGh -Kind 'pr' -Number $assoc.Pr -Repo $Repo
      if ($r.Ok) {
        if ($r.State -eq 'MERGED') { $removeRules.Add('pr-merged') } else { $retainRules.Add('pr-not-merged') }
      } else {
        $rows.Add((New-LaneReapRow @{ row = 'error'; task = $tn; what = 'gh-pr'; detail = "pr $($assoc.Pr): $($r.Error)" }))
        $errorCount++
        $retainRules.Add('pr-state-unknown')
      }
    }

    if ($null -ne $assoc.ControllerPid -and $null -ne $assoc.ControllerStarted) {
      $alive = Test-FleetControllerAlive -ControllerPid $assoc.ControllerPid -StartedAt $assoc.ControllerStarted
      if (-not $alive.Found) {
        $ctrl = "pid=$($assoc.ControllerPid) gone"
        $removeRules.Add('controller-gone')
      } elseif ([math]::Abs(([DateTimeOffset]$alive.CreationTime - [DateTimeOffset]$assoc.ControllerStarted).TotalSeconds) -le $script:ControllerTimeToleranceSec) {
        $ctrl = "pid=$($assoc.ControllerPid) alive"
        $retainRules.Add('controller-alive')
      } else {
        $ctrl = "pid=$($assoc.ControllerPid) reused (recorded $($assoc.ControllerStarted.ToString('o')), live $($alive.CreationTime.ToString('o')))"
        $removeRules.Add('controller-gone')
      }
    } elseif ($assoc.ControllerPid -or $assoc.ControllerStarted) {
      # Half a controller identity proves nothing in either direction.
      $ctrl = 'partial (pid or start time missing)'
      $retainRules.Add('controller-unprovable')
    }

    if ($removeRules.Count -gt 0) {
      foreach ($rule in $removeRules) {
        $rows.Add((New-LaneReapRow @{ row = 'reason'; task = $tn; rule = $rule; decision = 'remove'; issue = $assoc.Issue; pr = $assoc.Pr; controller = $ctrl }))
      }
      $removeDecisions++
      if ($Apply) {
        # Re-check both state and registration identity at the point of the
        # mutation.  A Ready stale task can be replaced by a new Running
        # worker between the report read and this call; never unregister it.
        $current = Get-FleetTaskByName -TaskName $tn
        if (-not $current) {
          $rows.Add((New-LaneReapRow @{ row = 'action'; task = $tn; verb = 'unregister'; mode = 'apply'; result = 'skipped-race'; detail = 'registration absent at apply time' }))
          continue
        }
        if ([string]$current.State -eq 'Running') {
          $rows.Add((New-LaneReapRow @{ row = 'action'; task = $tn; verb = 'unregister'; mode = 'apply'; result = 'skipped-race'; detail = 'registration became Running at apply time' }))
          continue
        }
        $currentIdentity = Get-FleetTaskIdentity $current
        if (-not $initialIdentity -or -not $currentIdentity) {
          $rows.Add((New-LaneReapRow @{ row = 'action'; task = $tn; verb = 'unregister'; mode = 'apply'; result = 'skipped-race'; detail = 'registration identity unavailable at apply time' }))
          continue
        }
        if ($currentIdentity -ne $initialIdentity) {
          $rows.Add((New-LaneReapRow @{ row = 'action'; task = $tn; verb = 'unregister'; mode = 'apply'; result = 'skipped-race'; detail = 'registration identity changed at apply time' }))
          continue
        }
        try {
          Remove-FleetTaskRegistration -TaskName $tn
          $applied.Add($tn)
          $rows.Add((New-LaneReapRow @{ row = 'action'; task = $tn; verb = 'unregister'; mode = 'apply'; result = 'ok' }))
        } catch {
          $rows.Add((New-LaneReapRow @{ row = 'error'; task = $tn; what = 'unregister'; detail = $_.Exception.Message }))
          $rows.Add((New-LaneReapRow @{ row = 'action'; task = $tn; verb = 'unregister'; mode = 'apply'; result = 'failed' }))
          $errorCount++
        }
      } else {
        $rows.Add((New-LaneReapRow @{ row = 'action'; task = $tn; verb = 'unregister'; mode = 'dry-run'; result = 'ok'; detail = 'not performed (-Apply not given)' }))
      }
    } else {
      $rule = if ($retainRules.Count -gt 0) { $retainRules -join '+' } else { 'no-association-evidence' }
      $detail = if ($assoc.Notes.Count -gt 0) { $assoc.Notes -join '; ' } else { $null }
      $rows.Add((New-LaneReapRow @{ row = 'reason'; task = $tn; rule = $rule; decision = 'retain'; issue = $assoc.Issue; pr = $assoc.Pr; controller = $ctrl; detail = $detail }))
    }
  }

  return @{
    Rows            = $rows
    Applied         = $applied
    CandidateCount  = $candidates.Count
    RemoveDecisions = $removeDecisions
    ErrorCount      = $errorCount
    ExitCode        = $(if ($errorCount -gt 0) { 1 } else { 0 })
  }
}

# --- main (suppressed in the test harness's child pwsh) ----------------------

if ($env:LANE_REAP_LIBRARY -ne '1') {
  $result = Invoke-LaneReap -Apply:$Apply -TaskName $TaskName -LogDir $LogDir -Repo $Repo
  foreach ($r in $result.Rows) { Format-LaneReapRow $r }
  $mode = if ($Apply) { 'apply' } else { 'dry-run' }
  "row=summary mode=$mode candidates=$($result.CandidateCount) removes=$($result.RemoveDecisions) applied=$($result.Applied.Count) errors=$($result.ErrorCount)"
  exit $result.ExitCode
}
