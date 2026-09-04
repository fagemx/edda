#!/bin/sh
# Regression tests for the GH-782 GitHub issue claim convention.
set -eu
cd "$(git rev-parse --show-toplevel)"
script=scripts/fleet-claim-issue.sh
sh -n "$script"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/bin"
cat > "$work/bin/gh" <<'STUB'
#!/bin/sh
orig_args="$*"
mode=$1
[ "$mode" != issue ] || mode=$2
[ "${GH_FAIL:-}" != "$mode" ] || { echo 'fixture gh failure' >&2; exit 1; }
jq_expr=''
while [ $# -gt 0 ]; do
    if [ "$1" = --jq ]; then shift; jq_expr=$1; fi
    shift
done
case "$mode" in
    view) jq -r "$jq_expr" "$GH_FIXTURE" ;;
    pr) jq -r "$jq_expr" "$GH_PRS" ;;
    comment | edit) printf '%s\n' "$orig_args" >> "$GH_CALLS" ;;
    *) echo "unexpected gh: $orig_args" >&2; exit 1 ;;
esac
STUB
chmod +x "$work/bin/gh"
calls="$work/calls.log"
out="$work/out.txt"
fail() { echo "FAIL: $1" >&2; exit 1; }

run_case() {
    expected=$1; fixture=$2; prs=$3; shift 3
    printf '%s' "$fixture" > "$work/issue.json"
    printf '%s' "$prs" > "$work/prs.json"
    : > "$calls"
    rc=0
    GH_FIXTURE="$work/issue.json" GH_PRS="$work/prs.json" GH_CALLS="$calls" \
        PATH="$work/bin:$PATH" sh "$script" "$@" > "$out" 2>&1 || rc=$?
    [ "$rc" -eq "$expected" ] || fail "expected $expected, got $rc: $(cat "$out")"
}
no_write() { [ ! -s "$calls" ] || fail "unexpected writes: $(cat "$calls")"; }
UNCLAIMED='{"labels":[{"name":"fleet:ready"}],"comments":[]}'
ROUTED='{"labels":[{"name":"lane:feature"},{"name":"lane:4090"}],"comments":[]}'
OTHER='{"comments":[{"body":"taking: 4090/worker-2 at t","createdAt":"2026-09-02T06:30:00Z"}]}'
SELF='{"comments":[{"body":"taking: 4090/worker-1 at t","createdAt":"t"}]}'
MERGED='[{"number":716,"state":"MERGED","title":"fix (GH-656)","headRefName":"unrelated"}]'
OPEN='[{"number":800,"state":"OPEN","title":"other","headRefName":"codex/GH656-claim"}]'

run_case 0 "$ROUTED" '[]' --check 656 4090/worker-1
no_write
echo 'ok routing labels do not carry claims (RED on base)'
run_case 1 "$OTHER" '[]' --check 656 4090/worker-1
no_write
grep -q '4090/worker-2' "$out" || fail 'same-machine refusal must name full identity'
grep -q '2026-09-02T06:30:00Z' "$out" || fail 'refusal must retain timestamp'
echo 'ok same-machine different role refuses'
run_case 1 '{"comments":[{"body":"taking: 4090"}]}' '[]' 656 4090/worker-1
no_write
echo 'ok legacy bare-token claim is foreign'
run_case 0 "$UNCLAIMED" '[]' 656 4090/worker-1
grep -q 'issue comment 656 --body taking: 4090/worker-1 at ' "$calls" || fail 'claim comment missing'
grep -q 'issue edit 656 --add-label fleet:claimed --remove-label fleet:ready --add-assignee @me' "$calls" || fail 'queue/assignee edit missing'
echo 'ok write mode claims and updates queue/assignee'
run_case 0 "$SELF" '[]' 656 4090/worker-1
if grep -q 'issue comment' "$calls"; then fail 'self claim adds duplicate comment'; fi
grep -q 'issue edit' "$calls" || fail 'self claim must repair a partial label write'
run_case 0 "$SELF" '[]' 656 4090/worker-1 --check
no_write
echo 'ok self claim is idempotent and check is read-only'
run_case 1 "$UNCLAIMED" "$MERGED" --check 656 4090/worker-1
no_write
grep -q '#716 (merged)' "$out" || fail 'merged delivery number/state missing'
grep -q 'drop fleet:ready' "$out" || fail 'merged delivery guidance missing'
run_case 1 "$UNCLAIMED" "$OPEN" --check 656 4090/worker-1
no_write
grep -q 'open PR #800' "$out" || fail 'open PR number/state missing'
echo 'ok merged title and open branch matches block dispatch'
run_case 0 "$UNCLAIMED" '[{"number":716,"state":"CLOSED","title":"GH-656","headRefName":"gh656"}]' --check 656 4090/worker-1
run_case 0 "$UNCLAIMED" '[{"number":716,"state":"OPEN","title":"GH-6560","headRefName":"gh6560"}]' --check 656 4090/worker-1
no_write
echo 'ok closed-unmerged and different issue numbers do not block'
run_case 0 '{"comments":[{"body":"remember: taking: x/y\n  taking:   \n"}]}' '[]' --check 656 4090/worker-1
run_case 1 '{"comments":[{"body":"taking: 4090/worker-1\n  taking:   4090/worker-2"}]}' '[]' --check 656 4090/worker-1
no_write
echo 'ok line parsing and multiple markers match Rust guard'
for invalid in 4090 '4090/worker 1' 4090//x /worker-1 4090/; do
    run_case 2 "$UNCLAIMED" '[]' --check 656 "$invalid"
    no_write
done
run_case 2 "$UNCLAIMED" '[]'
echo 'ok invalid identity and usage exit 2'
for mode in pr view comment edit; do
    GH_FAIL=$mode; export GH_FAIL
    run_case 2 "$UNCLAIMED" '[]' 656 4090/worker-1
    if [ "$mode" != edit ]; then no_write; fi
done
unset GH_FAIL
run_case 2 "$UNCLAIMED" 'not json' --check 656 4090/worker-1
no_write
echo 'ok gh failures and malformed PR response fail closed with exit 2'
echo 'all fleet-claim-issue.sh self-tests passed'
