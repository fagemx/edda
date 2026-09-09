#!/bin/sh
# GH-1009 — the executor for `fleet.merged-artifact-cleanup`.
#
# The authority already exists and is quoted in `.claude/CLAUDE.md` (Build
# lanes, Verification cost): an artifact whose PR is MERGED — the merged PR's
# remote branch and its lane worktree — may be reclaimed, because the squash
# commit is on `main` and GitHub keeps `refs/pull/N/head`, so SHA-pinned
# verdicts stay resolvable. Nothing executed it, so 57 worktrees and 345
# branches accumulated (2026-09-06 measurement) and every triage round paid to
# re-derive each branch's PR state by hand.
#
# R7 is the other half and is never crossed here: closed-unmerged, open, dirty,
# ambiguous and no-PR items are LISTED and never touched. The default mode is
# a dry run; `--apply` removes only the rows this script already printed as
# RECLAIM. Every check that errors demotes its item to KEEP — an item whose
# state could not be established is not a reclamation candidate.
#
# TRANSITIONAL CARRIER. `mechanism.shell-role=one-line-adapter-only` (ratified)
# marks a new shell program with control flow as migration debt, and this is
# one. It is accepted for this ticket as an explicitly-transitional carrier on
# the condition it stays a thin loop: every per-item fact below comes from
# `git` or `gh` directly, and the only processing applied to their output is
# field extraction. The eventual product home is an `edda fleet reclaim` verb,
# where the classification becomes typed and testable in Rust; this file is
# expected to shrink to the one line that calls it. Do not grow judgement here
# — take it to the verb.
#
# usage:
#   sh scripts/fleet/reclaim-merged.sh [--apply] [--protect <name>]...
#                                      [--pr-limit <n>]
#
# `--protect <name>` matches a worktree directory's basename or a branch name
# exactly, and may be repeated; a protected item is always KEEP. The main
# checkout, anything nested inside it (agent worktrees under
# `.claude/worktrees/`), the worktree this script runs from, and locked
# worktrees are protected unconditionally and need no flag.
#
# Exit codes: 0 = ran, 2 = usage, 3 = the PR table could not be read (without
# it every item's PR state is unknown, so nothing may be reclaimed).
set -eu

usage() {
    echo "usage: $0 [--apply] [--protect <name>]... [--pr-limit <n>]" >&2
}

apply=0
pr_limit=2000
protect=''
while [ $# -gt 0 ]; do
    case $1 in
        --apply) apply=1; shift ;;
        --protect) [ $# -ge 2 ] || { usage; exit 2; }; protect="$protect
$2"; shift 2 ;;
        --pr-limit) [ $# -ge 2 ] || { usage; exit 2; }; pr_limit=$2; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) usage; exit 2 ;;
    esac
done
case $pr_limit in
    '' | *[!0-9]*) usage; exit 2 ;;
esac

cd "$(git rev-parse --show-toplevel)"

work=$(mktemp -d "${TMPDIR:-/tmp}/reclaim-merged.XXXXXX")
trap 'rm -rf "$work"' 0
trap 'rm -rf "$work"; exit 130' HUP INT TERM

TAB=$(printf '\t')

# ── the PR table ─────────────────────────────────────────────────────
#
# One `gh` call for the whole repository rather than one per branch: with ~90
# local branches the per-branch form spends a minute and a half on round trips
# to learn what a single paginated list already says. `--jq` does the field
# extraction, so nothing downstream parses JSON.
#
# `headRefOid` is carried because a PR's state alone does not license deleting
# a ref: the branch must still point at the commit the PR was merged from. A
# branch that has moved on carries work no PR ever saw.
if ! gh pr list --state all --limit "$pr_limit" \
        --json number,state,headRefName,headRefOid,mergeCommit \
        --jq '.[] | [.headRefName, (.number|tostring), .state, .headRefOid, (.mergeCommit.oid // "-")] | @tsv' \
        >"$work/prs.tsv" 2>"$work/gh.err"; then
    echo "reclaim-merged: gh pr list failed — $(head -n 1 "$work/gh.err")" >&2
    exit 3
