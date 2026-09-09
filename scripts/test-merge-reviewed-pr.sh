#!/bin/sh
# Offline self-test for scripts/merge-reviewed-pr.sh's union gate (GH-1057).
#
# The defect this covers: the script selected the LATEST trusted review pinned
# to the head and asked only that one for its verdict, so an earlier standing
# `Changes Requested` stopped mattering the moment a later `LGTM (P0=0, P1=0)`
# existed on the same SHA — exactly the failure the union rule (GH-742,
# REVIEW.md §8) is written to prevent, on the operator's merge entrypoint.
#
# What is under test here is the WIRING, not the rule. The union rule has one
# implementation, `edda review gate` (GH-769), reached over §7 verdict comments
# by `edda review deliver` (GH-1030); both are covered by that crate's own
# tests. This fixture stubs `edda` and asserts the script asks it about the
# right head, refuses whatever it answers short of a union pass, and leaves the
# pre-existing accept path byte-identical.
#
# Fully offline: `gh` and `edda` are stubbed on PATH ahead of the real ones and
# every invocation is recorded, so no network, no repository and no GitHub
# credentials are touched. `jq` is the real one — the script under test needs
# it, and so does no assertion here.
#
# usage: sh scripts/test-merge-reviewed-pr.sh
set -eu

cd "$(git rev-parse --show-toplevel)"
script=scripts/merge-reviewed-pr.sh

sh -n "$script" || { echo "FAIL: sh -n $script" >&2; exit 1; }
sh -n "$0" || { echo "FAIL: sh -n $0" >&2; exit 1; }

work=$(mktemp -d "${TMPDIR:-/tmp}/test-merge-reviewed-pr.XXXXXX")
cleanup() { rm -rf "$work"; }
trap cleanup 0 HUP INT TERM

mkdir -p "$work/bin" "$work/fixtures"

fail() {
    echo "FAIL: $1" >&2
    exit 1
}

PR=4242
HEAD_SHA=aaaaaaaabbbbbbbbccccccccdddddddd11112222

# ── stubs ────────────────────────────────────────────────────────────
#
# `gh` answers exactly the reads the script makes, off fixture files, and
# records every call so a case can prove what was and was not reached. Anything
# else is a hard error rather than a silent success: a stub that shrugs at an
# unexpected call turns a wiring regression into a green test.
#
# GH-993 adds two more calls, made by the verdict-drift.sh subprocess the
# script now runs before anything else: `pr list` (the open-PR enumeration)
# and a second, differently-shaped `pr view --json comments` (per-PR
# comments — distinguished from the script's own `pr view --json
# headRefOid,state` by that flag, since both start with the same two argv
# words). Both apply the caller's real `--jq` filter via real jq, the way
# scripts/fleet/test-verdict-drift.sh's stub already does, so the R23/R993
# regexes in verdict-drift.sh are exercised, not assumed. Unset
# STUB_DRIFT_PRS defaults `pr list` to `[]` (zero open PRs) so cases 1-5,
# which predate GH-993 and set no drift fixture, see a vacuously clean
# fleet and are unaffected; unset STUB_DRIFT_COMMENTS defaults a drift
# `pr view` to empty comments (no verdict — the not-ready shape).

cat >"$work/bin/gh" <<'STUB'
#!/bin/sh
printf 'gh %s\n' "$*" >>"$GH_CALLS"
jqfilter=
prevarg=
for a in "$@"; do
    [ "$prevarg" = "--jq" ] && jqfilter=$a
    prevarg=$a
done
case "${1:-} ${2:-}" in
    "pr view")
        case "$*" in
            *"--json comments"*)
                src=${STUB_DRIFT_COMMENTS:-}
                if [ -n "$src" ] && [ -f "$src" ]; then jq -r "$jqfilter" <"$src"
                else printf '{"comments":[]}\n' | jq -r "$jqfilter"; fi
                ;;
            *) printf '%s\tOPEN\n' "$STUB_HEAD" ;;
        esac
        ;;
    "pr list")
        if [ "${STUB_DRIFT_LIST_FAIL:-0}" = 1 ]; then
            echo "gh stub: simulated pr list failure" >&2
            exit 1
        fi
        src=${STUB_DRIFT_PRS:-}
        if [ -n "$src" ] && [ -f "$src" ]; then jq -r "$jqfilter" <"$src"
        else printf '[]\n' | jq -r "$jqfilter"; fi
        ;;
    "api --paginate") cat "$STUB_COMMENTS" ;;
    "pr checks") exit "${STUB_CHECKS_EXIT:-0}" ;;
    *) echo "gh stub: unexpected invocation: $*" >&2; exit 1 ;;
