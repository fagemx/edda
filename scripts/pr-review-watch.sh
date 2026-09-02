#!/bin/sh
# PR review watcher (GH-632, option A — local watcher).
#
# Polls open PRs and, for each non-draft PR whose head SHA has not been
# reviewed yet, hands it to scripts/review-pr.sh which launches the
# read-only reviewer and posts the SHA-pinned verdict back to the PR.
#
# Hard rules (ratified decisions, see docs/guides/operator-runbook.md §六):
#   - reviewer is always read-only (review-pr.sh enforces this)
#   - reviewer model is fixed for the transition period; never downgraded
#   - the watcher never merges — merge is operator authority (pr.merge-policy)
#   - state file lives outside git (~/.edda/pr-review-watch.state)
#
# Usage:
#   pr-review-watch.sh run [interval-seconds]   poll forever (default 60s)
#   pr-review-watch.sh once [interval-ignored]  single poll cycle
#   pr-review-watch.sh decide < pr-list.json    offline triage (testable):
#                                               reads `gh pr list --state open
#                                               --json number,headRefOid,isDraft,
#                                               labels` output, prints one line
#                                               per PR: "REVIEW <n> <sha>" or
#                                               "SKIP <n> <reason>"
#
# Environment:
#   PR_REVIEW_WATCH_STATE   state file (default: ~/.edda/pr-review-watch.state)
#   PR_REVIEW_WATCH_SCRIPTS directory holding review-pr.sh (default: script dir)
#
# State file format: one "<pr-number> <head-sha>" line per posted review
# verdict. On a failed review attempt nothing is recorded, but review-pr.sh
# labels the PR `review:unreviewed`, which stops the watcher for that PR
# (fleet.review-provider-overload: 未審查是誠實狀態).

set -eu

state_file=${PR_REVIEW_WATCH_STATE:-"${HOME:-$USERPROFILE}/.edda/pr-review-watch.state"}
scripts_dir=${PR_REVIEW_WATCH_SCRIPTS:-"$(CDPATH= cd -- "$(dirname "$0")" && pwd)"}

log() {
    printf '%s pr-review-watch: %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$*"
}

# state_has <file> <number> <sha> — has this PR already been reviewed at sha?
state_has() {
    grep -qx "$2 $3" "$1" 2>/dev/null
}

# decide — read PR JSON on stdin, print triage lines. Pure: no gh calls.
decide() {
    jq -c '.[]' | while IFS= read -r pr; do
        number=$(printf '%s' "$pr" | jq -r '.number')
        sha=$(printf '%s' "$pr" | jq -r '.headRefOid // ""')
        draft=$(printf '%s' "$pr" | jq -r '.isDraft // false')
        unreviewed=$(printf '%s' "$pr" | \
            jq -r '[.labels[]?.name] | index("review:unreviewed") != null')
        if [ "$draft" = "true" ]; then
            printf 'SKIP %s draft\n' "$number"
        elif [ "$unreviewed" = "true" ]; then
            printf 'SKIP %s review-unreviewed\n' "$number"
        elif [ -z "$sha" ]; then
            printf 'SKIP %s missing-head\n' "$number"
        elif state_has "$state_file" "$number" "$sha"; then
            printf 'SKIP %s already-reviewed\n' "$number"
        else
            printf 'REVIEW %s %s\n' "$number" "$sha"
        fi
    done
}

# record_state <number> <sha> — append after a verdict is posted.
record_state() {
    mkdir -p "$(dirname -- "$state_file")"
    printf '%s %s\n' "$1" "$2" >>"$state_file"
}

run_cycle() {
    gh pr list --state open --json number,headRefOid,isDraft,labels | decide |
    while IFS=' ' read -r action number sha; do
        case $action in
            REVIEW)
                log "PR #$number head $sha needs review — dispatching reviewer"
                if sh "$scripts_dir/review-pr.sh" "$number" "$sha"; then
                    record_state "$number" "$sha"
                    log "PR #$number review posted"
                else
                    log "PR #$number review failed (review-pr.sh handled fallback/label)"
                fi
                ;;
            SKIP)
                log "PR #$number skipped: $sha"
                ;;
        esac
    done
}

case ${1:-run} in
    decide)
        decide
        ;;
    once)
        run_cycle
        ;;
    run)
        interval=${2:-${PR_REVIEW_WATCH_INTERVAL:-60}}
        log "watching open PRs every ${interval}s (state: $state_file)"
        while :; do
            if ! run_cycle; then
                log "poll cycle failed (gh or jq error); retrying next interval"
            fi
            sleep "$interval"
        done
        ;;
    *)
        printf 'usage: %s {run|once|decide}\n' "$0" >&2
        exit 2
        ;;
esac
