#!/bin/sh
# Offline pass-through test for scripts/fleet/reclaim-merged.sh (GH-1093).
#
# The reclaimer's verdict ladder moved into the typed product verb
# `edda fleet reclaim` (crates/edda-cli/src/cmd_fleet_reclaim.rs), so what is
# left in the shell is a one-line adapter (`mechanism.shell-role=
# one-line-adapter-only`). Its whole contract is forwarding: whatever argv the
# caller passed arrives at the verb unchanged, in order, and the verb's exit
# status becomes the adapter's. The fixture matrix that used to live here —
# dry-run classification, R7 exclusions, the merged-clean reclamation, the
# live-peer protection — is now the Rust integration test
# crates/edda-cli/tests/reclaim_merged.rs, because the `fleet-tests` CI job
# does not build the Rust binary and a fixture that needs it cannot run there.
#
# Fully offline, POSIX sh only. A stub `edda` first on PATH records the argv of
# every invocation and answers with a chosen exit status; no `gh`, no `jq`, no
# network, no real repository.
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

# ── the stub verb ────────────────────────────────────────────────────
#
# It records one line per invocation and exits with EDDA_STUB_RC (default 0).
# PATH is prefixed so the adapter's `exec edda …` resolves here, not to any
# real binary on the workstation.

mkdir -p "$work/bin"
cat >"$work/bin/edda" <<'STUB'
#!/bin/sh
printf '%s\n' "$*" >>"$EDDA_CALLS"
printf '%s\n' "$#" >>"$EDDA_ARGC"
exit "${EDDA_STUB_RC:-0}"
STUB
chmod +x "$work/bin/edda"
PATH="$work/bin:$PATH"
export PATH

calls=$work/calls
argc=$work/argc
EDDA_CALLS=$calls
EDDA_ARGC=$argc
EDDA_STUB_RC=0
export EDDA_CALLS EDDA_ARGC EDDA_STUB_RC

# ── harness ──────────────────────────────────────────────────────────

fail() {
    echo "FAIL: $1" >&2
    [ ! -f "$calls" ] || { echo '--- recorded ---' >&2; cat "$calls" >&2; }
    [ ! -f "$work/err" ] || { echo '--- stderr ---' >&2; cat "$work/err" >&2; }
    exit 1
}

run() {
    : >"$calls"
    : >"$argc"
    set +e
    sh "$script" "$@" >"$work/out" 2>"$work/err"
    code=$?
    set -e
}

# The full recorded transcript, exactly — one line per invocation, so a missing
# or duplicated call fails too. A multi-line expectation is compared whole.
expect_calls() { # expected label
    got=$(cat "$calls")
    [ "$got" = "$1" ] || fail "$2: recorded '$got', expected '$1'"
    [ "$(cat "$argc")" = "$3" ] || fail "$2: verb received $(cat "$argc") args, expected $3"
}

# ── case 1: representative argv forwards verbatim, in order ──────────

run --apply --protect foo --protect 'bar baz' --pr-limit 7
[ "$code" -eq 0 ] || fail "case 1: adapter exited $code"
expect_calls 'fleet reclaim --apply --protect foo --protect bar baz --pr-limit 7' 'case 1' 9
[ "$(wc -l <"$calls")" -eq 1 ] || fail "case 1: expected exactly one invocation"

# ── case 2: no arguments forwards the bare verb ──────────────────────

run
[ "$code" -eq 0 ] || fail "case 2: adapter exited $code"
expect_calls 'fleet reclaim' 'case 2' 2

# ── case 3: the verb's non-zero status becomes the adapter's ─────────
#
# The adapter uses `exec`, so the process is replaced and the status must
# survive unchanged. Exit 3 is the PR-table-unreadable code the verb uses.

EDDA_STUB_RC=3
export EDDA_STUB_RC
run --apply
[ "$code" -eq 3 ] || fail "case 3: adapter exited $code, expected 3"
expect_calls 'fleet reclaim --apply' 'case 3' 3

echo "PASS scripts/fleet/test-reclaim-merged.sh"