esac
STUB
chmod +x "$work/bin/gh"

# `edda` stands in for `edda review deliver --json`. STUB_DELIVER names the
# report file; unset means the verb could not judge at all, which the real verb
# signals by exiting 2 with nothing on stdout.
cat >"$work/bin/edda" <<'STUB'
#!/bin/sh
printf 'edda %s\n' "$*" >>"$EDDA_CALLS"
if [ -z "${STUB_DELIVER:-}" ]; then
    echo 'edda review deliver: read comments of PR: gh: not authenticated' >&2
    exit 2
fi
cat "$STUB_DELIVER"
exit "${STUB_DELIVER_EXIT:-0}"
STUB
chmod +x "$work/bin/edda"

PATH="$work/bin:$PATH"
export PATH
STUB_HEAD=$HEAD_SHA
export STUB_HEAD

# ── comment fixtures ─────────────────────────────────────────────────
#
# The REST issue-comments shape the script reads through `gh api --paginate
# --slurp`: an array of pages, each an array of comments carrying
# author_association, body, updated_at and a numeric id.

sed "s/@SHA@/$HEAD_SHA/g" >"$work/fixtures/changes-then-lgtm.json" <<'JSON'
[[
  {
    "id": 111,
    "author_association": "OWNER",
    "updated_at": "2026-09-08T10:00:00Z",
    "body": "## Code Review: Round 1 — PR #4242 @ @SHA@\n\n- escalations: none\n\n### Verdict\n\nChanges Requested, P0=0, P1=1\n"
  },
  {
    "id": 222,
    "author_association": "OWNER",
    "updated_at": "2026-09-08T11:00:00Z",
    "body": "## Code Review: Round 2 — PR #4242 @ @SHA@\n\n- escalations: none\n\n### Verdict\n\nLGTM (P0=0, P1=0)\n"
  }
]]
JSON

sed "s/@SHA@/$HEAD_SHA/g" >"$work/fixtures/lgtm-only.json" <<'JSON'
[[
  {
    "id": 222,
    "author_association": "OWNER",
    "updated_at": "2026-09-08T11:00:00Z",
    "body": "## Code Review: Round 1 — PR #4242 @ @SHA@\n\n- escalations: none\n\n### Verdict\n\nLGTM (P0=0, P1=0)\n"
  }
]]
JSON

# ── deliver reports ──────────────────────────────────────────────────
#
# The `--json` shape of `edda review deliver` (crates/edda-cli/src/cmd_review/
# deliver.rs): `status` is the union state — success / failure / error — and
# `malformed` lists §7 comments the union could not read.

cat >"$work/fixtures/deliver-failure.json" <<'JSON'
{"pr":4242,"status":"failure","verdicts":["Changes Requested\t0\t1","LGTM\t0\t0"],"malformed":[],"shadow":[],"exit_code":0}
JSON

cat >"$work/fixtures/deliver-success.json" <<'JSON'
{"pr":4242,"status":"success","verdicts":["LGTM\t0\t0"],"malformed":[],"shadow":[],"exit_code":0}
JSON

cat >"$work/fixtures/deliver-malformed.json" <<'JSON'
{"pr":4242,"status":"success","verdicts":["LGTM\t0\t0"],"malformed":["5573431960"],"shadow":[],"exit_code":3}
JSON

# ── harness ──────────────────────────────────────────────────────────

case_no=0
run_case() {
    case_no=$((case_no + 1))
    GH_CALLS=$work/gh-calls-$case_no
    EDDA_CALLS=$work/edda-calls-$case_no
    export GH_CALLS EDDA_CALLS
    : >"$GH_CALLS"
    : >"$EDDA_CALLS"
    set +e
    out=$(sh "$script" "$PR" 2>"$work/err-$case_no")
    code=$?
    set -e
    err=$(cat "$work/err-$case_no")
}

# ── case 1: an earlier Changes Requested under a later LGTM is refused ──
#
# doneWhen bullet 1. The latest trusted review pinned to this head IS a
# qualifying LGTM — every pre-existing check passes — so this case fails
# against the latest-review-wins script and can only pass once the union gate
# stands.

