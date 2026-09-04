# LanePrivilegeSpike.psm1 — shared fail-closed helpers for the GH-690 lane privilege spike.
#
# Design invariants (do not weaken):
#   1. Principal comes from WindowsIdentity, never from environment variables —
#      a spoofed USERNAME/USERPROFILE must never let an unrestricted caller pass.
#   2. Protected-credential probes only open and immediately dispose a read handle;
#      they never read, log, or print file content. Only real access-denied evidence
#      satisfies the negative property; sharing violations and other IO errors are
#      INCONCLUSIVE, never protection proof (Round1 F5).
#   3. Token values never appear in output, argv, environment, or files. All emitted
#      text passes through Hide-SecretPatterns. The publication path moves the token
#      exclusively through a stdin pipe into git's in-memory credential cache (Round1 F3).
#   4. secret-ref resolution is real or explicitly UNSUPPORTED — never a mock value.
#   5. Publication binds the actual destination (resolved push URL) to the repo
#      allowlist immediately before the push, and emits exactly one explicit refspec
#      with tag-following disabled (Round1 F2).
#   6. Uncertain gh metadata (parse failure, error exit, login-without-token) is
#      INCONCLUSIVE, never a pass (Round1 F1).

Set-StrictMode -Version 3.0

# Well-known exit codes (documented in README.md; keep in sync).
$script:EXIT_OK                 = 0
$script:EXIT_PREFLIGHT_FAILED   = 2
$script:EXIT_PRINCIPAL_REFUSED  = 3
$script:EXIT_PROTECTION_FAIL    = 4   # negative test observed READABLE / reachable token
$script:EXIT_PROVIDER_UNSUPPORTED = 5
$script:EXIT_INCONCLUSIVE       = 6
$script:EXIT_PUBLICATION_FAILED = 7

# Token shapes: GitHub OAuth/user/server/fine-grained PATs and generic bearer-ish
# values. Used ONLY to scrub output; never to validate a token.
$script:SecretPatterns = @(
    'gh[pousr]_[A-Za-z0-9]{16,}',
    'github_pat_[A-Za-z0-9_]{20,}',
    'AKIA[0-9A-Z]{16}',
    '-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----'
)

function Get-SpikePrincipal {
    <#
        .SYNOPSIS
        Returns the real Windows principal of the current process.
        .DESCRIPTION
        Deliberately ignores $env:USERNAME / $env:USERPROFILE: those are per-process
        values an unrestricted caller can set to anything. WindowsIdentity is the
        process token, which is what the operating system enforces ACLs against.
    #>
    return [System.Security.Principal.WindowsIdentity]::GetCurrent().Name
}

function Test-SpikePrincipalMatch {
    param(
        [Parameter(Mandatory)][string]$Expected,
        [Parameter(Mandatory)][string]$Actual
    )
    return [string]::Equals($Expected, $Actual, [System.StringComparison]::OrdinalIgnoreCase)
}

function Hide-SecretPatterns {
    <#
        .SYNOPSIS
        Replaces anything that looks like a credential with a fixed placeholder.
        Applied to every line this harness prints.
    #>
    param(
        [Parameter(Mandatory)][AllowEmptyString()][string]$Text
    )
    $out = $Text
    foreach ($pattern in $script:SecretPatterns) {
        $out = [regex]::Replace($out, $pattern, '[REDACTED]')
    }
    return $out
}

function Write-SpikeSafe {
    param([Parameter(Mandatory)][AllowEmptyString()][string]$Text)
    Write-Output (Hide-SecretPatterns -Text $Text)
}

