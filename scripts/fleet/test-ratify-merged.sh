#!/bin/sh
# Offline self-test for scripts/fleet/ratify-merged.sh (GH-764, GH-671).
#
# Two halves. Cases 1-7 cover ratification: `edda` is stubbed and every
# invocation recorded, and `gh` is never reached because each case passes
# --body-file/--sha/--title. Cases 8-11 cover the projection step, which
# branches, commits and pushes for real — inside throwaway repos, with `edda`,
# `gh` and a pass-through `git` stubbed on PATH. Writes only under its temp
# dir, so it is safe in CI and on a workstation with a live ledger.
#
# usage: sh scripts/fleet/test-ratify-merged.sh
set -eu

# Cases 1-7 must never reach the projection step at all. The projection cases
# drop this, but only inside a throwaway repo — see run_hook().
EDDA_PROJECTION=off; export EDDA_PROJECTION

cd "$(git rev-parse --show-toplevel)"
hook=scripts/fleet/ratify-merged.sh
# The projection cases run the hook from inside another repo, where the
# relative path no longer resolves.
hook_abs=$(pwd)/$hook

sh -n "$hook" || { echo "FAIL: sh -n $hook" >&2; exit 1; }
sh -n "$0" || { echo "FAIL: sh -n $0" >&2; exit 1; }

work=$(mktemp -d "${TMPDIR:-/tmp}/test-ratify-merged.XXXXXX")
cleanup() { rm -rf "$work"; }
trap cleanup 0 HUP INT TERM

mkdir -p "$work/bin" "$work/fixtures"

fail() {
    echo "FAIL: $1" >&2
    exit 1
}

SHA=03c604ffea4b2a1731b7866e7f701374eb03b156

# ── fixtures ─────────────────────────────────────────────────────────

cat >"$work/fixtures/one-key.md" <<'MD'
closes #764

Decision: ratify.evidence-form

Body prose that also says the word decision: nowhere near the line start.
MD

cat >"$work/fixtures/two-keys.md" <<'MD'
closes #764

Issue: #764
Decision: ratify.evidence-form, ratify.rule-form
MD

cat >"$work/fixtures/no-line.md" <<'MD'
closes #999

An ordinary PR that implements no decision at all.
Someone quoting "Decision: not.a.key" mid-sentence must not count.
MD

# ── stub edda: records args, and fails for one known key ─────────────

cat >"$work/bin/edda" <<'STUB'
#!/bin/sh
printf '%s\n' "$*" >>"$EDDA_CALLS"
for a in "$@"; do
    case $a in
        missing.key) echo "no active decision for key 'missing.key'" >&2; exit 1 ;;
        already.binding) echo "'already.binding' is already binding (by operator) — nothing written."; exit 0 ;;
    esac
done
echo "Ratified — now binding."
STUB
chmod +x "$work/bin/edda"
PATH="$work/bin:$PATH"
export PATH

# ── case 1: one Decision key → one typed evidence ratify ─────────────

EDDA_CALLS=$work/calls-1
export EDDA_CALLS
: >"$EDDA_CALLS"
out=$(sh "$hook" 764 --sha "$SHA" --title "feat(cli): typed ratify" \
    --body-file "$work/fixtures/one-key.md") || fail "case 1 exited non-zero"
grep -q "ratify.evidence-form -> exit 0" <<EOF || fail "case 1: missing log line: $out"
$out
EOF
calls=$(wc -l <"$EDDA_CALLS")
[ "$calls" -eq 1 ] || fail "case 1: expected 1 edda call, got $calls"
grep -qF "ratify ratify.evidence-form --evidence pr#764@$SHA --note feat(cli): typed ratify" \
    "$EDDA_CALLS" || fail "case 1: wrong edda args: $(cat "$EDDA_CALLS")"

# ── case 2: no Decision line → nothing ratified, still exit 0 ────────

EDDA_CALLS=$work/calls-2
export EDDA_CALLS
: >"$EDDA_CALLS"
out=$(sh "$hook" 999 --sha "$SHA" --body-file "$work/fixtures/no-line.md") \
    || fail "case 2 exited non-zero"
[ ! -s "$EDDA_CALLS" ] || fail "case 2: edda was called: $(cat "$EDDA_CALLS")"
case $out in
    *"no Decision: line"*) : ;;
    *) fail "case 2: expected a no-op message, got: $out" ;;
esac

# ── case 3: unknown key → edda exits 1, the hook still exits 0 ───────

