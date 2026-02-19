# ─────────────────────────────────────────────────────
# Edda Conductor Demo — Windows Terminal version
# ─────────────────────────────────────────────────────
#
# Usage:
#   .\run-demo.ps1 [plan.yaml]
#
# This opens Windows Terminal with split panes showing:
#   Left:  Conductor execution
#   Right: File watcher
#
# Prerequisites:
#   - Windows Terminal (wt.exe)
#   - edda.exe in PATH
#   - claude CLI logged in

param(
    [string]$Plan = "plan.yaml"
)

$DemoDir = Join-Path $env:TEMP "edda-demo-$(Get-Random)"
New-Item -ItemType Directory -Path $DemoDir -Force | Out-Null
Copy-Item $Plan "$DemoDir\plan.yaml"

Write-Host ""
Write-Host "📁 Demo workspace: $DemoDir" -ForegroundColor Cyan
Write-Host ""

# Windows Terminal with split panes
# Left: conductor | Right top: file watch | Right bottom: manual edda commands
$wtArgs = @(
    "new-tab",
    "--title", "Edda Demo",
    "-d", $DemoDir,
    "powershell", "-NoExit", "-Command", 
    "Write-Host '🎬 Conductor — press Enter to start' -ForegroundColor Yellow; Write-Host 'edda conduct run plan.yaml' -ForegroundColor Cyan",
    ";",
    "split-pane", "-H", "-s", "0.4",
    "-d", $DemoDir,
    "powershell", "-NoExit", "-Command",
    "while (`$true) { Clear-Host; Write-Host '📂 Files:' -ForegroundColor Green; Get-ChildItem -Recurse -Name -File | Where-Object { `$_ -notmatch 'target|\.edda|\.lock' } | Select-Object -First 25; Start-Sleep 2 }"
)

Write-Host "Opening Windows Terminal..." -ForegroundColor Green
Start-Process "wt.exe" -ArgumentList $wtArgs

Write-Host ""
Write-Host "┌─────────────────────────────────────────────┐"
Write-Host "│  In the left pane, run:                     │"
Write-Host "│  edda conduct run plan.yaml                 │"
Write-Host "│                                             │"
Write-Host "│  Watch agents build the project live!       │"
Write-Host "└─────────────────────────────────────────────┘"
