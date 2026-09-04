# GH-800 Windows ACP drill driver — standalone, no edda CLI dependency.
#
# Drives one ACP target through a 2-step rail chain over newline-delimited
# JSON-RPC stdio, capturing a full transcript of every line exchanged:
#   step 1: initialize -> session/new (cwd = worktree) -> session/prompt -> stopReason
#           session id is persisted to the session-map file (the task.session
#           carrier), then the process is killed (restart simulation)
#   step 2: fresh process -> initialize -> session/load(recorded id) -> prompt "continue"
#
# Honesty rules (issue-800-current):
#   - a target is supported only if BOTH steps return stopReason end_turn
#   - any failure is reported with the failing method and the exact error;
#     a passing --help probe is never a drill success
#   - session/request_permission from the agent is always answered (read-only
#     drill policy: reject_once when offered, outcome "cancelled" otherwise),
#     so the driver can never hang a turn
#
# Usage:
#   pwsh scripts/drill-acp-800.ps1 -Target grok -WorkTree C:\temp\acp-drill-wt `
#     -OutDir C:\temp\acp-drill-out
# Optional: -Program / -ProgramArgs to attempt package-local execution when
# the target binary is not on PATH; -Verifier to prepend documented
# read-only flags; -Prompt1/-Prompt2 to override the drill prompts.

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("grok", "kilo", "pi", "claude")]
    [string]$Target,
    [Parameter(Mandatory = $true)]
    [string]$WorkTree,
    [Parameter(Mandatory = $true)]
    [string]$OutDir,
    [string]$Program,
    [string]$ProgramArgs,
    [switch]$Verifier,
    [string]$Prompt1 = "Drill step 1: create a file named acp-drill-step1.txt containing the word step1. Then stop.",
    [string]$Prompt2 = "Drill step 2: read acp-drill-step1.txt and report its contents. Then stop.",
    [int]$StepTimeoutSec = 300
)

$ErrorActionPreference = "Stop"

# Per-target default endpoints (mirrors crates/edda-conductor/src/agent/acp_targets.rs).
$Endpoints = @{
    grok   = @{ Program = "grok"; Args = @("agent", "stdio"); ReadOnly = @("--sandbox", "read-only") }
    kilo   = @{ Program = "kilo"; Args = @("acp"); ReadOnly = @() }
    pi     = @{ Program = "pi-acp"; Args = @(); ReadOnly = @() }
    claude = @{ Program = "npx"; Args = @("--yes", "@agentclientprotocol/claude-agent-acp"); ReadOnly = @() }
}

if (-not $Program) {
    $Program = $Endpoints[$Target].Program
    $ArgList = @($Endpoints[$Target].Args)
} else {
    $ArgList = @()
    if ($ProgramArgs) {
        # Tolerate a caller-quoted value, then split on whitespace.
        $ArgList = ($ProgramArgs.Trim('"') -split "\s+") | Where-Object { $_ }
    }
}
if ($Verifier) { $ArgList = @($Endpoints[$Target].ReadOnly) + $ArgList }

if (-not (Test-Path $WorkTree)) { throw "work tree does not exist: $WorkTree" }
if (-not (Test-Path $OutDir)) { New-Item -ItemType Directory -Path $OutDir | Out-Null }

# F7 nested-session guard: the child is an independent stdio agent and must
# not inherit the parent host's session environment.
$StripEnv = @("CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT", "CLAUDE_CODE_SSE_PORT")

