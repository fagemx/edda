param(
    [Parameter(Mandatory)]
    [string]$Archive,

    [string]$Repository = "C:\ai_agent\edda-wt-gh693-review-subject"
)

$expectedArchiveHash = "a303e1a5042c6a112c430920e1a7bd0257f3dccc1a7dc9a44909aeef4ae33214"
$sourceSha = "f94960306377e01e5e395704bc0876ec1fb4257b"
$sourcePath = "crates/edda-cli/src/cmd_dispatch.rs"
$requiredFields = @(
    "pub permission_mode: Option<String>",
    "pub model: Option<String>",
    "pub thinking: Option<String>",
    "pub tools: Option<Vec<String>>",
    "pub exclude_tools: Option<Vec<String>>",
    "pub session_dir: Option<String>"
)

if ((Get-FileHash -LiteralPath $Archive -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expectedArchiveHash) {
    throw "official v0.3.0 archive hash did not match"
}

$unpacked = Join-Path ([System.IO.Path]::GetTempPath()) ("edda-gh693-" + [guid]::NewGuid())
try {
    Expand-Archive -LiteralPath $Archive -DestinationPath $unpacked -Force
    $binary = Get-ChildItem -LiteralPath $unpacked -Recurse -Filter edda.exe | Select-Object -First 1 -ExpandProperty FullName
    if (-not $binary) { throw "official archive contained no edda.exe" }

    $version = & $binary --version
    if ($LASTEXITCODE -ne 0 -or $version -ne "edda 0.3.0") { throw "unexpected official v0.3.0 version result" }

    & $binary dispatch --help *> $null
    if ($LASTEXITCODE -ne 2) { throw "official v0.3.0 dispatch probe did not exit 2" }

    $source = git -C $Repository show "$sourceSha`:$sourcePath"
    if ($LASTEXITCODE -ne 0) { throw "could not read PR #684 source at $sourceSha" }
    $sourceText = $source -join "`n"
    foreach ($field in $requiredFields) {
        if ($sourceText -notmatch [regex]::Escape($field)) { throw "missing source field: $field" }
    }

    $fixture = Join-Path $unpacked "base-sha-fixture"
    git init -q --initial-branch main $fixture
    git -C $fixture config user.email "gh693@example.invalid"
    git -C $fixture config user.name "GH-693 fixture"
    Set-Content -LiteralPath (Join-Path $fixture "Cargo.toml") -Value "[workspace.package]`nversion = `"0.3.0`""
    git -C $fixture add Cargo.toml
    git -C $fixture commit -qm "base version"
    $baseSha = git -C $fixture rev-parse HEAD
    Set-Content -LiteralPath (Join-Path $fixture "Cargo.toml") -Value "[workspace.package]`nversion = `"0.4.0`""
    git -C $fixture commit -am "advance local main" -q
    $mutableBranchVersion = (git -C $fixture show "main:Cargo.toml") -join "`n"
    $pinnedBaseVersion = (git -C $fixture show "$baseSha`:Cargo.toml") -join "`n"
    if ($mutableBranchVersion -notmatch '0.4.0' -or $pinnedBaseVersion -notmatch '0.3.0') {
        throw "full base SHA did not isolate the version read from local main"
    }
} finally {
    if (Test-Path -LiteralPath $unpacked) { Remove-Item -LiteralPath $unpacked -Recurse -Force }
}

Write-Output "GH-693 runtime and source claim routing verified"
