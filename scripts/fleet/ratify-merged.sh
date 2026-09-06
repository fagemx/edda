#!/bin/sh
# GH-764 — post-merge ratification: turn a merged PR into binding decisions.
#
# `decision.auto-ratify` (binding) says the machine ratifies and the operator
# reads exceptions. The evidence form of that rule is this one: a PR that
# implements a decision and passes the gates IS the authority for it, so the
# merge is the moment the decision becomes binding. This script is the whole
# hook — read the PR body's `Decision:` line, then for each key run
#
#     edda ratify <key> --evidence "pr#<N>@<sha>" --note "<PR title>"
#
# and log the exit code. It judges nothing else: `edda` owns "is that a real
# key" (exit 1) and "is that evidence well formed" (exit 2), which is the
# point of putting the typed form in the binary instead of in a script.
#
# A PR with no `Decision:` line ratifies nothing and is not an error — most
# PRs implement no decision.
#
# usage:
#   sh scripts/fleet/ratify-merged.sh <pr-number> [--sha <40-hex>]
#                                     [--body-file <file>] [--title <text>]
#
# `--body-file` / `--title` / `--sha` make the script runnable with no
# network, which is how scripts/fleet/test-ratify-merged.sh exercises it.
# Without them it reads the PR with `gh`.
#
# The merge step that should call this does not exist yet — #769 owns it
# (#762 was closed NOT_PLANNED on 2026-09-06 and its substance folded into
# #769's doneWhen), and `scripts/pr-review-watch.sh` never merges by design.
# There it is one line. Until then this runs by hand after a merge.
#
# Exit codes: 0 = ran (whatever the individual ratifies did), 2 = usage.
# A failing `edda ratify` is logged, never fatal: a hook that aborts a
# post-merge sequence because one key was already binding is worse than one
# that reports it.
set -eu

cd "$(git rev-parse --show-toplevel)"

usage() {
    echo "usage: $0 <pr-number> [--sha <40-hex>] [--body-file <file>] [--title <text>]" >&2
}

pr=""
sha=""
body_file=""
title=""
while [ $# -gt 0 ]; do
    case $1 in
        --sha) [ $# -ge 2 ] || { usage; exit 2; }; sha=$2; shift 2 ;;
        --body-file) [ $# -ge 2 ] || { usage; exit 2; }; body_file=$2; shift 2 ;;
        --title) [ $# -ge 2 ] || { usage; exit 2; }; title=$2; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        -*) usage; exit 2 ;;
        *) [ -z "$pr" ] || { usage; exit 2; }; pr=$1; shift ;;
    esac
done

case $pr in
    '' | *[!0-9]*) usage; exit 2 ;;
esac

work=$(mktemp -d "${TMPDIR:-/tmp}/ratify-merged.XXXXXX")
cleanup() { rm -rf "$work"; }
trap cleanup 0 HUP INT TERM

body=$work/body.md
if [ -n "$body_file" ]; then
    cat "$body_file" >"$body"
else
    gh pr view "$pr" --json body,title,mergeCommit,headRefOid \
        >"$work/pr.json" || { echo "ratify-merged: gh pr view $pr failed" >&2; exit 2; }
    jq -r '.body // ""' <"$work/pr.json" >"$body"
    [ -n "$title" ] || title=$(jq -r '.title // ""' <"$work/pr.json")
    [ -n "$sha" ] || sha=$(jq -r '.mergeCommit.oid // .headRefOid // ""' <"$work/pr.json")
fi

# The SHA is not defaulted or abbreviated: evidence that cannot be resolved
# back to one commit is not evidence, and `edda ratify` rejects it anyway.
case $sha in
    *[!0-9a-fA-F]* | '') echo "ratify-merged: need a full 40-hex --sha for pr#$pr" >&2; exit 2 ;;
esac
[ "${#sha}" -eq 40 ] || { echo "ratify-merged: --sha must be 40 hex chars" >&2; exit 2; }

# `Decision: <key>[, <key>]` — same shape as the existing `Issue: #N` line.
# Only a line that starts with the label counts, so prose quoting the word
# never ratifies anything.
keys=$(sed -n 's/^[Dd]ecision:[[:space:]]*//p' "$body" | tr ',' '\n' \
    | sed 's/^[[:space:]]*//; s/[[:space:]]*$//' | grep -v '^$' || true)

if [ -z "$keys" ]; then
    echo "ratify-merged: pr#$pr has no Decision: line — nothing to ratify"
    exit 0
fi

printf '%s\n' "$keys" | while IFS= read -r key; do
    [ -n "$key" ] || continue
    if [ -n "$title" ]; then
        edda ratify "$key" --evidence "pr#$pr@$sha" --note "$title" || code=$?
    else
        edda ratify "$key" --evidence "pr#$pr@$sha" || code=$?
    fi
    code=${code:-0}
    echo "ratify-merged: $key -> exit $code"
    unset code
done

exit 0
