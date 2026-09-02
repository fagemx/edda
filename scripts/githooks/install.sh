#!/bin/sh
# Install git-native hooks (core.hooksPath). POSIX sh; idempotent.
set -eu
cd "$(git rev-parse --show-toplevel)"
chmod +x scripts/githooks/pre-commit scripts/githooks/commit-msg scripts/githooks/install.sh
git config core.hooksPath scripts/githooks
echo "githooks installed: core.hooksPath = scripts/githooks"
echo "uninstall: git config --unset core.hooksPath"
