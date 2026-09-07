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

# `projection()` below leaves two marks on the operator's own checkout: the
# generated mirror in the working tree, and the scratch branch it commits on.
# Undoing both belongs here and not at each call site, because the paths that
# lose the operator their branch are the ones that never reach a call site —
# a `set -e` abort, or an INT during `git push`.
restore_branch=""

# The mirror is generated, so discarding it costs nothing — but it has to go
# all the way down to untracked files. `git checkout --` alone cannot: a
# half-written export stays in the tree, and every later run then stops at the
# clean-tree guard until someone clears it by hand. Path-limited throughout:
# nothing outside the generated mirror is ever swept.
reset_mirror() {
    git reset --quiet -- docs/decisions 2>/dev/null || true
    git checkout --quiet -- docs/decisions 2>/dev/null || true
    git clean --quiet --force -d -- docs/decisions 2>/dev/null || true
}

cleanup() {
    if [ -n "$restore_branch" ]; then
        reset_mirror
        git checkout --quiet "$restore_branch" 2>/dev/null || true
        restore_branch=""
    fi
    rm -rf "$work"
}
trap cleanup 0
# A signal has to stop the script. Left as a plain `trap cleanup INT`, the
# shell resumes at the next command — which would run `gh pr create` on a
# branch cleanup() has already switched away from.
trap 'cleanup; exit 130' HUP INT TERM

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
fi

[ -z "$keys" ] || printf '%s\n' "$keys" | while IFS= read -r key; do
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

# ── decision projection (GH-671) ─────────────────────────────────────
#
# `ledger.cross-machine-projection` (ratified) is quoted, not restated: the
# mirror is committed under `docs/decisions/`, regeneration "happens at wave
# close, not per decision ... always in its own commit chore(ledger): export
# decision projection @ <ts>".
#
# This is the export half of
# `fleet.ledger-sync-trigger=import-on-sessionstart-export-at-wave-close-post-merge`.
# Two constraints that decision measured shape the rest:
#
#   (a) `main` is protected — PR required, `CI Gate` + `Independent Review`
#       required, zero bypass actors — so the projection commit reaches
#       `main` through a PR. This step never commits on the default branch
#       itself: it branches, commits, pushes and opens the PR.
#   (b) INDEX.md's `- **Exported at**:` stamp is rewritten on every export,
#       so the tree is *always* dirty afterwards. The no-op test below
#       compares decision CONTENT with that stamp excluded; comparing the
#       stamp would open a chore commit after every merge, forever.
#
# Fail-soft throughout: every failure reports and returns 0, for the same
# reason a failing `edda ratify` is not fatal. Nothing below restores the
# checkout or the tree itself — `cleanup()` at the top of the file owns that,
# so the operator lands back on their branch whether this returns, aborts, or
# is interrupted.
projection() {
    if [ "${EDDA_PROJECTION:-on}" = off ]; then
        echo "ratify-merged: projection off (EDDA_PROJECTION=off)"
        return 0
    fi
    if ! command -v edda >/dev/null 2>&1; then
        echo "ratify-merged: projection skipped — edda not on PATH" >&2
        return 0
    fi
    default=$(git symbolic-ref --quiet --short refs/remotes/origin/HEAD 2>/dev/null | sed 's|^origin/||')
    default=${default:-main}
    branch=$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo HEAD)
    if [ "$branch" != "$default" ]; then
        echo "ratify-merged: projection skipped — on '$branch', not '$default'"
        return 0
    fi
    # A dirty tree would ride along into the branch switch below.
    if [ -n "$(git status --porcelain)" ]; then
        echo "ratify-merged: projection skipped — working tree not clean"
        return 0
    fi

    # From here the tree is dirty by our own hand and, shortly, the checkout
    # is off $default; arming cleanup() before the export is what makes an
    # interrupted export reversible too.
    restore_branch=$default

    if ! edda export md --out docs/decisions >/dev/null; then
        echo "ratify-merged: projection skipped — edda export md failed" >&2
        return 0
    fi

    # (b): the stamp line is excluded, so an export that moved nothing but
    # the clock counts as unchanged.
    #
    # Drop ONLY the `+++ `/`--- ` file headers. The obvious-looking
    # `grep -v '^[+-][+-]'` also eats every added or removed markdown bullet
    # (`+- **Value**: …`, `-- **Value**: …`), and `render_domain` writes a
    # decision's whole payload as those bullets — so editing a decision inside
    # an already-tracked domain file produced a diff made of nothing else,
    # `changed` came back empty, and the projection reported "unchanged" and
    # never opened a PR. Only brand-new domains got through, via `added`.
    changed=$(git diff -U0 -- docs/decisions | grep '^[+-]' | grep -v '^+++ ' | grep -v '^--- ' | grep -v '^[+-]- \*\*Exported at\*\*:' | head -n 1)
    added=$(git ls-files --others --exclude-standard -- docs/decisions | head -n 1)
    if [ -z "$changed" ] && [ -z "$added" ]; then
        echo "ratify-merged: projection unchanged — no decision content moved"
        return 0
    fi

    ts=$(date -u +%Y-%m-%dT%H:%M:%SZ)
    work_branch="ledger/projection-$(printf '%s' "$ts" | tr ':' '-')"
    if ! git checkout --quiet -b "$work_branch"; then
        echo "ratify-merged: projection skipped — cannot branch" >&2
        return 0
    fi
    # Path-limited: never sweeps anything outside the generated mirror.
    if ! git add -- docs/decisions; then
        echo "ratify-merged: projection skipped — cannot stage the mirror" >&2
        return 0
    fi
    if ! git commit --quiet -m "chore(ledger): export decision projection @ $ts" -- docs/decisions; then
        echo "ratify-merged: projection commit failed" >&2
        return 0
    fi
    if git push --quiet origin "$work_branch" 2>/dev/null; then
        gh pr create --base "$default" --head "$work_branch" --title "chore(ledger): export decision projection @ $ts" --body-file - <<PRBODY || echo "ratify-merged: projection PR not opened — open it from $work_branch" >&2
Generated at wave close by \`scripts/fleet/ratify-merged.sh\`.

Regenerates the committed decision mirror under \`docs/decisions/\` per
\`ledger.cross-machine-projection\`. Every file is generated by
\`edda export md\` — never hand-edit one.
PRBODY
    else
        echo "ratify-merged: projection pushed nothing — branch $work_branch is local" >&2
    fi
    echo "ratify-merged: projection exported @ $ts on $work_branch"
}

projection

exit 0
