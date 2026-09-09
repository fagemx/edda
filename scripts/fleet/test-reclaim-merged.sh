#!/bin/sh
# Offline self-test for scripts/fleet/reclaim-merged.sh (GH-1009).
#
# What is under test is the EXCLUSION boundary, not the removal: R7 says a
# reclaimer that is merely usually right is a data-loss tool, so the cases
# below are written from the keep side. A dirty worktree, an open PR, a
# worktree nested in the main checkout, a local ref that has moved past the
# merged head and a branch with no PR at all must survive `--apply` intact,
# and the one item that satisfies every condition must actually go.
#
# Fully offline. `gh` is stubbed on PATH ahead of the real one and answers the
# single `gh pr list` the script makes off a fixture table, so no network and
# no GitHub credentials are touched. `git` is the real one, driving a
# throwaway repository with a bare `origin` beside it — worktree registration,
# `git worktree remove`'s own refusals and `git push --delete` are the
# behavior under test, and a stub for them would test nothing.
#
# usage: sh scripts/fleet/test-reclaim-merged.sh
set -eu

cd "$(git rev-parse --show-toplevel)"
script=$(pwd)/scripts/fleet/reclaim-merged.sh

sh -n "$script" || { echo "FAIL: sh -n $script" >&2; exit 1; }
sh -n "$0" || { echo "FAIL: sh -n $0" >&2; exit 1; }

work=$(mktemp -d "${TMPDIR:-/tmp}/test-reclaim-merged.XXXXXX")
work=$(cd "$work" && pwd -P)
cleanup() { chmod -R u+w "$work" 2>/dev/null || true; rm -rf "$work"; }
trap cleanup 0
trap 'cleanup; exit 130' HUP INT TERM

TAB=$(printf '\t')

fail() {
    echo "FAIL: $1" >&2
    [ ! -f "$work/out" ] || { echo '--- last run ---' >&2; cat "$work/out" >&2; }
    exit 1
}

# ── the repository under test ────────────────────────────────────────

mkdir -p "$work/bin"
git init --quiet --bare "$work/origin.git"
git init --quiet "$work/repo"
repo=$work/repo
cd "$repo"
git symbolic-ref HEAD refs/heads/main
git config user.email test@example.com
git config user.name 'reclaim test'
git config commit.gpgsign false
echo seed >seed.txt
git add seed.txt
git commit --quiet -m seed
base=$(git rev-parse HEAD)
git remote add origin "$work/origin.git"

for b in merged-clean merged-dirty open-clean nested-merged moved no-pr; do
    git branch "$b" main
done

# `moved` is the ref that outran its own merged PR: one extra commit that no
# PR ever saw, which `refs/pull/N/head` therefore does not preserve.
git checkout --quiet moved
echo later >later.txt
git add later.txt
git commit --quiet -m 'work the PR never saw'
moved_tip=$(git rev-parse HEAD)
git checkout --quiet main

git push --quiet origin main merged-clean merged-dirty open-clean moved

# Four linked worktrees. `nested` sits inside the main checkout on purpose —
# that is the shape of the agent worktrees under `.claude/worktrees/`, and
# removing one of those reaches into the operator's own checkout.
git worktree add --quiet "$work/wt-merged-clean" merged-clean
git worktree add --quiet "$work/wt-merged-dirty" merged-dirty
git worktree add --quiet "$work/wt-open" open-clean
git worktree add --quiet "$repo/nested" nested-merged
echo scratch >"$work/wt-merged-dirty/uncommitted.txt"

# A merged PR whose local ref is already gone but whose remote branch is not:
# the half of the authority that iterating local refs alone never sees.
git push --quiet origin "merged-clean:remote-only"

# Paths are compared against what `git worktree list` prints, and git spells a
# Windows path `C:/...` where the shell spells the same directory `/c/...`.
# Asking git for each one keeps both sides in git's own spelling.
g_repo=$(cd "$repo" && git rev-parse --show-toplevel)
g_clean=$(cd "$work/wt-merged-clean" && git rev-parse --show-toplevel)
g_dirty=$(cd "$work/wt-merged-dirty" && git rev-parse --show-toplevel)
g_open=$(cd "$work/wt-open" && git rev-parse --show-toplevel)
g_nested=$(cd "$repo/nested" && git rev-parse --show-toplevel)

