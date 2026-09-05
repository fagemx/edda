#!/bin/sh
# guard-push.sh — lefthook pre-push gate against force-pushes over reviewed
# heads (GH-913). Reads lefthook's pre-push stdin, one record per line:
#   <local-ref> <local-sha> <remote-ref> <remote-sha>
# A fast-forward push, a new branch, or a branch deletion passes without any
# network call. A non-fast-forward push is refused when the branch has an
# open PR — and refused just the same when the open-PR lookup cannot run
# (fail closed): a guard that cannot check must not wave the push through.
# Single escape, deliberately loud: FLEET_ALLOW_FORCE_PUSH=1.
set -eu

if [ "${FLEET_ALLOW_FORCE_PUSH:-}" = "1" ]; then
    echo "guard-push: FLEET_ALLOW_FORCE_PUSH=1 — push accepted without checks; this rewrite is on your shell history" >&2
    exit 0
fi

refuse() {
    echo "guard-push: $1 Refused. An authorized rewrite is a deliberate act: set FLEET_ALLOW_FORCE_PUSH=1 for this one push." >&2
    exit 1
}

while read -r local_ref local_sha remote_ref remote_sha || [ -n "${local_ref:-}" ]; do
    [ -n "$local_ref" ] || continue
    case "$local_sha" in
        *[!0]*) : ;;
        *) continue ;;  # branch deletion: nothing can be destroyed
    esac
    case "$remote_sha" in
        *[!0]*) : ;;
        *) continue ;;  # new branch: no remote head to overwrite
    esac

    if git merge-base --is-ancestor "$remote_sha" "$local_sha" 2>/dev/null; then
        continue  # fast-forward: append-only, the property verdicts rely on
    fi

    branch=${local_ref#refs/heads/}
    if ! command -v gh >/dev/null 2>&1; then
        refuse "cannot check the open-PR state of $branch: gh is not on PATH, failing closed."
    fi
    pr=
    if ! pr=$(gh pr list --head "$branch" --state open --json number --jq '.[0].number' 2>/dev/null); then
        refuse "cannot check the open-PR state of $branch: gh pr list failed, failing closed."
    fi
    pr=$(printf '%s' "$pr" | tr -d '[:space:]')
    if [ -z "$pr" ] || [ "$pr" = "null" ]; then
        continue  # no open PR on this branch
    fi
    refuse "non-fast-forward push of $branch ($local_sha) would overwrite $remote_sha, the head of open PR #$pr."
done
exit 0