fi

# The remote side of the same question, in one call. A missing or unreachable
# `origin` leaves the table empty, which reads downstream as "no remote branch
# to reclaim" — the conservative answer.
git ls-remote --heads origin 2>/dev/null \
    | sed "s|${TAB}refs/heads/|${TAB}|" >"$work/remote.tsv" || true

# `pr_row <branch>` prints the single PR row for a branch, or nothing at all.
# Printing nothing for two rows is deliberate: a branch name reused across PRs
# has no single state, and a caller that saw the first row would act on the
# wrong one. `pr_count` below tells the two empty answers apart.
pr_row() {
    awk -F"$TAB" -v b="$1" '$1 == b { rows[++n] = $0 } END { if (n == 1) print rows[1] }' \
        "$work/prs.tsv"
}

pr_count() {
    awk -F"$TAB" -v b="$1" '$1 == b { n++ } END { print n + 0 }' "$work/prs.tsv"
}

is_protected() {
    printf '%s\n' "$protect" | grep -qxF -- "$1"
}

is_nested() {
    case $1 in "$2"/*) return 0 ;; *) return 1 ;; esac
}

# ── snapshot ─────────────────────────────────────────────────────────
count_worktrees() { git worktree list | wc -l | tr -d ' '; }
count_branches() { git for-each-ref --format='x' refs/heads | wc -l | tr -d ' '; }

before_wt=$(count_worktrees)
before_br=$(count_branches)
printf 'reclaim-merged: before  worktrees=%s  branches=%s  prs=%s\n' \
    "$before_wt" "$before_br" "$(wc -l <"$work/prs.tsv" | tr -d ' ')"
[ "$apply" -eq 1 ] || printf 'reclaim-merged: dry run — nothing is removed without --apply\n'
printf '\n'

# ── worktree pass ────────────────────────────────────────────────────
#
# `--porcelain` is the only stable shape: the human `git worktree list` packs
# path, sha and branch into one padded line, and a path containing spaces
# makes that unsplittable. The awk below turns each record into one TSV row
# and extracts nothing else.
git worktree list --porcelain >"$work/wt.raw"
awk -v OFS="$TAB" '
    function emit() { if (p != "") print p, (b == "" ? "-" : b), (h == "" ? "-" : h), lk, pn }
    /^worktree /  { emit(); p = substr($0, 10); b = ""; h = ""; lk = 0; pn = 0; next }
    /^HEAD /      { h = substr($0, 6); next }
    /^branch /    { b = substr($0, 8); sub(/^refs\/heads\//, "", b); next }
    /^locked/     { lk = 1; next }
    /^prunable/   { pn = 1; next }
    END           { emit() }
' "$work/wt.raw" >"$work/wt.tsv"

main_path=$(head -n 1 "$work/wt.tsv" | cut -f1)
self_path=$(git rev-parse --show-toplevel)

printf 'VERDICT\tKIND\tITEM\tBRANCH\tPR\tSTATE\tTREE/SHA\tREASON\n'

: >"$work/wt.reclaim"
while IFS="$TAB" read -r path branch head locked prunable; do
    [ -n "$path" ] || continue
    base=${path##*/}
    row=$(pr_row "$branch")
    pr='-'; state='-'; merge_oid='-'
    if [ -n "$row" ]; then
        pr='#'$(printf '%s' "$row" | cut -f2)
        state=$(printf '%s' "$row" | cut -f3)
        merge_oid=$(printf '%s' "$row" | cut -f5)
    fi

    # The tree state is read before any verdict so the printed row always
    # reports it, even for items excluded for another reason first.
    tree='clean'
    if [ ! -d "$path" ]; then
        tree='missing'
    elif ! git -C "$path" status --porcelain >"$work/status" 2>/dev/null; then
        tree='error'
    elif [ -s "$work/status" ]; then
        tree='dirty'
    fi

    # Order matters: the unconditional protections come first, so a protected
    # item is never even evaluated against its PR state.
    verdict='KEEP'
    if [ "$path" = "$main_path" ]; then
        reason='main-checkout'
    elif is_nested "$path" "$main_path"; then
        reason='nested-in-main-checkout'
    elif [ "$path" = "$self_path" ]; then
        reason='running-from-here'
    elif [ "$locked" = 1 ]; then
        reason='locked'
    elif is_protected "$base" || { [ "$branch" != '-' ] && is_protected "$branch"; }; then
        reason='protected'
    elif [ "$prunable" = 1 ] || [ "$tree" = 'missing' ]; then
        reason='prunable — run git worktree prune'
    elif [ "$branch" = '-' ]; then
        reason='detached — no branch, no PR to judge by'
    elif [ -z "$row" ]; then
        if [ "$(pr_count "$branch")" -gt 0 ]; then
            reason='pr-ambiguous — branch name reused across PRs'
        else
            reason='no-pr'
        fi
    elif [ "$state" != 'MERGED' ]; then
        reason="pr-$state"
    elif [ "$tree" != 'clean' ]; then
        reason="tree-$tree"
    else
        verdict='RECLAIM'
        reason='pr-merged, tree clean'
        printf '%s\t%s\t%s\t%s\n' "$path" "$branch" "$pr" "$merge_oid" >>"$work/wt.reclaim"
    fi
    printf '%s\tworktree\t%s\t%s\t%s\t%s\t%s\t%s\n' \
        "$verdict" "$path" "$branch" "$pr" "$state" "$tree" "$reason"