EDDA_CALLS=$work/calls-3
export EDDA_CALLS
: >"$EDDA_CALLS"
printf 'Decision: missing.key\n' >"$work/fixtures/missing.md"
out=$(sh "$hook" 764 --sha "$SHA" --body-file "$work/fixtures/missing.md") \
    || fail "case 3: a failing ratify must not fail the hook"
case $out in
    *"missing.key -> exit 1"*) : ;;
    *) fail "case 3: exit code not reported: $out" ;;
esac

# ── case 4: already binding → exit 0, reported, no second write ──────

EDDA_CALLS=$work/calls-4
export EDDA_CALLS
: >"$EDDA_CALLS"
printf 'Decision: already.binding\n' >"$work/fixtures/already.md"
out=$(sh "$hook" 764 --sha "$SHA" --body-file "$work/fixtures/already.md") \
    || fail "case 4 exited non-zero"
case $out in
    *"already.binding -> exit 0"*) : ;;
    *) fail "case 4: expected exit 0 for an already-binding key: $out" ;;
esac

# ── case 5: two keys on one line → two ratifies ──────────────────────

EDDA_CALLS=$work/calls-5
export EDDA_CALLS
: >"$EDDA_CALLS"
sh "$hook" 764 --sha "$SHA" --body-file "$work/fixtures/two-keys.md" >/dev/null \
    || fail "case 5 exited non-zero"
calls=$(wc -l <"$EDDA_CALLS")
[ "$calls" -eq 2 ] || fail "case 5: expected 2 edda calls, got $calls"
grep -qF "ratify ratify.rule-form --evidence pr#764@$SHA" "$EDDA_CALLS" \
    || fail "case 5: second key not ratified: $(cat "$EDDA_CALLS")"

# ── case 6: a short SHA is refused before any write ──────────────────

EDDA_CALLS=$work/calls-6
export EDDA_CALLS
: >"$EDDA_CALLS"
if sh "$hook" 764 --sha 03c604f --body-file "$work/fixtures/one-key.md" >/dev/null 2>&1; then
    fail "case 6: an abbreviated SHA must be refused"
fi
[ ! -s "$EDDA_CALLS" ] || fail "case 6: edda ran despite a bad SHA"

# ── case 7: a non-numeric PR is a usage error (exit 2) ───────────────

set +e
sh "$hook" not-a-number --sha "$SHA" --body-file "$work/fixtures/one-key.md" >/dev/null 2>&1
code=$?
set -e
[ "$code" -eq 2 ] || fail "case 7: expected exit 2, got $code"

# ── projection stubs (GH-671) ────────────────────────────────────────

mkdir -p "$work/pbin"

# Resolved while only $work/bin is on PATH, so it is the real binary the
# pass-through stub below re-enters.
REAL_GIT=$(command -v git)
export REAL_GIT

# `edda export md` writes a stamp line that moves on every run over content
# that does not — the shape case 9 turns on. Everything else here reproduces
# what crates/edda-cli/src/cmd_export.rs writes for the one flag this hook
# passes (`--out`): the generated-file banner (`HEADER`, cmd_export.rs:150),
# `<out>/INDEX.md` from `render_index` (cmd_export.rs:272-313), and one
# `<out>/decisions/<domain>.md` per domain from `render_domain`
# (cmd_export.rs:42-52, 153-246) — note the `decisions/` level, which the
# projection's own paths and case 12 both depend on. Simplified: one domain
# holding one unratified decision with fixed field values, and no `notes.md`,
# because `--include-notes` is a flag the hook never passes.
cat >"$work/pbin/edda" <<'STUB'
#!/bin/sh
[ "${1:-}" = export ] || { echo "Ratified — now binding."; exit 0; }
out=docs/decisions
machine=
while [ $# -gt 0 ]; do
    case $1 in
        --out) out=$2; shift 2 ;;
        --machine) machine=$2; shift 2 ;;
        *) shift ;;
    esac
