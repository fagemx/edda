#!/bin/sh
# Offline regression: independent controller scratches share review ownership.
set -eu
root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' 0 HUP INT TERM
mkdir -p "$tmp/bin" "$tmp/one" "$tmp/two"
export EDDA_REVIEW_COORD_DIR="$tmp/coord" EDDA_REPO=fixture/repo
export FIXTURE="$tmp" PATH="$tmp/bin:$PATH"
sha=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
other=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
cat > "$tmp/bin/gh" <<'EOF'
#!/bin/sh
case "$*" in
  *'--json comments'*) [ ! -f "$FIXTURE/api-fail" ] || exit 1; cat "$FIXTURE/comments" ;;
  *'--json headRefOid,state'*) printf '%s\tOPEN\n' "$(cat "$FIXTURE/head")" ;;
  *'api --paginate --slurp repos/fixture/repo/issues/12/comments?per_page=100'*) [ ! -f "$FIXTURE/api-fail" ] || exit 1; cat "$FIXTURE/comments-api" ;;
  *'pr checks'*) [ ! -f "$FIXTURE/checks-fail" ] ;;
  *'pr merge'*) printf '%s\n' "$*" > "$FIXTURE/merge" ;;
  *) echo "unexpected gh: $*" >&2; exit 1 ;;
esac
EOF
chmod +x "$tmp/bin/gh"
printf '## Code Review: Round 4 — PR #12 @ %s\n' "$sha" > "$tmp/comments"
printf '%s\n' "$sha" > "$tmp/head"
cat > "$tmp/comments-api" <<EOF
[
  [
    {
      "id": 10,
      "author_association": "MEMBER",
      "updated_at": "2026-09-04T00:00:00Z",
      "body": "## Code Review: Round 7 — PR #12 @ $sha\n- escalations: none\n### Verdict\nLGTM (P0=0, P1=0)"
    }
  ]
]
EOF
round=$(sh "$root/scripts/review-round.sh" reserve 12 "$sha" 1 "$tmp/one")
[ "$round" = 5 ]
if sh "$root/scripts/review-round.sh" reserve 12 "$sha" 1 "$tmp/two"; then
  echo 'FAIL: second controller acquired an active review' >&2; exit 1
fi
printf 'DISPATCH_EXIT=0\n' > "$tmp/one/review-pr12-r5.done"
# GH-702 terminal receipts: DISPATCH_EXIT alone is an in-progress receipt while
# the old worker may still be checking/cleaning the shared review worktree.
# Legacy receipts (no WORKTREE_CHECK line) are terminal only once the worktree
# is actually gone.
mkdir -p "$tmp/one/wt-review-pr12"
if sh "$root/scripts/review-round.sh" reserve 12 "$other" 1 "$tmp/two"; then
  echo 'FAIL: in-progress round (worktree still present) admitted a new reviewer' >&2; exit 1
fi
rmdir "$tmp/one/wt-review-pr12"
round=$(sh "$root/scripts/review-round.sh" reserve 12 "$other" 1 "$tmp/two")
[ "$round" = 6 ]
# A receipt carrying a non-clean WORKTREE_CHECK preserves the failure and keeps
# the slot closed even though the worktree is gone — cleanup ownership is
# uncertain, so an operator resolves the round; no reviewer is auto-admitted.
printf 'DISPATCH_EXIT=0\nWORKTREE_CHECK=failed: worktree remove failed (file locked)\n' > "$tmp/two/review-pr12-r6.done"
if sh "$root/scripts/review-round.sh" reserve 12 "$sha" 1 "$tmp/one"; then
  echo 'FAIL: failed worktree cleanup admitted a new reviewer' >&2; exit 1
fi
printf 'DISPATCH_EXIT=0\nWORKTREE_CHECK=ok\n' > "$tmp/two/review-pr12-r6.done"
round=$(sh "$root/scripts/review-round.sh" reserve 12 "$sha" 1 "$tmp/one")
[ "$round" = 7 ]
if sh "$root/scripts/review-round.sh" release 12 "$sha" 6; then
  echo 'FAIL: wrong SHA released another owner' >&2; exit 1