function New-AcpChild {
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = "cmd.exe"
    $psi.Arguments = "/C `"$Program`" " + ($ArgList -join " ")
    $psi.WorkingDirectory = $WorkTree
    $psi.UseShellExecute = $false
    $psi.RedirectStandardInput = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    foreach ($name in $StripEnv) { $psi.EnvironmentVariables.Remove($name) | Out-Null }
    $proc = [System.Diagnostics.Process]::Start($psi)
    return $proc
}

function Write-Transcript([string]$Path, [string]$Direction, [string]$Line) {
    $entry = @{
        ts   = (Get-Date).ToUniversalTime().ToString("o")
        dir  = $Direction
        line = (Format-Redacted $Line)
    }
    Add-Content -Path $Path -Value ($entry | ConvertTo-Json -Compress) -Encoding utf8
}

# Redact credential-shaped values before anything is persisted. Grok echoes
# the operator's global MCP config (with env values) in
# _x.ai/mcp/servers_updated — an observed leak class on 2026-09-04 — so
# transcripts are scrubbed structurally, and by pattern when a line does not
# parse as JSON.
function Format-Redacted([string]$Line) {
    $secretPattern = "(?i)(api[-_]?key|token|secret|password|credential)"
    try {
        $value = $Line | ConvertFrom-Json -AsHashtable -ErrorAction Stop
        return (Redact-Node $value $secretPattern | ConvertTo-Json -Compress -Depth 20)
    } catch {
        # Pattern fallback: "name":"...value..." pairs whose name is
        # credential-shaped, then the transcript wrapper's own keys.
        return [regex]::Replace($Line, '"([^"\\]*' + $secretPattern + '[^"\\]*)"\\s*:\\s*"([^"\\]*)"', '`$1":"[REDACTED]"')
    }
}

function Redact-Node($node, [string]$pattern) {
    if ($node -is [System.Collections.IDictionary]) {
        foreach ($k in @($node.Keys)) {
            if ("$k" -match $pattern) { $node[$k] = "[REDACTED]" }
            else { $node[$k] = Redact-Node $node[$k] $pattern }
        }
        return $node
    }
    if ($node -is [System.Collections.IList]) {
        for ($i = 0; $i -lt $node.Count; $i++) { $node[$i] = Redact-Node $node[$i] $pattern }
        return $node
    }
    return $node
}

function Send-Acp([System.Diagnostics.Process]$Proc, [string]$Path, [hashtable]$Message) {
    $line = $Message | ConvertTo-Json -Compress -Depth 10
    Write-Transcript $Path "out" $line
    $Proc.StandardInput.WriteLine($line)
    $Proc.StandardInput.Flush()
}

function Read-Acp([System.Diagnostics.Process]$Proc, [string]$Path, [int]$TimeoutSec) {
    $task = $Proc.StandardOutput.ReadLineAsync()
    if (-not $task.Wait([TimeSpan]::FromSeconds($TimeoutSec))) {
        return $null
    }
    $line = $task.Result
    if ($null -eq $line) { return $null }
    Write-Transcript $Path "in" $line
    return $line | ConvertFrom-Json
}

# Pump that routes lines until the JSON-RPC response with $Id arrives,
# answering session/request_permission (read-only) on the way and
# discarding notifications. Returns the response object or $null on timeout.
function Wait-Response([System.Diagnostics.Process]$Proc, [string]$Path, [string]$Id, [int]$TimeoutSec) {
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    while ((Get-Date) -lt $deadline) {
        $remaining = [int]($deadline - (Get-Date)).TotalSeconds
        if ($remaining -le 0) { break }
        $msg = Read-Acp $Proc $Path $remaining
        if ($null -eq $msg) { return $null }
        if ($msg.id -eq $Id -and ($null -ne $msg.result -or $null -ne $msg.error)) {
            return $msg
        }
        if ($msg.method -eq "session/request_permission") {
            $options = @($msg.params.options)
            $reject = $options | Where-Object { $_.kind -eq "reject_once" } | Select-Object -First 1
            $outcome = if ($reject) {
                @{ outcome = "selected"; optionId = $reject.optionId }
            } else {
                @{ outcome = "cancelled" }
            }
            Send-Acp $Proc $Path @{
                jsonrpc = "2.0"; id = $msg.id; result = @{ outcome = $outcome }
            }
        }
    }
    return $null
}

$mapPath = Join-Path $OutDir "session-map-$Target.json"
$steps = @()
$sessionId = $null
$supported = $true
$failingMethod = $null
$failingError = $null