STUB_COMMENTS=$work/fixtures/changes-then-lgtm.json
STUB_DELIVER=$work/fixtures/deliver-failure.json
export STUB_COMMENTS STUB_DELIVER
run_case
[ "$code" -ne 0 ] || fail "case 1: a standing Changes Requested was merged over: $out"
case $err in
    *union*) : ;;
    *) fail "case 1: the refusal must name the union, got: $err" ;;
esac
grep -qF "review deliver --pr $PR --sha $HEAD_SHA --json" "$EDDA_CALLS" \
    || fail "case 1: the gate was not asked about this head: $(cat "$EDDA_CALLS")"
if grep -q 'pr checks' "$GH_CALLS"; then
    fail "case 1: required checks were queried after the union already refused"
fi

# ── case 2: a lone qualifying LGTM still merges — accept path unchanged ─
#
# doneWhen bullet 2. Same stdout line, same exit code, and the required-checks
# call still made: the union gate is an ADDITIONAL refusal, never a
# replacement for anything above it.

STUB_COMMENTS=$work/fixtures/lgtm-only.json
STUB_DELIVER=$work/fixtures/deliver-success.json
export STUB_COMMENTS STUB_DELIVER
run_case
[ "$code" -eq 0 ] || fail "case 2: the accept path broke (exit $code): $err"
[ "$out" = "review accepted: PR #$PR @ $HEAD_SHA" ] \
    || fail "case 2: accept output changed: $out"
grep -qF "pr checks $PR" "$GH_CALLS" \
    || fail "case 2: --required checks were skipped: $(cat "$GH_CALLS")"

# ── case 3: a gate that cannot judge refuses, it does not wave through ──
#
# `edda review deliver` exits 2 with no JSON when it cannot read the comments
# at all. An unreadable answer is not an approval.

STUB_COMMENTS=$work/fixtures/lgtm-only.json
unset STUB_DELIVER
run_case
[ "$code" -ne 0 ] || fail "case 3: an unjudgeable head was accepted: $out"
case $err in
    *union*) : ;;
    *) fail "case 3: expected a union-gate refusal, got: $err" ;;
esac

# ── case 4: a verdict comment the union could not read refuses ─────────
#
# A §7 heading below line 1 is a round the union never saw (GH-917). The
# reported union is `success` here precisely because the blocking round is
# invisible to it, which is why `malformed` has to be its own refusal.

STUB_COMMENTS=$work/fixtures/lgtm-only.json
STUB_DELIVER=$work/fixtures/deliver-malformed.json
export STUB_COMMENTS STUB_DELIVER
run_case
[ "$code" -ne 0 ] || fail "case 4: a malformed verdict comment was ignored: $out"
case $err in
    *malformed*) : ;;
    *) fail "case 4: expected a malformed-comment refusal, got: $err" ;;
esac

# ── case 5: exit code is not the gate, but it is not silent either ─────
#
# `edda review deliver` exits by delivery outcome (0/1/3), not by the union
# (Round 1 review, P1-2 — the exit code was previously discarded with no
# signal at all). Here deliver exits 1 (partially delivered) while the union
# itself still reads `success`: the merge must still proceed — the union
# `.status` field alone decides the refusal — but a warning naming the
# nonzero exit must land on stderr so the operator can see the write did not
# fully land.

STUB_COMMENTS=$work/fixtures/lgtm-only.json
STUB_DELIVER=$work/fixtures/deliver-success.json
STUB_DELIVER_EXIT=1
export STUB_COMMENTS STUB_DELIVER STUB_DELIVER_EXIT
run_case
[ "$code" -eq 0 ] || fail "case 5: a nonzero deliver exit with a union pass must not block the merge (exit $code): $err"
[ "$out" = "review accepted: PR #$PR @ $HEAD_SHA" ] \
    || fail "case 5: accept output changed: $out"
grep -qF "pr checks $PR" "$GH_CALLS" \
    || fail "case 5: --required checks were skipped: $(cat "$GH_CALLS")"
case $err in
    *warning*) : ;;
    *) fail "case 5: expected a warning on stderr naming the failed write, got: $err" ;;
esac

# ── case 6: an unrelated open PR with no verdict refuses everything (GH-993) ──
#
# The fleet-wide drift gate (scripts/fleet/verdict-drift.sh) now runs before
# any of the checks above. It is deliberately whole-open-set, not scoped to
# $PR — the same scope daily-digest.sh already shares with it via
# EDDA_OPEN_PR_LIMIT — so an unrelated PR's missing verdict blocks this PR's
# --check/--merge too, and blocks it before the union gate or required
# checks are even asked.

