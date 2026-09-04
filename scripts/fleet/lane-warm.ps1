# lane-warm.ps1 — post-merge warmer for idle lane worktrees (GH-626).
#
# After a main merge, the controller warms each IDLE lane so the next issue
# starts with a hot target dir (#613 measured: warm lane ≈1s to first compile
# vs 30-84s cold). Warm means, strictly:
#
#   * the lane worktree was prepared by lane-prepare.ps1 (fixed path
#     <main-repo-parent>\<repo>-wt-<lane>; warm never creates it),
#   * no edda-lane-* task is Running on it (busy gate),
#   * the worktree is clean and its branch is pushed or detached (the same
#     unpushed gate as lane-prepare: the remote TIP must match; an upstream
#     configuration alone is not accepted),
#   * rust-lld.exe is available in the active toolchain sysroot — the shipped
#     .cargo/config.toml pins it as the MSVC linker (GH-810), so a missing
#     rust-lld would only surface as a mid-build linker error. Fail fast
#     instead. No linker path is ever set or guessed here; the repo config
#     ships it.
#
# Then it checks out the merged main tip (detached — no branch is created or
# moved) and runs `cargo build --workspace --all-targets` — build only, never
# tests — with the lane env from Get-LaneBuildEnv. The target dir is always
# the FIXED lane dir <lane-root>\<lane>, never a per-SHA directory. Trimming
# is deliberately not part of warm: stale `incremental` sessions are reclaimed
# by age per .claude/CLAUDE.md (Build lanes), and #613 measured trimming buys
# nothing.
#
# usage:
#   pwsh -NoProfile -File scripts/fleet/lane-warm.ps1 -BuildLane <lane> `
#        -Repo <main checkout> [-MainRef origin/main] [-LaneRoot <dir>] `
#        (-Warm | -DryRun | -PrintEnv)
#
# Exactly one mode is required: -PrintEnv (print the wrapper env, no repo
# needed), -DryRun (all read-only gates + exact commands, zero side effects —
# no fetch, no checkout, no cargo), or -Warm (the explicit requested warm).
#
# The lifecycle hook (a scheduled post-merge warmer) is intentionally NOT
# registered here: the controller invokes this script per idle lane after a
# merge, per the operator authorization on GH-626 — no hidden service without
# need.
param(
  [Parameter(Mandatory = $true)][string]$BuildLane,
  [string]$Repo = '',
  [string]$MainRef = 'origin/main',
  [string]$LaneRoot = '',
  [switch]$PrintEnv,
  [switch]$DryRun,
  [switch]$Warm
)

$ErrorActionPreference = 'Stop'

function Fail([string]$Msg) {
  [Console]::Error.WriteLine("lane-warm: $Msg")
  exit 1
}

# --- shared helpers (kept in sync with lane-prepare.ps1) ---------------------

$allowedBuildLanes = @('worker-1', 'worker-2', 'verifier', 'verifier-2')
if ($BuildLane -and $allowedBuildLanes -notcontains $BuildLane) {
  Fail "-BuildLane '$BuildLane' is not an allowed build lane (verification.cost-discipline allows only: $($allowedBuildLanes -join ', '))"
}

# The FIXED lane worktree path (identical formula to lane-prepare.ps1, which
# is the only thing allowed to create it).
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