done <"$work/wt.tsv"

if [ "$apply" -eq 1 ] && [ -s "$work/wt.reclaim" ]; then
    printf '\n'
    while IFS="$TAB" read -r path branch pr merge_oid; do
        # Never `--force`, and never `rm -rf`: a refusal means git sees state
        # this script's own checks missed, and the correct response to that is
        # to leave the worktree alone and say so.
        if git worktree remove "$path" 2>"$work/rm.err"; then
            printf 'reclaimed worktree\t%s\tbranch=%s\tpr=%s\tsquash=%s\n' \
                "$path" "$branch" "$pr" "$merge_oid"
        else
            printf 'KEPT worktree\t%s\tgit worktree remove refused: %s\n' \
                "$path" "$(head -n 1 "$work/rm.err")" >&2
        fi
    done <"$work/wt.reclaim"
fi

# ── branch pass ──────────────────────────────────────────────────────
#
# After the worktree pass, because a branch checked out in a worktree cannot
# be deleted until that worktree is gone — and under --apply some of them just
# went. The list is therefore re-read rather than reused.
printf '\n'
git worktree list --porcelain | sed -n 's|^branch refs/heads/||p' >"$work/checkedout"
default=$(git symbolic-ref --quiet --short refs/remotes/origin/HEAD 2>/dev/null | sed 's|^origin/||')
default=${default:-main}

