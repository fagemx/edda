#!/bin/sh
# Shared machine-local review ownership, independent of controller scratch dirs.
# Published PR rounds provide the floor when another workstation reviewed first.
set -eu
die() { echo "review-round: $*" >&2; exit 1; }
if [ "${1:-}" = --help ]; then
  echo 'usage: review-round.sh reserve PR SHA minimum-round scratch | release PR SHA round'
  exit 0
fi
action=${1:-}; pr=${2:-}; sha=${3:-}; requested=${4:-}
printf '%s\n' "$pr" | grep -qE '^[1-9][0-9]*$' || die 'invalid PR'
printf '%s\n' "$sha" | grep -qE '^[0-9a-f]{40}$' || die 'expected full SHA'
printf '%s\n' "$requested" | grep -qE '^[1-9][0-9]{0,8}$' || die 'invalid round'
repo=${EDDA_REPO:-fagemx/edda}
printf '%s\n' "$repo" | grep -qE '^[A-Za-z0-9_-]+/[A-Za-z0-9_.-]+$' || die 'invalid repository'
coord=${EDDA_REVIEW_COORD_DIR:-$HOME/.edda/review-coordination}
dir="$coord/$repo/pr$pr"
mkdir -p "$dir"
# Never infer a stale lock from time or a reused PID. Interrupted reservations
# fail closed; the operator can inspect their metadata before recovery.
mkdir "$dir/mutex" 2>/dev/null || die "reservation busy: $dir/mutex"
trap 'rmdir "$dir/mutex"' 0
trap 'exit 1' HUP INT TERM
active="$dir/active"
if [ "$action" = release ]; then
  [ -f "$active" ] || exit 0
  oldround=$(sed -n '1p' "$active")
  oldsha=$(sed -n '2p' "$active")
  [ "$oldround" = "$requested" ] && [ "$oldsha" = "$sha" ] || die 'owner mismatch; retained active review'
  rm "$active"
  exit 0
fi
[ "$action" = reserve ] || die 'expected reserve or release'
scratch=${5:-}
[ -d "$scratch" ] || die 'scratch must already exist'
scratch=$(CDPATH= cd -- "$scratch" && pwd)
case "$scratch" in *'
'*) die 'scratch cannot contain a newline' ;; esac
if [ -f "$active" ]; then
  oldround=$(sed -n '1p' "$active")
  oldsha=$(sed -n '2p' "$active")
  olddone=$(sed -n '3p' "$active")
  oldscratch=$(dirname -- "$olddone")
  if ! grep -qE '^DISPATCH_EXIT=[0-9]+[[:space:]]*$' "$olddone" 2>/dev/null; then
    die "PR #$pr round $oldround at $oldsha still owns review; receipt: $olddone"
  fi
  # DISPATCH_EXIT alone is the IN-PROGRESS receipt, not a terminal one: the
  # round's worker may still be checking and cleaning the shared review
  # worktree at this scratch path, and admitting a new reviewer now would put
  # two writers in the same tree. The terminal receipt is the single
  # WORKTREE_CHECK line written (atomically, last) after all worktree check
  # and cleanup: ok admits the next round; any other value preserves the
  # failure info and keeps the slot closed for the operator to resolve.
  # Legacy receipts (pre-WORKTREE_CHECK) fall back to the filesystem: their
  # round ended cleanly only if the worktree is actually gone.
  if grep -qE '^WORKTREE_CHECK=' "$olddone" 2>/dev/null; then
    if ! grep -qE '^WORKTREE_CHECK=ok[[:space:]]*$' "$olddone" 2>/dev/null; then
      die "PR #$pr round $oldround at $oldsha has a non-clean terminal receipt; worktree cleanup not confirmed for $olddone — resolve the round below before reviewing ($(grep -E '^WORKTREE_CHECK=' "$olddone" | tail -1))"
    fi
  elif [ -e "$oldscratch/wt-review-pr$pr" ]; then
    die "PR #$pr round $oldround at $oldsha owns review: receipt $olddone has no terminal WORKTREE_CHECK and the review worktree still exists ($oldscratch/wt-review-pr$pr) — the old worker may still be checking or cleaning it"
  fi
fi
comments=$(gh pr view "$pr" --repo "$repo" --json comments --jq '.comments[].body') || die 'cannot read published rounds'
floor=$(printf '%s\n' "$comments" | sed -n 's/^## Code Review: Round \([0-9][0-9]*\).*/\1/p' | sort -n | tail -1)
floor=${floor:-0}
last=0
[ ! -f "$dir/counter" ] || last=$(cat "$dir/counter")
printf '%s\n' "$last" "$floor" | grep -qEv '^[0-9]{1,9}$' && die 'invalid round state'
[ "$floor" -le "$last" ] || last=$floor
next=$((last + 1))
[ "$requested" -le "$next" ] || next=$requested
[ "$next" -le 999999999 ] || die 'round counter exhausted'
# Write the counter before publishing ownership: a crash may skip a round but
# can never reuse one. The mutex protects the two writes from other controllers.
printf '%s\n' "$next" > "$dir/counter.new"
mv "$dir/counter.new" "$dir/counter"
printf '%s\n%s\n%s\n' "$next" "$sha" "$scratch/review-pr$pr-r$next.done" > "$dir/active.new"
mv "$dir/active.new" "$active"
printf '%s\n' "$next"
