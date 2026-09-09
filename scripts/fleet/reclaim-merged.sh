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
# `git` or `gh` directly, the only processing applied to their output is field
# extraction and joining those fields onto one row per item, and the judgement
# is the plain `if` ladder you can read in one screen. The eventual product
# home is an `edda fleet reclaim` verb, where that ladder becomes typed and
# unit-testable in Rust; this file is expected to shrink to the one line that
# calls it. Do not grow judgement here — take it to the verb.
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
# it every item's PR state is unknown, so nothing may be reclaimed), 4 = a
# post-delete verification re-read failed — the affected refs are reported
# KEPT/unverified on stderr, not receipted as reclaimed.
set -eu

usage() {
    echo "usage: $0 [--apply] [--protect <name>]... [--pr-limit <n>]" >&2
}

apply=0
pr_limit=2000
protect=''
verify_failed=0
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

# Pure shell on purpose. The obvious `printf '%s\n' "$protect" | grep -qxF`
# costs two processes, and this predicate is asked once per worktree and twice
# per branch — on a workstation where a spawn measures ~2.7s that alone put a
# 46-worktree, 229-branch dry run past forty minutes. `$protect` is a
# newline-separated list, so a newline-delimited substring test IS the exact
# match.
protect_nl="$protect
"
is_protected() {
    case $protect_nl in
        *"
$1
"*) return 0 ;;
    esac
    return 1
}

is_nested() {
    case $1 in "$2"/*) return 0 ;; *) return 1 ;; esac
}

# ── the fact tables ──────────────────────────────────────────────────
#
# Four reads, each answering one question for the whole repository at once,
# and then one join per pass. The per-item form of these — a `gh pr list
# --head` and a `git rev-parse` inside the loop — costs one process per item
# per question; on a workstation where a process spawn measures ~2.7s that is
# twenty minutes of round trips to learn what four calls already say.
#
# `headRefOid` is carried because a PR's state alone does not license deleting
# a ref: the ref must still point at the commit the PR was merged from. A ref
# that has moved on carries work no PR ever saw, and `refs/pull/N/head` does
# not preserve it.
if ! gh pr list --state all --limit "$pr_limit" \
        --json number,state,headRefName,headRefOid,mergeCommit \
        --jq '.[] | [.headRefName, (.number|tostring), .state, .headRefOid, (.mergeCommit.oid // "-")] | @tsv' \
        >"$work/prs.tsv" 2>"$work/gh.err"; then
    echo "reclaim-merged: gh pr list failed — $(head -n 1 "$work/gh.err")" >&2
    exit 3
fi

# A missing or unreachable `origin` leaves this table empty, which reads
# downstream as "no remote branch to reclaim" — the conservative answer.
git ls-remote --heads origin 2>/dev/null \
    | sed "s|${TAB}refs/heads/|${TAB}|" >"$work/remote.tsv" || true

git worktree list --porcelain >"$work/wt.raw"

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
# makes that unsplittable. The first awk turns each record into one TSV row;
# the second joins the PR table onto it by head branch. A branch that carries
# two PR rows is left with a count and no fields — a name reused across PRs
# has no single state, and a reader that took the first row would act on the
# wrong one.
awk -v OFS="$TAB" '
    function emit() { if (p != "") print p, (b == "" ? "-" : b), (h == "" ? "-" : h), lk, pn }
    /^worktree /  { emit(); p = substr($0, 10); b = ""; h = ""; lk = 0; pn = 0; next }
    /^HEAD /      { h = substr($0, 6); next }
    /^branch /    { b = substr($0, 8); sub(/^refs\/heads\//, "", b); next }
    /^locked/     { lk = 1; next }
    /^prunable/   { pn = 1; next }
    END           { emit() }
' "$work/wt.raw" >"$work/wt.base"

join_prs() { # <file-of-rows> <1-based field holding the branch name>
    awk -F"$TAB" -v OFS="$TAB" -v prs="$work/prs.tsv" -v key="$2" '
        FILENAME == prs {
            c[$1]++
            if (c[$1] == 1) { num[$1] = $2; st[$1] = $3; ho[$1] = $4; mo[$1] = $5 }
            next
        }
        {
            b = $key
            one = (c[b] == 1)
            print $0, c[b] + 0, (one ? num[b] : "-"), (one ? st[b] : "-"), \
                  (one ? ho[b] : "-"), (one ? mo[b] : "-")
        }
    ' "$work/prs.tsv" "$1"
}

join_prs "$work/wt.base" 2 >"$work/wt.tsv"

main_path=$(head -n 1 "$work/wt.base" | cut -f1)
self_path=$(git rev-parse --show-toplevel)

printf 'VERDICT\tKIND\tITEM\tBRANCH\tPR\tSTATE\tTREE/SHA\tREASON\n'

: >"$work/wt.reclaim"
while IFS="$TAB" read -r path branch head locked prunable prcount pr state head_oid merge_oid; do
    [ -n "$path" ] || continue
    [ "$pr" = '-' ] || pr="#$pr"

    # The tree state is read before any verdict so the printed row always
    # reports it, which is what makes the dry run auditable rather than just
    # a list of conclusions.
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
    elif is_protected "${path##*/}" || { [ "$branch" != '-' ] && is_protected "$branch"; }; then
        reason='protected'
    elif [ "$prunable" = 1 ] || [ "$tree" = 'missing' ]; then
        reason='prunable — run git worktree prune'
    elif [ "$branch" = '-' ]; then
        reason='detached — no branch, no PR to judge by'
    elif [ "$prcount" -gt 1 ]; then
        reason='pr-ambiguous — branch name reused across PRs'
    elif [ "$prcount" -eq 0 ]; then
        reason='no-pr'
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
# went, so the checked-out set is re-read rather than reused.
#
# The row set is the UNION of local and remote names: a merged PR's remote
# branch left behind after its local ref was already deleted is half of what
# the authority names, and iterating local refs alone never sees it.
#
# The checked-out set subtracts the worktrees the pass above just reclaimed.
# Without that subtraction a dry run reports every reclaimable branch as
# `checked-out` — true at the instant it looks, false by the time `--apply`
# reaches the branch pass, and a dry run that does not predict `--apply` is
# the one thing this script cannot be.
printf '\n'
git worktree list --porcelain | sed -n 's|^branch refs/heads/||p' >"$work/checkedout.all"
awk -v gone="$work/wt.reclaim" -F"$TAB" '
    FILENAME == gone { g[$2] = 1; next }
    !($0 in g)       { print }