: >"$work/br.reclaim"
: >"$work/remote.reclaim"
git for-each-ref --format="%(refname:short)${TAB}%(objectname)" refs/heads >"$work/branches.tsv"
while IFS="$TAB" read -r branch tip; do
    [ -n "$branch" ] || continue
    row=$(pr_row "$branch")
    pr='-'; state='-'; head_oid='-'; merge_oid='-'
    if [ -n "$row" ]; then
        pr='#'$(printf '%s' "$row" | cut -f2)
        state=$(printf '%s' "$row" | cut -f3)
        head_oid=$(printf '%s' "$row" | cut -f4)
        merge_oid=$(printf '%s' "$row" | cut -f5)
    fi
    remote_sha=$(awk -F"$TAB" -v b="$branch" '$2 == b { print $1 }' "$work/remote.tsv")

    verdict='KEEP'
    if [ "$branch" = "$default" ]; then
        reason='default-branch'
    elif grep -qxF "$branch" "$work/checkedout"; then
        reason='checked-out'
    elif is_protected "$branch"; then
        reason='protected'
    elif [ -z "$row" ]; then
        if [ "$(pr_count "$branch")" -gt 0 ]; then
            reason='pr-ambiguous — branch name reused across PRs'
        else
            reason='no-pr'
        fi
    elif [ "$state" != 'MERGED' ]; then
        reason="pr-$state"
    elif [ "$tip" != "$head_oid" ]; then
        # The PR is merged but the local ref has moved: whatever is on it now
        # was never in that PR, so `refs/pull/N/head` does not preserve it.
        reason='local-ahead-of-pr'
    else
        verdict='RECLAIM'
        reason='pr-merged, tip = merged head'
        printf '%s\t%s\t%s\n' "$branch" "$pr" "$merge_oid" >>"$work/br.reclaim"
    fi
    printf '%s\tlocal-branch\t%s\t%s\t%s\t%s\t%s\t%s\n' \
        "$verdict" "$branch" "$branch" "$pr" "$state" "$tip" "$reason"

    [ -n "$remote_sha" ] || continue
    rverdict='KEEP'
    if is_protected "$branch"; then
        rreason='protected'
    elif [ -z "$row" ]; then
        rreason='no-pr'
    elif [ "$state" != 'MERGED' ]; then
        rreason="pr-$state"
    elif [ "$remote_sha" != "$head_oid" ]; then
        rreason='remote-moved-since-merge'
    else
        rverdict='RECLAIM'
        rreason='pr-merged, remote tip = merged head'
        printf '%s\t%s\t%s\n' "$branch" "$pr" "$merge_oid" >>"$work/remote.reclaim"
    fi
    printf '%s\tremote-branch\torigin/%s\t%s\t%s\t%s\t%s\t%s\n' \
        "$rverdict" "$branch" "$branch" "$pr" "$state" "$remote_sha" "$rreason"
done <"$work/branches.tsv"

if [ "$apply" -eq 1 ]; then
    printf '\n'
    while IFS="$TAB" read -r branch pr merge_oid; do
        if git branch -D "$branch" >/dev/null 2>"$work/rm.err"; then
            printf 'reclaimed local branch\t%s\tpr=%s\tsquash=%s\n' \
                "$branch" "$pr" "$merge_oid"
        else
            printf 'KEPT local branch\t%s\tgit branch -D refused: %s\n' \
                "$branch" "$(head -n 1 "$work/rm.err")" >&2
        fi
    done <"$work/br.reclaim"
    while IFS="$TAB" read -r branch pr merge_oid; do
        if git push origin --delete "$branch" >/dev/null 2>"$work/rm.err"; then
            printf 'reclaimed remote branch\torigin/%s\tpr=%s\tsquash=%s\n' \
                "$branch" "$pr" "$merge_oid"
        else
            printf 'KEPT remote branch\torigin/%s\tpush --delete refused: %s\n' \
                "$branch" "$(head -n 1 "$work/rm.err")" >&2
        fi
    done <"$work/remote.reclaim"
fi

printf '\n'
if [ "$apply" -eq 1 ]; then
    printf 'reclaim-merged: after   worktrees=%s  branches=%s (before: %s / %s)\n' \
        "$(count_worktrees)" "$(count_branches)" "$before_wt" "$before_br"
else
    printf 'reclaim-merged: dry run — %s worktree(s), %s local branch(es), %s remote branch(es) would be reclaimed by --apply\n' \
        "$(wc -l <"$work/wt.reclaim" | tr -d ' ')" \
        "$(wc -l <"$work/br.reclaim" | tr -d ' ')" \
        "$(wc -l <"$work/remote.reclaim" | tr -d ' ')"
fi
exit 0
