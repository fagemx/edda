#!/usr/bin/env bash
# File-length ratchet (GH-779). Zero extra dependencies: bash + awk + git.
#
#   --staged  each staged *.rs blob  (git cat-file -p <newsha>) must be
#             <= its ceiling. Called from scripts/githooks/pre-commit.
#   --tree    each tracked *.rs (git ls-files) must be <= its ceiling.
#             Called from the CI Format job.
#
# Default ceiling is 1000. scripts/file-length-ceilings.txt overrides
# per path (`<path> <ceiling>`). Lowering a ceiling is free; raising one
# is an ordinary reviewable diff to the data file, not a hook failure.

set -eu

mode=${1:-}
if [ "$mode" != "--staged" ] && [ "$mode" != "--tree" ]; then
    echo "usage: $0 --staged|--tree" >&2
    exit 2
fi

root=$(git rev-parse --show-toplevel)
cd "$root"

ceilings_file="$root/scripts/file-length-ceilings.txt"
default=1000
zero40=0000000000000000000000000000000000000000

ceiling_for() {
    path=$1
    if [ -f "$ceilings_file" ]; then
        awk -v p="$path" '
            $0 ~ /^[ \t]*#/ { next }
            NF < 2 { next }
            $1 == p { print $2; found = 1; exit }
            END { if (!found) exit 1 }
        ' "$ceilings_file" && return 0
    fi
    printf '%s\n' "$default"
}

violations=$(mktemp)
trap 'rm -f "$violations"' EXIT

check() {
    path=$1
    count=$2
    ceil=$(ceiling_for "$path")
    if [ "$count" -gt "$ceil" ]; then
        printf '%s is %s lines (ceiling %s)\n' "$path" "$count" "$ceil" >>"$violations"
    fi
}

line_count_from_blob() {
    # NR counts records; an empty blob is 0 lines.
    git cat-file -p "$1" | awk 'END { print NR+0 }'
}

if [ "$mode" = "--staged" ]; then
    staged_z=$(mktemp)
    trap 'rm -f "$violations" "$staged_z"' EXIT
    git diff --cached --raw -z --no-renames --diff-filter=ACMR >"$staged_z"
    [ -s "$staged_z" ] || exit 0
    while IFS= read -r -d '' meta; do
        IFS= read -r -d '' f || break
        [ -n "$f" ] || continue
        case "$f" in
            *.rs) ;;
            *) continue ;;
        esac
        rest=${meta#:}
        rest=${rest#* }
        rest=${rest#* }
        rest=${rest#* }
        newsha=${rest%% *}
        [ "$newsha" != "$zero40" ] || continue
        check "$f" "$(line_count_from_blob "$newsha")"
    done <"$staged_z"
else
    while IFS= read -r -d '' f; do
        [ -n "$f" ] || continue
        check "$f" "$(line_count_from_blob ":$f")"
    done < <(git ls-files -z -- '*.rs')
fi

if [ -s "$violations" ]; then
    echo "file-length: these paths exceed their ceiling:" >&2
    while IFS= read -r line; do
        echo "file-length: $line" >&2
    done <"$violations"
    exit 1
fi
exit 0
