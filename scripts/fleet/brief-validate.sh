#!/bin/sh
# brief-validate.sh — dry-run a lane brief's authored steps in a throwaway
# worktree at the brief's pinned base SHA before any lane launch (GH-930).
# The 09-05 flash wave STOPped five lanes on brief defects alone: authors
# reasoned instead of measuring. This closes that hole mechanically.
#
# What it validates: the brief's authored span (between the exact lines
# <<AUTHORED: BEGIN>> and <<AUTHORED: END>>), applied in order inside a
# worktree at the facts block's `base full SHA` —
#   - every concrete `edit({...})` must match exactly once, then mutates the
#     tree the later steps see
#   - a concrete `write({...})` lands its content verbatim; a bare-identifier
#     content (a metavariable) is skipped with a note, never guessed
#   - every step carrying an `output:` expectation is executed and compared:
#     `empty stdout, exit 0`, an all-digits literal, or a text literal (the
#     trailing period is stripped on both sides)
#   - every concretely touched file then passes its per-type syntax gate:
#     `sh -n` for *.sh, a PowerShell parser run for *.ps1, others skipped
# All pass  → prints VALID and a `git diff --cached --stat` summary, writes
#             the full expected diff to <brief>.expected.diff, exit 0
# Any miss  → prints INVALID step=<n> with the verbatim output, exit 1, and
#             writes no expected diff
# Bad usage or a malformed brief → exit 2
#
# usage:
#   sh scripts/fleet/brief-validate.sh <brief-path>
set -eu

usage() {
    echo "usage: $0 <brief-path>" >&2
    exit 2
}

