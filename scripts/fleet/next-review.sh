#!/bin/sh
# next-review.sh — one command from an open PR to a posted review comment (GH-886).
# The skill-less controller's review half of the loop: pin the head SHA, count
# the rounds, refuse past the cap without operator authority, build the same
# brief the Claude line reviews from, run the read-only glm engine in a
# detached worktree, and post the result — as a SHADOW round when --shadow is
# given (no labels, no Independent Review status, per review.gh880-shadow),
# otherwise by delegating to review-pr.sh's verdict flow.
#
# usage:
#   sh scripts/fleet/next-review.sh <pr> [--shadow] [--operator-granted] [--dry-run]
#
# Never merges, never force-pushes, and edits exactly one label: needs-operator.
set -eu

usage() {
    echo "usage: $0 <pr> [--shadow] [--operator-granted] [--dry-run]" >&2
}

die() {
    echo "next-review: $1" >&2
    exit 2
}

repo=${EDDA_REPO:-fagemx/edda}
model=${EDDA_REVIEW_MODEL:-openrouter/z-ai/glm-5.3-flash}
pr=
shadow=0
granted=0
dry=0
while [ $# -gt 0 ]; do
    case "$1" in
        --shadow) shadow=1; shift ;;
        --operator-granted) granted=1; shift ;;
        --dry-run) dry=1; shift ;;
        -h|--help) usage; exit 0 ;;
        --) shift; break ;;
        -*)
            die "unknown option $1" ;;
        *)
            if [ -z "$pr" ]; then pr=$1; shift
            else die "unexpected argument $1"
            fi ;;
    esac
done
[ -n "$pr" ] || { usage; exit 2; }
case "$pr" in
    *[!0-9]*|'') die "pr must be a positive integer, got '$pr'" ;;
esac

self_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
root=$(CDPATH= cd -- "$self_dir/../.." && pwd)
scratch=${EDDA_FLEET_SCRATCH:-$HOME/.edda/fleet}
wt="$scratch/wt-review-pr$pr"

echo "== head SHA"
head_sha=$(gh pr view "$pr" --repo "$repo" --json headRefOid --jq .headRefOid) ||
    die "gh pr view $pr failed"
case "$head_sha" in
    [0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]\
[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]\
[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]\
[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]) : ;;
    *) die "head SHA is not a 40-hex SHA: '$head_sha'" ;;
esac
echo "head: $head_sha"

echo "== round count"
rounds=$(gh pr view "$pr" --repo "$repo" --json comments \
    --jq '[.comments[] | select(.body | startswith("## Code Review: Round"))] | length') ||
    die "gh pr view $pr comments failed"
echo "rounds already posted: $rounds"
case "$rounds" in
    *[!0-9]*|'') die "could not read the round count from the PR comments" ;;
esac
if [ "$rounds" -ge 3 ] && [ "$granted" = 0 ]; then
    if [ "$dry" = 0 ]; then
        gh issue edit "$pr" --repo "$repo" --add-label needs-operator >/dev/null ||
            die "could not add the needs-operator label"
    fi
    die "round cap: $rounds review rounds posted and no --operator-granted — labeled needs-operator, launching nothing"
fi
round=$((rounds + 1))

echo "== brief"
brief_cmd="sh scripts/review-pr.sh $pr $round --dry-run"
echo "cmd: $brief_cmd"
if [ "$dry" = 1 ]; then
    sh "$self_dir/../review-pr.sh" "$pr" "$round" --dry-run 2>/dev/null || true
    echo "(dry-run: the real run regenerates this brief against the pinned head)"
    echo "== engine command"
    echo "cmd: edda dispatch --agent pi --model $model --tools read,grep,find,ls --thinking max --budget-usd 1 --timeout-sec 1500 --json --prompt-file <brief> (cwd: $wt)"
    echo "== post"
    if [ "$shadow" = 1 ]; then
        echo "cmd: gh pr comment $pr --repo $repo --body-file <verdict>  # heading suffix ' (SHADOW)', body line 'shadow: true', no labels, no status"
    else
        echo "cmd: EDDA_REVIEW_AGENT=pi sh scripts/review-pr.sh $pr $round  # full verdict flow"
    fi
    echo "== dry-run: nothing launched or posted"
    exit 0