done
# resolve_machine()'s precedence (cmd_export.rs:321-347). Constant for the
# life of a run, so unlike the stamp it never moves the diff on its own.
machine=${machine:-${EDDA_MACHINE:-${COMPUTERNAME:-${HOSTNAME:-unknown}}}}
header='<!-- edda-ledger-export v1 — GENERATED FILE, DO NOT EDIT — SQLite ledger is authoritative -->'
mkdir -p "$out/decisions"
# The bullet shape `render_domain` actually writes: a decision's whole payload
# is `- **Field**: …` lines. Case 12 depends on that, because a diff made only
# of those lines is what the change detector used to throw away.
# EDDA_EXPORT_VALUE moves one decision's value without touching anything else.
{
    printf '%s\n' "$header"
    printf '%s\n' '# Domain: `ledger`' ''
    printf '%s\n' '1 active decision(s), sorted by key.' ''
    printf '%s\n' '## `ledger.cross-machine-projection`' ''
    printf '%s\n' "- **Value**: \`${EDDA_EXPORT_VALUE:-committed-mirror}\`"
    printf '%s\n' '- **Reason**: seeded by the stub'
    printf '%s\n' '- **Branch/ts**: `main` · 2026-01-01T00:00:00Z'
    printf '%s\n' '- **Governance**: unratified (agent)'
    printf '%s\n' '- **Scope**: local'
    printf '%s\n' '- **Authority**: agent'
    printf '%s\n' '- **Reversibility**: medium'
    printf '%s\n' '- **event_id**: `evt_stub`' ''
} >"$out/decisions/ledger.md"
# Half a mirror, then failure: the export is not atomic, so a run that dies
# mid-way is the state case 11 has to survive. `execute()` writes the domain
# files first and INDEX.md last, so this is where a real mid-run death lands.
if [ -n "${EDDA_EXPORT_PARTIAL:-}" ]; then
    echo "edda export md: simulated failure" >&2
    exit 1
fi
{
    printf '%s\n' "$header"
    printf '%s\n' '# Ledger export index' ''
    printf '%s\n' "- **Exported at**: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    printf '%s\n' "- **Exporting machine**: $machine"
    printf '%s\n' '- **Total decisions**: 1' ''
    printf '%s' 'SQLite ledger is the single source of truth. These files exist so '
    printf '%s\n' 'humans can `git diff` a snapshot of decisions and notes.' ''
    printf '%s\n' '## Decisions by domain' ''
    printf '%s\n' '- [`ledger`](./decisions/ledger.md) — 1 decision(s)' ''
} >"$out/INDEX.md"
STUB

cat >"$work/pbin/gh" <<'STUB'
#!/bin/sh
printf '%s\n' "$*" >>"$GH_CALLS"
cat >"$GH_BODY"
STUB

# Pass-through, so the hook drives a real repo. GIT_FAIL_SUBCOMMAND injects
# the one failure case 10 needs: no real-git setup fails `git add` on demand
# without also breaking the recovery that case is measuring.
cat >"$work/pbin/git" <<'STUB'
#!/bin/sh
if [ -n "${GIT_FAIL_SUBCOMMAND:-}" ] && [ "${1:-}" = "$GIT_FAIL_SUBCOMMAND" ]; then
    echo "git $1: simulated failure" >&2
    exit 1
fi
exec "$REAL_GIT" "$@"
STUB

chmod +x "$work/pbin/edda" "$work/pbin/gh" "$work/pbin/git"

# $1 = name. Leaves $work/$1 checked out on main with one commit, pushing to
# a bare $work/$1.git that stands in for the operator's fork.
mkrepo() {
    _repo=$work/$1
    _origin=$work/$1.git
    git init --quiet --bare "$_origin"
    git init --quiet -b main "$_repo"
    # LF in, LF out. With autocrlf on — a Windows default — every exported
    # line would read as changed and case 9 could never fire.
    git -C "$_repo" config core.autocrlf false
    git -C "$_repo" config user.email fleet@example.invalid
    git -C "$_repo" config user.name "fleet test"
    git -C "$_repo" config commit.gpgsign false
    printf 'seed\n' >"$_repo/README.md"
    git -C "$_repo" add README.md
    git -C "$_repo" commit --quiet -m "chore: seed"
    git -C "$_repo" remote add origin "$_origin"
    git -C "$_repo" push --quiet origin main
    git -C "$_repo" symbolic-ref refs/remotes/origin/HEAD refs/remotes/origin/main
}

# $1 = repo name, then VAR=value pairs for that run. The file-level
# EDDA_PROJECTION=off is dropped here and nowhere else, so cases 1-7 keep
# their guarantee. `cd || exit` is load-bearing: without it a failed cd would
# run a live projection against this repo.
run_hook() {
    _repo=$work/$1
    shift
    (
        cd "$_repo" || exit 1
        PATH="$work/pbin:$PATH"
        export PATH
        unset EDDA_PROJECTION
        env "$@" sh "$hook_abs" 999 --sha "$SHA" --body-file "$work/fixtures/no-line.md" 2>&1
    )
}