function Assert-SafeSpikeBranch {
    <#
        .SYNOPSIS
        Fails closed unless $Branch is an exact member of the branch allowlist AND
        is under the spike/ namespace. main / master / HEAD are refused explicitly
        so a misconfigured allowlist can never authorize a main push.
    #>
    param(
        [Parameter(Mandatory)][string]$Branch,
        [Parameter(Mandatory)][string[]]$BranchAllowList
    )
    if ([string]::IsNullOrWhiteSpace($Branch)) {
        throw "Branch guard: empty branch name refused."
    }
    $forbidden = @('main', 'master', 'HEAD', 'origin/main', 'origin/master')
    if ($forbidden -contains $Branch) {
        throw "Branch guard: '$Branch' may never be a spike push target."
    }
    if (-not $Branch.StartsWith('spike/', [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Branch guard: spike pushes must live under 'spike/' (got '$Branch')."
    }
    if ($Branch.Contains('..') -or $Branch.Contains(' ') -or $Branch.Contains('*')) {
        throw "Branch guard: branch name contains forbidden characters: '$Branch'."
    }
    $exact = $BranchAllowList | Where-Object {
        [string]::Equals($_, $Branch, [System.StringComparison]::OrdinalIgnoreCase)
    }
    if (-not $exact) {
        throw ("Branch guard: branch '{0}' is not in the allowlist [{1}]." -f
            $Branch, ($BranchAllowList -join ', '))
    }
    return $true
}

function Assert-SafeRepoTarget {
    <#
        .SYNOPSIS
        Validates one allowlist entry as a well-formed 'owner/repo' pair (no
        wildcards, no traversal). The ACTUAL publication destination is bound
        separately by Get-EffectivePushUrl / Test-PushUrlAllowed immediately
        before the push (Round1 F2).
    #>
    param(
        [Parameter(Mandatory)][string]$Repo,
        [Parameter(Mandatory)][string[]]$RepoAllowList
    )
    if ([string]::IsNullOrWhiteSpace($Repo) -or $Repo.Contains('*')) {
        throw "Repo guard: empty or wildcard repo refused."
    }
    if ($Repo -notmatch '^[A-Za-z0-9][A-Za-z0-9_.-]*/[A-Za-z0-9][A-Za-z0-9_.-]*$') {
        throw "Repo guard: '$Repo' is not a well-formed owner/repo pair."
    }
    $exact = $RepoAllowList | Where-Object {
        [string]::Equals($_, $Repo, [System.StringComparison]::OrdinalIgnoreCase)
    }
    if (-not $exact) {
        throw ("Repo guard: repo '{0}' is not in the allowlist [{1}]." -f
            $Repo, ($RepoAllowList -join ', '))
    }
    return $true
}

function Test-PushUrlAllowed {
    <# Clean HTTPS only: credential-cache cannot control SSH or URL-embedded auth. #>
    param(
        [Parameter(Mandatory)][string]$Url,
        [Parameter(Mandatory)][string[]]$RepoAllowList
    )
    if ([string]::IsNullOrWhiteSpace($Url)) { return $false }
    try { $uri = [uri]$Url } catch { return $false }
    if ($uri.Scheme -ne 'https' -or $uri.Host -ne 'github.com' -or -not [string]::IsNullOrEmpty($uri.UserInfo) -or
        $uri.Query -or $uri.Fragment -or $uri.Port -ne 443) { return $false }
    $path = $uri.AbsolutePath.Trim('/')
    if ($path.EndsWith('.git', [System.StringComparison]::OrdinalIgnoreCase)) { $path = $path.Substring(0, $path.Length - 4) }
    if ($path -notmatch '^[A-Za-z0-9][A-Za-z0-9_.-]*/[A-Za-z0-9][A-Za-z0-9_.-]*$' -or $path.Contains('..')) { return $false }
    return [bool]($RepoAllowList | Where-Object { [string]::Equals($_, $path, [System.StringComparison]::OrdinalIgnoreCase) })
}

function Get-EffectivePushUrl {
    <# Resolves every rewritten target Git would use, requiring exactly one clean HTTPS URL. #>
    param(
        [Parameter(Mandatory)][string]$WorkspacePath,
        [Parameter(Mandatory)][string[]]$RepoAllowList,
        [string]$RemoteName = 'origin'
    )
    # `remote get-url --push --all` applies Git's url.*.pushInsteadOf rules and
    # returns every pushurl (or every fallback URL), unlike config --get.
    $urls = @(& git -C $WorkspacePath remote get-url --push --all $RemoteName 2>$null |
        ForEach-Object { "$($_)".Trim() } | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
    if ($LASTEXITCODE -ne 0 -or $urls.Count -ne 1) {
        throw ("Push destination guard: remote '{0}' must resolve to exactly one effective push URL (got {1})." -f $RemoteName, $urls.Count)
    }
    $url = $urls[0]
    if (-not (Test-PushUrlAllowed -Url $url -RepoAllowList $RepoAllowList)) {
        throw ("Push destination guard: effective push URL '{0}' is not a clean allowlisted HTTPS destination [{1}]." -f
            (Hide-SecretPatterns -Text $url), ($RepoAllowList -join ', '))
    }
    return $url
}

function Resolve-SpikePrincipalProfile {
    <# Resolves a principal's profile through its Windows SID/ProfileList mapping, not USERPROFILE. #>
    param([Parameter(Mandatory)][string]$Principal)
    try {
        $sid = ([System.Security.Principal.NTAccount]$Principal).Translate([System.Security.Principal.SecurityIdentifier]).Value
        $key = "HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList\$sid"
        $profile = (Get-ItemProperty -LiteralPath $key -Name ProfileImagePath -ErrorAction Stop).ProfileImagePath
        if ([string]::IsNullOrWhiteSpace($profile) -or -not [System.IO.Path]::IsPathRooted($profile)) { throw 'profile path missing' }
        return [System.IO.Path]::GetFullPath([Environment]::ExpandEnvironmentVariables($profile))
    }
    catch { throw "Protected-target guard: cannot resolve trusted Windows profile for '$Principal'." }
}

function Assert-ProtectedCredentialTargets {
    <# Requires exactly the documented operator credential files under the trusted operator profile. #>
    param(
        [Parameter(Mandatory)][string]$OperatorPrincipal,
        [Parameter(Mandatory)][string]$RestrictedPrincipal,
        [Parameter(Mandatory)][string[]]$ProtectedCredentialFiles,
        [scriptblock]$ProfileResolver = ${function:Resolve-SpikePrincipalProfile}
    )
    $operatorProfile = & $ProfileResolver $OperatorPrincipal
    $restrictedProfile = & $ProfileResolver $RestrictedPrincipal
    $expected = @('.claude\.credentials.json', '.codex\auth.json', '.pi\agent\auth.json') |
        ForEach-Object { [System.IO.Path]::GetFullPath((Join-Path $operatorProfile $_)) }
    if ($ProtectedCredentialFiles.Count -ne $expected.Count) { throw 'Protected-target guard: exactly three explicit operator credential targets are required.' }
    $actual = @()
    foreach ($inputPath in $ProtectedCredentialFiles) {
        if ([string]::IsNullOrWhiteSpace($inputPath) -or -not [System.IO.Path]::IsPathRooted($inputPath)) {
            throw "Protected-target guard: credential target must be a nonempty absolute input path."
        }
        $full = [System.IO.Path]::GetFullPath($inputPath)
        if ($full.StartsWith($restrictedProfile, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw 'Protected-target guard: a target inside the trusted restricted profile is refused.'
        }
        $actual += $full
    }
    if (($actual | Select-Object -Unique).Count -ne $actual.Count -or
        (@($actual | Where-Object { $expected -notcontains $_ }).Count -ne 0)) {
        throw 'Protected-target guard: targets must be the exact explicit credential set under the trusted operator profile.'
    }
    return $actual
}

enum CredentialProbeResult {
    AccessDenied    # real access-denied evidence — protection property MET
    Readable        # protection property NOT met — baseline failure signal
    NotFound        # inconclusive: target absent, nothing proven
    Inconclusive    # inconclusive: sharing violation / transient IO state, NOT access control (Round1 F5)
    ProbeError      # probe itself failed for an unexpected reason
}

function Get-CredentialFileProbe {
    <#
        .SYNOPSIS
        Attempts to open a read handle on a protected credential file and immediately
        disposes it. NEVER reads, stores, or prints any byte of file content.
        .OUTPUTS
        [CredentialProbeResult]. Only UnauthorizedAccessException maps to AccessDenied;
        sharing violations and other IO failures map to Inconclusive (Round1 F5).
    #>
    param([Parameter(Mandatory)][string]$Path)

    try {
        if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
            return [CredentialProbeResult]::NotFound
        }
        $stream = $null
        try {
            $stream = [System.IO.File]::Open(
                $Path,
                [System.IO.FileMode]::Open,
                [System.IO.FileAccess]::Read,
                [System.IO.FileShare]::Read)
            # Disposed without a single read. Length metadata is not content.
            return [CredentialProbeResult]::Readable
        }
        catch [System.UnauthorizedAccessException] {
            return [CredentialProbeResult]::AccessDenied
        }
        catch [System.IO.IOException] {
            # Sharing violation or transient IO state: we could not open the file,
            # but that is NOT proof of an ACL denial. Inconclusive, never evidence.
            return [CredentialProbeResult]::Inconclusive
        }
        finally {
            if ($null -ne $stream) { $stream.Dispose() }
        }
    }
    catch {
        return [CredentialProbeResult]::ProbeError
    }
}

enum TokenProviderKind {
    NodeBroker    # edda-node:// — real scheme; resolution requires the node credential-broker endpoint (GH-685 follow-up, not implemented in node v0)
    WindowsCredMan# credman://  — real scheme; resolution requires the CredentialManager PowerShell module
    Forbidden     # file: / env: — refused by design (secrets never on disk, never in env)
    Unsupported   # unknown scheme — explicitly unsupported, never mocked
}

function Get-TokenProviderKind {
    <#
        .SYNOPSIS
        Classifies a token provider reference by scheme. Pure syntax work: no
        resolution, no I/O, no secret contact. Safe for preflight.
    #>
    param([Parameter(Mandatory)][string]$ProviderRef)

    if ([string]::IsNullOrWhiteSpace($ProviderRef)) {
        throw "Token provider reference is empty."
    }
    if ($ProviderRef.StartsWith('edda-node://', [System.StringComparison]::OrdinalIgnoreCase)) {
        return [TokenProviderKind]::NodeBroker
    }
    if ($ProviderRef.StartsWith('credman://', [System.StringComparison]::OrdinalIgnoreCase)) {
        return [TokenProviderKind]::WindowsCredMan
    }
    if ($ProviderRef -match '^(?i)(file|env):') {
        return [TokenProviderKind]::Forbidden
    }
    return [TokenProviderKind]::Unsupported
}

function Resolve-BrokerToken {
    <#
        .SYNOPSIS
        Resolves a token provider reference to a SecureString — for real, or fails
        with an explicit UNSUPPORTED classification. Never returns a mock value.
        .NOTES
        The returned SecureString stays in memory and is consumed only by
        Invoke-SpikePublication, which moves it through a stdin pipe into git's
        in-memory credential cache. It must never be persisted, logged, or passed
        through argv/environment.
    #>
    param([Parameter(Mandatory)][string]$ProviderRef)

    $kind = Get-TokenProviderKind -ProviderRef $ProviderRef
    switch ($kind) {
        'Forbidden' {
            throw ("UNSUPPORTED: provider scheme '{0}' is forbidden by design " +
                   "(secrets never on disk, never in env; decision fleet.lane-privilege)." -f
                   ($ProviderRef -replace '^(?i)[^:]+://.*$', '$1:…'))
        }
        'Unsupported' {
            throw "UNSUPPORTED: unknown provider scheme in reference (scheme refused; no mock substitution)."
        }
        'NodeBroker' {
            # The node credential-broker endpoint does not exist in node v0 (GH-685
            # ships /api/sync only). Saying so IS the honest result; substituting a
            # fake token here would manufacture false spike evidence.
            throw ("UNSUPPORTED: node credential-broker endpoint is not implemented in node v0 " +
                   "(GH-685 design: docs/superpowers/specs/2026-09-02-edda-node-agent-transport-design.md). " +
                   "Real resolution requires the GH-690 follow-up broker.")
        }
        'WindowsCredMan' {
            $module = Get-Module -ListAvailable -Name CredentialManager
            if ($null -eq $module) {
                throw "UNSUPPORTED: CredentialManager PowerShell module not installed; real credman:// resolution unavailable on this host."
            }
            $name = ($ProviderRef -replace '^(?i)credman://', '') -replace '/.*$', ''
            $cred = Get-StoredCredential -Target $name -AsPlainText:$false
            if ($null -eq $cred) {
                throw "UNSUPPORTED: no credential named '$name' in Windows Credential Manager (fail-closed; no mock substitution)."
            }
            $secure = $cred.Password
            $cred = $null
            return $secure
        }
        default {
            throw "UNSUPPORTED: unhandled provider kind."
        }
    }
}

function Get-GhTokenSourceMetadata {
    <#
        .SYNOPSIS
        Reduces `gh auth status` output to identity/token-SOURCE metadata only.
        Token values (even the masked forms gh prints) are dropped BEFORE anything
        is returned or printed. The source class is parsed FROM the token line
        before it is discarded (Round1 F1).
        .PARAMETER StatusText
        Synthetic injection point for fixture tests: when supplied, these lines are
        reduced instead of invoking gh. The real action path omits this parameter.
        .NOTES
        Throws (fail closed) when gh is missing or `gh auth status` exits nonzero —
        a failed status must never be read as "no token".
    #>
    param([string[]]$StatusText)

    $meta = [pscustomobject]@{
        LoggedIn     = $false
        Host         = $null
        Account      = $null
        TokenPresent = $false
        TokenSource  = 'none'
        GhExit       = 0
    }

    if ($null -eq $StatusText -or $StatusText.Count -eq 0) {
        if ($null -eq (Get-Command gh -ErrorAction SilentlyContinue)) {
            throw "INCONCLUSIVE: gh CLI not found; token-source metadata cannot be collected."
        }
        $raw = & gh auth status 2>&1
        $meta.GhExit = $LASTEXITCODE
        if ($meta.GhExit -ne 0) {
            throw ("INCONCLUSIVE: gh auth status exited {0}; token-source state unknown." -f $meta.GhExit)
        }
    }
    else {
        $raw = $StatusText
        $meta.GhExit = 0
    }

    foreach ($line in @($raw)) {
        $text = "$line"
        if ($text -match '(?i)token') {
            # Parse the source class from the token line BEFORE discarding it.
            # An annotated token line is BOTH present and sourced (Round1 F1).
            if ($text -match '(?i)no token|token: none') {
                # An explicit absence statement is not a token.
            }
            else {
                $meta.TokenPresent = $true
                if ($text -match '\((keyring|infile|env|oauth_token)\)') {
                    $meta.TokenSource = $Matches[1]
                }
                else {
                    $meta.TokenSource = 'unknown'
                }
            }
            # Any other token-bearing line is dropped here; it never reaches output.
            continue
        }
        if ($text -match 'github\.com') { $meta.Host = 'github.com' }
        if ($text -match 'Logged in to github\.com (?:as|account) ([A-Za-z0-9-]+)') {
            $meta.LoggedIn = $true
            $meta.Account = $Matches[1]
        }
    }
    return $meta
}

function Get-GhMetadataVerdict {
    <#
        .SYNOPSIS
        Adjudicates gh metadata for the negative property "no token is reachable
        from the restricted lane". Fail closed: any present token is a violation
        (including unparsable source); logged-in-without-token and error exits are
        inconclusive, never a pass (Round1 F1).
        .OUTPUTS
        'ok-no-login' | 'deny-token-present' | 'inconclusive-ambiguous' | 'inconclusive-error'
    #>
    param(
        [Parameter(Mandatory)]$Metadata,
        [int]$GhExitCode = 0
    )
    if ($GhExitCode -ne 0) { return 'inconclusive-error' }
    if ($Metadata.TokenPresent) { return 'deny-token-present' }
    if ($Metadata.LoggedIn) { return 'inconclusive-ambiguous' }
    return 'ok-no-login'
}

function ConvertFrom-SecureStringInMemory {
    <#
        .SYNOPSIS
        Decrypts a SecureString to plaintext for immediate, in-process use only.
        The caller must consume the value without storing, logging, or passing it
        through argv/environment, and must drop the reference immediately.
    #>
    param([Parameter(Mandatory)][securestring]$SecureValue)
    $bstr = [System.Runtime.InteropServices.Marshal]::SecureStringToBSTR($SecureValue)
    try {
        return [System.Runtime.InteropServices.Marshal]::PtrToStringBSTR($bstr)
    }
    finally {
        [System.Runtime.InteropServices.Marshal]::ZeroFreeBSTR($bstr)
    }
}

function Invoke-GitProcess {
    <#
        .SYNOPSIS
        Real git invoker: runs git with the given argument array and optional stdin
        payload, and returns an observable result object. Arguments are passed as
        an array (never interpolated into a command line string).
    #>
    param(
        [Parameter(Mandatory)][string[]]$GitArgs,
        [string]$StdinInput,
        [string]$WorkingDirectory
    )
    $out = $null
    if ($PSBoundParameters.ContainsKey('WorkingDirectory') -and -not [string]::IsNullOrWhiteSpace($WorkingDirectory)) {
        if ($PSBoundParameters.ContainsKey('StdinInput') -and $null -ne $StdinInput) {
            $out = $StdinInput | & git @GitArgs 2>&1
        }
        else {
            Push-Location $WorkingDirectory
            try { $out = & git @GitArgs 2>&1 } finally { Pop-Location }
        }
    }
    elseif ($PSBoundParameters.ContainsKey('StdinInput') -and $null -ne $StdinInput) {
        $out = $StdinInput | & git @GitArgs 2>&1
    }
    else {
        $out = & git @GitArgs 2>&1
    }
    return [pscustomobject]@{
        ExitCode = $LASTEXITCODE
        Output   = @($out | ForEach-Object { "$_" })
        Args     = $GitArgs
    }
}

function Test-GitCredentialCacheAvailable {
    <#
        .SYNOPSIS
        True only when git ships the in-memory credential-cache helper on this host.
        The publication path is UNSUPPORTED without it (no persistent fallback).
    #>
    return ($null -ne (Get-Command 'git-credential-cache' -ErrorAction SilentlyContinue))
}

function Invoke-SpikePublication {
    <#
        .SYNOPSIS
        Publishes the spike branch using a NONPERSISTENT, process-local credential
        path (Round1 F3): the resolved token is moved through a stdin pipe into
        git's in-memory credential-cache daemon (isolated helper list, short
        timeout), the push uses that cache and ONLY that cache, and the entry is
        rejected and the daemon stopped afterwards. No `gh auth login` is involved;
        nothing is written to disk, keyring, environment, or argv.
        .PARAMETER GitInvoker
        Injectable git delegate for fixture tests (default: Invoke-GitProcess).
        Receives the same parameter set and must return an object with ExitCode,
        Output, Args. Stdin is intentionally never retained in result objects.
        .OUTPUTS
        PSCustomObject: Verdict ('published'|'auth-failed'|'push-failed'|
        'cleanup-incomplete'), PushExit, CleanupExit, Notes.
        A 'published' verdict proves ONLY that git published the ref; it does not
        by itself prove the token was broker-scoped — that requires the broker's
        own audit trail (documented in README.md).
    #>
    param(
        [Parameter(Mandatory)][securestring]$Token,
        [Parameter(Mandatory)][string]$WorkspacePath,
        [Parameter(Mandatory)][string]$DestinationUrl,
        [Parameter(Mandatory)][string]$RefSpec,
        [string]$CredentialUsername = 'x-access-token',
        [string]$CredentialHost = 'github.com',
        [int]$CacheTimeoutSeconds = 120,
        [scriptblock]$GitInvoker = ${function:Invoke-GitProcess}
    )

    $helperArgs = @(
        '-c', 'credential.helper=',
        '-c', "credential.helper=cache --timeout=$CacheTimeoutSeconds"
    )

    # Build the credential payload; the plaintext token exists only here, in
    # process memory, and only long enough to pipe it to `git credential approve`.
    $plain = ConvertFrom-SecureStringInMemory -SecureValue $Token
    $payload = "protocol=https`nhost=$CredentialHost`nusername=$CredentialUsername`npassword=$plain`n`n"
    $plain = $null   # drop the reference; the cached copy is evicted in cleanup

    $notes = [System.Collections.Generic.List[string]]::new()
    $pushExit = $null
    $cleanupExit = 0
    $verdict = 'auth-failed'
    $originalVerdict = $null
    $transportDir = Join-Path ([System.IO.Path]::GetTempPath()) ("gh690-transport-" + [guid]::NewGuid().ToString('N'))
    $savedGitEnvironment = @{}

    try {
        # Bind the named workspace before resolving its source object. Snapshot and
        # clear every inherited GIT_* routing/config variable FIRST: `git -C` alone
        # does not defeat GIT_DIR, GIT_WORK_TREE, or GIT_OBJECT_DIRECTORY.
        $globalConfig = Join-Path $transportDir 'global.gitconfig'
        New-Item -ItemType Directory -Path $transportDir -Force | Out-Null
        New-Item -ItemType File -Path $globalConfig -Force | Out-Null
        Get-ChildItem Env: | Where-Object { $_.Name -like 'GIT_*' } | ForEach-Object {
            $savedGitEnvironment[$_.Name] = $_.Value
            Remove-Item ("Env:" + $_.Name) -ErrorAction Stop
        }
        $env:GIT_CONFIG_NOSYSTEM = '1'; $env:GIT_CONFIG_GLOBAL = $globalConfig
        $head = @(& git -C $WorkspacePath rev-parse HEAD 2>$null); $head = ("$head").Trim()
        $commonDir = @(& git -C $WorkspacePath rev-parse --path-format=absolute --git-common-dir 2>$null); $commonDir = ("$commonDir").Trim()
        $objects = @(& git -C $WorkspacePath rev-parse --path-format=absolute --git-path objects 2>$null); $objects = ("$objects").Trim()
        $expectedObjects = if ([string]::IsNullOrWhiteSpace($commonDir)) { $null } else { [System.IO.Path]::GetFullPath((Join-Path $commonDir 'objects')) }
        if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($head) -or [string]::IsNullOrWhiteSpace($objects) -or
            $null -eq $expectedObjects -or -not [string]::Equals([System.IO.Path]::GetFullPath($objects), $expectedObjects, [System.StringComparison]::OrdinalIgnoreCase) -or
            -not (Test-Path -LiteralPath $objects -PathType Container) -or $RefSpec -notmatch '^HEAD:') {
            throw 'publication transport isolation could not bind the named workspace HEAD/object store.'
        }
        $env:GIT_ALTERNATE_OBJECT_DIRECTORIES = $objects; $env:GIT_TERMINAL_PROMPT = '0'
        & git init --bare --quiet $transportDir 2>$null
        if ($LASTEXITCODE -ne 0) { throw 'publication transport isolation could not create its disposable git directory.' }
        $isolatedRefSpec = $RefSpec -replace '^HEAD:', ($head + ':')

        # Approve failure and an invoker exception both refuse publication.
        $approve = & $GitInvoker -GitArgs @($helperArgs + @('credential', 'approve')) -StdinInput $payload
        if ($approve.ExitCode -ne 0) {
            $notes.Add('credential approve failed; publication refused before any push')
        }
        else {
            # Push from the isolated repository. The validated URL is now consumed
            # under the same empty config source that excludes rewrites and headers.
            $push = & $GitInvoker -GitArgs @($helperArgs + @('-c', 'push.followTags=false', 'push', $DestinationUrl, $isolatedRefSpec)) -WorkingDirectory $transportDir
            $pushExit = $push.ExitCode
            if ($push.ExitCode -ne 0) { $notes.Add('git push exited nonzero'); $verdict = 'push-failed' }
            else { $verdict = 'published' }
        }
    }
    catch {
        $notes.Add('publication invocation threw; publication refused')
        $verdict = 'push-failed'
    }
    finally {
        # Cleanup must run even after a terminating injected/native failure.
        try {
            $reject = & $GitInvoker -GitArgs @($helperArgs + @('credential', 'reject')) -StdinInput $payload
            if ($reject.ExitCode -ne 0) { $cleanupExit = 1 }
        }
        catch { $cleanupExit = 1 }
        try {
            $cacheExit = & $GitInvoker -GitArgs @(@('credential-cache', 'exit'))
            if ($cacheExit.ExitCode -ne 0) { $cleanupExit = 1 }
        }
        catch { $cleanupExit = 1 }
        $payload = $null
        Get-ChildItem Env: | Where-Object { $_.Name -like 'GIT_*' } | ForEach-Object { Remove-Item ("Env:" + $_.Name) -ErrorAction SilentlyContinue }
        foreach ($name in $savedGitEnvironment.Keys) { Set-Item ("Env:" + $name) $savedGitEnvironment[$name] }
        Remove-Item -LiteralPath $transportDir -Recurse -Force -ErrorAction SilentlyContinue
    }
    $originalVerdict = $verdict
    if ($cleanupExit -ne 0) {
        $notes.Add('cleanup incomplete: credential reject/cache daemon exit failed — inspect git credential-cache processes')
        $verdict = 'cleanup-incomplete'
    }
    return [pscustomobject]@{
        Verdict = $verdict; OriginalVerdict = $originalVerdict; PushExit = $pushExit
        CleanupExit = $cleanupExit; Notes = $notes
    }
}

Export-ModuleMember -Function @(
    'Get-SpikePrincipal',
    'Test-SpikePrincipalMatch',
    'Hide-SecretPatterns',
    'Write-SpikeSafe',
    'Assert-SafeSpikeBranch',
    'Assert-SafeRepoTarget',
    'Test-PushUrlAllowed',
    'Get-EffectivePushUrl',
    'Resolve-SpikePrincipalProfile',
    'Assert-ProtectedCredentialTargets',
    'Get-CredentialFileProbe',
    'Get-TokenProviderKind',
    'Resolve-BrokerToken',
    'Get-GhTokenSourceMetadata',
    'Get-GhMetadataVerdict',
    'ConvertFrom-SecureStringInMemory',
    'Invoke-GitProcess',
    'Test-GitCredentialCacheAvailable',
    'Invoke-SpikePublication'
)
Export-ModuleMember -Variable @(
    'EXIT_OK', 'EXIT_PREFLIGHT_FAILED', 'EXIT_PRINCIPAL_REFUSED',
    'EXIT_PROTECTION_FAIL', 'EXIT_PROVIDER_UNSUPPORTED', 'EXIT_INCONCLUSIVE',
    'EXIT_PUBLICATION_FAILED'
)