# ── the PR table the stub serves ─────────────────────────────────────
#
# Same five fields the script asks `gh pr list --jq` for: head branch, number,
# state, head oid, merge oid. `moved`'s head oid is deliberately the base
# commit — its local ref has since advanced.
squash=1111111111111111111111111111111111111111
{
    printf 'merged-clean\t101\tMERGED\t%s\t%s\n' "$base" "$squash"
    printf 'merged-dirty\t102\tMERGED\t%s\t%s\n' "$base" "$squash"
    printf 'open-clean\t103\tOPEN\t%s\t-\n' "$base"
    printf 'nested-merged\t104\tMERGED\t%s\t%s\n' "$base" "$squash"
    printf 'moved\t105\tMERGED\t%s\t%s\n' "$base" "$squash"
    printf 'remote-only\t106\tMERGED\t%s\t%s\n' "$base" "$squash"
} >"$work/prs.tsv"

cat >"$work/bin/gh" <<'STUB'
#!/bin/sh
printf 'gh %s\n' "$*" >>"$GH_CALLS"
case "${1:-} ${2:-}" in
    "pr list") cat "$STUB_PRS" ;;
    *) echo "gh stub: unexpected invocation: $*" >&2; exit 1 ;;
esac
STUB
chmod +x "$work/bin/gh"
PATH="$work/bin:$PATH"
export PATH
STUB_PRS=$work/prs.tsv
export STUB_PRS

# ── harness ──────────────────────────────────────────────────────────

case_no=0
run() {
    case_no=$((case_no + 1))
    GH_CALLS=$work/gh-calls-$case_no
    export GH_CALLS
    : >"$GH_CALLS"
    set +e
    (cd "$repo" && sh "$script" "$@") >"$work/out" 2>"$work/err"
    code=$?
    set -e
}

# Rows are TSV: verdict, kind, item, branch, pr, state, tree/sha, reason.
row() { awk -F"$TAB" -v k="$1" -v i="$2" '$2 == k && $3 == i { print }' "$work/out"; }

# Verdict is the first field and reason the last, so both come out of the one
# `row` call by parameter expansion. This matters: on the workstation this
# runs on a process spawn measures ~2.7s, and a three-spawn assertion helper
# called thirty times is two minutes of pure harness.
expect() { # kind item verdict reason-substring label
    got=$(row "$1" "$2")
    [ -n "$got" ] || fail "$5: no row for $1 $2"
    got_v=${got%%"$TAB"*}
    got_r=${got##*"$TAB"}
    [ "$got_v" = "$3" ] || fail "$5: $1 $2 is $got_v, expected $3 (reason: $got_r)"
    case $got_r in
        *"$4"*) : ;;
        *) fail "$5: $1 $2 reason is '$got_r', expected to mention '$4'" ;;
    esac
}

# ── case 1: the dry run classifies, and removes nothing ──────────────
#
# doneWhen bullet 1: every candidate listed with its worktree, branch, PR,
# state and tree state, strictly scoped to merged PRs.

wt_before=$(cd "$repo" && git worktree list | wc -l)
run
[ "$code" -eq 0 ] || fail "case 1: dry run exited $code: $(cat "$work/err")"
grep -q 'pr list --state all' "$GH_CALLS" || fail "case 1: the PR table was never read"

