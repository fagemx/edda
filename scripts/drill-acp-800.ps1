# GH-800 Windows ACP drill driver — standalone, no edda CLI dependency.
#
# Drives one ACP target through a 2-step rail chain over newline-delimited
# JSON-RPC stdio, persisting a safe metadata trace only:
#   step 1: initialize -> session/new (cwd = worktree) -> session/prompt -> stopReason
#           the session id stays in memory, then the process is killed (restart simulation)
#   step 2: fresh process -> initialize -> session/load(in-memory id) -> prompt "continue"
#
# Honesty rules (issue-800-current):
#   - a target is supported only if BOTH steps return stopReason end_turn
#   - any failure is reported with the failing method and a safe status;
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
    [ValidateSet("grok", "kilo", "pi", "claude")]
    [string]$Target,
    [string]$WorkTree,
    [string]$OutDir,
    [string]$Program,
    [string]$ProgramArgs,
    [switch]$Verifier,
    [string]$Prompt1 = "Drill step 1: create a file named acp-drill-step1.txt containing the word step1. Then stop.",
    [string]$Prompt2 = "Drill step 2: read acp-drill-step1.txt and report its contents. Then stop.",
    [int]$StepTimeoutSec = 300,
    [switch]$SelfTest
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

# Persist a deliberately tiny protocol trace, never a serialized message.
# In particular, agent notifications can contain global MCP configuration,
# headers, userinfo, unknown nested fields, or error text. Those values stay
# in memory solely long enough to drive the protocol.
$KnownMethods = @("initialize", "session/new", "session/load", "session/prompt", "session/request_permission")
$KnownRequestIds = @("1", "2", "3")
$KnownStopReasons = @("end_turn", "max_tokens", "refusal", "cancelled")
$UsageKeys = @("totalTokens", "inputTokens", "outputTokens")

function Add-NumericFacts($entry, $value, [string]$prefix) {
    foreach ($key in $UsageKeys) {
        $fact = $value.$key
        if ($fact -is [byte] -or $fact -is [int16] -or $fact -is [int] -or $fact -is [int64] -or
            $fact -is [uint16] -or $fact -is [uint32] -or $fact -is [uint64]) {
            $entry["$prefix$key"] = [uint64]$fact
        }
    }
}

function Get-SafeTraceEntry([string]$Direction, [string]$Line) {
    $entry = [ordered]@{ ts = (Get-Date).ToUniversalTime().ToString("o"); dir = $Direction; kind = "malformed" }
    try { $message = $Line | ConvertFrom-Json -AsHashtable -ErrorAction Stop } catch { return $entry }
    if ($null -eq $message) { return $entry }

    $id = "$($message.id)"
    if ($KnownRequestIds -contains $id) { $entry.requestId = $id }
    if ($message.ContainsKey("method")) {
        $entry.kind = if ($Direction -eq "out") { "request" } else { "notification" }
        $method = "$($message.method)"
        $entry.method = if ($KnownMethods -contains $method) { $method } else { "other" }
    } elseif ($message.ContainsKey("result") -or $message.ContainsKey("error")) {
        $entry.kind = "response"
        $entry.status = if ($message.ContainsKey("error")) { "error" } else { "ok" }
        if ($message.ContainsKey("result")) {
            $result = $message.result
            if ($result -is [System.Collections.IDictionary]) {
                if ($result.protocolVersion -is [byte] -or $result.protocolVersion -is [int16] -or
                    $result.protocolVersion -is [int] -or $result.protocolVersion -is [int64]) {
                    $entry.protocolVersion = [int]$result.protocolVersion
                }
                if ($KnownStopReasons -contains "$($result.stopReason)") { $entry.stopReason = "$($result.stopReason)" }
                if ($result.usage -is [System.Collections.IDictionary]) { Add-NumericFacts $entry $result.usage "usage_" }
            }
        }
    }
    return $entry
}

function Write-Transcript([string]$Path, [string]$Direction, [string]$Line) {
    $entry = Get-SafeTraceEntry $Direction $Line
    Add-Content -Path $Path -Value ($entry | ConvertTo-Json -Compress) -Encoding utf8
}

function Invoke-SafeTraceCanary {
    $path = Join-Path ([System.IO.Path]::GetTempPath()) ("edda-acp-safe-trace-" + [guid]::NewGuid() + ".jsonl")
    try {
        Write-Transcript $path "in" '{"jsonrpc":"2.0","method":"_x.ai/mcp/servers_updated","params":{"unknown":{"token":"CANARY_NESTED"},"headers":{"Authorization":"CANARY_HEADER"},"url":"https://CANARY_USERINFO@example.test"},"items":["CANARY_ARRAY"]}'
        Write-Transcript $path "in" 'not-json CANARY_MALFORMED https://CANARY_USERINFO@example.test'
        Write-Transcript $path "out" '{"jsonrpc":"2.0","id":"1","method":"initialize","params":{"clientInfo":{"name":"CANARY_AGENT"}}}'
        $trace = Get-Content -Raw $path
        foreach ($canary in @("CANARY_NESTED", "CANARY_HEADER", "CANARY_USERINFO", "CANARY_ARRAY", "CANARY_MALFORMED", "CANARY_AGENT")) {
            if ($trace.Contains($canary)) { throw "safe trace leaked synthetic canary" }
        }
        if (-not $trace.Contains('"method":"initialize"') -or -not $trace.Contains('"method":"other"') -or -not $trace.Contains('"kind":"malformed"')) {
            throw "safe trace did not retain required method-stage evidence"
        }
    } finally { Remove-Item $path -ErrorAction SilentlyContinue }
}

