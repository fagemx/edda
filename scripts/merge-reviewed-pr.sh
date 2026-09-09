#!/bin/sh
# Operator entrypoint. --merge must only be used with explicit operator authority.
# Default is validation only — no merge; GitHub's match-head option closes the
# last race. The union gate's own `edda review deliver` is the write surface
# this script opens: it can publish the `review:*` label, the `Independent
# Review` commit status, and — on a malformed §7 comment — a one-time notice
# comment on the PR; see the comment above the `report=` assignment below.
# Requires an `edda` binary built with `review deliver` (GH-1030,
# post-2026-09-08); a stale or
# missing binary is not detected separately here — it reports as a union
# refusal (fail-closed). Check with `edda --version`.
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
# Not read-only, and said plainly: deliver can make three kinds of GitHub
# write — the `review:*` label, the `Independent Review` commit status the
# union implies, and, when a §7 comment does not parse, a one-time `review:
# malformed verdict comment <id>` notice on the PR (crates/edda-cli/src/
# cmd_review/delivery.rs, `deliver()`'s notice loop; GH-917 / #917). It reads
# the current label and status first and writes nothing when they already
# match, so on the ordinary path (the reviewing session delivered its own
# round, nothing malformed) this call makes zero GitHub writes. Where it does
# write, the label/status write is the union verdict for the very SHA about
# to be merged, and a malformed notice is posted under the operator's own
# token — exactly the path the `$malformed` refusal below exists to serve.
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
#
# Not gating still means visible: `|| true` below keeps a nonzero delivery
# exit from aborting the script (that decision stands — see above), but the
# exit code and any individual failed write (`status_write`/`label`/
# `label_removed` carrying outcome "failed") are still worth the operator's
# attention, since a failed write means the label/status this refusal relies
# on was never published. Report both, to stderr, without gating on them.
report_exit=0
report=$(GH_REPO="$repo" "${EDDA_BIN:-edda}" review deliver --pr "$pr" --sha "$head" --json) \
  || report_exit=$?
failed_writes=$(printf '%s\n' "$report" | jq -r '
    [
      {name: "status", w: .status_write},
      {name: (.label.name // "label"), w: .label},
      {name: (.label_removed.name // "label_removed"), w: .label_removed}
    ]
    | map(select(.w != null and .w.outcome == "failed") | .name)
    | join(", ")
  ' 2>/dev/null) || failed_writes=''
if [ "$report_exit" -ne 0 ] || [ -n "$failed_writes" ]; then
  echo "merge-reviewed-pr: warning: edda review deliver did not cleanly write for $head (exit $report_exit)${failed_writes:+; failed: $failed_writes}" >&2
fi
# `union_state` (not `state` — that name is already the PR's OPEN/CLOSED
# state read above) is the union's own verdict; it alone decides the refusal
# below, independent of the write-outcome warning above.
union_state=$(printf '%s\n' "$report" | jq -er '.status') \
  || die "union gate gave no readable answer for $head; refusing"
# `arrays` fails closed on a key-less or reshaped report: `.malformed | length`
# reads a missing key as `null`, and `null | length` is `0`, which would pass
# this check rather than refuse it. `arrays` only lets an actual array through,
# so a missing/renamed key produces no output, `-e` sees nothing, and the
# command fails into the `die` below — the same fail-closed shape as `.status`.
malformed=$(printf '%s\n' "$report" | jq -er '.malformed | arrays | length') \
  || die "union gate gave no readable answer for $head; refusing"
[ "$union_state" = success ] \
  || die "union over every §7 verdict comment pinned to $head is '$union_state', not a pass; a later LGTM does not override an earlier Changes Requested (GH-742)"
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
