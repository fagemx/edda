#!/bin/sh
# next-issue.sh — one command from a fleet:ready issue to a launched lane (GH-886).
# The skill-less controller's pick-and-launch half of the loop: lint the queue,
# refuse a claimed or non-ready issue, render the brief with brief-from-issue.sh
# (GH-885), refuse to launch a brief whose authored middle is still unfilled,
# then task, claim, worktree, and launch through the same guarded scripts the
# Claude line uses.
#
# usage:
#   sh scripts/fleet/next-issue.sh <issue> <machine>/<role> [--dry-run]
#
# --dry-run prints every command it would run and creates, claims and launches
# nothing. Never merges, force-pushes, or touches labels.
set -eu

usage() {
    echo "usage: $0 <issue> <machine>/<role> [--dry-run]" >&2
}

die() {
    echo "next-issue: $1" >&2
    exit 2
}

repo=${EDDA_REPO:-fagemx/edda}
issue=
machine=
dry=0
while [ $# -gt 0 ]; do
    case "$1" in
        --dry-run) dry=1; shift ;;
        -h|--help) usage; exit 0 ;;
        --) shift; break ;;
        -*)
            die "unknown option $1" ;;
        *)
            if [ -z "$issue" ]; then issue=$1; shift
            elif [ -z "$machine" ]; then machine=$1; shift
            else die "unexpected argument $1"
            fi ;;
    esac
done
[ -n "$issue" ] && [ -n "$machine" ] || { usage; exit 2; }
case "$issue" in
    *[!0-9]*|'') die "issue must be a positive integer, got '$issue'" ;;
esac
case "$machine" in
    */*) : ;;
    *) die "machine identity must be <machine>/<role>, got '$machine'" ;;
esac

self_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
root=$(CDPATH= cd -- "$self_dir/../.." && pwd)

echo "== ready-queue lint"
sh "$self_dir/ready-queue-lint.sh" || die "ready-queue-lint failed"

echo "== issue state"
json=$(gh issue view "$issue" --repo "$repo" --json state,title,labels,comments) ||
    die "gh issue view $issue failed"
state=$(printf '%s' "$json" | jq -r .state)
[ "$state" = "OPEN" ] || die "issue $issue is $state, not OPEN"
has_claimed=$(printf '%s' "$json" | jq '[.labels[].name] | index("fleet:claimed") != null')
if [ "$has_claimed" = "true" ]; then
    claimant=$(printf '%s' "$json" | jq -r \
        '[.comments[].body] | map(capture("taking: (?<who>[^ ]+) at").who) | last // "unknown"')
    die "issue $issue is already claimed (fleet:claimed) by ${claimant:-unknown}"
fi
has_ready=$(printf '%s' "$json" | jq '[.labels[].name] | index("fleet:ready") != null')
[ "$has_ready" = "true" ] || die "issue $issue does not carry fleet:ready"
title=$(printf '%s' "$json" | jq -r .title)
[ -n "$title" ] && [ "$title" != "null" ] || die "issue $issue has no title"
# interpolated into the double-quoted task-new line below
title_safe=$(printf '%s' "$title" | tr -d '"`$')

echo "== branch and worktree"
type=$(printf '%s' "$title" | sed -n 's/^\([a-z][a-z0-9]*\)([^)]*)[:].*/\1/p')
[ -n "$type" ] || type=feat
slug=$(printf '%s' "$title" | sed -n 's/^[^(]*([^)]*)[:][[:space:]]*//p' \
    | tr 'A-Z' 'a-z' | sed 's/[^a-z0-9][^a-z0-9]*/-/g; s/^-+//; s/-+$//' | cut -c1-40 \
    | sed 's/-*$//')
[ -n "$slug" ] || slug="issue-$issue"
branch="$type/gh$issue-$slug"
common_dir=$(git -C "$root" rev-parse --path-format=absolute --git-common-dir)
# The main checkout's parent and name: <parent>/<repo>-wt-gh<issue>, parallel
# to the main checkout, not nested inside whatever worktree this runs from.
repo_root=$(dirname -- "$common_dir")
wt="$(dirname -- "$repo_root")/$(basename -- "$repo_root")-wt-gh$issue"
worktree_cmd="git worktree add -b $branch $wt origin/main"
echo "branch:   $branch (from origin/main)"
echo "worktree: $wt"
echo "cmd:      $worktree_cmd"

