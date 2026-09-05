# Synthetic leak canaries for the GH-800 ACP drill's persisted metadata trace.
# No agent binary, credential store, or network access is required.
$ErrorActionPreference = "Stop"
$scriptPath = Join-Path $PSScriptRoot "drill-acp-800.ps1"
& $scriptPath -SelfTest
if ($LASTEXITCODE -ne 0) { throw "safe trace canaries failed with exit code $LASTEXITCODE" }