if ($SelfTest) { Invoke-SafeTraceCanary; Write-Host "safe trace canaries: passed"; exit 0 }
if (-not $Target) { throw "target is required" }
if (-not $WorkTree -or -not (Test-Path $WorkTree)) { throw "work tree does not exist: $WorkTree" }
if (-not $OutDir) { throw "out directory is required" }
if (-not (Test-Path $OutDir)) { New-Item -ItemType Directory -Path $OutDir | Out-Null }


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

$steps = @()
# The session id from session/new remains in memory for step 2 only. It is
# agent-supplied protocol data, so the drill never persists it in a session map.
$sessionId = $null
$supported = $true
$failingMethod = $null
$failureStatus = $null

for ($step = 1; $step -le 2; $step++) {
    $transcript = Join-Path $OutDir ("transcript-{0}-step{1}.jsonl" -f $Target, $step)
    Remove-Item $transcript -ErrorAction SilentlyContinue
    $stepResult = @{ step = "$step"; ok = $false; stopReason = $null; status = "not_started"; failingMethod = $null }
    $proc = $null
    $stage = "spawn"
    try {
        $proc = New-AcpChild
        Start-Sleep -Milliseconds 500
        if ($proc.HasExited) {
            throw "spawn: program '$Program' exited immediately (exit code $($proc.ExitCode)); not installed or not resolvable"
        }
        $stage = "initialize"
        Send-Acp $proc $transcript @{ jsonrpc = "2.0"; id = "1"; method = "initialize"; params = @{
            protocolVersion = 1
            clientCapabilities = @{ fs = @{ readTextFile = $false; writeTextFile = $false }; terminal = $false }
            clientInfo = @{ name = "edda-drill-800"; version = "0.1" }
        } }
        $resp = Wait-Response $proc $transcript "1" $StepTimeoutSec
        if ($null -eq $resp) { throw "initialize: no response within timeout" }
        if ($resp.error) { throw "initialize: $($resp.error.message)" }

        if ($step -eq 1) {
            $stage = "session/new"
            Send-Acp $proc $transcript @{ jsonrpc = "2.0"; id = "2"; method = "session/new"; params = @{
                cwd = $WorkTree; mcpServers = @()
            } }
            $resp = Wait-Response $proc $transcript "2" $StepTimeoutSec
            if ($null -eq $resp) { throw "session/new: no response within timeout" }
            if ($resp.error) { throw "session/new: $($resp.error.message)" }
            $sessionId = $resp.result.sessionId
        } else {
            $stage = "session/load"
            if (-not $sessionId) { throw "session/load: no session id retained from step 1" }
            Send-Acp $proc $transcript @{ jsonrpc = "2.0"; id = "2"; method = "session/load"; params = @{
                sessionId = $sessionId; cwd = $WorkTree; mcpServers = @()
            } }
            $resp = Wait-Response $proc $transcript "2" $StepTimeoutSec
            if ($null -eq $resp) { throw "session/load: no response within timeout" }
            if ($resp.error) { throw "session/load: $($resp.error.message)" }
        }
        $promptText = if ($step -eq 1) { $Prompt1 } else { $Prompt2 }
        $stage = "session/prompt"
        Send-Acp $proc $transcript @{ jsonrpc = "2.0"; id = "3"; method = "session/prompt"; params = @{
            sessionId = $sessionId
            prompt = @(@{ type = "text"; text = $promptText })
        } }
        $resp = Wait-Response $proc $transcript "3" $StepTimeoutSec
        if ($null -eq $resp) { throw "session/prompt: no response within timeout" }
        if ($resp.error) { throw "session/prompt: $($resp.error.message)" }
        $stepResult.stopReason = $resp.result.stopReason
        if ($resp.result.stopReason -eq "end_turn") { $stepResult.ok = $true; $stepResult.status = "end_turn" }
        else { throw "session/prompt: unexpected stop reason" }
    } catch {
        # Do not persist exception text: RPC error bodies, command errors, and
        # malformed output are agent-controlled and can contain credentials.
        $stepResult.failingMethod = $stage
        $stepResult.status = if ($_.Exception.Message -match "no response") { "timeout" }
            elseif ($_.Exception.Message -match "exited immediately") { "spawn_failed" }
            elseif ($_.Exception.Message -match "WriteLine") { "write_failed" }
            elseif ($_.Exception.Message -match "stop reason") { "unexpected_stop_reason" }
            else { "protocol_error" }
        $failingMethod = $stage
        $failureStatus = $stepResult.status
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

$result = [ordered]@{
    target = $Target
    verifier = [bool]$Verifier
    supported = $supported
    failingMethod = $failingMethod
    failureStatus = $failureStatus
    steps = $steps
}
$resultPath = Join-Path $OutDir "drill-result-$Target.json"
$result | ConvertTo-Json -Depth 10 | Set-Content -Path $resultPath -Encoding utf8
Write-Host "drill result: $resultPath"
if (-not $supported) { exit 1 }