[ $# -eq 1 ] || usage
brief=$1
[ -f "$brief" ] || { echo "brief-validate: no such brief: $brief" >&2; exit 2; }

root=$(git rev-parse --show-toplevel)
base=$(sed -n 's/^base full SHA: \([0-9a-f]\{40\}\).*$/\1/p' "$brief" | head -1)
[ -n "$base" ] || {
    echo "brief-validate: no 'base full SHA: <40-hex>' line in $brief" >&2
    exit 2
}

tmp_span=$(mktemp "${TMPDIR:-/tmp}/brief-validate.span.XXXXXX")
tmp_err=$(mktemp "${TMPDIR:-/tmp}/brief-validate.err.XXXXXX")
tmp_old=$(mktemp "${TMPDIR:-/tmp}/brief-validate.old.XXXXXX")
tmp_new=$(mktemp "${TMPDIR:-/tmp}/brief-validate.new.XXXXXX")
tmp_json=$(mktemp "${TMPDIR:-/tmp}/brief-validate.json.XXXXXX")
tmp_touched=$(mktemp "${TMPDIR:-/tmp}/brief-validate.touched.XXXXXX")
wt=$(mktemp -d "${TMPDIR:-/tmp}/brief-validate.wt.XXXXXX")
cleanup() {
    rm -f "$tmp_span" "$tmp_err" "$tmp_old" "$tmp_new" "$tmp_json" "$tmp_touched"
    git -C "$root" worktree remove --force "$wt" >/dev/null 2>&1 || :
    git -C "$root" worktree prune >/dev/null 2>&1 || :
}
trap cleanup 0 HUP INT TERM

# The authored span: everything between the two exact marker lines. Step and
# expectation lines may carry presentation indent (the rendered brief indents
# output lines). A step header is a dotted step number, then a space — both
# `9.1 write(...)` and `10. git status` shapes.
awk '
    /^<<AUTHORED: BEGIN>>$/ { inspan = 1; next }
    /^<<AUTHORED: END>>$/   { inspan = 0; next }
    inspan && /^[ ]*output:[ ]?/ {
        line = $0
        sub(/^[ ]*output:[ ]?/, "", line)
        print "EXPECT\t" line
        next
    }
    inspan && /^[ ]*[0-9][0-9.]*[ ]/ {
        num = $0
        sub(/^[ ]*/, "", num)
        sub(/[ \t].*$/, "", num)
        line = $0
        sub(/^[ ]*[0-9][0-9.]*[ ][ ]*/, "", line)
        print "STEP\t" num "\t" line
        next
    }
' "$brief" >"$tmp_span"

if [ ! -s "$tmp_span" ]; then
    echo "brief-validate: no <<AUTHORED: BEGIN>>/<<AUTHORED: END>> span with steps in $brief" >&2
    exit 2
fi

git -C "$root" worktree add --detach "$wt" "$base" >/dev/null 2>&1 || {
    echo "brief-validate: cannot create worktree at $base" >&2
    exit 2
}

invalid() {  # invalid <step> <detail...> — verbatim evidence, exit 1, no diff
    printf 'INVALID step=%s\n' "$1"
    shift
    printf '%s\n' "$*"
    exit 1
}

cur_num=
cur_body=
cur_exp=

emit_result() {
    # flush on the next STEP or at the end of the span
    :
}

flush_step() {
    [ -n "$cur_num" ] || return 0
    # a distinct name: the caller's read-loop `body` variable must survive
    st_body=$cur_body
    exp=${cur_exp%.}
    case "$st_body" in
        "write("*)
            json=${st_body#"write("}
            json=${json%)}
            if ! printf '%s' "$json" | jq -e . >/dev/null 2>&1; then
                echo "SKIP step=$cur_num (tool call is not concrete JSON)"
                cur_num=
                return 0
            fi
            path=$(printf '%s' "$json" | jq -r '.path // empty')
            case "$path" in
                ""|/*|*..*|*[!A-Za-z0-9._/-]*)
                    invalid "$cur_num" "write path is not a plain repo-relative path: $path" ;;
            esac
            ctype=$(printf '%s' "$json" | jq -r '.content | type' 2>/dev/null || :)
            if [ "$ctype" != "string" ]; then
                echo "SKIP step=$cur_num (content is a metavariable, not a concrete string)"
                cur_num=
                return 0
            fi
            mkdir -p "$wt/$(dirname "$path")"
            printf '%s' "$json" | jq -j '.content' >"$wt/$path"
            printf '%s\t%s\n' "$cur_num" "$path" >>"$tmp_touched"
            if [ -n "$exp" ]; then
                # the launcher documents the write tool's success line without
                # the byte count; that shape is what a brief must expect
                [ "$exp" = "Successfully wrote bytes to $path" ] ||
                    invalid "$cur_num" "expectation mismatch: want the write tool's success line for $path, brief says: $cur_exp"
            fi
            ;;
        "edit("*)
            json=${st_body#"edit("}
            json=${json%)}
            if ! printf '%s' "$json" | jq -e . >/dev/null 2>&1; then
                invalid "$cur_num" "edit call is not concrete JSON: $st_body"
            fi
            path=$(printf '%s' "$json" | jq -r '.path // empty')
            case "$path" in
                ""|/*|*..*|*[!A-Za-z0-9._/-]*)
                    invalid "$cur_num" "edit path is not a plain repo-relative path: $path" ;;
            esac
            [ -f "$wt/$path" ] || invalid "$cur_num" "edit target does not exist at $base: $path"
            n=$(printf '%s' "$json" | jq '.edits | length')
            i=0
            while [ "$i" -lt "$n" ]; do
                if ! printf '%s' "$json" | jq -e ".edits[$i].oldText | type == \"string\"" >/dev/null 2>&1; then
                    invalid "$cur_num" "edit oldText is not a concrete string"
                fi
                if ! printf '%s' "$json" | jq -e ".edits[$i].newText | type == \"string\"" >/dev/null 2>&1; then
                    invalid "$cur_num" "edit newText is not a concrete string"
                fi
                printf '%s' "$json" | jq -j ".edits[$i].oldText" >"$tmp_old"
                printf '%s' "$json" | jq -j ".edits[$i].newText" >"$tmp_new"
                count=$(jq -Rs --rawfile old "$tmp_old" 'split($old) | length - 1' <"$wt/$path")
                if [ "$count" -ne 1 ]; then
                    invalid "$cur_num" "oldText matches $count times in $path, want exactly 1: $(cat "$tmp_old")"
                fi
                jq -Rs --rawfile old "$tmp_old" --rawfile new "$tmp_new" \
                    'split($old) | join($new)' <"$wt/$path" >"$tmp_json"
                jq -j . "$tmp_json" >"$wt/$path"
                i=$((i + 1))
            done
            printf '%s\t%s\n' "$cur_num" "$path" >>"$tmp_touched"
            if [ -n "$exp" ]; then
                [ "$exp" = "Successfully replaced $n block(s) in $path" ] ||
                    invalid "$cur_num" "expectation mismatch: want the edit tool's success line for $path, brief says: $cur_exp"
            fi
            ;;
        *)
            # a command step: only validated when it carries an expectation
            [ -n "$exp" ] || return 0
            rc=0
            out=$(cd "$wt" && sh -c "$st_body" 2>"$tmp_err") || rc=$?
            if [ "$rc" -ne 0 ]; then
                invalid "$cur_num" "exit=$rc output=$(cat "$tmp_err")$out"
            fi
            case "$exp" in
                "empty stdout"*)
                    [ -z "$out" ] || invalid "$cur_num" "want empty stdout, got: $out" ;;
                *)
                    [ "$out" = "$exp" ] || invalid "$cur_num" "want: $exp
got: $out" ;;
            esac
            ;;
    esac
}

while IFS="$(printf '\t')" read -r kind num body; do
    case "$kind" in
        STEP)
            flush_step
            cur_num=$num
            cur_body=$body
            cur_exp=
            ;;
        EXPECT)
            if [ -n "$cur_exp" ]; then
                cur_exp="$cur_exp
$num"
            else
                cur_exp=$num
            fi
            ;;
    esac
done <"$tmp_span"
flush_step

# per-type syntax gates over every concretely touched file; a file edited by
# several steps reports the step that last touched it
tmp_gates=$(mktemp "${TMPDIR:-/tmp}/brief-validate.gates.XXXXXX")
trap 'rm -f "$tmp_gates"; rm -f "$tmp_span" "$tmp_err" "$tmp_old" "$tmp_new" "$tmp_json" "$tmp_touched"; git -C "$root" worktree remove --force "$wt" >/dev/null 2>&1 || :; git -C "$root" worktree prune >/dev/null 2>&1 || :' 0 HUP INT TERM
awk -F'\t' '{ line[$2] = $1 } END { for (f in line) print line[f] "\t" f }' \
    "$tmp_touched" >"$tmp_gates"
while IFS="$(printf '\t')" read -r st f; do
    case "$f" in
        *.sh)
            if ! sh -n "$wt/$f" 2>"$tmp_err"; then
                invalid "$st" "sh -n $f: $(cat "$tmp_err")"
            fi
            ;;
        *.ps1)
            if ! pwsh -NoProfile -Command "\$t=\$null; \$e=\$null; [void][System.Management.Automation.Language.Parser]::ParseFile('$wt/$f', [ref]\$t, [ref]\$e); if (\$e -and \$e.Count -gt 0) { \$e | ForEach-Object { [Console]::Error.WriteLine(\$_.Message) }; exit 1 }" 2>"$tmp_err"; then
                invalid "$st" "pwsh parse $f: $(cat "$tmp_err")"
            fi
            ;;
        *)
            : ;;
    esac
done <"$tmp_gates"

(cd "$wt" && git add -A && git diff --cached --stat)
echo "VALID"
git -C "$wt" diff --cached >"$brief.expected.diff"
exit 0
