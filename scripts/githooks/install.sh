#!/bin/sh
# Enable edda's git-native hooks (pre-commit, commit-msg, pre-push) via
# core.hooksPath.
# Zero external dependencies: no lefthook, no npm, nothing to download.
# commit-msg and pre-push are POSIX sh; pre-commit is bash (present on Git
# Bash and Linux).
#
# Run once per clone / worktree:
#     sh scripts/githooks/install.sh
# Verify:
#     git config core.hooksPath
# Bypass everything:
#     git commit --no-verify
# Skip clippy only (message gets a [skip-clippy] tag):
#     SKIP_CLIPPY=1 git commit ...

set -eu

git rev-parse --show-toplevel >/dev/null 2>&1 || {
    echo "install.sh: not inside a git repository" >&2
    exit 1
}

# Linux runs hook files directly, so they need the exec bit (also recorded
# in the git index; this covers checkouts that lost it).
dir=$(dirname "$0")
chmod +x "$dir/pre-commit" "$dir/commit-msg" "$dir/pre-push" 2>/dev/null || true

# A relative hooksPath is resolved by git against the top of the working
# tree, so it works from any subdirectory and survives repo moves.
git config core.hooksPath scripts/githooks

echo "installed: core.hooksPath=scripts/githooks"
echo "  pre-commit : 1 MB cap; cargo fmt (staged *.rs / Cargo.*);"
echo "               cargo clippy (touched crates/* only); markdown lint (staged *.md)"
echo "  commit-msg : <type>(<scope>): <description>; merge + wip( pass;"
echo "               [skip-clippy] tagging for SKIP_CLIPPY=1"
echo "  pre-push   : scripts/fleet/guard-push.sh — refuses a non-fast-forward"
echo "               push over an open PR head, and any update to a ref that"
echo "               already exists outside refs/heads/ (GH-957)"
echo "runtime   : pre-commit needs bash (Git Bash and Linux both have it);"
echo "            commit-msg and pre-push are POSIX sh"
echo "bypass: git commit --no-verify skips the commit-time hooks; CI runs on"
echo "        pull requests and pushes to main, so a feature branch is still"
echo "        gated through its PR's CI Gate."
echo "        git push --no-verify skips the push guard, and CI does NOT cover"
echo "        that: it cannot restore a reviewed head already overwritten."
echo "        The guard's own one-push escape is FLEET_ALLOW_FORCE_PUSH=1"
