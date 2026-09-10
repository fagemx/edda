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
# GH-993: also refuses, for both --check and --merge, unless
# scripts/fleet/verdict-drift.sh exits 0 across every open PR — see the
# comment above that call, below.
set -eu
die() { echo "merge-reviewed-pr: $*" >&2; exit 1; }
if [ "${1:-}" = --help ]; then
  echo 'usage: merge-reviewed-pr.sh PR [--check|--merge] [--body-file <path>] (merge requires operator authority)'
  echo '  --check (default) validates only. --merge squash-merges with the subject'
  echo '  ALWAYS pinned to the PR title via --subject (GH-1100), validated against the'
  echo '  conventional commit rule (REVIEW.md §5 U4) before any merge: a wip(...),'
  echo '  empty-scope, or missing-type title refuses. The " (#N)" PR back-reference'
  echo '  GitHub only adds to a subject it picks itself is appended here instead, so'
  echo '  the squash commit keeps its PR pointer. The merge body comes from'
  echo '  --body-file, or is composed as a receipt (reviewed SHA, review round, CI run).'
  exit 0
fi
self_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
pr=${1:-}
printf '%s\n' "$pr" | grep -qE '^[1-9][0-9]*$' || die 'invalid PR'
shift
# GH-1100: flags after the PR number. `--body-file <path>` joins --check/--merge
# (still the default action); anything unrecognized refuses, and a missing
# --body-file operand refuses rather than eating the next flag.
action=--check
body_file=
while [ "$#" -gt 0 ]; do
  case "$1" in
    --check|--merge) action=$1 ;;
    --body-file)
      [ "$#" -ge 2 ] || die '--body-file requires a path'
      # An empty operand used to slip through as "no body file": the -z test
      # below short-circuits on it, so `--body-file ''` silently composed a
      # receipt while every other malformed operand refused. Refuse here too.
      [ -n "$2" ] || die '--body-file requires a non-empty path'
      body_file=$2
      shift
      ;;
    *) die "unexpected argument: $1 (expected --check, --merge, or --body-file <path>)" ;;
  esac
  shift
done
[ -z "$body_file" ] || [ -f "$body_file" ] || die "body file not found: $body_file"
# --body-file only has a reader on the merge path. Accepting it under --check
# and then ignoring it tells the caller their body was taken when it was not.
[ -z "$body_file" ] || [ "$action" = --merge ] || die '--body-file applies to --merge only'
repo=${EDDA_REPO:-fagemx/edda}
printf '%s\n' "$repo" | grep -qE '^[A-Za-z0-9_-]+/[A-Za-z0-9_.-]+$' || die 'invalid repository'
# GH-993: a completed review round must reach the PR regardless of transport
# — the observed failure was a `Review Response: Round N` comment with no
# matching `Code Review: Round N` anywhere on the PR (a subagent's report
# that was never posted). verdict-drift.sh already detects that, plus "no
# verdict on head", a stale verdict, and CONFLICTING mergeability, across
# every open PR (GH-914/958) — but its only prior caller (daily-digest.sh)
# captures the exit code and reports it without blocking (by design — see
# the comment there). This is the "declare a PR done" entrypoint
# docs/guides/pi-controller-runbook.md:125 already names as the place to
# run verdict-drift.sh, by policy; wiring it here enforces that policy
# instead of relying on an operator remembering it, for --check and --merge
# alike (nothing above this point branches on $action, so both share this
# code path). The whole open-PR set is checked, not just $pr: a fleet-wide
# check scoped down to one PR would stop being the check GH-958 shares with
# daily-digest.sh, and this is deliberately the same blocking scope
# pi-controller-runbook.md already names, not a narrower one invented here.
#
# `drift_rc` is read directly from `$?` of the substitution on the next
# line — nothing pipes into or out of it, so there is no stage for the exit
# code to hide behind (the near-miss the issue's own author records:
# `verdict-drift.sh | head -20; echo $?` reads `head`'s exit code, not
# verdict-drift.sh's — GH-993). Every nonzero exit refuses, not only exit 1
# (drift found): exit 2 (verdict-drift.sh could not read PR state at all)
# must refuse too, or a broken read would wave every merge through clean.
drift_rc=0
drift_out=$(EDDA_REPO="$repo" sh "$self_dir/fleet/verdict-drift.sh" 2>&1) || drift_rc=$?
if [ "$drift_rc" -ne 0 ]; then
  printf '%s\n' "$drift_out" >&2
  die "verdict-drift.sh is not clean across the open PR set (exit $drift_rc) — refusing until every open PR carries a verdict on its head (output above)"
