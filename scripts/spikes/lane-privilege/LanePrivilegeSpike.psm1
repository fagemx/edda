# LanePrivilegeSpike.psm1 — shared fail-closed helpers for the GH-690 lane privilege spike.
#
# Design invariants (do not weaken):
#   1. Principal comes from WindowsIdentity, never from environment variables —
#      a spoofed USERNAME/USERPROFILE must never let an unrestricted caller pass.
#   2. Protected-credential probes only open and immediately dispose a read handle;
#      they never read, log, or print file content.
#   3. Token values never appear in output, argv, environment, or files. All emitted
#      text passes through Hide-SecretPatterns.
#   4. secret-ref resolution is real or explicitly UNSUPPORTED — never a mock value.
#   5. Push targets only the exact allowlisted spike branch; main is always refused.

Set-StrictMode -Version 3.0

# Well-known exit codes (documented in README.md; keep in sync).
$script:EXIT_OK                 = 0
$script:EXIT_PREFLIGHT_FAILED   = 2
$script:EXIT_PRINCIPAL_REFUSED  = 3
$script:EXIT_PROTECTION_FAIL    = 4   # negative test observed READABLE / token present
$script:EXIT_PROVIDER_UNSUPPORTED = 5
$script:EXIT_INCONCLUSIVE       = 6

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
    param(
        [Parameter(Mandatory)][string]$Repo,
        [Parameter(Mandatory)][string[]]$RepoAllowList
    )
    if ([string]::IsNullOrWhiteSpace($Repo) -or $Repo.Contains('*')) {
        throw "Repo guard: empty or wildcard repo refused."
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

enum CredentialProbeResult {
    AccessDenied    # protection property MET (fail-closed success for the negative test)
    Readable        # protection property NOT met — baseline failure signal
    NotFound        # inconclusive: nothing was proven
    ProbeError      # probe itself failed for an unexpected reason
}

function Get-CredentialFileProbe {
    <#
        .SYNOPSIS
        Attempts to open a read handle on a protected credential file and immediately
        disposes it. NEVER reads, stores, or prints any byte of file content.
        .OUTPUTS
        [CredentialProbeResult]
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
            # Sharing violation or an ACL-mediated IO error is still "denied to us".
            return [CredentialProbeResult]::AccessDenied
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
        The returned SecureString must stay in memory and be consumed by the one
        authorized sink (currently: `gh auth login --with-token` stdin). Callers
        must not persist, log, or pass it through argv/environment.
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
        Runs `gh auth status` and reduces it to identity/token-SOURCE metadata only.
        Token values (even the masked ones gh prints) are dropped before anything
        is returned or printed.
        .OUTPUTS
        PSCustomObject with LoggedIn (bool), Host, Account, TokenPresent (bool),
        TokenSource (string: 'keyring' | 'infile' | 'env' | 'unknown' | 'none').
    #>
    $meta = [pscustomobject]@{
        LoggedIn     = $false
        Host         = $null
        Account      = $null
        TokenPresent = $false
        TokenSource  = 'none'
    }
    if ($null -eq (Get-Command gh -ErrorAction SilentlyContinue)) {
        throw "INCONCLUSIVE: gh CLI not found; token-source metadata cannot be collected."
    }
    $raw = & gh auth status 2>&1
    $meta | Add-Member -NotePropertyName GhExit -NotePropertyValue $LASTEXITCODE
    foreach ($line in @($raw)) {
        $text = "$line"
        # Drop every line that could carry token material before it can escape.
        if ($text -match 'Token:') { 
            if ($text -notmatch 'no token|token: none') { $meta.TokenPresent = $true }
            continue
        }
        if ($text -match 'github\.com') { $meta.Host = 'github.com' }
        if ($text -match 'Logged in to github\.com as ([A-Za-z0-9-]+)') {
            $meta.LoggedIn = $true
            $meta.Account = $Matches[1]
        }
        if ($text -match 'Token: .*\((keyring|infile|env)\)') {
            $meta.TokenSource = $Matches[1]
        }
    }
    return $meta
}

Export-ModuleMember -Function @(
    'Get-SpikePrincipal',
    'Test-SpikePrincipalMatch',
    'Hide-SecretPatterns',
    'Write-SpikeSafe',
    'Assert-SafeSpikeBranch',
    'Assert-SafeRepoTarget',
    'Get-CredentialFileProbe',
    'Get-TokenProviderKind',
    'Resolve-BrokerToken',
    'Get-GhTokenSourceMetadata'
)
Export-ModuleMember -Variable @(
    'EXIT_OK', 'EXIT_PREFLIGHT_FAILED', 'EXIT_PRINCIPAL_REFUSED',
    'EXIT_PROTECTION_FAIL', 'EXIT_PROVIDER_UNSUPPORTED', 'EXIT_INCONCLUSIVE'
)
