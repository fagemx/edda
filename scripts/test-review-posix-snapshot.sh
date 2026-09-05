#!/bin/sh
# Focused GH-702 regression fixture for the generated Linux runner only.
set -eu
root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' 0 HUP INT TERM
tree="$tmp/scratch/wt-review-pr9998"
mkdir -p "$tmp/bin" "$tmp/fail-bin" "$tree"
git -C "$tree" init -q
git -C "$tree" config user.name fixture
git -C "$tree" config user.email fixture@example.invalid
printf clean > "$tree/tracked.txt"
printf 'ignored.txt\n' > "$tree/.gitignore"
git -C "$tree" add tracked.txt .gitignore
git -C "$tree" commit -qm fixture
export GH_HEAD=$(git -C "$tree" rev-parse HEAD)
export EDDA_FLEET_SCRATCH="$tmp/scratch" EDDA_FLEET_ROOT="$root" EDDA_REVIEW_SPEC="$root/REVIEW.md"
export REVIEW_TREE="$tree" CALLS="$tmp/calls" REAL_GIT="$(command -v git)"
cat > "$tmp/bin/gh" <<'EOF'
#!/bin/sh
case "$*" in
 *headRefOid*|*baseRefOid*) echo "$GH_HEAD" ;;
 *headRefName*|*baseRefName*) echo main ;;
 *title*) echo fixture ;;
 *body*) printf 'Issue: #702\n## doneWhen\nRestrict reviewers\n' ;;
 *--name-only*) echo scripts/review-pr.sh ;;
esac
EOF
cat > "$tmp/bin/uname" <<'EOF'
#!/bin/sh
echo Linux
EOF
cat > "$tmp/bin/claude" <<'EOF'
#!/bin/sh
if [ "$*" = --help ]; then echo '--tools <tools> --disallowedTools, --disallowed-tools <tools> --permission-mode <mode>'; fi
EOF
cat > "$tmp/bin/edda" <<'EOF'
#!/bin/sh
if [ "$*" = 'dispatch --help' ]; then
  [ "${MODE:-modern}" = refusal ] || echo '--tools <TOOLS> --exclude-tools <EXCLUDE_TOOLS> --permission-mode <MODE>'
  exit 0
fi
echo launch >> "$CALLS"
if [ "${MODE:-modern}" = mutate ]; then
  printf changed > "$REVIEW_TREE/tracked.txt"
  printf changed > "$REVIEW_TREE/$(printf 'odd\nname.txt')"
  printf changed > "$REVIEW_TREE/untracked.txt"
  printf changed > "$REVIEW_TREE/ignored.txt"
fi
EOF
cat > "$tmp/fail-bin/git" <<'EOF'
#!/bin/sh
if [ "$1" = ls-files ] && [ "${MODE:-}" = inventory-failure ]; then exit 7; fi
exec "$REAL_GIT" "$@"
EOF
chmod +x "$tmp/bin"/* "$tmp/fail-bin/git"
export PATH="$tmp/bin:$PATH"
sh "$root/scripts/review-pr.sh" 9998 --dry-run >/dev/null
runner="$tmp/scratch/review-pr9998-r1-run.sh"
sed '/git -C .* worktree remove /d' "$runner" > "$tmp/run.sh"
chmod +x "$tmp/run.sh"
head -1 "$tmp/run.sh" | grep -qx '#!/usr/bin/env bash'

assert_cleaned() {
  [ ! -e "$tmp/scratch/review-pr9998-r1.done.before-snapshot" ]
  [ ! -e "$tmp/scratch/review-pr9998-r1.done.after-snapshot" ]
}

# A newline path proves the generated Bash reader preserves Git's NUL stream.
printf dirty > "$tree/tracked.txt"
printf dirty > "$tree/$(printf 'odd\nname.txt')"
printf dirty > "$tree/untracked.txt"
printf dirty > "$tree/ignored.txt"
MODE=mutate; export MODE
: > "$CALLS"
if "$tmp/run.sh" >"$tmp/mutate.out" 2>&1; then exit 1; fi
grep -q '^WORKTREE_CHECK=failed' "$tmp/scratch/review-pr9998-r1.done"
[ -s "$CALLS" ]
assert_cleaned

# `git ls-files` failure must not be hidden by the reader pipeline.
MODE=inventory-failure; export MODE
PATH="$tmp/fail-bin:$PATH"; export PATH
: > "$CALLS"
if "$tmp/run.sh" >"$tmp/inventory.out" 2>&1; then exit 1; fi
grep -q '^DISPATCH_EXIT=2' "$tmp/scratch/review-pr9998-r1.done"
[ ! -s "$CALLS" ]
assert_cleaned

# Capability refusal happens after the pre-snapshot and must release it.
PATH=$(printf '%s' "$PATH" | sed "s|$tmp/fail-bin:||"); export PATH
MODE=refusal; export MODE
: > "$CALLS"
if "$tmp/run.sh" >"$tmp/refusal.out" 2>&1; then exit 1; fi
grep -q '^DISPATCH_EXIT=2' "$tmp/scratch/review-pr9998-r1.done"
[ ! -s "$CALLS" ]
assert_cleaned
echo 'generated POSIX snapshot fixture passed (newline path, inventory failure, refusal cleanup)'
