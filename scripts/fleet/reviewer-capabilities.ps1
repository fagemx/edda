# GH-702: keep the restricted verbs aligned with reviewer-capabilities.sh.
$ReviewTools = 'Read,Grep,Glob,Bash(git *),Bash(gh *),Bash(edda *),Bash(sh *)'
$ReviewDenied = 'Edit,Write,NotebookEdit,mcp__*'
function Assert-ReviewCapabilities([string]$Transport = 'edda-dispatch') {
    if ($Transport -eq 'edda-dispatch') {
        $helpText = (& edda dispatch --help 2>&1) -join "`n"
        if ($LASTEXITCODE -ne 0) { throw 'review capability check: edda dispatch --help failed; refusing reviewer launch' }
        foreach ($flag in @('--tools', '--exclude-tools')) {
            if ($helpText -notmatch ('(?m)(^|[\s,])' + [regex]::Escape($flag) + '([\s=]|$)')) {
                throw "review capability check: edda dispatch lacks $flag; refusing reviewer launch (upgrade edda)"
            }
        }
    } elseif ($Transport -ne 'claude-stdin') { throw 'review capability check: unknown transport' }
    $helpText = (& claude --help 2>&1) -join "`n"
    if ($LASTEXITCODE -ne 0) { throw 'review capability check: claude --help failed; refusing reviewer launch' }
    foreach ($flag in @('--tools', '--disallowedTools')) {
        if ($helpText -notmatch ('(?m)(^|[\s,])' + [regex]::Escape($flag) + '([\s=]|$)')) {
            throw "review capability check: claude lacks $flag; refusing reviewer launch (upgrade claude)"
        }
    }
}