expect worktree "$g_clean" RECLAIM 'pr-merged' 'case 1'
expect worktree "$g_dirty" KEEP 'tree-dirty' 'case 1'
expect worktree "$g_open" KEEP 'pr-OPEN' 'case 1'
expect worktree "$g_nested" KEEP 'nested-in-main-checkout' 'case 1'
expect worktree "$g_repo" KEEP 'main-checkout' 'case 1'
expect local-branch moved KEEP 'local-ahead-of-pr' 'case 1'
expect local-branch no-pr KEEP 'no-pr' 'case 1'
expect local-branch merged-dirty KEEP 'checked-out' 'case 1'
expect local-branch nested-merged KEEP 'checked-out' 'case 1'
expect local-branch main KEEP 'default-branch' 'case 1'
# The dry run has to predict --apply: this branch is checked out only in the
# worktree the same run already listed as RECLAIM.
expect local-branch merged-clean RECLAIM 'pr-merged' 'case 1'
expect remote-branch origin/moved KEEP 'remote-moved-since-merge' 'case 1'
expect remote-branch origin/open-clean KEEP 'pr-OPEN' 'case 1'
expect remote-branch origin/merged-dirty KEEP 'local-kept' 'case 1'
expect remote-branch origin/merged-clean RECLAIM 'pr-merged' 'case 1'
expect remote-branch origin/remote-only RECLAIM 'pr-merged' 'case 1'
[ -z "$(row local-branch remote-only)" ] \
    || fail 'case 1: a local row was invented for a branch that exists only on origin'

# The dirty worktree's tree state has to be READ, not inferred from the
# verdict: a row that says `clean` and keeps for some other reason would pass
# a verdict-only assertion and still be the bug this test exists to catch.
[ "$(row worktree "$g_dirty" | cut -f7)" = dirty ] \
    || fail "case 1: the dirty worktree was not reported dirty"

[ "$(cd "$repo" && git worktree list | wc -l)" -eq "$wt_before" ] \
    || fail 'case 1: a dry run removed a worktree'
[ -d "$work/wt-merged-clean" ] || fail 'case 1: a dry run removed the reclaim candidate'

# ── case 2: --protect keeps an otherwise-qualifying item ─────────────

run --protect wt-merged-clean
expect worktree "$g_clean" KEEP 'protected' 'case 2'
[ -d "$work/wt-merged-clean" ] || fail 'case 2: a protected worktree was removed'

# ── case 3: --apply removes exactly the RECLAIM set ──────────────────
#
# doneWhen bullets 2 and 3: receipts for what went, and nothing dirty or
# open-PR touched.

run --apply
[ "$code" -eq 0 ] || fail "case 3: --apply exited $code: $(cat "$work/err")"

[ ! -d "$work/wt-merged-clean" ] || fail 'case 3: the merged clean worktree survived --apply'
grep -q "reclaimed worktree.*wt-merged-clean.*pr=#101.*squash=$squash" "$work/out" \
    || fail 'case 3: no receipt for the reclaimed worktree'

[ -d "$work/wt-merged-dirty" ] || fail 'case 3: a DIRTY worktree was removed'
[ -f "$work/wt-merged-dirty/uncommitted.txt" ] || fail 'case 3: uncommitted work was destroyed'
[ -d "$work/wt-open" ] || fail 'case 3: an OPEN-PR worktree was removed'
[ -d "$repo/nested" ] || fail 'case 3: a worktree nested in the main checkout was removed'
[ -d "$repo/.git" ] || fail 'case 3: the main checkout was removed'

cd "$repo"
has_local() { git show-ref --verify --quiet "refs/heads/$1"; }
has_remote() { [ -n "$(git ls-remote --heads origin "$1")" ]; }

if has_local merged-clean; then fail 'case 3: the merged branch survived --apply'; fi
grep -q "reclaimed local branch.*merged-clean.*pr=#101" "$work/out" \
    || fail 'case 3: no receipt for the reclaimed local branch'
has_local moved || fail 'case 3: a branch ahead of its PR was deleted'
has_local no-pr || fail 'case 3: a branch with no PR was deleted'
has_local merged-dirty || fail 'case 3: a checked-out branch was deleted'

if has_remote merged-clean; then fail 'case 3: the merged remote branch survived --apply'; fi
grep -q "reclaimed remote branch.*origin/merged-clean.*pr=#101" "$work/out" \
    || fail 'case 3: no receipt for the reclaimed remote branch'
