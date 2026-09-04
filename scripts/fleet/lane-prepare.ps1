# lane-prepare.ps1 — prepare the lane-bound persistent worktree for one of the
# four ratified build lanes (GH-626).
#
# One lane owns ONE worktree at a FIXED path for its whole lifetime:
#
#   <main-repo-parent>\<repo-name>-wt-<build-lane>
#
# (main checkout C:\ai_agent\edda -> C:\ai_agent\edda-wt-worker-1). The path
# never changes per issue, so cargo fingerprints survive branch switches and
# only touched crates recompile (#613 measured: warm lane, branch switched on
# the same path, ~1s to first compile). Per issue this script switches that
# worktree to a NEW branch cut from origin/main, gated on:
#
#   * the lane's scheduled task is not Running on the worktree (busy gate),
#   * the worktree is clean (`git status --porcelain` empty; dirty gate),
#   * the CURRENT branch is pushed — verified by comparing the local tip to
#     refs/remotes/origin/<branch>; an upstream configuration alone is NOT
#     accepted (unpushed gate).
#
# The script NEVER deletes a branch, a worktree, or source files, and NEVER
# force-checks-out: every refusal is a loud exit 1 that leaves state untouched.
# Re-running with the same branch while the worktree sits on it (clean) is an
# idempotent no-op.
#
# Scope note: this script fixes the WORKTREE path only. CARGO_TARGET_DIR
# (<lane root>\<lane>; lane root = $env:LOCALAPPDATA\fleet-workstation\lanes,
# overridable by FLEET_LANE_ROOT) is set by the lane wrapper (lane-launch.ps1)
# and by lane-warm.ps1's Get-LaneBuildEnv. Both helpers derive the worktree
# path with the identical formula above; lane-warm.ps1 refuses a lane worktree
# it did not prepare, so the two cannot drift.
#
# usage:
#   pwsh -NoProfile -File scripts/fleet/lane-prepare.ps1 -BuildLane <lane> `
#        -Branch <new-branch> -Repo <main checkout> [-StartRef origin/main] [-DryRun]
#
# -DryRun runs every read-only gate, prints the exact commands a real run
# would execute, and mutates nothing (no fetch — fetch moves refs).
#
# Prints on success: lane=, worktree=, branch=, base= (pinned start SHA).
# Exit 0 = prepared (or already prepared); 1 = refused / error, nothing changed.
param(
  [Parameter(Mandatory = $true)][string]$BuildLane,
  [Parameter(Mandatory = $true)][string]$Branch,
  [Parameter(Mandatory = $true)][string]$Repo,
  [string]$StartRef = 'origin/main',
  [switch]$DryRun
)

$ErrorActionPreference = 'Stop'

function Fail([string]$Msg) {
  [Console]::Error.WriteLine("lane-prepare: $Msg")
  exit 1
}

# --- shared helpers (kept in sync with lane-warm.ps1) -----------------------

$allowedBuildLanes = @('worker-1', 'worker-2', 'verifier', 'verifier-2')
if ($BuildLane -and $allowedBuildLanes -notcontains $BuildLane) {
  Fail "-BuildLane '$BuildLane' is not an allowed build lane (verification.cost-discipline allows only: $($allowedBuildLanes -join ', '))"
}

# The FIXED lane worktree path (lane-launch.ps1 and lane-warm.ps1 derive the
# same path; do not change one without the other).
function Get-LaneWorktreePath([string]$MainRepo, [string]$Lane) {
  $parent = Split-Path -Parent $MainRepo
  $name = Split-Path -Leaf $MainRepo
  return Join-Path $parent ($name + '-wt-' + $Lane)
}