' "$work/wt.reclaim" "$work/checkedout.all" >"$work/checkedout"
git for-each-ref --format="%(refname:short)${TAB}%(objectname)" refs/heads >"$work/local.tsv"
default=$(git symbolic-ref --quiet --short refs/remotes/origin/HEAD 2>/dev/null | sed 's|^origin/||')
default=${default:-main}

awk -F"$TAB" -v OFS="$TAB" \
    -v loc="$work/local.tsv" -v rem="$work/remote.tsv" -v co="$work/checkedout" '
    FILENAME == loc { tip[$1] = $2; seen[$1] = 1; next }
    FILENAME == rem { rsha[$2] = $1; seen[$2] = 1; next }
    FILENAME == co  { out[$1] = 1; next }
    END {
        for (b in seen)
            print b, (b in tip ? tip[b] : "-"), (b in rsha ? rsha[b] : "-"), (out[b] ? 1 : 0)
    }
' "$work/local.tsv" "$work/remote.tsv" "$work/checkedout" | sort >"$work/br.base"
join_prs "$work/br.base" 1 >"$work/branches.tsv"

: >"$work/br.reclaim"
: >"$work/remote.reclaim"
while IFS="$TAB" read -r branch tip remote_sha checkedout prcount pr state head_oid merge_oid; do
    [ -n "$branch" ] || continue
    [ "$pr" = '-' ] || pr="#$pr"

    verdict=''
    reason=''
    if [ "$tip" != '-' ]; then
        verdict='KEEP'
        if [ "$branch" = "$default" ]; then
            reason='default-branch'
        elif [ "$checkedout" = 1 ]; then
            reason='checked-out'
        elif is_protected "$branch"; then
            reason='protected'
        elif [ "$prcount" -gt 1 ]; then
            reason='pr-ambiguous — branch name reused across PRs'
        elif [ "$prcount" -eq 0 ]; then
            reason='no-pr'
        elif [ "$state" != 'MERGED' ]; then
            reason="pr-$state"
        elif [ "$tip" != "$head_oid" ]; then
            reason='local-ahead-of-pr'
        else
            verdict='RECLAIM'
            reason='pr-merged, tip = merged head'
            printf '%s\t%s\t%s\n' "$branch" "$pr" "$merge_oid" >>"$work/br.reclaim"
        fi
        printf '%s\tlocal-branch\t%s\t%s\t%s\t%s\t%s\t%s\n' \
            "$verdict" "$branch" "$branch" "$pr" "$state" "$tip" "$reason"
    fi

    [ "$remote_sha" != '-' ] || continue
    rverdict='KEEP'
    if [ "$branch" = "$default" ]; then
        rreason='default-branch'
    elif is_protected "$branch"; then
        rreason='protected'
    elif [ "$prcount" -gt 1 ]; then
        rreason='pr-ambiguous — branch name reused across PRs'
    elif [ "$prcount" -eq 0 ]; then
        rreason='no-pr'
    elif [ "$state" != 'MERGED' ]; then
        rreason="pr-$state"
    elif [ "$remote_sha" != "$head_oid" ]; then
        rreason='remote-moved-since-merge'
    elif [ -n "$verdict" ] && [ "$verdict" != 'RECLAIM' ]; then
        # The local ref survived for some reason — a dirty lane, a ref ahead
        # of its PR, an explicit protection. Whatever that reason was, it is
        # also a reason to leave the operator somewhere to push it.
        rreason="local-kept — $reason"
    else
        rverdict='RECLAIM'
        rreason='pr-merged, remote tip = merged head'
        printf '%s\t%s\t%s\n' "$branch" "$pr" "$merge_oid" >>"$work/remote.reclaim"
    fi
    printf '%s\tremote-branch\torigin/%s\t%s\t%s\t%s\t%s\t%s\n' \
        "$rverdict" "$branch" "$branch" "$pr" "$state" "$remote_sha" "$rreason"