if has_remote remote-only; then fail 'case 3: a remote-only merged branch survived --apply'; fi
has_remote moved || fail 'case 3: a remote branch that moved past its merged head was deleted'
has_remote open-clean || fail 'case 3: an OPEN-PR remote branch was deleted'
has_remote merged-dirty || fail 'case 3: the remote of a kept dirty lane was deleted'

grep -q 'reclaim-merged: after' "$work/out" || fail 'case 3: no before/after snapshot line'

# ── case 4: a second --apply is a no-op ──────────────────────────────
#
# Everything left is excluded for a reason that does not decay, so a repeated
# run must find nothing — a reclaimer that creeps outward on each pass is the
# same defect as one that over-reaches on the first.

run --apply
[ "$code" -eq 0 ] || fail "case 4: second --apply exited $code: $(cat "$work/err")"
if grep -q '^RECLAIM' "$work/out"; then fail 'case 4: a second pass found new candidates'; fi
[ -d "$work/wt-merged-dirty" ] && [ -d "$work/wt-open" ] && [ -d "$repo/nested" ] \
    || fail 'case 4: a second pass removed a kept worktree'

# ── case 5: an unreadable PR table reclaims nothing ──────────────────
#
# Without PR state every item's state is unknown, and unknown is not merged.

STUB_PRS=$work/does-not-exist
export STUB_PRS
run --apply
[ "$code" -eq 3 ] || fail "case 5: expected exit 3 on an unreadable PR table, got $code"
if grep -q 'reclaimed' "$work/out"; then fail 'case 5: something was removed without PR state'; fi

# ── case 6: a batch the remote only PARTIALLY accepts is verified, not
#            trusted by the push's exit code ─────────────────────────
#
# `delete_batched` sends every remote reclaim of one run in ONE `git push
# --delete`. A real remote can reject one ref out of that batch — a branch
# protection rule, a ref that moved server-side — while still accepting the
# rest of the same push, and the combined command exits non-zero either way.
# That is exactly why the receipt comes from re-reading `git ls-remote`
# afterwards instead of the push's exit code: an `update` hook that rejects
# one ref out of two, offline, is the stand-in for that server behavior. Two
# fresh remote-only branches keep this batch isolated from every branch the
# earlier cases already resolved.

git branch partial-a main
git branch partial-b main
git push --quiet origin partial-a partial-b
git branch -D partial-a partial-b >/dev/null

{
    cat "$work/prs.tsv"
    printf 'partial-a\t201\tMERGED\t%s\t%s\n' "$base" "$squash"
    printf 'partial-b\t202\tMERGED\t%s\t%s\n' "$base" "$squash"
} >"$work/prs-case6.tsv"
STUB_PRS=$work/prs-case6.tsv
export STUB_PRS

cat >"$work/origin.git/hooks/update" <<'HOOK'
#!/bin/sh
case "$1" in
    refs/heads/partial-b) echo "rejected by hook: $1" >&2; exit 1 ;;
esac
exit 0
HOOK
chmod +x "$work/origin.git/hooks/update"

run --apply
[ "$code" -eq 0 ] || fail "case 6: --apply exited $code: $(cat "$work/err")"

if has_remote partial-a; then fail 'case 6: the ref the hook ACCEPTED survived the batch'; fi
grep -q "reclaimed remote branch.*origin/partial-a.*pr=#201" "$work/out" \
    || fail 'case 6: no receipt for the ref the hook accepted'

has_remote partial-b || fail 'case 6: a ref the hook REJECTED was deleted anyway'
grep -q "KEPT remote branch.*origin/partial-b.*still present after delete" "$work/err" \
    || fail 'case 6: the rejected ref was not reported KEPT'
if grep -q "reclaimed remote branch.*origin/partial-b" "$work/out"; then
    fail 'case 6: a ref the remote REJECTED was receipted as reclaimed — exit-code trust, not verification'
fi

echo 'PASS scripts/fleet/test-reclaim-merged.sh'
