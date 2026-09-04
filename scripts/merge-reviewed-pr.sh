#!/bin/sh
# Operator entrypoint. --merge must only be used with explicit operator authority.
# Default is read-only validation; GitHub's match-head option closes the last race.
set -eu
die() { echo "merge-reviewed-pr: $*" >&2; exit 1; }
if [ "${1:-}" = --help ]; then
  echo 'usage: merge-reviewed-pr.sh PR [--merge] (merge requires operator authority)'
  exit 0
fi
pr=${1:-}; action=${2:---check}
printf '%s\n' "$pr" | grep -qE '^[1-9][0-9]*$' || die 'invalid PR'
case "$action" in --check|--merge) ;; *) die 'expected --check or --merge' ;; esac
[ "$#" -le 2 ] || die 'too many arguments'
repo=${EDDA_REPO:-fagemx/edda}
printf '%s\n' "$repo" | grep -qE '^[A-Za-z0-9_-]+/[A-Za-z0-9_.-]+$' || die 'invalid repository'
facts=$(gh pr view "$pr" --repo "$repo" --json headRefOid,state --jq '[.headRefOid,.state]|@tsv') || die 'cannot read PR head'
head=$(printf '%s\n' "$facts" | cut -f1)
state=$(printf '%s\n' "$facts" | cut -f2)
printf '%s\n' "$head" | grep -qE '^[0-9a-f]{40}$' || die 'invalid PR head'
[ "$state" = OPEN ] || die "PR is $state"
# `gh pr view --json comments` does not expose a comment `updatedAt` field.
# Read the REST issue-comments shape instead: it carries `updated_at`, and
# --paginate/--slurp makes the selection global rather than accidentally
# choosing the newest item of just the final page.  A malformed trusted review
# fails closed; choosing an older LGTM after an edited blocker is unsafe.
comments=$(gh api --paginate --slurp "repos/$repo/issues/$pr/comments?per_page=100") || die 'cannot read reviews'
body=$(printf '%s\n' "$comments" | jq -er '
  [ .[] | .[] ]
  | map(select(
      (.author_association == "OWNER" or .author_association == "MEMBER" or .author_association == "COLLABORATOR")
      and (.body | type == "string" and test("(?m)^## Code Review: Round [0-9]+"))
    )) as $trusted
  | if ($trusted | length) == 0 then ""
    elif ($trusted | map(select((.updated_at | type) != "string" or (.updated_at | length) == 0 or (.id | type) != "number")) | length) != 0
      then error("trusted review lacks REST updated_at or numeric id")
    else $trusted | sort_by([.updated_at, .id]) | last | .body
    end
') || die 'cannot select latest updated trusted review'
header=$(printf '%s\n' "$body" | grep -m1 '^## Code Review: Round ' || true)
printf '%s\n' "$header" | grep -qE "^## Code Review: Round [0-9]+ .*PR #$pr @ $head([^0-9a-f]|$)" || die "latest trusted review is not pinned to current head $head"
printf '%s\n' "$body" | grep -qE '^- escalations: none[[:space:]]*$' || die 'review has missing or unresolved escalations'
verdict=$(printf '%s\n' "$body" | awk '/^### Verdict[[:space:]]*$/{found=1;next} found && NF{print;exit}')
printf '%s\n' "$verdict" | grep -qE '^LGTM \(P0=0, P1=0\)([[:space:]]|$)' || die 'latest review does not approve with P0=0/P1=0'
case "$verdict" in *'Changes Requested'*|*provisional*) die 'provisional or conflicting verdict' ;; esac
gh pr checks "$pr" --repo "$repo" --required || die 'required checks are not green'
echo "review accepted: PR #$pr @ $head"
if [ "$action" = --merge ]; then
  gh pr merge "$pr" --repo "$repo" --squash --match-head-commit "$head"
fi
