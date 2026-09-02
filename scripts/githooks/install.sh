#!/bin/sh
# Enable edda's git-native hooks (pre-commit, commit-msg) via core.hooksPath.
# Zero external dependencies: no lefthook, no npm, nothing to download.
# commit-msg is POSIX sh; pre-commit is bash (present on Git Bash and Linux).
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
chmod +x "$dir/pre-commit" "$dir/commit-msg" 2>/dev/null || true

# A relative hooksPath is resolved by git against the top of the working
# tree, so it works from any subdirectory and survives repo moves.
git config core.hooksPath scripts/githooks

echo "installed: core.hooksPath=scripts/githooks"
echo "  pre-commit : 1 MB cap; cargo fmt (staged *.rs / Cargo.*);"
echo "               cargo clippy (touched crates/* only); markdown lint (staged *.md)"
echo "  commit-msg : <type>(<scope>): <description>; merge + wip( pass;"
echo "               [skip-clippy] tagging for SKIP_CLIPPY=1"
echo "runtime   : pre-commit needs bash (Git Bash and Linux both have it);"
echo "            commit-msg is POSIX sh"
echo "bypass: git commit --no-verify — CI runs on pull requests and pushes to main;"
echo "        a feature branch is gated through its PR's CI Gate"
