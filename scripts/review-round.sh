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
  # DISPATCH_EXIT is deliberately not terminal: the reviewer may still be
  # checking and removing the shared worktree, then unregistering its scheduled
  # task. The worker buffers the complete receipt and atomically renames it
  # only after those steps. Do not infer completion from a missing worktree:
  # legacy or partial receipts fail closed until an operator resolves them.
  terminal=$(cat "$olddone" 2>/dev/null) || die "cannot read active receipt $olddone"
  if ! printf '%s\n' "$terminal" | grep -qx 'WORKTREE_CHECK=unchanged' \
    || ! printf '%s\n' "$terminal" | grep -qx 'WORKTREE_CLEANUP=removed' \
    || ! printf '%s\n' "$terminal" | grep -Eq '^TASK_CLEANUP=(unregistered|not-applicable)$' \
    || ! printf '%s\n' "$terminal" | grep -qx 'TERMINAL_RECEIPT=complete'; then
    die "PR #$pr round $oldround at $oldsha has no clean atomic terminal receipt; preserve its source/log/worktree and resolve it before reviewing ($olddone)"
  fi
fi
# REST issue comments expose author_association; only the merge guard's
# trusted associations may advance the shared round floor.
comments=$(gh api --paginate --slurp "repos/$repo/issues/$pr/comments?per_page=100") || die 'cannot read published rounds'
floor=$(printf '%s\n' "$comments" | jq -er '
  [ .[] | .[] | select(.author_association == "OWNER" or .author_association == "MEMBER" or .author_association == "COLLABORATOR")
    | .body | select(type == "string")
    | capture("(?m)^## Code Review: Round (?<round>[0-9]+)").round | tonumber ]
  | if length == 0 then 0 else max end
' 2>/dev/null) || die 'cannot parse trusted published rounds'
floor=${floor:-0}
last=0
[ ! -f "$dir/counter" ] || last=$(cat "$dir/counter")
if printf '%s\n' "$last" "$floor" | grep -qEv '^[0-9]{1,9}$'; then
  die 'invalid round state'
fi
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