lane=edda-lane-gh$issue
lanes_dir="${TEMP:-/tmp}/edda-lanes"
if [ -f "$lanes_dir/$lane.done" ] || [ -f "$lanes_dir/$lane.log" ]; then
    n=2
    while [ -f "$lanes_dir/$lane-r$n.done" ] || [ -f "$lanes_dir/$lane-r$n.log" ]; do
        n=$((n + 1))
    done
    lane="$lane-r$n"
fi
brief_path="${EDDA_FLEET_SCRATCH:-$HOME/.edda/fleet}/brief-gh$issue.md"
render_cmd="sh scripts/fleet/brief-from-issue.sh $issue --lane-name $lane --worktree $wt --branch $branch"

echo "== brief"
brief_note=
if [ -f "$brief_path" ] && ! grep -q '^<<AUTHORED STEPS>>$' "$brief_path"; then
    brief_note="keeping authored brief: $brief_path"
    scope_csv=$(sed -n 's/^scope paths: //p' "$brief_path" | head -1)
else
    if [ "$dry" = 1 ]; then
        scope_csv=$(sh "$self_dir/brief-from-issue.sh" "$issue" --lane-name "$lane" \
            --worktree "$wt" --branch "$branch" | sed -n 's/^scope paths: //p' | head -1)
        render_cmd="$render_cmd > $brief_path"
    else
        sh "$self_dir/brief-from-issue.sh" "$issue" --lane-name "$lane" \
            --worktree "$wt" --branch "$branch" >"$brief_path"
        brief_note="rendered: $brief_path"
        scope_csv=$(sed -n 's/^scope paths: //p' "$brief_path" | head -1)
    fi
fi
[ -n "$scope_csv" ] || die "rendered brief has no scope paths line"
task_paths=
path_entry=$scope_csv
while [ -n "$path_entry" ]; do
    p=${path_entry%%, *}
    case "$p" in
        *,*) path_entry=${path_entry#*, } ;;
        *) path_entry= ;;
    esac
    task_paths="$task_paths --path $p"
done
task_cmd="edda task new \"$title_safe\" --assignee ${machine#*/}$task_paths --brief $brief_path --key gh$issue"

if [ "$dry" = 0 ] && grep -q '^<<AUTHORED STEPS>>$' "$brief_path"; then
    die "brief still contains <<AUTHORED STEPS>> — fill the authored middle in $brief_path, then rerun"
fi

echo "== task"
echo "cmd: $task_cmd"

echo "== brief"
if [ -n "$brief_note" ]; then
    echo "$brief_note"
fi
echo "cmd:  $render_cmd"
echo "brief path: $brief_path"

echo "== claim"
claim_cmd="sh scripts/fleet-claim-issue.sh $issue $machine"
echo "cmd: $claim_cmd"

echo "== launch"
case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*)
        launch_cmd="pwsh -NoProfile -File scripts/fleet/lane-launch.ps1 -Name $lane -Brief $brief_path -Cwd $wt -Agent pi -TimeoutSec 5400 -BudgetUsd 3" ;;
    *)
        launch_cmd="pi --model openrouter/z-ai/glm-5.3-flash --session-id lane-$lane \"\$(cat $brief_path)\"  # unattended runs need a process supervisor" ;;
esac
echo "cmd: $launch_cmd"
echo "lane name:   $lane"
echo "log:         %TEMP%\\edda-lanes\\$lane.log"
echo "done marker: %TEMP%\\edda-lanes\\$lane.done"

if [ "$dry" = 1 ]; then
    echo "== dry-run: nothing created, claimed, or launched"
    exit 0
fi

if [ ! -d "$wt" ]; then
    git -C "$root" worktree add -b "$branch" "$wt" origin/main >/dev/null
else
    echo "worktree already exists: $wt (not recreated)"
fi

echo "== running task new"
sh -c "$task_cmd"

echo "== running claim"
sh "$self_dir/../fleet-claim-issue.sh" "$issue" "$machine"

echo "== launching lane"
case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*)
        pwsh -NoProfile -File "$self_dir/lane-launch.ps1" -Name "$lane" \
            -Brief "$brief_path" -Cwd "$wt" -Agent pi -TimeoutSec 5400 -BudgetUsd 3 ;;
    *)
        die "unattended launch on POSIX needs a process supervisor — run interactively: $launch_cmd" ;;
esac
echo "lane launched: $lane (watch %TEMP%\\edda-lanes\\$lane.done; log .log alongside)"