fi
facts=$(gh pr view "$pr" --repo "$repo" --json headRefOid,state --jq '[.headRefOid,.state]|@tsv') || die 'cannot read PR head'
head=$(printf '%s\n' "$facts" | cut -f1)
state=$(printf '%s\n' "$facts" | cut -f2)
printf '%s\n' "$head" | grep -qE '^[0-9a-f]{40}$' || die 'invalid PR head'
[ "$state" = OPEN ] || die "PR is $state"
# GH-1100: the squash subject is pinned to the PR title, never left to GitHub.
# With no --subject, GitHub picks the squash subject itself, and for a PR with
# exactly one commit it uses that commit's subject verbatim — which wrote a
# `wip(review): ...` checkpoint (fa0d011) onto main permanently. The title is
# read here and checked against the conventional-commit subject rule
# (REVIEW.md §5 U4) in both modes: earlier signal on --check, and the
# mandatory refusal on the merge path, before `gh pr merge` is ever reached.
subject=$(gh pr view "$pr" --repo "$repo" --json title --jq '.title') || die 'cannot read PR title'
printf '%s\n' "$subject" | grep -qE '^(feat|fix|docs|refactor|test|chore|perf|build|ci|style|revert)(\([a-z0-9._/-]+\))?!?: .+' \
  || die "PR title is not a conventional commit subject (REVIEW.md §5 U4) and would become the squash subject as-is: '$subject' — rename the PR before merging"
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
  # GH-1100: `--subject` is mandatory — without it GitHub chooses the squash
  # subject (the single-commit hazard above). `--body-file` is the caller's
  # file when given; otherwise a minimal receipt is composed naming the
  # reviewed SHA, the LGTM round parsed from $header, and the CI run link from
  # `gh pr checks --json` (the `link` field carries the Actions run URL).
  merge_body=$body_file
  if [ -z "$merge_body" ]; then
    round=$(printf '%s\n' "$header" | sed -n 's/^## Code Review: Round \([0-9][0-9]*\).*/\1/p')
    [ -n "$round" ] || die 'cannot parse the review round for the merge body'
    checks_json=$(gh pr checks "$pr" --repo "$repo" --required --json name,state,link) || die 'cannot read check runs for the merge body'
    ci_link=$(printf '%s\n' "$checks_json" | jq -r 'map(.link // empty) | map(select(length > 0)) | (first // "")') || ci_link=''
    [ -n "$ci_link" ] || die 'no check run link found for the merge body'
    merge_body=$(mktemp "${TMPDIR:-/tmp}/merge-reviewed-pr-body.XXXXXX") || die 'cannot create the merge body file'
    trap 'rm -f "$merge_body"' 0 HUP INT TERM
    {
      printf 'Squash merge via scripts/merge-reviewed-pr.sh.\n\n'
      printf '%s\n' "- Reviewed SHA: $head"
      printf '%s\n' "- Code review: PR #$pr Round $round — LGTM (P0=0, P1=0)"
      printf '%s\n' "- CI: $ci_link"
    } >"$merge_body"
  fi
  # GH-1100 Round 2: GitHub appends the ` (#N)` PR back-reference only to a
  # squash subject IT chooses; a subject supplied through --subject is used
  # verbatim, with no suffix. Pinning the title without re-adding the suffix
  # would therefore strip the PR pointer from every future squash commit on
  # main — measured, not theorised: of the last 60 subjects on main, 58 carry
  # ` (#N)` and the only two that do not are exactly the two merged by hand
  # with an explicit --subject. R7 forbids rewriting main, so each such commit
  # would stay pointer-less forever. Compose what GitHub would have written.
  #
  # $subject stays the bare title: the U4 check above judges the title alone,
  # never the title-plus-suffix, so a trailing ` (#N)` can neither rescue a
  # bad title nor break a good one.
  #
  # The suffix is skipped only when the title already ends in THIS PR's own
  # number — the one case where appending would duplicate it. A title ending
  # in some OTHER PR's number still gets ` (#$pr)` appended: leaving a foreign
  # number as the trailing back-reference would point `git log` at an
  # unrelated PR, which is worse than a title that reads `… (#999) (#$pr)`.
  merge_subject="$subject (#$pr)"
  case "$subject" in
    *" (#$pr)") merge_subject=$subject ;;
  esac
  gh pr merge "$pr" --repo "$repo" --squash --match-head-commit "$head" --subject "$merge_subject" --body-file "$merge_body"
fi
