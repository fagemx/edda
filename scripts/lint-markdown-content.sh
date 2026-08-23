#!/bin/sh
set -eu

cd "$(git rev-parse --show-toplevel)"
files=$(mktemp)
trap 'rm -f "$files"' 0 HUP INT TERM
git ls-files -z --format='./%(path)' -- '*.md' >"$files"

if xargs -0 awk '
function marker(s,    i, c) {
    for (i = 0; i < 3 && substr(s, 1, 1) == " "; i++)
        s = substr(s, 2)
    c = substr(s, 1, 1)
    if (c != "`" && c != "~")
        return 0
    mark_width = 0
    while (substr(s, mark_width + 1, 1) == c)
        mark_width++
    if (mark_width < 3)
        return 0
    mark_char = c
    mark_rest = substr(s, mark_width + 1)
    return 1
}

function container_line(s, track,    indent, base, rest) {
    for (indent = 0; substr(s, indent + 1, 1) == " "; indent++);
    if (!track)
        return indent >= container_indent ? substr(s, container_indent + 1) : s
    if (s !~ /^[ \t]*$/ && indent < container_indent)
        container_indent = 0
    base = container_indent
    rest = substr(s, base + 1)
    if (match(rest, /^ {0,3}([-+*]|[0-9]{1,9}[.)])[ \t]{1,4}/) &&
        substr(rest, RLENGTH + 1, 1) !~ /[ \t]/) {
        container_indent = base + RLENGTH
        return substr(rest, RLENGTH + 1)
    }
    return rest
}

FNR == 1 {
    in_fence = 0
    fence_char = ""
    fence_width = 0
    container_indent = 0
}

{
    line = $0
    sub(/\r$/, "", line)
    fence_line = container_line(line, !in_fence)

    if (in_fence) {
        if (marker(fence_line) && mark_char == fence_char &&
            mark_width >= fence_width && mark_rest ~ /^[ \t]*$/)
            in_fence = 0
        next
    }

    if (marker(fence_line) && !(mark_char == "`" && mark_rest ~ /`/)) {
        in_fence = 1
        fence_char = mark_char
        fence_width = mark_width
        next
    }

    if (line ~ /^running [0-9]+ tests?$/ ||
        line ~ /^test result: (ok|FAILED)\./ ||
        line ~ /^test [A-Za-z0-9_:]+ \.\.\. (ok|FAILED|ignored)$/ ||
        line ~ /^ *(Compiling|Finished|Running|Checking) /) {
        printf "%s:%d: tool output outside a fenced block: %s\n", \
            substr(FILENAME, 3), FNR, line
        failed = 1
    }
}

END { exit failed }
' <"$files"; then
    exit 0
fi

printf '%s\n' \
    'Markdown content lint failed. Put command transcripts inside fenced code blocks.' \
    'Blocked outside fences:' \
    '  ^running [0-9]+ tests?$' \
    '  ^test result: (ok|FAILED)\.' \
    '  ^test [A-Za-z0-9_:]+ \.\.\. (ok|FAILED|ignored)$' \
    '  ^ *(Compiling|Finished|Running|Checking) ' >&2
exit 1
