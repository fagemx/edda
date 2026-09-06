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
hooks_src=$(pwd)/scripts/githooks
lefthook_yml=$(pwd)/lefthook.yml

sh -n "$guard" || {
    echo "FAIL: sh -n $guard" >&2
    exit 1
}
sh -n "$hooks_src/pre-push" || {
    echo "FAIL: sh -n $hooks_src/pre-push" >&2
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
# GH_PR_BRANCH, when set, makes the answer branch-specific the way the real
# gh is. Without it a guard that looks up the WRONG branch still gets an
# answer, so a case turning on WHICH branch was asked about cannot fail.
head=
prev=
for a in "$@"; do
    [ "$prev" = "--head" ] && head=$a
    prev=$a
done
if [ -n "${GH_PR_BRANCH:-}" ] && [ "$head" != "$GH_PR_BRANCH" ]; then
    exit 0
fi
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

lstag() {
    git ls-remote "$repo_url" "refs/tags/$1" >"$work/lstag.out"
    cut -f1 "$work/lstag.out"
}

lsremote() {
    # no pipeline on the lookup: a failed git ls-remote must kill the test
    # under set -e, not surface as an empty value that satisfies an assertion
    git ls-remote "$repo_url" "refs/heads/$1" >"$work/lsremote.out"
    cut -f1 "$work/lsremote.out"
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

# Case 7: the first push of a NEW tag is accepted — a zero remote_sha leaves
# the loop before any namespace or PR check.
git -C "$clone" tag reltest "$(git -C "$clone" rev-parse HEAD~1)"
if (cd "$clone" && git push -q origin reltest) 2>"$work/err"; then
    [ -n "$(lstag reltest)" ] || fail 7 "remote tag missing after push"
else
    fail 7 "new tag push refused: $(cat "$work/err")"
fi
echo "PASS 7 new tag push accepted"

# Case 8 (GH-957): re-pointing that tag is refused. The guard used to strip
# refs/heads/ off the LOCAL ref and hand "refs/tags/reltest" to
# `gh pr list --head`, which matches nothing, so the push sailed through — and
# re-pointing a release tag silently changes what an already-shipped version
# resolves to (install.sh reads tags).
before=$(lstag reltest)
git -C "$clone" tag -f reltest "$(git -C "$clone" rev-parse HEAD)" >/dev/null
if (cd "$clone" && git push --force origin reltest) >"$work/out" 2>"$work/err"; then
    fail 8 "moving an existing tag was accepted"
fi
[ "$(lstag reltest)" = "$before" ] || fail 8 "remote tag moved despite refusal"
grep -q "refs/tags/reltest" "$work/err" ||
    fail 8 "stderr does not name the tag ref: $(cat "$work/err")"
grep -q "FLEET_ALLOW_FORCE_PUSH=1" "$work/err" ||
    fail 8 "stderr does not name the escape: $(cat "$work/err")"
echo "PASS 8 moving an existing tag refused"

# Case 9 (GH-957): the open-PR lookup must read the REMOTE ref. A PR head IS
# the remote branch, so `git push origin scratch:main` rewrites PR #906 while
# the local ref says "scratch". The branch-aware gh stub answers only for
# main, so a guard asking about the local ref gets no PR and waves it through.
git -C "$clone" branch -f scratch HEAD
before=$(lsremote main)
GH_PR_NUMBER=906
GH_PR_BRANCH=main
export GH_PR_NUMBER GH_PR_BRANCH
if (cd "$clone" && git push --force origin scratch:main) >"$work/out" 2>"$work/err"; then
    fail 9 "non-fast-forward push of a differently named local ref over an open PR head was accepted"
fi
[ "$(lsremote main)" = "$before" ] || fail 9 "remote ref changed despite refusal"
grep -q "#906" "$work/err" || fail 9 "stderr does not name the PR: $(cat "$work/err")"
grep -q "of main " "$work/err" ||
    fail 9 "refusal names the local ref, not the remote branch: $(cat "$work/err")"
echo "PASS 9 open-PR lookup reads the remote ref"

# Case 10 (GH-957): the git-native installer must wire pre-push. CONTRIBUTING
# points clones at scripts/githooks/install.sh, which set core.hooksPath for
# pre-commit and commit-msg only — so a clone installed that way pushed with
# no guard at all while a lefthook clone had one. Proven through the INSTALLED
# path: the hand-written .git/hooks/pre-push is removed first, so any refusal
# below can only come from core.hooksPath.
rm -f "$clone/.git/hooks/pre-push"
mkdir -p "$clone/scripts/fleet"
cp -R "$hooks_src" "$clone/scripts/githooks"
cp "$guard" "$clone/scripts/fleet/guard-push.sh"
(cd "$clone" && sh scripts/githooks/install.sh) >"$work/out" 2>&1 ||
    fail 10 "githooks/install.sh failed: $(cat "$work/out")"
[ "$(git -C "$clone" config core.hooksPath)" = "scripts/githooks" ] ||
    fail 10 "install.sh did not set core.hooksPath"
[ -f "$clone/scripts/githooks/pre-push" ] ||
    fail 10 "scripts/githooks has no pre-push hook to install"
grep -q "pre-push" "$work/out" ||
    fail 10 "install.sh output does not mention the pre-push hook: $(cat "$work/out")"
before=$(lsremote main)
if (cd "$clone" && git push --force origin main) >"$work/out" 2>"$work/err"; then
    fail 10 "the installed pre-push hook accepted a non-fast-forward push over an open PR"
fi
[ "$(lsremote main)" = "$before" ] || fail 10 "remote ref changed despite refusal"
grep -q "#906" "$work/err" ||
    fail 10 "installed-hook refusal does not name the PR: $(cat "$work/err")"
git -C "$clone" config --unset core.hooksPath
echo "PASS 10 git-native install.sh wires the pre-push guard"

# Case 11 (GH-957): the lefthook layer. Cases 1-10 drive a native git hook,
# which proves the guard but never that lefthook DELIVERS the stdin records to
# it — drop `use_stdin: true` and the guard reads an empty stdin, runs zero
# iterations and accepts every push, with every case above still green.
#   (a) always: the repo's own lefthook.yml still routes pre-push through the
#       guard with use_stdin;
#   (b) only where a lefthook binary exists: drive the same refusal through
#       real lefthook. lefthook is absent from the lane hosts, so (b) announces
#       a SKIP rather than passing silently.
prepush_block=$(awk '/^pre-push:/{f=1;next} f && /^[^[:space:]]/{f=0} f' "$lefthook_yml")
printf '%s' "$prepush_block" | grep -q 'guard-push.sh' ||
    fail 11 "lefthook.yml pre-push no longer runs scripts/fleet/guard-push.sh"
printf '%s' "$prepush_block" | grep -q 'use_stdin: *true' ||
    fail 11 "lefthook.yml lost use_stdin: true — the guard would read an empty stdin and accept every push"
if command -v lefthook >/dev/null 2>&1; then
    cat >"$clone/lefthook.yml" <<YAML
pre-push:
  commands:
    guard-push:
      run: sh "$guard"
      use_stdin: true
YAML
    (cd "$clone" && lefthook install) >"$work/out" 2>&1 ||
        fail 11 "lefthook install failed: $(cat "$work/out")"
    before=$(lsremote main)
    if (cd "$clone" && git push --force origin main) >"$work/out" 2>"$work/err"; then
        fail 11 "the lefthook layer accepted a non-fast-forward push over an open PR"
    fi
    [ "$(lsremote main)" = "$before" ] || fail 11 "remote ref changed despite the lefthook refusal"
    grep -q "#906" "$work/err" ||
        fail 11 "lefthook refusal does not name the PR, so the records were not delivered: $(cat "$work/err")"
    echo "PASS 11 lefthook use_stdin delivers the pre-push records to the guard"
else
    echo "SKIP 11 lefthook stdin contract: no lefthook binary on PATH (lefthook.yml contract asserted above; run this on a host that has it)"
fi
unset GH_PR_NUMBER GH_PR_BRANCH

echo "ALL PASS"