# ── case 8: projection exports, commits, pushes and opens the PR ─────
#
# EDDA_PROJECTION is left unset, which also pins the documented default in
# `${EDDA_PROJECTION:-on}`: off is a choice, not the resting state.

mkrepo proj-ok
GH_CALLS=$work/gh-8; GH_BODY=$work/gh-8.body
export GH_CALLS GH_BODY
: >"$GH_CALLS"
out=$(run_hook proj-ok) || fail "case 8: projection run exited non-zero: $out"
case $out in
    *"projection exported @"*) : ;;
    *) fail "case 8: projection did not run: $out" ;;
esac
pushed=$(git -C "$work/proj-ok.git" for-each-ref --format='%(refname:short)' refs/heads/ledger)
[ -n "$pushed" ] || fail "case 8: nothing pushed to origin"
subject=$(git -C "$work/proj-ok.git" log -1 --format=%s "$pushed")
case $subject in
    "chore(ledger): export decision projection @ "????-??-??T??:??:??Z) : ;;
    *) fail "case 8: wrong commit subject: $subject" ;;
esac
git -C "$work/proj-ok.git" ls-tree -r --name-only "$pushed" \
    | grep -q '^docs/decisions/INDEX.md$' || fail "case 8: the mirror is not in the commit"
grep -qF "pr create --base main --head $pushed --title $subject --body-file -" "$GH_CALLS" \
    || fail "case 8: wrong gh args: $(cat "$GH_CALLS")"
grep -q 'edda export md' "$GH_BODY" || fail "case 8: PR body lost: $(cat "$GH_BODY")"
# The projection commit reaches main through the PR, never directly (a).
if git -C "$work/proj-ok" ls-tree -r --name-only main | grep -q '^docs/'; then
    fail "case 8: the mirror landed on main without a PR"
fi
# The operator gets their branch and a clean tree back.
now=$(git -C "$work/proj-ok" rev-parse --abbrev-ref HEAD)
[ "$now" = main ] || fail "case 8: left the checkout on '$now'"
[ -z "$(git -C "$work/proj-ok" status --porcelain)" ] \
    || fail "case 8: left the tree dirty: $(git -C "$work/proj-ok" status --porcelain)"

# ── case 9: only the stamp moved → no branch, no commit, no PR ───────
#
# The seeded mirror is what the stub exports, aged by one stamp — comparing
# the stamp would open a chore commit after every merge, forever (b).

mkrepo proj-noop
(
    cd "$work/proj-noop" || exit 1
    PATH="$work/pbin:$PATH"
    export PATH
    edda export md --out docs/decisions
) || fail "case 9: could not seed the mirror"
sed 's/^- \*\*Exported at\*\*: .*/- **Exported at**: 2000-01-01T00:00:00Z/' \
    "$work/proj-noop/docs/decisions/INDEX.md" >"$work/aged-index.md"
cp "$work/aged-index.md" "$work/proj-noop/docs/decisions/INDEX.md"
git -C "$work/proj-noop" add docs/decisions
git -C "$work/proj-noop" commit --quiet -m "chore(ledger): seed the mirror"

GH_CALLS=$work/gh-9; GH_BODY=$work/gh-9.body
export GH_CALLS GH_BODY
: >"$GH_CALLS"
out=$(run_hook proj-noop EDDA_PROJECTION=on) || fail "case 9: exited non-zero: $out"
case $out in
    *"projection unchanged"*) : ;;
    *) fail "case 9: expected a no-op, got: $out" ;;
esac
[ ! -s "$GH_CALLS" ] || fail "case 9: opened a PR for a stamp-only export"
[ -z "$(git -C "$work/proj-noop.git" for-each-ref --format='%(refname:short)' refs/heads/ledger)" ] \
    || fail "case 9: pushed a branch for a stamp-only export"
commits=$(git -C "$work/proj-noop" rev-list --count main)
[ "$commits" -eq 2 ] || fail "case 9: main gained a commit (now $commits)"
[ -z "$(git -C "$work/proj-noop" status --porcelain)" ] \
    || fail "case 9: left the tree dirty: $(git -C "$work/proj-noop" status --porcelain)"

# ── case 10: a projection that fails after branching restores the branch ──
#
# `git add` is the only step between the branch switch and the commit, so
# failing it is that whole error path in one command. Until the trap owned
# the checkout, this left the operator on ledger/projection-<ts>.