# Compare git/Task Scheduler paths robustly: git prints worktree paths with
# forward slashes, Join-Path produces backslashes; normalize both.
function ConvertTo-NormPath([string]$p) {
  return ($p -replace '/', '\').TrimEnd('\').ToLowerInvariant()
}

function Test-WtRegistered([string]$MainRepo, [string]$WtPath) {
  $want = ConvertTo-NormPath $WtPath
  foreach ($line in @(& git -C $MainRepo worktree list --porcelain)) {
    if ($line -like 'worktree *' -and ((ConvertTo-NormPath $line.Substring('worktree '.Length)) -eq $want)) {
      return $true
    }
  }
  return $false
}

# Busy gate: any scheduled task named edda-lane-* in State = Running whose
# action WorkingDirectory is the lane worktree (lane-launch.ps1 registers the
# task with that WorkingDirectory). A stopped, registered task is a finished
# lane, not a busy one. Windows-only by design — the fleet runs on Windows.
function Test-LaneTaskBusy([string]$WtPath) {
  $want = ConvertTo-NormPath $WtPath
  try {
    # An empty result proves no matching tasks; a failed query proves nothing.
    $tasks = @(Get-ScheduledTask -TaskName 'edda-lane-*' -ErrorAction Stop)
  } catch {
    Fail "cannot query Task Scheduler for lane activity: $($_.Exception.Message); refusing to switch a lane whose idle state cannot be proven"
  }
  foreach ($t in $tasks) {
    if ($t.State -ne 'Running') { continue }
    foreach ($a in $t.Actions) {
      $wd = if ($a.WorkingDirectory) { $a.WorkingDirectory.TrimEnd('\') } else { '' }
      if ($wd -and ((ConvertTo-NormPath $wd) -eq $want)) { return $true }
    }
  }
  return $false
}

function Get-WtBranch([string]$WtPath) {
  # Empty output = detached HEAD; its commit gets a separate durability gate.
  return (& git -C $WtPath symbolic-ref --quiet HEAD 2>$null)
}

# A detached worktree is safe to switch only when its current commit is still
# reachable from origin's remote-tracking refs. Cleanliness alone does not
# protect a unique detached commit from becoming reflog-only after checkout.
function Test-DetachedHeadDurable([string]$WtPath) {
  $head = (& git -C $WtPath rev-parse --verify 'HEAD^{commit}').Trim()
  if ($LASTEXITCODE -ne 0 -or -not $head) {
    Fail "cannot resolve detached HEAD in '$WtPath'; refusing to switch it"
  }
  $remoteRefs = @(& git -C $WtPath for-each-ref --contains $head --format='%(refname)' 'refs/remotes/origin/')
  return (@($remoteRefs | Where-Object { $_ }).Count -gt 0)
}

# The unpushed gate: the local tip must MATCH THE REMOTE TIP
# (refs/remotes/origin/<branch> exists and points at the same commit). A
# configured upstream alone proves nothing — branch.<name>.remote can point
# anywhere while local commits remain unpushed (measured trap, GH-626 tests).
function Test-BranchPushed([string]$WtPath, [string]$Branch) {
  $remoteRef = "refs/remotes/origin/$Branch"
  $remoteTip = & git -C $WtPath rev-parse --verify --quiet "$remoteRef^{commit}"
  if (-not $remoteTip) { return $false }
  $localTip = & git -C $WtPath rev-parse --verify "refs/heads/$Branch^{commit}"
  return ($remoteTip -eq $localTip)
}

# --- resolve the main repo (works from the main checkout or any worktree) ---

& git -C $Repo rev-parse --git-dir >$null 2>$null
if ($LASTEXITCODE -ne 0) { Fail "-Repo '$Repo' is not a git repository" }
$commonDir = (& git -C $Repo rev-parse --path-format=absolute --git-common-dir).Trim()
$MainRepo = Split-Path -Parent $commonDir
if (-not (Test-Path -LiteralPath $MainRepo)) { Fail "main repo '$MainRepo' (from $Repo) does not exist" }

# Branch-name validation: git's own ref rules (rejects ../, leading '-', ~^:,
# etc.); no whitespace, no deletion, no force anywhere in this script.
& git -C $MainRepo check-ref-format --branch $Branch >$null 2>$null
if ($LASTEXITCODE -ne 0) { Fail "-Branch '$Branch' is not a valid branch name (git check-ref-format)" }

$WtPath = Get-LaneWorktreePath $MainRepo $BuildLane

# --- 1. busy gate ------------------------------------------------------------

if (Test-LaneTaskBusy $WtPath) {
  Fail "lane '$BuildLane' is busy: a running edda-lane-* task is bound to '$WtPath'; stop it with scripts/fleet/lane-stop.ps1 first"
}

# --- 2. branch already exists ------------------------------------------------

$branchExists = (& git -C $MainRepo show-ref --verify --quiet "refs/heads/$Branch")
if ($LASTEXITCODE -eq 0) {
  # Only acceptable shape: the lane worktree itself is on that branch and
  # clean — a re-prepare of the same issue. Anything else is refused loudly:
  # branches are never reused, moved, or deleted here.
  if (-not (Test-Path -LiteralPath $WtPath -PathType Container)) {
    Fail "branch '$Branch' already exists but the lane worktree '$WtPath' is missing; refusing to reuse the branch — pick a new branch name (branches are never deleted here)"
  }
  if (-not (Test-WtRegistered $MainRepo $WtPath)) {
    Fail "'$WtPath' exists but is not a registered worktree of '$MainRepo'; refusing to touch it"
  }
  $dirty = (& git -C $WtPath status --porcelain)
  if ($dirty) { Fail "lane worktree '$WtPath' is dirty:`n$dirty"; }
  $cur = Get-WtBranch $WtPath
  if ($cur -ne "refs/heads/$Branch") {
    Fail "branch '$Branch' already exists and is checked out elsewhere; refusing (branches are never deleted or force-moved)"
  }
  $msg = "already prepared: lane=$BuildLane worktree=$WtPath branch=$Branch (clean, no-op)"
  if ($DryRun) { "dry-run: $msg" } else { $msg }
  exit 0
}

# --- 3. existing lane worktree: shape, clean, unpushed gates -----------------

if (Test-Path -LiteralPath $WtPath) {
  if (-not (Test-Path -LiteralPath $WtPath -PathType Container)) {
    Fail "'$WtPath' exists and is not a directory; refusing to overwrite"
  }
  if (-not (Test-WtRegistered $MainRepo $WtPath)) {
    Fail "'$WtPath' exists but is not a registered worktree of '$MainRepo'; refusing to touch it (remove the collision by hand if it is yours)"
  }
  $dirty = (& git -C $WtPath status --porcelain)
  if ($dirty) { Fail "lane worktree '$WtPath' is dirty:`n$dirty" }

  $cur = Get-WtBranch $WtPath
  if ($cur) {
    $curBranch = $cur -replace '^refs/heads/', ''
    if (-not (Test-BranchPushed $WtPath $curBranch)) {
      $remoteState = if (& git -C $WtPath rev-parse --verify --quiet "refs/remotes/origin/$curBranch^{commit}") { 'differs from' } else { 'is missing from' }
      Fail "branch '$curBranch' ($(& git -C $WtPath rev-parse --short HEAD)) is not pushed: the remote tip $remoteState refs/remotes/origin/$curBranch. A configured upstream alone is not sufficient; push the branch before switching the lane worktree"
    }
  } elseif (-not (Test-DetachedHeadDurable $WtPath)) {
    Fail "detached HEAD $(& git -C $WtPath rev-parse --short HEAD) is not reachable from an origin remote-tracking ref; attach and push it before switching the lane worktree"
  }
}

# --- 4. dry-run: report the exact commands, change nothing -------------------

if ($DryRun) {
  $baseSha = & git -C $MainRepo rev-parse --verify --quiet "$StartRef^{commit}"
  if (-not $baseSha) { Fail "start ref '$StartRef' does not resolve (dry-run does not fetch; run a real prepare or fetch first)" }
  "dry-run: no changes made"
  "dry-run: would fetch: git -C $MainRepo fetch origin"
  if (Test-Path -LiteralPath $WtPath) {
    "dry-run: would switch: git -C $WtPath checkout -b $Branch $StartRef"
  } else {
    "dry-run: would create worktree: git -C $MainRepo worktree add -b $Branch $WtPath $StartRef"
  }
  "dry-run: worktree=$WtPath branch=$Branch start=$StartRef"
  exit 0
}

# --- 5. fetch + create/switch ------------------------------------------------

& git -C $MainRepo fetch origin 2>$null
if ($LASTEXITCODE -ne 0) { Fail "git fetch origin failed in '$MainRepo'" }
$baseSha = & git -C $MainRepo rev-parse --verify "$StartRef^{commit}"
if ($LASTEXITCODE -ne 0 -or -not $baseSha) { Fail "start ref '$StartRef' does not resolve after fetch" }

if (Test-Path -LiteralPath $WtPath) {
  & git -C $WtPath checkout -b $Branch $baseSha 2>$null
  if ($LASTEXITCODE -ne 0) { Fail "git checkout -b $Branch failed in '$WtPath'" }
} else {
  & git -C $MainRepo worktree add -b $Branch $WtPath $baseSha 2>$null
  if ($LASTEXITCODE -ne 0) { Fail "git worktree add -b $Branch $WtPath failed" }
}

"lane=$BuildLane"
"worktree=$WtPath"
"branch=$Branch"
"base=$baseSha"