for ($step = 1; $step -le 2; $step++) {
    $transcript = Join-Path $OutDir ("transcript-{0}-step{1}.jsonl" -f $Target, $step)
    Remove-Item $transcript -ErrorAction SilentlyContinue
    $stepResult = @{ step = "$step"; ok = $false; sessionId = $null; stopReason = $null; error = $null; failingMethod = $null }
    $proc = $null
    try {
        $proc = New-AcpChild
        Start-Sleep -Milliseconds 500
        if ($proc.HasExited) {
            throw "spawn: program '$Program' exited immediately (exit code $($proc.ExitCode)); not installed or not resolvable"
        }
        Send-Acp $proc $transcript @{ jsonrpc = "2.0"; id = "1"; method = "initialize"; params = @{
            protocolVersion = 1
            clientCapabilities = @{ fs = @{ readTextFile = $false; writeTextFile = $false }; terminal = $false }
            clientInfo = @{ name = "edda-drill-800"; version = "0.1" }
        } }
        $resp = Wait-Response $proc $transcript "1" $StepTimeoutSec
        if ($null -eq $resp) { throw "initialize: no response within timeout" }
        if ($resp.error) { throw "initialize: $($resp.error.message)" }

        if ($step -eq 1) {
            Send-Acp $proc $transcript @{ jsonrpc = "2.0"; id = "2"; method = "session/new"; params = @{
                cwd = $WorkTree; mcpServers = @()
            } }
            $resp = Wait-Response $proc $transcript "2" $StepTimeoutSec
            if ($null -eq $resp) { throw "session/new: no response within timeout" }
            if ($resp.error) { throw "session/new: $($resp.error.message)" }
            $sessionId = $resp.result.sessionId
        } else {
            if (-not $sessionId) { throw "session/load: no recorded session id from step 1" }
            Send-Acp $proc $transcript @{ jsonrpc = "2.0"; id = "2"; method = "session/load"; params = @{
                sessionId = $sessionId; cwd = $WorkTree; mcpServers = @()
            } }
            $resp = Wait-Response $proc $transcript "2" $StepTimeoutSec
            if ($null -eq $resp) { throw "session/load: no response within timeout" }
            if ($resp.error) { throw "session/load: $($resp.error.message)" }
        }
        $stepResult.sessionId = $sessionId

        $promptText = if ($step -eq 1) { $Prompt1 } else { $Prompt2 }
        Send-Acp $proc $transcript @{ jsonrpc = "2.0"; id = "3"; method = "session/prompt"; params = @{
            sessionId = $sessionId
            prompt = @(@{ type = "text"; text = $promptText })
        } }
        $resp = Wait-Response $proc $transcript "3" $StepTimeoutSec
        if ($null -eq $resp) { throw "session/prompt: no response within timeout" }
        if ($resp.error) { throw "session/prompt: $($resp.error.message)" }
        $stepResult.stopReason = $resp.result.stopReason
        if ($resp.result.stopReason -eq "end_turn") { $stepResult.ok = $true }
        else { throw "session/prompt: stopReason $($resp.result.stopReason)" }
    } catch {
        $stepResult.error = $_.Exception.Message
        $method = $null
        if ($_.Exception.Message -match "^([^:]+):") { $method = $Matches[1] }
        $stepResult.failingMethod = $method
        if ($method) { $failingMethod = $method }
        $failingError = $stepResult.error
        $supported = $false
    } finally {
        if ($proc -and -not $proc.HasExited) {
            $proc.Kill()
            $proc.WaitForExit(10000) | Out-Null
        }
        if ($proc) { $proc.Dispose() }
    }
    $steps += ,$stepResult
    if (-not $stepResult.ok) { break }
}

# Persist the session mapping exactly as the runner writes task.session.
if ($sessionId) {
    @{ target = $Target; acp_session_id = $sessionId; recorded = (Get-Date).ToUniversalTime().ToString("o") } |
        ConvertTo-Json -Compress | Set-Content -Path $mapPath -Encoding utf8
}

$result = [ordered]@{
    target = $Target
    verifier = [bool]$Verifier
    supported = $supported
    failingMethod = $failingMethod
    error = $failingError
    steps = $steps
}
$resultPath = Join-Path $OutDir "drill-result-$Target.json"
$result | ConvertTo-Json -Depth 10 | Set-Content -Path $resultPath -Encoding utf8
Write-Host "drill result: $resultPath"
if (-not $supported) { exit 1 }
