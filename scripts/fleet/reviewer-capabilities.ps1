# GH-702: keep builtin inventory and plan-mode write barrier aligned with
# reviewer-capabilities.sh. Bash(...) permission patterns are not --tools
# values on Claude Code 2.1.259. Trailing class accepts ',' (GH-893): the
# help text prints comma-separated aliases.
$ReviewTools = 'Read,Grep,Glob,Bash'
$ReviewDenied = 'Edit,Write,NotebookEdit,mcp__*'
$ReviewPermissionMode = 'plan'
# pi gets an allowlist, not an exclude list: pi on Windows exposes a separate
# powershell tool an exclude list would miss (review.execution-policy).
$PiReviewTools = 'read,grep,find,ls'
function Assert-ReviewCapabilities([string]$Transport = 'edda-dispatch') {
    if ($Transport -eq 'edda-dispatch') {
        $helpText = (& edda dispatch --help 2>&1) -join "`n"
        if ($LASTEXITCODE -ne 0) { throw 'review capability check: edda dispatch --help failed; refusing reviewer launch' }
        foreach ($flag in @('--tools', '--exclude-tools', '--permission-mode')) {
            if ($helpText -notmatch ('(?m)(^|[\s,])' + [regex]::Escape($flag) + '([\s=,]|$)')) {
                throw "review capability check: edda dispatch lacks $flag; refusing reviewer launch (upgrade edda)"
            }
        }
    } elseif ($Transport -eq 'pi-dispatch') {
        $helpText = (& edda dispatch --help 2>&1) -join "`n"
        if ($LASTEXITCODE -ne 0) { throw 'review capability check: edda dispatch --help failed; refusing reviewer launch' }
        foreach ($flag in @('--model', '--tools')) {
            if ($helpText -notmatch ('(?m)(^|[\s,])' + [regex]::Escape($flag) + '([\s=]|$)')) {
                throw "review capability check: edda dispatch lacks $flag; refusing reviewer launch (upgrade edda)"
            }
        }
        $helpText = (& pi --help 2>&1) -join "`n"
        if ($LASTEXITCODE -ne 0) { throw 'review capability check: pi --help failed; refusing reviewer launch' }
        foreach ($flag in @('--tools', '--session-id', '--model')) {
            if ($helpText -notmatch ('(?m)(^|[\s,])' + [regex]::Escape($flag) + '([\s,=]|$)')) {
                throw "review capability check: pi lacks $flag; refusing reviewer launch (upgrade pi)"
            }
        }
        return
    } elseif ($Transport -ne 'claude-stdin') { throw 'review capability check: unknown transport' }
    $helpText = (& claude --help 2>&1) -join "`n"
    if ($LASTEXITCODE -ne 0) { throw 'review capability check: claude --help failed; refusing reviewer launch' }
    foreach ($flag in @('--tools', '--disallowedTools', '--permission-mode')) {
        if ($helpText -notmatch ('(?m)(^|[\s,])' + [regex]::Escape($flag) + '([\s=,]|$)')) {
            throw "review capability check: claude lacks $flag; refusing reviewer launch (upgrade claude)"
        }
    }
}