mkrepo proj-fail
GH_CALLS=$work/gh-10; GH_BODY=$work/gh-10.body
export GH_CALLS GH_BODY
: >"$GH_CALLS"
set +e
out=$(run_hook proj-fail EDDA_PROJECTION=on GIT_FAIL_SUBCOMMAND=add)
code=$?
set -e
now=$(git -C "$work/proj-fail" rev-parse --abbrev-ref HEAD)
[ "$now" = main ] || fail "case 10: a failed projection left the checkout on '$now'"
[ -z "$(git -C "$work/proj-fail" status --porcelain)" ] \
    || fail "case 10: a failed projection left the tree dirty: $(git -C "$work/proj-fail" status --porcelain)"
[ "$code" -eq 0 ] || fail "case 10: a failed projection must stay fail-soft, got exit $code: $out"
[ ! -s "$GH_CALLS" ] || fail "case 10: opened a PR despite a failed stage"

# ── case 11: a half-written export does not stall the next run ───────
#
# `git checkout --` cannot remove untracked files, so a failed export used to
# park the tree in a state the clean-tree guard refuses on every later run.

mkrepo proj-partial
GH_CALLS=$work/gh-11; GH_BODY=$work/gh-11.body
export GH_CALLS GH_BODY
: >"$GH_CALLS"
out=$(run_hook proj-partial EDDA_PROJECTION=on EDDA_EXPORT_PARTIAL=1) \
    || fail "case 11: a failed export must stay fail-soft: $out"
case $out in
    *"edda export md failed"*) : ;;
    *) fail "case 11: the export failure was not reported: $out" ;;
esac
[ -z "$(git -C "$work/proj-partial" status --porcelain)" ] \
    || fail "case 11: a half-written export was left in the tree: $(git -C "$work/proj-partial" status --porcelain)"
out=$(run_hook proj-partial EDDA_PROJECTION=on) || fail "case 11: second run exited non-zero: $out"
case $out in
    *"projection exported @"*) : ;;
    *) fail "case 11: the next run stalled instead of exporting: $out" ;;
esac


# ── case 12: a decision EDITED inside an already-tracked domain exports ──
#
# The case the whole projection exists for, and the one the change detector
# used to miss. `render_domain` writes a decision as `- **Field**:` bullets, so
# editing one yields a diff of nothing but bullet lines — which the old
# `grep -v '^[+-][+-]'` discarded along with the `+++`/`---` headers it was
# aiming at. Result: `changed` empty, `added` empty (the file is tracked), and
# a silent "unchanged" forever. Case 9 cannot see this: it pins the no-op, and
# a filter that discards everything produces a no-op too.

mkrepo proj-edit
(
    cd "$work/proj-edit" || exit 1
    PATH="$work/pbin:$PATH"
    export PATH
    edda export md --out docs/decisions
) || fail "case 12: could not seed the mirror"
git -C "$work/proj-edit" add docs/decisions
git -C "$work/proj-edit" commit --quiet -m "chore(ledger): seed the mirror"

GH_CALLS=$work/gh-12; GH_BODY=$work/gh-12.body
export GH_CALLS GH_BODY
: >"$GH_CALLS"
out=$(run_hook proj-edit EDDA_PROJECTION=on EDDA_EXPORT_VALUE=ruling-was-revised) \
    || fail "case 12: exited non-zero: $out"
case $out in
    *"projection unchanged"*)
        fail "case 12: an edited decision was reported unchanged: $out" ;;
esac
[ -s "$GH_CALLS" ] || fail "case 12: no PR opened for an edited decision: $out"
branch=$(git -C "$work/proj-edit.git" for-each-ref --format='%(refname:short)' refs/heads/ledger)
[ -n "$branch" ] || fail "case 12: nothing pushed for an edited decision"
# The pushed branch is where the revised value has to be. The operator's own
# checkout is deliberately NOT: cleanup() puts it back on main, so the file in
# the working tree holds the seeded value again — which is what the clean-tree
# assertion below pins.
git -C "$work/proj-edit.git" show "$branch:docs/decisions/decisions/ledger.md" \
    | grep -q 'ruling-was-revised' \
    || fail "case 12: the pushed branch does not carry the revised value"
[ -z "$(git -C "$work/proj-edit" status --porcelain)" ] \
    || fail "case 12: left the tree dirty: $(git -C "$work/proj-edit" status --porcelain)"
[ "$(git -C "$work/proj-edit" rev-parse --abbrev-ref HEAD)" = "main" ] \
    || fail "case 12: left the checkout off main"

EDDA_PROJECTION=off; export EDDA_PROJECTION

echo "PASS: $0"