fi
sh "$root/scripts/review-round.sh" release 12 "$other" 6
touch "$tmp/api-fail"
if sh "$root/scripts/review-round.sh" reserve 12 "$sha" 1 "$tmp/one"; then
  echo 'FAIL: API outage accepted as empty round history' >&2; exit 1
fi
rm "$tmp/api-fail"
# Real concurrent processes: exactly one reservation may succeed, across
# distinct scratch roots sharing one round counter.
sh "$root/scripts/review-round.sh" reserve 12 "$sha" 1 "$tmp/one" > "$tmp/a" 2> "$tmp/a.err" & a=$!
sh "$root/scripts/review-round.sh" reserve 12 "$sha" 1 "$tmp/two" > "$tmp/b" 2> "$tmp/b.err" & b=$!
success=0
if wait "$a"; then success=$((success + 1)); fi
if wait "$b"; then success=$((success + 1)); fi
[ "$success" = 1 ]
# The winner's active receipt has no terminal WORKTREE_CHECK yet (it does not
# even exist until the worker starts writing it), so the loser is refused again
# immediately after the race, not only during it.
if sh "$root/scripts/review-round.sh" reserve 12 "$sha" 1 "$tmp/one"; then
  echo 'FAIL: post-race reserve admitted while the winner owns without a terminal receipt' >&2; exit 1
fi
sh "$root/scripts/merge-reviewed-pr.sh" 12
[ ! -e "$tmp/merge" ]
sh "$root/scripts/merge-reviewed-pr.sh" 12 --merge
grep -q -- "--match-head-commit $sha" "$tmp/merge"
rm "$tmp/merge"
printf '%s\n' "$other" > "$tmp/head"
if sh "$root/scripts/merge-reviewed-pr.sh" 12 --merge; then
  echo 'FAIL: stale verdict merged' >&2; exit 1
fi
printf '%s\n' "$sha" > "$tmp/head"
sed 's/escalations: none/escalations: needs escalation/' "$tmp/comments-api" > "$tmp/new"
mv "$tmp/new" "$tmp/comments-api"
if sh "$root/scripts/merge-reviewed-pr.sh" 12 --merge; then
  echo 'FAIL: provisional review merged' >&2; exit 1
fi
cat > "$tmp/comments-api" <<EOF
[
  [
    {
      "id": 10,
      "author_association": "MEMBER",
      "updated_at": "2026-09-04T00:00:00Z",
      "body": "## Code Review: Round 7 — PR #12 @ $sha\n- escalations: none\n### Verdict\nLGTM (P0=0, P1=0)"
    },
    {
      "id": 11,
      "author_association": "MEMBER",
      "updated_at": "2026-09-04T01:00:00Z",
      "body": "## Code Review: Round 8 — PR #12 @ $sha\n- escalations: none\n### Verdict\nChanges Requested, P0=0, P1=1"
    }
  ]
]
EOF
if sh "$root/scripts/merge-reviewed-pr.sh" 12 --merge; then
  echo 'FAIL: newer blocking review merged' >&2; exit 1
fi
[ ! -e "$tmp/merge" ]
# An older-created review later edited to blockers is the latest trustworthy
# verdict by REST `updated_at`, even if a later-created LGTM remains present.
cat > "$tmp/comments-api" <<EOF
[
  [
    {
      "id": 10,
      "author_association": "MEMBER",
      "updated_at": "2026-09-04T02:00:00Z",
      "body": "## Code Review: Round 7 — PR #12 @ $sha\n- escalations: none\n### Verdict\nChanges Requested, P0=0, P1=1"
    },
    {
      "id": 11,
      "author_association": "MEMBER",
      "updated_at": "2026-09-04T01:00:00Z",
      "body": "## Code Review: Round 8 — PR #12 @ $sha\n- escalations: none\n### Verdict\nLGTM (P0=0, P1=0)"
    }
  ]
]
EOF
if sh "$root/scripts/merge-reviewed-pr.sh" 12 --merge; then
  echo 'FAIL: an older-created but later-edited blocking review merged' >&2; exit 1
fi
echo 'PASS: shared rounds, concurrent admission, terminal receipts, REST latest-edited trusted reviews, API errors, stale/provisional/blocking verdicts, matched merge'
