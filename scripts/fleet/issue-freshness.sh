#!/bin/sh
# issue-freshness.sh — re-derive an issue's applicability at dispatch time
# (GH-931). Issues are opened faster than anyone re-reads them, so every
# dispatch re-checks the body against today's tree instead of trusting the
# day it was written. Checks, one PASS/FAIL/WARN line each:
#   1. every path-shaped backtick token in the body exists at the pinned
#      origin/main SHA (git cat-file -e); negated mentions ("no `X`") are
#      skipped, unparseable tokens WARN without blocking
#   2. every `edda <verb>` referenced in the body has a working --help
#   3. the body carries a non-empty doneWhen section
#   4. no merged PR already delivers the issue (search GH-<n> and #<n>)
# Any FAIL: label the issue fleet:stale, comment the FAIL list, exit 1.
# All PASS (WARN allowed): exit 0.
#
# usage:
#   sh scripts/fleet/issue-freshness.sh <issue-number>
set -eu

usage() {
    echo "usage: $0 <issue-number>" >&2
    exit 2
}

repo=${EDDA_REPO:-fagemx/edda}
issue=
while [ $# -gt 0 ]; do
    case "$1" in
        -h|--help) usage ;;
        --) shift; break ;;
        -*) usage ;;
        *)
            if [ -z "$issue" ]; then issue=$1; shift
            else usage
            fi ;;
    esac
done
[ -n "$issue" ] || usage
case "$issue" in
    *[!0-9]*|'') usage ;;
esac

cd "$(git rev-parse --show-toplevel)"
sha=$(git rev-parse origin/main)
body=$(gh issue view "$issue" --repo "$repo" --json body --jq .body | tr -d '\r')

# Predicted-surface tokens are proposals for files that may not exist yet,
# never references: the freshness scan reads the rest of the body only.
body=$(printf '%s\n' "$body" | awk '
    /^##[ \t]*Predicted surface[ \t]*$/ { skip = 1; next }
    /^##[ \t]/ { skip = 0 }
    skip { next }
    { print }
')

fail=0
fail_lines=

record_fail() {
    fail=1
    fail_lines="$fail_lines$1
"
}

# 1. path tokens: split on backticks; even records are the tokens, and the
# prose record before each one decides whether the mention is negated. A
# token counts as path-shaped when it contains a slash or ends in .sh/.ps1;
# anything weirder WARNs without blocking.
tmp=$(mktemp "${TMPDIR:-/tmp}/issue-freshness.XXXXXX")
trap 'rm -f "$tmp"' 0 HUP INT TERM
printf '%s\n' "$body" | awk -v RS='`' '
    NR % 2 == 1 { prev = $0; next }
    {
        p = prev
        sub(/[ \t\r\n]+$/, "", p)
        if (p ~ /([Nn]o|[Nn]ot|[Ww]ithout)$/) next
        print
    }
' >"$tmp"
while IFS= read -r tok; do
    [ -n "$tok" ] || continue
    case "$tok" in
        */*)
            # a slash token is only credible when its last segment names a
            # file (carries a dot); `if/else` and friends WARN instead
            case ${tok##*/} in
                *.*)
                    case "$tok" in
                        *[!A-Za-z0-9._/-]*|-*|/|.|..|*/..*|../*)
                            echo "WARN unparsed $tok"
                            continue ;;
                    esac
                    if git cat-file -e "$sha:$tok" 2>/dev/null; then
                        echo "PASS path $tok"
                    else
                        echo "FAIL path $tok missing at $sha"
                        record_fail "FAIL path $tok missing at $sha"
                    fi
                    ;;
                *)
                    echo "WARN unparsed $tok"
                    ;;
            esac
            ;;
        *.*)
            # bare filename: a repo-relative probe cannot see it, so resolve
            # it against tracked paths and probe only on an exact single hit
            case "$tok" in
                *[!A-Za-z0-9._-]*)
                    echo "WARN unparsed $tok"
                    continue ;;
            esac
            resolved=$(git ls-files -- "*$tok" | head -n 1)
            count=$(git ls-files -- "*$tok" | wc -l)
            if [ "$count" -eq 1 ] && git cat-file -e "$sha:$resolved" 2>/dev/null; then
                echo "PASS path $tok (resolved $resolved)"
            else
                echo "WARN bare name $tok does not resolve to exactly one tracked file"
            fi
            ;;
        *) continue ;;
    esac
done <"$tmp"

# 2. edda verbs: every `edda <verb>` mention must survive its own --help.
verbs=$(printf '%s\n' "$body" | grep -oE '`edda [a-z][a-z0-9-]*' | sed 's/^`edda //' | sort -u)
for verb in $verbs; do
    if edda "$verb" --help >/dev/null 2>&1; then
        echo "PASS edda $verb"
    else
        echo "FAIL edda $verb --help exited nonzero"
        record_fail "FAIL edda $verb --help exited nonzero"
    fi
done

# 3. doneWhen: present and non-empty (blank lines do not count as content).
section=$(printf '%s\n' "$body" | awk '
    /^##[ \t]*doneWhen[ \t]*$/ { p = 1; next }
    /^##[ \t]/ { p = 0 }
    p { print }
')
if [ -z "$(printf '%s' "$section" | tr -d '[:space:]')" ]; then
    echo "FAIL doneWhen missing or empty"
    record_fail "FAIL doneWhen missing or empty"
else
    echo "PASS doneWhen"
fi

# 4. delivered: any merged PR whose title/body/branch matches the issue.
delivering() {
    # a merged PR delivers this issue only when GitHub resolved a closing
    # reference to it; mentions in title, body or branch do not count
    gh pr list --repo "$repo" --state merged --search "$1" \
        --json number,closingIssuesReferences \
        --jq ".[] | select(any(.closingIssuesReferences[]?; .number == $issue)) | .number" || :
}
pr1=$(delivering "GH-$issue")
pr2=$(delivering "#$issue")
merged=$(printf '%s\n%s\n' "$pr1" "$pr2" | tr -d '\r' | sort -u | sed '/^$/d')
if [ -n "$merged" ]; then
    spaced=$(printf '%s' "$merged" | tr '\n' ' ')
    echo "FAIL delivered by merged PR(s): $spaced"
    record_fail "FAIL delivered by merged PR(s): $spaced"
else
    echo "PASS not delivered"
fi

if [ "$fail" -eq 1 ]; then
    gh label create fleet:stale --repo "$repo" >/dev/null 2>&1 || :
    gh issue edit "$issue" --repo "$repo" --add-label fleet:stale >/dev/null
    {
        printf 'fleet:stale — freshness gate FAILED at %s:\n' "$sha"
        printf '%s\n' "$fail_lines"
    } | gh issue comment "$issue" --repo "$repo" --body-file - >/dev/null
    exit 1
fi
exit 0
