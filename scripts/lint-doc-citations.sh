#!/usr/bin/env bash
# GH-794. Zero added dependencies: bash + awk + git and shell utilities.
set -euo pipefail
mode=${1:---tree}
case "$mode" in --tree|--staged) ;; *) echo "usage: $0 --tree|--staged" >&2; exit 2;; esac
helper=$(cd "$(dirname "$0")" && pwd)/lint-doc-citations.awk
cd "$(git rev-parse --show-toplevel)"
tmp=$(mktemp -d)
trap 'rm -rf -- "$tmp"' EXIT
# Target syntax is ASCII without whitespace. Git quoting cannot turn any
# other filename into a matching target. Citing paths are passed NUL-safely.
git ls-files --format='%(objectmode) %(path)' > "$tmp/tracked"
git ls-files -z --format='./%(path)' -- '*.md' 'crates/*.rs' > "$tmp/docs"
# Do not follow a tracked or working-tree documentation symlink outside the
# repository. Target symlinks are excluded by the mode manifest as well.
git ls-files -z --format='%(objectmode) %(path)' -- '*.md' 'crates/*.rs' > "$tmp/doc-modes"
while IFS= read -r -d '' entry; do
    case "$entry" in 100644\ *|100755\ *) ;; *) echo "non-regular citing file: $entry" >&2; exit 1;; esac
    if [ "$mode" = --tree ] && [ -L "${entry:7}" ]; then
        echo "symlink citing file: ${entry:7}" >&2; exit 1
    fi
done < "$tmp/doc-modes"
if [ "$mode" = --staged ]; then
    # One coherent index snapshot, including unchanged documents: source-only
    # changes must be checked and unstaged repairs must not mask stale refs.
    # checkout-index fails closed on conflicts.
    git checkout-index --all --prefix="$tmp/tree/"
    cd "$tmp/tree"
fi
xargs -0 awk -v tracked_file="$tmp/tracked" -f "$helper" < "$tmp/docs"
