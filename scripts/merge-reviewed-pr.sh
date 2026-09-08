#!/bin/sh
# Operator entrypoint. --merge must only be used with explicit operator authority.
# Default is validation only — no merge; GitHub's match-head option closes the
# last race. The one write it can make is the union gate's own `edda review
# deliver` publishing the label/status for the verdict it just read; see there.
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
# The union gate (GH-1057) — an ADDITIONAL refusal, on top of everything above.
#
# Every check so far judges the *latest* trusted review pinned to this head.
# REVIEW.md §8's rule is the *union* over every verdict pinned to it: a later
# LGTM never overrides an earlier standing Changes Requested on the same SHA
# (GH-742). PR #1055 was merged over exactly that shape on 2026-09-07.
#
# That rule has one implementation, `edda review gate` (GH-769), and one reader
# of the §7 verdict comments that carry verdicts between machines,
# `edda review deliver` (GH-1030), which feeds those comments to it. Verdicts
# do not cross machines in the ledger yet (D8-debt(#671)), so the comment path
# is the only source that answers correctly here. What follows is a call and a
# field read: no verdict parsing and no union arithmetic live in this shell
# (mechanism.shell-role=one-line-adapter-only).
#
# `--sha "$head"` pins the question to the head every check above accepted, and
# skips deliver's own PR resolution, which fetches commits and would therefore
# require a checkout. GH_REPO carries $repo to `gh` the way `--repo` does
# above, so this step does not narrow where the script may run.
#
# Not read-only, and said plainly: deliver also publishes what it reads — the
# `review:*` label and the `Independent Review` commit status the union
# implies. It reads the current label and status first and writes nothing when
# they already match, so on the ordinary path (the reviewing session delivered
# its own round) this call makes zero GitHub writes. Where it does write, the
# write is the union verdict for the very SHA about to be merged.
#
# The window step is deliberately NOT wired here. `edda review gate --base`
# needs both commits present locally, which would turn this `gh --repo` script
# into one that requires a checkout. At this entrypoint the forge already makes
# the equivalent check at the moment it counts: `gh pr merge
# --match-head-commit "$head"` below refuses if anything landed on the PR after
# the reviewed SHA, and GitHub's own mergeStateStatus/branch protection refuses
# a base the PR is behind. That is judged sufficient here; a reviewer wanting
# the stronger tree-level window check should run `edda review gate <sha>
# --base <ref>` from a checkout, which is what it is for.
#
# `edda review deliver` exits by *delivery* outcome (0 delivered, 1 partial,
# 3 status withheld), not by the union — the union is `--json`'s `status`
# field. So its exit code is deliberately not the gate. Exit 2 ("cannot judge")
# prints no JSON at all, which the `jq -e` reads below turn into a refusal.
report=$(GH_REPO="$repo" "${EDDA_BIN:-edda}" review deliver --pr "$pr" --sha "$head" --json) || true
state=$(printf '%s\n' "$report" | jq -er '.status') \
  || die "union gate gave no readable answer for $head; refusing"
malformed=$(printf '%s\n' "$report" | jq -er '.malformed | length') \
  || die "union gate gave no readable answer for $head; refusing"
[ "$state" = success ] \
  || die "union over every trusted review pinned to $head is '$state', not a pass; a later LGTM does not override an earlier Changes Requested (GH-742)"
# A §7 heading below line 1 is a round the union never saw (GH-917), so the
# union above can read `success` precisely because a blocking round is
# invisible to it. Fail closed rather than merge on a verdict set with a hole.
[ "$malformed" = 0 ] \
  || die "$malformed verdict comment(s) on $head are malformed and outside the union; refusing"
gh pr checks "$pr" --repo "$repo" --required || die 'required checks are not green'
echo "review accepted: PR #$pr @ $head"
if [ "$action" = --merge ]; then
  gh pr merge "$pr" --repo "$repo" --squash --match-head-commit "$head"
fi