DRIFT_SHA=dddddddddddddddddddddddddddddddddddddddd
printf '[{"number":9001,"headRefOid":"%s","baseRefName":"main"}]\n' "$DRIFT_SHA" >"$work/fixtures/drift-dirty-prs.json"
STUB_DRIFT_PRS=$work/fixtures/drift-dirty-prs.json
export STUB_DRIFT_PRS
run_case
unset STUB_DRIFT_PRS
[ "$code" -ne 0 ] || fail "case 6: an unrelated PR with no verdict did not block: $out"
case $err in
    *verdict-drift*) : ;;
    *) fail "case 6: expected a verdict-drift refusal, got: $err" ;;
esac
if grep -q 'api --paginate' "$GH_CALLS"; then
    fail "case 6: the union gate was reached after drift already refused: $(cat "$GH_CALLS")"
fi
if grep -q 'pr checks' "$GH_CALLS"; then
    fail "case 6: required checks were queried after drift already refused: $(cat "$GH_CALLS")"
fi

# ── case 7: verdict-drift.sh itself failing to read also refuses (GH-993) ──
#
# The fail-closed proof the issue requires: not only "drift was found"
# (case 6) but "the check that looks for drift could not even run" must
# block too — exit 2, not only exit 1 — or a broken read would wave every
# merge through clean.

STUB_DRIFT_LIST_FAIL=1
export STUB_DRIFT_LIST_FAIL
run_case
unset STUB_DRIFT_LIST_FAIL
[ "$code" -ne 0 ] || fail "case 7: a failed drift read did not block: $out"
case $err in
    *verdict-drift*) : ;;
    *) fail "case 7: expected a verdict-drift refusal, got: $err" ;;
esac
if grep -q 'api --paginate' "$GH_CALLS"; then
    fail "case 7: the union gate was reached after the drift read failed: $(cat "$GH_CALLS")"
fi

# ── case 8: a genuinely clean, non-vacuous drift state does not block ──────
#
# Cases 1-5 all run under the drift gate's vacuous default (zero open PRs
# in STUB_DRIFT_PRS) — clean, but the empty-set kind of clean the issue's
# own doneWhen calls out as needing a real fixture instead. This is that
# fixture: one populated, healthy open PR, distinct from $PR itself, with a
# real LGTM pinned to its head. The accept path stays byte-identical.
#
# `authorAssociation":"OWNER"` is required on this comment since Round 1
# review's P1 fix: verdict-drift.sh now trusts only OWNER/MEMBER/
# COLLABORATOR comments (scripts/fleet/verdict-drift.sh), mirroring the
# union gate's own filter five lines below in this script. The trust-filter
# logic itself is unit-tested in scripts/fleet/test-verdict-drift.sh (cases
# 16-18); this fixture only needs to stay trusted so this wiring case keeps
# proving what it always proved.

OTHER_SHA=eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee
printf '[{"number":9002,"headRefOid":"%s","baseRefName":"main","mergeable":"MERGEABLE"}]\n' "$OTHER_SHA" >"$work/fixtures/drift-clean-prs.json"
sed "s/@SHA@/$OTHER_SHA/g" >"$work/fixtures/drift-clean-comments.json" <<'JSON'
{"comments":[{"body":"## Code Review: Round 1 — PR #9002 @ @SHA@\n\n### Verdict\nLGTM (P0=0, P1=0)","authorAssociation":"OWNER"}]}
JSON
STUB_DRIFT_PRS=$work/fixtures/drift-clean-prs.json
STUB_DRIFT_COMMENTS=$work/fixtures/drift-clean-comments.json
STUB_COMMENTS=$work/fixtures/lgtm-only.json
STUB_DELIVER=$work/fixtures/deliver-success.json
export STUB_DRIFT_PRS STUB_DRIFT_COMMENTS STUB_COMMENTS STUB_DELIVER
run_case
unset STUB_DRIFT_PRS STUB_DRIFT_COMMENTS
[ "$code" -eq 0 ] || fail "case 8: a healthy, non-vacuous open set was refused (exit $code): $err"
[ "$out" = "review accepted: PR #$PR @ $HEAD_SHA" ] \
    || fail "case 8: accept output changed: $out"
grep -qF "pr checks $PR" "$GH_CALLS" \
    || fail "case 8: --required checks were skipped: $(cat "$GH_CALLS")"

echo "PASS scripts/test-merge-reviewed-pr.sh ($case_no cases)"
