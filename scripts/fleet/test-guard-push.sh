#!/bin/sh
# Self-test for scripts/fleet/guard-push.sh (GH-913). Installs the guard as
# the pre-push hook of a throwaway clone and drives real `git push` commands
# against a local bare origin, so every case exercises the same stdin record
# a lefthook run would deliver. A stub `gh` earlier on PATH controls the
# open-PR answer per case. Nothing is written outside the mktemp -d sandbox:
# not the real repo, not the real remote, not $HOME.
set -eu

cd "$(git rev-parse --show-toplevel)"
guard=$(pwd)/scripts/fleet/guard-push.sh

sh -n "$guard" || {
    echo "FAIL: sh -n $guard" >&2
    exit 1
}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

mkdir -p "$work/bin"
cat > "$work/bin/gh" <<'STUB'
#!/bin/sh
if [ -n "${GH_FAIL:-}" ]; then
    echo "gh: stub failure" >&2
    exit 1
fi
# Stands in for `gh pr list --head <branch> --state open --json number
# --jq '.[0].number'`: prints the first open PR number, or nothing.
printf '%s\n' "${GH_PR_NUMBER:-}"
exit 0
STUB
chmod +x "$work/bin/gh"
PATH="$work/bin:$PATH"
export PATH

git init --bare -q -b main "$work/origin.git"
repo_url="$work/origin.git"
git clone -q "$repo_url" "$work/clone"
clone=$work/clone
git -C "$clone" config user.email guard-test@example.com
git -C "$clone" config user.name guard-test

# The clone's pre-push hook execs the real guard with git's own stdin record.
mkdir -p "$clone/.git/hooks"
printf '#!/bin/sh\nexec sh "%s" "$@"\n' "$guard" >"$clone/.git/hooks/pre-push"
chmod +x "$clone/.git/hooks/pre-push"

lsremote() {
    git ls-remote "$repo_url" "refs/heads/$1" | cut -f1
}

fail() {
    echo "FAIL $1: $2" >&2
    exit 1
}

echo one >"$clone/file.txt"
git -C "$clone" add file.txt
git -C "$clone" commit -qm one
git -C "$clone" push -q -u origin main 2>"$work/err"

# Case 1: fast-forward push accepted, remote ref moved.
before=$(lsremote main)
echo two >"$clone/file.txt"
git -C "$clone" commit -aqm two
if (cd "$clone" && git push -q origin main) 2>"$work/err"; then
    after=$(lsremote main)
    [ -n "$after" ] || fail 1 "remote ref vanished"
    [ "$before" != "$after" ] || fail 1 "remote ref unchanged after fast-forward push"
    [ "$after" = "$(git -C "$clone" rev-parse HEAD)" ] ||
        fail 1 "remote ref is not the pushed head"
else
    fail 1 "fast-forward push refused: $(cat "$work/err")"
fi
echo "PASS 1 fast-forward push accepted"

# Case 2: amend then a non-fast-forward push on a branch with an open PR —
# refused, stderr names the PR and both SHAs, remote ref byte-identical.
before=$(lsremote main)
git -C "$clone" commit --amend -qm two-amended
local_sha=$(git -C "$clone" rev-parse HEAD)
GH_PR_NUMBER=906
export GH_PR_NUMBER
if (cd "$clone" && git push --force-with-lease origin main) >"$work/out" 2>"$work/err"; then
    fail 2 "non-fast-forward push over an open PR was accepted"
fi
after=$(lsremote main)
[ "$after" = "$before" ] || fail 2 "remote ref changed despite refusal"
grep -q '#906' "$work/err" || fail 2 "stderr does not name the PR: $(cat "$work/err")"
grep -q "$before" "$work/err" || fail 2 "stderr does not name the remote SHA"
grep -q "$local_sha" "$work/err" || fail 2 "stderr does not name the local SHA"
grep -q 'FLEET_ALLOW_FORCE_PUSH=1' "$work/err" ||
    fail 2 "stderr does not name the escape: $(cat "$work/err")"
unset GH_PR_NUMBER
echo "PASS 2 non-fast-forward push with open PR refused"

# Case 3: the same rewrite with FLEET_ALLOW_FORCE_PUSH=1 is accepted.
before=$(lsremote main)
if (cd "$clone" && FLEET_ALLOW_FORCE_PUSH=1 git push --force-with-lease origin main) 2>"$work/err"; then
    after=$(lsremote main)
    [ "$before" != "$after" ] || fail 3 "remote ref unchanged after authorized rewrite"
else
    fail 3 "authorized rewrite refused: $(cat "$work/err")"
fi
echo "PASS 3 authorized force push accepted"

# Case 4: first push of a new branch accepted.
git -C "$clone" branch b2
if (cd "$clone" && git push -q origin b2) 2>"$work/err"; then
    [ -n "$(lsremote b2)" ] || fail 4 "remote branch missing after push"
else
    fail 4 "new-branch push refused: $(cat "$work/err")"
fi
echo "PASS 4 new branch push accepted"

# Case 5: branch deletion accepted.
if (cd "$clone" && git push -q origin :b2) 2>"$work/err"; then
    [ -z "$(lsremote b2)" ] || fail 5 "remote branch still present after deletion"
else
    fail 5 "branch deletion refused: $(cat "$work/err")"
fi
echo "PASS 5 branch deletion accepted"

# Case 6: non-fast-forward while the PR lookup cannot run — refused, fail closed.
before=$(lsremote main)
git -C "$clone" commit --amend -qm two-amended-again
GH_FAIL=1
export GH_FAIL
if (cd "$clone" && git push --force-with-lease origin main) >"$work/out" 2>"$work/err"; then
    fail 6 "push accepted although the open-PR check could not run"
fi
after=$(lsremote main)
[ "$after" = "$before" ] || fail 6 "remote ref changed despite fail-closed refusal"
grep -q 'failing closed' "$work/err" ||
    fail 6 "stderr does not say it failed closed: $(cat "$work/err")"
unset GH_FAIL
echo "PASS 6 failed open-PR check refused (fail closed)"

echo "ALL PASS"