fi

brief_out=$(sh "$self_dir/../review-pr.sh" "$pr" "$round" --dry-run) || die "review-pr.sh dry-run failed"
brief_path=$(printf '%s\n' "$brief_out" | sed -n 's/^brief=//p')
[ -n "$brief_path" ] || die "could not read the brief path from review-pr.sh output"
echo "brief: $brief_path"
[ -f "$brief_path" ] || die "brief file missing: $brief_path"
base_sha=$(printf '%s\n' "$brief_out" | sed -n 's/^spec=REVIEW.md@\([0-9a-f]*\).*/\1/p' | head -1)
[ -n "$base_sha" ] || die "could not read the spec SHA from review-pr.sh output"

echo "== review worktree"
if [ -d "$wt" ]; then
    git -C "$wt" checkout -q --detach "$head_sha"
else
    git -C "$root" worktree add --detach "$wt" "$head_sha" >/dev/null
fi
git -C "$wt" show "$base_sha:REVIEW.md" >"$wt/.edda-review-spec.md"

echo "== engine (read-only glm, thinking max)"
engine_cmd="edda dispatch --agent pi --model $model --tools read,grep,find,ls --thinking max --budget-usd 1 --timeout-sec 1500 --json --prompt-file $brief_path"
echo "cmd: $engine_cmd"
engine_out=$(cd "$wt" && edda dispatch --agent pi --model "$model" \
    --tools read,grep,find,ls --thinking max --budget-usd 1 --timeout-sec 1500 \
    --json --prompt-file "$brief_path") || die "engine dispatch failed"
echo "$engine_out" >"$wt/next-review-engine.json"
outcome=$(printf '%s' "$engine_out" | jq -r .outcome)
[ "$outcome" = "done" ] || die "engine outcome is $outcome (see $wt/next-review-engine.json)"
verdict=$(printf '%s' "$engine_out" | jq -r .result_text)
model_observed=$(printf '%s' "$engine_out" | jq -r .model_observed)
cost=$(printf '%s' "$engine_out" | jq -r '.cost_usd // "unmeasured"')
elapsed_ms=$(printf '%s' "$engine_out" | jq -r '.elapsed_ms // empty')
[ -n "$verdict" ] && [ "$verdict" != "null" ] || die "engine returned no verdict text"

echo "== post"
head_now=$(gh pr view "$pr" --repo "$repo" --json headRefOid --jq .headRefOid)
if [ "$head_now" != "$head_sha" ]; then
    die "head moved between brief and post ($head_sha -> $head_now) — posting nothing"
fi
verdict_file="$wt/next-review-r$round-verdict.md"
printf '%s\n' "$verdict" >"$verdict_file"
if [ "$shadow" = 1 ]; then
    # SHADOW shape (review.gh880-shadow): heading suffix, shadow: true, no
    # labels, no Independent Review status, not a merge authority.
    sed -i "1s/\$/ (SHADOW)/" "$verdict_file"
    sed -i "1a shadow: true" "$verdict_file"
    cost_line="$cost"
    [ -n "$elapsed_ms" ] && cost_line="$cost / ${elapsed_ms}ms"
    sed -i "s|^- cost: .*|- cost: $cost_line (dispatch --json)|" "$verdict_file"
    gh pr comment "$pr" --repo "$repo" --body-file "$verdict_file" >/dev/null ||
        die "gh pr comment failed"
    echo "posted SHADOW round $round: $verdict_file"
else
    echo "delegating the verdict flow to review-pr.sh (labels and status included)"
    EDDA_REVIEW_AGENT=pi sh "$self_dir/../review-pr.sh" "$pr" "$round"
fi