done <"$work/branches.tsv"

# ── deletion ─────────────────────────────────────────────────────────
#
# Both ref deletions are BATCHED and then VERIFIED, rather than run one ref
# per command and trusted by exit code. `git push origin --delete` accepts
# many refs in one push, and one ref per push measured ~30s of round trip on
# this workstation — 115 merged remote branches is most of two hours that way,
# against one push batched. `git branch -D` batches for the same reason at
# smaller stakes.
#
# The verification is what makes the batch honest: a partially applied batch
# still deletes the refs it could, so the receipt cannot come from the exit
# code. Each list is re-read from git afterwards and a receipt is printed only
# for a ref that is actually gone; anything still standing gets a KEPT line
# naming it. Chunked at 50 so no single command line grows unbounded.
#
# The re-read itself is verified too, not trusted by exit code either: an
# unreachable origin here would otherwise read as "nothing survived the
# delete" and receipt every ref in the batch as reclaimed — the exact
# fail-open this script exists to close. `git ls-remote`'s own exit status is
# checked directly (never the exit status of a pipeline it feeds), and on
# failure the whole batch is reported KEPT/unverified with no receipt for any
# ref in it; the run's exit code (4) reflects the failure instead of masking
# it as a normal 0.
delete_batched() { # <reclaim-file> <"local"|"remote">
    # Read both arguments out before the first `set --`: inside a function the
    # positional parameters ARE the arguments, so building a batch in them
    # destroys $1 and $2.
    list=$1
    kind=$2
    [ -s "$list" ] || return 0
    set --
    n=0
    while IFS="$TAB" read -r branch pr merge_oid; do
        set -- "$@" "$branch"
        n=$((n + 1))
        if [ "$n" -ge 50 ]; then
            if [ "$kind" = local ]; then
                git branch -D "$@" >/dev/null 2>>"$work/del.err" || true
            else
                git push origin --delete "$@" >/dev/null 2>>"$work/del.err" || true
            fi
            set --
            n=0
        fi
    done <"$list"
    if [ "$n" -gt 0 ]; then
        if [ "$kind" = local ]; then
            git branch -D "$@" >/dev/null 2>>"$work/del.err" || true
        else
            git push origin --delete "$@" >/dev/null 2>>"$work/del.err" || true
        fi
    fi

    if [ "$kind" = local ]; then
        git for-each-ref --format='%(refname:short)' refs/heads >"$work/after.txt"
        label='local branch'
        prefix=''
    else
        if git ls-remote --heads origin >"$work/remote-after.raw" 2>"$work/lsremote.err"; then
            sed "s|^.*${TAB}refs/heads/||" "$work/remote-after.raw" >"$work/after.txt"
        else
            # The re-read failed — network drop, token expiry, a 5xx, anything
            # between the push above and this line. An empty after.txt here is
            # indistinguishable from "origin now has no branches", and every
            # ref in $list would read as gone. Do not let that manufacture a
            # receipt: report every ref in this batch KEPT/unverified instead,
            # on stderr, and fail the run's exit code rather than exit 0 on an
            # unverified batch.
            verify_failed=1
            while IFS="$TAB" read -r branch pr merge_oid; do
                printf 'KEPT remote branch\torigin/%s\tunverified — git ls-remote failed re-reading origin after delete: %s\n' \
                    "$branch" "$(head -n 1 "$work/lsremote.err")" >&2
            done <"$list"
            return 0
        fi
        label='remote branch'
        prefix='origin/'
    fi
    awk -F"$TAB" -v OFS="$TAB" -v after="$work/after.txt" -v label="$label" \
        -v prefix="$prefix" -v kept="$work/kept.txt" '
        FILENAME == after { still[$1] = 1; next }
        {
            if ($1 in still)
                print "KEPT " label, prefix $1, "still present after delete" >kept
            else
                print "reclaimed " label, prefix $1, "pr=" $2, "squash=" $3
        }
    ' "$work/after.txt" "$list"
    if [ -s "$work/kept.txt" ]; then
        cat "$work/kept.txt" >&2
        : >"$work/kept.txt"
    fi
}

if [ "$apply" -eq 1 ]; then
    printf '\n'
    : >"$work/kept.txt"
    delete_batched "$work/br.reclaim" local
    delete_batched "$work/remote.reclaim" remote
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
if [ "$verify_failed" -eq 1 ]; then
    echo 'reclaim-merged: a post-delete re-read could not be verified — see KEPT/unverified lines above' >&2
    exit 4
fi
exit 0