function Test-LaneTaskBusy([string]$WtPath) {
  $want = ConvertTo-NormPath $WtPath
  foreach ($t in @(Get-ScheduledTask -TaskName 'edda-lane-*' -ErrorAction SilentlyContinue)) {
    if ($t.State -ne 'Running') { continue }
    foreach ($a in $t.Actions) {
      $wd = if ($a.WorkingDirectory) { $a.WorkingDirectory.TrimEnd('\') } else { '' }
      if ($wd -and ((ConvertTo-NormPath $wd) -eq $want)) { return $true }
    }
  }
  return $false
}

function Get-WtBranch([string]$WtPath) {
  return (& git -C $WtPath symbolic-ref --quiet HEAD 2>$null)
}

function Test-BranchPushed([string]$WtPath, [string]$Branch) {
  $remoteTip = & git -C $WtPath rev-parse --verify --quiet "refs/remotes/origin/$Branch^{commit}"
  if (-not $remoteTip) { return $false }
  $localTip = & git -C $WtPath rev-parse --verify "refs/heads/$Branch^{commit}"
  return ($remoteTip -eq $localTip)
}

# The lane build env — the single source of truth for what a lane wrapper must
# set. Exposed via -PrintEnv for the lane-launch.ps1 integration (parent
# handoff on GH-626): CARGO_INCREMENTAL=1 for worker lanes (focused -p builds),
# 0 for verifier lanes (the C5 run), and the CI-matching debug trim. The
# linker is deliberately absent: shipped .cargo/config.toml pins rust-lld.exe
# for x86_64-pc-windows-msvc, and no machine-specific path is duplicated.
function Get-LaneBuildEnv([string]$Lane, [string]$Root) {
  $incremental = if ($Lane -like 'verifier*') { '0' } else { '1' }
  return [ordered]@{
    'CARGO_TARGET_DIR'         = (Join-Path $Root $Lane)
    'CARGO_INCREMENTAL'        = $incremental
    'CARGO_PROFILE_DEV_DEBUG'  = 'line-tables-only'
    'CARGO_PROFILE_TEST_DEBUG' = 'line-tables-only'
  }
}

# Lane root precedence: explicit -LaneRoot > FLEET_LANE_ROOT > the default
# under LOCALAPPDATA (same default as lane-launch.ps1). Quoted throughout —
# a bare assignment of that path is a PowerShell parser error.
function Resolve-LaneRoot([string]$ExplicitRoot) {
  if ($ExplicitRoot) { return $ExplicitRoot }
  if ($env:FLEET_LANE_ROOT) { return $env:FLEET_LANE_ROOT }
  return (Join-Path $env:LOCALAPPDATA 'fleet-workstation\lanes')
}

# rust-lld availability, exactly as the shipped .cargo/config.toml resolves
# it: a bare linker name inside the toolchain's target bin directory.
function Get-RustLldState() {
  try {
    $sysroot = (& rustc --print sysroot 2>$null)
    if (-not $sysroot -or $LASTEXITCODE -ne 0) { return 'absent (rustc not runnable)' }
    $lld = Join-Path ([string]$sysroot) 'lib\rustlib\x86_64-pc-windows-msvc\bin\rust-lld.exe'
    if (Test-Path -LiteralPath $lld -PathType Leaf) { return "present ($lld)" }
    return "absent (expected $lld)"
  } catch {
    return 'absent (rustc not runnable)'
  }
}

# --- mode selection -----------------------------------------------------------

$modes = @()
if ($PrintEnv) { $modes += 'PrintEnv' }
if ($DryRun) { $modes += 'DryRun' }
if ($Warm) { $modes += 'Warm' }
if ($modes.Count -ne 1) {
  Fail "exactly one of -PrintEnv, -DryRun, -Warm is required (got: $(if ($modes) { $modes -join ', ' } else { 'none' }))"
}

# --- -PrintEnv: expose the wrapper env for the lane-launch integration -------

if ($PrintEnv) {
  $root = Resolve-LaneRoot $LaneRoot
  $env0 = Get-LaneBuildEnv $BuildLane $root
  "# lane wrapper env for -BuildLane $BuildLane (lane-launch.ps1 integration, GH-626)"
  foreach ($k in $env0.Keys) { "$k=$($env0[$k])" }
  "# linker: not set here — shipped .cargo/config.toml pins rust-lld.exe for x86_64-pc-windows-msvc"
  exit 0
}

# --- -DryRun / -Warm: the lane worktree must already be prepared --------------

if (-not $Repo) { Fail "-Repo is required for -DryRun and -Warm" }
& git -C $Repo rev-parse --git-dir >$null 2>$null
if ($LASTEXITCODE -ne 0) { Fail "-Repo '$Repo' is not a git repository" }
$commonDir = (& git -C $Repo rev-parse --path-format=absolute --git-common-dir).Trim()
$MainRepo = Split-Path -Parent $commonDir

$WtPath = Get-LaneWorktreePath $MainRepo $BuildLane
if (-not (Test-Path -LiteralPath $WtPath -PathType Container)) {
  Fail "lane worktree '$WtPath' does not exist; run scripts/fleet/lane-prepare.ps1 -BuildLane $BuildLane first (warm never creates worktrees)"
}
if (-not (Test-WtRegistered $MainRepo $WtPath)) { Fail "'$WtPath' is not a registered worktree of '$MainRepo'; refusing" }

$reasons = @()
if (Test-LaneTaskBusy $WtPath) { $reasons += "busy: a running edda-lane-* task is bound to '$WtPath'" }
$dirty = (& git -C $WtPath status --porcelain)
if ($dirty) { $reasons += "dirty worktree:`n$dirty" }
$cur = Get-WtBranch $WtPath
$pushed = $true
if ($cur) {
  $curBranch = $cur -replace '^refs/heads/', ''
  $pushed = Test-BranchPushed $WtPath $curBranch
  if (-not $pushed) {
    $remoteState = if (& git -C $WtPath rev-parse --verify --quiet "refs/remotes/origin/$curBranch^{commit}") { 'differs from' } else { 'is missing from' }
    $reasons += "branch '$curBranch' is not pushed: the remote tip $remoteState refs/remotes/origin/$curBranch; an upstream configuration alone is not sufficient"
  }
}
$rustLld = Get-RustLldState

if ($reasons.Count -gt 0) {
  $msg = "warm refused for lane '$BuildLane' on '$WtPath': $($reasons -join '; ')"
  if ($DryRun) { [Console]::Error.WriteLine("lane-warm: dry-run: would refuse: $msg"); exit 1 }
  Fail $msg
}

if ($rustLld -like 'absent*') {
  $msg = "rust-lld.exe is $rustLld — the shipped .cargo/config.toml pins it as the MSVC linker, so the build would fail; update the toolchain (GH-810)"
  if ($DryRun) { [Console]::Error.WriteLine("lane-warm: dry-run: would refuse: $msg"); exit 1 }
  Fail $msg
}

$laneRoot = Resolve-LaneRoot $LaneRoot
$buildEnv = Get-LaneBuildEnv $BuildLane $laneRoot

if ($DryRun) {
  "warm: dry-run, no changes made"
  "warm: worktree=$WtPath"
  foreach ($k in $buildEnv.Keys) { "warm: $k=$($buildEnv[$k])" }
  "warm: linker=from shipped .cargo/config.toml (rust-lld.exe); rustlld=$rustLld"
  "warm: would fetch: git -C $MainRepo fetch origin"
  "warm: would checkout: git -C $WtPath checkout --detach $MainRef"
  "warm: would run (cwd=$WtPath): cargo build --workspace --all-targets"
  exit 0
}

# --- the explicit requested warm ----------------------------------------------

& git -C $MainRepo fetch origin 2>$null
if ($LASTEXITCODE -ne 0) { Fail "git fetch origin failed in '$MainRepo'" }
$mainSha = & git -C $MainRepo rev-parse --verify "$MainRef^{commit}"
if ($LASTEXITCODE -ne 0 -or -not $mainSha) { Fail "main ref '$MainRef' does not resolve after fetch" }

& git -C $WtPath checkout --detach $mainSha 2>$null
if ($LASTEXITCODE -ne 0) { Fail "git checkout --detach $mainSha failed in '$WtPath'" }

foreach ($k in $buildEnv.Keys) { Set-Item -Path "env:$k" -Value $buildEnv[$k] }

Push-Location -LiteralPath $WtPath
try {
  $start = Get-Date
  & cargo build --workspace --all-targets 2>&1 | ForEach-Object { "$_" } | Write-Output
  $code = $LASTEXITCODE
  $seconds = [math]::Round(((Get-Date) - $start).TotalSeconds, 1)
  "warm: lane=$BuildLane worktree=$WtPath main=$mainSha"
  "warm: cargo exit=$code duration=${seconds}s target_dir=$($buildEnv['CARGO_TARGET_DIR'])"
  exit $code
} finally {
  Pop-Location
}
